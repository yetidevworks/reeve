# reeve

```
 _ __ ___  _____   _____
| '__/ _ \/ _ \ \ / / _ \
| | |  __/  __/\ V /  __/
|_|  \___|\___| \_/ \___|
       RunCloud, scaled down — for localhost.
```

A local web development stack manager for macOS. Think RunCloud, scaled down,
localhost-only, open source: provision and manage web servers, **per-vhost PHP
versions**, SSL, and DNS — from one CLI and TUI.

Written in Rust. It does not bundle servers or PHP; it orchestrates
Homebrew-installed ones and manages them as your-user launchd services (no sudo).

![reeve](screenshot.webp)

## Why

The classic macOS + Homebrew + Apache + `mod_php` setup links exactly **one**
PHP version at a time. reeve drops `mod_php` for **PHP-FPM, one master per
version**, so two vhosts can run two different PHP versions *simultaneously* —
the thing the old switcher-script approach can't do.

## Highlights

- **Per-vhost PHP version** — each vhost picks its PHP; versions run side by side.
- **Per-version PHP tuning** — `memory_limit`, upload sizes, OPcache, FPM pool,
  timezone, and a one-click **Xdebug** toggle (off/debug/profile), all without
  hand-editing `php.ini`.
- **Multiple backends** behind one abstraction: **Caddy**, **Apache**, **nginx**
  (OpenLiteSpeed is wired but unsupported on macOS — see below).
- **Run several servers at once**, on different ports, managed independently.
- **Background services** — install/start/stop **MySQL, MariaDB, PostgreSQL,
  Redis, memcached, Mailpit** via Homebrew + launchd, alongside the web stack.
- **Framework presets** — `laravel`, `wordpress`, `symfony`, `grav`, `drupal`
  set the right docroot + rewrites automatically; or point a vhost at an
  upstream dev server as a **reverse proxy** (Vite, Node, …).
- **Directory parking** (Valet-style) — park `~/Sites` and every subfolder
  auto-serves as `<folder>.test`, framework auto-detected, no per-project setup.
- **Trusted local SSL** via a shared mkcert CA — `https://app.test` with no warnings.
- **Wildcard DNS** for one or more TLDs (`.test`, `.localhost`, `.lan`, …) via a
  user-run dnsmasq (no root daemon).
- **Default site** — optionally serve a catch-all from your sites root, so
  `http(s)://localhost` works without a per-project vhost.
- **`logs` + `doctor`** — tail any service's log, and a one-shot health check of
  the whole stack (Homebrew, servers, FPM, services, DNS, certs, ports).
- **TUI dashboard** with live status, plus a scriptable CLI — same engine underneath.
- **No sudo** for day-to-day use; servers bind 80/443 as your user via launchd.

## Requirements

