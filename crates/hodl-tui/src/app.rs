//! Top-level app state machine.
//!
//! Drives the Screen enum through its lifecycle:
//! Lock → (unlock) → Accounts → Receive / Settings.
//! Re-entering Lock from any screen is a re-lock, not a quit.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::Backend;

use hodl_config::Config;
use hodl_wallet::{UnlockedWallet, Wallet};

use crate::account::{self, AccountAction, AccountState};
use crate::clipboard::ClipboardHandle;
use crate::lock::{self, Outcome as LockOutcome};
use crate::onboarding::{self, OnboardingMode, OnboardingOutcome, OnboardingState};
use crate::receive::{self, ReceiveAction, ReceiveState};
use crate::send::{self, SendAction, SendState};
use crate::settings::{self, SettingsAction, SettingsState};

pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

// Box large variants to keep the enum footprint smaller.
#[allow(clippy::large_enum_variant)]
enum Screen {
    Lock,
    Onboarding(Box<OnboardingState>),
    Accounts(Box<AccountState>),
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
    idle_timeout: Duration,
    last_activity: Instant,
    clipboard: ClipboardHandle,
}

impl App {
    pub fn new_unlock(
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
        })
    }

    pub fn new_create(
        data_root: PathBuf,
        wallet_name: String,
        idle_timeout: Duration,
    ) -> Result<Self> {
        let config = load_config(&data_root);
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
        })
    }

    pub fn new_restore(
        data_root: PathBuf,
        wallet_name: String,
        idle_timeout: Duration,
    ) -> Result<Self> {
        let config = load_config(&data_root);
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
                    match lock::event_loop(terminal, wallet, self.idle_timeout)? {
                        LockOutcome::Quit => return Ok(()),
                        LockOutcome::AutoLocked => continue,
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
                            if let Screen::Accounts(s) = &mut self.screen {
                                account::draw(f, f.area(), s);
                            }
                        })?;
                        continue;
                    }

                    let ev = event::read()?;
                    if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Press) {
                        self.last_activity = Instant::now();
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
                        Some(AccountAction::OpenReceive(addr)) => {
                            let path = "m/84'/0'/0'/0/0".to_string();
                            self.screen = Screen::Receive(ReceiveState::new(addr, path));
                        }
                        Some(AccountAction::OpenSend {
                            address,
                            account,
                            change_branch,
                            index,
                            balance_sats,
                        }) => {
                            let send_state = SendState::new(
                                address,
                                account,
                                change_branch,
                                index,
                                balance_sats,
                                self.config.clone(),
                            );
                            self.screen = Screen::Send(Box::new(send_state));
                        }
                        Some(AccountAction::OpenSettings) => {
                            self.screen = Screen::Settings;
                        }
                        None => {
                            terminal.draw(|f| {
                                if let Screen::Accounts(s) = &mut self.screen {
                                    account::draw(f, f.area(), s);
                                }
                            })?;
                        }
                    }
                }
                Screen::Receive(_) => {
                    if !event::poll(Duration::from_millis(250))? {
                        terminal.draw(|f| {
                            if let Screen::Receive(s) = &mut self.screen {
                                receive::draw(f, f.area(), s);
                            }
                        })?;
                        continue;
                    }

                    let ev = event::read()?;
                    if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Press) {
                        self.last_activity = Instant::now();
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
                        None => {
                            terminal.draw(|f| {
                                if let Screen::Receive(s) = &mut self.screen {
                                    receive::draw(f, f.area(), s);
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
    }
}

fn load_config(data_root: &Path) -> Config {
    let path = Config::default_path().unwrap_or_else(|_| data_root.join("config.toml"));
    Config::load(&path).unwrap_or_default()
}
