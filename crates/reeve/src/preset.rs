//! Framework presets: the per-app-type knowledge each backend needs to render a
//! correct vhost — the conventional public subdirectory (on
//! [`crate::state::Framework::public_subdir`]), the front-controller
//! `try_files` fallback, and the security rules that block direct access to
//! sensitive paths (which Apache gets for free from the app's `.htaccess`, but
//! nginx and Caddy must be told explicitly).

use crate::state::Framework;

/// The `try_files` target for an nginx `location /` block, per framework.
/// Drupal routes everything through a single front controller without the
/// `$uri/` directory probe; the rest share Laravel/WordPress/Symfony/Grav's
/// standard front-controller fallback.
pub fn nginx_try_files(fw: Framework) -> &'static str {
    match fw {
        Framework::Drupal => "$uri /index.php?$query_string",
        _ => "$uri $uri/ /index.php?$query_string",
    }
}

/// Security `location` blocks for an nginx `server { }`, 8-space indented to sit
/// alongside the other vhost directives. Must be emitted BEFORE the
/// `location ~ \.php$` block: nginx evaluates regex locations in order and takes
/// the first match, so these have to win over the generic PHP handler for paths
/// like `/user/accounts/…`. Empty for frameworks without specific rules.
///
/// Grav's rules mirror its shipped `webserver-configs/nginx.conf`: block the
/// VCS/cache/log dirs, the sensitive `user/{accounts,config,env}` folders, and
/// all of `user/data` except public media uploads, plus scripts under
/// system/vendor/user and a few root files.
pub fn nginx_security(fw: Framework) -> &'static str {
    match fw {
        Framework::Grav => {
            r#"        location ~* /(\.git|cache|bin|logs|backup|tests)/.*$ { return 403; }
        location ~* /user/(accounts|config|env)/.*$ { return 403; }
        location ~* /user/data/.*\.(jpe?g|png|gif|webp|avif|bmp|ico|mp4|webm|ogg|ogv|mov|mp3|wav|m4a|flac|pdf)$ { try_files $uri =404; }
        location ~* /user/data/.*$ { return 403; }
        location ~* /(system|vendor)/.*\.(txt|xml|md|html|htm|shtml|shtm|json|yaml|yml|php|php2|php3|php4|php5|phar|phtml|pl|py|cgi|twig|sh|bat)$ { return 403; }
        location ~* /user/.*\.(txt|md|json|yaml|yml|php|php2|php3|php4|php5|phar|phtml|pl|py|cgi|twig|sh|bat)$ { return 403; }
        location ~ /(LICENSE\.txt|composer\.lock|composer\.json|nginx\.conf|web\.config|htaccess\.txt|\.htaccess) { return 403; }
        location ~ /\.env(\.|$) { return 403; }
"#
        }
        _ => "",
    }
}

/// Security directives for a Caddy site block, tab-indented. Mirrors Grav's
/// shipped `webserver-configs/Caddyfile`: rewrite sensitive paths to an internal
/// sentinel that responds 403, while allowing public media uploads under
/// `user/data`. Caddy orders directives by its built-in directive order, so the
/// placement within the block doesn't matter. Empty for other frameworks.
pub fn caddy_security(fw: Framework) -> &'static str {
    match fw {
        Framework::Grav => {
            // Caddy inline path matchers only do `*` globs, not regex — so each
            // rule needs a `path_regexp` named matcher. `respond` is terminal and
            // ordered before php_fastcgi/file_server by Caddy's directive order,
            // so a matched path returns 403 before it can be served. The
            // user/data matcher AND-combines a path match with a NOT on media
            // extensions, so public uploads (images/video/pdf) still serve.
            "\t@reeve_grav_dirs path_regexp (?i)^/(\\.git|cache|bin|logs|backups?|tests)/\n\
             \t@reeve_grav_userconf path_regexp (?i)^/user/(accounts|config|env)/\n\
             \t@reeve_grav_userdata {\n\
             \t\tpath_regexp (?i)^/user/data/\n\
             \t\tnot path_regexp (?i)\\.(jpe?g|png|gif|webp|avif|bmp|ico|mp4|webm|ogg|ogv|mov|mp3|wav|m4a|flac|pdf)$\n\
             \t}\n\
             \t@reeve_grav_sys path_regexp (?i)^/(system|vendor)/.*\\.(txt|xml|md|html|htm|shtml|shtm|yaml|yml|php|php2|php3|php4|php5|phar|phtml|pl|py|cgi|twig|sh|bat)$\n\
             \t@reeve_grav_userscripts path_regexp (?i)^/user/.*\\.(txt|md|yaml|yml|php|php2|php3|php4|php5|phar|phtml|pl|py|cgi|twig|sh|bat)$\n\
             \t@reeve_grav_rootfiles path_regexp (?i)/(LICENSE\\.txt|composer\\.lock|composer\\.json|nginx\\.conf|web\\.config|htaccess\\.txt|\\.htaccess)$\n\
             \t@reeve_grav_dotenv path_regexp \\.env(\\..+)?$\n\
             \trespond @reeve_grav_dirs 403\n\
             \trespond @reeve_grav_userconf 403\n\
             \trespond @reeve_grav_userdata 403\n\
             \trespond @reeve_grav_sys 403\n\
             \trespond @reeve_grav_userscripts 403\n\
             \trespond @reeve_grav_rootfiles 403\n\
             \trespond @reeve_grav_dotenv 403\n"
        }
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grav_security_blocks_sensitive_and_allows_media() {
        let n = nginx_security(Framework::Grav);
        assert!(n.contains("/user/(accounts|config|env)/"));
        // media extensions get a try_files pass, not a 403
        assert!(n.contains("flac|pdf)$ { try_files"));
        let c = caddy_security(Framework::Grav);
        assert!(c.contains("path_regexp (?i)^/user/(accounts|config|env)/"));
        assert!(c.contains("not path_regexp")); // user/data media carve-out
        assert!(c.contains("respond @reeve_grav_userconf 403"));
    }

    #[test]
    fn generic_has_no_security_rules() {
        assert_eq!(nginx_security(Framework::Generic), "");
        assert_eq!(caddy_security(Framework::Generic), "");
    }
}
