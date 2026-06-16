mod backends;
mod brew;
mod cli;
mod config;
mod daemon;
mod dns;
mod doctor;
mod logs;
mod ops;
mod park;
mod paths;
mod php;
mod preset;
mod services;
mod ssl;
mod state;
mod tui;
mod update;
mod vhost;

use anyhow::Result;
use backends::{backend_for, server_service_id};
use clap::Parser;
use cli::{
    Cli, Commands, DnsCommands, ParkCommands, PhpCommands, ServerCommands, ServiceCommands,
    SslCommands, VhostCommands,
};
use config::{load_config, save_config, Config};
use state::{load_state, save_state, Backend, Server, Vhost};
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => tui::run().await,
        Some(Commands::Init) => cmd_init(),
        Some(Commands::Php(c)) => cmd_php(c),
        Some(Commands::Server(c)) => cmd_server(c),
        Some(Commands::Vhost(c)) => cmd_vhost(c),
        Some(Commands::Service(c)) => cmd_service(c),
        Some(Commands::Park(c)) => cmd_park(c),
        Some(Commands::Apply) => cmd_apply(),
        Some(Commands::Validate) => cmd_validate(),
        Some(Commands::Ssl(c)) => cmd_ssl(c),
        Some(Commands::Dns(c)) => cmd_dns(c),
        Some(Commands::Logs {
            target,
            lines,
            follow,
        }) => cmd_logs(target, lines, follow),
        Some(Commands::Doctor) => cmd_doctor(),
        Some(Commands::TuiSnapshot {
            width,
            height,
            modal,
        }) => {
            print!("{}", tui::snapshot(width, height, &modal)?);
            Ok(())
        }
    }
}

