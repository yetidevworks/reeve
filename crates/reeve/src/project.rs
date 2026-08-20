//! Per-project overrides: an optional `.reeve.toml` in a project's root
//! directory. It travels with the project (gitignore it if it holds secrets),
//! works for parked and declared sites alike, and is read fresh on every
//! `apply`, so editing it and re-applying is all it takes.
//!
//! ```toml
//! # .reeve.toml
//! docroot = "dist"        # serve this subdir instead of the preset's default
//! preset = "grav"         # override framework auto-detection / the vhost preset
//!
//! [env]                   # passed to PHP-FPM per request: getenv() / $_SERVER
//! DB_USER = "root"
//! DB_PASS = "secret"
//! ```
//!
//! Every key is optional. `docroot` is relative to the project directory and
//! wins over the preset's public subdir. `env` entries are rendered into the
//! vhost on every backend (Apache `SetEnv`, nginx `fastcgi_param`, Caddy
//! `php_fastcgi { env … }`), so they reach PHP the same way regardless of
//! which server fronts the site.

use crate::state::{join_subdir, Framework, Vhost};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path};

/// File name looked up in the project root.
pub const FILE_NAME: &str = ".reeve.toml";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProjectConfig {
    /// Subdirectory (relative to the project root) to serve as the docroot.
    #[serde(default)]
    pub docroot: Option<String>,
    /// Framework preset, overriding parked auto-detection or a vhost's preset.
    #[serde(default)]
    pub preset: Option<Framework>,
    /// Environment variables handed to PHP-FPM with every request.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl ProjectConfig {
    /// Reject anything that can't be rendered safely into a server config:
    /// an absolute or parent-escaping docroot, env names that aren't valid
    /// identifiers, and values carrying line breaks (config injection).
    fn validate(&self, path: &Path) -> Result<()> {
        if let Some(d) = &self.docroot {
            let rel = Path::new(d);
            if rel.is_absolute() {
                bail!(
                    "{}: `docroot` must be relative to the project (got '{d}')",
                    path.display()
                );
            }
            if rel.components().any(|c| c == Component::ParentDir) {
                bail!(
                    "{}: `docroot` must stay inside the project (got '{d}')",
                    path.display()
                );
            }
        }
        for (k, v) in &self.env {
            let mut chars = k.chars();
            let ok_first = chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
            let ok_rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !ok_first || !ok_rest {
                bail!(
                    "{}: env name '{k}' is not a valid variable name \
                     (letters, digits and `_`, not starting with a digit)",
                    path.display()
                );
            }
            if v.chars().any(|c| c.is_control()) {
                bail!(
                    "{}: env value for '{k}' contains a control character",
                    path.display()
                );
            }
        }
        Ok(())
    }
}

/// Read `<dir>/.reeve.toml`. `Ok(None)` when there is no such file; an error
/// (naming the file) when it exists but is malformed or fails validation.
pub fn load(dir: &Path) -> Result<Option<ProjectConfig>> {
    let path = dir.join(FILE_NAME);
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    let cfg: ProjectConfig =
        toml::from_str(&text).with_context(|| format!("Invalid {}", path.display()))?;
    cfg.validate(&path)?;
    Ok(Some(cfg))
}

/// A vhost's serving parameters after the project file has had its say.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Directory the webserver actually serves.
    pub docroot: String,
    /// Preset whose rewrite/security rules apply.
    pub preset: Framework,
    /// Environment variables to pass to PHP-FPM.
    pub env: BTreeMap<String, String>,
}

/// Combine a vhost record with its project's `.reeve.toml` (if any). The
/// docroot field of a vhost is the *project* root; the served docroot is that
/// plus either the file's `docroot` or the preset's public subdir.
pub fn resolve(v: &Vhost) -> Result<Resolved> {
    let project = load(Path::new(&v.docroot))?.unwrap_or_default();
    let preset = project.preset.unwrap_or(v.preset);
    let docroot = match project.docroot.as_deref() {
        Some(sub) => join_subdir(&v.docroot, sub),
        None => join_subdir(&v.docroot, preset.public_subdir()),
    };
    Ok(Resolved {
        docroot,
        preset,
        env: project.env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!("reeve-project-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn vhost(root: &Path, preset: Framework) -> Vhost {
        Vhost {
            server_name: "app.test".into(),
            server: "caddy".into(),
            docroot: root.display().to_string(),
            php_version: "8.3".into(),
            ssl: false,
            preset,
            proxy_target: None,
        }
    }

    #[test]
    fn no_file_falls_back_to_preset() {
        let base = tmp("nofile");
        let r = resolve(&vhost(&base, Framework::Laravel)).unwrap();
        assert_eq!(r.docroot, format!("{}/public", base.display()));
        assert_eq!(r.preset, Framework::Laravel);
        assert!(r.env.is_empty());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn file_overrides_docroot_preset_and_adds_env() {
        let base = tmp("full");
        fs::write(
            base.join(FILE_NAME),
            "docroot = \"dist/\"\npreset = \"grav\"\n[env]\nDB_USER = \"root\"\nDB_PASS = \"p@ss word\"\n",
        )
        .unwrap();
        let r = resolve(&vhost(&base, Framework::Laravel)).unwrap();
        assert_eq!(r.docroot, format!("{}/dist", base.display()));
        assert_eq!(r.preset, Framework::Grav);
        assert_eq!(r.env.get("DB_USER").map(String::as_str), Some("root"));
        assert_eq!(r.env.get("DB_PASS").map(String::as_str), Some("p@ss word"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn preset_only_still_uses_its_subdir() {
        let base = tmp("preset");
        fs::write(base.join(FILE_NAME), "preset = \"drupal\"\n").unwrap();
        let r = resolve(&vhost(&base, Framework::Generic)).unwrap();
        assert_eq!(r.docroot, format!("{}/web", base.display()));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_bad_docroot_and_env() {
        let base = tmp("bad");
        let cases = [
            "docroot = \"/etc\"\n",
            "docroot = \"../other\"\n",
            "[env]\n\"1BAD\" = \"x\"\n",
            "[env]\n\"DB-USER\" = \"x\"\n",
            "[env]\nOK = \"line\\nbreak\"\n",
            "preset = \"rails\"\n",
            "docroot = [1]\n",
        ];
        for c in cases {
            fs::write(base.join(FILE_NAME), c).unwrap();
            let err = load(&base).unwrap_err().to_string();
            assert!(
                err.contains(FILE_NAME),
                "error should name the file: {err} (case {c:?})"
            );
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_file_is_none() {
        let base = tmp("none");
        assert!(load(&base).unwrap().is_none());
        let _ = fs::remove_dir_all(&base);
    }
}
