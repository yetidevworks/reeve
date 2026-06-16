//! ratatui dashboard — stacked Servers / PHP / Vhosts panels with live status
//! and action keys. Shares all lifecycle logic with the CLI via `crate::ops`.

mod pathpick;
mod ui;

use crate::backends::settings_defs;
use crate::brew::Brew;
use crate::config::{load_config, save_config, Config};
use crate::daemon::{self, Status};
use crate::doctor::{self, Health};
use crate::logs;
use crate::ops;
use crate::php;
use crate::state::{load_state, Backend, Framework, State, XdebugMode};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::{self, Stdout};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Servers,
    Php,
    Vhosts,
    Services,
}

pub struct App {
    pub config: Config,
    pub state: State,
    pub focus: Panel,
    pub sel_server: usize,
    pub sel_php: usize,
    pub sel_vhost: usize,
    pub sel_service: usize,
    pub server_status: Vec<Status>,
    pub php_status: Vec<Status>,
    pub service_status: Vec<Status>,
    /// Parked-directory sites expanded from `state.parks` (read-only, derived).
    pub parked_vhosts: Vec<crate::state::Vhost>,
    pub message: String,
    pub wizard: Option<VhostWizard>,
    /// When set, a "remove vhost <name>?" confirmation is showing.
    pub confirm_remove: Option<String>,
    /// Server-edit modal (ports/backend), when open.
    pub server_wizard: Option<ServerWizard>,
    /// Per-backend settings modal, when open.
    pub settings_modal: Option<SettingsModal>,
    /// PHP-install input modal, when open.
    pub php_install: Option<PhpInstallModal>,
    /// When set, a "remove PHP <version>?" confirmation is showing.
    pub confirm_remove_php: Option<String>,
    /// A PHP version queued for install — run_loop suspends the TUI, installs
    /// it (showing brew output), then resumes. Avoids a frozen screen.
    pub pending_install: Option<String>,
    /// Whether `*.tld` system resolution is configured (/etc/resolver present).
    pub dns_ok: bool,
    /// "remove server <name>?" confirmation, when showing.
    pub confirm_remove_server: Option<String>,
    /// PHP extensions manager modal, when open.
    pub ext_modal: Option<ExtModal>,
    /// A queued (slow) pecl add/remove — run_loop suspends the TUI to show output.
    pub pending_ext: Option<PendingExt>,
    /// Global preferences (local TLD, sites root, default backend) modal.
    pub config_modal: Option<ConfigModal>,
    /// Per-version PHP settings modal (php.ini / OPcache / FPM pool), when open.
    pub php_settings: Option<PhpSettingsModal>,
    /// A queued Xdebug enable that needs a (slow) pecl install — run_loop
    /// suspends the TUI to show output, like `pending_ext`.
    pub pending_xdebug: Option<(String, XdebugMode)>,
    /// Service-picker modal (choose a kind to add), when open.
    pub service_picker: Option<ServicePicker>,
    /// A queued service start that may need a (slow) brew install — run_loop
    /// suspends the TUI to show output.
    pub pending_service: Option<crate::state::ServiceKind>,
    /// A queued server start whose backend needs a (slow) brew install.
    pub pending_server: Option<String>,
    /// Log-viewer modal (read-only tail of a service log), when open.
    pub log_modal: Option<LogModal>,
    /// Worst-of health across all diagnostics, shown in the title bar.
    pub health: Health,
    /// Set after an action that may have disturbed the terminal (e.g. the admin
    /// dialog); run_loop does a full repaint to clear any leftover artifacts.
    force_clear: bool,
    should_quit: bool,
}

/// Read-only log viewer for a single service's log file.
pub struct LogModal {
    pub label: String,
    pub lines: Vec<String>,
    /// First visible line index (scroll offset).
    pub scroll: usize,
}

/// Trailing lines to load into the log viewer.
pub const LOG_TAIL_LINES: usize = 500;

/// Global preferences modal. Edits `config.toml` (local TLD, sites root,
/// default backend) — distinct from per-server [`SettingsModal`].
pub struct ConfigModal {
    pub tld: String,
    pub sites_root: String,
    pub backend_idx: usize,
    /// 0 = TLD, 1 = sites root, 2 = default backend.
    pub field: usize,
    pub error: Option<String>,
    /// TLD as saved on open, so we can warn when it changes (DNS needs re-setup).
    pub orig_tld: String,
}

/// Editable fields in the global config modal.
pub const CONFIG_FIELDS: usize = 3;

/// Modal to type a PHP version to install.
pub struct PhpInstallModal {
    pub version: String,
    pub error: Option<String>,
}

/// PHP extensions manager modal for one version.
pub struct ExtModal {
    pub version: String,
    /// Currently-loaded extensions (`php -m`), sorted.
    pub loaded: Vec<String>,
    /// Text being typed to add a new extension.
    pub input: String,
    /// Highlighted entry in `loaded` (used when `input` is empty).
    pub sel: usize,
    pub error: Option<String>,
}

/// A queued pecl operation, run outside the alternate screen (it's slow).
pub enum ExtAction {
    Add(String),
    Remove(String),
}
pub struct PendingExt {
    pub version: String,
    pub action: ExtAction,
}

/// Per-backend settings modal state. `values` parallels
/// `backends::settings_defs(backend)`.
pub struct SettingsModal {
    pub server_name: String,
    pub backend: Backend,
    pub values: Vec<String>,
    pub field: usize,
    pub error: Option<String>,
}

/// Service-picker modal: choose a service kind to add + start. `sel` indexes
/// `ServiceKind::all()`.
pub struct ServicePicker {
    pub sel: usize,
}

/// Per-version PHP settings modal state. `values` parallels
/// `php::php_settings_defs()`.
pub struct PhpSettingsModal {
    pub version: String,
    pub values: Vec<String>,
    pub field: usize,
    pub error: Option<String>,
}

/// Backends in selector order.
pub const BACKENDS: [Backend; 4] = [
    Backend::Caddy,
    Backend::Apache,
    Backend::Nginx,
    Backend::Ols,
];
/// Editable fields in the server modal (Backend, HTTP, HTTPS, Default site).
pub const SERVER_FIELDS: usize = 4;

/// Modal form state for editing a server's ports/backend.
pub struct ServerWizard {
    pub backend_idx: usize,
    pub http: String,
    pub https: String,
    /// Serve a catch-all default site on the HTTP port.
    pub default_site: bool,
    pub field: usize,
    pub error: Option<String>,
    /// `Some(orig)` when editing an existing server; `None` when creating one.
    pub editing: Option<String>,
    /// Whether each backend (parallel to `BACKENDS`) has its brew formula present.
    pub installed: [bool; 4],
}

/// Number of editable fields in the new-vhost wizard
/// (host, docroot, php, server, ssl, preset).
pub const WIZARD_FIELDS: usize = 6;

/// Modal form state for creating a vhost.
pub struct VhostWizard {
    pub server_name: String,
    pub docroot: String,
    pub php_idx: usize,
    pub server_idx: usize,
    pub ssl: bool,
    pub field: usize,
    pub error: Option<String>,
    /// `Some(original_host)` when editing an existing vhost; `None` when creating.
    pub editing: Option<String>,
    /// Whether the Doc root autocomplete dropdown is open (captures ↑/↓).
    pub dropdown_open: bool,
    /// Highlighted entry within the open dropdown.
    pub path_sel: Option<usize>,
    /// Framework preset (field 5).
    pub preset: Framework,
    /// Proxy target carried through edits (set only via the CLI `--proxy`).
    pub proxy_target: Option<String>,
}

impl VhostWizard {
    /// Live filesystem suggestions for the current `docroot` text (capped).
    pub fn path_suggestions(&self) -> Vec<pathpick::PathEntry> {
        pathpick::read_dir_filtered(&self.docroot, PATH_SUGGESTION_CAP)
    }

