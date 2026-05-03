use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use zeroize::Zeroize;

use hodl_wallet::Wallet;
use hodl_wallet::mnemonic::{self, WordCount};
use hodl_wallet::storage;
use hodl_wallet::vault::KdfParams;

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
    /// Create a new wallet vault interactively.
    Init {
        /// Wallet name (vault file is `<name>.vault`).
        #[arg(default_value = "default")]
        name: String,

        /// Number of words: 12 or 24.
        #[arg(long, default_value_t = 12)]
        words: u8,
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
        Cmd::Init { name, words } => init_cmd(&data_root, &name, words),
        Cmd::Unlock { name } => hodl_tui::run(data_root, name),
    }
}

fn init_cmd(data_root: &std::path::Path, name: &str, words: u8) -> Result<()> {
    let count = match words {
        12 => WordCount::Twelve,
        24 => WordCount::TwentyFour,
        n => return Err(anyhow!("--words must be 12 or 24 (got {n})")),
    };

    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return Err(anyhow!("`hodl init` is interactive; run it from a TTY"));
    }
    let mut stdin = stdin.lock();
    let mut stdout = std::io::stdout();

    let mnem = mnemonic::generate(count)?;
    println!(
        "\nGenerated {}-word mnemonic. WRITE THIS DOWN — it is the only backup:\n",
        count.words()
    );
    println!("    {}\n", mnem);

    print!("Optional BIP-39 passphrase (25th word) — leave empty for none: ");
    stdout.flush()?;
    let mut passphrase = String::new();
    stdin.read_line(&mut passphrase)?;
    let passphrase = passphrase.trim_end_matches(['\n', '\r']).to_string();

    let mut password = read_password_twice(&mut stdin, &mut stdout)?;

    let phrase = mnem.to_string();
    let _wallet = Wallet::create(
        data_root,
        name,
        &phrase,
        &passphrase,
        password.as_bytes(),
        KdfParams::default(),
    )
    .context("failed to create vault")?;
    password.zeroize();

    println!(
        "\nVault written to {}",
        storage::vault_path(data_root, name).display()
    );
    println!("Run `hodl unlock {name}` to open the lock screen.");
    Ok(())
}

fn read_password_twice<R: BufRead, W: Write>(stdin: &mut R, stdout: &mut W) -> Result<String> {
    loop {
        write!(stdout, "Set vault password: ")?;
        stdout.flush()?;
        let mut a = String::new();
        stdin.read_line(&mut a)?;
        let a = a.trim_end_matches(['\n', '\r']).to_string();
        if a.is_empty() {
            writeln!(stdout, "  password cannot be empty")?;
            continue;
        }

        write!(stdout, "Confirm password:     ")?;
        stdout.flush()?;
        let mut b = String::new();
        stdin.read_line(&mut b)?;
        let mut b = b.trim_end_matches(['\n', '\r']).to_string();
        if a != b {
            writeln!(stdout, "  passwords do not match — try again")?;
            b.zeroize();
            continue;
        }
        b.zeroize();
        return Ok(a);
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
        // The full embedded ASCII art block must appear verbatim in the
        // long-form help.
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
