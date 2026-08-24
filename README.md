# reeve

```
 _ __ ___  _____   _____
| '__/ _ \/ _ \ \ / / _ \
| | |  __/  __/\ V /  __/
|_|  \___|\___| \_/ \___|
       RunCloud, scaled down — for localhost.
```

A local web development stack manager for **macOS and Linux**. Think RunCloud,
scaled down, localhost-only, open source: provision and manage web servers,
**per-vhost PHP versions**, SSL, and DNS — from one CLI and TUI.

Written in Rust. It does not bundle servers or PHP; it orchestrates
Homebrew-installed ones and manages them as per-user services — launchd on
macOS, systemd user units on Linux — with no sudo for day-to-day use.

![reeve](screenshot.png)

## Why

The classic macOS + Homebrew + Apache + `mod_php` setup links exactly **one**
PHP version at a time. reeve drops `mod_php` for **PHP-FPM, one master per
version**, so two vhosts can run two different PHP versions *simultaneously* —
the thing the old switcher-script approach can't do.

## Highlights

- **Per-vhost PHP version** — each vhost picks its PHP; versions run side by side.
- **Per-version PHP tuning** — `memory_limit`, upload sizes, OPcache, FPM pool,
  timezone, and a one-click **Xdebug** toggle (off/debug/profile), all without
  hand-editing `php.ini`. In `debug` mode Xdebug attaches only to requests
  carrying `XDEBUG_SESSION`/`XDEBUG_TRIGGER` — set by any IDE debug run
  configuration or browser extension — so background traffic from your other
  sites can't steal the IDE's connection slot.
- **Multiple backends** behind one abstraction: **Caddy**, **Apache**, **nginx**
  (OpenLiteSpeed is wired but unsupported on macOS — see below).
- **Run several servers at once**, on different ports, managed independently.
- **Background services** — install/start/stop **MySQL, MariaDB, PostgreSQL,
  Redis, memcached, Mailpit** via Homebrew + launchd/systemd, alongside the web stack.
- **Framework presets** — `laravel`, `wordpress`, `symfony`, `grav`, `drupal`
  set the right docroot + rewrites automatically, `public` serves any app from
  its `public/` subdir; or point a vhost at an upstream dev server as a
  **reverse proxy** (Vite, Node, …).
- **Directory parking** (Valet-style) — park `~/Sites` and every subfolder
  auto-serves as `<folder>.test`, framework auto-detected, no per-project setup.
  Odd folder names are slugified to valid hostnames automatically.
- **Per-project overrides** — an optional `.reeve.toml` in a project sets its
  docroot, preset, and environment variables for PHP, on every backend.
- **Trusted local SSL** via a shared mkcert CA — `https://app.test` with no
  warnings. `--ssl` vhosts also redirect plain `http://` to HTTPS automatically,
  so an old bookmark or a bare-domain visit lands on the secure URL.
- **Wildcard DNS** for one or more TLDs (`.test`, `.localhost`, `.lan`, …) via a
  user-run dnsmasq (no root daemon).
- **Default site** — optionally serve a catch-all from your sites root (with an
  optional framework preset), so `http(s)://localhost` works without a vhost.
  Each server can override its default root, or share the global sites root.
- **`logs` + `doctor`** — tail any service's log, and a one-shot health check of
  the whole stack (Homebrew, servers, FPM, services, DNS, certs, ports).
- **Live traffic monitor** (`t` in the TUI, or `reeve traffic`) — a full-screen
  requests/sec chart with status-class breakdown, latency, top hosts/paths, and
  a live request tail, filterable to one server or one vhost. Backed by a
  consistent access log per server (`logs/server-<name>-access.log`) on every
  backend, so `reeve logs <server>-access` works the same everywhere too.
- **Honest, port-aware status** — `running` only when the port is actually bound;
  otherwise `loaded, not bound`, `:<port> held by <process>`, or `crashed`.
  `start`/`restart` preflight the ports and name any foreign process holding them.
- **TUI dashboard** with live status — mouse-clickable, scrollable panels (a
  dedicated **Parked** panel for parked sites) — plus a scriptable CLI on the
  same engine.
