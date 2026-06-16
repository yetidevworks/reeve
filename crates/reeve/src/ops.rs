//! Shared server lifecycle operations, used by both the CLI and the TUI so the
//! two never drift. These functions mutate state and (re)render native configs;
//! they do not print — callers format their own feedback.

use crate::backends::{backend_for, server_service_id};
use crate::brew::Brew;
use crate::config::{load_config, save_config};
use crate::daemon::{self, Status};
use crate::php;
use crate::probe::{self, PortState};
use crate::services;
use crate::ssl;
use crate::state::{
    load_state, save_state, ManagedServiceInstance, PhpVersion as Php, Server, ServiceKind, Vhost,
    XdebugMode,
};
use anyhow::{anyhow, bail, Result};

/// Fetch a server from state by name, or error.
pub fn require_server(name: &str) -> Result<Server> {
    load_state()?
        .get_server(name)
        .cloned()
        .ok_or_else(|| anyhow!("Server '{name}' not found"))
}

/// The honest, port-aware state of a web server — what launchd *and* the
/// network agree on, so reeve never claims "running" for a server bound to
/// nothing or one whose port a foreign process has stolen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeState {
    /// Not loaded and nothing on its port.
    Stopped,
    /// Our launchd job is bound to its port and serving.
    Serving,
    /// launchd job is loaded but nothing is listening (empty config / bind
    /// failure) — the case that previously showed a misleading "running".
    LoadedNotBound,
    /// A process reeve doesn't manage holds one of the server's ports.
    PortConflict { port: u16, holder: String, pid: u32 },
    /// launchd reports the job crashed.
    Crashed,
}

impl ServeState {
    /// Short status string for the CLI/TUI status column.
    pub fn label(&self) -> String {
        match self {
            ServeState::Stopped => "stopped".to_string(),
            ServeState::Serving => "running".to_string(),
            ServeState::LoadedNotBound => "loaded, not bound".to_string(),
            ServeState::PortConflict { port, holder, .. } => {
                format!(":{port} held by {holder}")
            }
            ServeState::Crashed => "crashed".to_string(),
        }
    }
}

/// The TCP ports a server will actually bind: always HTTP, plus HTTPS when it
/// serves a default site or any SSL vhost.
fn ports_to_bind(server: &Server, state: &crate::state::State) -> Vec<u16> {
    let mut ports = vec![server.http_port];
    let needs_https = server.default_site
        || crate::park::effective_vhosts_for(state, &server.name)
            .iter()
            .any(|v| v.ssl);
    if needs_https {
        ports.push(server.https_port);
    }
    ports
}

/// Resolve a server's honest, port-aware state (see [`ServeState`]).
pub fn serve_state(server: &Server) -> ServeState {
    let id = server_service_id(server);
    let our = daemon::pid(&id);
    let state = load_state().unwrap_or_default();
    // A foreign holder on any port we'd bind is a conflict, whether or not our
    // own job happens to be loaded.
    if let Some((port, pid, holder)) = probe::first_foreign(&ports_to_bind(server, &state), our) {
        return ServeState::PortConflict { port, holder, pid };
    }
    match probe::classify_port(server.http_port, our) {
        PortState::OursBound => ServeState::Serving,
        PortState::Foreign { pid, name } => ServeState::PortConflict {
            port: server.http_port,
            holder: name,
            pid,
        },
        PortState::Free => match daemon::status(&id) {
            Status::Running => ServeState::LoadedNotBound,
            Status::Error => ServeState::Crashed,
            Status::Stopped => ServeState::Stopped,
        },
    }
}

/// Outcome of starting a server/service: the resulting launchd status, plus an
/// optional note when reeve first had to stop a conflicting `brew services`
/// instance of the same software to free the port.
pub struct Started {
    pub status: Status,
    pub handoff: Option<String>,
}

/// When a port reeve needs is held by a foreign process, and Homebrew is
/// running its own copy of `formula` as a `brew services` job, stop that job so
/// reeve can take over. Returns a note when it stopped one. Best-effort — a
/// manually-launched (non-brew) process is left for the caller to report.
fn reclaim_from_brew(brew: &Brew, formula: &str) -> Option<String> {
    if !brew.service_started(formula) {
        return None;
    }
    brew.stop_service(formula).ok().map(|()| {
        format!("stopped Homebrew's own '{formula}' (brew services) so reeve can manage it")
    })
}

