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
}

/// Binds a hostname to a docroot, an owning server, and a PHP version.
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub servers: Vec<Server>,
    #[serde(default)]
    pub php_versions: Vec<PhpVersion>,
    #[serde(default)]
    pub vhosts: Vec<Vhost>,
}

impl State {
    pub fn get_server(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.name == name)
    }

    pub fn get_php(&self, version: &str) -> Option<&PhpVersion> {
        self.php_versions.iter().find(|p| p.version == version)
    }

    /// Vhosts owned by a given server.
    pub fn vhosts_for(&self, server: &str) -> Vec<&Vhost> {
        self.vhosts.iter().filter(|v| v.server == server).collect()
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
        if self.get_php(&vhost.php_version).is_none() {
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
            settings: Default::default(),
        }
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
    fn state_toml_roundtrip() {
        let mut s = State::default();
        s.servers.push(server("caddy", 80, 443));
        s.php_versions.push(PhpVersion {
            version: "8.3".into(),
            fpm_socket: "/run/php83.sock".into(),
            extensions: vec![],
        });
        s.vhosts.push(Vhost {
            server_name: "a.test".into(),
            server: "caddy".into(),
            docroot: "/Sites/a".into(),
            php_version: "8.3".into(),
            ssl: true,
        });
        let toml = toml::to_string_pretty(&s).unwrap();
        let back: State = toml::from_str(&toml).unwrap();
        assert_eq!(back.servers.len(), 1);
        assert_eq!(back.vhosts[0].server_name, "a.test");
        assert!(back.vhosts[0].ssl);
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
        };
        assert!(s.add_vhost(v.clone()).is_err()); // no server yet
        s.add_server(server("caddy", 80, 443)).unwrap();
        assert!(s.add_vhost(v.clone()).is_err()); // no php yet
        s.php_versions.push(PhpVersion {
            version: "8.3".into(),
            fpm_socket: "/run/php83.sock".into(),
            extensions: vec![],
        });
        s.add_vhost(v).unwrap();
        assert_eq!(s.vhosts.len(), 1);
    }
}
