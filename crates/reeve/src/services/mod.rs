//! Managed background services (databases, cache, mail) driven via Homebrew +
//! launchd. reeve adopts each formula's default datadir/config and owns only
//! the launchd lifecycle, logs, and health checks — mirroring how `backends`
//! wraps web servers. Each service runs in the FOREGROUND so launchd's
//! `KeepAlive` supervises it directly (no `brew services`).

use crate::brew::Brew;
use crate::daemon::ServiceSpec;
use crate::paths;
use crate::state::ServiceKind;
use anyhow::Result;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Static description of how to run one service against brew's defaults.
struct ServiceDef {
    formula: &'static str,
    /// Non-core tap that provides the formula, if any.
    tap: Option<&'static str>,
    /// Binary path relative to `opt/<formula>/`, e.g. `bin/redis-server`.
    bin: &'static str,
    /// TCP port reeve probes for health and shows in listings.
    port: u16,
    /// One-line description for listings.
    blurb: &'static str,
}

fn def(kind: ServiceKind) -> ServiceDef {
    match kind {
        ServiceKind::Mysql => ServiceDef {
            formula: "mysql",
            tap: None,
            bin: "bin/mysqld",
            port: 3306,
            blurb: "MySQL database",
        },
        ServiceKind::Mariadb => ServiceDef {
            formula: "mariadb",
            tap: None,
            bin: "bin/mariadbd",
            port: 3306,
            blurb: "MariaDB database",
        },
        ServiceKind::Postgres => ServiceDef {
            formula: "postgresql@16",
            tap: None,
            bin: "bin/postgres",
            port: 5432,
            blurb: "PostgreSQL 16 database",
        },
        ServiceKind::Redis => ServiceDef {
            formula: "redis",
            tap: None,
            bin: "bin/redis-server",
            port: 6379,
            blurb: "Redis key-value store",
        },
        ServiceKind::Memcached => ServiceDef {
            formula: "memcached",
            tap: None,
            bin: "bin/memcached",
            port: 11211,
            blurb: "memcached cache",
        },
        ServiceKind::Mailpit => ServiceDef {
            formula: "mailpit",
            tap: None,
            bin: "bin/mailpit",
            port: 8025,
            blurb: "Mailpit mail catcher (SMTP :1025, UI :8025)",
        },
    }
}

/// Foreground launch arguments, pointed at brew's default datadir/config.
fn args(kind: ServiceKind, brew: &Brew) -> Vec<String> {
    let p = |sub: &str| brew.prefix.join(sub).display().to_string();
    match kind {
        ServiceKind::Mysql | ServiceKind::Mariadb => {
            vec![format!("--datadir={}", p("var/mysql"))]
        }
        ServiceKind::Postgres => vec!["-D".into(), p("var/postgresql@16")],
        // The conf already sets `daemonize no`; pass it again to be explicit.
        ServiceKind::Redis => vec![p("etc/redis.conf"), "--daemonize".into(), "no".into()],
        ServiceKind::Memcached => vec!["-p".into(), "11211".into()],
        // Mailpit's defaults (SMTP :1025, UI :8025) are what we want.
        ServiceKind::Mailpit => vec![],
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

/// The TCP port a service listens on (health probe + listings).
pub fn port(kind: ServiceKind) -> u16 {
    def(kind).port
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
pub fn service_spec(brew: &Brew, kind: ServiceKind) -> Result<ServiceSpec> {
    let d = def(kind);
    Ok(ServiceSpec {
        service: service_id(kind),
        program: brew.opt(d.formula).join(d.bin),
        args: args(kind, brew),
        log: paths::logs_dir()?.join(format!("{}.log", service_id(kind))),
        keep_alive: true,
        run_at_load: true,
    })
}

/// Cheap health probe: can we open a TCP connection to the service's port?
pub fn health(kind: ServiceKind) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], def(kind).port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}