fn cmd_dns(c: DnsCommands) -> Result<()> {
    let brew = brew::Brew::detect()?;
    let cfg = load_config().unwrap_or_default();
    let tlds = cfg.local_tlds;
    match c {
        DnsCommands::Setup => {
            let ok = dns::setup(&brew, &tlds)?;
            let list = tlds
                .iter()
                .map(|t| format!("*.{t}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "✓ dnsmasq resolving {list} → 127.0.0.1 [{}]",
                daemon::status(dns::service_id()).as_str()
            );
            if ok {
                println!("✓ /etc/resolver configured — {list} resolve system-wide");
            } else {
                println!("⚠ one or more /etc/resolver files not configured");
            }
            Ok(())
        }
        DnsCommands::Status => {
            println!(
                "dnsmasq service : {}",
                daemon::status(dns::service_id()).as_str()
            );
            for tld in &tlds {
                println!(
                    "/etc/resolver/{tld} : {}",
                    if dns::resolver_ready(tld) {
                        "present"
                    } else {
                        "missing"
                    }
                );
            }
            Ok(())
        }
    }
}

fn cmd_logs(target: Option<String>, lines: usize, follow: bool) -> Result<()> {
    let state = load_state().unwrap_or_default();
    let Some(target) = target else {
        println!("Available logs:");
        for t in logs::targets(&state)? {
            let where_ = if t.path.exists() {
                t.path.display().to_string()
            } else {
                "(no log yet)".to_string()
            };
            println!("  {:<16} {}", t.label, where_);
        }
        println!("\nView one with `reeve logs <target> [-n N] [--follow]`.");
        return Ok(());
    };
    let path = logs::resolve(&state, &target)?;
    if follow {
        logs::follow(&path, lines)
    } else {
        logs::tail(&path, lines)
    }
}

fn cmd_doctor() -> Result<()> {
    let brew = brew::Brew::detect().ok();
    let cfg = load_config().unwrap_or_default();
    let state = load_state().unwrap_or_default();
    let checks = doctor::run(brew.as_ref(), &cfg, &state);
    for c in &checks {
        println!("{} {:<14} {}", c.health.symbol(), c.name, c.detail);
    }
    let (mut warn, mut fail) = (0, 0);
    for c in &checks {
        match c.health {
            doctor::Health::Warn => warn += 1,
            doctor::Health::Fail => fail += 1,
            doctor::Health::Ok => {}
        }
    }
    println!();
    if fail > 0 {
        anyhow::bail!("{fail} check(s) failed, {warn} warning(s)");
    }
    if warn > 0 {
        println!("All critical checks passed ({warn} warning(s)).");
    } else {
        println!("All checks passed.");
    }
    Ok(())
}

fn cmd_init() -> Result<()> {
    let brew = brew::Brew::detect_or_offer_install()?;
    println!("✓ Homebrew detected at {}", brew.prefix.display());

    paths::ensure_dirs()?;
    println!("✓ Config root: {}", paths::config_dir()?.display());

    // Write config.toml if absent, seeding the detected brew prefix.
    let cfg_path = paths::config_path()?;
    if cfg_path.exists() {
        println!("• config.toml already exists — leaving it untouched");
    } else {
        let cfg = Config {
            brew_prefix: brew.prefix.display().to_string(),
            ..Config::default()
        };
        save_config(&cfg)?;
        println!("✓ Wrote {}", cfg_path.display());
    }

    // Seed an empty state file if absent.
    if !paths::state_path()?.exists() {
        save_state(&state::State::default())?;
        println!("✓ Wrote {}", paths::state_path()?.display());
    }

    // Report any PHP versions already installed via brew so the user can adopt
    // them instead of reinstalling.
    let existing = php::discover(&brew);
    if !existing.is_empty() {
        println!();
        println!("Found existing PHP install(s): {}", existing.join(", "));
        println!("  Adopt one with `reeve php install <version>` (reuses the brew install).");
    }

    println!();
    println!("Next steps:");
    println!("  reeve php install 8.3       # install a PHP version + FPM master");
    println!("  reeve server add caddy      # add a web server");
    println!("  reeve vhost add app.test --root ~/Sites/app --php 8.3 --server caddy");
    Ok(())
}

fn cmd_php(c: PhpCommands) -> Result<()> {
    match c {
        PhpCommands::Install { version } => {
            // Ensure brew exists (offer install) before the shared path runs.
            brew::Brew::detect_or_offer_install()?;
            ops::install_php(&version)?;
            println!(
                "✓ PHP {version} ready — FPM master '{}' is {}",
                daemon::label(&php::service_id(&version)),
                daemon::status(&php::service_id(&version)).as_str()
            );
            Ok(())
        }
        PhpCommands::List => {
            let state = load_state()?;
            if state.php_versions.is_empty() {
                println!("No PHP versions installed. Run `reeve php install <version>`.");
            } else {
                let cfg = load_config().ok();
                let default = cfg.and_then(|c| c.default_php);
                println!("{:<8} {:<9} {:<8} SOCKET", "VERSION", "FPM", "DEFAULT");
                for p in &state.php_versions {
                    let status = daemon::status(&php::service_id(&p.version));
                    let is_default = default.as_deref() == Some(p.version.as_str());
                    println!(
                        "{:<8} {:<9} {:<8} {}",
                        p.version,
                        status.as_str(),
                        if is_default { "*" } else { "" },
                        p.fpm_socket
                    );
                }
            }
            Ok(())
        }
        PhpCommands::Use { version } => {
            ops::set_default_php(&version)?;
            println!("✓ Default PHP set to {version}");
            Ok(())
        }
        PhpCommands::Ext(ext) => cmd_php_ext(ext),
        PhpCommands::Settings { version } => {
            let state = load_state()?;
            let php = state
                .get_php(&version)
                .ok_or_else(|| anyhow::anyhow!("PHP {version} is not managed"))?;
            println!("PHP {version} settings (★ = overridden):");
            println!("{:<28} {:<12} DEFAULT", "KEY", "VALUE");
            for def in php::php_settings_defs() {
                let val = php.setting(def.key, def.default);
                let overridden = php.settings.contains_key(def.key);
                println!(
                    "{}{:<27} {:<12} {}",
                    if overridden { "★" } else { " " },
                    def.key,
                    val,
                    def.default
                );
            }
            println!("Xdebug: {} (port {})", php.xdebug.as_str(), php.xdebug_port);
            Ok(())
        }
        PhpCommands::Set {
            version,
            key,
            value,
        } => {
            ops::set_php_setting(&version, &key, &value)?;
            println!("✓ PHP {version}: {key} = {value} (FPM restarted)");
            Ok(())
        }
        PhpCommands::Xdebug { version, mode } => {
            brew::Brew::detect_or_offer_install()?;
            let mode = state::XdebugMode::from_str(&mode)?;
            ops::set_xdebug(&version, mode)?;
            println!("✓ PHP {version}: Xdebug {} (FPM restarted)", mode.as_str());
            Ok(())
        }
    }
}

fn cmd_php_ext(ext: cli::ExtCommands) -> Result<()> {
    use cli::ExtCommands;
    let brew = brew::Brew::detect()?;
    match ext {
        ExtCommands::List { version } => {
            let mods = php::extensions::list(&brew, &version)?;
            println!("PHP {version} — {} loaded extensions:", mods.len());
            for chunk in mods.chunks(4) {
                println!("  {}", chunk.join("  "));
            }
            Ok(())
        }
        ExtCommands::Add { version, name } => {
            if php::extensions::list(&brew, &version)?
                .iter()
                .any(|m| m.eq_ignore_ascii_case(&name))
            {
                println!("• {name} is already loaded in PHP {version}");
                return Ok(());
            }
            php::extensions::add(&brew, &version, &name)?;
            ops::restart_fpm(&version)?;
            let loaded = php::extensions::list(&brew, &version)?
                .iter()
                .any(|m| m.eq_ignore_ascii_case(&name));
            if loaded {
                println!("✓ {name} installed and loaded in PHP {version} (FPM restarted)");
            } else {
                println!(
                    "⚠ {name} built but not showing in `php -m`. It may need an ini entry; \
                     check `reeve php ext list {version}`."
                );
            }
            Ok(())
        }
        ExtCommands::Remove { version, name } => {
            php::extensions::remove(&brew, &version, &name)?;
            ops::restart_fpm(&version)?;
            println!("✓ {name} removed from PHP {version} (FPM restarted)");
            Ok(())
        }
    }
}

fn cmd_server(c: ServerCommands) -> Result<()> {
    match c {
        ServerCommands::Add {
            backend,
            name,
            http,
            https,
            default_site,
        } => {
            let backend = Backend::from_str(&backend)?;
            let name = name.unwrap_or_else(|| backend.to_string());
            let mut state = load_state()?;
            state.add_server(Server {
                name: name.clone(),
                backend,
                http_port: http,
                https_port: https,
                enabled: false,
                default_site,
                settings: Default::default(),
            })?;
            save_state(&state)?;
            // Install the backend's brew formula now (visible), rather than
            // letting the first `apply`/`start` stall silently on it.
            let brew = brew::Brew::detect_or_offer_install()?;
            backend_for(backend).ensure_installed(&brew)?;
            println!("✓ Added {backend} server '{name}' (http:{http} https:{https})");
            println!("  Run `reeve apply` to render and start it.");
            Ok(())
        }
        ServerCommands::List => {
            let state = load_state()?;
            if state.servers.is_empty() {
                println!("No servers. Run `reeve server add <backend>`.");
            } else {
                println!(
                    "{:<14} {:<8} {:<7} {:<7} STATUS",
                    "NAME", "BACKEND", "HTTP", "HTTPS"
                );
                for s in &state.servers {
                    let status = daemon::status(&server_service_id(s));
                    println!(
                        "{:<14} {:<8} {:<7} {:<7} {}",
                        s.name,
                        s.backend,
                        s.http_port,
                        s.https_port,
                        status.as_str()
                    );
                }
            }
            Ok(())
        }
        ServerCommands::Remove { name } => {
            let mut state = load_state()?;
            let before = state.servers.len();
            state.servers.retain(|s| s.name != name);
            if state.servers.len() == before {
                anyhow::bail!("Server '{name}' not found");
            }
            state.vhosts.retain(|v| v.server != name);
            save_state(&state)?;
            println!("✓ Removed server '{name}' (and its vhosts)");
            Ok(())
        }
        ServerCommands::Start { name } => {
            let status = ops::start_server(&name)?;
            println!("✓ Started '{name}' — {}", status.as_str());
            Ok(())
        }
        ServerCommands::Stop { name } => {
            ops::stop_server(&name)?;
            println!("✓ Stopped '{name}'");
            Ok(())
        }
        ServerCommands::Restart { name } => {
            let status = ops::restart_server(&name)?;
            println!("✓ Restarted '{name}' — {}", status.as_str());
            Ok(())
        }
    }
}

fn cmd_vhost(c: VhostCommands) -> Result<()> {
    match c {
        VhostCommands::Add {
            server_name,
            root,
            php,
            server,
            ssl,
            preset,
            proxy,
        } => {
            let preset = state::Framework::from_str(&preset)?;
            let mut state = load_state()?;
            state.add_vhost(Vhost {
                server_name: server_name.clone(),
                server,
                docroot: root,
                php_version: php,
                ssl,
                preset,
                proxy_target: proxy,
            })?;
            save_state(&state)?;
            println!("✓ Added vhost '{server_name}'");
            println!("  Run `reeve apply` to render and reload.");
            Ok(())
        }
        VhostCommands::List => {
            let state = load_state()?;
            if state.vhosts.is_empty() {
                println!("No vhosts. Run `reeve vhost add <host> ...`.");
            } else {
                println!(
                    "{:<22} {:<12} {:<6} {:<5} {:<9} TARGET",
                    "HOST", "SERVER", "PHP", "SSL", "PRESET"
                );
                for v in &state.vhosts {
                    let target = v.proxy_target.clone().unwrap_or_else(|| v.docroot.clone());
                    println!(
                        "{:<22} {:<12} {:<6} {:<5} {:<9} {}",
                        v.server_name, v.server, v.php_version, v.ssl, v.preset, target
                    );
                }
            }
            Ok(())
        }
        VhostCommands::Remove { server_name } => {
            let mut state = load_state()?;
            let before = state.vhosts.len();
            state.vhosts.retain(|v| v.server_name != server_name);
            if state.vhosts.len() == before {
                anyhow::bail!("Vhost '{server_name}' not found");
            }
            save_state(&state)?;
            println!("✓ Removed vhost '{server_name}'");
            Ok(())
        }
    }
}

fn cmd_service(c: ServiceCommands) -> Result<()> {
    use state::ServiceKind;
    match c {
        ServiceCommands::Add { kind } => {
            let kind = ServiceKind::from_str(&kind)?;
            ops::add_service(kind)?;
            println!("✓ Added service '{kind}' — `reeve service start {kind}` to run it.");
            Ok(())
        }
        ServiceCommands::Start { kind } => {
            brew::Brew::detect_or_offer_install()?;
            let kind = ServiceKind::from_str(&kind)?;
            let status = ops::start_service(kind)?;
            println!(
                "✓ Started '{kind}' on :{} — {}",
                services::port(kind),
                status.as_str()
            );
            Ok(())
        }
        ServiceCommands::Stop { kind } => {
            let kind = ServiceKind::from_str(&kind)?;
            ops::stop_service(kind)?;
            println!("✓ Stopped '{kind}'");
            Ok(())
        }
        ServiceCommands::Restart { kind } => {
            let kind = ServiceKind::from_str(&kind)?;
            let status = ops::restart_service(kind)?;
            println!("✓ Restarted '{kind}' — {}", status.as_str());
            Ok(())
        }
        ServiceCommands::Remove { kind } => {
            let kind = ServiceKind::from_str(&kind)?;
            ops::remove_service(kind)?;
            println!("✓ Removed service '{kind}'");
            Ok(())
        }
        ServiceCommands::List => {
            let state = load_state()?;
            if state.services.is_empty() {
                println!(
                    "No services. Add one with `reeve service add <mysql|mariadb|postgres|redis|memcached|mailpit>`."
                );
                return Ok(());
            }
            println!("{:<12} {:<7} {:<9} STATUS", "SERVICE", "PORT", "ENABLED");
            for s in &state.services {
                let status = daemon::status(&services::service_id(s.kind));
                println!(
                    "{:<12} {:<7} {:<9} {}",
                    s.kind,
                    services::port(s.kind),
                    if s.enabled { "yes" } else { "no" },
                    status.as_str()
                );
            }
            Ok(())
        }
    }
}

fn cmd_park(c: ParkCommands) -> Result<()> {
    match c {
        ParkCommands::Add {
            dir,
            server,
            php,
            tld,
            ssl,
        } => {
            let cfg = load_config()?;
            let root = expand_tilde(&dir);
            let tld = tld.unwrap_or_else(|| cfg.local_tlds[0].clone());
            let n = ops::add_park(&root, &server, &php, &tld, ssl)?;
            println!("✓ Parked {root} → *.{tld} ({n} site(s) found)");
            println!("  Run `reeve apply` to render and reload.");
            Ok(())
        }
        ParkCommands::Remove { dir } => {
            let root = expand_tilde(&dir);
            ops::remove_park(&root)?;
            println!("✓ Unparked {root} — run `reeve apply` to drop its sites");
            Ok(())
        }
        ParkCommands::List => {
            let state = load_state()?;
            if state.parks.is_empty() {
                println!("No parked directories. Park one with `reeve park add <dir> --server <s> --php <v>`.");
                return Ok(());
            }
            for p in &state.parks {
                let sites = park::expand(p);
                println!(
                    "{}  → *.{}  ({}, PHP {}, {})",
                    p.root,
                    p.tld,
                    p.server,
                    p.php_version,
                    if p.ssl { "https" } else { "http" }
                );
                for v in sites {
                    println!("    {}  {}", v.server_name, v.docroot);
                }
            }
            Ok(())
        }
    }
}

/// Expand a leading `~` to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    path.to_string()
}

