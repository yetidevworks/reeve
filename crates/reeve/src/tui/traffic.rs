//! The full-screen live traffic view: requests/sec sparkline, status-class
//! breakdown, top hosts/paths, and a scrolling request tail — filterable to
//! one server or one vhost. Data comes from `crate::traffic::Monitor`, which
//! keeps collecting after the view closes so reopening keeps history.

use super::App;
use crate::traffic::{fmt_bytes, now_epoch, stats, Filter, Stats};
use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Sparkline},
    Frame,
};

const ACCENT: Color = Color::Cyan;

/// Color for an HTTP status class.
fn status_color(status: u16) -> Color {
    match status {
        200..=299 => Color::Green,
        300..=399 => Color::Cyan,
        400..=499 => Color::Yellow,
        _ => Color::Red,
    }
}

/// The cycling filter list: all servers, each server, then each distinct
/// vhost (declared + parked).
pub fn filters(app: &App) -> Vec<Filter> {
    let mut out = vec![Filter::All];
    for s in &app.state.servers {
        out.push(Filter::Server(s.name.clone()));
    }
    let mut hosts: Vec<String> = app
        .state
        .vhosts
        .iter()
        .map(|v| v.server_name.clone())
        .chain(app.parked_vhosts.iter().map(|v| v.server_name.clone()))
        .collect();
    hosts.sort();
    hosts.dedup();
    out.extend(hosts.into_iter().map(Filter::Vhost));
    out
}

/// Keys while the traffic view is open. `/` starts a text search that narrows
/// everything live; Esc/q/t leave; ←→ cycle the scope filter.
pub fn handle_key(app: &mut App, code: KeyCode) {
    // The `/` search input captures every key while typing.
    if app.traffic_search_input {
        match code {
            // Enter keeps the query applied; Esc abandons it.
            KeyCode::Enter => app.traffic_search_input = false,
            KeyCode::Esc => {
                app.traffic_search.clear();
                app.traffic_search_input = false;
            }
            KeyCode::Backspace => {
                app.traffic_search.pop();
            }
            KeyCode::Char(c) => app.traffic_search.push(c),
            _ => {}
        }
        return;
    }
    let len = filters(app).len().max(1);
    match code {
        KeyCode::Char('/') => app.traffic_search_input = true,
        // With a kept query, the first Esc clears it; the next one leaves.
        KeyCode::Esc if !app.traffic_search.is_empty() => app.traffic_search.clear(),
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => {
            app.traffic_view = false;
            // Statuses went stale while the dashboard was hidden.
            app.refresh();
        }
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Up | KeyCode::Char('k') => {
            app.traffic_filter_idx = (app.traffic_filter_idx + len - 1) % len;
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            app.traffic_filter_idx = (app.traffic_filter_idx + 1) % len;
        }
        // Long parked lists make the filter cycle big — page through it.
        KeyCode::PageUp => {
            app.traffic_filter_idx = app.traffic_filter_idx.saturating_sub(10);
        }
        KeyCode::PageDown => {
            app.traffic_filter_idx = (app.traffic_filter_idx + 10).min(len - 1);
        }
        KeyCode::Char('0') | KeyCode::Home => {
            app.traffic_filter_idx = 0;
            app.traffic_search.clear();
        }
        _ => {}
    }
}

pub fn render(f: &mut Frame, app: &App) {
    let filters = filters(app);
    let idx = app.traffic_filter_idx.min(filters.len().saturating_sub(1));
    let filter = filters.get(idx).cloned().unwrap_or(Filter::All);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // title
            Constraint::Length(10), // requests/sec sparkline
            Constraint::Length(9),  // status / top hosts / top paths
            Constraint::Min(4),     // live tail
            Constraint::Length(1),  // key bar
        ])
        .split(f.area());

    // One sparkline column per second, as wide as the panel allows.
    let window = (chunks[1].width.saturating_sub(2) as usize).clamp(30, 300);
    let now = now_epoch();
    let empty = std::collections::VecDeque::new();
    let events = app.traffic.as_ref().map(|m| &m.events).unwrap_or(&empty);
    let st = stats(events, &filter, &app.traffic_search, window, now);

    render_title(f, app, chunks[0], &filter, idx, filters.len());
    render_chart(f, chunks[1], &st, window);

    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(30),
            Constraint::Percentage(35),
            Constraint::Min(20),
        ])
        .split(chunks[2]);
    render_totals(f, mid[0], &st);
    render_top(f, mid[1], "Top hosts", &st.top_hosts);
    render_top(f, mid[2], "Top paths", &st.top_paths);

    render_tail(f, app, chunks[3], events, &filter);
    render_keys(f, app, chunks[4]);
}

