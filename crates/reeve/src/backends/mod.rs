//! Web server backend abstraction. Each backend renders the same logical
//! inventory (servers + vhosts + per-vhost PHP) into its own native config
//! syntax, all sharing the PHP-FPM-over-socket proxy pattern.

use crate::brew::Brew;
use crate::config::Config;
use crate::daemon::ServiceSpec;
use crate::state::{Backend, Server, State, Vhost};
use anyhow::Result;

mod apache;
mod caddy;
mod nginx;
mod ols;

pub trait WebServerBackend {
    /// This backend's [`Backend`] id. Part of the trait API; not all callers
    /// use it yet.
    #[allow(dead_code)]
    fn id(&self) -> Backend;

    /// Homebrew formula that provides this server.
    fn formula(&self) -> &'static str;

    /// Ensure the backend's brew formula (and any tap) is installed.
    fn ensure_installed(&self, brew: &Brew) -> Result<()>;

    /// Render this server + its vhosts into `generated/<backend>/...`.
    fn render(
        &self,
        server: &Server,
        vhosts: &[&Vhost],
        state: &State,
        cfg: &Config,
        brew: &Brew,
    ) -> Result<()>;

    /// Run the backend's native config test (`caddy validate`, `nginx -t`, …).
    fn validate(&self, server: &Server, brew: &Brew) -> Result<()>;

    /// Graceful reload to pick up config changes. Part of the trait API;
    /// lifecycle currently goes through restart instead.
    #[allow(dead_code)]
    fn reload(&self, server: &Server, brew: &Brew) -> Result<()>;

    /// The launchd service that runs this server instance.
    fn service_spec(&self, server: &Server, brew: &Brew) -> Result<ServiceSpec>;
}

/// launchd service id for a server instance, e.g. "server-caddy".
pub fn server_service_id(server: &Server) -> String {
    format!("server-{}", server.name)
}

/// Hostname the default-site HTTPS catch-all presents a cert for.
pub const DEFAULT_SITE_HOST: &str = "localhost";

/// The `host[:port]` authority a plain-HTTP request to an `--ssl` vhost should
/// be redirected to. The port is omitted when HTTPS runs on the standard 443,
/// but reeve's alt servers use non-standard ports (caddy 1443, nginx 2443), so
/// it must be included there or the redirect would land on a dead 443.
pub fn https_authority(host: &str, https_port: u16) -> String {
    if https_port == 443 {
        host.to_string()
    } else {
        format!("{host}:{https_port}")
    }
}

/// FPM socket for the default-site catch-all: the configured default PHP
/// version, else the first installed version. `None` if no PHP is managed
/// (the default site then serves static files only).
pub fn default_php_socket(state: &State, cfg: &Config) -> Option<String> {
    let ver = cfg
        .default_php
        .clone()
        .or_else(|| state.php_versions.first().map(|p| p.version.clone()))?;
    state.get_php(&ver).map(|p| p.fpm_socket.clone())
}

/// A tunable setting a backend exposes in the TUI Settings modal.
pub struct SettingDef {
    pub key: &'static str,
    pub label: &'static str,
    pub default: &'static str,
    pub help: &'static str,
}

/// The settings each backend exposes. Each backend's `render` reads these keys
/// from `Server::setting(key, default)` and emits the matching native config.
pub fn settings_defs(backend: Backend) -> &'static [SettingDef] {
    match backend {
        Backend::Caddy => &[SettingDef {
            key: "max_body",
            label: "Max request body",
            default: "100MB",
            help: "e.g. 100MB, 1GB",
        }],
        Backend::Apache => &[
            SettingDef {
                key: "limit_request_body",
                label: "Max upload bytes",
                default: "0",
                help: "0 = unlimited",
            },
            SettingDef {
                key: "timeout",
                label: "Timeout (s)",
                default: "300",
                help: "request timeout",
            },
            SettingDef {
                key: "keepalive_timeout",
                label: "Keep-alive (s)",
                default: "5",
                help: "idle conn hold",
            },
            SettingDef {
                key: "max_keepalive_requests",
                label: "Max keep-alive reqs",
                default: "100",
                help: "0 = unlimited",
            },
        ],
        Backend::Nginx => &[
            SettingDef {
                key: "client_max_body_size",
                label: "Max upload size",
                default: "64m",
                help: "e.g. 64m, 1g",
            },
            SettingDef {
                key: "worker_connections",
                label: "Worker connections",
                default: "256",
                help: "per worker",
            },
        ],
        Backend::Ols => &[],
    }
}

/// Construct the backend implementation for a given [`Backend`].
pub fn backend_for(backend: Backend) -> Box<dyn WebServerBackend> {
    match backend {
        Backend::Caddy => Box::new(caddy::Caddy),
        Backend::Apache => Box::new(apache::Apache),
        Backend::Nginx => Box::new(nginx::Nginx),
        Backend::Ols => Box::new(ols::Ols),
    }
}
