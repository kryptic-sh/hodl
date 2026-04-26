//! Ratatui-based terminal UI for hodl.
//!
//! M1 surface: a lock screen that prompts for the vault password, calls
//! `hodl_wallet::Wallet::unlock`, and on success transitions to a placeholder
//! "unlocked" screen. Auto-locks after a configurable idle timeout (default
//! 5 minutes).

pub mod lock;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use hodl_wallet::Wallet;

/// Default idle auto-lock timeout.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Run the lock-screen TUI against an existing vault.
pub fn run(data_root: PathBuf, wallet_name: String) -> Result<()> {
    run_with_timeout(data_root, wallet_name, DEFAULT_IDLE_TIMEOUT)
}

/// Same as [`run`] but with a configurable idle timeout.
pub fn run_with_timeout(
    data_root: PathBuf,
    wallet_name: String,
    idle_timeout: Duration,
) -> Result<()> {
    let wallet = Wallet::open(&data_root, &wallet_name)?;

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = lock::event_loop(&mut terminal, &wallet, idle_timeout);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result.map(|_| ())
}

/// Stub kept for the bin's existing call shape: prints a hint.
pub fn run_default() -> Result<()> {
    eprintln!("hodl: no wallet name provided; try `hodl init` or `hodl unlock <name>`");
    Ok(())
}