fn cmd_apply() -> Result<()> {
    let brew = brew::Brew::detect()?;
    let cfg = load_config()?;
    let state = load_state()?;
    if state.servers.is_empty() {
        println!("Nothing to apply — no servers defined.");
        return Ok(());
    }
    for server in &state.servers {
        let backend = backend_for(server.backend);
        backend.ensure_installed(&brew)?;
        let vhosts_owned = park::effective_vhosts_for(&state, &server.name);
        let vhosts: Vec<&Vhost> = vhosts_owned.iter().collect();
        ops::ensure_vhost_certs(&vhosts, &brew)?;
        // The default site's HTTPS catch-all needs a `localhost` cert.
        if server.default_site && !ssl::exists(backends::DEFAULT_SITE_HOST) {
            ssl::mint(&brew, backends::DEFAULT_SITE_HOST)?;
        }
        backend.render(server, &vhosts, &state, &cfg, &brew)?;
        backend.validate(server, &brew)?;
        if server.enabled {
            let spec = backend.service_spec(server, &brew)?;
            daemon::install(&spec)?;
            daemon::restart(&server_service_id(server))?;
        }
        let status = if server.enabled {
            daemon::status(&server_service_id(server)).as_str()
        } else {
            "stopped"
        };
        println!(
            "✓ {} ({}) — {} vhost(s) [{}]",
            server.name,
            server.backend,
            vhosts.len(),
            status
        );
    }
    Ok(())
}

