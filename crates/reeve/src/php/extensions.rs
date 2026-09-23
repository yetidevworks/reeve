//! PHP extension management, per version, via that version's own `pecl`/`php`
//! binaries. Each PHP version's extensions are independent — installing `redis`
//! for 8.3 doesn't touch 8.4.

use crate::brew::Brew;
use crate::php::formula;
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn php_bin(brew: &Brew, version: &str) -> PathBuf {
    brew.opt(&formula(version)).join("bin/php")
}

fn pecl_bin(brew: &Brew, version: &str) -> PathBuf {
    brew.opt(&formula(version)).join("bin/pecl")
}

/// Loaded modules for a version (`php -m`), excluding section headers.
pub fn list(brew: &Brew, version: &str) -> Result<Vec<String>> {
    let php = php_bin(brew, version);
    if !php.exists() {
        bail!("PHP {version} is not installed");
    }
    let out = Command::new(&php)
        .arg("-m")
        .output()
        .with_context(|| format!("Failed to run `php -m` for {version}"))?;
    if !out.status.success() {
        bail!("`php -m` failed for {version}");
    }
    let mods = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('['))
        .map(|l| l.to_string())
        .collect();
    Ok(mods)
}

/// Is an extension currently loaded for a version (case-insensitive `php -m`)?
pub fn is_loaded(brew: &Brew, version: &str, name: &str) -> Result<bool> {
    Ok(list(brew, version)?
        .iter()
        .any(|m| m.eq_ignore_ascii_case(name)))
}

/// Install + enable an extension via PECL for a specific version.
/// Auto-accepts PECL's interactive prompts with default answers.
pub fn add(brew: &Brew, version: &str, name: &str) -> Result<()> {
    let pecl = pecl_bin(brew, version);
    if !pecl.exists() {
        bail!(
            "pecl not found for PHP {version} (is {} installed?)",
            formula(version)
        );
    }
    // Building PECL extensions needs autoconf + a compiler (Command Line Tools).
    if !brew.is_installed("autoconf") {
        println!("Installing autoconf (needed to build PHP extensions)…");
        brew.install("autoconf")?;
    }

    println!("Building {name} for PHP {version} via pecl…");
    // phpize/pecl shell out to `autoconf`, `m4`, `make` by bare name. Under a
    // PATH-less environment (SSH, launchd) those aren't found, so inject the
    // brew bin dir into PATH and point PHP_AUTOCONF at the absolute binary.
    let brew_bin = brew.prefix.join("bin");
    let path = format!(
        "{}:{}",
        brew_bin.display(),
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into())
    );
    let mut child = Command::new(&pecl)
        .args(["install", "-f", name])
        .env("PATH", path)
        .env("PHP_AUTOCONF", brew_bin.join("autoconf"))
        .env("PHP_AUTOHEADER", brew_bin.join("autoheader"))
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn pecl install {name}"))?;
    // Feed newlines so config prompts (e.g. apcu's debug question) take defaults.
    if let Some(stdin) = child.stdin.take() {
        let mut stdin = stdin;
        let _ = stdin.write_all(b"\n\n\n\n\n\n\n\n");
    }
    let status = child.wait().context("pecl install failed to run")?;
    if !status.success() {
        bail!("pecl could not build '{name}' for PHP {version}");
    }
    Ok(())
}

/// Uninstall an extension via PECL for a specific version. Tolerates the
/// extension not being installed.
pub fn remove(brew: &Brew, version: &str, name: &str) -> Result<()> {
    let pecl = pecl_bin(brew, version);
    if !pecl.exists() {
        bail!("pecl not found for PHP {version}");
    }
    let out = Command::new(&pecl)
        .args(["uninstall", name])
        .output()
        .with_context(|| format!("Failed to run pecl uninstall {name}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.contains("not installed") && !stderr.trim().is_empty() {
            bail!("pecl uninstall failed: {}", stderr.trim());
        }
    }
    // pecl removes the .so but leaves its `extension=<name>.so` line behind,
    // which then errors on every startup. Strip any dangling entry ourselves.
    strip_extension_lines(brew, version, name)?;
    Ok(())
}

/// reeve's own conf.d file keeping Xdebug off for command-line PHP. The `zz-`
/// prefix sorts it after Homebrew's `ext-xdebug.ini`, so its value wins.
const CLI_XDEBUG_INI: &str = "zz-reeve-xdebug.ini";

