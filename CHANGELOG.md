# Changelog

All notable changes to reeve are documented here.

## 0.3.7

### Fixed
- **TUI columns no longer drift on long hostnames.** The Vhosts and Parked panels padded the address to a fixed 30 columns, so anything longer pushed that row's `server`, `php` and `path` right and left the lists looking ragged. The address column is now sized to the widest address across both panels — capped so one outlier can't squeeze the path, and shortened with an ellipsis when it has to be (the link still opens the full address).

## 0.3.6

### Added
- **`reeve vhost show <host>`.** Prints how a host is actually served — which server and whether it came from a declared vhost or a parked folder, the project root, the resolved docroot, preset, PHP version, and the `.reeve.toml` feeding it (with the env names it contributes, or where reeve expected to find the file).

### Fixed
- **A misplaced `.reeve.toml` is no longer silent.** The file is read from the *project root*, so one dropped in the served docroot (`public/`, `web/`, `dist/`) or in a parked directory did nothing at all — the site kept serving, just without its env. `apply` now warns and names the path it should move to. (Re-opens/closes #4, thanks @frumbert.)

## 0.3.5

### Added
- **Per-project `.reeve.toml`.** Drop one in a project root to set its `docroot` (any subdir, e.g. `dist`), force a `preset` when auto-detection guesses wrong, and declare `[env]` variables that reach PHP via `getenv()` / `$_SERVER` on every request. It works for parked and declared sites alike, is rendered natively on each backend (Apache `SetEnv`, nginx `fastcgi_param`, Caddy `php_fastcgi { env … }`), and is re-read on every `apply`. Malformed files, parent-escaping docroots, and invalid variable names fail `apply` with an error naming the file; nginx additionally refuses values containing `$`, which it would interpolate and cannot escape. (Closes #4, thanks @frumbert.)
- **`public` preset.** A generic app served from its `public/` subdir with no framework-specific rewrites — selectable with `--preset public` and in the TUI.

### Fixed
- **Parked projects with a static `public/` were served from the wrong directory.** A folder whose only index was `public/index.html` (a Vite/Astro/Eleventy build, say) was picked up as a site but served from the project root, so you got a directory listing instead of the page. It now gets the `public` preset; a bare `public/index.php` does too, instead of being labelled `laravel`. (Closes #5, thanks @frumbert.)
- **`SetEnv` in `.htaccess` no longer 500s on Apache.** reeve's httpd didn't load `mod_env`, so `SetEnv`/`PassEnv`/`UnsetEnv` were "Invalid command" errors. It's now part of the base module set.
- **Apache now passes the `Authorization` header to PHP-FPM** (`CGIPassAuth On`), matching nginx and Caddy, so bearer-token and Basic-auth APIs work without the `SetEnvIf Authorization` `.htaccess` workaround.

## 0.3.4

### Fixed
- **Xdebug `debug` mode no longer fights itself across vhosts.** The FPM master
  started every request with `xdebug.start_with_request=yes`, so any request
  from any vhost — a background AJAX call, another site you had open — raced for
  the IDE's debug connection. IDEs accept one simultaneous connection by
  default, so the page you actually wanted to debug would silently never attach.
  `debug` now uses `trigger`: sessions start on demand, from an IDE debug run
  configuration, a browser extension, or `?XDEBUG_SESSION=1` on the URL.
  `profile` mode is unchanged and still profiles every request.
  (Thanks @ruslanbelziuk.)

## 0.3.3

### Added
- **Live traffic monitor.** Press `t` in the dashboard (or run `reeve traffic`)
  for a full-screen live view of requests hitting your sites: a requests/sec
  chart over the last couple of minutes, status-class breakdown (2xx/3xx/4xx/5xx)
  with latency and bytes, top hosts and top paths, and a color-coded live
  request tail. `←`/`→` cycles the scope through all servers, each server, and
  each vhost (declared + parked); `PgUp`/`PgDn` jumps through long filter lists;
  `/` starts a live text search that narrows everything by host, path, or
  server name (enter keeps it, esc clears it).
  The collector tails the access logs on background threads only while the
  dashboard runs, keeps the last 10 minutes of events, and survives
  closing/reopening the view.
- **Consistent access logs on every backend.** Each server now writes an access
  log to the same standard path, `logs/server-<name>-access.log`. Apache and
  nginx share one flat, greppable format (timestamp, vhost, method, URI, status,
  bytes, duration, client); Caddy — which can't be shaped into a custom line
  format — writes its native JSON to the same path, and reeve parses both into
  the same event stream. No more hunting for where each web server keeps its
  logs.
- **Access and error logs are now first-class log targets.** `reeve logs` lists
  `server-<name>-access` for every server (and `server-<name>-error` for
  Apache/nginx, which split errors into their own file), so
  `reeve logs server-caddy-access --follow` works like any other target.

## 0.3.2

### Fixed
- **DNS setup no longer fails silently without a GUI session.** `reeve dns setup`
  (and the TUI's `D`) escalates to write the root-owned system resolver. On
  macOS it used only the native admin dialog (osascript `with administrator
  privileges`), which can't be drawn over SSH or from a headless context — so it
  failed with "admin authorization was cancelled or failed" without ever
  prompting. reeve now detects whether a GUI session can host that dialog (a real
  user at `/dev/console`, not a remote shell) and, when it can't, falls back to
  an interactive terminal `sudo`. In the TUI the dashboard suspends around the
  prompt so the password entry is visible, then resumes — the same treatment the
  other privileged operations already use. Linux always takes the interactive
  path, which also fixes the prompt being swallowed by the TUI's alternate
  screen. Neither path lets a password flow through reeve's own process.

## 0.3.1

### Fixed
- **`--ssl` vhosts now redirect plain HTTP to HTTPS** on all three backends
  (Apache, Caddy, nginx). Previously an SSL vhost was rendered only on the HTTPS
  port, so visiting the bare `http://` URL either returned a confusing 403
  (Apache with no default site) or silently served the catch-all default site's
  content instead of the real one. Each SSL vhost now also gets a companion
  HTTP block that issues a 301 to the HTTPS URL, preserving the request path and
  query, and including the HTTPS port when it is non-standard (e.g. caddy 1443,
  nginx 2443). On Apache the redirect vhosts render after the default-site
  catch-all so that catch-all remains the default for genuinely unmatched hosts.

## 0.3.0

### Added
- **Linux support.** reeve now runs on Linux (Ubuntu-first) against Homebrew on
  Linux (Linuxbrew), with the same workflow as macOS: `init`, `php install`,
  `server add`, `vhost add`, `apply`, `dns setup`, `ssl trust`, the TUI, and
  `doctor` all work end-to-end. Managed services run as systemd **user** units
  (`~/.config/systemd/user/reeve-*.service`) instead of launchd agents, selected
  at compile time so macOS behavior is unchanged. `init` enables login lingering
  (so services persist across logout and start at boot) and offers to allow
  unprivileged binding to :80/:443 via a `sysctl` drop-in. DNS integrates with
  systemd-resolved (a `resolved.conf.d` drop-in in place of `/etc/resolver`);
  SSL trust verifies against the system CA store with `openssl verify`; and the
  FPM pool runs under the user's real primary group rather than macOS's `staff`.
  `doctor` gains Linux checks for the systemd user bus, login lingering, and the
  privileged-port sysctl.

### Fixed
- `doctor` now judges a web server on the ports it actually binds (HTTP, HTTPS,
  or both) instead of assuming HTTP. An HTTPS-only server (all-SSL vhosts, no
  default site) is now reported as running on :443 rather than failing with
  "not listening on :80". On Linux the listener probe uses `ss` (iproute2),
  which is present on minimal installs where `lsof` is not.

## 0.2.8

### Added
- Set the CLI `php` version straight from the dashboard: press `C` on the PHP
  panel to point the `~/.reeve/bin` shim at the selected version (the same thing
  `reeve php cli <ver>` does), without dropping to a terminal. The PHP panel
  title now shows the current CLI version, e.g. `PHP  (cli: 8.5)`.

### Changed
- Changing the default PHP version now takes effect immediately. Setting the
  default (via `reeve php use <ver>`, or `d` on the dashboard's PHP panel) also
  re-renders and restarts every *running* server that serves a catch-all default
  site (e.g. `localhost` serving the sites root). Previously the default site's
  FPM socket is baked into each server's config, so the change didn't reach a
  running server until it was manually restarted — the default site kept serving
  the old version. Only running default-site servers are touched; stopped ones
  are left stopped.

## 0.2.7

### Added
- Vhost and Parked URLs in the dashboard are now real OSC 8 hyperlinks. Capable
  terminals (iTerm2, Ghostty, WezTerm, Kitty, …) make them click-to-open
  (usually with the terminal's modifier, e.g. Cmd-click in iTerm2), instead of
  relying on loose URL auto-detection. Terminals without OSC 8 support just
  render the plain underlined text, unchanged. Because ratatui's cell buffer
  can't carry OSC 8, the links are stamped onto the terminal after each draw and
  re-applied on scroll/selection changes.

### Changed
- The URL underline now spans only the address itself, not the padding out to
  the column width, so it lines up with the terminal's own hyperlink underline.

## 0.2.6

### Fixed
- FPM masters failing to start on macOS 26 (Tahoe) with "FPM master started
  but its socket never appeared". reeve now loads launchd jobs with the modern
  `launchctl bootstrap`/`bootout` API (in the `gui/<uid>` domain) instead of the
  legacy `load -w`/`unload`. On recent macOS, `load -w` silently refuses to
  start a label that launchd has marked *disabled* (e.g. after an earlier
  crash-loop auto-disabled it) while still reporting success; loading now runs
  `launchctl enable` first to clear that sticky state, and retries `bootstrap`
  while a just-removed job is still tearing down.
- `php install` no longer reports an FPM master as "ready" when it isn't. The
  post-start health check now requires launchd to actually own a running master
  (a real PID) *and* the socket to be live — previously a leftover or hand-run
  socket at the same path could make a failed start look successful. The old
  socket is also removed before restart so a stale one can't mask a failure.

## 0.2.5

### Added
- Per-server default-site docroot. Each server can have its own `default_root`
  (CLI `server add --root <dir>`, or the "Default root" field in the server
  edit modal); unset falls back to the global `sites_root`, so existing setups
  are unchanged. The Servers panel now shows each server's effective default
  root, aligned with the vhost/parked path column.

### Changed
- TUI path columns abbreviate your home directory to `~` (paths outside home
  stay absolute) — shorter and easier to scan.

## 0.2.4

### Added
- `php cli` and `reeve init` now offer to add `~/.reeve/bin` to your shell
  profile (`~/.zshrc` / `~/.bash_profile`) so a CLI PHP switch actually takes
  effect, instead of only printing the line. It skips the prompt when the line
  is already there (just tells you to reload the shell), prints `fish_add_path`
  guidance for fish, and never blocks when stdin isn't a TTY.

## 0.2.3

### Added
- `php cli [version]` — switch the terminal `php` (and `pecl`/`phpize`/
  `php-config`/`phar`) to an installed version, like the old `sphp`. It works
  by repointing symlinks in `~/.reeve/bin`, so Homebrew's link state is never
  touched and switching is instant. Add `~/.reeve/bin` to the front of your
  PATH once; `doctor` warns if it isn't, and `php cli` with no version reports
  the current one.

### Fixed
- When an FPM master's socket never appears, `php install` / `apply` now run
  `php-fpm -t` in-process and tail the FPM error log to report the real cause.
  Previously the error pointed at the launchd log, which is usually empty —
  php-fpm writes startup errors to its own `error_log`, and a dyld/link failure
  dies before any log opens — so the failure was undiagnosable.

## 0.2.2

### Added
- `update [--check]` — self-update reeve to the latest GitHub release. Detects
  how reeve was installed: Homebrew and cargo installs print the right upgrade
  command, while a plain binary (e.g. under `~/.local/bin`) is downloaded and
  replaced in place. `--check` only reports whether a newer version exists.

## 0.2.1

### Fixed
- Vhost save no longer appends the hostname to a docroot that ends in a slash.
  The path dropdown leaves a trailing slash when you enter a folder, so editing
  a vhost to point at a shared directory (e.g. `~/workspace/`) silently became
  `~/workspace/<host>`, a missing folder Apache served as 403 Forbidden. The
  host is now only appended for a brand-new vhost left at the untouched default
  sites root; otherwise the chosen directory is used verbatim, letting several
  hosts share one docroot.

## 0.2.0

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
  and per-backend rewrites. Selectable in the TUI new-vhost wizard. A server's
  catch-all **default site** can take a preset too (`server add --preset …`, or
  the TUI new-server wizard), so the non-vhost webroot gets front-controller
  routing instead of plain file serving.
- **Grav security rules for nginx & Caddy**: the `grav` preset now emits the
  access-control rules Grav's `.htaccess` provides on Apache — blocking direct
  access to `user/{accounts,config,env}`, non-media files under `user/data`,
  scripts in `system`/`vendor`/`user`, dotenv files, etc. (mirrors Grav 2.0's
  shipped `webserver-configs`). Caddy uses `path_regexp` matchers since its
  inline path matchers don't support regex.
- **Reverse-proxy vhosts**: `vhost add --proxy http://localhost:5173` renders a
  proxy vhost (no PHP) on Caddy/nginx/Apache — for Vite, Node, and other
  upstream dev servers.
- **Directory parking** (Valet-style): `park add <dir> --server <s> --php <v>`
  auto-serves every web subfolder as `<folder>.<tld>` with framework
  auto-detection (Laravel/Symfony/Drupal/WordPress/Grav); new folders appear on
  the next `apply`. `park list|remove`, and parked sites show (read-only) in a
  dedicated, scrollable **Parked** TUI panel separate from declared vhosts.
- **Mouse + scrolling in the TUI**: click a panel/row to focus and select it,
  the scroll wheel scrolls the panel under the cursor, and `PageUp`/`PageDown`/
  `Home`/`End` page through long lists. Panels taller than their box (e.g. a
  parked `~/Sites` with dozens of projects) now scroll to keep the selection in
  view instead of clipping.
- **Full TUI/CLI parity**: the new-vhost wizard gained a reverse-proxy field, a
  park manager (`p`) handles add/remove, `?` opens the full `doctor` report, and
  `T` runs `ssl trust` — so everything the CLI can do is reachable in the TUI.
- **Secret path anonymizer** (`~` in the TUI, intentionally absent from the key
  bar): rewrites your real home directory to `/Users/andy` in every displayed
  path — Vhosts/Parked docroots, the status line, the log viewer, and the
  `doctor` report — so screenshots don't leak your username. Editable fields are
  left untouched.
- **Honest, port-aware server status**: `server list`, `doctor`, and the TUI
  server panel now report what launchd *and* the network agree on — `running`
  only when the port is actually bound, otherwise `loaded, not bound`,
  `:<port> held by <process>`, or `crashed`. `server start`/`restart` run a
  preflight that refuses to launch when a process reeve doesn't manage already
  holds a target port, naming the holder instead of silently failing to bind.

### Changed
- **Consistent TUI shortcuts**: `r` = restart, `x` = stop, and `R`/`Del` =
  remove in *every* panel (previously `r` removed in the PHP/Vhosts panels and
  `x` toggled Xdebug). Xdebug moved to `X`. Editing/adding/removing a vhost now
  auto-applies (re-renders + restarts the running servers) instead of asking you
  to restart the server by hand.
- **Sortable Vhosts panel**: keys `1`-`4` sort by host/server/php/path, pressing
  the same column again flips direction. Installed PHP versions list ascending
  (7.3, 8.3, 8.4, …) rather than in install order.

### Fixed
- **Auto-hand off from `brew services`**: when a server or service starts and a
  conflicting `brew services` instance of the same software (e.g. a leftover
  `brew services start redis`/`httpd`) is holding the port, reeve now stops that
  job and reports the handoff, instead of crash-looping on the held port. A
  hand-launched (non-brew) process still yields the clear "port held by …" error.
- **Parked folders with awkward names** are slugified into valid hostnames — a
  folder literally named `grav-helios 2` becomes `grav-helios-2.test` instead of
  producing an invalid host that mkcert rejects and aborts the whole `apply`.
- `apply` now reconciles **every** PHP-FPM master (not just whichever was last
  touched), so launchd/plist changes propagate to all installed versions.
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