    /// True when the dropdown should capture ↑/↓ (Doc root field + open).
    fn dropdown_active(&self) -> bool {
        self.field == FIELD_DOCROOT && self.dropdown_open
    }

    /// Move to a field; landing on Doc root (re)opens its dropdown.
    fn goto_field(&mut self, field: usize) {
        self.field = field;
        self.path_sel = None;
        if field == FIELD_DOCROOT {
            self.dropdown_open = true;
        }
    }

    fn next_field(&mut self) {
        self.goto_field((self.field + 1) % WIZARD_FIELDS);
    }

    fn prev_field(&mut self) {
        self.goto_field((self.field + WIZARD_FIELDS - 1) % WIZARD_FIELDS);
    }
}

/// Max entries shown in the Doc root dropdown.
pub const PATH_SUGGESTION_CAP: usize = 8;

impl App {
    fn load() -> Result<Self> {
        let mut app = App {
            config: load_config().unwrap_or_default(),
            state: State::default(),
            focus: Panel::Servers,
            sel_server: 0,
            sel_php: 0,
            sel_vhost: 0,
            sel_service: 0,
            server_status: Vec::new(),
            php_status: Vec::new(),
            service_status: Vec::new(),
            parked_vhosts: Vec::new(),
            message: "ready".into(),
            wizard: None,
            confirm_remove: None,
            server_wizard: None,
            settings_modal: None,
            php_install: None,
            confirm_remove_php: None,
            pending_install: None,
            dns_ok: false,
            confirm_remove_server: None,
            ext_modal: None,
            pending_ext: None,
            config_modal: None,
            php_settings: None,
            pending_xdebug: None,
            service_picker: None,
            pending_service: None,
            pending_server: None,
            log_modal: None,
            health: Health::Ok,
            force_clear: false,
            should_quit: false,
        };
        app.refresh();
        Ok(app)
    }

    /// Reload state from disk and recompute live launchd statuses.
    fn refresh(&mut self) {
        self.state = load_state().unwrap_or_default();
        self.config = load_config().unwrap_or_default();
        self.server_status = self
            .state
            .servers
            .iter()
            .map(|s| daemon::status(&crate::backends::server_service_id(s)))
            .collect();
        self.php_status = self
            .state
            .php_versions
            .iter()
            .map(|p| daemon::status(&php::service_id(&p.version)))
            .collect();
        self.service_status = self
            .state
            .services
            .iter()
            .map(|s| daemon::status(&crate::services::service_id(s.kind)))
            .collect();
        self.parked_vhosts = crate::park::expand_all(&self.state);
        self.dns_ok = crate::dns::resolver_ok_all(&self.config.local_tlds);
        let brew = Brew::detect().ok();
        self.health = doctor::summary(&doctor::run(brew.as_ref(), &self.config, &self.state));
        self.clamp();
    }

    fn clamp(&mut self) {
        self.sel_server = self
            .sel_server
            .min(self.state.servers.len().saturating_sub(1));
        self.sel_php = self
            .sel_php
            .min(self.state.php_versions.len().saturating_sub(1));
        self.sel_vhost = self
            .sel_vhost
            .min(self.state.vhosts.len().saturating_sub(1));
        self.sel_service = self
            .sel_service
            .min(self.state.services.len().saturating_sub(1));
    }

    fn focus_len(&self) -> usize {
        match self.focus {
            Panel::Servers => self.state.servers.len(),
            Panel::Php => self.state.php_versions.len(),
            Panel::Vhosts => self.state.vhosts.len(),
            Panel::Services => self.state.services.len(),
        }
    }

    fn sel_mut(&mut self) -> &mut usize {
        match self.focus {
            Panel::Servers => &mut self.sel_server,
            Panel::Php => &mut self.sel_php,
            Panel::Vhosts => &mut self.sel_vhost,
            Panel::Services => &mut self.sel_service,
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let len = self.focus_len();
        if len == 0 {
            return;
        }
        let sel = self.sel_mut();
        let new = (*sel as isize + delta).rem_euclid(len as isize) as usize;
        *sel = new;
    }

    fn cycle_focus(&mut self, forward: bool) {
        // Visual order top→bottom: Servers, Vhosts, PHP, Services.
        self.focus = match (self.focus, forward) {
            (Panel::Servers, true) => Panel::Vhosts,
            (Panel::Vhosts, true) => Panel::Php,
            (Panel::Php, true) => Panel::Services,
            (Panel::Services, true) => Panel::Servers,
            (Panel::Servers, false) => Panel::Services,
            (Panel::Services, false) => Panel::Php,
            (Panel::Php, false) => Panel::Vhosts,
            (Panel::Vhosts, false) => Panel::Servers,
        };
    }

    /// Run an action, capturing success/error into the message line instead of
    /// tearing down the TUI.
    fn act(&mut self, label: &str, result: Result<String>) {
        self.message = match result {
            Ok(msg) => format!("✓ {msg}"),
            Err(e) => format!("✗ {label}: {e}"),
        };
        self.refresh();
    }

    fn selected_server_name(&self) -> Option<String> {
        self.state
            .servers
            .get(self.sel_server)
            .map(|s| s.name.clone())
    }

    fn selected_php_version(&self) -> Option<String> {
        self.state
            .php_versions
            .get(self.sel_php)
            .map(|p| p.version.clone())
    }

    fn selected_service(&self) -> Option<crate::state::ServiceKind> {
        self.state.services.get(self.sel_service).map(|s| s.kind)
    }

    fn selected_vhost_name(&self) -> Option<String> {
        self.state
            .vhosts
            .get(self.sel_vhost)
            .map(|v| v.server_name.clone())
    }
}

/// Render a single dashboard frame to plain text using ratatui's TestBackend.
/// Lets us verify TUI layout + data binding without a real terminal.
pub fn snapshot(width: u16, height: u16, modal: &str) -> Result<String> {
    let mut app = App::load()?;
    match modal {
        "wizard" => open_wizard_for_snapshot(&mut app),
        "server" => open_edit_server(&mut app),
        "newserver" => open_new_server(&mut app),
        "settings" => open_settings(&mut app),
        "php" => {
            app.php_install = Some(PhpInstallModal {
                version: "8.".into(),
                error: None,
            })
        }
        "ext" => open_ext_modal(&mut app),
        "phpsettings" => open_php_settings(&mut app),
        "service" => app.service_picker = Some(ServicePicker { sel: 0 }),
        "config" => open_config_modal(&mut app),
        "log" => {
            app.log_modal = Some(LogModal {
                label: "server-caddy".into(),
                lines: vec![
                    "[caddy] serving on :80".into(),
                    "[caddy] tls handshake ok".into(),
                ],
                scroll: 0,
            })
        }
        _ => {}
    }
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| ui::render(f, &app))?;
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    Ok(out)
}

