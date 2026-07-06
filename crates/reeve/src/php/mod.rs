//! PHP version management. Each version runs its own php-fpm master listening
//! on a dedicated unix socket, so vhosts can target different versions
//! simultaneously (the core capability mod_php cannot provide).

pub mod extensions;

use crate::brew::Brew;
use crate::daemon::{self, ServiceSpec};
use crate::paths;
use crate::state::PhpVersion;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

/// Brew formula name for a PHP version, e.g. "8.3" -> "[email protected]".
pub fn formula(version: &str) -> String {
    format!("php@{version}")
}

/// Compact form, e.g. "8.3" -> "83".
fn compact(version: &str) -> String {
    version.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// FPM socket path for a version, e.g. "8.3" -> `<run>/php83.sock`.
pub fn fpm_socket(version: &str) -> Result<PathBuf> {
    Ok(paths::run_dir()?.join(format!("php{}.sock", compact(version))))
}

/// launchd service id for a version's fpm master, e.g. "php-83".
pub fn service_id(version: &str) -> String {
    format!("php-{}", compact(version))
}

/// The FPM master's combined stdout/stderr launchd log for a version,
/// e.g. `<logs>/php83-launchd.log`.
pub fn launchd_log(version: &str) -> Result<PathBuf> {
    Ok(paths::logs_dir()?.join(format!("php{}-launchd.log", compact(version))))
}

/// Absolute php-fpm binary for a version (shivammathur layout).
fn fpm_binary(brew: &Brew, version: &str) -> PathBuf {
    brew.opt(&formula(version)).join("sbin/php-fpm")
}

/// php.ini directory for a version, e.g. `<prefix>/etc/php/8.3`.
fn ini_dir(brew: &Brew, version: &str) -> PathBuf {
    brew.etc("php").join(version)
}

fn current_user() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| {
            Command::new("id")
                .arg("-un")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "_www".to_string())
        })
}

/// The FPM pool group. macOS keeps `staff` (its admin users' primary group,
/// unchanged from the original behavior); other platforms use the current
/// user's real primary group. FPM ignores user/group unless it runs as root
/// (reeve never does), so this is cosmetic, but `staff` is wrong off macOS.
fn fpm_group() -> String {
    if cfg!(target_os = "macos") {
        "staff".to_string()
    } else {
        Command::new("id")
            .arg("-gn")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(current_user)
    }
}

/// Where a php.ini key lands in the generated FPM pool: a raw pool directive
/// (`pm.max_children`), a `php_admin_value[...]`, or a `php_admin_flag[...]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhpSettingKind {
    /// Emitted verbatim in `[www]`, e.g. `pm.max_children = 10`.
    Pool,
    /// `php_admin_value[key] = value`.
    Value,
    /// `php_admin_flag[key] = on|off`.
    Flag,
}

/// A tunable PHP setting reeve exposes per version. Mirrors
/// [`crate::backends::SettingDef`] but adds where the value is rendered.
pub struct PhpSettingDef {
    pub key: &'static str,
    pub label: &'static str,
    pub default: &'static str,
    pub help: &'static str,
    pub kind: PhpSettingKind,
}

