//! Top-level app state machine.
//!
//! Drives the Screen enum through its lifecycle:
//! Lock → (unlock) → Accounts → Receive / Settings.
//! Re-entering Lock from any screen is a re-lock, not a quit.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::Backend;

use hodl_config::{AddressBook, Config, KnownHosts};
use hodl_core::ChainId;
use hodl_wallet::{UnlockedWallet, Wallet};

use crate::account::{self, AccountAction, AccountState};
use crate::address_book::{self, AddressBookAction, AddressBookState};
use crate::addresses::{self, AddressesAction, AddressesState};
use crate::clipboard::ClipboardHandle;
use crate::help::{HelpAction, HelpOverlay};
use crate::lock::{self, Outcome as LockOutcome};
use crate::onboarding::{self, OnboardingMode, OnboardingOutcome, OnboardingState};
use crate::receive::{self, ReceiveAction, ReceiveState};
use crate::scan_cache::ScanCache;
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
    Addresses(Box<AddressesState>),
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
    /// TOFU cert pin store shared across all scan and send threads.
    known_hosts: Arc<Mutex<KnownHosts>>,
    /// Encrypted, per-wallet on-disk cache of `WalletScan` per chain.
    /// `Some` only while a wallet is unlocked — the cache key is derived
    /// from the unlocked seed so re-locking drops both the in-memory
    /// hashmap and the key (zeroized via `ScanCache::Drop`).
    scan_cache: Option<ScanCache>,
    /// Stashed AccountState while the Addresses sub-view is open.
    /// Preserves the cached WalletScan so re-entering Accounts does not
    /// trigger a fresh network round-trip.
    accounts_stash: Option<Box<AccountState>>,
}