fn cmd_validate() -> Result<()> {
    let brew = brew::Brew::detect()?;
    let state = load_state()?;
    if state.servers.is_empty() {
        println!("No servers to validate.");
        return Ok(());
    }
    let mut all_ok = true;
    for server in &state.servers {
        let backend = backend_for(server.backend);
        match backend.validate(server, &brew) {
            Ok(()) => println!("✓ {} ({}) config valid", server.name, server.backend),
            Err(e) => {
                all_ok = false;
                println!("✗ {} ({}): {e}", server.name, server.backend);
            }
        }
    }
    if !all_ok {
        anyhow::bail!("one or more server configs are invalid");
    }
    Ok(())
}

fn cmd_ssl(c: SslCommands) -> Result<()> {
    let brew = brew::Brew::detect()?;
    match c {
        SslCommands::Mint { server_name } => {
            ssl::mint(&brew, &server_name)?;
            let (cert, _) = ssl::cert_paths(&server_name)?;
            println!("✓ Minted certificate for {server_name}");
            println!("  {}", cert.display());
            if !trust_store_ready(&brew) {
                println!("  Run `reeve ssl trust` once so browsers trust it.");
            }
            Ok(())
        }
        SslCommands::Trust => {
            ssl::ensure_ca(&brew)?;
            println!("✓ mkcert local CA installed into the trust store");
            Ok(())
        }
        SslCommands::Ca => {
            println!("{}", ssl::ca_cert(&brew)?.display());
            Ok(())
        }
    }
}

/// Best-effort check that the mkcert CA root exists (i.e. has been generated).
fn trust_store_ready(brew: &brew::Brew) -> bool {
    ssl::ca_cert(brew).map(|p| p.exists()).unwrap_or(false)
}
