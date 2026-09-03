//! The declarative source of truth (`state.toml`): which servers, vhosts, and
//! PHP versions exist. reeve renders native configs from this and
//! reconciles running launchd services to match it.

use crate::paths;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::str::FromStr;

/// Which web server implements a given [`Server`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Caddy,
    Apache,
    Nginx,
    Ols,
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::Caddy => "caddy",
            Backend::Apache => "apache",
            Backend::Nginx => "nginx",
            Backend::Ols => "ols",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` honors width/fill flags (e.g. `{:<8}`); `write_str` would not.
        f.pad(self.as_str())
    }
}

impl FromStr for Backend {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "caddy" => Ok(Backend::Caddy),
            "apache" | "httpd" => Ok(Backend::Apache),
            "nginx" => Ok(Backend::Nginx),
            "ols" | "openlitespeed" | "litespeed" => Ok(Backend::Ols),
            other => bail!("Unknown backend '{other}'. Use caddy|apache|nginx|ols."),
        }
    }
}

/// A web server instance. Multiple servers (even of the same backend) can run
/// at once on different ports and are managed independently.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub backend: Backend,
    pub http_port: u16,
    pub https_port: u16,
    #[serde(default)]
    pub enabled: bool,
    /// When true, serve a catch-all "default site" on the HTTP port from the
    /// configured sites root, for any host not matched by a vhost (e.g. plain
    /// `http://localhost:<port>`). Uses the default PHP version for `.php`.
    #[serde(default)]
    pub default_site: bool,
    /// Framework preset applied to the default site's docroot (front-controller
    /// rewrites + security rules), mirroring a vhost's `preset`. Generic =
    /// plain try_files. Only meaningful when `default_site` is true.
    #[serde(default)]
    pub default_preset: Framework,
    /// Docroot for this server's catch-all default site. `None` falls back to
    /// the global `config.sites_root`, so existing servers keep serving the
    /// shared root; set it to give one server its own default root. Only
    /// meaningful when `default_site` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_root: Option<String>,
    /// Per-backend tunables (keys defined by each backend; see
    /// `backends::settings_defs`). Empty = all defaults.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub settings: std::collections::BTreeMap<String, String>,
}

impl Server {
    /// A setting value, falling back to the backend's default.
    pub fn setting<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.settings
            .get(key)
            .map(|s| s.as_str())
            .unwrap_or(default)
    }

    /// The default site's docroot: this server's override, else the global
    /// sites root. Trailing slash trimmed and a leading `~` expanded, since
    /// none of the backends understand a tilde in a docroot. Only used when
    /// `default_site` is on.
    pub fn effective_default_root(&self, sites_root: &str) -> String {
        expand_tilde(
            self.default_root
                .as_deref()
                .unwrap_or(sites_root)
                .trim_end_matches('/'),
        )
    }
}

/// Xdebug operating mode for a PHP version. `Off` neutralizes an installed
/// Xdebug (near-zero overhead in Xdebug 3); the others set `xdebug.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum XdebugMode {
    #[default]
    Off,
    Debug,
    Profile,
}

impl XdebugMode {
    /// The value to write for `xdebug.mode`.
    pub fn as_str(&self) -> &'static str {
        match self {
            XdebugMode::Off => "off",
            XdebugMode::Debug => "debug",
            XdebugMode::Profile => "profile",
        }
    }

    /// The value to write for `xdebug.start_with_request`.
    ///
    /// Debug waits for an explicit trigger (an `XDEBUG_SESSION` /
    /// `XDEBUG_TRIGGER` cookie or query param, which IDE debug run
    /// configurations and browser extensions set for you): IDEs accept one
    /// simultaneous connection by default, so with `yes` a background request
    /// from another vhost grabs the slot and the page you actually wanted to
    /// debug never attaches. Profiling has no such contention, and a profiler
    /// you have to opt each request into produces no cachegrind files at all
    /// for a normal page load, so it stays on for every request.
    pub fn start_with_request(&self) -> &'static str {
        match self {
            XdebugMode::Off => "no",
            XdebugMode::Debug => "trigger",
            XdebugMode::Profile => "yes",
        }
    }

    pub fn is_off(&self) -> bool {
        matches!(self, XdebugMode::Off)
    }

    /// Cycle off → debug → profile → off (for the TUI toggle).
    pub fn next(&self) -> Self {
        match self {
            XdebugMode::Off => XdebugMode::Debug,
            XdebugMode::Debug => XdebugMode::Profile,
            XdebugMode::Profile => XdebugMode::Off,
        }
    }
}