/// Every per-version PHP tunable, in display order. The `key` doubles as the
/// `php.ini` directive / pool key and the storage key in `PhpVersion::settings`.
pub fn php_settings_defs() -> &'static [PhpSettingDef] {
    use PhpSettingKind::*;
    &[
        // php.ini runtime limits.
        PhpSettingDef {
            key: "memory_limit",
            label: "Memory limit",
            default: "256M",
            help: "e.g. 256M, 1G",
            kind: Value,
        },
        PhpSettingDef {
            key: "upload_max_filesize",
            label: "Max upload size",
            default: "64M",
            help: "e.g. 64M",
            kind: Value,
        },
        PhpSettingDef {
            key: "post_max_size",
            label: "Max POST size",
            default: "64M",
            help: "≥ upload size",
            kind: Value,
        },
        PhpSettingDef {
            key: "max_execution_time",
            label: "Max exec time (s)",
            default: "60",
            help: "0 = unlimited",
            kind: Value,
        },
        PhpSettingDef {
            key: "max_input_vars",
            label: "Max input vars",
            default: "2000",
            help: "form/array inputs",
            kind: Value,
        },
        PhpSettingDef {
            key: "date.timezone",
            label: "Timezone",
            default: "UTC",
            help: "e.g. America/New_York",
            kind: Value,
        },
        PhpSettingDef {
            key: "display_errors",
            label: "Display errors",
            default: "on",
            help: "on | off",
            kind: Flag,
        },
        // OPcache.
        PhpSettingDef {
            key: "opcache.enable",
            label: "OPcache",
            default: "1",
            help: "1 = on, 0 = off",
            kind: Value,
        },
        PhpSettingDef {
            key: "opcache.memory_consumption",
            label: "OPcache MB",
            default: "128",
            help: "shared memory (MB)",
            kind: Value,
        },
        PhpSettingDef {
            key: "opcache.revalidate_freq",
            label: "OPcache revalidate",
            default: "2",
            help: "seconds; 0 = always",
            kind: Value,
        },
        // FPM process manager.
        PhpSettingDef {
            key: "pm",
            label: "Process manager",
            default: "dynamic",
            help: "dynamic | static | ondemand",
            kind: Pool,
        },
        PhpSettingDef {
            key: "pm.max_children",
            label: "Max children",
            default: "10",
            help: "worker ceiling",
            kind: Pool,
        },
        PhpSettingDef {
            key: "pm.start_servers",
            label: "Start servers",
            default: "2",
            help: "initial workers",
            kind: Pool,
        },
        PhpSettingDef {
            key: "pm.min_spare_servers",
            label: "Min spare",
            default: "1",
            help: "idle floor",
            kind: Pool,
        },
        PhpSettingDef {
            key: "pm.max_spare_servers",
            label: "Max spare",
            default: "4",
            help: "idle ceiling",
            kind: Pool,
        },
    ]
}

/// Render the self-contained FPM config (global + one pool) for a version into
/// `generated/fpm/phpXY.conf`, returning its path. reeve owns this file;
/// it never touches Homebrew's default php-fpm.conf. All php.ini / OPcache /
/// pool tunables and the Xdebug mode come from the [`PhpVersion`] record.
pub fn render_fpm_conf(php: &PhpVersion) -> Result<PathBuf> {
    paths::ensure_dirs()?;
    let c = compact(&php.version);
    let conf = build_fpm_conf(php)?;
    let path = paths::generated_dir()?
        .join("fpm")
        .join(format!("php{c}.conf"));
    std::fs::write(&path, conf)
        .with_context(|| format!("Failed to write FPM config {}", path.display()))?;
    Ok(path)
}

/// Build the FPM pool config string for a version (pure; no disk writes), so it
/// can be unit-tested. `render_fpm_conf` writes the result.
fn build_fpm_conf(php: &PhpVersion) -> Result<String> {
    let version = &php.version;
    let c = compact(version);
    let socket = fpm_socket(version)?;
    let user = current_user();
    let group = fpm_group();
    let run = paths::run_dir()?;
    let logs = paths::logs_dir()?;

    // Header + pool identity.
    let mut conf = format!(
        "; Generated by reeve — do not edit by hand.\n\
         ; PHP {version} FPM master.\n\
         [global]\n\
         pid = {run}/php{c}-fpm.pid\n\
         error_log = {logs}/php{c}-fpm.log\n\
         daemonize = no\n\
         \n\
         [www]\n\
         user = {user}\n\
         group = {group}\n\
         listen = {socket}\n\
         listen.owner = {user}\n\
         listen.group = {group}\n\
         listen.mode = 0660\n",
        version = version,
        c = c,
        run = run.display(),
        logs = logs.display(),
        user = user,
        group = group,
        socket = socket.display(),
    );

    // Tunable directives, grouped by where they land.
    for def in php_settings_defs() {
        // `opcache.enable` is a startup-only directive: emitting it as a
        // per-request `php_admin_value` makes PHP warn "Zend OPcache can't be
        // temporary enabled" on every request. It's passed as a `-d` startup
        // define instead (see `fpm_define_args`).
        if def.key == "opcache.enable" {
            continue;
        }
        let val = php.setting(def.key, def.default);
        match def.kind {
            PhpSettingKind::Pool => conf.push_str(&format!("{} = {}\n", def.key, val)),
            PhpSettingKind::Value => {
                conf.push_str(&format!("php_admin_value[{}] = {}\n", def.key, val))
            }
            PhpSettingKind::Flag => {
                conf.push_str(&format!("php_admin_flag[{}] = {}\n", def.key, val))
            }
        }
    }

    // Fixed plumbing reeve always owns (logging + env passthrough).
    conf.push_str(&format!(
        "catch_workers_output = yes\n\
         clear_env = no\n\
         php_admin_value[error_log] = {logs}/php{c}-php.log\n\
         php_admin_flag[log_errors] = on\n",
        logs = logs.display(),
        c = c,
    ));

    // NB: Xdebug and `opcache.enable` are NOT written here. `xdebug.mode` and
    // `opcache.enable` are PHP_INI_SYSTEM startup directives, and some Homebrew
    // conf.d files hard-set `xdebug.mode=debug`; a pool `php_admin_value` does
    // not reliably override a zend_extension's startup mode, leaving Xdebug
    // actively instrumenting every call (5-50x slower for Twig/Grav). They are
    // forced on the FPM command line via `fpm_define_args`, where they win.

    Ok(conf)
}

