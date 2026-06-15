//! Shared server lifecycle operations, used by both the CLI and the TUI so the
//! two never drift. These functions mutate state and (re)render native configs;
//! they do not print — callers format their own feedback.

use crate::backends::{backend_for, server_service_id};
use crate::brew::Brew;
use crate::config::{load_config, save_config};
use crate::daemon::{self, Status};
use crate::php;
use crate::ssl;
use crate::state::{load_state, save_state, Server, Vhost};
use anyhow::{anyhow, bail, Result};

/// Fetch a server from state by name, or error.
pub fn require_server(name: &str) -> Result<Server> {
    load_state()?
        .get_server(name)
        .cloned()
        .ok_or_else(|| anyhow!("Server '{name}' not found"))
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

/// Render + validate one server's native config from current state.
pub fn render_server(server: &Server) -> Result<()> {
    let brew = Brew::detect()?;
    let cfg = load_config()?;
    let state = load_state()?;
    let backend = backend_for(server.backend);
    backend.ensure_installed(&brew)?;
    let vhosts = state.vhosts_for(&server.name);
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
pub fn start_server(name: &str) -> Result<Status> {
    let server = require_server(name)?;
    render_server(&server)?;
    let brew = Brew::detect()?;
    let backend = backend_for(server.backend);
    let spec = backend.service_spec(&server, &brew)?;
    daemon::install(&spec)?;
    daemon::restart(&server_service_id(&server))?;
    set_enabled(name, true)?;
    Ok(daemon::status(&server_service_id(&server)))
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

/// Set the default PHP version for new vhosts.
pub fn set_default_php(version: &str) -> Result<()> {
    let mut cfg = load_config()?;
    cfg.default_php = Some(version.to_string());
    save_config(&cfg)
}

/// Re-render and restart a running server (picks up config changes).
pub fn restart_server(name: &str) -> Result<Status> {
    let server = require_server(name)?;
    render_server(&server)?;
    let brew = Brew::detect()?;
    let backend = backend_for(server.backend);
    let spec = backend.service_spec(&server, &brew)?;
    daemon::install(&spec)?;
    daemon::restart(&server_service_id(&server))?;
    Ok(daemon::status(&server_service_id(&server)))
}