impl FromStr for XdebugMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "off" | "0" | "false" => Ok(XdebugMode::Off),
            "debug" | "on" => Ok(XdebugMode::Debug),
            "profile" => Ok(XdebugMode::Profile),
            other => bail!("Unknown Xdebug mode '{other}'. Use off|debug|profile."),
        }
    }
}

fn default_xdebug_port() -> u16 {
    9003
}

/// An installed PHP version with its own php-fpm master + socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhpVersion {
    /// e.g. "8.3"
    pub version: String,
    /// Unix socket the fpm pool listens on (under run/).
    pub fpm_socket: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    /// php.ini / OPcache / FPM-pool overrides. Keys are defined by
    /// [`crate::php::php_settings_defs`]; empty = all defaults.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub settings: std::collections::BTreeMap<String, String>,
    /// Xdebug mode (off unless explicitly enabled).
    #[serde(default, skip_serializing_if = "XdebugMode::is_off")]
    pub xdebug: XdebugMode,
    /// Debugger client port Xdebug connects back to (IDE listens here).
    #[serde(default = "default_xdebug_port")]
    pub xdebug_port: u16,
}

impl Default for PhpVersion {
    fn default() -> Self {
        Self {
            version: String::new(),
            fpm_socket: String::new(),
            extensions: Vec::new(),
            settings: std::collections::BTreeMap::new(),
            xdebug: XdebugMode::Off,
            xdebug_port: default_xdebug_port(),
        }
    }
}

impl PhpVersion {
    /// A setting value, falling back to the supplied default.
    pub fn setting<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.settings
            .get(key)
            .map(|s| s.as_str())
            .unwrap_or(default)
    }
}

/// A framework preset: picks the conventional public subdir and rewrite rules
/// so a vhost "just works" for that app type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framework {
    #[default]
    Generic,
    Laravel,
    Wordpress,
    Symfony,
    Grav,
    Drupal,
    /// Any app that serves from a `public/` subdir without framework-specific
    /// rewrites — a plain PHP front controller, a static build output, etc.
    Public,
}

impl Framework {
    pub fn as_str(&self) -> &'static str {
        match self {
            Framework::Generic => "generic",
            Framework::Laravel => "laravel",
            Framework::Wordpress => "wordpress",
            Framework::Symfony => "symfony",
            Framework::Grav => "grav",
            Framework::Drupal => "drupal",
            Framework::Public => "public",
        }
    }

    pub fn is_generic(&self) -> bool {
        matches!(self, Framework::Generic)
    }

    /// Every preset, in selector order.
    pub fn all() -> [Framework; 7] {
        [
            Framework::Generic,
            Framework::Laravel,
            Framework::Wordpress,
            Framework::Symfony,
            Framework::Grav,
            Framework::Drupal,
            Framework::Public,
        ]
    }

    /// Conventional public subdirectory served as the docroot (empty = root).
    pub fn public_subdir(&self) -> &'static str {
        match self {
            Framework::Laravel | Framework::Symfony | Framework::Public => "public",
            Framework::Drupal => "web",
            _ => "",
        }
    }
}

impl fmt::Display for Framework {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl FromStr for Framework {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "generic" | "" | "none" => Ok(Framework::Generic),
            "laravel" => Ok(Framework::Laravel),
            "wordpress" | "wp" => Ok(Framework::Wordpress),
            "symfony" => Ok(Framework::Symfony),
            "grav" => Ok(Framework::Grav),
            "drupal" => Ok(Framework::Drupal),
            "public" => Ok(Framework::Public),
            other => bail!(
                "Unknown preset '{other}'. Use \
                 generic|laravel|wordpress|symfony|grav|drupal|public."
            ),
        }
    }
}

/// Expand a leading `~`/`~/` to the user's home directory. Docroots are typed
/// by hand in the TUI and on the CLI, but no web server backend expands a
/// tilde itself — an unexpanded `~/...` root is resolved relative to the
/// server's working directory and every request 404s.
pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.display().to_string();
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    path.to_string()
}

/// `root/sub`, or just `root` when `sub` is empty. Tolerates a trailing slash
/// on `root` and leading/trailing slashes on `sub`.
pub fn join_subdir(root: &str, sub: &str) -> String {
    let sub = sub.trim_matches('/');
    if sub.is_empty() {
        root.to_string()
    } else {
        format!("{}/{}", root.trim_end_matches('/'), sub)
    }
}