pub async fn run() -> Result<()> {
    if load_config().is_err() {
        println!("reeve is not configured. Run `reeve init` first.");
        return Ok(());
    }

    let mut terminal = setup_terminal()?;
    let mut app = App::load()?;
    let res = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    res
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        if app.force_clear {
            terminal.clear()?;
            app.force_clear = false;
        }
        terminal.draw(|f| ui::render(f, app))?;

        // Poll with a timeout so statuses refresh even without keypresses.
        if event::poll(Duration::from_millis(2000))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key.code, key.modifiers);
                }
            }
        } else {
            app.refresh();
        }

        // A queued PHP install runs outside the alternate screen so brew's
        // (long) output is visible, then we resume the dashboard.
        if let Some(ver) = app.pending_install.take() {
            run_install_suspended(terminal, app, &ver)?;
        }

        // A queued extension add/remove (pecl is slow) runs the same way.
        if let Some(pe) = app.pending_ext.take() {
            run_ext_suspended(terminal, app, pe)?;
        }

        // Enabling Xdebug may need to build it via pecl — same suspend treatment.
        if let Some((ver, mode)) = app.pending_xdebug.take() {
            run_xdebug_suspended(terminal, app, &ver, mode)?;
        }

        // Starting a service may need a (slow) brew install — same treatment.
        if let Some(kind) = app.pending_service.take() {
            run_service_start_suspended(terminal, app, kind)?;
        }

        // Starting a server may need to install its backend — same treatment.
        if let Some(name) = app.pending_server.take() {
            run_server_start_suspended(terminal, app, &name)?;
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    // Modals capture all input while open.
    if app.confirm_remove.is_some() {
        handle_confirm_key(app, code);
        return;
    }
    if app.wizard.is_some() {
        handle_wizard_key(app, code);
        return;
    }
    if app.server_wizard.is_some() {
        handle_server_wizard_key(app, code);
        return;
    }
    if app.settings_modal.is_some() {
        handle_settings_key(app, code);
        return;
    }
    if app.php_install.is_some() {
        handle_php_install_key(app, code);
        return;
    }
    if app.confirm_remove_php.is_some() {
        handle_php_confirm_key(app, code);
        return;
    }
    if app.confirm_remove_server.is_some() {
        handle_server_confirm_key(app, code);
        return;
    }
    if app.ext_modal.is_some() {
        handle_ext_key(app, code);
        return;
    }
    if app.config_modal.is_some() {
        handle_config_key(app, code);
        return;
    }
    if app.php_settings.is_some() {
        handle_php_settings_key(app, code);
        return;
    }
    if app.service_picker.is_some() {
        handle_service_picker_key(app, code);
        return;
    }
    if app.log_modal.is_some() {
        handle_log_key(app, code);
        return;
    }
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => app.should_quit = true,
        KeyCode::Tab => app.cycle_focus(true),
        KeyCode::BackTab => app.cycle_focus(false),
        // Any arrow (or hjkl) moves the selection within the focused panel.
        // Left/Right matter for the horizontal PHP row; Up/Down for the lists.
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Left | KeyCode::Char('h') => app.move_sel(-1),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Right | KeyCode::Char('l') => app.move_sel(1),
        KeyCode::Enter => match app.focus {
            // Enter activates: starts a server / restarts a PHP FPM master.
            Panel::Servers => start_selected_server(app),
            Panel::Php => restart_selected_fpm(app),
            Panel::Vhosts => {}
            Panel::Services => start_selected_service(app),
        },
        KeyCode::Char('s') if app.focus == Panel::Servers => open_settings(app),
        KeyCode::Char('s') if app.focus == Panel::Php => open_php_settings(app),
        KeyCode::Char('x') if app.focus == Panel::Servers => {
            if let Some(name) = app.selected_server_name() {
                let r = ops::stop_server(&name).map(|_| format!("stopped '{name}'"));
                app.act("stop", r);
            }
        }
        KeyCode::Char('x') if app.focus == Panel::Php => toggle_xdebug(app),
        KeyCode::Char('x') if app.focus == Panel::Services => {
            if let Some(kind) = app.selected_service() {
                let r = ops::stop_service(kind).map(|_| format!("stopped '{kind}'"));
                app.act("stop", r);
            }
        }
        KeyCode::Char('r') => match app.focus {
            Panel::Servers => {
                if let Some(name) = app.selected_server_name() {
                    let r = ops::restart_server(&name)
                        .map(|st| format!("restarted '{name}' — {}", st.as_str()));
                    app.act("restart", r);
                }
            }
            Panel::Php => {
                // Context-aware: 'r' removes (unmanages) the selected PHP version.
                if let Some(ver) = app.selected_php_version() {
                    app.confirm_remove_php = Some(ver);
                }
            }
            Panel::Vhosts => {
                // Context-aware: 'r' removes the selected vhost (with confirm).
                if let Some(name) = app.selected_vhost_name() {
                    app.confirm_remove = Some(name);
                }
            }
            Panel::Services => {
                if let Some(kind) = app.selected_service() {
                    let r = ops::restart_service(kind)
                        .map(|st| format!("restarted '{kind}' — {}", st.as_str()));
                    app.act("restart", r);
                }
            }
        },
        KeyCode::Char('a') => {
            let r = apply_all().map(|n| format!("applied {n} server(s)"));
            app.act("apply", r);
        }
        KeyCode::Char('n') => match app.focus {
            // 'n' = new, scoped to the focused panel: server / vhost / PHP install.
            Panel::Servers => open_new_server(app),
            Panel::Vhosts => open_wizard(app),
            Panel::Php => {
                app.php_install = Some(PhpInstallModal {
                    version: String::new(),
                    error: None,
                })
            }
            Panel::Services => app.service_picker = Some(ServicePicker { sel: 0 }),
        },
        KeyCode::Char('v') => validate_all(app),
        KeyCode::Char('L') => open_log(app),
        KeyCode::Char('c') => open_config_modal(app),
        KeyCode::Delete | KeyCode::Backspace => match app.focus {
            // Remove the focused item (always behind a confirm).
            Panel::Servers => {
                if let Some(name) = app.selected_server_name() {
                    app.confirm_remove_server = Some(name);
                }
            }
            Panel::Vhosts => {
                if let Some(name) = app.selected_vhost_name() {
                    app.confirm_remove = Some(name);
                }
            }
            Panel::Php => {
                if let Some(ver) = app.selected_php_version() {
                    app.confirm_remove_php = Some(ver);
                }
            }
            Panel::Services => {
                if let Some(kind) = app.selected_service() {
                    let r = ops::remove_service(kind).map(|_| format!("removed '{kind}'"));
                    app.act("remove", r);
                }
            }
        },
        KeyCode::Char('d') if app.focus == Panel::Php => {
            if let Some(ver) = app.selected_php_version() {
                let r = ops::set_default_php(&ver).map(|_| format!("default PHP set to {ver}"));
                app.act("default", r);
            }
        }
        KeyCode::Char('e') => match app.focus {
            Panel::Vhosts => open_edit_wizard(app),
            Panel::Servers => open_edit_server(app),
            Panel::Php => open_ext_modal(app),
            Panel::Services => {}
        },
        KeyCode::Char('D') => {
            let tlds = app.config.local_tlds.clone();
            let list = tlds
                .iter()
                .map(|t| format!("*.{t}"))
                .collect::<Vec<_>>()
                .join(", ");
            app.message = "requesting admin access for DNS setup…".into();
            let r = Brew::detect()
                .and_then(|brew| crate::dns::setup(&brew, &tlds))
                .map(|ok| {
                    if ok {
                        format!("{list} now resolve system-wide")
                    } else {
                        "dnsmasq running but some /etc/resolver files not set".to_string()
                    }
                });
            app.act("dns setup", r);
            // The admin dialog can disturb the terminal — force a clean repaint.
            app.force_clear = true;
        }
        _ => {}
    }
}

/// Open the read-only log viewer for the focused row's service. Vhosts show
/// their owning server's log (vhosts don't have a log of their own).
fn open_log(app: &mut App) {
    let label = match app.focus {
        Panel::Servers => app.selected_server_name().map(|n| format!("server-{n}")),
        Panel::Php => app.selected_php_version().map(|v| format!("php-{v}")),
        Panel::Vhosts => app
            .state
            .vhosts
            .get(app.sel_vhost)
            .map(|v| format!("server-{}", v.server)),
        Panel::Services => app.selected_service().map(crate::services::service_id),
    };
    let Some(label) = label else {
        app.message = "nothing selected to show a log for".into();
        return;
    };
    match logs::resolve(&app.state, &label) {
        Ok(path) => {
            let lines = logs::tail_lines(&path, LOG_TAIL_LINES);
            app.log_modal = Some(LogModal {
                label,
                lines,
                scroll: 0,
            });
        }
        Err(e) => app.message = format!("✗ log: {e}"),
    }
}

