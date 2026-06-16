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