/// Binds a hostname to a project directory, an owning server, and a PHP
/// version. `docroot` is the *project* root; what the server actually serves
/// comes from [`crate::project::resolve`] (preset subdir or `.reeve.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vhost {
    /// e.g. "grav.test"
    pub server_name: String,
    /// Owning [`Server::name`].
    pub server: String,
    pub docroot: String,
    /// [`PhpVersion::version`] this vhost executes PHP with.
    pub php_version: String,
    #[serde(default)]
    pub ssl: bool,
    /// Framework preset controlling docroot subdir + rewrites.
    #[serde(default, skip_serializing_if = "Framework::is_generic")]
    pub preset: Framework,
    /// When set, the vhost is a reverse proxy to this upstream (e.g.
    /// `http://localhost:5173`) instead of serving PHP/files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_target: Option<String>,
}

impl Vhost {
    /// True when this vhost is a reverse proxy rather than a file/PHP server.
    pub fn is_proxy(&self) -> bool {
        self.proxy_target.is_some()
    }
}

/// A managed background service reeve can install/start/stop via Homebrew +
/// launchd (databases, cache, mail). reeve adopts each formula's default
/// datadir/config and just owns the launchd lifecycle and logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    Mysql,
    Mariadb,
    Postgres,
    Redis,
    Memcached,
    Mailpit,
}

impl ServiceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceKind::Mysql => "mysql",
            ServiceKind::Mariadb => "mariadb",
            ServiceKind::Postgres => "postgres",
            ServiceKind::Redis => "redis",
            ServiceKind::Memcached => "memcached",
            ServiceKind::Mailpit => "mailpit",
        }
    }

    /// Every kind, in display order.
    pub fn all() -> [ServiceKind; 6] {
        [
            ServiceKind::Mysql,
            ServiceKind::Mariadb,
            ServiceKind::Postgres,
            ServiceKind::Redis,
            ServiceKind::Memcached,
            ServiceKind::Mailpit,
        ]
    }
}

impl fmt::Display for ServiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl FromStr for ServiceKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "mysql" => Ok(ServiceKind::Mysql),
            "mariadb" => Ok(ServiceKind::Mariadb),
            "postgres" | "postgresql" | "pg" => Ok(ServiceKind::Postgres),
            "redis" => Ok(ServiceKind::Redis),
            "memcached" | "memcache" => Ok(ServiceKind::Memcached),
            "mailpit" => Ok(ServiceKind::Mailpit),
            other => bail!(
                "Unknown service '{other}'. Use \
                 mysql|mariadb|postgres|redis|memcached|mailpit."
            ),
        }
    }
}

/// A managed-service entry in `state.toml`. One instance per kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedServiceInstance {
    pub kind: ServiceKind,
    #[serde(default)]
    pub enabled: bool,
    /// Listening-port overrides, keyed by the service's port definitions (see
    /// [`crate::services::port_defs`]). Absent keys use the formula's default
    /// port, so a service nobody has re-pointed behaves exactly as before.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub ports: std::collections::BTreeMap<String, u16>,
}

impl ManagedServiceInstance {
    /// A new instance on every default port, not yet started.
    pub fn new(kind: ServiceKind) -> Self {
        Self {
            kind,
            enabled: false,
            ports: Default::default(),
        }
    }

    /// The configured port for `key`, falling back to the service's default.
    pub fn port(&self, key: &str, default: u16) -> u16 {
        self.ports.get(key).copied().unwrap_or(default)
    }
}

/// A parked directory: every immediate subfolder with web content is served
/// automatically as `<subfolder>.<tld>` by `server`, no per-project vhost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Park {
    /// Absolute directory whose subfolders become vhosts.
    pub root: String,
    /// Owning [`Server::name`].
    pub server: String,
    /// PHP version every parked site runs.
    pub php_version: String,
    /// TLD appended to each subfolder name.
    pub tld: String,
    #[serde(default)]
    pub ssl: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub servers: Vec<Server>,
    #[serde(default)]
    pub php_versions: Vec<PhpVersion>,
    #[serde(default)]
    pub vhosts: Vec<Vhost>,
    #[serde(default)]
    pub services: Vec<ManagedServiceInstance>,
    #[serde(default)]
    pub parks: Vec<Park>,
}

/// Numeric sort key for a PHP version string like "8.4" → (8, 4), so versions
/// order naturally instead of lexically ("8.10" after "8.9").
pub fn version_key(v: &str) -> (u32, u32) {
    let mut it = v.split('.');
    let major = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor)
}

