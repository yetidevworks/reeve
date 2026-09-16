//! Apache module catalog: which `mod_*.so` the installed httpd ships, which
//! ones reeve always loads, and which ones a server has opted into.
//!
//! Unlike PHP extensions there is nothing to build — httpd ships ~110 shared
//! objects and "enabling" one is just a `LoadModule` line in the generated
//! conf. So this is discovery plus bookkeeping, not a package manager.

use crate::brew::Brew;
use crate::state::Server;
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Modules a minimal-but-functional httpd needs to start, serve, and proxy to
/// PHP-FPM. (module name, shared-object filename). Always loaded; not pickable.
pub const BASE_MODULES: &[(&str, &str)] = &[
    ("mpm_event_module", "mod_mpm_event.so"),
    ("authz_core_module", "mod_authz_core.so"),
    ("unixd_module", "mod_unixd.so"),
    ("log_config_module", "mod_log_config.so"),
    ("mime_module", "mod_mime.so"),
    ("dir_module", "mod_dir.so"),
    ("proxy_module", "mod_proxy.so"),
    ("proxy_fcgi_module", "mod_proxy_fcgi.so"),
    ("rewrite_module", "mod_rewrite.so"),
    ("headers_module", "mod_headers.so"),
    ("setenvif_module", "mod_setenvif.so"),
    // SetEnv/PassEnv/UnsetEnv — per-site env vars from `.reeve.toml` and from
    // a project's own .htaccess (AllowOverride All). Without it a `SetEnv` in
    // .htaccess is an "Invalid command" and every request 500s.
    ("env_module", "mod_env.so"),
    // Alias/Redirect/RedirectMatch. Every vhost renders `AllowOverride All`,
    // so a project's own .htaccess is free to use these — and a bare
    // `Redirect 301 /old /new` is an "Invalid command" 500 without it. Stock
    // httpd.conf loads it too.
    ("alias_module", "mod_alias.so"),
    // Directory listings. Every vhost renders `Options Indexes`, which is a
    // silent no-op (404 on an index-less directory) unless this is loaded.
    ("autoindex_module", "mod_autoindex.so"),
];

/// Loaded on top of [`BASE_MODULES`] whenever a server serves HTTPS.
pub const SSL_MODULES: &[(&str, &str)] = &[
    ("ssl_module", "mod_ssl.so"),
    ("socache_shmcb_module", "mod_socache_shmcb.so"),
];

/// The three MPMs are mutually exclusive — loading a second one is a hard
/// startup failure (`AH00534: Configuration error: More than one MPM loaded`),
/// so reeve pins mpm_event in [`BASE_MODULES`] and keeps every `mpm_*` out of
/// the pickable catalog entirely.
const MPM_PREFIX: &str = "mpm_";