const CLI_XDEBUG_BODY: &str = "\
; Written by reeve and rewritten on every FPM (re)start; edits are overwritten.
; Keeps Xdebug off for command-line PHP. Homebrew's ext-xdebug.ini sets
; xdebug.mode=debug, which would otherwise slow every CLI script down (about
; 7x on function calls). Web requests are unaffected: reeve sets their mode on
; the FPM master, see `reeve xdebug`. To debug a single CLI run:
;   XDEBUG_MODE=debug XDEBUG_SESSION=1 php script.php
[xdebug]
xdebug.mode = \"off\"
";

/// Keep Xdebug off for a version's command-line PHP while Xdebug is installed
/// for it, and drop reeve's override once it isn't. FPM is unaffected either
/// way: its `-d xdebug.mode` startup define outranks every ini file.
pub fn sync_cli_xdebug_ini(brew: &Brew, version: &str) -> Result<()> {
    let path = brew
        .etc("php")
        .join(version)
        .join("conf.d")
        .join(CLI_XDEBUG_INI);
    let declared = ini_files(brew, version).iter().any(|f| {
        fs::read_to_string(f).is_ok_and(|c| c.lines().any(|l| is_extension_line(l, "xdebug")))
    });
    if !declared {
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove {}", path.display()))?;
        }
        return Ok(());
    }
    if fs::read_to_string(&path).is_ok_and(|c| c == CLI_XDEBUG_BODY) {
        return Ok(());
    }
    fs::write(&path, CLI_XDEBUG_BODY).with_context(|| format!("Failed to write {}", path.display()))
}

/// All ini files that could declare an extension for a version: php.ini + conf.d.
fn ini_files(brew: &Brew, version: &str) -> Vec<PathBuf> {
    let base = brew.etc("php").join(version);
    let mut files = vec![base.join("php.ini")];
    if let Ok(rd) = fs::read_dir(base.join("conf.d")) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "ini") {
                files.push(e.path());
            }
        }
    }
    files
}

/// Is this a (non-commented) `extension=`/`zend_extension=` line for `name`?
fn is_extension_line(line: &str, name: &str) -> bool {
    let t = line.trim();
    if t.starts_with(';') {
        return false;
    }
    let lower = t.to_lowercase();
    (lower.starts_with("extension") || lower.starts_with("zend_extension"))
        && lower.contains(&name.to_lowercase())
}

/// Remove any `extension=<name>` lines from a version's ini files.
fn strip_extension_lines(brew: &Brew, version: &str, name: &str) -> Result<()> {
    for f in ini_files(brew, version) {
        let Ok(content) = fs::read_to_string(&f) else {
            continue;
        };
        let kept: Vec<&str> = content
            .lines()
            .filter(|l| !is_extension_line(l, name))
            .collect();
        if kept.len() != content.lines().count() {
            fs::write(&f, format!("{}\n", kept.join("\n")))
                .with_context(|| format!("Failed to update {}", f.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_xdebug_override_follows_whether_xdebug_is_installed() {
        let root = std::env::temp_dir().join(format!("reeve-xdebug-ini-{}", std::process::id()));
        let conf_d = root.join("etc/php/8.3/conf.d");
        fs::create_dir_all(&conf_d).unwrap();
        let brew = Brew {
            prefix: root.clone(),
        };
        let ours = conf_d.join(CLI_XDEBUG_INI);

        // No Xdebug declared: nothing written.
        sync_cli_xdebug_ini(&brew, "8.3").unwrap();
        assert!(!ours.exists());

        // Homebrew's ini loads Xdebug in debug mode: reeve's override appears.
        let ext = conf_d.join("ext-xdebug.ini");
        fs::write(
            &ext,
            "[xdebug]\nzend_extension=\"xdebug.so\"\nxdebug.mode=debug\n",
        )
        .unwrap();
        sync_cli_xdebug_ini(&brew, "8.3").unwrap();
        assert_eq!(fs::read_to_string(&ours).unwrap(), CLI_XDEBUG_BODY);
        // It must sort after the file it overrides.
        assert!(CLI_XDEBUG_INI > "ext-xdebug.ini");

        // A commented-out load doesn't count, and the override goes away.
        fs::write(&ext, ";zend_extension=\"xdebug.so\"\n").unwrap();
        sync_cli_xdebug_ini(&brew, "8.3").unwrap();
        assert!(!ours.exists());

        fs::remove_dir_all(&root).ok();
    }
}