impl State {
    pub fn get_server(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.name == name)
    }

    pub fn get_service(&self, kind: ServiceKind) -> Option<&ManagedServiceInstance> {
        self.services.iter().find(|s| s.kind == kind)
    }

    pub fn get_php(&self, version: &str) -> Option<&PhpVersion> {
        self.php_versions.iter().find(|p| p.version == version)
    }

    /// Sort installed PHP versions ascending (7.3, 8.3, 8.4, …) so listings are
    /// stable regardless of install order.
    pub fn sort_php(&mut self) {
        self.php_versions.sort_by_key(|p| version_key(&p.version));
    }

    pub fn add_server(&mut self, server: Server) -> Result<()> {
        if self.servers.iter().any(|s| s.name == server.name) {
            bail!("Server '{}' already exists", server.name);
        }
        // Reject port collisions across enabled servers.
        for existing in &self.servers {
            if existing.http_port == server.http_port || existing.https_port == server.https_port {
                bail!(
                    "Port conflict with server '{}' ({}/{})",
                    existing.name,
                    existing.http_port,
                    existing.https_port
                );
            }
        }
        self.servers.push(server);
        Ok(())
    }

    pub fn add_vhost(&mut self, vhost: Vhost) -> Result<()> {
        if self
            .vhosts
            .iter()
            .any(|v| v.server_name == vhost.server_name)
        {
            bail!("Vhost '{}' already exists", vhost.server_name);
        }
        if self.get_server(&vhost.server).is_none() {
            bail!("Server '{}' does not exist", vhost.server);
        }
        // Proxy vhosts don't serve PHP, so they don't need a PHP version.
        if !vhost.is_proxy() && self.get_php(&vhost.php_version).is_none() {
            bail!(
                "PHP {} is not installed. Run `reeve php install {}`.",
                vhost.php_version,
                vhost.php_version
            );
        }
        self.vhosts.push(vhost);
        Ok(())
    }
}

