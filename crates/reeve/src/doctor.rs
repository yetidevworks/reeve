//! Stack diagnostics. Aggregates the health of every moving part — Homebrew,
//! web servers, FPM masters, DNS resolvers, the mkcert CA, and port usage —
//! into a flat list of pass/warn/fail checks shared by the `doctor` CLI command
//! and (as a one-line summary) the TUI.

use crate::backends::backend_for;
use crate::brew::Brew;
use crate::config::Config;
use crate::daemon::{self, Status};
use crate::dns;
use crate::ops::ServeState;
use crate::php;
use crate::services;
use crate::ssl;
use crate::state::State;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    Warn,
    Fail,
}

impl Health {
    pub fn symbol(&self) -> &'static str {
        match self {
            Health::Ok => "✓",
            Health::Warn => "⚠",
            Health::Fail => "✗",
        }
    }
}

/// A single diagnostic line.
pub struct Check {
    pub name: String,
    pub health: Health,
    pub detail: String,
}

impl Check {
    fn new(name: impl Into<String>, health: Health, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            health,
            detail: detail.into(),
        }
    }
}

/// Run every diagnostic and return the results in display order. `brew` is
/// optional because Homebrew detection can itself fail (the first check).
pub fn run(brew: Option<&Brew>, cfg: &Config, state: &State) -> Vec<Check> {
    let mut checks = Vec::new();

    // Homebrew.
    match brew {
        Some(b) => checks.push(Check::new(
            "Homebrew",
            Health::Ok,
            b.prefix.display().to_string(),
        )),
        None => {
            checks.push(Check::new(
                "Homebrew",
                Health::Fail,
                "not found — install from https://brew.sh",
            ));
        }
    }

    // Port conflicts among enabled servers.
    for conflict in port_conflicts(state) {
        checks.push(Check::new("Ports", Health::Fail, conflict));
    }

    // Web servers: formula installed + launchd status.
    for s in &state.servers {
        let backend = backend_for(s.backend);
        if let Some(b) = brew {
            if !b.is_installed(backend.formula()) {
                checks.push(Check::new(
                    format!("server {}", s.name),
                    Health::Warn,
                    format!(
                        "{} not installed (`reeve apply` will install it)",
                        backend.formula()
                    ),
                ));
                continue;
            }
        }
        // Honest, port-aware status: launchd "running" is not enough — the
        // process must actually be bound, and no foreign process may hold the
        // port (checked even for disabled servers, to catch e.g. a stray
        // `brew services` httpd sitting on :80).
        let (health, detail) = match crate::ops::serve_state(s) {
            ServeState::Serving => {
                let ports = crate::ops::serving_ports(s);
                let list = if ports.is_empty() {
                    format!(":{}/:{}", s.http_port, s.https_port)
                } else {
                    ports
                        .iter()
                        .map(|p| format!(":{p}"))
                        .collect::<Vec<_>>()
                        .join("/")
                };
                (Health::Ok, format!("running on {list}"))
            }
            ServeState::Stopped => (Health::Ok, "stopped".to_string()),
            ServeState::LoadedNotBound => (
                Health::Fail,
                format!(
                    "loaded but not listening — empty config or failed bind (`reeve logs server-{}`)",
                    s.name
                ),
            ),
            ServeState::PortConflict { port, holder, pid } => (
                Health::Fail,
                format!("port {port} held by '{holder}' (pid {pid}) — not managed by reeve"),
            ),
            ServeState::Crashed => {
                (Health::Fail, "crashed — check `reeve logs`".to_string())
            }
        };
        checks.push(Check::new(format!("server {}", s.name), health, detail));
    }

    // PHP: formula installed + FPM socket alive.
    for p in &state.php_versions {
        if let Some(b) = brew {
            if !php::is_installed(b, &p.version) {
                checks.push(Check::new(
                    format!("php {}", p.version),
                    Health::Fail,
                    format!(
                        "{} missing — run `reeve php install {}`",
                        php::formula(&p.version),
                        p.version
                    ),
                ));
                continue;
            }
        }
        let socket = std::path::Path::new(&p.fpm_socket);
        if daemon::socket_alive(socket) {
            checks.push(Check::new(
                format!("php {}", p.version),
                Health::Ok,
                "FPM socket live".to_string(),
            ));
        } else {
            checks.push(Check::new(
                format!("php {}", p.version),
                Health::Warn,
                "FPM socket not present — master stopped?".to_string(),
            ));
        }
    }

    // CLI php shim: only reported once the user has opted in by setting one.
    if let Some(cli_ver) = php::current_cli_php() {
        let on_path = brew.map(php::shim_on_path).unwrap_or(false);
        if on_path {
            checks.push(Check::new(
                "cli php",
                Health::Ok,
                format!("{cli_ver} (~/.reeve/bin on PATH)"),
            ));
        } else {
            checks.push(Check::new(
                "cli php",
                Health::Warn,
                format!(
                    "shim set to {cli_ver} but ~/.reeve/bin isn't ahead of Homebrew on PATH — add it to use it"
                ),
            ));
        }
    }

    // Managed services: formula installed + launchd status + port reachable.
    for svc in &state.services {
        let id = services::service_id(svc.kind);
        if let Some(b) = brew {
            if !services::is_installed(b, svc.kind) {
                checks.push(Check::new(
                    format!("service {}", svc.kind),
                    Health::Warn,
                    format!(
                        "{} not installed (`reeve service start {}` installs it)",
                        services::formula(svc.kind),
                        svc.kind
                    ),
                ));
                continue;
            }
        }
        if svc.enabled {
            let (health, detail) = match daemon::status(&id) {
                Status::Running if services::health(svc.kind) => (
                    Health::Ok,
                    format!("running, port {} open", services::port(svc.kind)),
                ),
                Status::Running => (
                    Health::Warn,
                    format!("up but port {} not answering yet", services::port(svc.kind)),
                ),
                Status::Stopped => (Health::Warn, "enabled but not running".to_string()),
                Status::Error => (Health::Fail, "crashed — check `reeve logs`".to_string()),
            };
            checks.push(Check::new(format!("service {}", svc.kind), health, detail));
        } else {
            checks.push(Check::new(
                format!("service {}", svc.kind),
                Health::Ok,
                "stopped".to_string(),
            ));
        }
    }

    // DNS resolvers, one per configured TLD.
    let dnsmasq = daemon::status(dns::service_id());
    for tld in &cfg.local_tlds {
        if dns::resolver_ok(tld) && dnsmasq == Status::Running {
            checks.push(Check::new(
                format!("dns .{tld}"),
                Health::Ok,
                "resolving → 127.0.0.1".to_string(),
            ));
        } else if !dns::resolver_ready(tld) {
            checks.push(Check::new(
                format!("dns .{tld}"),
                Health::Warn,
                "no system resolver entry — run `reeve dns setup`".to_string(),
            ));
        } else {
            checks.push(Check::new(
                format!("dns .{tld}"),
                Health::Warn,
                format!("resolver present but dnsmasq is {}", dnsmasq.as_str()),
            ));
        }
    }

    // mkcert CA: generated on disk AND actually in the trust store.
    let generated = brew
        .and_then(|b| ssl::ca_cert(b).ok())
        .map(|p| p.exists())
        .unwrap_or(false);
    let trusted = brew.map(ssl::is_trusted).unwrap_or(false);
    match (generated, trusted) {
        (_, true) => checks.push(Check::new(
            "SSL CA",
            Health::Ok,
            "mkcert root trusted (local HTTPS works)".to_string(),
        )),
        (true, false) => checks.push(Check::new(
            "SSL CA",
            Health::Warn,
            "generated but not trusted — run `reeve ssl trust` (TUI: T)".to_string(),
        )),
        (false, false) => checks.push(Check::new(
            "SSL CA",
            Health::Warn,
            "not installed — run `reeve ssl trust`".to_string(),
        )),
    }

    #[cfg(target_os = "linux")]
    linux_checks(&mut checks, state);

    checks
}

