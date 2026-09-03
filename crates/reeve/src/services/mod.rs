//! Managed background services (databases, cache, mail) driven via Homebrew +
//! launchd. reeve adopts each formula's default datadir/config and owns only
//! the launchd lifecycle, logs, health checks, and listening ports — mirroring
//! how `backends` wraps web servers. Each service runs in the FOREGROUND so
//! launchd's `KeepAlive` supervises it directly (no `brew services`).

use crate::brew::Brew;
use crate::daemon::ServiceSpec;
use crate::paths;
use crate::state::{ManagedServiceInstance, ServiceKind};
use anyhow::{bail, Result};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// One configurable listening port for a service. Defaults match what the brew
/// formula binds on its own, so a service nobody has re-pointed behaves exactly
/// as it did before ports became configurable.
pub struct PortDef {
    /// Key stored under the service's `ports` table in `state.toml`.
    pub key: &'static str,
    /// Field label in the TUI / `reeve service ports`.
    pub label: &'static str,
    pub default: u16,
    /// One-line hint shown beside the field.
    pub help: &'static str,
    /// The port reeve health-probes and lists as *the* port for the service.
    pub primary: bool,
}

/// Static description of how to run one service against brew's defaults.
struct ServiceDef {
    formula: &'static str,
    /// Non-core tap that provides the formula, if any.
    tap: Option<&'static str>,
    /// Binary path relative to `opt/<formula>/`, e.g. `bin/redis-server`.
    bin: &'static str,
    /// Ports the service listens on and reeve can re-point.
    ports: &'static [PortDef],
    /// One-line description for listings.
    blurb: &'static str,
}

/// The single-port shape most services have.
const fn one_port(default: u16, help: &'static str) -> [PortDef; 1] {
    [PortDef {
        key: "port",
        label: "Port",
        default,
        help,
        primary: true,
    }]
}

const MYSQL_PORTS: [PortDef; 1] = one_port(3306, "client connections");
const POSTGRES_PORTS: [PortDef; 1] = one_port(5432, "client connections");
const REDIS_PORTS: [PortDef; 1] = one_port(6379, "client connections");
const MEMCACHED_PORTS: [PortDef; 1] = one_port(11211, "client connections");
const MAILPIT_PORTS: [PortDef; 2] = [
    PortDef {
        key: "smtp",
        label: "SMTP port",
        default: 1025,
        help: "what apps send mail to",
        primary: false,
    },
    PortDef {
        key: "ui",
        label: "Web UI port",
        default: 8025,
        help: "web UI + API",
        primary: true,
    },
];

fn def(kind: ServiceKind) -> ServiceDef {
    match kind {
        ServiceKind::Mysql => ServiceDef {
            formula: "mysql",
            tap: None,
            bin: "bin/mysqld",
            ports: &MYSQL_PORTS,
            blurb: "MySQL database",
        },
        ServiceKind::Mariadb => ServiceDef {
            formula: "mariadb",
            tap: None,
            bin: "bin/mariadbd",
            ports: &MYSQL_PORTS,
            blurb: "MariaDB database",
        },
        ServiceKind::Postgres => ServiceDef {
            formula: "postgresql@16",
            tap: None,
            bin: "bin/postgres",
            ports: &POSTGRES_PORTS,
            blurb: "PostgreSQL 16 database",
        },
        ServiceKind::Redis => ServiceDef {
            formula: "redis",
            tap: None,
            bin: "bin/redis-server",
            ports: &REDIS_PORTS,
            blurb: "Redis key-value store",
        },
        ServiceKind::Memcached => ServiceDef {
            formula: "memcached",
            tap: None,
            bin: "bin/memcached",
            ports: &MEMCACHED_PORTS,
            blurb: "memcached cache",
        },
        ServiceKind::Mailpit => ServiceDef {
            formula: "mailpit",
            tap: None,
            bin: "bin/mailpit",
            ports: &MAILPIT_PORTS,
            blurb: "Mailpit mail catcher (SMTP :1025, UI :8025)",
        },
    }
}

