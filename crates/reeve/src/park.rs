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
/// right docroot + rewrites automatically. Falls back to `Generic`.
fn detect_framework(dir: &Path) -> Framework {
    // Laravel/Symfony: a `public/` dir with a front controller.
    if dir.join("public/index.php").is_file() {
        if dir.join("artisan").is_file() {
            return Framework::Laravel;
        }
        if dir.join("bin/console").is_file() || dir.join("symfony.lock").is_file() {
            return Framework::Symfony;
        }
        // A bare public/index.php is still Laravel-shaped (public docroot).
        return Framework::Laravel;
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
    Framework::Generic
}

/// True if a directory has any web content worth serving (an index file in the
/// root or in a conventional public subdir).
fn is_web_project(dir: &Path) -> bool {
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
        out.push(Vhost {
            server_name: format!("{}.{}", name.to_lowercase(), park.tld),
            server: park.server.clone(),
            docroot: path.display().to_string(),
            php_version: park.php_version.clone(),
            ssl: park.ssl,
            preset: detect_framework(&path),
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
            shop.effective_docroot(),
            format!("{}/shop/public", base.display())
        );
        assert!(shop.ssl);

        let _ = fs::remove_dir_all(&base);
    }
}
