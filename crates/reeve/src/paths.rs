//! Centralized filesystem layout for reeve.
//!
//! Everything reeve owns lives under a single config root resolved via
//! `dirs::config_dir()` (matching the sibling ytunnel tool) — on macOS that is
//! `~/Library/Application Support/reeve`, on Linux `~/.config/reeve`.
//! Native web-server and FPM configs are *generated* into `generated/`; users
//! never hand-edit them.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Root config directory, e.g. `~/.config/reeve`.
pub fn config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("Could not determine config directory")?
        .join("reeve");
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn state_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("state.toml"))
}

/// Generated native configs (Caddyfile, apache/nginx/ols vhosts, fpm pools).
pub fn generated_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("generated"))
}

/// Unix sockets, pid files, and other runtime bits. Deliberately kept under a
/// space-free, short path (`~/.reeve/run`) rather than the config root:
/// socket paths get embedded verbatim into `SetHandler proxy:unix:` /
/// `fastcgi_pass unix:` directives that don't tolerate spaces, and unix socket
/// paths have a ~104-char length limit on macOS.
pub fn run_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".reeve").join("run"))
}

/// CLI shim directory (`~/.reeve/bin`). Holds symlinks (`php`, `pecl`, …) that
/// point at the user's chosen CLI PHP version. Kept under `~/.reeve` (not the
/// config root) so it's a short, space-free path the user prepends to PATH.
pub fn shim_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".reeve").join("bin"))
}

/// mkcert-minted per-vhost certificates.
pub fn certs_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("certs"))
}

/// Per-service log files.
pub fn logs_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("logs"))
}

/// Create the full directory skeleton. Idempotent.
pub fn ensure_dirs() -> Result<()> {
    for dir in [
        config_dir()?,
        generated_dir()?,
        generated_dir()?.join("caddy"),
        generated_dir()?.join("apache"),
        generated_dir()?.join("nginx"),
        generated_dir()?.join("ols"),
        generated_dir()?.join("fpm"),
        run_dir()?,
        shim_dir()?,
        certs_dir()?,
        logs_dir()?,
    ] {
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    }
    Ok(())
}
