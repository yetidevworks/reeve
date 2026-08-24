//! Rendering for the stacked dashboard (ytunnel-style: bordered panels, accent
//! on focus, status line at the bottom).

use super::{App, HyperLink, Panel, PanelHit};
use crate::daemon::Status;
use crate::doctor::Health;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

const ACCENT: Color = Color::Cyan;

pub fn render(f: &mut Frame, app: &App) {
    // Cleared each frame; the vhost/parked panels repopulate it, and run_loop
    // stamps the entries as OSC 8 hyperlinks after the draw completes.
    app.links.borrow_mut().clear();

    // The traffic view replaces the whole dashboard while it's open. Clear the
    // mouse hit-rects too so clicks can't reach the hidden panels.
    if app.traffic_view {
        app.hit_rects.borrow_mut().clear();
        super::traffic::render(f, app);
        return;
    }

    let servers_h = (app.state.servers.len() as u16 + 2).clamp(3, 14);
    let php_h = 3;
    let services_h = (app.state.services.len() as u16 + 2).clamp(3, 8);
    let declared_h = (app.state.vhosts.len() as u16 + 2).clamp(3, 8);

    // The list with the most rows gets the leftover space and scrolls
    // internally; the other stays a fixed height. Directory parking usually
    // makes Parked the long one, but with no parks let Vhosts expand instead.
    let (vhosts_c, parked_c) = if app.parked_vhosts.is_empty() {
        (Constraint::Min(3), Constraint::Length(3))
    } else {
        (Constraint::Length(declared_h), Constraint::Min(3))
    };

    // Order top→bottom: Servers, Vhosts, Parked (one expands/scrolls), PHP,
    // Services, then a context-aware key bar and a status/message line.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),          // title
            Constraint::Length(servers_h),  // servers
            vhosts_c,                       // declared vhosts
            parked_c,                       // parked sites
            Constraint::Length(php_h),      // php
            Constraint::Length(services_h), // services
            Constraint::Length(1),          // key bar
            Constraint::Length(1),          // status/message
        ])
        .split(f.area());

    render_title(f, app, chunks[0]);
    render_servers(f, app, chunks[1]);
    let vhost_off = render_vhosts(f, app, chunks[2]);
    let parked_off = render_parked(f, app, chunks[3]);
    render_php(f, app, chunks[4]);
    render_services(f, app, chunks[5]);
    render_keys(f, app, chunks[6]);
    render_status(f, app, chunks[7]);

    // Record each panel's geometry so the mouse handler can map clicks/wheel
    // back to a panel + row (offset = first visible row index).
    {
        let mut hits = app.hit_rects.borrow_mut();
        hits.clear();
        hits.push(PanelHit {
            panel: Panel::Servers,
            rect: chunks[1],
            offset: 0,
            len: app.state.servers.len(),
        });
        hits.push(PanelHit {
            panel: Panel::Vhosts,
            rect: chunks[2],
            offset: vhost_off,
            len: app.state.vhosts.len(),
        });
        hits.push(PanelHit {
            panel: Panel::Parked,
            rect: chunks[3],
            offset: parked_off,
            len: app.parked_vhosts.len(),
        });
        hits.push(PanelHit {
            panel: Panel::Php,
            rect: chunks[4],
            offset: 0,
            len: app.state.php_versions.len(),
        });
        hits.push(PanelHit {
            panel: Panel::Services,
            rect: chunks[5],
            offset: 0,
            len: app.state.services.len(),
        });
    }

    if app.wizard.is_some() {
        render_wizard(f, app);
    }
    if app.server_wizard.is_some() {
        render_server_wizard(f, app);
    }
    if app.settings_modal.is_some() {
        render_settings_modal(f, app);
    }
    if app.php_install.is_some() {
        render_php_install(f, app);
    }
    if app.ext_modal.is_some() {
        render_ext_modal(f, app);
    }
    if app.config_modal.is_some() {
        render_config_modal(f, app);
    }
    if app.php_settings.is_some() {
        render_php_settings_modal(f, app);
    }
    if app.service_picker.is_some() {
        render_service_picker(f, app);
    }
    if app.park_modal.is_some() {
        render_park_modal(f, app);
    }
    if app.log_modal.is_some() {
        render_log_modal(f, app);
    }
    if app.doctor_modal.is_some() {
        render_doctor_modal(f, app);
    }
    if app.confirm_remove.is_some() {
        render_confirm(f, app, "vhost", app.confirm_remove.as_deref());
    }
    if app.confirm_remove_php.is_some() {
        render_confirm(f, app, "PHP", app.confirm_remove_php.as_deref());
    }
    if app.confirm_remove_server.is_some() {
        render_confirm(f, app, "server", app.confirm_remove_server.as_deref());
    }
}