impl App {
    pub fn new_unlock(data_root: PathBuf, wallet_name: String) -> Result<Self> {
        let wallet = Wallet::open(&data_root, &wallet_name)?;
        let config = load_config(&data_root);
        let idle_timeout = idle_timeout_from_config(&config);
        let clipboard = ClipboardHandle::new()?;
        let known_hosts = load_known_hosts(&data_root);
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
            known_hosts,
            scan_cache: None,
            accounts_stash: None,
        })
    }

    pub fn new_create(data_root: PathBuf, wallet_name: String) -> Result<Self> {
        let config = load_config(&data_root);
        let idle_timeout = idle_timeout_from_config(&config);
        let clipboard = ClipboardHandle::new()?;
        let known_hosts = load_known_hosts(&data_root);
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
            known_hosts,
            scan_cache: None,
            accounts_stash: None,
        })
    }

    pub fn new_restore(data_root: PathBuf, wallet_name: String) -> Result<Self> {
        let config = load_config(&data_root);
        let idle_timeout = idle_timeout_from_config(&config);
        let clipboard = ClipboardHandle::new()?;
        let known_hosts = load_known_hosts(&data_root);
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
            known_hosts,
            scan_cache: None,
            accounts_stash: None,
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
        let known_hosts = load_known_hosts(&data_root);
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
            known_hosts,
            scan_cache: None,
            accounts_stash: None,
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
                            self.scan_cache = None;
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
                            // Build the per-wallet scan cache from the freshly-derived
                            // cache key so subsequent `start_load` calls can prime
                            // the summary card from disk before the network roundtrip.
                            if let (Some(unlocked), Some(wallet)) = (&self.unlocked, &self.wallet) {
                                let key = unlocked.cache_key();
                                self.scan_cache =
                                    Some(ScanCache::open(&self.data_root, &wallet.name, key));
                            }
                            // Build the Accounts screen and immediately kick off the
                            // background load so the animated spinner appears at once.
                            let mut acc_state = self.make_accounts();
                            if let Some(unlocked) = &self.unlocked {
                                let cached = self
                                    .scan_cache
                                    .as_ref()
                                    .and_then(|c| c.get(acc_state.current_chain));
                                acc_state.start_load(unlocked, cached);
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
                    // While a background load is in flight, reset `last_activity`
                    // so the idle timeout cannot fire mid-load.
                    if let Screen::Accounts(s) = &self.screen
                        && s.is_scanning()
                    {
                        self.last_activity = Instant::now();
                    }

                    if self.last_activity.elapsed() >= self.idle_timeout {
                        self.do_lock();
                        continue;
                    }

                    // Poll the background load channel before waiting for events.
                    // If data just arrived we redraw immediately; if still empty
                    // we fall through to event::poll with the short timeout.
                    if let Screen::Accounts(s) = &mut self.screen
                        && s.poll_scan()
                    {
                        // If the scan just completed (Done), persist the fresh
                        // snapshot to the on-disk cache. `take()` so the same
                        // scan isn't rewritten on every subsequent poll.
                        if let Some(scan) = s.completed_scan.take()
                            && let Some(cache) = self.scan_cache.as_mut()
                        {
                            cache.put(s.current_chain, scan);
                        }
                        // State changed (rows arrived or error) — redraw.
                        terminal.draw(|f| {
                            let area = f.area();
                            account::draw(f, area, s);
                        })?;
                        continue;
                    }

                    // Use a short timeout while loading so the spinner animates
                    // smoothly; fall back to 250 ms when idle.
                    let loading = matches!(&self.screen, Screen::Accounts(s) if s.is_scanning());
                    let wait = if loading {
                        Duration::from_millis(80)
                    } else {
                        Duration::from_millis(250)
                    };

                    if !event::poll(wait)? {
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
                    if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Press)
                        || matches!(&ev, Event::Mouse(_))
                    {
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

                    let action = match &mut self.screen {
                        Screen::Accounts(s) => {
                            if let Event::Key(k) = ev {
                                if k.kind == KeyEventKind::Press {
                                    // handle_key blocks scan-dependent
                                    // actions while is_scanning() is true.
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
                            // Prime from cache (if any) so the new chain card is
                            // populated immediately while the background scan runs.
                            if let Screen::Accounts(s) = &mut self.screen
                                && let Some(unlocked) = &self.unlocked
                            {
                                let cached = self
                                    .scan_cache
                                    .as_ref()
                                    .and_then(|c| c.get(s.current_chain));
                                s.start_load(unlocked, cached);
                            }
                        }
                        Some(AccountAction::Resync) => {
                            // Force fresh scan — bypass the cache prime so the
                            // user sees the spinner clearly. The on-disk blob
                            // is overwritten on `ScanEvent::Done` below.
                            if let Screen::Accounts(s) = &mut self.screen
                                && let Some(unlocked) = &self.unlocked
                            {
                                s.start_load(unlocked, None);
                            }
                        }
                        Some(AccountAction::OpenAddressBook) => {
                            let book_path = AddressBook::default_path()
                                .unwrap_or_else(|_| self.data_root.join("address_book.toml"));
                            let book = AddressBook::load(&book_path).unwrap_or_default();
                            let ab_state = AddressBookState::new(book, book_path);
                            self.screen = Screen::AddressBook(Box::new(ab_state));
                        }
                        Some(AccountAction::OpenReceive) => {
                            // Resolve the best receive address from the scan
                            // (first used receive, or derive 0 as fallback).
                            if let (Screen::Accounts(s), Some(unlocked)) =
                                (&self.screen, &self.unlocked)
                                && let Some((addr, path)) = s.pick_receive(unlocked)
                            {
                                self.screen = Screen::Receive(ReceiveState::new(addr, path));
                            }
                        }
                        Some(AccountAction::OpenAddresses) => {
                            // Take the AccountState out of the screen so we
                            // can stash it. The screen temporarily holds a
                            // sentinel Lock value while we build Addresses.
                            let old_screen = std::mem::replace(&mut self.screen, Screen::Lock);
                            if let Screen::Accounts(acc_state) = old_screen {
                                let chain = acc_state.current_chain;
                                let scan = acc_state.scan.clone();
                                self.accounts_stash = Some(acc_state);

                                match scan {
                                    None => {
                                        tracing::debug!(
                                            "OpenAddresses fired with no scan — ignoring"
                                        );
                                        if let Some(stashed) = self.accounts_stash.take() {
                                            self.screen = Screen::Accounts(stashed);
                                        }
                                    }
                                    Some(scan) => {
                                        // Compute paths up front — no network.
                                        // BIP-44 family path: m/{purpose}'/{coin}'/{account}'/{change}/{index}
                                        // BTC family purpose comes from the chain's default_send_purpose;
                                        // EVM and Monero are pinned at BIP-44.
                                        let coin = chain.slip44();
                                        let purpose: u32 = match chain {
                                            ChainId::Ethereum
                                            | ChainId::BscMainnet
                                            | ChainId::Monero => 44,
                                            _ => hodl_chain_bitcoin::BitcoinChain::default_send_purpose(chain).number(),
                                        };
                                        let paths: Vec<String> = scan
                                            .used
                                            .iter()
                                            .map(|u| {
                                                format!(
                                                    "m/{purpose}'/{coin}'/0'/{}/{}",
                                                    u.change, u.index
                                                )
                                            })
                                            .collect();
                                        let addr_state = AddressesState::new(&scan, chain, &paths);
                                        self.screen = Screen::Addresses(Box::new(addr_state));
                                    }
                                }
                            }
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
                                Arc::clone(&self.known_hosts),
                                self.data_root.clone(),
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
                    if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Press)
                        || matches!(&ev, Event::Mouse(_))
                    {
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
                            self.enter_accounts();
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
                Screen::Addresses(_) => {
                    if !event::poll(Duration::from_millis(250))? {
                        terminal.draw(|f| {
                            let area = f.area();
                            if let Screen::Addresses(s) = &mut self.screen {
                                addresses::draw(f, area, s);
                            }
                            if let Some(ref mut overlay) = self.help_overlay {
                                overlay.draw(f, area);
                            }
                        })?;
                        continue;
                    }

                    let ev = event::read()?;
                    if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Press)
                        || matches!(&ev, Event::Mouse(_))
                    {
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
                            if let Screen::Addresses(s) = &mut self.screen {
                                addresses::draw(f, area, s);
                            }
                            if let Some(ref mut overlay) = self.help_overlay {
                                overlay.draw(f, area);
                            }
                        })?;
                        continue;
                    }

                    // Mouse scroll: one row per event.
                    if let (Event::Mouse(m), Screen::Addresses(s)) = (&ev, &mut self.screen) {
                        match m.kind {
                            MouseEventKind::ScrollUp => {
                                s.move_selection(-1);
                                self.last_activity = Instant::now();
                            }
                            MouseEventKind::ScrollDown => {
                                s.move_selection(1);
                                self.last_activity = Instant::now();
                            }
                            _ => {}
                        }
                    }

                    let action = match &mut self.screen {
                        Screen::Addresses(s) => {
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
                        Some(AddressesAction::Close) => {
                            // Restore the stashed AccountState so the cached
                            // WalletScan is preserved and no re-scan is needed.
                            if let Some(stashed) = self.accounts_stash.take() {
                                self.screen = Screen::Accounts(stashed);
                            } else {
                                // Fallback: rebuild and re-scan if the stash was
                                // somehow lost.
                                self.enter_accounts();
                            }
                        }
                        Some(AddressesAction::Quit) => return Ok(()),
                        Some(AddressesAction::ShowHelp) => {
                            if let Screen::Addresses(s) = &self.screen {
                                self.help_overlay =
                                    Some(HelpOverlay::new("Addresses", s.help_lines()));
                            }
                        }
                        None => {
                            terminal.draw(|f| {
                                let area = f.area();
                                if let Screen::Addresses(s) = &mut self.screen {
                                    addresses::draw(f, area, s);
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
                    if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Press)
                        || matches!(&ev, Event::Mouse(_))
                    {
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
                            self.enter_accounts();
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
                            self.enter_accounts();
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
                            self.enter_accounts();
                        }
                        SettingsAction::Back => {
                            self.enter_accounts();
                        }
                        SettingsAction::Quit => return Ok(()),
                    }
                }
            }
        }
    }

    fn make_accounts(&self) -> AccountState {
        AccountState::new(
            self.data_root.clone(),
            self.config.clone(),
            Arc::clone(&self.known_hosts),
        )
    }

    /// Build a fresh `AccountState`, prime it from the on-disk scan cache
    /// (if a snapshot exists for the default chain), kick off the background
    /// resync, and switch the screen to it. Used by every "back to accounts"
    /// transition so they all share the same cache-prime + resync behaviour.
    fn enter_accounts(&mut self) {
        let mut acc_state = self.make_accounts();
        if let Some(unlocked) = &self.unlocked {
            let cached = self
                .scan_cache
                .as_ref()
                .and_then(|c| c.get(acc_state.current_chain));
            acc_state.start_load(unlocked, cached);
        }
        self.screen = Screen::Accounts(Box::new(acc_state));
    }

    fn do_lock(&mut self) {
        self.unlocked = None;
        // Drop the per-wallet cache. `ScanCache::Drop` zeroizes the
        // cache key — the on-disk blobs remain (they re-decrypt on
        // the next unlock).
        self.scan_cache = None;
        self.screen = Screen::Lock;
        self.last_activity = Instant::now();
        self.help_overlay = None;
    }
}

fn load_config(data_root: &Path) -> Config {
    let path = Config::default_path().unwrap_or_else(|_| data_root.join("config.toml"));
    Config::load(&path).unwrap_or_default()
}

/// Load the TOFU known-hosts store from `<data_root>/known_hosts.toml`.
/// Returns an empty default if the file is missing. Never writes to disk.
fn load_known_hosts(data_root: &Path) -> Arc<Mutex<KnownHosts>> {
    let kh = KnownHosts::load(data_root).unwrap_or_default();
    Arc::new(Mutex::new(kh))
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
