//! Service management with per-OS backends. macOS runs php-fpm masters and web
//! servers as user LaunchAgents via launchd; Linux runs them as systemd **user**
//! units. The public API is identical on both platforms so every caller
//! (`ops`, `php`, `services`, `dns`, `backends`, `tui`, `probe`) stays
//! platform-agnostic — the backend is selected at compile time here.

use std::fs;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod launchd;
// `load` is part of the preserved public backend API but has no in-crate caller
// (everything uses `restart`), so allow the otherwise-unused re-export.
#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use launchd::{install, label, load, pid, restart, status, uninstall, unload};

#[cfg(target_os = "linux")]
mod systemd;
#[cfg(target_os = "linux")]
#[allow(unused_imports)]
pub use systemd::{install, label, load, pid, restart, status, uninstall, unload};

// Linux-only host helpers (login lingering, user-bus reachability) used by
// `init` and `doctor`. macOS has no analogue, so these are gated out there and
// their call sites are likewise `#[cfg(target_os = "linux")]`.
#[cfg(target_os = "linux")]
pub use systemd::{enable_linger, linger_enabled, user_bus_ok};

/// Declarative description of a managed process. Rendered into a launchd plist
/// on macOS or a systemd unit file on Linux.
pub struct ServiceSpec {
    /// Short service id (without prefix), e.g. `php-83` or `server-caddy`.
    pub service: String,
    /// Absolute path to the program to run.
    pub program: PathBuf,
    /// Program arguments (the program itself is prepended automatically).
    pub args: Vec<String>,
    /// Combined stdout/stderr log path.
    pub log: PathBuf,
    /// Restart on unexpected exit.
    pub keep_alive: bool,
    /// Start as soon as the service is installed/loaded.
    pub run_at_load: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Stopped,
    Error,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Stopped => "stopped",
            Status::Error => "error",
        }
    }
}

/// True if a unix socket path exists and is a socket (cheap FPM health check).
/// Shared by both backends — the FPM socket contract is identical everywhere.
pub fn socket_alive(path: &std::path::Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path)
        .map(|m| m.file_type().is_socket())
        .unwrap_or(false)
}
