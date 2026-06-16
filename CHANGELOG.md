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
- `logs <target> [-n N] [--follow]` — view or tail any managed service's log
  (server, PHP-FPM, dnsmasq); omit the target to list what's available. The TUI
  gains an `L` log viewer for the focused row.
- `doctor` — one-shot stack diagnosis (Homebrew, web servers, FPM sockets, DNS
  resolvers, mkcert CA, port conflicts) as pass/warn/fail lines; the TUI title
  bar shows a live health dot.
- **Per-version PHP runtime settings**: `php settings <ver>` shows the effective
  php.ini / OPcache / FPM-pool values, `php set <ver> <key> <value>` overrides
  one (the FPM pool, previously hardcoded, is now fully data-driven). The TUI
  PHP panel gains an `s` settings modal.
- **Xdebug toggle**: `php xdebug <ver> off|debug|profile` (installs Xdebug via
  pecl on first enable, manages `xdebug.mode`/client port in the FPM pool). The
  TUI cycles it with `x` and marks active versions in the PHP panel.
- **Managed services tier**: `service add|start|stop|restart|remove|list` for
  MySQL, MariaDB, PostgreSQL, Redis, memcached, and Mailpit — installed via
  Homebrew and supervised by launchd, adopting each formula's default datadir
  (no initdb). New **Services** panel in the TUI with an add-picker. `doctor`
  and `logs` cover services too.
- Web-server backends now install their Homebrew formula up front on
  `server add` / first start (visible brew output) instead of stalling silently.
- **Framework presets**: `vhost add --preset laravel|wordpress|symfony|grav|drupal`
  sets the conventional public docroot (Laravel/Symfony `public/`, Drupal `web/`)
  and per-backend rewrites. Selectable in the TUI new-vhost wizard.
- **Reverse-proxy vhosts**: `vhost add --proxy http://localhost:5173` renders a
  proxy vhost (no PHP) on Caddy/nginx/Apache — for Vite, Node, and other
  upstream dev servers.
- **Directory parking** (Valet-style): `park add <dir> --server <s> --php <v>`
  auto-serves every web subfolder as `<folder>.<tld>` with framework
  auto-detection (Laravel/Symfony/Drupal/WordPress/Grav); new folders appear on
  the next `apply`. `park list|remove`, and parked sites show (read-only) in the
  TUI Vhosts panel.
- **Full TUI/CLI parity**: the new-vhost wizard gained a reverse-proxy field, a
  park manager (`p`) handles add/remove, `?` opens the full `doctor` report, and
  `T` runs `ssl trust` — so everything the CLI can do is reachable in the TUI.

### Known limitations
- **OpenLiteSpeed** is wired in but unusable on macOS: the only community
  Homebrew tap fails to build its deprecated `admin_php` dependency against the
  macOS 26 SDK. Use Caddy, Apache, or nginx.
- Setting up system-wide `*.test` resolution writes `/etc/resolver/<tld>`, which
  requires a one-time `sudo` (that path is root-owned). Everything else runs as
  your user with no sudo.