/// `-d key=value` startup defines for the php-fpm master. Used for directives
/// that must be set at startup (and that conf.d files like `ext-xdebug.ini`
/// otherwise hard-set): Xdebug's mode/port and `opcache.enable`. Passing these
/// on the command line reliably overrides the scanned ini files, unlike a pool
/// `php_admin_value`.
fn fpm_define_args(php: &PhpVersion) -> Vec<String> {
    let mut d = Vec::new();
    // Default Xdebug fully off so an installed-but-idle Xdebug stops adding
    // per-call overhead; when enabled, set the client port + auto-trigger too.
    d.push("-d".into());
    d.push(format!("xdebug.mode={}", php.xdebug.as_str()));
    if !php.xdebug.is_off() {
        d.push("-d".into());
        d.push(format!("xdebug.client_port={}", php.xdebug_port));
        d.push("-d".into());
        d.push("xdebug.start_with_request=yes".into());
    }
    // opcache.enable as a startup define (avoids the per-request warning).
    d.push("-d".into());
    d.push(format!(
        "opcache.enable={}",
        php.setting("opcache.enable", "1")
    ));
    d
}

/// Stand up (or restart) the launchd-managed FPM master for a version, applying
/// the version's php.ini / OPcache / pool / Xdebug settings.
pub fn ensure_fpm_running(brew: &Brew, php: &PhpVersion) -> Result<()> {
    let version = &php.version;
    let conf = render_fpm_conf(php)?;
    let bin = fpm_binary(brew, version);
    if !bin.exists() {
        bail!(
            "php-fpm binary not found at {} — is {} installed?",
            bin.display(),
            formula(version)
        );
    }
    let mut args = vec![
        "--nodaemonize".into(),
        "--fpm-config".into(),
        conf.display().to_string(),
        "-c".into(),
        ini_dir(brew, version).display().to_string(),
    ];
    args.extend(fpm_define_args(php));
    let spec = ServiceSpec {
        service: service_id(version),
        program: bin,
        args,
        log: launchd_log(version)?,
        keep_alive: true,
        run_at_load: true,
    };
    daemon::install(&spec)?;

    // Remove any pre-existing socket before (re)starting so the health check
    // can't be fooled into reporting success against a stale socket — or one a
    // hand-run `php-fpm` is holding open — that our launchd job doesn't own.
    let socket = fpm_socket(version)?;
    let _ = std::fs::remove_file(&socket);

    daemon::restart(&service_id(version))?;

    // Health check: require launchd to actually own a running master AND the
    // socket to be live. A socket file alone is not enough (a foreign/stale
    // process can hold one); a PID alone is not enough (the master may still be
    // binding). Both together mean *our* FPM master is up and accepting work.
    let service = service_id(version);
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if daemon::pid(&service).is_some() && daemon::socket_alive(&socket) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(150));
    }

    // Socket never showed. launchd's redirected log is frequently empty here:
    // php-fpm sends startup errors to its own `[global] error_log`, and a
    // dyld/link failure dies before any log opens at all. So capture the real
    // reason ourselves with a synchronous config test, plus any FPM-log tail.
    let diag = fpm_failure_diagnostics(brew, version, &conf);
    let fpm_log = paths::logs_dir()?.join(format!("php{}-fpm.log", compact(version)));
    let where_to_look = format!(
        "Logs (if any): {} and {}.",
        fpm_log.display(),
        launchd_log(version)?.display()
    );
    if diag.trim().is_empty() {
        bail!(
            "FPM master for PHP {version} started but its socket never appeared at {}.\n\
             No diagnostics captured. {where_to_look}",
            socket.display(),
        )
    }
    bail!(
        "FPM master for PHP {version} started but its socket never appeared at {}.\n\n{diag}\n\n\
         {where_to_look}",
        socket.display(),
    )
}

