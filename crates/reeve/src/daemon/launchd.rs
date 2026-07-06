//! launchd (macOS) service management. Both php-fpm masters and web servers run
//! as user LaunchAgents under `~/Library/LaunchAgents/com.reeve.*.plist`.
//! Generic over the service label so one implementation serves every daemon.

use super::{ServiceSpec, Status};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const LABEL_PREFIX: &str = "com.reeve";

/// Reverse-DNS label for a managed service, e.g. `com.reeve.php-83`.
pub fn label(service: &str) -> String {
    format!("{LABEL_PREFIX}.{service}")
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

/// The per-user GUI launchd domain target, e.g. `gui/501`. All of reeve's
/// services are user LaunchAgents, so they live in this domain.
fn domain() -> String {
    // Safe: getuid() has no failure mode and no memory safety concerns.
    format!("gui/{}", unsafe { libc::getuid() })
}

/// Full service target within the GUI domain, e.g. `gui/501/com.reeve.php-85`.
fn service_target(service: &str) -> String {
    format!("{}/{}", domain(), label(service))
}

/// Load (start) a service via the modern `launchctl bootstrap` API.
///
/// We deliberately avoid the legacy `launchctl load -w`: on macOS 26 (Tahoe)
/// `load -w` silently refuses to start a label that launchd has marked
/// *disabled* (e.g. after an earlier crash-loop auto-disabled it), and reports
/// success while spawning nothing. `enable` reliably clears that sticky disabled
/// flag, and `bootstrap` is the supported way to load a plist into a domain.
pub fn load(service: &str) -> Result<()> {
    let path = plist_path(service)?;
    if !path.exists() {
        bail!("No plist for service '{service}'. Install it first.");
    }

    // Clear any sticky "disabled" state so bootstrap can actually start it.
    // Best-effort: a never-disabled label makes this a no-op.
    let _ = Command::new("launchctl")
        .args(["enable", &service_target(service)])
        .output();

    // bootstrap can transiently fail with "Input/output error" (errno 5) while a
    // just-booted-out job in the same domain is still tearing down. That's a
    // "retry" signal, not a real failure, so spin briefly before giving up.
    for attempt in 0..6 {
        let out = Command::new("launchctl")
            .arg("bootstrap")
            .arg(domain())
            .arg(&path)
            .output()
            .context("Failed to run launchctl bootstrap")?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Already loaded is the desired end state.
        if stderr.contains("already bootstrapped")
            || stderr.contains("already loaded")
            || stderr.contains("service already loaded")
            || stderr.contains("Operation already in progress")
        {
            return Ok(());
        }
        // Domain still busy from a prior bootout — wait and retry.
        let retryable = stderr.contains("Input/output error") || stderr.contains(": 5:");
        if retryable && attempt < 5 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            continue;
        }
        if !stderr.trim().is_empty() {
            bail!("launchctl bootstrap failed: {}", stderr.trim());
        }
        return Ok(());
    }
    Ok(())
}

/// Unload (stop) a service via `launchctl bootout`. Tolerates "not loaded".
pub fn unload(service: &str) -> Result<()> {
    let path = plist_path(service)?;
    if !path.exists() {
        return Ok(());
    }
    let out = Command::new("launchctl")
        .arg("bootout")
        .arg(service_target(service))
        .output()
        .context("Failed to run launchctl bootout")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Not loaded / unknown is the desired end state already.
        let benign = stderr.contains("No such process")
            || stderr.contains("not loaded")
            || stderr.contains("Could not find")
            || stderr.contains("could not find");
        if !benign && !stderr.trim().is_empty() {
            bail!("launchctl bootout failed: {}", stderr.trim());
        }
    }
    Ok(())
}

/// Restart = bootout then bootstrap. The full unload/load cycle (rather than
/// `kickstart`) is required so a rewritten plist's new `ProgramArguments` are
/// actually re-read — `kickstart` would just re-run the already-loaded job def.
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
