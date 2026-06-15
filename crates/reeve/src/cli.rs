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
    /// Manage PHP extensions for a version.
    #[command(subcommand)]
    Ext(ExtCommands),
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
        #[arg(long)]
        php: String,
        #[arg(long)]
        server: String,
        #[arg(long)]
        ssl: bool,
    },
    /// List vhosts.
    List,
    /// Remove a vhost.
    Remove { server_name: String },
}

#[derive(Subcommand, Debug)]
pub enum SslCommands {
    /// Mint a certificate for a hostname from the shared local CA.
    Mint { server_name: String },
    /// Install the mkcert local CA into the system trust store (may prompt for
    /// admin authorization). Run once so browsers trust local certs.
    Trust,
    /// Print the local CA root certificate path.
    Ca,
}
