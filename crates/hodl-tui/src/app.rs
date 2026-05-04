//! Top-level app state machine.
//!
//! Drives the Screen enum through its lifecycle:
//! Lock → (unlock) → Accounts → Receive / Settings.
//! Re-entering Lock from any screen is a re-lock, not a quit.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::Backend;

use hodl_config::{AddressBook, Config};
use hodl_wallet::{UnlockedWallet, Wallet};

use crate::account::{self, AccountAction, AccountState};
use crate::address_book::{self, AddressBookAction, AddressBookState};
use crate::clipboard::ClipboardHandle;
use crate::help::{HelpAction, HelpOverlay};
use crate::lock::{self, Outcome as LockOutcome};
use crate::onboarding::{self, OnboardingMode, OnboardingOutcome, OnboardingState};
use crate::receive::{self, ReceiveAction, ReceiveState};
use crate::send::{self, SendAction, SendState};
use crate::settings::{self, SettingsAction, SettingsState};

/// Fallback idle timeout when the config cannot be loaded.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

// Box large variants to keep the enum footprint smaller.
#[allow(clippy::large_enum_variant)]
enum Screen {
    Lock,
    Onboarding(Box<OnboardingState>),
    Accounts(Box<AccountState>),
    AddressBook(Box<AddressBookState>),
    Receive(ReceiveState),
    Send(Box<SendState>),
    Settings,
}

pub struct App {
    data_root: PathBuf,
    wallet: Option<Wallet>,
    unlocked: Option<UnlockedWallet>,
    config: Config,
    screen: Screen,
    /// Derived from `config.lock.idle_timeout_secs` at construction time.
    idle_timeout: Duration,
    last_activity: Instant,
    clipboard: ClipboardHandle,
    /// Contextual help overlay; drawn on top of the active screen when `Some`.
    help_overlay: Option<HelpOverlay>,
}

impl App {
    pub fn new_unlock(data_root: PathBuf, wallet_name: String) -> Result<Self> {
        let wallet = Wallet::open(&data_root, &wallet_name)?;
        let config = load_config(&data_root);
        let idle_timeout = idle_timeout_from_config(&config);
        let clipboard = ClipboardHandle::new()?;
        Ok(Self {
            data_root,
            wallet: Some(wallet),
            unlocked: None,
            config,
            screen: Screen::Lock,
            idle_timeout,
            last_activity: Instant::now(),
            clipboard,
            help_overlay: None,
        })
    }

    pub fn new_create(data_root: PathBuf, wallet_name: String) -> Result<Self> {
        let config = load_config(&data_root);
        let idle_timeout = idle_timeout_from_config(&config);
        let clipboard = ClipboardHandle::new()?;
        let ob_state = OnboardingState::new(OnboardingMode::Create, data_root.clone(), wallet_name);
        Ok(Self {
            data_root,
            wallet: None,
            unlocked: None,
            config,
            screen: Screen::Onboarding(Box::new(ob_state)),
            idle_timeout,
            last_activity: Instant::now(),
            clipboard,
            help_overlay: None,
        })
    }

    pub fn new_restore(data_root: PathBuf, wallet_name: String) -> Result<Self> {
        let config = load_config(&data_root);
        let idle_timeout = idle_timeout_from_config(&config);
        let clipboard = ClipboardHandle::new()?;
        let ob_state =
            OnboardingState::new(OnboardingMode::Restore, data_root.clone(), wallet_name);
        Ok(Self {
            data_root,
            wallet: None,
            unlocked: None,
            config,
            screen: Screen::Onboarding(Box::new(ob_state)),
            idle_timeout,
            last_activity: Instant::now(),
            clipboard,
            help_overlay: None,
        })
    }