- **No sudo** for day-to-day use; servers bind 80/443 as your user via launchd
  on macOS, or systemd user units on Linux (after a one-time `sysctl` that
  `reeve init` offers to set up).

## Requirements

- **macOS** (built and tested on macOS 26 "Tahoe", Apple Silicon), or **Linux**
  (Ubuntu-first; needs systemd with a user session bus, and systemd-resolved for
  DNS). See [Linux notes](#linux) below.
- [Homebrew](https://brew.sh) — reeve will offer to install it if missing
  (Linuxbrew on Linux)

## Installation

### Homebrew (recommended)

```bash
brew install yetidevworks/reeve/reeve
```

### From crates.io

```bash
cargo install reeve-cli
```

(The crate is published as `reeve-cli` because `reeve` is already taken on
crates.io; the installed binary is still `reeve`.)

### From source

```bash
git clone https://github.com/yetidevworks/reeve
cd reeve
cargo install --path crates/reeve
```

### Pre-built binaries

Download from [GitHub Releases](https://github.com/yetidevworks/reeve/releases).

### Updating

```bash
reeve update           # update to the latest release
reeve update --check   # just check whether a newer version exists
```

`update` detects how reeve was installed. Homebrew and cargo installs print the
matching upgrade command (`brew upgrade reeve` / `cargo install --force
reeve-cli`); a plain binary (e.g. under `~/.local/bin`) is downloaded and
replaced in place. Quit and relaunch reeve afterwards to pick up the new binary.

## Quick start

```bash
reeve init                 # detect Homebrew, scaffold config/state
reeve php install 8.3      # install a PHP version + its FPM master
reeve php install 8.4      # ...and another, running at the same time
reeve server add caddy     # add a web server (default ports 80/443)
reeve vhost add app.test --root ~/Sites/app --php 8.3 --server caddy --ssl
reeve server start caddy   # render config, mint cert, start
```

Add a database and a mail catcher, tune PHP, or use a framework preset:

```bash
reeve service add mysql && reeve service start mysql   # MySQL on :3306
reeve service add mailpit && reeve service start mailpit  # SMTP :1025, UI :8025
reeve php set 8.3 memory_limit 512M                    # tune php.ini
reeve php xdebug 8.3 debug                              # Xdebug, attaches on XDEBUG_SESSION
reeve vhost add shop.test --root ~/Sites/shop --php 8.3 --server caddy --ssl --preset laravel
reeve vhost add ui.test --proxy http://localhost:5173 --server caddy --ssl   # reverse proxy
```

Or park a directory so every project in it just works, no per-site setup:

```bash
reeve park add ~/Sites --server caddy --php 8.3 --ssl   # ~/Sites/blog → blog.test, etc.
reeve apply                                             # picks up new folders anytime
```

Parked folders are served from `public/` or `web/` when a framework (or just a `public/index.*`) is detected, otherwise from the folder itself. When that guess is wrong, or a project needs environment variables, drop a `.reeve.toml` in the project root — it works for parked and declared sites alike, on Apache, nginx, and Caddy, and is re-read on every `apply`:

```toml
# .reeve.toml — every key is optional
docroot = "dist"        # subdir to serve, relative to the project
preset = "grav"         # override auto-detection / the vhost's preset

[env]                   # reaches PHP as getenv("DB_USER") / $_SERVER["DB_USER"]
DB_USER = "root"
DB_PASS = "secret"
```

The file belongs beside the project, **not** inside the directory being served — a `.reeve.toml` in `public/`, `web/`, or `dist/` is never read, and `apply` warns when it finds one there. To see exactly what a host resolves to, including which project file (if any) is feeding it:

```bash
reeve vhost show app.test
```

Add it to `.gitignore` if it holds secrets. On Apache, a plain `.htaccess` works too (`AllowOverride All`, with `SetEnv` and `CGIPassAuth` available); nginx can't express a literal `$` in a value, so `apply` refuses such entries with a clear error rather than passing a mangled value to PHP.

One-time DNS + trust setup so `*.test` resolves system-wide and certs are trusted:

```bash
reeve dns setup            # installs dnsmasq + wires the system resolver
reeve ssl trust            # install the mkcert CA into your trust store (once)
```

`dns setup` points the system resolver at reeve's dnsmasq for each TLD. On macOS
it writes the root-owned `/etc/resolver/<tld>` files via the native admin dialog
— no manual `sudo`. On Linux it writes a systemd-resolved drop-in via `sudo`. In
the TUI, press `D`.

Then `https://app.test` just works in the browser.

## TUI

Run with no arguments to open the dashboard:

```
reeve
```

```
 reeve   [.test ✓ resolving]   ● health (L logs)
┌ Servers ──────────────────────────────────────────────┐
│ › caddy    caddy   :80/:443     ● running             │
│   apache   apache  :8080/:8543  ● running             │
│   nginx    nginx   :9090/:9443  ○ stopped             │
├ Vhosts ───────────────────────────────────────────────┤
│   https://app.test   caddy   8.3   ~/Sites/app        │
├ Parked (37)  [1–4 of 37] ─────────────────────────────┤
│ › https://blog.test  caddy   8.3   ~/Sites/blog       │
│   https://shop.test  caddy   8.3   ~/Sites/shop       │
├ PHP ──────────────────────────────────────────────────┤
│ 8.3★ ● running  debug    8.4 ● running                │
├ Services ─────────────────────────────────────────────┤
│ › mysql      :3306   ● running                        │
│   mailpit    :8025   ● running                        │
└───────────────────────────────────────────────────────┘
 enter start · x stop · r restart · n new · s settings · L log · q quit
```

Navigation is shared across panels; the key bar is context-aware (it shows the
keys for the focused panel).

| Key | Action |
|-----|--------|
| `↑` `↓` `←` `→` / `hjkl` | Move selection within the focused panel |
| `PgUp` `PgDn` / `Home` `End` | Page / jump through a long list (e.g. Parked) |
| mouse | Click a row to focus + select it; wheel scrolls the panel under the cursor |
| `Tab` / `Shift-Tab` | Switch panel (Servers → Vhosts → Parked → PHP → Services) |
| `n` | New — server / vhost / PHP install / service (per focused panel) |
| `e` | Edit — server (ports, backend, default site) / vhost / PHP extensions |
| `Enter` | Start server or service / restart FPM master |
| `x` · `r` | Stop · restart (Servers, Services) |
| `s` | Per-backend settings (Servers) / per-version PHP settings (PHP) |
| `X` | Cycle Xdebug off→debug→profile (PHP panel) |
| `d` | Set default PHP version (PHP panel) |
| `p` | Park a directory / manage parks (Vhosts or Parked panel) |
| `L` | View the focused item's log |
| `t` | Live traffic monitor (`←` `→` cycle the server/vhost filter, `/` text search, `Esc` back) |
| `?` | Full `doctor` health report |
| `T` | Install the mkcert CA into your trust store (`ssl trust`) |
| `Del` / `Backspace` | Remove the focused item (with confirm) |
| `a` · `v` | Apply · validate all configs |
| `c` | Preferences (TLDs, sites root, default backend) |
| `D` | Set up wildcard DNS (admin prompt) |
| `q` / `Esc` | Quit |

The new-vhost wizard (`n` on Vhosts) covers framework **presets** and a
**reverse-proxy** target, so everything the CLI does is reachable from the TUI.

### Traffic monitor

Press `t` (or run `reeve traffic`) for a full-screen live view of requests
hitting your sites — across all servers, one server, or one vhost:

```
 reeve traffic   filter: ‹ all servers ›   ● live
┌ Requests/sec — last 120s · 550 req · peak 15/s ──────────────────────────────┐
│                                        ▂▄▆█▆▄▂                          ▄█▇  │
│  ▁▂▃▄▃▂▁       ▂▄▆▄▂      ▁▃▅▃▁      ▂▄██████▄▂        ▂▃▄▃▂          ▃▆███▅ │
│▄█████████▄▄▄▄▆██████▆▄▄▅███████▅▄▄▄▆███████████▆▄▄▄▄▄▆███████▆▄▄▄▄▄▄▅███████│
└──────────────────────────────────────────────────────────────────────────────┘
┌ Status ────────────────────┐┌ Top hosts ──────────────┐┌ Top paths ──────────┐
│ rate  6/s  peak 15/s       ││ grav.test  ▐▐▐▐▐▐▐▐ 93  ││ /blog  ▐▐▐▐▐▐▐▐ 44  │
│ 2xx  ▐▐▐▐▐▐▐▐▐▐▐▐▐▐ 124    ││ app.caddy  ▐▐▐▐▐ 63     ││ /api   ▐▐▐▐▐ 41     │
│ 4xx  ▐▐▐▐           32     ││ api.test   ▐▐▐▐ 60      ││ /      ▐▐▐ 22       │
│ time  avg 27ms · max 120ms ││                         ││                     │
└────────────────────────────┘└─────────────────────────┘└─────────────────────┘
┌ Live requests ───────────────────────────────────────────────────────────────┐
│ 10:11:12 200 GET  grav.test   /blog                12ms   29.8 KB [apache]   │
│ 10:11:12 404 GET  app.caddy   /missing              2ms     236 B [caddy]    │
└──────────────────────────────────────────────────────────────────────────────┘
 ←→ filter   PgUp/Dn jump   / search   0 all   esc/t back
```

Every backend writes an access log to the same place
(`logs/server-<name>-access.log`) — Apache and nginx in a shared flat format,
Caddy in its native JSON — and reeve parses them all into the same stream. The
collector tails the logs only while the dashboard runs, keeps the last 10
minutes, and survives closing/reopening the view.

## Commands

| Command | Description |
| --- | --- |
| `init` | Detect Homebrew, scaffold config/state |
| `php install <ver>` | Install (or adopt) a PHP version + FPM master |
| `php list` / `php use <ver>` | List versions / set the default for new vhosts |
| `php cli [ver]` | Switch the terminal `php` (via the `~/.reeve/bin` shim); omit to show current |
| `php ext add\|remove\|list <ver> [name]` | Manage extensions per version (pecl) |
| `php settings <ver>` / `php set <ver> <key> <value>` | Show / tune php.ini, OPcache, FPM pool |
| `php xdebug <ver> off\|debug\|profile` | Toggle Xdebug for a version (`debug` attaches only to requests carrying `XDEBUG_SESSION`/`XDEBUG_TRIGGER`; `profile` profiles every request) |
| `server add <backend> [--http N --https N] [--default-site] [--root <dir>] [--preset <fw>]` | Add caddy\|apache\|nginx (optionally a catch-all default site; `--root` overrides its docroot, else the global sites root) |
| `server start\|stop\|restart\|list\|remove <name>` | Manage a server (independent) |
| `vhost add <host> --root <dir> --php <ver> --server <name> [--ssl] [--preset <fw>] [--proxy <url>]` | Add a vhost |
| `vhost list\|remove <host>` | Manage vhosts |
| `vhost show <host>` | Show how a host is served: docroot, preset, PHP, and its `.reeve.toml` |
| `service add\|start\|stop\|restart\|remove\|list <kind>` | Manage databases / redis / memcached / mailpit |
| `park add <dir> --server <name> --php <ver> [--tld T] [--ssl]` | Auto-serve every subfolder as `<folder>.<tld>` |
| `park list\|remove <dir>` | Manage parked directories |
| `apply` | Render generated configs + reconcile running services |
| `validate` | Run every backend's native config test |
| `traffic` | Open the dashboard straight into the live traffic monitor |
| `logs [<target>] [-n N] [--follow]` | View or tail a service's log (incl. `server-<name>-access`) |
| `doctor` | Diagnose the whole stack (brew, servers, FPM, services, DNS, certs, ports) |
| `update [--check]` | Self-update to the latest GitHub release (`--check` only reports) |
| `ssl mint <host>` / `ssl trust` / `ssl untrust` / `ssl status` / `ssl ca` | Local certificates: mint one, install/remove the mkcert CA in the trust store, or show its state |
| `dns setup` / `dns status` | Wildcard `*.test` DNS |

## CLI PHP version

`php cli <ver>` switches the `php` on your terminal — the modern replacement for
the old `sphp` script. Instead of running `brew link` (which mutates Homebrew's
global state and fights keg-only versions), reeve keeps a shim directory of
symlinks at `~/.reeve/bin` (`php`, `pecl`, `phpize`, `php-config`, `phar`) and
just repoints them. Switching is instant and Homebrew is never touched.

The shim needs to be on the front of your PATH once. `reeve init` and
`reeve php cli` offer to add it to your shell profile automatically — or add it
by hand:

```bash
# ~/.zshrc (or ~/.bash_profile)
export PATH="$HOME/.reeve/bin:$PATH"
```

```bash
reeve php cli 8.4    # point the CLI at PHP 8.4
reeve php cli        # show the current CLI version
php -v               # PHP 8.4.x …
```

`doctor` warns if `~/.reeve/bin` isn't ahead of Homebrew on your PATH. To revert
entirely, remove that line from your profile — nothing else changes.

## How it works

`state.toml` is the single source of truth (servers, PHP versions, vhosts).
reeve **renders** native configs (Caddyfile, httpd vhosts, nginx server
blocks, FPM pools) into `generated/`, then reconciles per-user services
(launchd agents on macOS, systemd user units on Linux) to match. You never
hand-edit generated files — you edit state via the CLI/TUI and `apply`.

```
# macOS
~/Library/Application Support/reeve/
  config.toml · state.toml · generated/ · certs/ · logs/
# Linux
~/.config/reeve/
  config.toml · state.toml · generated/ · certs/ · logs/

~/.reeve/run/            # sockets (kept space-free, short) — both platforms
```

## OpenLiteSpeed

OLS has no official macOS build, and the only community Homebrew tap fails to
compile its (2022-deprecated) `admin_php` dependency against the current macOS
SDK. The backend is wired in and will activate if a working OLS install appears,
but it is **not usable on macOS today**. Use Caddy, Apache, or nginx.

## Linux

reeve runs on Linux (Ubuntu-first) against Homebrew on Linux (Linuxbrew), with
the same CLI, TUI, and workflow as macOS. A few things work differently under
the hood, and `reeve init` sets most of them up for you:

- **Services are systemd user units** (`~/.config/systemd/user/reeve-*.service`)
  instead of launchd agents. This needs a user session bus, so reeve does **not**
  run inside bare containers or over a session-less login. `reeve doctor` checks
  that `systemctl --user` responds.
- **Login lingering** is enabled during `init` (`loginctl enable-linger`) so your
  servers keep running after you log out and come back automatically on boot.
- **Privileged ports.** Linux blocks non-root processes from binding ports below
  1024. `init` offers to write `/etc/sysctl.d/99-reeve.conf`
  (`net.ipv4.ip_unprivileged_port_start = 80`) with one `sudo` so servers can
  bind :80/:443. Decline it and use ports ≥ 1024 instead.
- **DNS** uses a systemd-resolved drop-in (`/etc/systemd/resolved.conf.d/reeve.conf`)
  rather than `/etc/resolver`, applied via `sudo`. Hosts that don't run
  systemd-resolved aren't supported for automatic DNS.
- **SSL trust** installs the mkcert CA into the system store. For browser trust,
  install `libnss3-tools` (apt) or `nss` (brew) so mkcert can add the CA to NSS
  too.

Not supported: distros without systemd (Alpine, WSL1) and container-only setups.

## Development

```bash
cargo build              # debug build
cargo test               # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all
cargo install --path crates/reeve   # install the local binary
```

The repo is a Cargo workspace; the crate lives in `crates/reeve`. CI runs
fmt + clippy + tests on macOS and Linux; tagged `v*` pushes build release
binaries, publish `reeve-cli` to crates.io, and update the Homebrew tap.

## License

[MIT](LICENSE) © 2026 Andy Miller