pub fn load_state() -> Result<State> {
    let path = paths::state_path()?;
    if !path.exists() {
        return Ok(State::default());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read state from {}", path.display()))?;
    toml::from_str(&contents).context("Invalid state.toml")
}

pub fn save_state(state: &State) -> Result<()> {
    paths::ensure_dirs()?;
    let path = paths::state_path()?;
    let contents = toml::to_string_pretty(state).context("Failed to serialize state")?;
    fs::write(&path, contents)
        .with_context(|| format!("Failed to write state to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, http: u16, https: u16) -> Server {
        Server {
            name: name.into(),
            backend: Backend::Caddy,
            http_port: http,
            https_port: https,
            enabled: false,
            default_site: false,
            default_preset: Framework::Generic,
            default_root: None,
            settings: Default::default(),
        }
    }

    #[test]
    fn default_root_overrides_else_falls_back_to_sites_root() {
        let mut s = server("a", 80, 443);
        // No override → global sites root (trailing slash trimmed).
        assert_eq!(s.effective_default_root("/Sites"), "/Sites");
        assert_eq!(s.effective_default_root("/Sites/"), "/Sites");
        // Override wins, also trimmed.
        s.default_root = Some("/var/www/".into());
        assert_eq!(s.effective_default_root("/Sites"), "/var/www");
    }

    #[test]
    fn default_root_expands_a_leading_tilde() {
        let home = dirs::home_dir().unwrap().display().to_string();
        let mut s = server("a", 80, 443);
        // No backend expands `~` itself, so it must never reach a docroot.
        s.default_root = Some("~/workspace/site/".into());
        assert_eq!(
            s.effective_default_root("/Sites"),
            format!("{home}/workspace/site")
        );
        // A bare `~` is the home directory, and a `~` mid-path is literal.
        s.default_root = Some("~".into());
        assert_eq!(s.effective_default_root("/Sites"), home);
        s.default_root = Some("/var/~/www".into());
        assert_eq!(s.effective_default_root("/Sites"), "/var/~/www");
        // The sites-root fallback gets the same treatment.
        s.default_root = None;
        assert_eq!(
            s.effective_default_root("~/workspace"),
            format!("{home}/workspace")
        );
    }

    #[test]
    fn backend_parse_roundtrip() {
        for s in ["caddy", "apache", "nginx", "ols"] {
            assert_eq!(s.parse::<Backend>().unwrap().as_str(), s);
        }
        assert_eq!("httpd".parse::<Backend>().unwrap(), Backend::Apache);
        assert_eq!("openlitespeed".parse::<Backend>().unwrap(), Backend::Ols);
        assert!("bogus".parse::<Backend>().is_err());
    }

    #[test]
    fn service_kind_parse_roundtrip() {
        for k in ServiceKind::all() {
            assert_eq!(k.as_str().parse::<ServiceKind>().unwrap(), k);
        }
        assert_eq!(
            "postgresql".parse::<ServiceKind>().unwrap(),
            ServiceKind::Postgres
        );
        assert!("bogus".parse::<ServiceKind>().is_err());
    }

    #[test]
    fn services_survive_toml_roundtrip() {
        let mut s = State::default();
        let mut svc = ManagedServiceInstance::new(ServiceKind::Mailpit);
        svc.enabled = true;
        svc.ports.insert("smtp".into(), 1026);
        s.services.push(svc);
        let toml = toml::to_string_pretty(&s).unwrap();
        let back: State = toml::from_str(&toml).unwrap();
        assert_eq!(back.services.len(), 1);
        assert_eq!(back.services[0].kind, ServiceKind::Mailpit);
        assert!(back.services[0].enabled);
        // The override survives; untouched ports still read their default.
        assert_eq!(back.services[0].port("smtp", 1025), 1026);
        assert_eq!(back.services[0].port("ui", 8025), 8025);
    }

    #[test]
    fn state_toml_roundtrip() {
        let mut s = State::default();
        s.servers.push(server("caddy", 80, 443));
        s.php_versions.push(PhpVersion {
            version: "8.3".into(),
            fpm_socket: "/run/php83.sock".into(),
            ..Default::default()
        });
        s.vhosts.push(Vhost {
            server_name: "a.test".into(),
            server: "caddy".into(),
            docroot: "/Sites/a".into(),
            php_version: "8.3".into(),
            ssl: true,
            preset: Framework::Laravel,
            proxy_target: None,
        });
        let toml = toml::to_string_pretty(&s).unwrap();
        let back: State = toml::from_str(&toml).unwrap();
        assert_eq!(back.servers.len(), 1);
        assert_eq!(back.vhosts[0].server_name, "a.test");
        assert!(back.vhosts[0].ssl);
        assert_eq!(back.vhosts[0].preset, Framework::Laravel);
        // Laravel serves from the public/ subdir.
        assert_eq!(
            crate::project::resolve(&back.vhosts[0]).unwrap().docroot,
            "/Sites/a/public"
        );
    }

    #[test]
    fn proxy_vhost_skips_php_requirement() {
        let mut s = State::default();
        s.add_server(server("caddy", 80, 443)).unwrap();
        let v = Vhost {
            server_name: "vite.test".into(),
            server: "caddy".into(),
            docroot: String::new(),
            php_version: String::new(),
            ssl: false,
            preset: Framework::Generic,
            proxy_target: Some("http://localhost:5173".into()),
        };
        // No PHP installed, but a proxy vhost doesn't need one.
        s.add_vhost(v).unwrap();
        assert!(s.vhosts[0].is_proxy());
    }

    #[test]
    fn add_server_rejects_dup_and_port_conflict() {
        let mut s = State::default();
        s.add_server(server("caddy", 80, 443)).unwrap();
        assert!(s.add_server(server("caddy", 8080, 8443)).is_err()); // dup name
        assert!(s.add_server(server("nginx", 80, 9443)).is_err()); // http conflict
        s.add_server(server("nginx", 8080, 8443)).unwrap(); // ok
        assert_eq!(s.servers.len(), 2);
    }

    #[test]
    fn add_vhost_requires_server_and_php() {
        let mut s = State::default();
        let v = Vhost {
            server_name: "a.test".into(),
            server: "caddy".into(),
            docroot: "/Sites/a".into(),
            php_version: "8.3".into(),
            ssl: false,
            preset: Framework::Generic,
            proxy_target: None,
        };
        assert!(s.add_vhost(v.clone()).is_err()); // no server yet
        s.add_server(server("caddy", 80, 443)).unwrap();
        assert!(s.add_vhost(v.clone()).is_err()); // no php yet
        s.php_versions.push(PhpVersion {
            version: "8.3".into(),
            fpm_socket: "/run/php83.sock".into(),
            ..Default::default()
        });
        s.add_vhost(v).unwrap();
        assert_eq!(s.vhosts.len(), 1);
    }
}
