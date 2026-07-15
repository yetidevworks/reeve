//! Live traffic monitoring from the servers' access logs.
//!
//! Every reeve-managed server writes an access log to
//! `logs/server-<name>-access.log`. Apache and nginx are configured (in
//! `backends/`) to write reeve's flat format:
//!
//! ```text
//! 2026-07-14T10:11:12-0700 grav.test GET "/index.php?x=1" 200 5123 12ms 127.0.0.1
//! ```
//!
//! Caddy can't be shaped into a custom line format, so it writes its native
//! JSON access log to the same path. Both parse into one [`AccessEvent`], so
//! the TUI traffic view (and anything else) can treat all backends alike.
//!
//! [`Monitor::start`] spawns one tail-follower thread per server; parsed
//! events arrive over a channel and are held in a bounded, time-pruned buffer.
//! [`stats`] aggregates that buffer (through a [`Filter`]) into a per-second
//! request series plus totals, for rendering.

use crate::paths;
use crate::state::State;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One parsed access-log line, normalized across backends.
#[derive(Clone, Debug)]
pub struct AccessEvent {
    /// Wall-clock second (epoch) the event was ingested — used for bucketing.
    pub epoch_sec: u64,
    /// Local HH:MM:SS for the live tail display.
    pub time_hms: String,
    /// The reeve server (i.e. which access log) this came from.
    pub server: String,
    /// Request host, lowercased, any `:port` stripped.
    pub host: String,
    pub method: String,
    /// Request path including the query string.
    pub path: String,
    pub status: u16,
    pub bytes: u64,
    pub duration_ms: f64,
}

/// Events older than this are pruned from the buffer.
const MAX_AGE_SECS: u64 = 600;
/// Hard cap on buffered events (a runaway load test shouldn't eat memory).
const MAX_EVENTS: usize = 20_000;
/// How often tailer threads poll their file for new lines.
const POLL_MS: u64 = 200;

/// Tails every server's access log on background threads and buffers the
/// parsed events. Dropping it stops the threads.
pub struct Monitor {
    rx: Option<Receiver<AccessEvent>>,
    shutdown: Arc<AtomicBool>,
    /// Server names being tailed, to detect when a rebuild is needed.
    pub servers: Vec<String>,
    /// Ingested events, oldest first. Pruned by [`Monitor::ingest`].
    pub events: VecDeque<AccessEvent>,
}

impl Monitor {
    /// Spawn one tail-follower per server in `state`. Missing log files are
    /// fine — the tailer waits for them to appear.
    pub fn start(state: &State) -> anyhow::Result<Monitor> {
        let dir = paths::logs_dir()?;
        let (tx, rx) = channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut servers = Vec::new();
        for s in &state.servers {
            let path = dir.join(format!("server-{}-access.log", s.name));
            servers.push(s.name.clone());
            spawn_tailer(s.name.clone(), path, tx.clone(), shutdown.clone());
        }
        Ok(Monitor {
            rx: Some(rx),
            shutdown,
            servers,
            events: VecDeque::new(),
        })
    }

    /// A monitor with pre-loaded events and no threads — for tests/snapshots.
    pub fn with_events(events: Vec<AccessEvent>) -> Monitor {
        Monitor {
            rx: None,
            shutdown: Arc::new(AtomicBool::new(false)),
            servers: Vec::new(),
            events: events.into(),
        }
    }

    /// Whether live collector threads are attached (false for test fixtures).
    pub fn rx_live(&self) -> bool {
        self.rx.is_some()
    }