/// Scroll / close the log viewer.
fn handle_log_key(app: &mut App, code: KeyCode) {
    let Some(m) = app.log_modal.as_mut() else {
        return;
    };
    let max = m.lines.len().saturating_sub(1);
    match code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('L') => app.log_modal = None,
        KeyCode::Up | KeyCode::Char('k') => m.scroll = m.scroll.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => m.scroll = (m.scroll + 1).min(max),
        KeyCode::PageUp => m.scroll = m.scroll.saturating_sub(20),
        KeyCode::PageDown => m.scroll = (m.scroll + 20).min(max),
        KeyCode::Home => m.scroll = 0,
        KeyCode::End => m.scroll = max,
        _ => {}
    }
}

/// Which backends (parallel to `BACKENDS`) have their brew formula installed.
fn backends_installed() -> [bool; 4] {
    let mut out = [false; 4];
    if let Ok(brew) = Brew::detect() {
        for (i, b) in BACKENDS.iter().enumerate() {
            out[i] = brew.is_installed(crate::backends::backend_for(*b).formula());
        }
    }
    out
}

/// Open the server-edit modal for the selected server.
fn open_edit_server(app: &mut App) {
    let Some(s) = app.state.servers.get(app.sel_server).cloned() else {
        return;
    };
    let backend_idx = BACKENDS.iter().position(|b| *b == s.backend).unwrap_or(0);
    app.server_wizard = Some(ServerWizard {
        backend_idx,
        http: s.http_port.to_string(),
        https: s.https_port.to_string(),
        default_site: s.default_site,
        field: 0,
        error: None,
        editing: Some(s.name),
        installed: backends_installed(),
    });
}

/// Open the new-server wizard with sane defaults (caddy on 80/443).
fn open_new_server(app: &mut App) {
    app.server_wizard = Some(ServerWizard {
        backend_idx: 0,
        http: "80".into(),
        https: "443".into(),
        default_site: false,
        field: 0,
        error: None,
        editing: None,
        installed: backends_installed(),
    });
}

/// Derive a unique instance name from the backend (caddy, caddy2, caddy3, …).
fn unique_server_name(state: &State, backend: Backend) -> String {
    let base = backend.to_string();
    if !state.servers.iter().any(|s| s.name == base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|cand| !state.servers.iter().any(|s| &s.name == cand))
        .unwrap_or(base)
}

fn handle_server_wizard_key(app: &mut App, code: KeyCode) {
    let w = app.server_wizard.as_mut().unwrap();
    match code {
        KeyCode::Esc => app.server_wizard = None,
        KeyCode::Enter => submit_server_wizard(app),
        KeyCode::Tab | KeyCode::Down => w.field = (w.field + 1) % SERVER_FIELDS,
        KeyCode::BackTab | KeyCode::Up => w.field = (w.field + SERVER_FIELDS - 1) % SERVER_FIELDS,
        KeyCode::Left if w.field == 0 => {
            w.backend_idx = (w.backend_idx + BACKENDS.len() - 1) % BACKENDS.len()
        }
        KeyCode::Right | KeyCode::Char(' ') if w.field == 0 => {
            w.backend_idx = (w.backend_idx + 1) % BACKENDS.len()
        }
        // Default-site toggle.
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if w.field == 3 => {
            w.default_site = !w.default_site
        }
        KeyCode::Backspace => match w.field {
            1 => {
                w.http.pop();
            }
            2 => {
                w.https.pop();
            }
            _ => {}
        },
        // Ports accept digits only.
        KeyCode::Char(c) if c.is_ascii_digit() => match w.field {
            1 => w.http.push(c),
            2 => w.https.push(c),
            _ => {}
        },
        _ => {}
    }
}

fn submit_server_wizard(app: &mut App) {
    let w = app.server_wizard.as_ref().unwrap();
    let editing = w.editing.clone();
    let backend = BACKENDS[w.backend_idx];
    let default_site = w.default_site;
    let http: Result<u16, _> = w.http.parse();
    let https: Result<u16, _> = w.https.parse();

    let set_err = |app: &mut App, msg: String| {
        if let Some(w) = app.server_wizard.as_mut() {
            w.error = Some(msg);
        }
    };

    let (Ok(http), Ok(https)) = (http, https) else {
        set_err(app, "Ports must be numbers (1-65535)".into());
        return;
    };
    if http == 0 || https == 0 {
        set_err(app, "Ports must be non-zero".into());
        return;
    }

    let result = (|| -> anyhow::Result<String> {
        let mut state = load_state()?;
        match &editing {
            // Edit: update the existing server (conflict-check against others).
            Some(name) => {
                for s in state.servers.iter().filter(|s| &s.name != name) {
                    if [s.http_port, s.https_port]
                        .iter()
                        .any(|p| *p == http || *p == https)
                    {
                        anyhow::bail!("port {http}/{https} conflicts with server '{}'", s.name);
                    }
                }
                let srv = state
                    .servers
                    .iter_mut()
                    .find(|s| &s.name == name)
                    .ok_or_else(|| anyhow::anyhow!("server '{name}' not found"))?;
                srv.backend = backend;
                srv.http_port = http;
                srv.https_port = https;
                srv.default_site = default_site;
                crate::state::save_state(&state)?;
                Ok(format!(
                    "updated '{name}' ({backend} :{http}/:{https}) — press 'r' to apply"
                ))
            }
            // Create: add a new instance (add_server checks dup name + port clash).
            None => {
                let name = unique_server_name(&state, backend);
                state.add_server(crate::state::Server {
                    name: name.clone(),
                    backend,
                    http_port: http,
                    https_port: https,
                    enabled: false,
                    default_site,
                    settings: Default::default(),
                })?;
                crate::state::save_state(&state)?;
                Ok(format!(
                    "added {backend} server '{name}' (:{http}/:{https}) — press enter to start"
                ))
            }
        }
    })();

    match result {
        Ok(msg) => {
            let creating = editing.is_none();
            app.server_wizard = None;
            app.message = format!("✓ {msg}");
            app.refresh();
            // Jump focus to the freshly-added server for an easy start.
            if creating && !app.state.servers.is_empty() {
                app.focus = Panel::Servers;
                app.sel_server = app.state.servers.len() - 1;
            }
        }
        Err(e) => set_err(app, e.to_string()),
    }
}

/// Confirm-key handler for "remove server <name>?".
fn handle_server_confirm_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            if let Some(name) = app.confirm_remove_server.take() {
                let r = remove_server(&name)
                    .map(|n| format!("removed server '{name}' (and {n} vhost(s))"));
                app.act("remove server", r);
            }
        }
        _ => app.confirm_remove_server = None,
    }
}

/// Stop + unmanage a server and drop it (with its vhosts) from state.
/// Returns the number of vhosts removed.
fn remove_server(name: &str) -> Result<usize> {
    let mut state = load_state()?;
    let Some(server) = state.servers.iter().find(|s| s.name == name).cloned() else {
        anyhow::bail!("server '{name}' not found");
    };
    // Tear down its launchd service (best-effort; it may already be stopped).
    daemon::uninstall(&crate::backends::server_service_id(&server)).ok();
    state.servers.retain(|s| s.name != name);
    let before = state.vhosts.len();
    state.vhosts.retain(|v| v.server != name);
    let removed = before - state.vhosts.len();
    crate::state::save_state(&state)?;
    Ok(removed)
}

/// Run every server's native config test, summarizing into the message line.
fn validate_all(app: &mut App) {
    let result = (|| -> anyhow::Result<String> {
        let brew = Brew::detect()?;
        let state = load_state()?;
        if state.servers.is_empty() {
            return Ok("no servers to validate".into());
        }
        let mut failures = Vec::new();
        for s in &state.servers {
            if let Err(e) = crate::backends::backend_for(s.backend).validate(s, &brew) {
                let first = e
                    .to_string()
                    .lines()
                    .next()
                    .unwrap_or("invalid")
                    .to_string();
                failures.push(format!("{}: {first}", s.name));
            }
        }
        if failures.is_empty() {
            Ok(format!(
                "all {} server config(s) valid",
                state.servers.len()
            ))
        } else {
            anyhow::bail!("{}", failures.join("; "))
        }
    })();
    app.act("validate", result);
}

