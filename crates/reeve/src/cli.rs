//! Command-line surface. With no subcommand, the TUI dashboard opens
//! (mirrors the ytunnel convention).

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "reeve",
    version,
    about = "Localhost web dev stack manager — web servers, per-vhost PHP, SSL, DNS.",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Detect Homebrew, scaffold config/state, set up local CA + DNS.
    Init,

    /// Manage PHP versions and their FPM masters.
    #[command(subcommand)]
    Php(PhpCommands),

    /// Manage web server instances.
    #[command(subcommand)]
    Server(ServerCommands),

    /// Manage virtual hosts.
    #[command(subcommand)]
    Vhost(VhostCommands),

    /// Manage background services (databases, redis, memcached, mailpit).
    #[command(subcommand)]
    Service(ServiceCommands),

    /// Park a directory so every subfolder auto-serves as <folder>.<tld>.
    #[command(subcommand)]
    Park(ParkCommands),

    /// Render generated configs and reconcile running services to state.
    Apply,

    /// Run every backend's native config test.
    Validate,

    /// Manage local SSL certificates.
    #[command(subcommand)]
    Ssl(SslCommands),

    /// Manage local wildcard DNS for the dev TLD (default .test).
    #[command(subcommand)]
    Dns(DnsCommands),

    /// View a managed service's log. Omit the target to list available logs.
    Logs {
        /// Target: server-<name>, php-<ver>, dnsmasq (or a bare server name).
        target: Option<String>,
        /// Number of trailing lines to show.
        #[arg(long, short = 'n', default_value_t = 50)]
        lines: usize,
        /// Stream new lines as they arrive (like `tail -f`).
        #[arg(long, short)]
        follow: bool,
    },

    /// Open the dashboard straight into the live traffic monitor.
    Traffic,

    /// Diagnose the stack: Homebrew, servers, FPM, DNS, certs, ports.
    Doctor,

    /// Self-update reeve to the latest GitHub release.
    Update {
        /// Only report whether an update is available; don't install.
        #[arg(long)]
        check: bool,
    },

    /// (hidden) Render one TUI frame to text — for testing without a terminal.
    #[command(hide = true)]
    TuiSnapshot {
        #[arg(long, default_value_t = 102)]
        width: u16,
        #[arg(long, default_value_t = 26)]
        height: u16,
        /// Open a modal for the snapshot: "wizard" (new vhost) or "server" (edit).
        #[arg(long, default_value = "")]
        modal: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum DnsCommands {
    /// Install dnsmasq, resolve *.<tld> to 127.0.0.1, and wire up the resolver.
    Setup,
    /// Show DNS service + resolver status.
    Status,
}

#[derive(Subcommand, Debug)]
pub enum PhpCommands {
    /// Install a PHP version (e.g. `8.3`) and stand up its FPM master.
    Install { version: String },
    /// List installed PHP versions and FPM status.
    List,
    /// Set the default PHP version for new vhosts.
    Use { version: String },
    /// Switch the CLI `php` (via the ~/.reeve/bin shim). Omit the version to
    /// show the current one.
    Cli { version: Option<String> },
    /// Manage PHP extensions for a version.
    #[command(subcommand)]
    Ext(ExtCommands),
    /// Show effective php.ini / OPcache / FPM settings for a version.
    Settings { version: String },
    /// Set a php.ini / OPcache / FPM setting (see `php settings <ver>` for keys).
    Set {
        version: String,
        key: String,
        value: String,
    },
    /// Toggle Xdebug for a version: off | debug | profile.
    Xdebug { version: String, mode: String },
}

#[derive(Subcommand, Debug)]
pub enum ExtCommands {
    /// Enable/install an extension for a PHP version.
    Add { version: String, name: String },
    /// Disable an extension for a PHP version.
    Remove { version: String, name: String },
    /// List extensions for a PHP version.
    List { version: String },
}

#[derive(Subcommand, Debug)]
pub enum ServerCommands {
    /// Add a server instance.
    Add {
        /// Backend: caddy|apache|nginx|ols
        backend: String,
        /// Instance name (defaults to the backend name).
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = 80)]
        http: u16,
        #[arg(long, default_value_t = 443)]
        https: u16,
        /// Serve a catch-all default site (sites root) on the HTTP port.
        #[arg(long)]
        default_site: bool,
        /// Docroot for the default site. Defaults to the global sites root.
        #[arg(long)]
        root: Option<String>,
        /// Framework preset for the default site: generic|laravel|wordpress|symfony|grav|drupal.
        #[arg(long, default_value = "generic")]
        preset: String,
    },
    /// Start a server.
    Start { name: String },
    /// Stop a server.
    Stop { name: String },
    /// Restart a server (picks up config changes).
    Restart { name: String },
    /// List servers and their status.
    List,
    /// Remove a server.
    Remove { name: String },
}

#[derive(Subcommand, Debug)]
pub enum VhostCommands {
    /// Add a vhost.
    Add {
        /// Hostname, e.g. grav.test
        server_name: String,
        #[arg(long)]
        root: String,
        /// PHP version (not needed for a `--proxy` vhost).
        #[arg(long, default_value = "")]
        php: String,
        #[arg(long)]
        server: String,
        #[arg(long)]
        ssl: bool,
        /// Framework preset: generic|laravel|wordpress|symfony|grav|drupal.
        #[arg(long, default_value = "generic")]
        preset: String,
        /// Make this a reverse proxy to an upstream URL (e.g. http://localhost:5173).
        /// Mutually exclusive with PHP serving.
        #[arg(long)]
        proxy: Option<String>,
    },
    /// List vhosts.
    List,
    /// Remove a vhost.
    Remove { server_name: String },
}

#[derive(Subcommand, Debug)]
pub enum ParkCommands {
    /// Park a directory: subfolders auto-serve as <folder>.<tld>.
    Add {
        /// Directory to park (e.g. ~/Sites).
        dir: String,
        #[arg(long)]
        server: String,
        #[arg(long)]
        php: String,
        /// TLD for parked hosts (defaults to your first configured TLD).
        #[arg(long)]
        tld: Option<String>,
        #[arg(long)]
        ssl: bool,
    },
    /// Stop parking a directory.
    Remove { dir: String },
    /// List parked directories and the sites they currently expand to.
    List,
}

#[derive(Subcommand, Debug)]
pub enum ServiceCommands {
    /// Add a service (mysql|mariadb|postgres|redis|memcached|mailpit) to state.
    Add { kind: String },
    /// Install if needed, then start a service.
    Start { kind: String },
    /// Stop a service.
    Stop { kind: String },
    /// Restart a service.
    Restart { kind: String },
    /// List services and their status.
    List,
    /// Remove a service from state (leaves the brew formula installed).
    Remove { kind: String },
}

#[derive(Subcommand, Debug)]
pub enum SslCommands {
    /// Mint a certificate for a hostname from the shared local CA.
    Mint { server_name: String },
    /// Install the mkcert local CA into the system trust store (may prompt for
    /// admin authorization). Run once so browsers trust local certs.
    Trust,
    /// Remove the mkcert local CA from the trust store (browsers stop trusting
    /// local certs). The CA files stay on disk for a later `trust`.
    Untrust,
    /// Show whether the local CA is generated and trusted, and where it lives.
    Status,
    /// Print the local CA root certificate path.
    Ca,
}
