//! Local SSL via a shared mkcert CA. The CA is installed once into the system
//! trust store; per-vhost certs are minted from it into `certs/`, so every
//! backend trusts the same root and browsers show no warning.

use crate::brew::Brew;
use crate::paths;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

fn mkcert_bin(brew: &Brew) -> PathBuf {
    brew.bin("mkcert")
}

/// Ensure the mkcert formula is installed.
pub fn ensure_installed(brew: &Brew) -> Result<()> {
    if !brew.is_installed("mkcert") {
        println!("Installing mkcert…");
        brew.install("mkcert")?;
    }
    Ok(())
}

/// Path to the mkcert CA root directory (`mkcert -CAROOT`).
pub fn caroot(brew: &Brew) -> Result<PathBuf> {
    let out = Command::new(mkcert_bin(brew))
        .arg("-CAROOT")
        .output()
        .context("Failed to run `mkcert -CAROOT`")?;
    if !out.status.success() {
        bail!("`mkcert -CAROOT` failed");
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

/// The CA root certificate path (`rootCA.pem`), if it exists. Useful for
/// `curl --cacert` verification and for pointing tools at the trust anchor.
pub fn ca_cert(brew: &Brew) -> Result<PathBuf> {
    Ok(caroot(brew)?.join("rootCA.pem"))
}

/// Ensure mkcert's local CA exists and is installed into the trust store.
/// `mkcert -install` is idempotent; on macOS the first run may prompt for
/// admin authorization to add the root to the System keychain. Output is
/// captured (not inherited) so it never corrupts the TUI's alternate screen.
/// Returns `true` if the CA was newly installed, `false` if it was already
/// present in the trust store.
pub fn ensure_ca(brew: &Brew) -> Result<bool> {
    ensure_installed(brew)?;
    let out = Command::new(mkcert_bin(brew))
        .arg("-install")
        .output()
        .context("Failed to run `mkcert -install`")?;
    if !out.status.success() {
        bail!(
            "`mkcert -install` failed: {}. You may need to authorize adding the \
             CA to your trust store, then re-run.",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // mkcert says "already installed" when nothing changed.
    Ok(!combined.contains("already installed"))
}

/// Remove the mkcert local CA from the trust store (`mkcert -uninstall`). Leaves
/// the CA files on disk, so a later `trust` re-installs the same root.
pub fn uninstall_ca(brew: &Brew) -> Result<()> {
    let out = Command::new(mkcert_bin(brew))
        .arg("-uninstall")
        .output()
        .context("Failed to run `mkcert -uninstall`")?;
    if !out.status.success() {
        bail!(
            "`mkcert -uninstall` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Whether the mkcert root CA is actually present in the macOS System keychain
/// (not merely generated on disk) — the real "is local HTTPS trusted" check.
pub fn is_trusted(brew: &Brew) -> bool {
    // mkcert's CA common name is "mkcert <user>@<host> …"; find it by the
    // stable "mkcert" prefix in the System keychain.
    let _ = brew;
    Command::new("security")
        .args([
            "find-certificate",
            "-c",
            "mkcert",
            "/Library/Keychains/System.keychain",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Certificate + key paths for a host (the convention the backends expect).
pub fn cert_paths(host: &str) -> Result<(PathBuf, PathBuf)> {
    let dir = paths::certs_dir()?;
    Ok((
        dir.join(format!("{host}.pem")),
        dir.join(format!("{host}-key.pem")),
    ))
}

/// Mint a certificate for `host` from the shared CA into `certs/`.
pub fn mint(brew: &Brew, host: &str) -> Result<()> {
    ensure_installed(brew)?;
    paths::ensure_dirs()?;
    let (cert, key) = cert_paths(host)?;
    // Capture (don't inherit) stdio: mkcert is chatty, and inherited output
    // would corrupt the TUI's alternate screen when minting during `apply`.
    let out = Command::new(mkcert_bin(brew))
        .arg("-cert-file")
        .arg(&cert)
        .arg("-key-file")
        .arg(&key)
        .arg(host)
        .output()
        .with_context(|| format!("Failed to run mkcert for {host}"))?;
    if !out.status.success() {
        bail!(
            "mkcert failed to mint a certificate for {host}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// True if both cert and key already exist for a host.
pub fn exists(host: &str) -> bool {
    cert_paths(host)
        .map(|(c, k)| c.exists() && k.exists())
        .unwrap_or(false)
}