/// Linux-only host checks: user session bus reachability, login lingering (boot
/// persistence), and the unprivileged-low-port sysctl when any enabled server
/// is configured on a port below 1024.
#[cfg(target_os = "linux")]
fn linux_checks(checks: &mut Vec<Check>, state: &State) {
    if daemon::user_bus_ok() {
        checks.push(Check::new(
            "systemd user bus",
            Health::Ok,
            "`systemctl --user` responding".to_string(),
        ));
    } else {
        checks.push(Check::new(
            "systemd user bus",
            Health::Fail,
            "`systemctl --user` not responding — no user session bus (needed to run services)"
                .to_string(),
        ));
    }

    if daemon::linger_enabled() {
        checks.push(Check::new(
            "login lingering",
            Health::Ok,
            "enabled — services persist across logout / reboot".to_string(),
        ));
    } else {
        checks.push(Check::new(
            "login lingering",
            Health::Warn,
            "off — services stop at logout; run `loginctl enable-linger` (reeve init offers this)"
                .to_string(),
        ));
    }

    let low_ports: Vec<u16> = state
        .servers
        .iter()
        .filter(|s| s.enabled)
        .flat_map(|s| [s.http_port, s.https_port])
        .filter(|&p| p < 1024)
        .collect();
    if !low_ports.is_empty() && !privileged_ports_allowed() {
        checks.push(Check::new(
            "privileged ports",
            Health::Fail,
            "servers use ports < 1024 but unprivileged binding isn't allowed — \
             `reeve init` offers the sysctl fix"
                .to_string(),
        ));
    }
}

