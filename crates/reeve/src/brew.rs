//! Homebrew orchestration. reeve does not bundle servers or PHP — it
//! drives brew-installed binaries. This module detects the brew prefix and
//! provides thin query/install helpers.

use anyhow::{bail, Context, Result};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Brew {
    pub prefix: PathBuf,
}

impl Brew {
    /// Detect the active Homebrew install. Checks the canonical Apple-silicon /
    /// Intel locations by absolute path first — `which brew` is unreliable under
    /// non-login shells (SSH, launchd) where `.zprofile` hasn't run — then falls
    /// back to `brew --prefix` for unusual custom prefixes.
    pub fn detect() -> Result<Self> {
        for candidate in ["/opt/homebrew", "/usr/local"] {
            let p = Path::new(candidate);
            if p.join("bin/brew").exists() {
                return Ok(Self {
                    prefix: p.to_path_buf(),
                });
            }
        }
        if let Ok(out) = Command::new("brew").arg("--prefix").output() {
            if out.status.success() {
                let prefix = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !prefix.is_empty() && Path::new(&prefix).join("bin/brew").exists() {
                    return Ok(Self {
                        prefix: PathBuf::from(prefix),
                    });
                }
            }
        }
        bail!("Homebrew not found")
    }

    /// Detect Homebrew, or — when running interactively — offer to install it.
    /// Returns an error (with guidance) if absent and not installed.
    pub fn detect_or_offer_install() -> Result<Self> {
        match Self::detect() {
            Ok(brew) => Ok(brew),
            Err(_) => {
                eprintln!("Homebrew is required but was not found.");
                if confirm("Install Homebrew now (runs the official installer)?")? {
                    install_homebrew()?;
                    Self::detect().context(
                        "Homebrew install finished but it still can't be located. \
                         Open a new terminal and re-run `reeve init`.",
                    )
                } else {
                    bail!("Install Homebrew from https://brew.sh, then re-run `reeve init`.")
                }
            }
        }
    }

    /// The absolute path to the `brew` binary for this install. Always use this
    /// rather than a bare `brew` so we work without a configured PATH.
    pub fn brew_bin(&self) -> PathBuf {
        self.prefix.join("bin/brew")
    }

    /// `<prefix>/etc/...`
    pub fn etc(&self, sub: &str) -> PathBuf {
        self.prefix.join("etc").join(sub)
    }

    /// `<prefix>/opt/<formula>` — the stable, version-pinned install path.
    pub fn opt(&self, formula: &str) -> PathBuf {
        self.prefix.join("opt").join(formula)
    }

    /// `<prefix>/bin/<name>`
    pub fn bin(&self, name: &str) -> PathBuf {
        self.prefix.join("bin").join(name)
    }

    /// Is a formula installed? Uses the `opt/<formula>` symlink, which is cheap
    /// and avoids spawning `brew list`.
    pub fn is_installed(&self, formula: &str) -> bool {
        self.opt(formula).exists()
    }

    /// Run `brew install <formula>`, streaming output to the user's terminal.
    pub fn install(&self, formula: &str) -> Result<()> {
        let status = Command::new(self.brew_bin())
            .arg("install")
            .arg(formula)
            .status()
            .with_context(|| format!("Failed to spawn `brew install {formula}`"))?;
        if !status.success() {
            bail!("`brew install {formula}` failed");
        }
        Ok(())
    }

    /// Ensure a tap is present (`brew tap <tap>`).
    pub fn ensure_tap(&self, tap: &str) -> Result<()> {
        let status = Command::new(self.brew_bin())
            .arg("tap")
            .arg(tap)
            .status()
            .with_context(|| format!("Failed to spawn `brew tap {tap}`"))?;
        if !status.success() {
            bail!("`brew tap {tap}` failed");
        }
        Ok(())
    }

    /// Trust a third-party tap (`brew trust <tap>`). Newer Homebrew refuses to
    /// load formulae from untrusted taps without this. Best-effort: older
    /// Homebrew has no `trust` subcommand, so a failure here is not fatal.
    pub fn trust_tap(&self, tap: &str) {
        let _ = Command::new(self.brew_bin()).arg("trust").arg(tap).status();
    }
}

/// Prompt the user for a yes/no answer on the terminal. Defaults to "no" when
/// stdin is not a TTY (e.g. scripted/CI runs), so we never block headless.
fn confirm(question: &str) -> Result<bool> {
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 0 {
        return Ok(false);
    }
    print!("{question} [y/N] ");
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Run the official Homebrew installer.
fn install_homebrew() -> Result<()> {
    let installer = r#"/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)""#;
    let status = Command::new("/bin/bash")
        .arg("-c")
        .arg(installer)
        .status()
        .context("Failed to run the Homebrew installer")?;
    if !status.success() {
        bail!("Homebrew installation failed");
    }
    Ok(())
}
