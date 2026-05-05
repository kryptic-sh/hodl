use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use hodl_wallet::storage;
use hodl_wallet::{WalletMeta, keyring as wallet_keyring};

/// ASCII-art banner. Single source of truth lives at
/// `crates/hodl-tui/src/splash/art.txt` — also embedded by
/// `hodl_tui::splash::ART`. Regenerate with:
///
/// ```sh
/// figlet -f "ANSI Regular" hodl > crates/hodl-tui/src/splash/art.txt
/// ```
const LONG_ABOUT: &str = concat!(
    "\n",
    include_str!("../../../crates/hodl-tui/src/splash/art.txt"),
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
        /// Store the vault password in the OS keyring on creation so
        /// subsequent launches skip the password prompt. Falls back to
        /// prompt if the keyring is unavailable. Note: the keyring entry
        /// is a bearer credential — do not use on shared machines.
        #[arg(long)]
        keyring: bool,
    },

    /// Restore a wallet from an existing BIP-39 mnemonic (modal TUI).
    Restore {
        /// Wallet name.
        #[arg(default_value = "default")]
        name: String,
        /// Store the vault password in the OS keyring after restore so
        /// subsequent launches skip the password prompt. Falls back to
        /// prompt if the keyring is unavailable.
        #[arg(long)]
        keyring: bool,
    },

    /// Open the lock screen for an existing wallet.
    Unlock {
        /// Wallet name.
        #[arg(default_value = "default")]
        name: String,
    },

    /// Manage the OS keyring entry for a wallet's vault password.
    Keyring {
        #[command(subcommand)]
        sub: KeyringSub,
    },
}

#[derive(Subcommand, Debug)]
enum KeyringSub {
    /// Remove the keyring entry for this wallet and disable keyring
    /// auto-unlock in its metadata. Subsequent launches will prompt
    /// for the vault password as normal.
    Remove {
        /// Wallet name.
        #[arg(default_value = "default")]
        name: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_root = match cli.data_dir.clone() {
        Some(p) => p,
        None => storage::default_data_dir()?,
    };

    init_logging(&data_root)?;

    match cli.cmd.unwrap_or(Cmd::Unlock {
        name: "default".into(),
    }) {
        Cmd::Init { name, keyring } => hodl_tui::run_create(data_root, name, keyring),
        Cmd::Restore { name, keyring } => hodl_tui::run_restore(data_root, name, keyring),
        Cmd::Unlock { name } => {
            // First-run convenience: bare `hodl` with no vault routes to
            // onboarding instead of failing with "vault not found".
            if !storage::vault_path(&data_root, &name).exists() {
                hodl_tui::run_create(data_root, name, false)
            } else {
                hodl_tui::run(data_root, name)
            }
        }
        Cmd::Keyring { sub } => match sub {
            KeyringSub::Remove { name } => {
                // Delete the OS keyring entry (idempotent — missing entry = ok).
                if let Err(e) = wallet_keyring::delete_password(&name) {
                    eprintln!("warning: could not remove keyring entry: {e}");
                }
                // Flip keyring = false in the meta file.
                match WalletMeta::load(&data_root, &name) {
                    Ok(mut meta) => {
                        meta.keyring = false;
                        if let Err(e) = meta.save(&data_root, &name) {
                            eprintln!("warning: could not update wallet meta: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("warning: could not load wallet meta: {e}");
                    }
                }
                println!("removed keyring entry for \"{name}\"");
                Ok(())
            }
        },
    }
}

/// Initialize tracing.
///
/// - Always writes to `<data_root>/hodl.log` (append, no ANSI). Sync writes
///   so a panic mid-render still leaves a usable trail for post-mortem.
/// - Also tees to stderr when **stdout is not a TTY** (piped / non-interactive
///   runs). When stdout is a TTY the TUI owns the terminal — extra stderr
///   output would corrupt the alt-screen frame, so we stay silent there and
///   the file is the only sink.
/// - Filter defaults to `info,hodl*=debug`; honor `RUST_LOG` if set.
fn init_logging(data_root: &Path) -> Result<()> {
    std::fs::create_dir_all(data_root)
        .with_context(|| format!("create data dir {}", data_root.display()))?;

    let log_path = data_root.join("hodl.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open log file {}", log_path.display()))?;

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "info,hodl=debug,hodl_tui=debug,hodl_wallet=debug,\
             hodl_chain_bitcoin=debug,hodl_chain_ethereum=debug,hodl_chain_monero=debug",
        )
    });

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_target(true);

    let stderr_layer = (!std::io::stdout().is_terminal()).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(std::io::stderr().is_terminal())
            .with_target(true)
    });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stderr_layer)
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        data_dir = %data_root.display(),
        log = %log_path.display(),
        "hodl starting"
    );
    Ok(())
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
            help.contains(hodl_tui::splash::ART),
            "long_help missing embedded splash art; got:\n{help}"
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
