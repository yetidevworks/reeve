//! Reading service log files. Every reeve-managed launchd service writes its
//! combined stdout/stderr under `logs/`. This module resolves a friendly target
//! name (`server-caddy`, `php-8.3`, `dnsmasq`, `svc-mysql`) to that file and
//! prints the tail or follows it live.

use crate::paths;
use crate::php;
use crate::services;
use crate::state::{Backend, State};
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// A named log the user can view, with its on-disk path.
pub struct LogTarget {
    pub label: String,
    pub path: PathBuf,
}

/// Every log reeve knows about from current state, in display order.
pub fn targets(state: &State) -> Result<Vec<LogTarget>> {
    let dir = paths::logs_dir()?;
    let mut out = Vec::new();
    for s in &state.servers {
        out.push(LogTarget {
            label: format!("server-{}", s.name),
            path: dir.join(format!("server-{}.log", s.name)),
        });
        // Every backend writes its access log to the same standard path (flat
        // format for Apache/nginx, native JSON for Caddy — see src/traffic.rs).
        out.push(LogTarget {
            label: format!("server-{}-access", s.name),
            path: dir.join(format!("server-{}-access.log", s.name)),
        });
        // Apache/nginx split errors into their own file; Caddy's go to the
        // service log above.
        if matches!(s.backend, Backend::Apache | Backend::Nginx) {
            out.push(LogTarget {
                label: format!("server-{}-error", s.name),
                path: dir.join(format!("server-{}-error.log", s.name)),
            });
        }
    }
    for p in &state.php_versions {
        out.push(LogTarget {
            label: format!("php-{}", p.version),
            path: php::launchd_log(&p.version)?,
        });
    }
    for svc in &state.services {
        let id = services::service_id(svc.kind);
        out.push(LogTarget {
            path: dir.join(format!("{id}.log")),
            label: id,
        });
    }
    out.push(LogTarget {
        label: "dnsmasq".into(),
        path: dir.join("dnsmasq.log"),
    });
    Ok(out)
}

/// Resolve a target name to a log file path. Matches a known label first
/// (case-insensitive), then falls back to `<target>.log` in the logs dir.
pub fn resolve(state: &State, target: &str) -> Result<PathBuf> {
    let known = targets(state)?;
    if let Some(t) = known.iter().find(|t| t.label.eq_ignore_ascii_case(target)) {
        return Ok(t.path.clone());
    }
    // Bare filename / stem fallback (e.g. "server-caddy.log" or "caddy").
    let dir = paths::logs_dir()?;
    for cand in [
        dir.join(target),
        dir.join(format!("{target}.log")),
        dir.join(format!("server-{target}.log")),
    ] {
        if cand.exists() {
            return Ok(cand);
        }
    }
    let labels = known
        .iter()
        .map(|t| t.label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    bail!("Unknown log target '{target}'. Available: {labels}");
}

/// Print the last `lines` lines of a log file.
pub fn tail(path: &Path, lines: usize) -> Result<()> {
    if !path.exists() {
        println!("(no log yet at {})", path.display());
        return Ok(());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    for line in last_lines(&content, lines) {
        println!("{line}");
    }
    Ok(())
}

/// Print the tail, then stream new lines as they are appended (like `tail -f`).
/// Blocks until interrupted (Ctrl-C).
pub fn follow(path: &Path, lines: usize) -> Result<()> {
    tail(path, lines)?;
    // Wait for the file to exist if it doesn't yet.
    let mut file = loop {
        match File::open(path) {
            Ok(f) => break f,
            Err(_) => thread::sleep(Duration::from_millis(500)),
        }
    };
    let mut pos = file.seek(SeekFrom::End(0))?;
    let mut reader = BufReader::new(file.try_clone()?);
    loop {
        let len = file.metadata()?.len();
        if len < pos {
            // File was truncated/rotated — restart from the top.
            pos = 0;
            reader.seek(SeekFrom::Start(0))?;
        }
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            thread::sleep(Duration::from_millis(400));
        } else {
            print!("{line}");
            pos += n as u64;
        }
    }
}

/// The last `n` lines of a string (used for the tail; small dev logs).
fn last_lines(content: &str, n: usize) -> Vec<&str> {
    let all: Vec<&str> = content.lines().collect();
    let start = all.len().saturating_sub(n);
    all[start..].to_vec()
}

/// Read up to `n` trailing lines of a file into a Vec, for the TUI viewer.
/// Returns an empty Vec if the file is missing.
pub fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    let mut buf = String::new();
    if File::open(path)
        .and_then(|mut f| f.read_to_string(&mut buf))
        .is_err()
    {
        return Vec::new();
    }
    last_lines(&buf, n).into_iter().map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_lines_takes_tail() {
        let s = "a\nb\nc\nd\ne";
        assert_eq!(last_lines(s, 2), vec!["d", "e"]);
        assert_eq!(last_lines(s, 10), vec!["a", "b", "c", "d", "e"]);
        assert_eq!(last_lines("", 3), Vec::<&str>::new());
    }
}