/// Modules that need other modules loaded alongside them. httpd does not
/// resolve these itself — you get an "Invalid command" 500 or a failed config
/// test instead — so reeve pulls prerequisites in automatically. Expanded
/// transitively, so `lbmethod_*` reaches `proxy` via `proxy_balancer`.
const PREREQS: &[(&str, &[&str])] = &[
    // Everything under mod_proxy needs the core proxy module.
    ("proxy_ajp_module", &["proxy_module"]),
    (
        "proxy_balancer_module",
        &["proxy_module", "slotmem_shm_module"],
    ),
    ("proxy_connect_module", &["proxy_module"]),
    ("proxy_express_module", &["proxy_module"]),
    ("proxy_fdpass_module", &["proxy_module"]),
    ("proxy_ftp_module", &["proxy_module"]),
    ("proxy_hcheck_module", &["proxy_module", "watchdog_module"]),
    ("proxy_html_module", &["proxy_module", "xml2enc_module"]),
    ("proxy_http_module", &["proxy_module"]),
    ("proxy_scgi_module", &["proxy_module"]),
    ("proxy_uwsgi_module", &["proxy_module"]),
    ("proxy_wstunnel_module", &["proxy_module"]),
    // Load balancer methods sit on top of the balancer.
    ("lbmethod_bybusyness_module", &["proxy_balancer_module"]),
    ("lbmethod_byrequests_module", &["proxy_balancer_module"]),
    ("lbmethod_bytraffic_module", &["proxy_balancer_module"]),
    (
        "lbmethod_heartbeat_module",
        &["proxy_balancer_module", "slotmem_shm_module"],
    ),
    // WebDAV providers need the DAV core.
    ("dav_fs_module", &["dav_module"]),
    ("dav_lock_module", &["dav_module"]),
    // Cache storage backends need the cache core.
    ("cache_disk_module", &["cache_module"]),
    (
        "cache_socache_module",
        &["cache_module", "socache_shmcb_module"],
    ),
    // Session storage backends need the session core.
    ("session_cookie_module", &["session_module"]),
    ("session_crypto_module", &["session_module"]),
    ("session_dbd_module", &["session_module", "dbd_module"]),
    // Authn providers need the authn core (and a SQL backend needs mod_dbd).
    ("authn_anon_module", &["authn_core_module"]),
    ("authn_dbd_module", &["authn_core_module", "dbd_module"]),
    ("authn_dbm_module", &["authn_core_module"]),
    ("authn_file_module", &["authn_core_module"]),
    (
        "authn_socache_module",
        &["authn_core_module", "socache_shmcb_module"],
    ),
    ("authz_dbd_module", &["dbd_module"]),
    // Auth front-ends are useless without a provider to check against and an
    // authz provider for `Require valid-user`.
    (
        "auth_basic_module",
        &[
            "authn_core_module",
            "authn_file_module",
            "authz_user_module",
        ],
    ),
    (
        "auth_digest_module",
        &[
            "authn_core_module",
            "authn_file_module",
            "authz_user_module",
        ],
    ),
    (
        "auth_form_module",
        &[
            "authn_core_module",
            "authn_file_module",
            "authz_user_module",
            "session_module",
            "session_cookie_module",
        ],
    ),
    // `AddOutputFilterByType` — the usual way these get wired up — is
    // mod_filter's directive, so a bare mod_deflate 500s a .htaccess.
    ("brotli_module", &["filter_module"]),
    ("deflate_module", &["filter_module"]),
    ("ext_filter_module", &["filter_module"]),
    ("substitute_module", &["filter_module"]),
    // Heartbeat/watchdog pair.
    ("heartbeat_module", &["watchdog_module"]),
    (
        "heartmonitor_module",
        &["watchdog_module", "slotmem_shm_module"],
    ),
    // reeve's generated `<Location>` for these uses `Require local`, whose
    // `local` provider lives in mod_authz_host. See [`snippet`].
    ("info_module", &["authz_host_module"]),
    ("status_module", &["authz_host_module"]),
    // Session cache provider for TLS resumption.
    ("ssl_module", &["socache_shmcb_module"]),
];

/// Where the installed httpd keeps its shared objects.
pub fn modules_dir(brew: &Brew) -> PathBuf {
    brew.opt("httpd").join("lib/httpd/modules")
}

/// `mod_rewrite.so` → `rewrite_module`. Verified to match the module symbol
/// each `.so` actually exports for all 112 modules brew's httpd 2.4 ships.
pub fn name_for_file(file: &str) -> Option<String> {
    let short = file.strip_suffix(".so")?.strip_prefix("mod_")?;
    if short.is_empty() {
        return None;
    }
    Some(format!("{short}_module"))
}

/// `rewrite_module` → `mod_rewrite.so`.
pub fn file_for_name(name: &str) -> String {
    format!("mod_{}.so", name.strip_suffix("_module").unwrap_or(name))
}