/// Open the PHP extensions manager for the selected version.
fn open_ext_modal(app: &mut App) {
    let Some(ver) = app.selected_php_version() else {
        app.message = "No PHP version selected".into();
        return;
    };
    match Brew::detect().and_then(|brew| php::extensions::list(&brew, &ver)) {
        Ok(mut loaded) => {
            loaded.sort_by_key(|s| s.to_lowercase());
            app.ext_modal = Some(ExtModal {
                version: ver,
                loaded,
                input: String::new(),
                sel: 0,
                error: None,
            });
        }
        Err(e) => app.message = format!("✗ extensions: {e}"),
    }
}

/// Key handling for the extensions modal: type a name + Enter to add; with the
/// input empty, ↑↓ select and Enter/Del remove the highlighted extension.
fn handle_ext_key(app: &mut App, code: KeyCode) {
    let m = app.ext_modal.as_mut().unwrap();
    m.error = None;
    let queue_remove = |app: &mut App| {
        let m = app.ext_modal.as_ref().unwrap();
        if let Some(name) = m.loaded.get(m.sel).cloned() {
            let ver = m.version.clone();
            app.ext_modal = None;
            app.pending_ext = Some(PendingExt {
                version: ver,
                action: ExtAction::Remove(name),
            });
        }
    };
    match code {
        KeyCode::Esc => app.ext_modal = None,
        KeyCode::Up => m.sel = m.sel.saturating_sub(1),
        KeyCode::Down if !m.loaded.is_empty() => {
            m.sel = (m.sel + 1).min(m.loaded.len() - 1);
        }
        KeyCode::Delete => queue_remove(app),
        KeyCode::Enter => {
            let name = m.input.trim().to_string();
            if name.is_empty() {
                // No text typed → remove the highlighted loaded extension.
                queue_remove(app);
            } else if m.loaded.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
                m.error = Some(format!("{name} is already loaded"));
            } else {
                let ver = m.version.clone();
                app.ext_modal = None;
                app.pending_ext = Some(PendingExt {
                    version: ver,
                    action: ExtAction::Add(name),
                });
            }
        }
        KeyCode::Backspace => {
            m.input.pop();
        }
        KeyCode::Char(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-' => m.input.push(c),
        _ => {}
    }
}

/// Open the global preferences modal (local TLD, sites root, default backend).
fn open_config_modal(app: &mut App) {
    let cfg = &app.config;
    let backend_idx = BACKENDS
        .iter()
        .position(|b| b.as_str() == cfg.default_backend)
        .unwrap_or(0);
    let tld = cfg.local_tlds.join(" ");
    app.config_modal = Some(ConfigModal {
        tld: tld.clone(),
        sites_root: cfg.sites_root.clone(),
        backend_idx,
        field: 0,
        error: None,
        orig_tld: tld,
    });
}

fn handle_config_key(app: &mut App, code: KeyCode) {
    let m = app.config_modal.as_mut().unwrap();
    match code {
        KeyCode::Esc => app.config_modal = None,
        KeyCode::Enter => submit_config(app),
        KeyCode::Tab | KeyCode::Down => m.field = (m.field + 1) % CONFIG_FIELDS,
        KeyCode::BackTab | KeyCode::Up => m.field = (m.field + CONFIG_FIELDS - 1) % CONFIG_FIELDS,
        KeyCode::Left if m.field == 2 => {
            m.backend_idx = (m.backend_idx + BACKENDS.len() - 1) % BACKENDS.len()
        }
        KeyCode::Right if m.field == 2 => m.backend_idx = (m.backend_idx + 1) % BACKENDS.len(),
        KeyCode::Backspace => match m.field {
            0 => {
                m.tld.pop();
            }
            1 => {
                m.sites_root.pop();
            }
            _ => {}
        },
        KeyCode::Char(c) => match m.field {
            // TLDs: DNS labels separated by space or comma (e.g. "test lan localhost").
            0 if c.is_ascii_alphanumeric() || c == '-' || c == ' ' || c == ',' => {
                m.tld.push(c.to_ascii_lowercase())
            }
            1 => m.sites_root.push(c),
            2 if c == ' ' => m.backend_idx = (m.backend_idx + 1) % BACKENDS.len(),
            _ => {}
        },
        _ => {}
    }
}

fn submit_config(app: &mut App) {
    let m = app.config_modal.as_ref().unwrap();
    let raw = m.tld.clone();
    let sites_root = m.sites_root.trim().to_string();
    let backend = BACKENDS[m.backend_idx];
    let orig_tld = m.orig_tld.clone();

    let set_err = |app: &mut App, msg: String| {
        if let Some(m) = app.config_modal.as_mut() {
            m.error = Some(msg);
        }
    };

    // Parse space/comma-separated TLDs, each a clean DNS label.
    let mut tlds = Vec::new();
    for tok in raw.split([' ', ',']).filter(|t| !t.is_empty()) {
        let t = tok.trim_matches('.').to_lowercase();
        if t.is_empty() {
            continue;
        }
        if !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            set_err(
                app,
                format!("'{tok}' is not a valid TLD (letters/digits/hyphens)"),
            );
            return;
        }
        if !tlds.contains(&t) {
            tlds.push(t);
        }
    }
    if tlds.is_empty() {
        set_err(
            app,
            "Enter at least one TLD (e.g. test lan localhost)".into(),
        );
        return;
    }
    if sites_root.is_empty() {
        set_err(app, "Sites root is required".into());
        return;
    }

    let changed = tlds.join(" ") != orig_tld;
    let result = (|| -> anyhow::Result<()> {
        let mut cfg = load_config()?;
        cfg.local_tlds = tlds.clone();
        cfg.sites_root = sites_root.clone();
        cfg.default_backend = backend.as_str().to_string();
        save_config(&cfg)
    })();

    match result {
        Ok(()) => {
            app.config_modal = None;
            let list = tlds
                .iter()
                .map(|t| format!(".{t}"))
                .collect::<Vec<_>>()
                .join(" ");
            app.message = if changed {
                format!("✓ config saved — TLDs: {list}; press D to set up DNS")
            } else {
                "✓ config saved".into()
            };
            app.refresh();
        }
        Err(e) => set_err(app, e.to_string()),
    }
}

/// Open the per-backend Settings modal for the selected server.
fn open_settings(app: &mut App) {
    let Some(s) = app.state.servers.get(app.sel_server).cloned() else {
        return;
    };
    let defs = settings_defs(s.backend);
    if defs.is_empty() {
        app.message = format!("{} has no tunable settings yet", s.backend);
        return;
    }
    let values = defs
        .iter()
        .map(|d| s.setting(d.key, d.default).to_string())
        .collect();
    app.settings_modal = Some(SettingsModal {
        server_name: s.name,
        backend: s.backend,
        values,
        field: 0,
        error: None,
    });
}

