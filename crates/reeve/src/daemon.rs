//! launchd (macOS) service management. Both php-fpm masters and web servers run
//! as user LaunchAgents under `~/Library/LaunchAgents/com.reeve.*.plist`.
//! Generic over the service label so one implementation serves every daemon.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const LABEL_PREFIX: &str = "com.reeve";

/// Reverse-DNS label for a managed service, e.g. `com.reeve.php-83`.
pub fn label(service: &str) -> String {
    format!("{LABEL_PREFIX}.{service}")
}

/// Declarative description of a launchd-managed process.
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
    /// Start as soon as the agent is loaded.
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

fn launch_agents_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join("Library/LaunchAgents"))
}

fn plist_path(service: &str) -> Result<PathBuf> {
    Ok(launch_agents_dir()?.join(format!("{}.plist", label(service))))
}

fn render_plist(spec: &ServiceSpec) -> String {
    let label = label(&spec.service);
    let mut args_xml = String::new();
    args_xml.push_str(&format!(
        "        <string>{}</string>\n",
        xml_escape(&spec.program.display().to_string())
    ));
    for arg in &spec.args {
        args_xml.push_str(&format!("        <string>{}</string>\n", xml_escape(arg)));
    }

    let keep_alive = if spec.keep_alive {
        "    <key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>\n"
    } else {
        ""
    };
    let run_at_load = if spec.run_at_load { "true" } else { "false" };
    let log = xml_escape(&spec.log.display().to_string());

    // ProcessType=Interactive, NOT Background. On Apple Silicon, launchd's
    // Background type throttles the job onto efficiency cores at low QoS — for
    // PHP-FPM that made every request 3-5x slower than the same PHP under brew's
    // (un-throttled) httpd. reeve's services sit in the active dev request path,
    // so they must run at interactive priority on the performance cores.
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}    </array>
    <key>RunAtLoad</key>
    <{run_at_load}/>
{keep_alive}    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Write the plist for a service. Does not load it.
pub fn install(spec: &ServiceSpec) -> Result<()> {
    let dir = launch_agents_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    if let Some(parent) = spec.log.parent() {
        fs::create_dir_all(parent).ok();
    }
    let path = plist_path(&spec.service)?;
    fs::write(&path, render_plist(spec))
        .with_context(|| format!("Failed to write plist {}", path.display()))?;
    Ok(())
}

/// Load (start) a service. Tolerates "already loaded".
pub fn load(service: &str) -> Result<()> {
    let path = plist_path(service)?;
    if !path.exists() {
        bail!("No plist for service '{service}'. Install it first.");
    }
    let out = Command::new("launchctl")
        .args(["load", "-w"])
        .arg(&path)
        .output()
        .context("Failed to run launchctl load")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.contains("already loaded") && !stderr.trim().is_empty() {
            bail!("launchctl load failed: {}", stderr.trim());
        }
    }
    Ok(())
}

/// Unload (stop) a service. Tolerates "not loaded".
pub fn unload(service: &str) -> Result<()> {
    let path = plist_path(service)?;
    if !path.exists() {
        return Ok(());
    }
    let out = Command::new("launchctl")
        .arg("unload")
        .arg(&path)
        .output()
        .context("Failed to run launchctl unload")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.contains("not loaded")
            && !stderr.contains("Could not find")
            && !stderr.trim().is_empty()
        {
            bail!("launchctl unload failed: {}", stderr.trim());
        }
    }
    Ok(())
}

/// Restart = unload then load. Picks up config changes.
pub fn restart(service: &str) -> Result<()> {
    unload(service).ok();
    load(service)
}

/// Remove a service entirely (unload + delete plist).
pub fn uninstall(service: &str) -> Result<()> {
    unload(service).ok();
    let path = plist_path(service)?;
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("Failed to remove plist {}", path.display()))?;
    }
    Ok(())
}

/// Query a service's status from `launchctl list <label>`.
pub fn status(service: &str) -> Status {
    let out = Command::new("launchctl")
        .args(["list", &label(service)])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            if s.contains("\"PID\"") {
                Status::Running
            } else if s.contains("\"LastExitStatus\"") && !s.contains("\"LastExitStatus\" = 0") {
                Status::Error
            } else {
                Status::Stopped
            }
        }
        _ => Status::Stopped,
    }
}

/// The PID launchd currently reports for a service, if it is running. Parsed
/// from `launchctl list <label>` (the `"PID" = N;` line).
pub fn pid(service: &str) -> Option<u32> {
    let out = Command::new("launchctl")
        .args(["list", &label(service)])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    for line in s.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("\"PID\" = ") {
            return rest.trim_end_matches(';').trim().parse::<u32>().ok();
        }
    }
    None
}

/// True if a unix socket path exists and is a socket (cheap FPM health check).
pub fn socket_alive(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path)
        .map(|m| m.file_type().is_socket())
        .unwrap_or(false)
}