fn render_php_install(f: &mut Frame, app: &App) {
    let m = app.php_install.as_ref().unwrap();
    let area = centered_rect(50, 8, f.area());
    f.render_widget(Clear, area);
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  Version: "),
            Span::styled(
                format!("{}▏", m.version),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
    ];
    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        "  e.g. 8.3 · enter install · esc cancel",
        Style::default().fg(Color::DarkGray),
    )));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " Install PHP ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_ext_modal(f: &mut Frame, app: &App) {
    let m = app.ext_modal.as_ref().unwrap();
    // Window the loaded list so long lists (php -m is big) stay on screen.
    let total = m.loaded.len();
    let win = 10usize;
    let start = if total <= win {
        0
    } else {
        m.sel.saturating_sub(win / 2).min(total - win)
    };
    let end = (start + win).min(total);
    let area = centered_rect(58, (end - start) as u16 + 9, f.area());
    f.render_widget(Clear, area);

    let typing = !m.input.trim().is_empty();
    let mut lines = vec![
        Line::from(vec![
            Span::raw("  Add: "),
            Span::styled(
                format!("{}▏", m.input),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   (e.g. redis, then enter)",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            format!("  Loaded ({total}):"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];
    if total == 0 {
        lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (i, ext) in m.loaded[start..end].iter().enumerate() {
        let idx = start + i;
        let active = idx == m.sel && !typing;
        let marker = if active { "› " } else { "  " };
        let style = if active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(format!("{marker}{ext}"), style)));
    }
    if end < total {
        lines.push(Line::from(Span::styled(
            "  …",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::raw(""));
    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        "  type+enter add · ↑↓ select · del/enter remove · esc close",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            format!(" PHP {} extensions ", m.version),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_config_modal(f: &mut Frame, app: &App) {
    let m = app.config_modal.as_ref().unwrap();
    let area = centered_rect(76, 16, f.area());
    f.render_widget(Clear, area);

    let backend = super::BACKENDS[m.backend_idx].as_str();
    let cursor = |s: &str, active: bool| {
        if active {
            format!("{s}▏")
        } else {
            s.to_string()
        }
    };
    let field_line = |idx: usize, label: &str, value: String| -> Line {
        let active = m.field == idx;
        let marker = if active { "› " } else { "  " };
        let vstyle = if active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::raw(format!("{marker}{label:<14}")),
            Span::styled(value, vstyle),
        ])
    };

    let mut lines = vec![
        Line::raw(""),
        field_line(0, "Local TLDs:", cursor(&m.tld, m.field == 0)),
        field_line(1, "Sites root:", cursor(&m.sites_root, m.field == 1)),
        field_line(2, "Def backend:", format!("‹ {backend} ›")),
        Line::raw(""),
    ];
    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    // Help text: how to enter multiple TLDs + which are safe.
    lines.push(Line::from(Span::styled(
        "  Space/comma-separated. Use reserved TLDs that can't be real domains:",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "    test       recommended — RFC 6761, never routable",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "    localhost  reserved; *.localhost always maps to loopback",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "    internal   ICANN-designated for private use   ·   lan  common, safe",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "  Avoid .dev/.app (real TLDs → forced HTTPS) and .local (Bonjour).",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  tab/↑↓ field · ←→ backend · enter save · esc cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " Preferences ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_log_modal(f: &mut Frame, app: &App) {
    let m = app.log_modal.as_ref().unwrap();
    let outer = f.area();
    // Near-fullscreen viewer with a small margin.
    let area = centered_rect(
        outer.width.saturating_sub(6),
        outer.height.saturating_sub(4),
        outer,
    );
    f.render_widget(Clear, area);

    // Visible window: interior height minus borders and the footer hint line.
    let win = area.height.saturating_sub(3) as usize;
    let total = m.lines.len();
    let scroll = m.scroll.min(total.saturating_sub(1));
    let end = (scroll + win).min(total);

    let mut lines: Vec<Line> = Vec::new();
    if total == 0 {
        lines.push(Line::from(Span::styled(
            "  (no log output yet)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for l in &m.lines[scroll..end] {
        lines.push(Line::from(Span::raw(app.anon(l))));
    }
    lines.push(Line::from(Span::styled(
        format!(
            "  lines {}-{} of {} · ↑↓/PgUp/PgDn scroll · esc close",
            scroll + 1,
            end,
            total
        ),
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            format!(" log: {} ", m.label),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_settings_modal(f: &mut Frame, app: &App) {
    let m = app.settings_modal.as_ref().unwrap();
    let defs = crate::backends::settings_defs(m.backend);
    let area = centered_rect(64, defs.len() as u16 * 2 + 6, f.area());
    f.render_widget(Clear, area);

    let mut lines = vec![Line::raw("")];
    for (i, def) in defs.iter().enumerate() {
        let active = m.field == i;
        let marker = if active { "› " } else { "  " };
        let val = m.values.get(i).cloned().unwrap_or_default();
        let cursor = if active { format!("{val}▏") } else { val };
        let vstyle = if active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::raw(format!("{marker}{:<22}", format!("{}:", def.label))),
            Span::styled(cursor, vstyle),
            Span::styled(
                format!("   ({})", def.help),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    lines.push(Line::raw(""));
    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        "  tab/↑↓ field · type to edit · enter save · esc cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            format!(" Settings: {} ({}) ", m.server_name, m.backend),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_php_settings_modal(f: &mut Frame, app: &App) {
    let m = app.php_settings.as_ref().unwrap();
    let defs = crate::php::php_settings_defs();
    // Window the (long) field list so it always fits, centered on the active row.
    let total = defs.len();
    let win = 12usize;
    let start = if total <= win {
        0
    } else {
        m.field.saturating_sub(win / 2).min(total - win)
    };
    let end = (start + win).min(total);
    let area = centered_rect(66, (end - start) as u16 + 6, f.area());
    f.render_widget(Clear, area);

    let mut lines = vec![Line::raw("")];
    for (i, def) in defs.iter().enumerate().take(end).skip(start) {
        let active = m.field == i;
        let marker = if active { "› " } else { "  " };
        let val = m.values.get(i).cloned().unwrap_or_default();
        let cursor = if active { format!("{val}▏") } else { val };
        let vstyle = if active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::raw(format!("{marker}{:<24}", format!("{}:", def.label))),
            Span::styled(format!("{cursor:<12}"), vstyle),
            Span::styled(
                format!(" ({})", def.help),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        "  tab/↑↓ field · type to edit · enter save+restart · esc cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            format!(" PHP {} settings ", m.version),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_server_wizard(f: &mut Frame, app: &App) {
    let w = app.server_wizard.as_ref().unwrap();
    let area = centered_rect(62, 13, f.area());
    f.render_widget(Clear, area);

    let backend = super::BACKENDS[w.backend_idx].as_str();
    let inst = if w.installed.get(w.backend_idx).copied().unwrap_or(false) {
        "installed"
    } else {
        "not installed — brew installs on start"
    };
    let field_line = |idx: usize, label: &str, value: String| -> Line {
        let active = w.field == idx;
        let marker = if active { "› " } else { "  " };
        let vstyle = if active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::raw(format!("{marker}{label:<12}")),
            Span::styled(value, vstyle),
        ])
    };
    let cursor = |s: &str, active: bool| {
        if active {
            format!("{s}▏")
        } else {
            s.to_string()
        }
    };

    let mut lines = vec![
        Line::raw(""),
        field_line(0, "Backend:", format!("‹ {backend} ›  ({inst})")),
        field_line(1, "HTTP port:", cursor(&w.http, w.field == 1)),
        field_line(2, "HTTPS port:", cursor(&w.https, w.field == 2)),
        field_line(
            3,
            "Default site:",
            if w.default_site {
                "[x] serve sites root on HTTP".into()
            } else {
                "[ ] off".into()
            },
        ),
        field_line(
            4,
            "Site preset:",
            format!(
                "‹ {} ›",
                crate::state::Framework::all()[w.preset_idx].as_str()
            ),
        ),
        field_line(
            5,
            "Default root:",
            if w.default_root.is_empty() && w.field != 5 {
                format!("{} (global sites root)", app.anon(&app.config.sites_root))
            } else {
                cursor(&w.default_root, w.field == 5)
            },
        ),
        Line::raw(""),
    ];
    if let Some(err) = &w.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        "  tab/↑↓ field · ←→/space change · enter save · esc cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let title = match &w.editing {
        Some(n) => format!(" Edit Server: {n} "),
        None => " New Server ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Context-aware key hints for the focused panel.
fn render_keys(f: &mut Frame, app: &App, area: Rect) {
    let keys: &[(&str, &str)] = match app.focus {
        Panel::Servers => &[
            ("enter", "start"),
            ("x", "stop"),
            ("r", "restart"),
            ("n", "new"),
            ("e", "edit"),
            ("s", "settings"),
            ("R/del", "remove"),
            ("a", "apply"),
            ("t", "traffic"),
            ("L", "log"),
            ("c", "config"),
            ("q", "quit"),
        ],
        Panel::Vhosts => &[
            ("n", "new"),
            ("e", "edit"),
            ("r", "restart"),
            ("R/del", "remove"),
            ("p", "park"),
            ("1-4", "sort"),
            ("a", "apply"),
            ("v", "validate"),
            ("t", "traffic"),
            ("L", "log"),
            ("q", "quit"),
        ],
        Panel::Parked => &[
            ("↑↓/scroll", "browse"),
            ("PgUp/Dn", "page"),
            ("p", "manage parks"),
            ("L", "log"),
            ("r", "apply"),
            ("q", "quit"),
        ],
        Panel::Php => &[
            ("enter/r", "restart"),
            ("x", "stop"),
            ("n", "install"),
            ("e", "exts"),
            ("s", "settings"),
            ("X", "xdebug"),
            ("d", "default"),
            ("C", "cli php"),
            ("R/del", "remove"),
            ("L", "log"),
            ("q", "quit"),
        ],
        Panel::Services => &[
            ("enter", "start"),
            ("x", "stop"),
            ("r", "restart"),
            ("n", "add"),
            ("R/del", "remove"),
            ("L", "log"),
            ("c", "config"),
            ("q", "quit"),
        ],
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

fn render_confirm(f: &mut Frame, _app: &App, noun: &str, name: Option<&str>) {
    let Some(name) = name else { return };
    let area = centered_rect(52, 5, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled(
            format!(" Remove {noun} "),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    let lines = vec![
        Line::raw(""),
        Line::from(format!("  Remove {noun} '{name}'?")),
        Line::from(Span::styled(
            "  y confirm · n/esc cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

fn render_wizard(f: &mut Frame, app: &App) {
    let w = app.wizard.as_ref().unwrap();
    let area = centered_rect(66, 17, f.area());
    f.render_widget(Clear, area);

    let php = app
        .state
        .php_versions
        .get(w.php_idx)
        .map(|p| p.version.as_str())
        .unwrap_or("-");
    let server = app
        .state
        .servers
        .get(w.server_idx)
        .map(|s| s.name.as_str())
        .unwrap_or("-");

    let field_line = |idx: usize, label: &str, value: String| -> Line {
        let active = w.field == idx;
        let marker = if active { "› " } else { "  " };
        let vstyle = if active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::raw(format!("{marker}{label:<10}")),
            Span::styled(value, vstyle),
        ])
    };

    let cursor = |s: &str, active: bool| {
        if active {
            format!("{s}▏")
        } else {
            s.to_string()
        }
    };
    let docroot_shown = if w.docroot.is_empty() && w.field != 1 {
        let host = if w.server_name.is_empty() {
            "<host>"
        } else {
            &w.server_name
        };
        format!(
            "{}/{}  (default)",
            app.config.sites_root.trim_end_matches('/'),
            host
        )
    } else {
        cursor(&w.docroot, w.field == 1)
    };
    let name_shown = if w.server_name.is_empty() && w.field != 0 {
        "(required, e.g. app.test)".to_string()
    } else {
        cursor(&w.server_name, w.field == 0)
    };

    let mut lines = vec![
        Line::raw(""),
        field_line(0, "Hostname:", name_shown),
        field_line(1, "Doc root:", docroot_shown),
        field_line(2, "PHP:", format!("‹ {php} ›")),
        field_line(3, "Server:", format!("‹ {server} ›")),
        field_line(
            4,
            "SSL:",
            if w.ssl {
                "[x] on".into()
            } else {
                "[ ] off".into()
            },
        ),
        field_line(5, "Preset:", format!("‹ {} ›", w.preset)),
        field_line(
            6,
            "Proxy:",
            if w.proxy.is_empty() && w.field != 6 {
                "(none — set to reverse-proxy instead of PHP)".to_string()
            } else {
                cursor(&w.proxy, w.field == 6)
            },
        ),
        Line::raw(""),
    ];
    if !w.proxy.is_empty() {
        lines.push(Line::from(Span::styled(
            "  → reverse-proxy vhost (PHP / preset / docroot ignored)",
            Style::default().fg(Color::Magenta),
        )));
    }
    if let Some(err) = &w.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        "  tab/↑↓ field · ←→/space change · enter create · esc cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let title = if w.editing.is_some() {
        " Edit Vhost "
    } else {
        " New Vhost "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);

    // Doc root autocomplete dropdown (Doc root field, dropdown open).
    if w.field == 1 && w.dropdown_open {
        let sugg = w.path_suggestions();
        if !sugg.is_empty() {
            let dd_w = 48u16.min(area.width.saturating_sub(4));
            let dd_h = (sugg.len() as u16 + 2).min(10);
            let dd = Rect {
                x: area.x + 13,
                y: (area.y + 4).min(area.y + area.height.saturating_sub(dd_h)),
                width: dd_w,
                height: dd_h,
            };
            f.render_widget(Clear, dd);
            // Highlight the selected row (or the first dir, the →/Tab target).
            let target = w.path_sel.or_else(|| sugg.iter().position(|e| e.is_dir));
            let rows: Vec<Line> = sugg
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let label = if e.is_dir {
                        format!(" {}/", e.name)
                    } else {
                        format!(" {}", e.name)
                    };
                    let style = if w.path_sel == Some(i) {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else if Some(i) == target {
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else if e.is_dir {
                        Style::default().fg(ACCENT)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    Line::from(Span::styled(label, style))
                })
                .collect();
            let dd_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " ↑↓ select · → open · tab complete · esc close ",
                    Style::default().fg(Color::DarkGray),
                ));
            f.render_widget(Paragraph::new(rows).block(dd_block), dd);
        }
    }
}

fn render_title(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let tlds = app
        .config
        .local_tlds
        .iter()
        .map(|t| format!(".{t}"))
        .collect::<Vec<_>>()
        .join(" ");
    let dns = if app.dns_ok {
        Span::styled(
            format!("   [{tlds} ✓ resolving]"),
            Style::default().fg(Color::Green),
        )
    } else {
        Span::styled(
            format!("   [{tlds} ⚠ not resolving — press D to enable]"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    };
    let (hsym, hcolor) = match app.health {
        Health::Ok => ("●", Color::Green),
        Health::Warn => ("●", Color::Yellow),
        Health::Fail => ("●", Color::Red),
    };
    let line = Line::from(vec![
        Span::styled(
            " reeve",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        dns,
        Span::raw("   "),
        Span::styled(hsym, Style::default().fg(hcolor)),
        Span::styled(
            " health  ? doctor · t traffic · L logs · T trust",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn panel_block(title: &str, focused: bool) -> Block<'_> {
    let style = if focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::default().fg(ACCENT)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .title(Span::styled(format!(" {title} "), style))
}

fn status_span(status: Status) -> Span<'static> {
    let (dot, color) = match status {
        Status::Running => ("● running", Color::Green),
        Status::Stopped => ("○ stopped", Color::DarkGray),
        Status::Error => ("✗ error", Color::Red),
    };
    Span::styled(dot, Style::default().fg(color))
}

/// Honest, port-aware status marker for a web server row.
fn serve_state_span(state: &crate::ops::ServeState) -> Span<'static> {
    use crate::ops::ServeState as S;
    let (text, color) = match state {
        S::Serving => ("● running".to_string(), Color::Green),
        S::Stopped => ("○ stopped".to_string(), Color::DarkGray),
        S::LoadedNotBound => ("⚠ loaded, not bound".to_string(), Color::Yellow),
        S::PortConflict { port, holder, .. } => (format!("✗ :{port} held by {holder}"), Color::Red),
        S::Crashed => ("✗ crashed".to_string(), Color::Red),
    };
    Span::styled(text, Style::default().fg(color))
}

fn row_style(selected: bool, focused: bool) -> Style {
    if selected && focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn render_servers(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.focus == Panel::Servers;
    let mut lines: Vec<Line> = Vec::new();
    if app.state.servers.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no servers — `reeve server add caddy`",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (i, s) in app.state.servers.iter().enumerate() {
        let sel = i == app.sel_server;
        let status = app
            .server_status
            .get(i)
            .cloned()
            .unwrap_or(crate::ops::ServeState::Stopped);
        // Pad the ports into a fixed-width column so the status marker aligns
        // whether ports are short (:80/:443) or long (:8080/:8443).
        let ports = format!(":{}/:{}", s.http_port, s.https_port);
        let head = format!(
            " {} {:<10} {:<7} {:<13} ",
            if sel && focused { "›" } else { " " },
            s.name,
            s.backend,
            ports,
        );
        let mut spans = vec![Span::raw(head), serve_state_span(&status)];
        // Catch-all default-site root, aligned with the vhost/parked path column.
        if s.default_site {
            let root = s.effective_default_root(&app.config.sites_root);
            spans.push(Span::styled(
                format!("   {}", app.anon(root)),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(spans).style(row_style(sel, focused)));
    }
    f.render_widget(
        Paragraph::new(lines).block(panel_block("Servers", focused)),
        area,
    );
}

fn render_php(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.focus == Panel::Php;
    let default = app.config.default_php.clone();
    let cli = crate::php::current_cli_php();
    // The CLI `php` version isn't per-item obvious, so name it in the title.
    let title = match &cli {
        Some(v) => format!("PHP  (cli: {v})"),
        None => "PHP".to_string(),
    };
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    if app.state.php_versions.is_empty() {
        spans.push(Span::styled(
            "no PHP — `reeve php install 8.3`",
            Style::default().fg(Color::DarkGray),
        ));
    }
    for (i, p) in app.state.php_versions.iter().enumerate() {
        let sel = i == app.sel_php;
        let status = app.php_status.get(i).copied().unwrap_or(Status::Stopped);
        let is_default = default.as_deref() == Some(p.version.as_str());
        let label = format!("{}{} ", p.version, if is_default { "★" } else { "" });
        spans.push(Span::styled(label, row_style(sel, focused)));
        spans.push(status_span(status));
        if !p.xdebug.is_off() {
            spans.push(Span::styled(
                format!(" 🐛{}", p.xdebug.as_str()),
                Style::default().fg(Color::Magenta),
            ));
        }
        spans.push(Span::raw("   "));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).block(panel_block(&title, focused)),
        area,
    );
}

fn render_services(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let focused = app.focus == Panel::Services;
    let mut lines: Vec<Line> = Vec::new();
    if app.state.services.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no services — press 'n' to add a database, redis, or mailpit",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (i, s) in app.state.services.iter().enumerate() {
        let sel = i == app.sel_service;
        let status = app
            .service_status
            .get(i)
            .copied()
            .unwrap_or(Status::Stopped);
        let head = format!(
            " {} {:<11} :{:<6} ",
            if sel && focused { "›" } else { " " },
            s.kind,
            crate::services::port(s.kind),
        );
        lines.push(
            Line::from(vec![Span::raw(head), status_span(status)]).style(row_style(sel, focused)),
        );
    }
    f.render_widget(
        Paragraph::new(lines).block(panel_block("Services", focused)),
        area,
    );
}

fn render_park_modal(f: &mut Frame, app: &App) {
    let m = app.park_modal.as_ref().unwrap();
    let parks = &app.state.parks;
    let area = centered_rect(74, parks.len().max(1) as u16 + 12, f.area());
    f.render_widget(Clear, area);

    let server = app
        .state
        .servers
        .get(m.server_idx)
        .map(|s| s.name.as_str())
        .unwrap_or("-");
    let php = app
        .state
        .php_versions
        .get(m.php_idx)
        .map(|p| p.version.as_str())
        .unwrap_or("-");

    let cursor = |s: &str, active: bool| {
        if active {
            format!("{s}▏")
        } else {
            s.to_string()
        }
    };
    let field_line = |idx: usize, label: &str, value: String| -> Line {
        let active = m.field == idx;
        let marker = if active { "› " } else { "  " };
        let vstyle = if active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::raw(format!("{marker}{label:<9}")),
            Span::styled(value, vstyle),
        ])
    };

    let mut lines = vec![Line::from(Span::styled(
        "  Parked directories (↑↓ select, del to remove):",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if parks.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (none yet)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (i, p) in parks.iter().enumerate() {
        let active = m.field == 0 && i == m.sel_park;
        let marker = if active { "  › " } else { "    " };
        let style = if active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{marker}{} → *.{} ({}, {})",
                p.root, p.tld, p.server, p.php_version
            ),
            style,
        )));
    }
    lines.push(Line::raw(""));
    // The same form adds and edits: re-parking a directory replaces its entry,
    // so say which one it's about to do rather than always claiming "add".
    let editing = parks.iter().any(|p| p.root == m.dir.trim());
    lines.push(Line::from(Span::styled(
        if editing {
            "  Edit this park:"
        } else {
            "  Add a park:"
        },
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(field_line(1, "Dir:", cursor(&m.dir, m.field == 1)));
    lines.push(field_line(2, "Server:", format!("‹ {server} ›")));
    lines.push(field_line(3, "PHP:", format!("‹ {php} ›")));
    lines.push(field_line(
        4,
        "SSL:",
        if m.ssl {
            "[x] on".into()
        } else {
            "[ ] off".into()
        },
    ));
    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        if editing {
            "  tab field · ←→/space change · enter save · esc close"
        } else {
            "  tab field · ←→/space change · enter add · esc close"
        },
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " Directory parking ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_doctor_modal(f: &mut Frame, app: &App) {
    let m = app.doctor_modal.as_ref().unwrap();
    let area = centered_rect(72, m.lines.len() as u16 + 4, f.area());
    f.render_widget(Clear, area);

    let mut lines = vec![Line::raw("")];
    for (health, text) in &m.lines {
        let color = match health {
            Health::Ok => Color::Green,
            Health::Warn => Color::Yellow,
            Health::Fail => Color::Red,
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", health.symbol()), Style::default().fg(color)),
            Span::raw(app.anon(text)),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "  any key to close",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " doctor ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_service_picker(f: &mut Frame, app: &App) {
    let m = app.service_picker.as_ref().unwrap();
    let kinds = crate::state::ServiceKind::all();
    let area = centered_rect(60, kinds.len() as u16 + 4, f.area());
    f.render_widget(Clear, area);

    let mut lines = vec![Line::raw("")];
    for (i, kind) in kinds.iter().enumerate() {
        let active = i == m.sel;
        let marker = if active { "› " } else { "  " };
        let style = if active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{:<11}", kind.as_str()), style),
            Span::styled(
                crate::services::blurb(*kind),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "  ↑↓ select · enter add+start · esc cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " Add service ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Browser URL for a vhost, derived from its owning server's scheme + port.
/// Ports 80/443 are omitted so the URL is clean (and clickable in iTerm/Ghostty).
pub fn vhost_url(app: &App, vhost: &crate::state::Vhost) -> String {
    match app.state.get_server(&vhost.server) {
        Some(s) if vhost.ssl => {
            if s.https_port == 443 {
                format!("https://{}", vhost.server_name)
            } else {
                format!("https://{}:{}", vhost.server_name, s.https_port)
            }
        }
        Some(s) => {
            if s.http_port == 80 {
                format!("http://{}", vhost.server_name)
            } else {
                format!("http://{}:{}", vhost.server_name, s.http_port)
            }
        }
        None => format!("(server '{}' missing)", vhost.server),
    }
}

/// Record a clickable URL region for a vhost/parked row so it can be stamped as
/// an OSC 8 hyperlink after the draw. `row` is the visible (0-based) index of the
/// line within the panel, and `shown` is the text actually drawn (already fitted
/// to the address column). The link begins after the panel's 1-cell border and
/// the 3-char row prefix (`" › "`), and is clipped to the panel's inner width so
/// it never overruns into the neighbouring columns — but `url` stays whole so a
/// click still opens the full address even when the visible text is shortened.
fn record_link(
    app: &App,
    area: ratatui::layout::Rect,
    row: usize,
    shown: &str,
    url: String,
    sel: bool,
    focused: bool,
) {
    let x = area.x + 4;
    let y = area.y + 1 + row as u16;
    let last_inner = area.x + area.width.saturating_sub(2);
    if x > last_inner {
        return;
    }
    let avail = (last_inner - x + 1) as usize;
    let text: String = shown.chars().take(avail).collect();
    if text.is_empty() {
        return;
    }
    app.links.borrow_mut().push(HyperLink {
        x,
        y,
        text,
        url,
        reversed: sel && focused,
        bold: sel && !focused,
    });
}

/// Shorten `s` to `w` columns, marking the cut with an ellipsis.
fn ellipsize(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(w - 1).collect();
    out.push('…');
    out
}

/// Width of the address column shared by the Vhosts and Parked panels, sized to
/// the widest address across both lists so every row's `server`/`php`/`path`
/// starts at the same column. A fixed width instead let any longer host shove
/// the rest of its row right, which is what put the columns out of step. Capped
/// so one outlier hostname can't squeeze the path column, and floored at the
/// width the panels have always used so a short list still looks the same.
fn url_col_width(app: &App, area_w: u16) -> usize {
    const MIN: usize = 30;
    const MAX: usize = 46;
    // Row prefix, the server and php columns, their separators, and enough left
    // over for a useful slice of the path.
    const OVERHEAD: usize = 3 + 1 + 8 + 1 + 5 + 1 + 20;
    let room = (area_w.saturating_sub(2) as usize).saturating_sub(OVERHEAD);
    let widest = app
        .state
        .vhosts
        .iter()
        .map(|v| vhost_url(app, v).chars().count())
        .chain(
            app.parked_vhosts
                .iter()
                .map(|v| parked_url(v).chars().count()),
        )
        .max()
        .unwrap_or(0);
    widest.clamp(MIN, MAX.min(room.max(MIN)))
}

/// The address a parked folder is reachable at.
fn parked_url(v: &crate::state::Vhost) -> String {
    let scheme = if v.ssl { "https" } else { "http" };
    format!("{scheme}://{}", v.server_name)
}

/// First visible row index for a list of `len` rows in a `view_h`-tall panel,
/// chosen to keep `sel` on screen (centered when scrolling is needed). Returns
/// 0 when everything fits.
fn window_offset(sel: usize, len: usize, view_h: usize) -> usize {
    let view_h = view_h.max(1);
    if len <= view_h {
        return 0;
    }
    let half = view_h / 2;
    sel.saturating_sub(half).min(len - view_h)
}

/// Render the declared vhosts (scrolling to keep the selection visible) and
/// return the first visible row index for mouse hit-testing.
fn render_vhosts(f: &mut Frame, app: &App, area: ratatui::layout::Rect) -> usize {
    let focused = app.focus == Panel::Vhosts;
    let view_h = area.height.saturating_sub(2) as usize;
    let len = app.state.vhosts.len();
    let off = window_offset(app.sel_vhost, len, view_h);
    let url_w = url_col_width(app, area.width);
    let mut lines: Vec<Line> = Vec::new();
    if len == 0 {
        lines.push(Line::from(Span::styled(
            "  no vhosts — press 'n' to create one",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (i, v) in app.state.vhosts.iter().enumerate().skip(off).take(view_h) {
        let sel = i == app.sel_vhost;
        let url = vhost_url(app, v);
        let shown = ellipsize(&url, url_w);
        // Recorded as an OSC 8 hyperlink (stamped after draw) so the URL is
        // click-to-open in capable terminals, not just auto-linkified text.
        record_link(app, area, i - off, &shown, url, sel, focused);
        // Underline only the address itself, not the padding out to the column
        // width, so it lines up with the terminal's own OSC 8 underline.
        let pad = url_w.saturating_sub(shown.chars().count());
        lines.push(
            Line::from(vec![
                Span::raw(format!(" {} ", if sel && focused { "›" } else { " " })),
                Span::styled(
                    shown,
                    Style::default()
                        .fg(ACCENT)
                        .add_modifier(Modifier::UNDERLINED),
                ),
                Span::raw(format!(
                    "{:pad$} {:<8} {:<5} {}",
                    "",
                    v.server,
                    v.php_version,
                    app.anon(&v.docroot)
                )),
            ])
            .style(row_style(sel, focused)),
        );
    }
    let cols = ["host", "server", "php", "path"];
    let arrow = if app.vhost_sort_asc { "▲" } else { "▼" };
    let title = format!(
        "Vhosts  [1-4 sort: {}{}]",
        cols.get(app.vhost_sort_col).copied().unwrap_or("host"),
        arrow
    );
    f.render_widget(
        Paragraph::new(lines).block(panel_block(&title, focused)),
        area,
    );
    off
}

/// Render the parked sites (derived from `state.parks`) as their own scrollable
/// panel. Read-only — selection is for inspection/scrolling. Returns the first
/// visible row index for mouse hit-testing.
fn render_parked(f: &mut Frame, app: &App, area: ratatui::layout::Rect) -> usize {
    let focused = app.focus == Panel::Parked;
    let view_h = area.height.saturating_sub(2) as usize;
    let len = app.parked_vhosts.len();
    let off = window_offset(app.sel_parked, len, view_h);
    let url_w = url_col_width(app, area.width);
    let mut lines: Vec<Line> = Vec::new();
    if len == 0 {
        lines.push(Line::from(Span::styled(
            "  no parked directories — press 'p' to park one (e.g. ~/Sites → *.test)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (i, v) in app.parked_vhosts.iter().enumerate().skip(off).take(view_h) {
        let sel = i == app.sel_parked;
        let url = parked_url(v);
        let shown = ellipsize(&url, url_w);
        record_link(app, area, i - off, &shown, url, sel, focused);
        let pad = url_w.saturating_sub(shown.chars().count());
        lines.push(
            Line::from(vec![
                Span::raw(format!(" {} ", if sel && focused { "›" } else { " " })),
                Span::styled(
                    shown,
                    Style::default()
                        .fg(ACCENT)
                        .add_modifier(Modifier::UNDERLINED),
                ),
                Span::raw(format!(
                    "{:pad$} {:<8} {:<5} {}",
                    "",
                    v.server,
                    v.php_version,
                    app.anon(&v.docroot)
                )),
            ])
            .style(row_style(sel, focused)),
        );
    }
    // Show the window position when the list is taller than the panel.
    let mut title = if len > view_h {
        format!("Parked  ({len})  [{}–{} of {len}]", off + 1, off + view_h)
    } else {
        format!("Parked  ({len})")
    };
    // Folders added since the last apply are listed here (the list is read from
    // disk) but aren't in any server's config, so nothing serves them — and on a
    // server with a catch-all default site they answer with the sites root
    // rather than failing, which reads as working. Call them out.
    if app.parked_unapplied > 0 {
        title.push_str(&format!(
            "  [{} not applied — press 'a']",
            app.parked_unapplied
        ));
    }
    f.render_widget(
        Paragraph::new(lines).block(panel_block(&title, focused)),
        area,
    );
    off
}

fn render_status(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {}", app.anon(&app.message)),
            Style::default().fg(Color::Gray),
        ))
        .alignment(Alignment::Left),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::ellipsize;

    #[test]
    fn ellipsize_only_shortens_what_overruns() {
        assert_eq!(ellipsize("https://short.test", 30), "https://short.test");
        assert_eq!(ellipsize("abcdef", 6), "abcdef");
        assert_eq!(ellipsize("abcdef", 5), "abcd…");
        assert_eq!(ellipsize("abcdef", 1), "…");
        assert_eq!(ellipsize("abcdef", 0), "");
        // Column width is in characters, so multi-byte hosts don't over-cut.
        assert_eq!(ellipsize("héllo-wörld", 6).chars().count(), 6);
    }
}
