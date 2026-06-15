# Changelog

All notable changes to reeve are documented here.

## 0.1.0 (unreleased)

Initial release.

### Added
- `init` — detect Homebrew (offer to install if missing), scaffold config/state,
  report any existing `php@*` installs for adoption.
- Declarative model: `state.toml` (servers, PHP versions, vhosts) as the single
  source of truth; native configs are rendered into `generated/` and launchd
  services reconciled to match.
- **Per-vhost PHP** via PHP-FPM, one launchd-managed master per version on its
  own socket. `php install` (installs or adopts a brew `php@x.y`), `php list`,
  `php use`.
- **PHP extension management** per version: `php ext add|remove|list` via that
  version's `pecl`, with FPM auto-restart and dangling-ini cleanup on removal.
- **Web server backends** behind one trait: **Caddy**, **Apache** (event MPM +
  mod_proxy_fcgi), **nginx** (fastcgi_pass). `server add|start|stop|restart|list|remove`,
  each server independent and able to run alongside others on different ports.
- **Local SSL** via a shared mkcert CA: `ssl mint|trust|ca`, with auto-minting
  for `--ssl` vhosts during `apply`/`start`.
- **Wildcard DNS** for the dev TLD via a user-run dnsmasq on an unprivileged
  port: `dns setup|status`.
- **TUI dashboard** (run with no arguments): stacked Servers / PHP / Vhosts
  panels with live status and start/stop/restart/apply actions, sharing one
  lifecycle engine with the CLI.
- `apply` (render + reconcile) and `validate` (per-backend native config test).

### Known limitations
- **OpenLiteSpeed** is wired in but unusable on macOS: the only community
  Homebrew tap fails to build its deprecated `admin_php` dependency against the
  macOS 26 SDK. Use Caddy, Apache, or nginx.
- Setting up system-wide `*.test` resolution writes `/etc/resolver/<tld>`, which
  requires a one-time `sudo` (that path is root-owned). Everything else runs as
  your user with no sudo.