    /// Drain newly-arrived events into the buffer and prune old ones.
    pub fn ingest(&mut self) {
        if let Some(rx) = &self.rx {
            while let Ok(ev) = rx.try_recv() {
                self.events.push_back(ev);
            }
        }
        let cutoff = now_epoch().saturating_sub(MAX_AGE_SECS);
        while let Some(front) = self.events.front() {
            if front.epoch_sec >= cutoff && self.events.len() <= MAX_EVENTS {
                break;
            }
            self.events.pop_front();
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// Current wall-clock time as epoch seconds.
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Tail-follow `path` (starting at EOF, like `tail -f`), parsing each new line
/// and sending events until shutdown or the receiver goes away. Handles the
/// file not existing yet and truncation/rotation.
fn spawn_tailer(server: String, path: PathBuf, tx: Sender<AccessEvent>, shutdown: Arc<AtomicBool>) {
    thread::spawn(move || {
        // Wait for the log to exist (a stopped server has none yet).
        let file = loop {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }
            match File::open(&path) {
                Ok(f) => break f,
                Err(_) => thread::sleep(Duration::from_millis(500)),
            }
        };
        let mut reader = BufReader::new(file);
        // Start at the end — the monitor shows live traffic, not history.
        let mut pos = reader.seek(SeekFrom::End(0)).unwrap_or(0);
        let mut line = String::new();
        loop {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }
            // Truncated/rotated (e.g. the user cleared the log) — restart.
            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.len() < pos {
                    pos = 0;
                    let _ = reader.seek(SeekFrom::Start(0));
                }
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => thread::sleep(Duration::from_millis(POLL_MS)),
                Ok(n) => {
                    pos += n as u64;
                    // A line without a trailing newline is still being written —
                    // rewind and retry so we don't parse half a line.
                    if !line.ends_with('\n') {
                        pos -= n as u64;
                        let _ = reader.seek(SeekFrom::Start(pos));
                        thread::sleep(Duration::from_millis(POLL_MS));
                        continue;
                    }
                    if let Some(ev) = parse_line(&server, line.trim_end()) {
                        if tx.send(ev).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
}

/// Parse one access-log line from any backend into an event, or None for
/// lines that aren't access entries (startup notices, malformed writes).
pub fn parse_line(server: &str, line: &str) -> Option<AccessEvent> {
    if line.starts_with('{') {
        parse_caddy_json(server, line)
    } else {
        parse_flat(server, line)
    }
}

/// reeve's flat format (Apache + nginx):
/// `<iso8601-ts> <host> <method> "<uri>" <status> <bytes> <dur><unit> <client>`
fn parse_flat(server: &str, line: &str) -> Option<AccessEvent> {
    let mut head = line.splitn(4, ' ');
    let ts = head.next()?;
    let host = head.next()?;
    let method = head.next()?;
    let rest = head.next()?.strip_prefix('"')?;
    let close = closing_quote(rest)?;
    let uri = &rest[..close];
    let mut tail = rest[close + 1..].split_whitespace();
    let status: u16 = tail.next()?.parse().ok()?;
    let bytes: u64 = tail.next()?.parse().ok()?;
    let duration_ms = parse_duration_ms(tail.next()?)?;
    Some(AccessEvent {
        epoch_sec: now_epoch(),
        // ISO 8601 puts HH:MM:SS at chars 11..19 for both backends' variants.
        time_hms: ts.get(11..19).unwrap_or("").to_string(),
        server: server.to_string(),
        host: strip_port(host).to_ascii_lowercase(),
        method: method.to_string(),
        path: uri.to_string(),
        status,
        bytes,
        duration_ms,
    })
}

/// Caddy's native JSON access log.
fn parse_caddy_json(server: &str, line: &str) -> Option<AccessEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let req = v.get("request")?;
    let status = v.get("status")?.as_u64()? as u16;
    let host = req.get("host").and_then(|h| h.as_str()).unwrap_or("");
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("-");
    let uri = req.get("uri").and_then(|u| u.as_str()).unwrap_or("-");
    let bytes = v.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
    let duration_ms = v.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0) * 1000.0;
    let ts = v
        .get("ts")
        .and_then(|t| t.as_f64())
        .map(|t| t as i64)
        .unwrap_or_else(|| now_epoch() as i64);
    Some(AccessEvent {
        epoch_sec: now_epoch(),
        time_hms: local_hms(ts),
        server: server.to_string(),
        host: strip_port(host).to_ascii_lowercase(),
        method: method.to_string(),
        path: uri.to_string(),
        status,
        bytes,
        duration_ms,
    })
}

/// Index of the first unescaped `"` in `s` (both Apache and nginx escape
/// embedded quotes in logged fields).
fn closing_quote(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// `12ms` / `0.012s` → milliseconds.
fn parse_duration_ms(tok: &str) -> Option<f64> {
    if let Some(n) = tok.strip_suffix("ms") {
        return n.parse().ok();
    }
    if let Some(n) = tok.strip_suffix('s') {
        return n.parse::<f64>().ok().map(|v| v * 1000.0);
    }
    tok.parse().ok()
}

/// Drop a trailing `:port` from a host, keeping IPv6 literals intact.
fn strip_port(host: &str) -> &str {
    if host.starts_with('[') {
        // [::1]:8080 → [::1]
        return host
            .split_once(']')
            .map(|(h, _)| &host[..h.len() + 1])
            .unwrap_or(host);
    }
    match host.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    }
}

/// Format epoch seconds as local HH:MM:SS (no chrono; libc is already a dep).
fn local_hms(epoch: i64) -> String {
    let t = epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// What the traffic view is scoped to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Server(String),
    Vhost(String),
}

impl Filter {
    pub fn matches(&self, ev: &AccessEvent) -> bool {
        match self {
            Filter::All => true,
            Filter::Server(name) => ev.server == *name,
            Filter::Vhost(host) => ev.host.eq_ignore_ascii_case(host),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Filter::All => "all servers".into(),
            Filter::Server(name) => format!("server: {name}"),
            Filter::Vhost(host) => format!("vhost: {host}"),
        }
    }
}

/// Case-insensitive substring match against an event's host, path, and server
/// name — the traffic view's `/` search. An empty query matches everything.
pub fn matches_query(ev: &AccessEvent, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_ascii_lowercase();
    // Hosts are lowercased at parse time; paths/servers may not be.
    ev.host.contains(&q)
        || ev.path.to_ascii_lowercase().contains(&q)
        || ev.server.to_ascii_lowercase().contains(&q)
}

/// Aggregated view of the event buffer over the last `window` seconds.
#[derive(Default)]
pub struct Stats {
    /// Requests per second, oldest → newest; `series.len() == window`.
    pub series: Vec<u64>,
    pub total: u64,
    pub s2xx: u64,
    pub s3xx: u64,
    pub s4xx: u64,
    pub s5xx: u64,
    pub bytes: u64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub peak_rps: u64,
    /// Busiest hosts in the window, descending.
    pub top_hosts: Vec<(String, u64)>,
    /// Busiest paths (query string stripped) in the window, descending.
    pub top_paths: Vec<(String, u64)>,
}

/// Aggregate `events` matching `filter` and the `/`-search `query` into
/// per-second buckets covering `[now - window + 1, now]`.
pub fn stats(
    events: &VecDeque<AccessEvent>,
    filter: &Filter,
    query: &str,
    window: usize,
    now: u64,
) -> Stats {
    let start = now.saturating_sub(window as u64 - 1);
    let mut st = Stats {
        series: vec![0; window],
        ..Default::default()
    };
    let mut dur_sum = 0.0;
    let mut hosts: HashMap<&str, u64> = HashMap::new();
    let mut paths: HashMap<&str, u64> = HashMap::new();
    for ev in events {
        if ev.epoch_sec < start
            || ev.epoch_sec > now
            || !filter.matches(ev)
            || !matches_query(ev, query)
        {
            continue;
        }
        st.series[(ev.epoch_sec - start) as usize] += 1;
        st.total += 1;
        match ev.status {
            200..=299 => st.s2xx += 1,
            300..=399 => st.s3xx += 1,
            400..=499 => st.s4xx += 1,
            500..=599 => st.s5xx += 1,
            _ => {}
        }
        st.bytes += ev.bytes;
        dur_sum += ev.duration_ms;
        if ev.duration_ms > st.max_ms {
            st.max_ms = ev.duration_ms;
        }
        *hosts.entry(ev.host.as_str()).or_default() += 1;
        let path = ev.path.split('?').next().unwrap_or(&ev.path);
        *paths.entry(path).or_default() += 1;
    }
    if st.total > 0 {
        st.avg_ms = dur_sum / st.total as f64;
    }
    st.peak_rps = st.series.iter().copied().max().unwrap_or(0);
    st.top_hosts = top_n(hosts, 10);
    st.top_paths = top_n(paths, 10);
    st
}

fn top_n(map: HashMap<&str, u64>, n: usize) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = map.into_iter().map(|(k, c)| (k.to_string(), c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

/// Human-readable byte count for the stats panel.
pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut val = b as f64;
    let mut unit = 0;
    while val >= 1024.0 && unit < UNITS.len() - 1 {
        val /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{b} B")
    } else {
        format!("{val:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apache_flat_line() {
        let line =
            r#"2026-07-14T10:11:12-0700 grav.test GET "/index.php?x=1" 200 5123 12ms 127.0.0.1"#;
        let ev = parse_line("apache", line).unwrap();
        assert_eq!(ev.server, "apache");
        assert_eq!(ev.host, "grav.test");
        assert_eq!(ev.method, "GET");
        assert_eq!(ev.path, "/index.php?x=1");
        assert_eq!(ev.status, 200);
        assert_eq!(ev.bytes, 5123);
        assert_eq!(ev.duration_ms, 12.0);
        assert_eq!(ev.time_hms, "10:11:12");
    }

    #[test]
    fn parses_nginx_flat_line() {
        let line = r#"2026-07-14T10:11:12-07:00 app.test POST "/api/save" 404 0 0.012s 127.0.0.1"#;
        let ev = parse_line("nginx", line).unwrap();
        assert_eq!(ev.host, "app.test");
        assert_eq!(ev.status, 404);
        assert!((ev.duration_ms - 12.0).abs() < 1e-9);
        assert_eq!(ev.time_hms, "10:11:12");
    }

    #[test]
    fn parses_caddy_json_line() {
        let line = r#"{"level":"info","ts":1752516672.5,"logger":"http.log.access","msg":"handled request","request":{"remote_ip":"127.0.0.1","proto":"HTTP/2.0","method":"GET","host":"grav.caddy:1443","uri":"/blog?page=2"},"duration":0.004,"size":9876,"status":200}"#;
        let ev = parse_line("caddy", line).unwrap();
        assert_eq!(ev.server, "caddy");
        assert_eq!(ev.host, "grav.caddy");
        assert_eq!(ev.method, "GET");
        assert_eq!(ev.path, "/blog?page=2");
        assert_eq!(ev.status, 200);
        assert_eq!(ev.bytes, 9876);
        assert!((ev.duration_ms - 4.0).abs() < 1e-9);
    }

    #[test]
    fn ignores_non_access_lines() {
        assert!(parse_line("caddy", "").is_none());
        assert!(parse_line("apache", "[notice] httpd starting").is_none());
        // Caddy runtime log without a request object.
        assert!(parse_line("caddy", r#"{"level":"info","msg":"serving"}"#).is_none());
    }

    #[test]
    fn escaped_quote_in_uri_is_handled() {
        let line = r#"2026-07-14T10:11:12-0700 a.test GET "/x\"y" 200 1 1ms 127.0.0.1"#;
        let ev = parse_line("apache", line).unwrap();
        assert_eq!(ev.path, r#"/x\"y"#);
        assert_eq!(ev.status, 200);
    }

    #[test]
    fn strips_ports_from_hosts() {
        assert_eq!(strip_port("grav.caddy:1443"), "grav.caddy");
        assert_eq!(strip_port("grav.test"), "grav.test");
        assert_eq!(strip_port("[::1]:8080"), "[::1]");
    }

    fn ev(sec: u64, server: &str, host: &str, status: u16, ms: f64) -> AccessEvent {
        AccessEvent {
            epoch_sec: sec,
            time_hms: "10:00:00".into(),
            server: server.into(),
            host: host.into(),
            method: "GET".into(),
            path: "/".into(),
            status,
            bytes: 100,
            duration_ms: ms,
        }
    }

    #[test]
    fn stats_buckets_and_filters() {
        let events: VecDeque<AccessEvent> = vec![
            ev(1000, "caddy", "a.test", 200, 10.0),
            ev(1001, "caddy", "a.test", 404, 20.0),
            ev(1001, "apache", "b.test", 500, 30.0),
            ev(950, "caddy", "a.test", 200, 5.0), // outside the window
        ]
        .into();

        let st = stats(&events, &Filter::All, "", 10, 1001);
        assert_eq!(st.total, 3);
        assert_eq!(st.series.len(), 10);
        assert_eq!(st.series[8], 1); // sec 1000
        assert_eq!(st.series[9], 2); // sec 1001
        assert_eq!((st.s2xx, st.s3xx, st.s4xx, st.s5xx), (1, 0, 1, 1));
        assert_eq!(st.peak_rps, 2);
        assert!((st.avg_ms - 20.0).abs() < 1e-9);
        assert_eq!(st.top_hosts[0], ("a.test".into(), 2));

        let st = stats(&events, &Filter::Server("apache".into()), "", 10, 1001);
        assert_eq!(st.total, 1);
        assert_eq!(st.s5xx, 1);

        let st = stats(&events, &Filter::Vhost("a.test".into()), "", 10, 1001);
        assert_eq!(st.total, 2);
    }

    #[test]
    fn search_query_matches_host_path_and_server() {
        let mut e = ev(1000, "caddy", "grav.test", 200, 1.0);
        e.path = "/Blog/post?x=1".into();
        assert!(matches_query(&e, ""));
        assert!(matches_query(&e, "grav"));
        assert!(matches_query(&e, "GRAV")); // case-insensitive
        assert!(matches_query(&e, "blog"));
        assert!(matches_query(&e, "caddy"));
        assert!(!matches_query(&e, "nginx"));

        // And it narrows stats() on top of the scope filter.
        let events: VecDeque<AccessEvent> = vec![
            ev(1000, "caddy", "grav.test", 200, 1.0),
            ev(1000, "caddy", "app.test", 200, 1.0),
        ]
        .into();
        let st = stats(&events, &Filter::All, "grav", 10, 1001);
        assert_eq!(st.total, 1);
        assert_eq!(st.top_hosts[0].0, "grav.test");
    }

    #[test]
    fn monitor_prunes_old_events() {
        let now = now_epoch();
        let mut m = Monitor::with_events(vec![
            ev(
                now.saturating_sub(2 * MAX_AGE_SECS),
                "caddy",
                "old.test",
                200,
                1.0,
            ),
            ev(now, "caddy", "new.test", 200, 1.0),
        ]);
        m.ingest();
        assert_eq!(m.events.len(), 1);
        assert_eq!(m.events[0].host, "new.test");
    }

    #[test]
    fn bytes_format() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2.0 KB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