/// Normalize whatever the user typed. `rewrite`, `mod_rewrite`,
/// `mod_rewrite.so` and `rewrite_module` all mean the same module.
pub fn canonical(input: &str) -> String {
    let lower = input.trim().to_lowercase();
    let s = lower.strip_suffix(".so").unwrap_or(&lower);
    let s = s.strip_prefix("mod_").unwrap_or(s);
    if s.ends_with("_module") {
        s.to_string()
    } else {
        format!("{s}_module")
    }
}

/// Is this one of the modules reeve always loads?
pub fn is_base(name: &str) -> bool {
    BASE_MODULES.iter().any(|(n, _)| *n == name)
}

/// Is this one of the modules reeve loads automatically for HTTPS?
pub fn is_ssl(name: &str) -> bool {
    SSL_MODULES.iter().any(|(n, _)| *n == name)
}

/// A module plus every module it transitively needs, deduped and sorted.
pub fn with_prereqs(names: &[String]) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = names.iter().map(|n| canonical(n)).collect();
    while let Some(n) = queue.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some((_, deps)) = PREREQS.iter().find(|(k, _)| *k == n) {
            queue.extend(deps.iter().map(|d| (*d).to_string()));
        }
    }
    seen.into_iter().collect()
}

/// Which of `enabled` would lose a prerequisite if `name` were turned off.
/// Used to refuse a removal that would break another module rather than
/// silently producing a config that fails its test.
pub fn dependents_of(name: &str, enabled: &[String]) -> Vec<String> {
    let name = canonical(name);
    enabled
        .iter()
        .map(|e| canonical(e))
        .filter(|e| *e != name && with_prereqs(std::slice::from_ref(e)).contains(&name))
        .collect()
}

/// Config a module needs before it does anything visible. Only the handful
/// that would otherwise look broken — loaded, but with nothing to show for it.
pub fn snippet(name: &str) -> Option<&'static str> {
    match name {
        "status_module" => Some(concat!(
            "<IfModule status_module>\n",
            "    ExtendedStatus On\n",
            "    <Location /server-status>\n",
            "        SetHandler server-status\n",
            "        Require local\n",
            "    </Location>\n",
            "</IfModule>\n\n",
        )),
        "info_module" => Some(concat!(
            "<IfModule info_module>\n",
            "    <Location /server-info>\n",
            "        SetHandler server-info\n",
            "        Require local\n",
            "    </Location>\n",
            "</IfModule>\n\n",
        )),
        _ => None,
    }
}

/// Where a module came from, for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Always loaded by reeve; not pickable.
    Base,
    /// Loaded automatically whenever the server serves HTTPS.
    Ssl,
    /// Opt-in.
    Optional,
}

/// One row of the catalog: a module the installed httpd ships.
#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub origin: Origin,
    /// Chosen explicitly on this server.
    pub enabled: bool,
    /// Pulled in as some other enabled module's prerequisite rather than
    /// chosen directly.
    pub implied: bool,
}

/// Every module name the installed httpd ships, `mpm_*` excluded, sorted.
pub fn installed(brew: &Brew) -> Result<Vec<String>> {
    let dir = modules_dir(brew);
    let rd = std::fs::read_dir(&dir).with_context(|| {
        format!(
            "No Apache modules at {} — is httpd installed?",
            dir.display()
        )
    })?;
    let mut names: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|f| name_for_file(&f))
        .filter(|n| !n.starts_with(MPM_PREFIX))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// The full catalog for a server: every shipped module, marked with where it