fn handle_settings_key(app: &mut App, code: KeyCode) {
    let n = app
        .settings_modal
        .as_ref()
        .map(|m| m.values.len())
        .unwrap_or(0)
        .max(1);
    let m = app.settings_modal.as_mut().unwrap();
    match code {
        KeyCode::Esc => app.settings_modal = None,
        KeyCode::Enter => submit_settings(app),
        KeyCode::Tab | KeyCode::Down => m.field = (m.field + 1) % n,
        KeyCode::BackTab | KeyCode::Up => m.field = (m.field + n - 1) % n,
        KeyCode::Backspace => {
            if let Some(v) = m.values.get_mut(m.field) {
                v.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(v) = m.values.get_mut(m.field) {
                v.push(c);
            }
        }
        _ => {}
    }
}

fn submit_settings(app: &mut App) {
    let m = app.settings_modal.as_ref().unwrap();
    let name = m.server_name.clone();
    let defs = settings_defs(m.backend);
    let values = m.values.clone();

    let result = (|| -> anyhow::Result<()> {
        let mut state = load_state()?;
        let srv = state
            .servers
            .iter_mut()
            .find(|s| s.name == name)
            .ok_or_else(|| anyhow::anyhow!("server '{name}' not found"))?;
        for (def, val) in defs.iter().zip(values.iter()) {
            let val = val.trim();
            // Store only non-default values to keep state minimal.
            if val.is_empty() || val == def.default {
                srv.settings.remove(def.key);
            } else {
                srv.settings.insert(def.key.to_string(), val.to_string());
            }
        }
        crate::state::save_state(&state)
    })();

    match result {
        Ok(()) => {
            app.settings_modal = None;
            app.message = format!("✓ saved settings for '{name}' — press 'r' to apply");
            app.refresh();
        }
        Err(e) => {
            if let Some(m) = app.settings_modal.as_mut() {
                m.error = Some(e.to_string());
            }
        }
    }
}

/// Open the per-version php.ini / OPcache / FPM settings modal.
fn open_php_settings(app: &mut App) {
    let Some(p) = app.state.php_versions.get(app.sel_php).cloned() else {
        return;
    };
    let values = php::php_settings_defs()
        .iter()
        .map(|d| p.setting(d.key, d.default).to_string())
        .collect();
    app.php_settings = Some(PhpSettingsModal {
        version: p.version,
        values,
        field: 0,
        error: None,
    });
}

fn handle_php_settings_key(app: &mut App, code: KeyCode) {
    let n = php::php_settings_defs().len().max(1);
    let m = app.php_settings.as_mut().unwrap();
    match code {
        KeyCode::Esc => app.php_settings = None,
        KeyCode::Enter => submit_php_settings(app),
        KeyCode::Tab | KeyCode::Down => m.field = (m.field + 1) % n,
        KeyCode::BackTab | KeyCode::Up => m.field = (m.field + n - 1) % n,
        KeyCode::Backspace => {
            if let Some(v) = m.values.get_mut(m.field) {
                v.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(v) = m.values.get_mut(m.field) {
                v.push(c);
            }
        }
        _ => {}
    }
}

fn submit_php_settings(app: &mut App) {
    let m = app.php_settings.as_ref().unwrap();
    let version = m.version.clone();
    let values = m.values.clone();
    let defs = php::php_settings_defs();

    let result = (|| -> Result<()> {
        let mut state = load_state()?;
        let php_rec = state
            .php_versions
            .iter_mut()
            .find(|p| p.version == version)
            .ok_or_else(|| anyhow::anyhow!("PHP {version} not found"))?;
        for (def, val) in defs.iter().zip(values.iter()) {
            let val = val.trim();
            // Store only non-default values to keep state minimal.
            if val.is_empty() || val == def.default {
                php_rec.settings.remove(def.key);
            } else {
                php_rec
                    .settings
                    .insert(def.key.to_string(), val.to_string());
            }
        }
        let record = php_rec.clone();
        crate::state::save_state(&state)?;
        let brew = Brew::detect()?;
        php::ensure_fpm_running(&brew, &record)
    })();

    match result {
        Ok(()) => {
            app.php_settings = None;
            app.message = format!("✓ saved PHP {version} settings (FPM restarted)");
            app.refresh();
        }
        Err(e) => {
            if let Some(m) = app.php_settings.as_mut() {
                m.error = Some(e.to_string());
            }
        }
    }
}

/// Cycle Xdebug off → debug → profile for the selected version. If enabling
/// needs a pecl install, defer to the suspended runner so output is visible.
fn toggle_xdebug(app: &mut App) {
    let Some(p) = app.state.php_versions.get(app.sel_php).cloned() else {
        return;
    };
    let next = p.xdebug.next();
    // Enabling for the first time may require building Xdebug — do it suspended.
    let needs_install = !next.is_off()
        && Brew::detect()
            .and_then(|b| php::extensions::is_loaded(&b, &p.version, "xdebug"))
            .map(|loaded| !loaded)
            .unwrap_or(true);
    if needs_install {
        app.pending_xdebug = Some((p.version.clone(), next));
        app.message = format!("installing Xdebug for PHP {}…", p.version);
    } else {
        let r = ops::set_xdebug(&p.version, next)
            .map(|_| format!("PHP {} Xdebug {}", p.version, next.as_str()));
        app.act("xdebug", r);
    }
}

fn restart_selected_fpm(app: &mut App) {
    if let Some(ver) = app.selected_php_version() {
        let r = ops::restart_fpm(&ver).map(|_| format!("restarted PHP {ver} FPM"));
        app.act("php restart", r);
    }
}

/// Start the selected server. If its backend's brew formula isn't installed yet,
/// defer to the suspended runner so the (slow) install output is visible.
fn start_selected_server(app: &mut App) {
    let Some(name) = app.selected_server_name() else {
        return;
    };
    let installed = app
        .state
        .get_server(&name)
        .map(|s| crate::backends::backend_for(s.backend).formula())
        .and_then(|formula| Brew::detect().ok().map(|brew| brew.is_installed(formula)))
        .unwrap_or(false);
    if installed {
        let r = ops::start_server(&name).map(|st| format!("started '{name}' — {}", st.as_str()));
        app.act("start", r);
    } else {
        app.pending_server = Some(name);
        app.message = "installing web server backend…".into();
    }
}

/// Start the selected service. If its brew formula isn't installed yet, defer to
/// the suspended runner so the (slow) install output is visible.
fn start_selected_service(app: &mut App) {
    let Some(kind) = app.selected_service() else {
        return;
    };
    let installed = Brew::detect()
        .map(|b| crate::services::is_installed(&b, kind))
        .unwrap_or(false);
    if installed {
        let r = ops::start_service(kind).map(|st| format!("started '{kind}' — {}", st.as_str()));
        app.act("start", r);
    } else {
        app.pending_service = Some(kind);
        app.message = format!("installing {kind}…");
    }
}

/// Pick a service kind to add, then queue its (possibly slow) start.
fn handle_service_picker_key(app: &mut App, code: KeyCode) {
    let kinds = crate::state::ServiceKind::all();
    let m = app.service_picker.as_mut().unwrap();
    match code {
        KeyCode::Esc => app.service_picker = None,
        KeyCode::Up | KeyCode::Char('k') => m.sel = (m.sel + kinds.len() - 1) % kinds.len(),
        KeyCode::Down | KeyCode::Char('j') => m.sel = (m.sel + 1) % kinds.len(),
        KeyCode::Enter => {
            let kind = kinds[m.sel];
            app.service_picker = None;
            // Register, then start (the start path installs if needed).
            if let Err(e) = ops::add_service(kind) {
                app.message = format!("✗ add service: {e}");
                return;
            }
            let installed = Brew::detect()
                .map(|b| crate::services::is_installed(&b, kind))
                .unwrap_or(false);
            if installed {
                let r = ops::start_service(kind)
                    .map(|st| format!("started '{kind}' — {}", st.as_str()));
                app.act("start", r);
            } else {
                app.pending_service = Some(kind);
                app.message = format!("installing {kind}…");
            }
        }
        _ => {}
    }
}

fn handle_php_install_key(app: &mut App, code: KeyCode) {
    let m = app.php_install.as_mut().unwrap();
    match code {
        KeyCode::Esc => app.php_install = None,
        KeyCode::Enter => {
            let ver = m.version.trim().to_string();
            if ver.is_empty() {
                m.error = Some("Enter a version, e.g. 8.3".into());
            } else if app.state.php_versions.iter().any(|p| p.version == ver) {
                m.error = Some(format!("PHP {ver} is already managed"));
            } else {
                // Defer to run_loop, which suspends the TUI to show brew output.
                app.php_install = None;
                app.pending_install = Some(ver);
            }
        }
        KeyCode::Backspace => {
            m.version.pop();
        }
        // Versions are digits and dots.
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => m.version.push(c),
        _ => {}
    }
}

fn handle_php_confirm_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            if let Some(ver) = app.confirm_remove_php.take() {
                let r = ops::remove_php(&ver).map(|_| format!("removed PHP {ver}"));
                app.act("remove php", r);
            }
        }
        _ => app.confirm_remove_php = None,
    }
}