- macOS (built and tested on macOS 26 "Tahoe", Apple Silicon)
- [Homebrew](https://brew.sh) — reeve will offer to install it if missing

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
reeve php xdebug 8.3 debug                              # one-click Xdebug
reeve vhost add shop.test --root ~/Sites/shop --php 8.3 --server caddy --ssl --preset laravel
reeve vhost add ui.test --proxy http://localhost:5173 --server caddy --ssl   # reverse proxy
```

Or park a directory so every project in it just works, no per-site setup:

```bash
reeve park add ~/Sites --server caddy --php 8.3 --ssl   # ~/Sites/blog → blog.test, etc.
reeve apply                                             # picks up new folders anytime
```

One-time DNS + trust setup so `*.test` resolves system-wide and certs are trusted:

```bash
reeve dns setup            # installs dnsmasq + wires /etc/resolver (one admin prompt)
reeve ssl trust            # install the mkcert CA into your trust store (once)
```

`dns setup` writes the root-owned `/etc/resolver/<tld>` files itself via the
native macOS admin dialog — no manual `sudo` needed. In the TUI, press `D`.

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
│   https://blog.test  caddy   8.3   ~/Sites/blog (parked)│
├ PHP ──────────────────────────────────────────────────┤
│ 8.3★ ● running 🐛debug    8.4 ● running               │
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
| `Tab` / `Shift-Tab` | Switch panel (Servers → Vhosts → PHP → Services) |
| `n` | New — server / vhost / PHP install / service (per focused panel) |
| `e` | Edit — server (ports, backend, default site) / vhost / PHP extensions |
| `Enter` | Start server or service / restart FPM master |
| `x` · `r` | Stop · restart (Servers, Services) |
| `s` | Per-backend settings (Servers) / per-version PHP settings (PHP) |
| `x` | Cycle Xdebug off→debug→profile (PHP panel) |
| `d` | Set default PHP version (PHP panel) |
| `p` | Park a directory / manage parks (Vhosts panel) |
| `L` | View the focused item's log |
| `?` | Full `doctor` health report |
| `T` | Install the mkcert CA into your trust store (`ssl trust`) |
| `Del` / `Backspace` | Remove the focused item (with confirm) |
| `a` · `v` | Apply · validate all configs |
| `c` | Preferences (TLDs, sites root, default backend) |
| `D` | Set up wildcard DNS (admin prompt) |
| `q` / `Esc` | Quit |

The new-vhost wizard (`n` on Vhosts) covers framework **presets** and a
**reverse-proxy** target, so everything the CLI does is reachable from the TUI.

## Commands

| Command | Description |
| --- | --- |
| `init` | Detect Homebrew, scaffold config/state |
| `php install <ver>` | Install (or adopt) a PHP version + FPM master |
| `php list` / `php use <ver>` | List versions / set the default |
| `php ext add\|remove\|list <ver> [name]` | Manage extensions per version (pecl) |
| `php settings <ver>` / `php set <ver> <key> <value>` | Show / tune php.ini, OPcache, FPM pool |
| `php xdebug <ver> off\|debug\|profile` | Toggle Xdebug for a version |
| `server add <backend> [--http N --https N]` | Add caddy\|apache\|nginx |
| `server start\|stop\|restart\|list <name>` | Manage a server (independent) |
| `vhost add <host> --root <dir> --php <ver> --server <name> [--ssl] [--preset <fw>] [--proxy <url>]` | Add a vhost |
| `vhost list\|remove <host>` | Manage vhosts |
| `service add\|start\|stop\|restart\|remove\|list <kind>` | Manage databases / redis / memcached / mailpit |
| `park add <dir> --server <name> --php <ver> [--tld T] [--ssl]` | Auto-serve every subfolder as `<folder>.<tld>` |
| `park list\|remove <dir>` | Manage parked directories |
| `apply` | Render generated configs + reconcile running services |
| `validate` | Run every backend's native config test |
| `logs [<target>] [-n N] [--follow]` | View or tail a service's log |
| `doctor` | Diagnose the whole stack (brew, servers, FPM, services, DNS, certs, ports) |
| `ssl mint <host>` / `ssl trust` / `ssl ca` | Local certificates |
| `dns setup` / `dns status` | Wildcard `*.test` DNS |

## How it works

`state.toml` is the single source of truth (servers, PHP versions, vhosts).
reeve **renders** native configs (Caddyfile, httpd vhosts, nginx server
blocks, FPM pools) into `generated/`, then reconciles launchd services to match.
You never hand-edit generated files — you edit state via the CLI/TUI and `apply`.

```
~/Library/Application Support/reeve/
  config.toml · state.toml · generated/ · certs/ · logs/
~/.reeve/run/            # sockets (kept space-free, short)
```

## OpenLiteSpeed

OLS has no official macOS build, and the only community Homebrew tap fails to
compile its (2022-deprecated) `admin_php` dependency against the current macOS
SDK. The backend is wired in and will activate if a working OLS install appears,
but it is **not usable on macOS today**. Use Caddy, Apache, or nginx.

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
