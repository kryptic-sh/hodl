use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use hodl_wallet::storage;

/// ASCII-art banner. Regenerate with:
///
/// ```sh
/// figlet -f "ANSI Regular" hodl > apps/hodl/src/art.txt
/// ```
const LONG_ABOUT: &str = concat!(
    "\n",
    include_str!("art.txt"),
    "\nLight crypto wallet — TUI · v",
    env!("CARGO_PKG_VERSION"),
);

#[derive(Parser, Debug)]
#[command(
    name = "hodl",
    version,
    about = "Light crypto wallet — TUI",
    long_about = LONG_ABOUT,
)]
struct Cli {
    /// Override the data directory (defaults to `$XDG_DATA_HOME/hodl`).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Create a new wallet vault (modal TUI onboarding).
    Init {
        /// Wallet name (vault file is `<name>.vault`).
        #[arg(default_value = "default")]
        name: String,
    },

    /// Restore a wallet from an existing BIP-39 mnemonic (modal TUI).
    Restore {
        /// Wallet name.
        #[arg(default_value = "default")]
        name: String,
    },

    /// Open the lock screen for an existing wallet.
    Unlock {
        /// Wallet name.
        #[arg(default_value = "default")]
        name: String,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let data_root = match cli.data_dir.clone() {
        Some(p) => p,
        None => storage::default_data_dir()?,
    };

    match cli.cmd.unwrap_or(Cmd::Unlock {
        name: "default".into(),
    }) {
        Cmd::Init { name } => hodl_tui::run_create(data_root, name),
        Cmd::Restore { name } => hodl_tui::run_restore(data_root, name),
        Cmd::Unlock { name } => hodl_tui::run(data_root, name),
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn version_flag_returns_pkg_version() {
        let cmd = Cli::command();
        let version = cmd.render_version();
        assert!(
            version.contains(env!("CARGO_PKG_VERSION")),
            "render_version output {version:?} missing CARGO_PKG_VERSION"
        );
    }

    #[test]
    fn long_help_contains_ascii_art() {
        let mut cmd = Cli::command();
        let help = cmd.render_long_help().to_string();
        assert!(
            help.contains(include_str!("art.txt")),
            "long_help missing embedded art.txt block; got:\n{help}"
        );
    }

    #[test]
    fn long_help_contains_pkg_version() {
        let mut cmd = Cli::command();
        let help = cmd.render_long_help().to_string();
        assert!(
            help.contains(env!("CARGO_PKG_VERSION")),
            "long_help missing CARGO_PKG_VERSION; got:\n{help}"
        );
    }
}
