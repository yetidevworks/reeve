//! Valet-style directory parking. A [`Park`] points at a directory; every
//! immediate subfolder that looks like a web project is served automatically as
//! `<subfolder>.<tld>`, with no per-project `vhost add`. New folders appear on
//! the next `apply`. Parked sites are *derived* — they never live in
//! `state.vhosts`; they're materialized into synthetic [`Vhost`]s at render time.

use crate::state::{Framework, Park, State, Vhost};
use std::fs;
use std::path::Path;

/// Files whose presence marks a subfolder as a servable web project.
const INDEX_MARKERS: [&str; 3] = ["index.php", "index.html", "index.htm"];

/// Detect a framework from a project directory's layout, so parked sites get the
/// right docroot + rewrites automatically. Anything else with a `public/`
/// index (a plain front controller, a static build) becomes `Public` so it's
/// served from that subdir; otherwise `Generic` serves the project root.
fn detect_framework(dir: &Path) -> Framework {
    // Laravel/Symfony: a `public/` dir with a front controller.
    if dir.join("public/index.php").is_file() {
        if dir.join("artisan").is_file() {
            return Framework::Laravel;
        }
        if dir.join("bin/console").is_file() || dir.join("symfony.lock").is_file() {
            return Framework::Symfony;
        }
        return Framework::Public;
    }
    if dir.join("web/index.php").is_file() {
        return Framework::Drupal;
    }
    if dir.join("wp-config.php").is_file() || dir.join("wp-load.php").is_file() {
        return Framework::Wordpress;
    }
    if dir.join("system/defines.php").is_file() && dir.join("user").is_dir() {
        return Framework::Grav;
    }
    // A static `public/` (e.g. a Vite/Astro/Eleventy build) — still a public
    // docroot, just no PHP front controller.
    if INDEX_MARKERS
        .iter()
        .any(|m| dir.join("public").join(m).is_file())
    {
        return Framework::Public;
    }
    Framework::Generic
}

/// The preset a parked folder is served with: its `.reeve.toml` `preset` if it
/// declares one, else layout detection. A malformed project file is ignored
/// here (listing must not fail); `apply` reports it when rendering.
fn preset_for(dir: &Path) -> Framework {
    crate::project::load(dir)
        .ok()
        .flatten()
        .and_then(|p| p.preset)
        .unwrap_or_else(|| detect_framework(dir))
}

/// Convert a folder name into a DNS-safe host label (the bit before the TLD).
/// Lowercases, keeps letters/digits/`.`, turns any other character (spaces,
/// underscores, …) into `-`, collapses repeated separators, and trims them off
/// the ends. Returns `None` when nothing valid is left, so the folder is
/// skipped rather than producing a hostname mkcert and the webserver reject
/// (e.g. a folder literally named `grav-helios 2` → `grav-helios-2`).
fn host_label(name: &str) -> Option<String> {
    let mut out = String::with_capacity(name.len());
    let mut prev_sep = true; // leading-separator guard
    for ch in name.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_sep = false;
        } else if c == '.' {
            if !prev_sep {
                out.push('.');
                prev_sep = true;
            }
        } else if !prev_sep {
            out.push('-');
            prev_sep = true;
        }
    }
    while out.ends_with('-') || out.ends_with('.') {
        out.pop();
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// True if a directory has any web content worth serving (an index file in the
/// root or in a conventional public subdir), or a `.reeve.toml` that says so.
fn is_web_project(dir: &Path) -> bool {
    if dir.join(crate::project::FILE_NAME).is_file() {
        return true;
    }
    if INDEX_MARKERS.iter().any(|m| dir.join(m).is_file()) {
        return true;
    }
    for sub in ["public", "web"] {
        if INDEX_MARKERS
            .iter()
            .any(|m| dir.join(sub).join(m).is_file())
        {
            return true;
        }
    }
    // A framework we recognize counts even without an index yet (e.g. Laravel
    // before `composer install`).
    detect_framework(dir) != Framework::Generic
}

/// Expand one park into synthetic vhosts, one per servable subfolder.
pub fn expand(park: &Park) -> Vec<Vhost> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let Ok(entries) = fs::read_dir(&park.root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Skip hidden/dot folders.
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if !is_web_project(&path) {
            continue;
        }
        // Skip names that can't form a valid hostname (rather than abort apply).
        let Some(label) = host_label(name) else {
            continue;
        };
        let server_name = format!("{label}.{}", park.tld);
        // Two folders can sanitize to the same label — first one wins.
        if !seen.insert(server_name.clone()) {
            continue;
        }
        out.push(Vhost {
            server_name,
            server: park.server.clone(),
            docroot: path.display().to_string(),
            php_version: park.php_version.clone(),
            ssl: park.ssl,
            preset: preset_for(&path),
            proxy_target: None,
        });
    }
    out.sort_by(|a, b| a.server_name.cmp(&b.server_name));
    out
}