fn handle_confirm_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            if let Some(name) = app.confirm_remove.take() {
                let r = remove_vhost(&name).map(|_| format!("removed vhost '{name}'"));
                app.act("remove", r);
            }
        }
        _ => app.confirm_remove = None,
    }
}

/// Remove a vhost from state by host name.
fn remove_vhost(name: &str) -> Result<()> {
    let mut state = load_state()?;
    let before = state.vhosts.len();
    state.vhosts.retain(|v| v.server_name != name);
    if state.vhosts.len() == before {
        anyhow::bail!("vhost '{name}' not found");
    }
    crate::state::save_state(&state)
}

/// Test helper: force the wizard open (on the Doc root field) for a snapshot.
fn open_wizard_for_snapshot(app: &mut App) {
    open_wizard(app);
    if let Some(w) = app.wizard.as_mut() {
        w.field = FIELD_DOCROOT;
    }
}

/// Open the new-vhost wizard, or explain what's missing.
fn open_wizard(app: &mut App) {
    if app.state.php_versions.is_empty() {
        app.message = "Install a PHP version first: `reeve php install 8.3`".into();
        return;
    }
    if app.state.servers.is_empty() {
        app.message = "Add a server first: `reeve server add caddy`".into();
        return;
    }
    // Pre-seed the doc root with the sites root (trailing slash) so the
    // autocomplete dropdown immediately lists projects there.
    let sites_root = format!("{}/", app.config.sites_root.trim_end_matches('/'));
    app.wizard = Some(VhostWizard {
        server_name: String::new(),
        docroot: sites_root,
        php_idx: app.sel_php.min(app.state.php_versions.len() - 1),
        server_idx: app.sel_server.min(app.state.servers.len() - 1),
        ssl: false,
        field: 0,
        error: None,
        editing: None,
        dropdown_open: true,
        path_sel: None,
        preset: Framework::Generic,
        proxy_target: None,
    });
}

/// Open the wizard pre-filled to edit the selected vhost.
fn open_edit_wizard(app: &mut App) {
    let Some(v) = app.state.vhosts.get(app.sel_vhost).cloned() else {
        return;
    };
    let php_idx = app
        .state
        .php_versions
        .iter()
        .position(|p| p.version == v.php_version)
        .unwrap_or(0);
    let server_idx = app
        .state
        .servers
        .iter()
        .position(|s| s.name == v.server)
        .unwrap_or(0);
    app.wizard = Some(VhostWizard {
        server_name: v.server_name.clone(),
        docroot: v.docroot,
        php_idx,
        server_idx,
        ssl: v.ssl,
        field: 0,
        error: None,
        editing: Some(v.server_name),
        dropdown_open: true,
        path_sel: None,
        preset: v.preset,
        proxy_target: v.proxy_target,
    });
}

/// The Doc root field index in the wizard.
const FIELD_DOCROOT: usize = 1;

fn handle_wizard_key(app: &mut App, code: KeyCode) {
    let php_len = app.state.php_versions.len();
    let srv_len = app.state.servers.len();
    let w = app.wizard.as_mut().unwrap();

    // ── Doc root with the dropdown OPEN: arrows navigate the list, Esc closes
    // just the dropdown, Tab/Shift-Tab still move fields (reliable escape).
    if w.dropdown_active() {
        let count = w.path_suggestions().len();
        match code {
            KeyCode::Esc => {
                w.dropdown_open = false;
                w.path_sel = None;
            }
            KeyCode::Down if count > 0 => {
                w.path_sel = Some(match w.path_sel {
                    None => 0,
                    Some(i) => (i + 1).min(count - 1),
                });
            }
            KeyCode::Up => match w.path_sel {
                Some(i) if i > 0 => w.path_sel = Some(i - 1),
                _ => w.path_sel = None,
            },
            KeyCode::Enter | KeyCode::Right => commit_highlighted_path(w),
            KeyCode::Tab => tab_complete_path(w),
            KeyCode::BackTab => w.prev_field(),
            KeyCode::Backspace => {
                w.docroot.pop();
                w.path_sel = None;
            }
            KeyCode::Char(c) => {
                w.docroot.push(c);
                w.path_sel = None;
            }
            _ => {}
        }
        return;
    }

    // ── All other fields (and Doc root with a dismissed dropdown).
    match code {
        KeyCode::Esc => app.wizard = None,
        KeyCode::Enter => submit_wizard(app),
        KeyCode::Tab | KeyCode::Down => w.next_field(),
        KeyCode::BackTab | KeyCode::Up => w.prev_field(),
        KeyCode::Left => wizard_adjust(w, php_len, srv_len, false),
        KeyCode::Right => wizard_adjust(w, php_len, srv_len, true),
        KeyCode::Backspace => match w.field {
            0 => {
                w.server_name.pop();
            }
            FIELD_DOCROOT => {
                // Typing on a dismissed dropdown reopens it.
                w.docroot.pop();
                w.dropdown_open = true;
            }
            _ => {}
        },
        KeyCode::Char(' ') => match w.field {
            0 => w.server_name.push(' '),
            2 | 3 | 5 => wizard_adjust(w, php_len, srv_len, true),
            4 => w.ssl = !w.ssl,
            _ => {}
        },
        KeyCode::Char(c) => match w.field {
            0 => w.server_name.push(c),
            FIELD_DOCROOT => {
                w.docroot.push(c);
                w.dropdown_open = true;
            }
            _ => {}
        },
        _ => {}
    }
}

/// Commit the highlighted dropdown entry (or the first directory if none is
/// highlighted) into `docroot`, keeping the dropdown open on the new listing.
fn commit_highlighted_path(w: &mut VhostWizard) {
    let sugg = w.path_suggestions();
    let entry = w
        .path_sel
        .and_then(|i| sugg.get(i))
        .or_else(|| sugg.iter().find(|e| e.is_dir));
    if let Some(e) = entry {
        w.docroot = pathpick::commit_entry(&w.docroot, e);
        w.path_sel = None;
    }
}

/// Shell-style Tab completion for the Doc root field.
fn tab_complete_path(w: &mut VhostWizard) {
    let all = pathpick::read_dir_filtered(&w.docroot, usize::MAX);
    if all.is_empty() {
        // Nothing to complete — advance to the next field.
        w.next_field();
        return;
    }
    if all.len() == 1 {
        w.docroot = pathpick::commit_entry(&w.docroot, &all[0]);
        return;
    }
    let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
    let lcp = pathpick::longest_common_prefix(&names);
    let (dir, prefix) = pathpick::split_path(&w.docroot);
    if lcp.chars().count() > prefix.chars().count() {
        // Extend to the common prefix and stay so the user can keep narrowing.
        w.docroot = format!("{dir}{lcp}");
    } else {
        // Already at the common prefix — nothing more to complete, so move on
        // instead of trapping the user on the field.
        w.next_field();
    }
}

/// Cycle the choice/toggle fields (PHP, Server, SSL).
fn wizard_adjust(w: &mut VhostWizard, php_len: usize, srv_len: usize, forward: bool) {
    let step = |i: usize, len: usize| {
        if len == 0 {
            0
        } else if forward {
            (i + 1) % len
        } else {
            (i + len - 1) % len
        }
    };
    match w.field {
        2 => w.php_idx = step(w.php_idx, php_len),
        3 => w.server_idx = step(w.server_idx, srv_len),
        4 => w.ssl = !w.ssl,
        5 => {
            let all = Framework::all();
            let cur = all.iter().position(|f| *f == w.preset).unwrap_or(0);
            w.preset = all[step(cur, all.len())];
        }
        _ => {}
    }
}