    /// Kept for tests — accepts an explicit timeout, bypasses config.
    pub fn new_unlock_with_timeout(
        data_root: PathBuf,
        wallet_name: String,
        idle_timeout: Duration,
    ) -> Result<Self> {
        let wallet = Wallet::open(&data_root, &wallet_name)?;
        let config = load_config(&data_root);
        let clipboard = ClipboardHandle::new()?;
        Ok(Self {
            data_root,
            wallet: Some(wallet),
            unlocked: None,
            config,
            screen: Screen::Lock,
            idle_timeout,
            last_activity: Instant::now(),
            clipboard,
            help_overlay: None,
        })
    }

    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
        loop {
            match &self.screen {
                Screen::Lock => {
                    let wallet = match &self.wallet {
                        Some(w) => w,
                        None => return Ok(()),
                    };
                    match lock::event_loop(terminal, wallet, self.idle_timeout, &self.data_root)? {
                        LockOutcome::Quit => return Ok(()),
                        LockOutcome::AutoLocked => continue,
                        LockOutcome::SwitchWallet(name) => {
                            // Re-lock: discard current unlocked state, load the new vault.
                            self.unlocked = None;
                            match Wallet::open(&self.data_root, &name) {
                                Ok(w) => {
                                    self.wallet = Some(w);
                                    self.screen = Screen::Lock;
                                }
                                Err(e) => {
                                    // Fall back to original wallet; lock screen will display
                                    // nothing special, so just log and continue.
                                    tracing::warn!("switch wallet failed: {e}");
                                }
                            }
                            continue;
                        }
                        LockOutcome::Unlocked(u) => {
                            self.unlocked = Some(u);
                            let mut acc_state = self.make_accounts();
                            if let Some(unlocked) = &self.unlocked {
                                acc_state.load_accounts(unlocked);
                            }
                            self.screen = Screen::Accounts(Box::new(acc_state));
                        }
                    }
                }
                Screen::Onboarding(_) => {
                    let ob_state = match &mut self.screen {
                        Screen::Onboarding(s) => s,
                        _ => unreachable!(),
                    };
                    match onboarding::event_loop(terminal, ob_state)? {
                        OnboardingOutcome::Quit => return Ok(()),
                        OnboardingOutcome::Created(w) | OnboardingOutcome::Restored(w) => {
                            self.wallet = Some(w);
                            self.screen = Screen::Lock;
                        }
                    }
                }
                Screen::Accounts(_) => {
                    if self.last_activity.elapsed() >= self.idle_timeout {
                        self.do_lock();
                        continue;
                    }

                    if !event::poll(Duration::from_millis(250))? {
                        terminal.draw(|f| {
                            let area = f.area();
                            if let Screen::Accounts(s) = &mut self.screen {
                                account::draw(f, area, s);
                            }
                            if let Some(ref mut overlay) = self.help_overlay {
                                overlay.draw(f, area);
                            }
                        })?;
                        continue;
                    }

                    let ev = event::read()?;
                    if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Press) {
                        self.last_activity = Instant::now();
                    }

                    // Overlay absorbs all keys when open.
                    if let Some(ref mut overlay) = self.help_overlay {
                        if let Event::Key(k) = ev
                            && k.kind == KeyEventKind::Press
                            && overlay.handle_key(k) == HelpAction::Close
                        {
                            self.help_overlay = None;
                        }
                        terminal.draw(|f| {
                            let area = f.area();
                            if let Screen::Accounts(s) = &mut self.screen {
                                account::draw(f, area, s);
                            }
                            if let Some(ref mut overlay) = self.help_overlay {
                                overlay.draw(f, area);
                            }
                        })?;
                        continue;
                    }

                    // Mouse scroll on the accounts table: one row per event,
                    // regardless of the OS-level scroll delta. Each
                    // ScrollUp/ScrollDown crossterm event maps to exactly one
                    // move_selection call so wheel speed cannot jump multiple rows.
                    if let (Event::Mouse(m), Screen::Accounts(s)) = (&ev, &mut self.screen) {
                        match m.kind {
                            MouseEventKind::ScrollUp => s.move_selection(-1),
                            MouseEventKind::ScrollDown => s.move_selection(1),
                            _ => {}
                        }
                    }

                    let action = match &mut self.screen {
                        Screen::Accounts(s) => {
                            if let Event::Key(k) = ev {
                                if k.kind == KeyEventKind::Press {
                                    s.handle_key(k)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };

                    match action {
                        Some(AccountAction::Lock) => self.do_lock(),
                        Some(AccountAction::Quit) => return Ok(()),
                        Some(AccountAction::ChainSwitched) => {
                            // Re-load accounts against the new chain. The picker
                            // already updated `current_chain` on the AccountState.
                            if let (Screen::Accounts(s), Some(unlocked)) =
                                (&mut self.screen, &self.unlocked)
                            {
                                s.load_accounts(unlocked);
                            }
                        }
                        Some(AccountAction::OpenAddressBook) => {
                            let book_path = AddressBook::default_path()
                                .unwrap_or_else(|_| self.data_root.join("address_book.toml"));
                            let book = AddressBook::load(&book_path).unwrap_or_default();
                            let ab_state = AddressBookState::new(book, book_path);
                            self.screen = Screen::AddressBook(Box::new(ab_state));
                        }
                        Some(AccountAction::OpenReceive(addr)) => {
                            let path = "m/84'/0'/0'/0/0".to_string();
                            self.screen = Screen::Receive(ReceiveState::new(addr, path));
                        }
                        Some(AccountAction::OpenSend {
                            chain,
                            account,
                            total_balance_sats,
                        }) => {
                            let send_state = SendState::new(
                                chain,
                                account,
                                total_balance_sats,
                                self.config.clone(),
                            );
                            self.screen = Screen::Send(Box::new(send_state));
                        }
                        Some(AccountAction::OpenSettings) => {
                            self.screen = Screen::Settings;
                        }
                        Some(AccountAction::ShowHelp) => {
                            if let Screen::Accounts(s) = &self.screen {
                                self.help_overlay =
                                    Some(HelpOverlay::new("Accounts", s.help_lines()));
                            }
                        }
                        None => {
                            terminal.draw(|f| {
                                let area = f.area();
                                if let Screen::Accounts(s) = &mut self.screen {
                                    account::draw(f, area, s);
                                }
                            })?;
                        }
                    }
                }
                Screen::AddressBook(_) => {
                    if !event::poll(Duration::from_millis(250))? {
                        terminal.draw(|f| {
                            let area = f.area();
                            if let Screen::AddressBook(s) = &mut self.screen {
                                address_book::draw(f, area, s);
                            }
                            if let Some(ref mut overlay) = self.help_overlay {
                                overlay.draw(f, area);
                            }
                        })?;
                        continue;
                    }

                    let ev = event::read()?;
                    if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Press) {
                        self.last_activity = Instant::now();
                    }

                    // Overlay absorbs all keys when open.
                    if let Some(ref mut overlay) = self.help_overlay {
                        if let Event::Key(k) = ev
                            && k.kind == KeyEventKind::Press
                            && overlay.handle_key(k) == HelpAction::Close
                        {
                            self.help_overlay = None;
                        }
                        terminal.draw(|f| {
                            let area = f.area();
                            if let Screen::AddressBook(s) = &mut self.screen {
                                address_book::draw(f, area, s);
                            }
                            if let Some(ref mut overlay) = self.help_overlay {
                                overlay.draw(f, area);
                            }
                        })?;
                        continue;
                    }

                    // Mouse scroll on the address book list: one row per event.
                    if let (Event::Mouse(m), Screen::AddressBook(s)) = (&ev, &mut self.screen) {
                        match m.kind {
                            MouseEventKind::ScrollUp => s.move_selection(-1),
                            MouseEventKind::ScrollDown => s.move_selection(1),
                            _ => {}
                        }
                    }

                    let action = match &mut self.screen {
                        Screen::AddressBook(s) => {
                            if let Event::Key(k) = ev {
                                if k.kind == KeyEventKind::Press {
                                    s.handle_key(k)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };

                    match action {
                        Some(AddressBookAction::Close) => {
                            let mut acc_state = self.make_accounts();
                            if let Some(unlocked) = &self.unlocked {
                                acc_state.load_accounts(unlocked);
                            }
                            self.screen = Screen::Accounts(Box::new(acc_state));
                        }
                        Some(AddressBookAction::Quit) => return Ok(()),
                        Some(AddressBookAction::ShowHelp) => {
                            if let Screen::AddressBook(s) = &self.screen {
                                self.help_overlay =
                                    Some(HelpOverlay::new("Address Book", s.help_lines()));
                            }
                        }
                        None => {
                            terminal.draw(|f| {
                                let area = f.area();
                                if let Screen::AddressBook(s) = &mut self.screen {
                                    address_book::draw(f, area, s);
                                }
                            })?;
                        }
                    }
                }
                Screen::Receive(_) => {
                    if !event::poll(Duration::from_millis(250))? {
                        terminal.draw(|f| {
                            let area = f.area();
                            if let Screen::Receive(s) = &mut self.screen {
                                receive::draw(f, area, s);
                            }
                            if let Some(ref mut overlay) = self.help_overlay {
                                overlay.draw(f, area);
                            }
                        })?;
                        continue;
                    }

                    let ev = event::read()?;
                    if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Press) {
                        self.last_activity = Instant::now();
                    }

                    // Overlay absorbs all keys when open.
                    if let Some(ref mut overlay) = self.help_overlay {
                        if let Event::Key(k) = ev
                            && k.kind == KeyEventKind::Press
                            && overlay.handle_key(k) == HelpAction::Close
                        {
                            self.help_overlay = None;
                        }
                        terminal.draw(|f| {
                            let area = f.area();
                            if let Screen::Receive(s) = &mut self.screen {
                                receive::draw(f, area, s);
                            }
                            if let Some(ref mut overlay) = self.help_overlay {
                                overlay.draw(f, area);
                            }
                        })?;
                        continue;
                    }

                    let action = match &mut self.screen {
                        Screen::Receive(s) => {
                            if let Event::Key(k) = ev {
                                if k.kind == KeyEventKind::Press {
                                    s.handle_key(k, &self.clipboard)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };

                    match action {
                        Some(ReceiveAction::Back) => {
                            let mut acc_state = self.make_accounts();
                            if let Some(unlocked) = &self.unlocked {
                                acc_state.load_accounts(unlocked);
                            }
                            self.screen = Screen::Accounts(Box::new(acc_state));
                        }
                        Some(ReceiveAction::Quit) => return Ok(()),
                        Some(ReceiveAction::ShowHelp) => {
                            if let Screen::Receive(s) = &self.screen {
                                self.help_overlay =
                                    Some(HelpOverlay::new("Receive", s.help_lines()));
                            }
                        }
                        None => {
                            terminal.draw(|f| {
                                let area = f.area();
                                if let Screen::Receive(s) = &mut self.screen {
                                    receive::draw(f, area, s);
                                }
                            })?;
                        }
                    }
                }
                Screen::Send(_) => {
                    let unlocked = match &self.unlocked {
                        Some(u) => u,
                        None => {
                            self.do_lock();
                            continue;
                        }
                    };
                    let send_state = match &mut self.screen {
                        Screen::Send(s) => s,
                        _ => unreachable!(),
                    };
                    match send::event_loop(terminal, send_state, unlocked)? {
                        SendAction::Back | SendAction::Quit => {
                            let mut acc_state = self.make_accounts();
                            if let Some(unlocked) = &self.unlocked {
                                acc_state.load_accounts(unlocked);
                            }
                            self.screen = Screen::Accounts(Box::new(acc_state));
                        }
                    }
                }
                Screen::Settings => {
                    let cfg_path = Config::default_path()
                        .unwrap_or_else(|_| self.data_root.join("config.toml"));
                    let mut settings_state = SettingsState::new(&self.config, cfg_path);

                    match settings::event_loop(terminal, &mut settings_state, &self.config)? {
                        SettingsAction::Saved(new_cfg) => {
                            self.config = new_cfg;
                            // Re-derive the idle timeout from the updated config.
                            self.idle_timeout = idle_timeout_from_config(&self.config);
                            let mut acc_state = self.make_accounts();
                            if let Some(unlocked) = &self.unlocked {
                                acc_state.load_accounts(unlocked);
                            }
                            self.screen = Screen::Accounts(Box::new(acc_state));
                        }
                        SettingsAction::Back => {
                            let mut acc_state = self.make_accounts();
                            if let Some(unlocked) = &self.unlocked {
                                acc_state.load_accounts(unlocked);
                            }
                            self.screen = Screen::Accounts(Box::new(acc_state));
                        }
                        SettingsAction::Quit => return Ok(()),
                    }
                }
            }
        }
    }

    fn make_accounts(&self) -> AccountState {
        AccountState::new(self.data_root.clone(), self.config.clone())
    }

    fn do_lock(&mut self) {
        self.unlocked = None;
        self.screen = Screen::Lock;
        self.last_activity = Instant::now();
        self.help_overlay = None;
    }
}

fn load_config(data_root: &Path) -> Config {
    let path = Config::default_path().unwrap_or_else(|_| data_root.join("config.toml"));
    Config::load(&path).unwrap_or_default()
}

/// Derive idle timeout from config. Falls back to `DEFAULT_IDLE_TIMEOUT`.
fn idle_timeout_from_config(config: &Config) -> Duration {
    let secs = config.lock.idle_timeout_secs;
    if secs == 0 {
        DEFAULT_IDLE_TIMEOUT
    } else {
        Duration::from_secs(secs)
    }
}
