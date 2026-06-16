# Changelog

All notable changes to reeve are documented here.

## 0.2.0 (unreleased)

Stack expansion (databases/services, PHP tuning, presets, parking),
full TUI/CLI parity, and performance/correctness fixes.

### Added
- `logs <target> [-n N] [--follow]` — view or tail any managed service's log
  (server, PHP-FPM, dnsmasq, service); omit the target to list what's available.
  The TUI gains an `L` log viewer for the focused row.
- `doctor` — one-shot stack diagnosis (Homebrew, web servers, FPM sockets, DNS
  resolvers, mkcert CA, port conflicts, services) as pass/warn/fail lines; the
  TUI title bar shows a live health dot.
- **Per-version PHP runtime settings**: `php settings <ver>` shows the effective
  php.ini / OPcache / FPM-pool values, `php set <ver> <key> <value>` overrides
  one (the FPM pool, previously hardcoded, is now fully data-driven). The TUI
  PHP panel gains an `s` settings modal.
- **Xdebug toggle**: `php xdebug <ver> off|debug|profile` (installs Xdebug via
  pecl on first enable, manages `xdebug.mode`/client port). The TUI cycles it
  with `x` and marks active versions in the PHP panel.
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
- **Honest, port-aware server status**: `server list`, `doctor`, and the TUI
  server panel now report what launchd *and* the network agree on — `running`
  only when the port is actually bound, otherwise `loaded, not bound`,
  `:<port> held by <process>`, or `crashed`. `server start`/`restart` run a
  preflight that refuses to launch when a process reeve doesn't manage already
  holds a target port, naming the holder instead of silently failing to bind.

### Fixed
- **launchd jobs run at interactive QoS, not `Background`.** Every reeve service
  plist set `ProcessType=Background`, which on Apple Silicon throttles the job
  onto efficiency cores at low priority — making each PHP-FPM request 3–5× slower
  than the same PHP under brew's (un-throttled) httpd. A Grav admin dashboard
  load went from ~5.5s to ~0.5s after switching to `ProcessType=Interactive`.
- **Apache HTTP keep-alive** is now enabled (`KeepAlive On` + tunable
  `KeepAliveTimeout` / `MaxKeepAliveRequests`). reeve builds Apache's config
  from scratch, where keep-alive was effectively off, so a browser opened a
  fresh TCP+TLS connection per asset — markedly slower for asset-heavy pages.
- **Xdebug and `opcache.enable` are forced as php-fpm startup defines** (`-d`)
  instead of pool `php_admin_value`s. A pool value does not reliably override a
  `zend_extension`'s startup mode, so Homebrew's `ext-xdebug.ini`
  (`xdebug.mode=debug`) left Xdebug instrumenting every call — a 5–50× slowdown
  for Twig/Grav and other function-heavy code, and it disabled OPcache JIT.
  Setting `opcache.enable` per-request also made PHP warn on every request.

## 0.1.0

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