fn render_title(f: &mut Frame, app: &App, area: Rect, filter: &Filter, idx: usize, total: usize) {
    let mut spans = vec![
        Span::styled(
            " reeve traffic",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   filter: "),
        Span::styled(
            format!("‹ {} ›", filter.label()),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ({}/{})", idx + 1, total),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    // The `/` search: a live cursor while typing, dimmed once kept.
    if app.traffic_search_input {
        spans.push(Span::styled(
            format!("   /{}▏", app.traffic_search),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    } else if !app.traffic_search.is_empty() {
        spans.push(Span::styled(
            format!("   /{}", app.traffic_search),
            Style::default().fg(Color::Yellow),
        ));
    }
    spans.push(
        if app.traffic.as_ref().map(|m| m.rx_live()).unwrap_or(false) {
            Span::styled("   ● live", Style::default().fg(Color::Green))
        } else {
            Span::styled("   ○ no collector", Style::default().fg(Color::DarkGray))
        },
    );
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_chart(f: &mut Frame, area: Rect, st: &Stats, window: usize) {
    let title = format!(
        "Requests/sec — last {window}s · {} req · peak {}/s",
        st.total, st.peak_rps
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    // Trim the series to the drawable width (right-aligned: newest at the right
    // edge), so a narrow terminal shows the most recent seconds.
    let w = area.width.saturating_sub(2) as usize;
    let data: Vec<u64> = st.series[st.series.len().saturating_sub(w)..].to_vec();
    let spark = Sparkline::default()
        .block(block)
        .data(data)
        .style(Style::default().fg(ACCENT));
    f.render_widget(spark, area);
}

fn render_totals(f: &mut Frame, area: Rect, st: &Stats) {
    let classes: [(&str, u64, Color); 4] = [
        ("2xx", st.s2xx, Color::Green),
        ("3xx", st.s3xx, Color::Cyan),
        ("4xx", st.s4xx, Color::Yellow),
        ("5xx", st.s5xx, Color::Red),
    ];
    let max = classes.iter().map(|c| c.1).max().unwrap_or(0).max(1);
    let bar_w = 14usize;
    let mut lines = vec![Line::from(vec![
        Span::raw(" rate  "),
        Span::styled(
            format!("{}/s", st.series.last().copied().unwrap_or(0)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  peak {}/s", st.peak_rps),
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    for (label, count, color) in classes {
        let filled = ((count as f64 / max as f64) * bar_w as f64).round() as usize;
        lines.push(Line::from(vec![
            Span::styled(format!(" {label}  "), Style::default().fg(color)),
            Span::styled("▐".repeat(filled), Style::default().fg(color)),
            Span::styled(
                format!("{}{count}", " ".repeat(bar_w - filled + 1)),
                Style::default().fg(Color::Gray),
            ),
        ]));
    }
    lines.push(Line::from(vec![
        Span::raw(" time  "),
        Span::raw(format!("avg {:.0}ms · max {:.0}ms", st.avg_ms, st.max_ms)),
    ]));
    lines.push(Line::from(vec![
        Span::raw(" sent  "),
        Span::raw(fmt_bytes(st.bytes)),
    ]));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Status ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_top(f: &mut Frame, area: Rect, title: &str, entries: &[(String, u64)]) {
    let view_h = area.height.saturating_sub(2) as usize;
    let max = entries.first().map(|e| e.1).unwrap_or(0).max(1);
    let mut lines: Vec<Line> = Vec::new();
    if entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no traffic yet)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    // Name column sized to the widest visible entry, bar fills the rest.
    let name_w = entries
        .iter()
        .take(view_h)
        .map(|e| e.0.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(8, area.width.saturating_sub(16) as usize);
    let bar_w = (area.width as usize).saturating_sub(name_w + 10).max(4);
    for (name, count) in entries.iter().take(view_h) {
        let shown: String = if name.chars().count() > name_w {
            let tail: String = name
                .chars()
                .rev()
                .take(name_w.saturating_sub(1))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("…{tail}")
        } else {
            name.clone()
        };
        let filled = ((*count as f64 / max as f64) * bar_w as f64).round() as usize;
        lines.push(Line::from(vec![
            Span::raw(format!(" {shown:<name_w$} ")),
            Span::styled("▐".repeat(filled.max(1)), Style::default().fg(ACCENT)),
            Span::styled(format!(" {count}"), Style::default().fg(Color::Gray)),
        ]));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_tail(
    f: &mut Frame,
    app: &App,
    area: Rect,
    events: &std::collections::VecDeque<crate::traffic::AccessEvent>,
    filter: &Filter,
) {
    let view_h = area.height.saturating_sub(2) as usize;
    // Newest at the bottom: walk backwards, then reverse for display.
    let mut rows: Vec<&crate::traffic::AccessEvent> = events
        .iter()
        .rev()
        .filter(|e| filter.matches(e) && crate::traffic::matches_query(e, &app.traffic_search))
        .take(view_h)
        .collect();
    rows.reverse();

    let mut lines: Vec<Line> = Vec::new();
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  waiting for requests — hit a site to see it here",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for e in rows {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", e.time_hms),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<3} ", e.status),
                Style::default()
                    .fg(status_color(e.status))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{:<4} ", e.method)),
            Span::styled(format!("{:<24} ", e.host), Style::default().fg(ACCENT)),
            Span::raw(format!("{:<36} ", clamp_str(&e.path, 36))),
            Span::styled(
                format!("{:>6.0}ms {:>9} ", e.duration_ms, fmt_bytes(e.bytes)),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!("[{}]", e.server),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Live requests ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let _ = app;
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Truncate a string to `max` chars with a trailing ellipsis.
fn clamp_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn render_keys(f: &mut Frame, app: &App, area: Rect) {
    let keys: &[(&str, &str)] = if app.traffic_search_input {
        &[("type", "search"), ("enter", "keep"), ("esc", "clear")]
    } else if !app.traffic_search.is_empty() {
        &[
            ("←→", "filter"),
            ("/", "edit search"),
            ("0/esc", "clear"),
            ("t", "back"),
        ]
    } else {
        &[
            ("←→", "filter"),
            ("PgUp/Dn", "jump"),
            ("/", "search"),
            ("0", "all"),
            ("esc/t", "back"),
        ]
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.push(Span::styled(
            (*k).to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {label}   "),
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
