use super::WebServerBackend;
use crate::brew::Brew;
use crate::config::Config;
use crate::daemon::ServiceSpec;
use crate::state::{Backend, Server, State, Vhost};
use anyhow::{bail, Result};

/// OpenLiteSpeed.
///
/// Status on macOS: **blocked upstream.** OLS has no official macOS build, and
/// the only community Homebrew tap (`puleeno/openlitespeed`) force-builds its
/// `admin_php` WebAdmin dependency — a PHP formula disabled since 2022 that
/// fails to compile against the macOS 26 (Tahoe) SDK (`configure: fatal: cannot
/// find libraries needed for sockets`). Verified broken 2026-06-15 on Tahoe/arm64
/// after fixing the tap's missing `pkg-config` dependency.
///
/// The backend is wired into the trait so it activates automatically if a
/// working OLS install appears (e.g. a fixed tap, a Linux host, or a Docker/VM
/// OLS). Until then `ensure_installed` will surface brew's build error.
pub struct Ols;

impl WebServerBackend for Ols {
    fn id(&self) -> Backend {
        Backend::Ols
    }

    fn formula(&self) -> &'static str {
        "openlitespeed"
    }

    fn ensure_installed(&self, brew: &Brew) -> Result<()> {
        if brew.is_installed(self.formula()) {
            return Ok(());
        }
        // The OLS tap builds from source and needs pkg-config; newer Homebrew
        // also requires explicitly trusting the third-party tap.
        if !brew.is_installed("pkgconf") && !brew.is_installed("pkg-config") {
            brew.install("pkg-config")?;
        }
        brew.ensure_tap("puleeno/openlitespeed")?;
        brew.trust_tap("puleeno/openlitespeed");
        brew.install(self.formula())?;
        Ok(())
    }

    fn render(
        &self,
        _server: &Server,
        _vhosts: &[&Vhost],
        _state: &State,
        _cfg: &Config,
        _brew: &Brew,
    ) -> Result<()> {
        bail!("{}", UNSUPPORTED)
    }

    fn validate(&self, _server: &Server, _brew: &Brew) -> Result<()> {
        bail!("{}", UNSUPPORTED)
    }

    fn reload(&self, _server: &Server, _brew: &Brew) -> Result<()> {
        bail!("{}", UNSUPPORTED)
    }

    fn service_spec(&self, _server: &Server, _brew: &Brew) -> Result<ServiceSpec> {
        bail!("{}", UNSUPPORTED)
    }
}

const UNSUPPORTED: &str = "OpenLiteSpeed isn't available on macOS: the only Homebrew tap \
(puleeno/openlitespeed) fails to build its admin_php dependency against the Tahoe SDK. \
Use caddy, apache, or nginx. See backends/ols.rs for details.";