/// Poll briefly for `ports` to come free after stopping a conflicting job — the
/// LISTEN socket usually goes the instant the process dies, but `brew services
/// stop` returns slightly before launchd has fully reaped it.
fn wait_port_release(ports: &[u16], our: Option<u32>) {
    for _ in 0..20 {
        if probe::first_foreign(ports, our).is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Clear the way for a server to bind: if a foreign process holds one of its
/// ports, try to hand off from a `brew services` copy of the same backend;
/// error only if an unmanaged process still holds the port afterward. Returns a
/// note describing any handoff.
fn preflight_ports(server: &Server) -> Result<Option<String>> {
    let id = server_service_id(server);
    let our = daemon::pid(&id);
    let state = load_state().unwrap_or_default();
    let ports = ports_to_bind(server, &state);
    if probe::first_foreign(&ports, our).is_none() {
        return Ok(None);
    }
    let mut note = None;
    if let Ok(brew) = Brew::detect() {
        note = reclaim_from_brew(&brew, backend_for(server.backend).formula());
        if note.is_some() {
            wait_port_release(&ports, our);
        }
    }
    if let Some((port, pid, name)) = probe::first_foreign(&ports, our) {
        bail!(
            "Port {port} is already in use by '{name}' (pid {pid}), which reeve doesn't manage. \
             Stop it (e.g. `brew services stop {name}`) or change this server's port, then retry."
        );
    }
    Ok(note)
}

/// Same handoff as [`preflight_ports`], for a managed service's single port.
fn preflight_service(kind: ServiceKind) -> Result<Option<String>> {
    let port = services::port(kind);
    let our = daemon::pid(&services::service_id(kind));
    if probe::first_foreign(&[port], our).is_none() {
        return Ok(None);
    }
    let mut note = None;
    if let Ok(brew) = Brew::detect() {
        note = reclaim_from_brew(&brew, services::formula(kind));
        if note.is_some() {
            wait_port_release(&[port], our);
        }
    }
    if let Some((port, pid, name)) = probe::first_foreign(&[port], our) {
        bail!(
            "Port {port} is already in use by '{name}' (pid {pid}), which reeve doesn't manage. \
             Stop it (e.g. `brew services stop {name}`), then retry."
        );
    }
    Ok(note)
}

/// Flip a server's enabled flag and persist.
pub fn set_enabled(name: &str, enabled: bool) -> Result<()> {
    let mut state = load_state()?;
    if let Some(s) = state.servers.iter_mut().find(|s| s.name == name) {
        s.enabled = enabled;
    }
    save_state(&state)
}

/// Mint certificates for any SSL vhosts that don't have one yet.
pub fn ensure_vhost_certs(vhosts: &[&Vhost], brew: &Brew) -> Result<()> {
    for v in vhosts {
        if v.ssl && !ssl::exists(&v.server_name) {
            ssl::mint(brew, &v.server_name)?;
        }
    }
    Ok(())
}

/// Render + validate one server's native config from current state. Includes
/// any parked subfolders pointed at this server (Valet-style).
pub fn render_server(server: &Server) -> Result<()> {
    let brew = Brew::detect()?;
    let cfg = load_config()?;
    let state = load_state()?;
    let backend = backend_for(server.backend);
    backend.ensure_installed(&brew)?;
    let vhosts_owned = crate::park::effective_vhosts_for(&state, &server.name);
    let vhosts: Vec<&Vhost> = vhosts_owned.iter().collect();
    ensure_vhost_certs(&vhosts, &brew)?;
    // The default site's HTTPS catch-all needs a `localhost` cert.
    if server.default_site && !ssl::exists(crate::backends::DEFAULT_SITE_HOST) {
        ssl::mint(&brew, crate::backends::DEFAULT_SITE_HOST)?;
    }
    backend.render(server, &vhosts, &state, &cfg, &brew)?;
    backend.validate(server, &brew)?;
    Ok(())
}

/// Render, install the launchd service, and (re)start a server. Marks enabled.
pub fn start_server(name: &str) -> Result<Started> {
    let server = require_server(name)?;
    let handoff = preflight_ports(&server)?;
    render_server(&server)?;
    let brew = Brew::detect()?;
    let backend = backend_for(server.backend);
    let spec = backend.service_spec(&server, &brew)?;
    daemon::install(&spec)?;
    daemon::restart(&server_service_id(&server))?;
    set_enabled(name, true)?;
    Ok(Started {
        status: daemon::status(&server_service_id(&server)),
        handoff,
    })
}

/// Stop a server and mark it disabled.
pub fn stop_server(name: &str) -> Result<()> {
    let server = require_server(name)?;
    daemon::unload(&server_service_id(&server))?;
    set_enabled(name, false)?;
    Ok(())
}

/// Install (or adopt) a PHP version, stand up its FPM master, register it in
/// state, and make it the default if none is set yet. Slow (brew).
pub fn install_php(version: &str) -> Result<()> {
    let brew = Brew::detect()?;
    let record = php::install(&brew, version)?;
    let mut state = load_state()?;
    state.php_versions.retain(|p| p.version != version);
    state.php_versions.push(record);
    state.sort_php();
    save_state(&state)?;
    let mut cfg = load_config()?;
    if cfg.default_php.is_none() {
        cfg.default_php = Some(version.to_string());
        save_config(&cfg)?;
    }
    Ok(())
}

/// Stop and unmanage a PHP version. Refuses if a vhost still uses it. Leaves the
/// brew formula installed (so it can be re-adopted); only stops the FPM master
/// and drops it from state.
pub fn remove_php(version: &str) -> Result<()> {
    let mut state = load_state()?;
    let users: Vec<String> = state
        .vhosts
        .iter()
        .filter(|v| v.php_version == version)
        .map(|v| v.server_name.clone())
        .collect();
    if !users.is_empty() {
        bail!("PHP {version} is used by: {}", users.join(", "));
    }
    daemon::uninstall(&php::service_id(version)).ok();
    state.php_versions.retain(|p| p.version != version);
    save_state(&state)?;
    let mut cfg = load_config()?;
    if cfg.default_php.as_deref() == Some(version) {
        cfg.default_php = state.php_versions.first().map(|p| p.version.clone());
        save_config(&cfg)?;
    }
    Ok(())
}

/// Restart a version's FPM master, applying its persisted php.ini / OPcache /
/// pool / Xdebug settings. Falls back to defaults if the version isn't yet in
/// state (e.g. mid-install).
pub fn restart_fpm(version: &str) -> Result<()> {
    let brew = Brew::detect()?;
    let state = load_state()?;
    let record = state.get_php(version).cloned().unwrap_or_else(|| Php {
        version: version.to_string(),
        fpm_socket: php::fpm_socket(version)
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        ..Default::default()
    });
    php::ensure_fpm_running(&brew, &record)
}

/// Stop a version's FPM master (unload its launchd job; leaves it installed and
/// in state, so `apply`/restart brings it back). Note any vhost on this version
/// will 502 until it's restarted.
pub fn stop_fpm(version: &str) -> Result<()> {
    daemon::unload(&php::service_id(version))
}

/// Set one php.ini / OPcache / FPM setting for a version, persist, and restart
/// the FPM master so it takes effect.
pub fn set_php_setting(version: &str, key: &str, value: &str) -> Result<()> {
    if !php::php_settings_defs().iter().any(|d| d.key == key) {
        bail!("Unknown PHP setting '{key}'. See `reeve php settings {version}`.");
    }
    let mut state = load_state()?;
    let rec = state
        .php_versions
        .iter_mut()
        .find(|p| p.version == version)
        .ok_or_else(|| anyhow!("PHP {version} is not managed"))?;
    rec.settings.insert(key.to_string(), value.to_string());
    let record = rec.clone();
    save_state(&state)?;
    let brew = Brew::detect()?;
    php::ensure_fpm_running(&brew, &record)
}

/// Set Xdebug mode for a version, install Xdebug via pecl if enabling and it's
/// missing, persist, and restart the FPM master. Slow when an install is needed.
pub fn set_xdebug(version: &str, mode: XdebugMode) -> Result<()> {
    let brew = Brew::detect()?;
    if !mode.is_off() && !php::extensions::is_loaded(&brew, version, "xdebug")? {
        php::extensions::add(&brew, version, "xdebug")?;
    }
    let mut state = load_state()?;
    let php_rec = state
        .php_versions
        .iter_mut()
        .find(|p| p.version == version)
        .ok_or_else(|| anyhow!("PHP {version} is not managed"))?;
    php_rec.xdebug = mode;
    let record = php_rec.clone();
    save_state(&state)?;
    php::ensure_fpm_running(&brew, &record)
}

/// Add a managed service to state (does not start it). Idempotent.
pub fn add_service(kind: ServiceKind) -> Result<()> {
    let mut state = load_state()?;
    if state.get_service(kind).is_none() {
        state.services.push(ManagedServiceInstance {
            kind,
            enabled: false,
        });
        save_state(&state)?;
    }
    Ok(())
}

/// Ensure a service is installed, stand up its launchd master, and mark it
/// enabled. Registers it in state first if absent. Slow when a brew install
/// is needed.
pub fn start_service(kind: ServiceKind) -> Result<Started> {
    let brew = Brew::detect()?;
    services::ensure_installed(&brew, kind)?;
    // A leftover `brew services` copy (e.g. `brew services start redis`) would
    // hold the port and make reeve's foreground master crash-loop; hand off.
    let handoff = preflight_service(kind)?;
    let spec = services::service_spec(&brew, kind)?;
    daemon::install(&spec)?;
    daemon::restart(&services::service_id(kind))?;
    let mut state = load_state()?;
    match state.services.iter_mut().find(|s| s.kind == kind) {
        Some(s) => s.enabled = true,
        None => state.services.push(ManagedServiceInstance {
            kind,
            enabled: true,
        }),
    }
    save_state(&state)?;
    Ok(Started {
        status: daemon::status(&services::service_id(kind)),
        handoff,
    })
}

/// Stop a service's launchd master and mark it disabled.
pub fn stop_service(kind: ServiceKind) -> Result<()> {
    daemon::unload(&services::service_id(kind))?;
    let mut state = load_state()?;
    if let Some(s) = state.services.iter_mut().find(|s| s.kind == kind) {
        s.enabled = false;
    }
    save_state(&state)
}

/// Restart a service's launchd master (re-installs the spec first).
pub fn restart_service(kind: ServiceKind) -> Result<Status> {
    let brew = Brew::detect()?;
    preflight_service(kind)?;
    let spec = services::service_spec(&brew, kind)?;
    daemon::install(&spec)?;
    daemon::restart(&services::service_id(kind))?;
    Ok(daemon::status(&services::service_id(kind)))
}

/// Stop a service and drop it from state. Leaves the brew formula installed.
pub fn remove_service(kind: ServiceKind) -> Result<()> {
    daemon::uninstall(&services::service_id(kind)).ok();
    let mut state = load_state()?;
    state.services.retain(|s| s.kind != kind);
    save_state(&state)
}

/// Park a directory (validates server + PHP + dir), replacing any existing park
/// for the same root. Returns the number of sites it currently expands to.
pub fn add_park(root: &str, server: &str, php: &str, tld: &str, ssl: bool) -> Result<usize> {
    if !std::path::Path::new(root).is_dir() {
        bail!("'{root}' is not a directory");
    }
    let mut state = load_state()?;
    if state.get_server(server).is_none() {
        bail!("Server '{server}' does not exist");
    }
    if state.get_php(php).is_none() {
        bail!("PHP {php} is not installed. Run `reeve php install {php}`.");
    }
    let park = crate::state::Park {
        root: root.to_string(),
        server: server.to_string(),
        php_version: php.to_string(),
        tld: tld.to_string(),
        ssl,
    };
    let n = crate::park::expand(&park).len();
    state.parks.retain(|p| p.root != root);
    state.parks.push(park);
    save_state(&state)?;
    Ok(n)
}

/// Stop parking a directory. Errors if it wasn't parked.
pub fn remove_park(root: &str) -> Result<()> {
    let mut state = load_state()?;
    let before = state.parks.len();
    state.parks.retain(|p| p.root != root);
    if state.parks.len() == before {
        bail!("'{root}' is not parked");
    }
    save_state(&state)
}

/// Set the default PHP version for new vhosts.
pub fn set_default_php(version: &str) -> Result<()> {
    let mut cfg = load_config()?;
    cfg.default_php = Some(version.to_string());
    save_config(&cfg)
}

/// Re-render and restart a running server (picks up config changes).
pub fn restart_server(name: &str) -> Result<Status> {
    let server = require_server(name)?;
    // Reclaim the port from a brew-services copy if one is squatting on it; the
    // handoff note isn't surfaced on restart (start is the primary path).
    preflight_ports(&server)?;
    render_server(&server)?;
    let brew = Brew::detect()?;
    let backend = backend_for(server.backend);
    let spec = backend.service_spec(&server, &brew)?;
    daemon::install(&spec)?;
    daemon::restart(&server_service_id(&server))?;
    Ok(daemon::status(&server_service_id(&server)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_state_labels() {
        assert_eq!(ServeState::Serving.label(), "running");
        assert_eq!(ServeState::Stopped.label(), "stopped");
        assert_eq!(ServeState::LoadedNotBound.label(), "loaded, not bound");
        assert_eq!(ServeState::Crashed.label(), "crashed");
        assert_eq!(
            ServeState::PortConflict {
                port: 80,
                holder: "httpd".into(),
                pid: 42,
            }
            .label(),
            ":80 held by httpd"
        );
    }
}