fn submit_wizard(app: &mut App) {
    let w = app.wizard.as_ref().unwrap();
    let name = w.server_name.trim().to_string();
    let php = app
        .state
        .php_versions
        .get(w.php_idx)
        .map(|p| p.version.clone());
    let server = app.state.servers.get(w.server_idx).map(|s| s.name.clone());
    let ssl = w.ssl;
    let preset = w.preset;
    let proxy_target = w.proxy_target.clone();
    let raw_root = w.docroot.trim();
    let docroot = if raw_root.is_empty() {
        format!("{}/{}", app.config.sites_root.trim_end_matches('/'), name)
    } else if raw_root.ends_with('/') {
        // Left on a directory — treat it as the parent and use the host as the
        // project folder, e.g. ~/Sites/ + app.test → ~/Sites/app.test.
        format!("{raw_root}{name}")
    } else {
        raw_root.to_string()
    };

    let set_err = |app: &mut App, msg: String| {
        if let Some(w) = app.wizard.as_mut() {
            w.error = Some(msg);
        }
    };

    if name.is_empty() {
        set_err(app, "Hostname is required".into());
        return;
    }
    let (Some(php), Some(server)) = (php, server) else {
        set_err(app, "Pick a PHP version and a server".into());
        return;
    };

    let editing = app.wizard.as_ref().and_then(|w| w.editing.clone());

    let result = (|| -> anyhow::Result<()> {
        let mut state = load_state()?;
        // Editing replaces the original (handles renames too).
        if let Some(orig) = &editing {
            state.vhosts.retain(|v| &v.server_name != orig);
        }
        state.add_vhost(crate::state::Vhost {
            server_name: name.clone(),
            server: server.clone(),
            docroot: docroot.clone(),
            php_version: php.clone(),
            ssl,
            preset,
            proxy_target,
        })?;
        crate::state::save_state(&state)
    })();

    match result {
        Ok(()) => {
            app.wizard = None;
            let verb = if editing.is_some() {
                "updated"
            } else {
                "created"
            };
            app.message = format!("✓ {verb} vhost '{name}' on '{server}' (PHP {php}) — press 'r' on the server to apply");
            app.refresh();
        }
        Err(e) => set_err(app, e.to_string()),
    }
}

/// Re-render + restart every enabled server.
fn apply_all() -> Result<usize> {
    let state = load_state()?;
    let mut n = 0;
    for server in state.servers.iter().filter(|s| s.enabled) {
        ops::restart_server(&server.name)?;
        n += 1;
    }
    Ok(n)
}

/// Suspend the TUI, run a (slow) PHP install with visible brew output, wait for
/// a keypress, then resume the dashboard.
fn run_install_suspended(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    version: &str,
) -> Result<()> {
    restore_terminal(terminal)?;
    println!("\n── Installing PHP {version} (this can take a few minutes) ──\n");
    let result = ops::install_php(version);
    match &result {
        Ok(()) => println!("\n✓ PHP {version} installed. Press any key to return…"),
        Err(e) => println!("\n✗ install failed: {e}\nPress any key to return…"),
    }
    // Wait for a keypress so the output stays readable.
    let _ = enable_raw_mode();
    loop {
        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    break;
                }
            }
        }
    }
    let _ = disable_raw_mode();
    *terminal = setup_terminal()?;
    app.message = match result {
        Ok(()) => format!("✓ installed PHP {version}"),
        Err(e) => format!("✗ install PHP {version}: {e}"),
    };
    app.refresh();
    Ok(())
}

/// Suspend the TUI, run a (slow) pecl add/remove with visible output, wait for
/// a keypress, then resume the dashboard.
fn run_ext_suspended(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    pe: PendingExt,
) -> Result<()> {
    restore_terminal(terminal)?;
    let (verb, name) = match &pe.action {
        ExtAction::Add(n) => ("Installing", n.clone()),
        ExtAction::Remove(n) => ("Removing", n.clone()),
    };
    println!("\n── {verb} {name} for PHP {} ──\n", pe.version);
    let result = run_ext_op(&pe);
    match &result {
        Ok(msg) => println!("\n✓ {msg}. Press any key to return…"),
        Err(e) => println!("\n✗ {e}\nPress any key to return…"),
    }
    let _ = enable_raw_mode();
    loop {
        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    break;
                }
            }
        }
    }
    let _ = disable_raw_mode();
    *terminal = setup_terminal()?;
    app.message = match result {
        Ok(msg) => format!("✓ {msg}"),
        Err(e) => format!("✗ {e}"),
    };
    app.refresh();
    Ok(())
}

/// Suspend the TUI, enable Xdebug (building it via pecl if needed) with visible
/// output, wait for a keypress, then resume the dashboard.
fn run_xdebug_suspended(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    version: &str,
    mode: XdebugMode,
) -> Result<()> {
    restore_terminal(terminal)?;
    println!(
        "\n── Enabling Xdebug ({}) for PHP {version} ──\n",
        mode.as_str()
    );
    let result = ops::set_xdebug(version, mode);
    match &result {
        Ok(()) => println!(
            "\n✓ Xdebug {} for PHP {version}. Press any key to return…",
            mode.as_str()
        ),
        Err(e) => println!("\n✗ {e}\nPress any key to return…"),
    }
    let _ = enable_raw_mode();
    loop {
        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    break;
                }
            }
        }
    }
    let _ = disable_raw_mode();
    *terminal = setup_terminal()?;
    app.message = match result {
        Ok(()) => format!("✓ PHP {version} Xdebug {}", mode.as_str()),
        Err(e) => format!("✗ xdebug: {e}"),
    };
    app.refresh();
    Ok(())
}

/// Suspend the TUI, install the backend (if needed) + start a server with
/// visible output, wait for a keypress, then resume the dashboard.
fn run_server_start_suspended(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    name: &str,
) -> Result<()> {
    restore_terminal(terminal)?;
    println!("\n── Installing backend + starting '{name}' ──\n");
    let result = ops::start_server(name);
    match &result {
        Ok(st) => println!("\n✓ '{name}' — {}. Press any key to return…", st.as_str()),
        Err(e) => println!("\n✗ {e}\nPress any key to return…"),
    }
    let _ = enable_raw_mode();
    loop {
        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    break;
                }
            }
        }
    }
    let _ = disable_raw_mode();
    *terminal = setup_terminal()?;
    app.message = match result {
        Ok(st) => format!("✓ '{name}' — {}", st.as_str()),
        Err(e) => format!("✗ {name}: {e}"),
    };
    app.refresh();
    Ok(())
}

/// Suspend the TUI, install (if needed) + start a service with visible output,
/// wait for a keypress, then resume the dashboard.
fn run_service_start_suspended(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    kind: crate::state::ServiceKind,
) -> Result<()> {
    restore_terminal(terminal)?;
    println!("\n── Installing + starting {kind} ──\n");
    let result = ops::start_service(kind);
    match &result {
        Ok(st) => println!("\n✓ {kind} — {}. Press any key to return…", st.as_str()),
        Err(e) => println!("\n✗ {e}\nPress any key to return…"),
    }
    let _ = enable_raw_mode();
    loop {
        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    break;
                }
            }
        }
    }
    let _ = disable_raw_mode();
    *terminal = setup_terminal()?;
    app.message = match result {
        Ok(st) => format!("✓ {kind} — {}", st.as_str()),
        Err(e) => format!("✗ {kind}: {e}"),
    };
    app.refresh();
    Ok(())
}

/// The actual pecl call + FPM restart for a queued extension op.
fn run_ext_op(pe: &PendingExt) -> Result<String> {
    let brew = Brew::detect()?;
    match &pe.action {
        ExtAction::Add(name) => {
            php::extensions::add(&brew, &pe.version, name)?;
            ops::restart_fpm(&pe.version)?;
            Ok(format!(
                "{name} installed for PHP {} (FPM restarted)",
                pe.version
            ))
        }
        ExtAction::Remove(name) => {
            php::extensions::remove(&brew, &pe.version, name)?;
            ops::restart_fpm(&pe.version)?;
            Ok(format!(
                "{name} removed from PHP {} (FPM restarted)",
                pe.version
            ))
        }
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