/// Best-effort diagnosis when an FPM master's socket never appears. launchd's
/// redirected log is often empty in that case — php-fpm writes startup errors
/// to its own `[global] error_log`, and a dyld/link failure dies before any log
/// opens. So run `php-fpm -t` synchronously (we capture its stderr directly,
/// including dyld/link/config errors the async launchd job swallows) and tail
/// the FPM error log, returning whatever explains the failure.
fn fpm_failure_diagnostics(brew: &Brew, version: &str, conf: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();

    let bin = fpm_binary(brew, version);
    match Command::new(&bin)
        .arg("-t")
        .arg("--fpm-config")
        .arg(conf)
        .arg("-c")
        .arg(ini_dir(brew, version))
        .output()
    {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stderr).into_owned();
            text.push_str(&String::from_utf8_lossy(&o.stdout));
            let text = text.trim();
            if !o.status.success() && !text.is_empty() {
                parts.push(format!("php-fpm config test failed:\n{text}"));
            }
        }
        Err(e) => parts.push(format!("could not run `{} -t`: {e}", bin.display())),
    }

    if let Ok(log) = paths::logs_dir().map(|d| d.join(format!("php{}-fpm.log", compact(version)))) {
        if let Ok(contents) = std::fs::read_to_string(&log) {
            let lines: Vec<&str> = contents.lines().collect();
            let tail = lines[lines.len().saturating_sub(8)..].join("\n");
            if !tail.trim().is_empty() {
                parts.push(format!("Last lines of {}:\n{tail}", log.display()));
            }
        }
    }

    parts.join("\n\n")
}

/// Is this PHP version's brew formula installed?
pub fn is_installed(brew: &Brew, version: &str) -> bool {
    brew.is_installed(&formula(version))
}

/// The toolchain binaries reeve shims for the CLI. `php` is the one that
/// matters; the rest keep `pecl`/`phpize`/`php-config`/`phar` in lockstep so
/// extension builds and version queries match the active CLI php.
const SHIM_BINARIES: &[&str] = &["php", "php-config", "phpize", "pecl", "phar"];

/// Point the `~/.reeve/bin` shims at `version`'s toolchain, making it the CLI
/// `php` (for any shell with `~/.reeve/bin` ahead of Homebrew's bin on PATH).
/// Pure symlink work — Homebrew's link state is never touched.
pub fn set_cli_php(brew: &Brew, version: &str) -> Result<()> {
    if !is_installed(brew, version) {
        bail!("PHP {version} is not installed — run `reeve php install {version}` first.");
    }
    let shim = paths::shim_dir()?;
    std::fs::create_dir_all(&shim)
        .with_context(|| format!("Failed to create shim dir {}", shim.display()))?;
    let bindir = brew.opt(&formula(version)).join("bin");

    for name in SHIM_BINARIES {
        let link = shim.join(name);
        // Always clear the old shim first so a repoint can't fail on EEXIST and
        // so binaries absent from the target version don't leave stale links.
        let _ = std::fs::remove_file(&link);
        let target = bindir.join(name);
        if !target.exists() {
            continue;
        }
        std::os::unix::fs::symlink(&target, &link).with_context(|| {
            format!("Failed to link {} -> {}", link.display(), target.display())
        })?;
    }
    Ok(())
}

/// The PHP version the CLI shim currently points at, if set. Read from the
/// `~/.reeve/bin/php` symlink's `php@<ver>` target (no subprocess).
pub fn current_cli_php() -> Option<String> {
    let link = paths::shim_dir().ok()?.join("php");
    let target = std::fs::read_link(&link).ok()?;
    // …/opt/php@8.4/bin/php  ->  8.4
    target
        .components()
        .find_map(|c| c.as_os_str().to_str()?.strip_prefix("php@"))
        .map(str::to_string)
}