/// Current `net.ipv4.ip_unprivileged_port_start`; low enough to bind :80?
#[cfg(target_os = "linux")]
fn privileged_ports_allowed() -> bool {
    std::fs::read_to_string("/proc/sys/net/ipv4/ip_unprivileged_port_start")
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .map(|start| start <= 80)
        .unwrap_or(false)
}

/// Duplicate http/https ports across enabled servers. Mirrors the guard in
/// `State::add_server` but reports every clashing pair for the report.
pub fn port_conflicts(state: &State) -> Vec<String> {
    let mut seen: HashMap<u16, String> = HashMap::new();
    let mut out = Vec::new();
    for s in state.servers.iter().filter(|s| s.enabled) {
        for port in [s.http_port, s.https_port] {
            if let Some(other) = seen.get(&port) {
                if other != &s.name {
                    out.push(format!(
                        "port {port} used by both '{other}' and '{}'",
                        s.name
                    ));
                }
            } else {
                seen.insert(port, s.name.clone());
            }
        }
    }
    out
}

/// Worst health across all checks (for a one-line summary).
pub fn summary(checks: &[Check]) -> Health {
    if checks.iter().any(|c| c.health == Health::Fail) {
        Health::Fail
    } else if checks.iter().any(|c| c.health == Health::Warn) {
        Health::Warn
    } else {
        Health::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Backend, Server};

    fn srv(name: &str, http: u16, https: u16, enabled: bool) -> Server {
        Server {
            name: name.into(),
            backend: Backend::Caddy,
            http_port: http,
            https_port: https,
            enabled,
            default_site: false,
            default_preset: Default::default(),
            default_root: None,
            settings: Default::default(),
        }
    }

    #[test]
    fn detects_port_conflicts_only_for_enabled() {
        let mut state = State::default();
        state.servers.push(srv("a", 80, 443, true));
        state.servers.push(srv("b", 80, 9443, true));
        state.servers.push(srv("c", 8080, 443, false)); // disabled — ignored
        let conflicts = port_conflicts(&state);
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].contains("port 80"));
    }
}