/// Every port a service kind listens on, with its default and label.
pub fn port_defs(kind: ServiceKind) -> &'static [PortDef] {
    def(kind).ports
}

/// The default port for one key (used when an instance has no override).
pub fn default_port(kind: ServiceKind, key: &str) -> Option<u16> {
    port_defs(kind)
        .iter()
        .find(|d| d.key == key)
        .map(|d| d.default)
}

/// An instance's configured port for `key`, falling back to the default.
pub fn port_of(inst: &ManagedServiceInstance, key: &str) -> u16 {
    let default = default_port(inst.kind, key).unwrap_or_default();
    inst.port(key, default)
}

/// Every port an instance actually listens on, in definition order.
pub fn ports(inst: &ManagedServiceInstance) -> Vec<u16> {
    port_defs(inst.kind)
        .iter()
        .map(|d| inst.port(d.key, d.default))
        .collect()
}

/// The port reeve probes for health and shows as the service's port.
pub fn primary_port(inst: &ManagedServiceInstance) -> u16 {
    let defs = port_defs(inst.kind);
    let d = defs.iter().find(|d| d.primary).unwrap_or(&defs[0]);
    inst.port(d.key, d.default)
}

/// Compact port list for tables, e.g. `3306` or `1025/8025`.
pub fn ports_display(inst: &ManagedServiceInstance) -> String {
    ports(inst)
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// Labelled port list for messages: `port 6379`, or `SMTP 1026, Web UI 8025`
/// for a service that binds more than one.
pub fn ports_summary(inst: &ManagedServiceInstance) -> String {
    let defs = port_defs(inst.kind);
    if defs.len() == 1 {
        return format!("port {}", inst.port(defs[0].key, defs[0].default));
    }
    defs.iter()
        .map(|d| {
            let label = d.label.trim_end_matches(" port");
            format!("{label} {}", inst.port(d.key, d.default))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reject a port key the service doesn't define, listing the ones it does.
pub fn check_port_key(kind: ServiceKind, key: &str) -> Result<()> {
    if port_defs(kind).iter().any(|d| d.key == key) {
        return Ok(());
    }
    let keys: Vec<&str> = port_defs(kind).iter().map(|d| d.key).collect();
    bail!("{kind} has no port '{key}'. Use {}.", keys.join("|"));
}

/// Foreground launch arguments, pointed at brew's default datadir/config and
/// this instance's configured ports.
fn args(inst: &ManagedServiceInstance, brew: &Brew) -> Vec<String> {
    let p = |sub: &str| brew.prefix.join(sub).display().to_string();
    let port = |key: &str| port_of(inst, key).to_string();
    match inst.kind {
        ServiceKind::Mysql | ServiceKind::Mariadb => {
            vec![
                format!("--datadir={}", p("var/mysql")),
                format!("--port={}", port("port")),
            ]
        }
        ServiceKind::Postgres => vec![
            "-D".into(),
            p("var/postgresql@16"),
            "-p".into(),
            port("port"),
        ],
        // The conf already sets `daemonize no`; pass it again to be explicit.
        // `--port` on the command line overrides the conf's own port.
        ServiceKind::Redis => vec![
            p("etc/redis.conf"),
            "--daemonize".into(),
            "no".into(),
            "--port".into(),
            port("port"),
        ],
        ServiceKind::Memcached => vec!["-p".into(), port("port")],
        // Mailpit binds every interface by default (`[::]:<port>`); keep that
        // and just point it at reeve's configured ports.
        ServiceKind::Mailpit => vec![
            "--smtp".into(),
            format!("[::]:{}", port("smtp")),
            "--listen".into(),
            format!("[::]:{}", port("ui")),
        ],
    }
}

/// launchd service id, e.g. `svc-mysql`.
pub fn service_id(kind: ServiceKind) -> String {
    format!("svc-{}", kind.as_str())
}

/// The Homebrew formula that provides a service.
pub fn formula(kind: ServiceKind) -> &'static str {
    def(kind).formula
}

/// One-line description for listings.
pub fn blurb(kind: ServiceKind) -> &'static str {
    def(kind).blurb
}

/// Is the service's brew formula installed?
pub fn is_installed(brew: &Brew, kind: ServiceKind) -> bool {
    brew.is_installed(def(kind).formula)
}

/// Ensure the formula (and its tap, if any) is installed.
pub fn ensure_installed(brew: &Brew, kind: ServiceKind) -> Result<()> {
    let d = def(kind);
    if let Some(tap) = d.tap {
        brew.ensure_tap(tap)?;
    }
    if !brew.is_installed(d.formula) {
        println!("Installing {}…", d.formula);
        brew.install(d.formula)?;
    }
    Ok(())
}

/// The launchd service spec that runs a service in the foreground.
pub fn service_spec(brew: &Brew, inst: &ManagedServiceInstance) -> Result<ServiceSpec> {
    let d = def(inst.kind);
    Ok(ServiceSpec {
        service: service_id(inst.kind),
        program: brew.opt(d.formula).join(d.bin),
        args: args(inst, brew),
        log: paths::logs_dir()?.join(format!("{}.log", service_id(inst.kind))),
        keep_alive: true,
        run_at_load: true,
    })
}

/// Cheap health probe: can we open a TCP connection to the service's port?
pub fn health(inst: &ManagedServiceInstance) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], primary_port(inst)));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(kind: ServiceKind) -> ManagedServiceInstance {
        ManagedServiceInstance::new(kind)
    }

    #[test]
    fn every_service_has_exactly_one_primary_port() {
        for kind in ServiceKind::all() {
            let defs = port_defs(kind);
            assert!(!defs.is_empty(), "{kind} defines no ports");
            assert_eq!(
                defs.iter().filter(|d| d.primary).count(),
                1,
                "{kind} must mark exactly one primary port"
            );
        }
    }

    #[test]
    fn ports_fall_back_to_defaults_until_overridden() {
        let mut m = inst(ServiceKind::Mailpit);
        assert_eq!(port_of(&m, "smtp"), 1025);
        assert_eq!(primary_port(&m), 8025);
        assert_eq!(ports_display(&m), "1025/8025");
        m.ports.insert("smtp".into(), 1026);
        assert_eq!(port_of(&m, "smtp"), 1026);
        assert_eq!(ports(&m), vec![1026, 8025]);
        assert_eq!(ports_summary(&m), "SMTP 1026, Web UI 8025");
    }

    #[test]
    fn single_port_services_summarize_bare() {
        let mut r = inst(ServiceKind::Redis);
        assert_eq!(ports_summary(&r), "port 6379");
        r.ports.insert("port".into(), 6380);
        assert_eq!(primary_port(&r), 6380);
        assert_eq!(ports_display(&r), "6380");
    }

    #[test]
    fn unknown_port_keys_are_rejected() {
        assert!(check_port_key(ServiceKind::Mailpit, "smtp").is_ok());
        assert!(check_port_key(ServiceKind::Mailpit, "ui").is_ok());
        let err = check_port_key(ServiceKind::Mailpit, "http").unwrap_err();
        assert!(err.to_string().contains("smtp|ui"), "{err}");
        assert!(check_port_key(ServiceKind::Redis, "smtp").is_err());
    }

    #[test]
    fn configured_ports_reach_the_launch_args() {
        let brew = Brew {
            prefix: std::path::PathBuf::from("/opt/homebrew"),
        };
        let mut m = inst(ServiceKind::Mailpit);
        m.ports.insert("smtp".into(), 1026);
        let a = args(&m, &brew);
        assert!(a.contains(&"[::]:1026".to_string()), "{a:?}");
        assert!(a.contains(&"[::]:8025".to_string()), "{a:?}");

        let mut db = inst(ServiceKind::Mysql);
        db.ports.insert("port".into(), 3307);
        assert!(args(&db, &brew).contains(&"--port=3307".to_string()));
    }
}