/// True when `~/.reeve/bin` is on PATH ahead of Homebrew's bin (so the shims
/// actually win). Used to warn the user that switching won't take effect yet.
pub fn shim_on_path(brew: &Brew) -> bool {
    let Ok(shim) = paths::shim_dir() else {
        return false;
    };
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    let brew_bin = brew.prefix.join("bin");
    for entry in std::env::split_paths(&path) {
        if entry == shim {
            return true;
        }
        if entry == brew_bin {
            // Homebrew's bin comes first — its linked php would shadow the shim.
            return false;
        }
    }
    false
}

/// Full install: ensure the tap + formula, then stand up the FPM master and
/// return the [`PhpVersion`] record to persist. Reuses an existing brew install
/// rather than reinstalling.
pub fn install(brew: &Brew, version: &str) -> Result<PhpVersion> {
    brew.ensure_tap("shivammathur/php")?;
    // Bleeding-edge versions (e.g. 8.6+) live only in the tap, not Homebrew
    // core, and newer Homebrew refuses to load untrusted taps. Trust it so
    // tap-only versions install. (Core versions like 8.4/8.5 ignore this.)
    brew.trust_tap("shivammathur/php");
    if is_installed(brew, version) {
        println!("• {} already installed — adopting it", formula(version));
    } else {
        println!(
            "Installing {} (this can take a few minutes)…",
            formula(version)
        );
        brew.install(&formula(version))?;
    }
    let record = PhpVersion {
        version: version.to_string(),
        fpm_socket: fpm_socket(version)?.display().to_string(),
        ..Default::default()
    };
    ensure_fpm_running(brew, &record)?;
    Ok(record)
}

/// Scan `<prefix>/opt/php@*` for already-installed versions.
pub fn discover(brew: &Brew) -> Vec<String> {
    let opt = brew.prefix.join("opt");
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&opt) {
        for e in entries.flatten() {
            if let Some(name) = e.file_name().to_str() {
                if let Some(ver) = name.strip_prefix("php@") {
                    found.push(ver.to_string());
                }
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::state::XdebugMode;

    #[test]
    fn naming_conventions() {
        assert_eq!(formula("8.3"), "php@8.3");
        assert_eq!(compact("8.3"), "83");
        assert_eq!(service_id("8.4"), "php-84");
        assert!(fpm_socket("8.3").unwrap().ends_with("php83.sock"));
    }

    #[test]
    fn fpm_conf_uses_defaults_then_overrides() {
        let mut php = PhpVersion {
            version: "8.3".into(),
            ..Default::default()
        };
        // Defaults render for untouched keys.
        let conf = build_fpm_conf(&php).unwrap();
        assert!(conf.contains("pm.max_children = 10"));
        assert!(conf.contains("php_admin_value[memory_limit] = 256M"));
        assert!(conf.contains("php_admin_flag[display_errors] = on"));
        // Xdebug and opcache.enable are startup defines, NOT pool values (a pool
        // php_admin_value doesn't reliably override conf.d's xdebug.mode).
        assert!(!conf.contains("xdebug.mode"));
        assert!(!conf.contains("php_admin_value[opcache.enable]"));
        assert_eq!(
            fpm_define_args(&php),
            vec!["-d", "xdebug.mode=off", "-d", "opcache.enable=1"]
        );

        // Pool overrides still flow through the conf.
        php.settings.insert("pm.max_children".into(), "32".into());
        php.settings.insert("memory_limit".into(), "1G".into());
        let conf = build_fpm_conf(&php).unwrap();
        assert!(conf.contains("pm.max_children = 32"));
        assert!(conf.contains("php_admin_value[memory_limit] = 1G"));

        // Enabling Xdebug adds the port + auto-trigger to the startup defines.
        php.xdebug = XdebugMode::Debug;
        php.xdebug_port = 9009;
        assert_eq!(
            fpm_define_args(&php),
            vec![
                "-d",
                "xdebug.mode=debug",
                "-d",
                "xdebug.client_port=9009",
                "-d",
                "xdebug.start_with_request=yes",
                "-d",
                "opcache.enable=1",
            ]
        );
    }
}