/// Every parked subfolder across all parks, skipping any host already declared
/// as a real vhost (an explicit vhost wins over a parked one).
pub fn expand_all(state: &State) -> Vec<Vhost> {
    let declared: std::collections::HashSet<&str> = state
        .vhosts
        .iter()
        .map(|v| v.server_name.as_str())
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for park in &state.parks {
        for v in expand(park) {
            if declared.contains(v.server_name.as_str()) {
                continue;
            }
            if seen.insert(v.server_name.clone()) {
                out.push(v);
            }
        }
    }
    out
}

/// All vhosts a server should serve: its declared vhosts plus any parked
/// subfolders pointed at it. Returned owned so callers can borrow uniformly.
pub fn effective_vhosts_for(state: &State, server: &str) -> Vec<Vhost> {
    let mut out: Vec<Vhost> = state
        .vhosts
        .iter()
        .filter(|v| v.server == server)
        .cloned()
        .collect();
    for v in expand_all(state) {
        if v.server == server {
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_label_sanitizes_and_skips() {
        assert_eq!(
            host_label("grav-helios 2").as_deref(),
            Some("grav-helios-2")
        );
        assert_eq!(host_label("My_Project").as_deref(), Some("my-project"));
        assert_eq!(host_label("admin.bak").as_deref(), Some("admin.bak"));
        assert_eq!(host_label("  spaced  ").as_deref(), Some("spaced"));
        assert_eq!(host_label("café").as_deref(), Some("caf")); // non-ascii dropped
        assert_eq!(host_label("   ").as_deref(), None); // nothing valid → skip
        assert_eq!(host_label("...").as_deref(), None);
    }

    #[test]
    fn expand_picks_web_projects_and_detects_laravel() {
        // Build a throwaway parked tree: blog (plain), shop (Laravel), docs
        // (no index — skipped), .hidden (dot — skipped).
        let base = std::env::temp_dir().join(format!("reeve-park-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("blog")).unwrap();
        fs::write(base.join("blog/index.php"), "<?php").unwrap();
        fs::create_dir_all(base.join("shop/public")).unwrap();
        fs::write(base.join("shop/public/index.php"), "<?php").unwrap();
        fs::write(base.join("shop/artisan"), "").unwrap();
        fs::create_dir_all(base.join("docs")).unwrap();
        fs::create_dir_all(base.join(".hidden")).unwrap();
        fs::write(base.join(".hidden/index.php"), "<?php").unwrap();

        let park = Park {
            root: base.display().to_string(),
            server: "caddy".into(),
            php_version: "8.3".into(),
            tld: "test".into(),
            ssl: true,
        };
        let sites = expand(&park);
        let names: Vec<&str> = sites.iter().map(|v| v.server_name.as_str()).collect();
        assert_eq!(names, vec!["blog.test", "shop.test"]); // sorted, docs/.hidden skipped

        let shop = sites.iter().find(|v| v.server_name == "shop.test").unwrap();
        assert_eq!(shop.preset, Framework::Laravel);
        assert_eq!(
            crate::project::resolve(shop).unwrap().docroot,
            format!("{}/shop/public", base.display())
        );
        assert!(shop.ssl);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn bare_public_dirs_get_public_preset_and_project_file_wins() {
        // api: public/index.php, no framework markers → `public` preset.
        // site: public/index.html only (static build) → `public` preset.
        // custom: no index anywhere, but a .reeve.toml → served, preset from file.
        let base = std::env::temp_dir().join(format!("reeve-park-pub-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("api/public")).unwrap();
        fs::write(base.join("api/public/index.php"), "<?php").unwrap();
        fs::create_dir_all(base.join("site/public")).unwrap();
        fs::write(base.join("site/public/index.html"), "<html>").unwrap();
        fs::create_dir_all(base.join("custom/dist")).unwrap();
        fs::write(
            base.join("custom/.reeve.toml"),
            "docroot = \"dist\"\npreset = \"grav\"\n",
        )
        .unwrap();

        let park = Park {
            root: base.display().to_string(),
            server: "apache".into(),
            php_version: "8.3".into(),
            tld: "test".into(),
            ssl: false,
        };
        let sites = expand(&park);
        let names: Vec<&str> = sites.iter().map(|v| v.server_name.as_str()).collect();
        assert_eq!(names, vec!["api.test", "custom.test", "site.test"]);

        let by = |n: &str| sites.iter().find(|v| v.server_name == n).unwrap();
        assert_eq!(by("api.test").preset, Framework::Public);
        let docroot = |n: &str| crate::project::resolve(by(n)).unwrap().docroot;
        assert_eq!(
            docroot("api.test"),
            format!("{}/api/public", base.display())
        );
        assert_eq!(by("site.test").preset, Framework::Public);
        assert_eq!(
            docroot("site.test"),
            format!("{}/site/public", base.display())
        );
        assert_eq!(by("custom.test").preset, Framework::Grav);
        let resolved = crate::project::resolve(by("custom.test")).unwrap();
        assert_eq!(resolved.docroot, format!("{}/custom/dist", base.display()));

        let _ = fs::remove_dir_all(&base);
    }
}