/// comes from and whether it is on.
///
/// `installed` hides every MPM, but reeve does load `mpm_event`, so the base
/// set is unioned back in — a list of what's loaded that omits the loaded MPM
/// would just be wrong. It comes back as [`Origin::Base`], so callers still
/// treat it as not-toggleable.
pub fn catalog(brew: &Brew, server: &Server) -> Result<Vec<Module>> {
    let chosen: BTreeSet<String> = server.modules.iter().map(|m| canonical(m)).collect();
    let effective: BTreeSet<String> = with_prereqs(&server.modules).into_iter().collect();
    let mut names: BTreeSet<String> = installed(brew)?.into_iter().collect();
    let dir = modules_dir(brew);
    for (name, file) in BASE_MODULES.iter().chain(SSL_MODULES) {
        if dir.join(file).exists() {
            names.insert((*name).to_string());
        }
    }
    Ok(names
        .into_iter()
        .map(|name| {
            let origin = if is_base(&name) {
                Origin::Base
            } else if is_ssl(&name) {
                Origin::Ssl
            } else {
                Origin::Optional
            };
            Module {
                enabled: chosen.contains(&name),
                implied: !chosen.contains(&name) && effective.contains(&name),
                origin,
                name,
            }
        })
        .collect())
}

/// Canonicalize a user-supplied module name and check it can actually be
/// enabled on this httpd, with a useful error when it can't.
pub fn resolve_choice(brew: &Brew, input: &str) -> Result<String> {
    let name = canonical(input);
    if name.starts_with(MPM_PREFIX) {
        bail!(
            "'{name}' is an MPM — only one may be loaded and reeve pins mpm_event. \
             Loading a second is a hard startup failure."
        );
    }
    if is_base(&name) {
        bail!("'{name}' is always loaded by reeve; nothing to enable.");
    }
    let installed = installed(brew)?;
    if installed.contains(&name) {
        return Ok(name);
    }
    // Offer the closest few names rather than just "not found". A plain
    // substring test misses the common case (a single wrong letter, as in
    // `wstunel`), so rank by edit distance and keep substring hits alongside.
    let stem = name.trim_end_matches("_module");
    let mut near: Vec<(usize, &String)> = installed
        .iter()
        .filter_map(|n| {
            let other = n.trim_end_matches("_module");
            let d = edit_distance(stem, other);
            let close = d <= 3 || other.contains(stem) || stem.contains(other);
            close.then_some((d, n))
        })
        .collect();
    near.sort_by_key(|(d, n)| (*d, (*n).clone()));
    near.truncate(5);
    if near.is_empty() {
        bail!(
            "No module '{}' in {}",
            file_for_name(&name),
            modules_dir(brew).display()
        );
    }
    bail!(
        "No module '{}'. Did you mean: {}?",
        file_for_name(&name),
        near.iter()
            .map(|(_, s)| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// Levenshtein distance, for "did you mean" suggestions. Two rolling rows —
/// the inputs are short module names, so this stays trivial.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The `LoadModule` set for a server, in emit order: base, SSL (when the server
/// serves HTTPS), then the opted-in modules and their prerequisites
/// alphabetically. Deduped, and entries whose `.so` is missing are dropped —
/// the same tolerance the base list has always had.
pub fn load_list(brew: &Brew, server: &Server, needs_ssl: bool) -> Vec<(String, PathBuf)> {
    let dir = modules_dir(brew);
    let mut emitted: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    let mut push = |name: String, out: &mut Vec<(String, PathBuf)>| {
        if !emitted.insert(name.clone()) {
            return;
        }
        let path = dir.join(file_for_name(&name));
        if path.exists() {
            out.push((name, path));
        }
    };
    for (name, _) in BASE_MODULES {
        push((*name).to_string(), &mut out);
    }
    if needs_ssl {
        for (name, _) in SSL_MODULES {
            push((*name).to_string(), &mut out);
        }
    }
    for name in with_prereqs(&server.modules) {
        push(name, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_with_the_file_convention() {
        assert_eq!(name_for_file("mod_rewrite.so").unwrap(), "rewrite_module");
        assert_eq!(
            name_for_file("mod_lbmethod_bybusyness.so").unwrap(),
            "lbmethod_bybusyness_module"
        );
        assert_eq!(file_for_name("rewrite_module"), "mod_rewrite.so");
        assert_eq!(
            file_for_name("socache_shmcb_module"),
            "mod_socache_shmcb.so"
        );
        // Not a module file.
        assert!(name_for_file("httpd.exp").is_none());
        assert!(name_for_file("mod_.so").is_none());
    }

    #[test]
    fn edit_distance_measures_typos() {
        assert_eq!(edit_distance("deflate", "deflate"), 0);
        assert_eq!(edit_distance("defalte", "deflate"), 2);
        assert_eq!(edit_distance("proxy_wstunel", "proxy_wstunnel"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[test]
    fn canonical_accepts_every_spelling() {
        for input in [
            "rewrite",
            "mod_rewrite",
            "mod_rewrite.so",
            "rewrite_module",
            "  MOD_Rewrite.SO  ",
        ] {
            assert_eq!(canonical(input), "rewrite_module", "input was {input:?}");
        }
    }

    #[test]
    fn prereqs_expand_transitively() {
        let got = with_prereqs(&["lbmethod_bybusyness".to_string()]);
        // lbmethod → balancer → proxy + slotmem.
        assert!(got.contains(&"lbmethod_bybusyness_module".to_string()));
        assert!(got.contains(&"proxy_balancer_module".to_string()));
        assert!(got.contains(&"proxy_module".to_string()));
        assert!(got.contains(&"slotmem_shm_module".to_string()));
    }

    #[test]
    fn prereqs_terminate_and_dedupe() {
        // Two modules sharing a prerequisite yield it once.
        let got = with_prereqs(&["deflate".to_string(), "brotli".to_string()]);
        assert_eq!(
            got.iter().filter(|n| *n == "filter_module").count(),
            1,
            "shared prerequisite should appear once: {got:?}"
        );
    }

    #[test]
    fn dependents_block_a_removal_that_would_break_config() {
        let enabled = vec!["proxy_balancer".to_string(), "deflate".to_string()];
        // slotmem_shm is a prerequisite of proxy_balancer.
        let deps = dependents_of("slotmem_shm", &enabled);
        assert_eq!(deps, vec!["proxy_balancer_module".to_string()]);
        // filter is only needed by deflate.
        assert_eq!(
            dependents_of("filter", &enabled),
            vec!["deflate_module".to_string()]
        );
        // Nothing depends on deflate itself.
        assert!(dependents_of("deflate", &enabled).is_empty());
    }

    #[test]
    fn every_prereq_key_is_unique() {
        let mut keys: Vec<&str> = PREREQS.iter().map(|(k, _)| *k).collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(before, keys.len(), "duplicate key in PREREQS");
    }

    #[test]
    fn prereq_graph_has_no_cycles_and_names_are_canonical() {
        for (k, deps) in PREREQS {
            assert_eq!(canonical(k), *k, "{k} is not a canonical name");
            for d in *deps {
                assert_eq!(canonical(d), *d, "{d} is not a canonical name");
            }
            // with_prereqs would hang rather than return on a cycle.
            let closure = with_prereqs(&[(*k).to_string()]);
            assert!(closure.contains(&(*k).to_string()));
        }
    }

    #[test]
    fn base_modules_need_no_prereqs_beyond_themselves() {
        // A base module listed as someone's prerequisite is fine, but a base
        // module must not itself depend on something reeve never loads.
        for (name, _) in BASE_MODULES {
            for dep in with_prereqs(&[(*name).to_string()]) {
                assert!(
                    is_base(&dep) || is_ssl(&dep),
                    "base module {name} needs {dep}, which reeve doesn't always load"
                );
            }
        }
    }

    #[test]
    fn snippets_only_exist_for_modules_that_declare_authz_host() {
        // Both snippets use `Require local`, so both must pull in mod_authz_host.
        for name in ["status_module", "info_module"] {
            assert!(snippet(name).is_some());
            assert!(
                with_prereqs(&[name.to_string()]).contains(&"authz_host_module".to_string()),
                "{name}'s snippet uses `Require local` but doesn't require authz_host"
            );
        }
        assert!(snippet("rewrite_module").is_none());
    }
}
