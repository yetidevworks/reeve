//! `reeve update [--check]` — self-update from the latest GitHub release.
//!
//! Mirrors the pattern used by sibling tools `bosun` / `ygrep`:
//! - hits `api.github.com/repos/yetidevworks/reeve/releases/latest`
//! - caches the result under reeve's config dir
//! - detects install method by inspecting `current_exe()`:
//!     * Homebrew  → print `brew upgrade reeve`
//!     * Cargo     → print `cargo install --force reeve-cli`
//!     * Binary    → download, extract, atomically replace in place
//!
//! Replacing the running binary is safe on Unix: the kernel keeps the
//! inode alive until every open fd / running mmap closes, so the user
//! can run `reeve update` from a separate shell while a TUI session is
//! still attached — the new binary slides in for the next launch.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const GITHUB_REPO_OWNER: &str = "yetidevworks";
const GITHUB_REPO_NAME: &str = "reeve";

// ---------- version helpers ----------

fn parse_version(v: &str) -> (u64, u64, u64) {
    let parts: Vec<u64> = v.split('.').filter_map(|p| p.parse().ok()).collect();
    (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    )
}

fn is_newer(current: &str, latest: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

// ---------- cache ----------

#[derive(serde::Serialize, serde::Deserialize)]
struct UpdateCache {
    latest_version: String,
    checked_at: u64,
}

fn cache_path() -> Result<PathBuf> {
    Ok(crate::paths::config_dir()?.join("update-check.json"))
}

fn write_cache(cache: &UpdateCache) -> Result<()> {
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string(cache)?)?;
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------- GitHub API ----------

fn fetch_latest_version() -> Result<String> {
    let url = format!(
        "https://api.github.com/repos/{GITHUB_REPO_OWNER}/{GITHUB_REPO_NAME}/releases/latest"
    );

    let resp = ureq::get(&url)
        .set(
            "User-Agent",
            &format!("reeve/{}", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/vnd.github.v3+json")
        .call()
        .context("failed to reach GitHub API")?;

    let body: serde_json::Value = resp
        .into_json()
        .context("failed to parse GitHub response")?;

    let tag = body["tag_name"]
        .as_str()
        .context("no tag_name in release")?;

    Ok(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

// ---------- platform ----------

/// Map the running target triple onto the asset suffix used by the
/// `release.yml` workflow. Returns `None` for unsupported targets (the
/// only release-prebuilt platforms are macOS x86_64/aarch64 and Linux
/// x86_64/aarch64); other platforms fall back to a printed instruction
/// to update via cargo / source.
fn platform_target() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some("darwin-aarch64");
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Some("darwin-x86_64");
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Some("linux-x86_64");
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Some("linux-aarch64");
    #[allow(unreachable_code)]
    None
}

// ---------- install method ----------

enum InstallMethod {
    Homebrew,
    Cargo,
    Binary(PathBuf),
}

fn detect_install_method() -> InstallMethod {
    let exe = std::env::current_exe().unwrap_or_default();
    let s = exe.to_string_lossy();

    if s.contains("/Cellar/") || s.contains("/homebrew/") {
        InstallMethod::Homebrew
    } else if s.contains("/.cargo/bin/") {
        // `cargo install reeve-cli` lands here. `cargo install --root
        // ~/.local` does NOT — that path is `~/.local/bin/reeve`, which
        // we treat as a binary install and replace directly.
        InstallMethod::Cargo
    } else {
        InstallMethod::Binary(exe)
    }
}

// ---------- public entry point ----------

/// `reeve update [--check]`
pub fn run(check_only: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");

    eprintln!("Checking for updates…");
    let latest = fetch_latest_version()?;

    let _ = write_cache(&UpdateCache {
        latest_version: latest.clone(),
        checked_at: now_secs(),
    });

    if !is_newer(current, &latest) {
        eprintln!("reeve v{current} is already the latest version.");
        return Ok(());
    }

    eprintln!("Update available: v{current} → v{latest}");

    if check_only {
        eprintln!("\nRun `reeve update` to install.");
        return Ok(());
    }

    match detect_install_method() {
        InstallMethod::Homebrew => {
            eprintln!("\nreeve was installed via Homebrew. Run:");
            eprintln!("  brew upgrade reeve");
        }
        InstallMethod::Cargo => {
            eprintln!("\nreeve was installed via cargo. Run:");
            eprintln!("  cargo install --force reeve-cli");
        }
        InstallMethod::Binary(exe_path) => {
            perform_update(&exe_path, &latest)?;
        }
    }

    Ok(())
}

// ---------- download & replace ----------

fn perform_update(exe_path: &Path, version: &str) -> Result<()> {
    let target = platform_target().context(
        "unsupported platform for self-update — install via cargo or download a release \
         binary from https://github.com/yetidevworks/reeve/releases",
    )?;

    // release.yml ships `reeve-<target>.tar.gz` (no version in the
    // filename — the asset name is constant per tag).
    let asset_name = format!("reeve-{target}.tar.gz");
    let download_url = format!(
        "https://github.com/{GITHUB_REPO_OWNER}/{GITHUB_REPO_NAME}/releases/download/v{version}/{asset_name}"
    );

    eprintln!("Downloading {asset_name}…");

    let tmp = std::env::temp_dir().join(format!("reeve-update-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let _cleanup = TempDirGuard(tmp.clone());

    let archive_path = tmp.join(&asset_name);

    let resp = ureq::get(&download_url)
        .set(
            "User-Agent",
            &format!("reeve/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .context("failed to download release")?;

    let mut body = resp.into_reader();
    let mut file = std::fs::File::create(&archive_path)?;
    std::io::copy(&mut body, &mut file)?;
    drop(file);

    extract_tar_gz(&archive_path, &tmp)?;

    let new_bin = tmp.join("reeve");
    if !new_bin.exists() {
        anyhow::bail!("binary not found in archive");
    }

    replace_binary(&new_bin, exe_path)?;

    eprintln!("Updated reeve to v{version}.");
    eprintln!("Quit and relaunch reeve to pick up the new binary.");
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let status = std::process::Command::new("tar")
        .args([
            "xzf",
            &archive.to_string_lossy(),
            "-C",
            &dest.to_string_lossy(),
        ])
        .status()
        .context("failed to run tar")?;

    if !status.success() {
        anyhow::bail!("tar extraction failed");
    }
    Ok(())
}

fn replace_binary(new_bin: &Path, exe_path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(new_bin, std::fs::Permissions::from_mode(0o755))?;

    // Atomic rename when src and dst share a filesystem; fall back to
    // copy otherwise (e.g. /tmp on tmpfs vs the install prefix).
    if std::fs::rename(new_bin, exe_path).is_err() {
        std::fs::copy(new_bin, exe_path)?;
    }
    Ok(())
}

/// RAII guard that removes a temp directory on drop.
struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_works() {
        assert_eq!(parse_version("0.2.1"), (0, 2, 1));
        assert_eq!(parse_version("1.0.0"), (1, 0, 0));
        assert_eq!(parse_version("10.20.30"), (10, 20, 30));
    }

    #[test]
    fn is_newer_works() {
        assert!(is_newer("0.2.1", "0.2.2"));
        assert!(is_newer("0.2.9", "0.3.0"));
        assert!(is_newer("0.9.9", "1.0.0"));
        assert!(!is_newer("0.2.1", "0.2.1"));
        assert!(!is_newer("0.2.2", "0.2.1"));
        assert!(!is_newer("1.0.0", "0.99.99"));
    }

    #[test]
    fn platform_target_resolves_on_supported_hosts() {
        // CI runs on linux-x86_64 / macos-aarch64 / macos-x86_64 — all
        // supported, as is local dev.
        assert!(platform_target().is_some());
    }
}
