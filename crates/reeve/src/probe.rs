//! Best-effort port-binding probes. launchd can report a job as "running" while
//! the process is bound to nothing (an empty config, or a failed bind because
//! another process owns the port). These helpers look at who is *actually*
//! listening so reeve can report whether a server is serving — and refuse to
//! start one on a port an external process already holds.

use std::collections::HashSet;
use std::process::{Command, Stdio};

/// PIDs holding a LISTEN socket on `port` (IPv4 or IPv6), via `lsof` (macOS).
#[cfg(target_os = "macos")]
pub fn port_listeners(port: u16) -> Vec<u32> {
    let out = Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"])
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.trim().parse::<u32>().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// PIDs holding a LISTEN socket on `port`, via `ss` (Linux). `lsof` is often
/// absent on minimal Linux installs, but `ss` (iproute2) is near-universal.
/// Without root, `ss -p` still reveals the pid for the current user's own
/// sockets — which is exactly what reeve needs to recognize its own servers.
#[cfg(target_os = "linux")]
pub fn port_listeners(port: u16) -> Vec<u32> {
    let out = Command::new("ss")
        .args(["-Htlnp"])
        .stderr(Stdio::null())
        .output();
    let Ok(o) = out else {
        return Vec::new();
    };
    let want = port.to_string();
    let mut pids = Vec::new();
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        // Columns: State Recv-Q Send-Q Local:Port Peer:Port [Process].
        // Match the local address column ending in `:<port>`.
        let local = match line.split_whitespace().nth(3) {
            Some(l) => l,
            None => continue,
        };
        if local.rsplit(':').next() != Some(want.as_str()) {
            continue;
        }
        // Extract every `pid=NNN` from the process column
        // (`users:(("caddy",pid=295,fd=3))`).
        for tok in line.split(|c: char| !c.is_ascii_alphanumeric() && c != '=') {
            if let Some(n) = tok.strip_prefix("pid=") {
                if let Ok(p) = n.parse::<u32>() {
                    pids.push(p);
                }
            }
        }
    }
    pids
}

/// Human-readable command name for a pid (`ps -o comm=`), for conflict messages.
pub fn process_name(pid: u32) -> String {
    let name = Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| {
            // `ps comm=` prints the full path; the basename is what users know.
            s.rsplit('/').next().unwrap_or(&s).to_string()
        });
    name.unwrap_or_else(|| format!("pid {pid}"))
}

/// True if `pid`'s parent chain includes `ancestor` (so a worker counts as
/// belonging to its launchd master). Bounded walk; tolerant of missing pids.
fn is_descendant_of(pid: u32, ancestor: u32) -> bool {
    let mut cur = pid;
    for _ in 0..16 {
        if cur == ancestor {
            return true;
        }
        if cur <= 1 {
            return false;
        }
        let ppid = Command::new("ps")
            .args(["-o", "ppid=", "-p", &cur.to_string()])
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            });
        match ppid {
            Some(p) => cur = p,
            None => return false,
        }
    }
    false
}

/// Who, if anyone, is bound to a port relative to a launchd job we expect to
/// own it (`our_pid`, from `daemon::pid`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortState {
    /// Nothing is listening.
    Free,
    /// Our launchd job (or one of its workers) is listening.
    OursBound,
    /// Some other process holds the port.
    Foreign { pid: u32, name: String },
}

/// Classify a single port against the launchd pid that should own it.
pub fn classify_port(port: u16, our_pid: Option<u32>) -> PortState {
    let listeners = port_listeners(port);
    if listeners.is_empty() {
        return PortState::Free;
    }
    if let Some(mine) = our_pid {
        if listeners
            .iter()
            .any(|&l| l == mine || is_descendant_of(l, mine))
        {
            return PortState::OursBound;
        }
    }
    let pid = listeners[0];
    PortState::Foreign {
        pid,
        name: process_name(pid),
    }
}

/// First foreign holder among `ports`, if any (for preflight conflict checks).
pub fn first_foreign(ports: &[u16], our_pid: Option<u32>) -> Option<(u16, u32, String)> {
    let mut seen = HashSet::new();
    for &port in ports {
        if !seen.insert(port) {
            continue;
        }
        if let PortState::Foreign { pid, name } = classify_port(port, our_pid) {
            return Some((port, pid, name));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_ports_means_no_conflict() {
        assert!(first_foreign(&[], None).is_none());
    }

    #[test]
    fn an_unused_high_port_is_free() {
        // Nothing should be listening here in a build/test environment (and if
        // lsof is unavailable, port_listeners returns empty → Free anyway).
        assert_eq!(classify_port(59_321, None), PortState::Free);
    }
}
