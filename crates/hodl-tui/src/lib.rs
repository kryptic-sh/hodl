//! Ratatui-based terminal UI for hodl.
//!
//! M2 surfaces: lock screen, onboarding (create + restore), accounts list,
//! receive (QR + OSC-52 clipboard yank), settings — all driven by
//! `hjkl-form` + `hjkl-picker` + `hjkl-clipboard`.

pub mod account;
pub mod app;
pub mod clipboard;
pub mod lock;
pub mod onboarding;
pub mod receive;
pub mod settings;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub use app::DEFAULT_IDLE_TIMEOUT;

/// Run the lock-screen TUI against an existing vault.
///
/// If the vault exists, starts at the lock screen. Transitions to the
/// account screen on successful unlock.
pub fn run(data_root: PathBuf, wallet_name: String) -> Result<()> {
    run_with_timeout(data_root, wallet_name, DEFAULT_IDLE_TIMEOUT)
}

/// Same as [`run`] but with a configurable idle timeout.
pub fn run_with_timeout(
    data_root: PathBuf,
    wallet_name: String,
    idle_timeout: Duration,
) -> Result<()> {
    let mut app = app::App::new_unlock(data_root, wallet_name, idle_timeout)?;
    with_terminal(|terminal| app.run(terminal))
}

/// Run the create-wallet onboarding TUI, then drop into the lock screen.
pub fn run_create(data_root: PathBuf, wallet_name: String) -> Result<()> {
    let mut app = app::App::new_create(data_root, wallet_name, DEFAULT_IDLE_TIMEOUT)?;
    with_terminal(|terminal| app.run(terminal))
}

/// Run the restore-wallet onboarding TUI, then drop into the lock screen.
pub fn run_restore(data_root: PathBuf, wallet_name: String) -> Result<()> {
    let mut app = app::App::new_restore(data_root, wallet_name, DEFAULT_IDLE_TIMEOUT)?;
    with_terminal(|terminal| app.run(terminal))
}

/// Stub kept for the bin's existing call shape.
pub fn run_default() -> Result<()> {
    eprintln!("hodl: no wallet name provided; try `hodl init` or `hodl unlock <name>`");
    Ok(())
}

fn with_terminal<F>(f: F) -> Result<()>
where
    F: FnOnce(&mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()>,
{
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = f(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
