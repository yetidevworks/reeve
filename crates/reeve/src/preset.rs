//! Framework presets: the per-app-type knowledge each backend needs to render a
//! correct vhost. The conventional public subdirectory lives on
//! [`crate::state::Framework::public_subdir`]; this module holds the
//! rewrite-rule differences, which today only nginx needs explicitly (Caddy's
//! `php_fastcgi` and Apache's `.htaccess`/`AllowOverride All` already do the
//! right thing for every preset).

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
