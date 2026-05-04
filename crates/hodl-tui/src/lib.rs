//! Ratatui-based terminal UI for hodl.
//!
//! M2 surfaces: lock screen, onboarding (create + restore), accounts list,
//! receive (QR + OSC-52 clipboard yank), settings — all driven by
//! `hjkl-form` + `hjkl-picker` + `hjkl-clipboard`.

pub mod account;
pub mod active_chain;
pub mod address_book;
pub mod app;
pub mod clipboard;
pub mod help;
pub mod lock;
pub mod onboarding;
pub mod receive;
pub mod send;
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
/// Idle timeout is read from `Config.lock.idle_timeout_secs` at startup.
pub fn run(data_root: PathBuf, wallet_name: String) -> Result<()> {
    let mut app = app::App::new_unlock(data_root, wallet_name)?;
    with_terminal(|terminal| app.run(terminal))
}

/// Same as [`run`] but overrides the idle timeout (used in tests).
pub fn run_with_timeout(
    data_root: PathBuf,
    wallet_name: String,
    idle_timeout: Duration,
) -> Result<()> {
    let mut app = app::App::new_unlock_with_timeout(data_root, wallet_name, idle_timeout)?;
    with_terminal(|terminal| app.run(terminal))
}

/// Run the create-wallet onboarding TUI, then drop into the lock screen.
pub fn run_create(data_root: PathBuf, wallet_name: String) -> Result<()> {
    let mut app = app::App::new_create(data_root, wallet_name)?;
    with_terminal(|terminal| app.run(terminal))
}

/// Run the restore-wallet onboarding TUI, then drop into the lock screen.
pub fn run_restore(data_root: PathBuf, wallet_name: String) -> Result<()> {
    let mut app = app::App::new_restore(data_root, wallet_name)?;
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
    // Wipe the alt-screen buffer before the first draw — some terminals
    // (kitty, wezterm) don't fully clear it on EnterAlternateScreen, and
    // ratatui's diff renderer only repaints changed cells, so prior shell
    // scrollback bleeds through any blank widget background.
    terminal.clear()?;

    let result = f(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
