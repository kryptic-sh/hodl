//! Address book TUI screen.
//!
//! `b` from the Accounts screen opens this overlay. `j/k` navigate the list,
//! `a` opens an add-contact form, `d` prompts for deletion confirmation,
//! `Enter` / `Esc` / `q` close the screen.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hjkl_form::{Field, FieldMeta, Form, FormMode, Input, TextFieldEditor};
use hjkl_picker::{PickerAction, PickerEvent, PickerLogic};
use hodl_config::{AddressBook, Contact};
use hodl_core::ChainId;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState};

/// Action emitted to the parent (app.rs) when the address book wants to close.
#[derive(Debug)]
pub enum AddressBookAction {
    Close,
    Quit,
    /// Open the contextual help overlay.
    ShowHelp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    List,
    Add,
    ConfirmDelete(usize),
    ChainPicker,
}

pub struct AddressBookState {
    book: AddressBook,
    path: PathBuf,
    table_state: TableState,
    mode: Mode,
    /// Add form fields: label, address, chain (text for now), note.
    add_form: Form,
    /// Picker overlay for chain selection (used in ChainPicker mode).
    chain_picker: Option<hjkl_picker::Picker>,
    /// Pending chain selection (string key) from the picker.
    pending_chain: Option<ChainId>,
    flash: Option<String>,
}

fn make_add_form() -> Form {
    Form::new()
        .with_field(Field::SingleLineText(TextFieldEditor::with_meta(
            FieldMeta::new("label"),
            1,
        )))
        .with_field(Field::SingleLineText(TextFieldEditor::with_meta(
            FieldMeta::new("address"),
            1,
        )))
        .with_field(Field::SingleLineText(TextFieldEditor::with_meta(
            FieldMeta::new("chain (e.g. bitcoin, ethereum, monero)"),
            1,
        )))
        .with_field(Field::SingleLineText(TextFieldEditor::with_meta(
            FieldMeta::new("note (optional)"),
            1,
        )))
}

fn field_text(form: &Form, idx: usize) -> String {
    match form.fields.get(idx) {
        Some(Field::SingleLineText(f)) => f.text(),
        _ => String::new(),
    }
}

/// Parse the freeform chain field into a ChainId.
fn parse_chain(s: &str) -> Option<ChainId> {
    match s.trim().to_lowercase().as_str() {
        "bitcoin" | "btc" => Some(ChainId::Bitcoin),
        "bitcoin-testnet" | "btc-testnet" | "tbtc" => Some(ChainId::BitcoinTestnet),
        "litecoin" | "ltc" => Some(ChainId::Litecoin),
        "dogecoin" | "doge" => Some(ChainId::Dogecoin),
        "bitcoin-cash" | "bch" => Some(ChainId::BitcoinCash),
        "navcoin" | "nav" => Some(ChainId::NavCoin),
        "ethereum" | "eth" => Some(ChainId::Ethereum),
        "bnb" | "bsc" | "bsc-mainnet" => Some(ChainId::BscMainnet),
        "monero" | "xmr" => Some(ChainId::Monero),
        _ => None,
    }
}

impl AddressBookState {
    pub fn new(book: AddressBook, path: PathBuf) -> Self {
        let mut table_state = TableState::default();
        if !book.entries.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            book,
            path,
            table_state,
            mode: Mode::List,
            add_form: make_add_form(),
            chain_picker: None,
            pending_chain: None,
            flash: None,
        }
    }

    fn move_selection(&mut self, delta: i32) {
        if self.book.entries.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(self.book.entries.len() as i32) as usize;
        self.table_state.select(Some(next));
    }

    fn try_save_contact(&mut self) {
        let label = field_text(&self.add_form, 0);
        let address = field_text(&self.add_form, 1);
        let chain_str = field_text(&self.add_form, 2);
        let note_raw = field_text(&self.add_form, 3);

        if label.trim().is_empty() {
            self.flash = Some("label is required".into());
            return;
        }
        if address.trim().is_empty() {
            self.flash = Some("address is required".into());
            return;
        }
        let chain = match parse_chain(&chain_str) {
            Some(c) => c,
            None => {
                self.flash = Some(format!(
                    "unknown chain '{}' — try: bitcoin, ethereum, monero, …",
                    chain_str.trim()
                ));
                return;
            }
        };
        let note = if note_raw.trim().is_empty() {
            None
        } else {
            Some(note_raw.trim().to_string())
        };

        self.book.entries.push(Contact {
            label: label.trim().to_string(),
            address: address.trim().to_string(),
            chain,
            note,
        });

        match self.book.save(&self.path) {
            Ok(()) => {
                self.flash = Some(format!("contact '{}' saved", label.trim()));
                self.table_state.select(Some(self.book.entries.len() - 1));
            }
            Err(e) => {
                self.book.entries.pop();
                self.flash = Some(format!("save failed: {e}"));
            }
        }

        self.add_form = make_add_form();
        self.mode = Mode::List;
    }

    fn delete_selected(&mut self, idx: usize) {
        if idx < self.book.entries.len() {
            let label = self.book.entries[idx].label.clone();
            self.book.entries.remove(idx);
            match self.book.save(&self.path) {
                Ok(()) => self.flash = Some(format!("'{label}' deleted")),
                Err(e) => {
                    self.flash = Some(format!("delete failed: {e}"));
                }
            }
            let new_sel = idx.min(self.book.entries.len().saturating_sub(1));
            if self.book.entries.is_empty() {
                self.table_state.select(None);
            } else {
                self.table_state.select(Some(new_sel));
            }
        }
        self.mode = Mode::List;
    }

    /// Keybind reference for the contextual help overlay.
    /// Mode-aware: returns different binds depending on current mode.
    pub fn help_lines(&self) -> Vec<(String, String)> {
        match self.mode {
            Mode::List => vec![
                ("j / ↓".into(), "Move selection down".into()),
                ("k / ↑".into(), "Move selection up".into()),
                ("a".into(), "Add new contact".into()),
                ("d".into(), "Delete selected contact".into()),
                ("Enter / q / Esc".into(), "Close address book".into()),
                ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
                ("?".into(), "Show this help".into()),
            ],
            Mode::Add => vec![
                ("i".into(), "Enter insert mode to edit".into()),
                ("Esc".into(), "Back to Normal / cancel add".into()),
                ("Tab / j / k".into(), "Move focus between fields".into()),
                ("Enter".into(), "Save contact".into()),
                ("Ctrl+C / Ctrl+D".into(), "Quit".into()),
                ("?".into(), "Show this help".into()),
            ],
            Mode::ConfirmDelete(_) => vec![
                ("y / Y".into(), "Confirm delete".into()),
                ("any other".into(), "Cancel delete".into()),
            ],
            Mode::ChainPicker => vec![
                ("j / ↓ / k / ↑".into(), "Navigate chains".into()),
                ("Enter".into(), "Select chain".into()),
                ("Esc".into(), "Cancel".into()),
            ],
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<AddressBookAction> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            return Some(AddressBookAction::Quit);
        }

        match self.mode {
            Mode::List => self.handle_list_key(key),
            Mode::Add => self.handle_add_key(key),
            Mode::ConfirmDelete(idx) => self.handle_confirm_delete_key(key, idx),
            Mode::ChainPicker => self.handle_chain_picker_key(key),
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Option<AddressBookAction> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1);
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1);
                None
            }
            KeyCode::Char('a') => {
                self.add_form = make_add_form();
                self.pending_chain = None;
                self.mode = Mode::Add;
                None
            }
            KeyCode::Char('d') => {
                if let Some(idx) = self.table_state.selected()
                    && idx < self.book.entries.len()
                {
                    self.mode = Mode::ConfirmDelete(idx);
                }
                None
            }
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => Some(AddressBookAction::Close),
            KeyCode::Char('?') => Some(AddressBookAction::ShowHelp),
            _ => None,
        }
    }

    fn handle_add_key(&mut self, key: KeyEvent) -> Option<AddressBookAction> {
        if key.code == KeyCode::Esc && self.add_form.mode == FormMode::Normal {
            self.add_form = make_add_form();
            self.mode = Mode::List;
            return None;
        }
        if key.code == KeyCode::Enter && self.add_form.mode == FormMode::Normal {
            self.try_save_contact();
            return None;
        }
        self.add_form.handle_input(Input::from(key));
        None
    }

    fn handle_confirm_delete_key(
        &mut self,
        key: KeyEvent,
        idx: usize,
    ) -> Option<AddressBookAction> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.delete_selected(idx);
            }
            _ => {
                self.mode = Mode::List;
            }
        }
        None
    }

    fn handle_chain_picker_key(&mut self, key: KeyEvent) -> Option<AddressBookAction> {
        if let Some(picker) = &mut self.chain_picker {
            match picker.handle_key(key) {
                PickerEvent::Cancel => {
                    self.chain_picker = None;
                    self.mode = Mode::Add;
                }
                PickerEvent::Select(PickerAction::None) | PickerEvent::None => {
                    picker.refresh();
                }
                PickerEvent::Select(_) => {
                    // Selection is captured by the picker source's `select` fn.
                    self.chain_picker = None;
                    self.mode = Mode::Add;
                }
            }
        }
        None
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, area: Rect, state: &mut AddressBookState) {
    match state.mode {
        Mode::List => draw_list(f, area, state),
        Mode::Add => draw_add_form(f, area, state),
        Mode::ConfirmDelete(idx) => {
            draw_list(f, area, state);
            draw_confirm_delete(f, area, state, idx);
        }
        Mode::ChainPicker => {
            draw_list(f, area, state);
            if let Some(picker) = &mut state.chain_picker {
                draw_chain_picker(f, area, picker);
            }
        }
    }
}

fn draw_list(f: &mut Frame, area: Rect, state: &mut AddressBookState) {
    let block = Block::default()
        .title(" hodl • Address Book ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Blue));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);

    if let Some(msg) = &state.flash {
        let p = Paragraph::new(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Yellow),
        )))
        .alignment(Alignment::Center);
        f.render_widget(p, chunks[0]);
    } else if state.book.entries.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled(
            "no contacts — press a to add one",
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center);
        f.render_widget(p, chunks[0]);
    } else {
        let header = Row::new(vec![
            ratatui::widgets::Cell::from("label")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            ratatui::widgets::Cell::from("chain")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            ratatui::widgets::Cell::from("address")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            ratatui::widgets::Cell::from("note")
                .style(Style::default().add_modifier(Modifier::BOLD)),
        ])
        .style(Style::default().fg(Color::Cyan));

        let rows: Vec<Row> = state
            .book
            .entries
            .iter()
            .map(|c| {
                Row::new(vec![
                    ratatui::widgets::Cell::from(c.label.clone()),
                    ratatui::widgets::Cell::from(c.chain.ticker()),
                    ratatui::widgets::Cell::from(c.address.clone()),
                    ratatui::widgets::Cell::from(c.note.as_deref().unwrap_or("—").to_string()),
                ])
            })
            .collect();

        let widths = [
            Constraint::Length(16),
            Constraint::Length(6),
            Constraint::Min(20),
            Constraint::Length(20),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().bg(Color::DarkGray))
            .highlight_symbol("> ");

        f.render_stateful_widget(table, chunks[0], &mut state.table_state);
    }

    let hint = Paragraph::new(Line::from(Span::styled(
        "j/k move • a add • d delete • enter/q close",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(hint, chunks[1]);
}

fn draw_add_form(f: &mut Frame, area: Rect, state: &mut AddressBookState) {
    let block = Block::default()
        .title(" hodl • Add Contact ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Green));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let field_names = ["label", "address", "chain", "note (optional)"];
    let constraints: Vec<Constraint> = field_names
        .iter()
        .map(|_| Constraint::Length(3))
        .chain(std::iter::once(Constraint::Min(1)))
        .collect();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, name) in field_names.iter().enumerate() {
        let text = field_text(&state.add_form, i);
        let focused = state.add_form.focused() == i;
        let mode_str = if focused && state.add_form.mode == FormMode::Insert {
            "-- INSERT -- "
        } else {
            ""
        };
        let para = Paragraph::new(Line::from(vec![
            Span::styled(mode_str, Style::default().fg(Color::Yellow)),
            Span::raw(text),
        ]))
        .block(
            Block::default()
                .title(format!(" {name} "))
                .borders(Borders::ALL)
                .style(if focused {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                }),
        );
        f.render_widget(para, chunks[i]);
    }

    if let Some(msg) = &state.flash {
        let p = Paragraph::new(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Yellow),
        )))
        .alignment(Alignment::Center);
        // chunks.last() is the Min(1) remainder row
        let last = *chunks.last().unwrap();
        f.render_widget(p, last);
    }

    let hint_area = chunks[field_names.len()];
    let hint = Paragraph::new(Line::from(Span::styled(
        "i to edit • esc to cancel • enter to save",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    f.render_widget(hint, hint_area);
}

fn draw_confirm_delete(f: &mut Frame, area: Rect, state: &AddressBookState, idx: usize) {
    let label = state
        .book
        .entries
        .get(idx)
        .map(|c| c.label.as_str())
        .unwrap_or("?");

    let w = area.width.min(50);
    let h = 5u16;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect::new(x, y, w, h);

    let block = Block::default()
        .title(" Confirm Delete ")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black).fg(Color::Red));
    let inner = block.inner(popup);

    f.render_widget(Clear, popup);
    f.render_widget(block, popup);

    let msg = Paragraph::new(vec![
        Line::from(Span::styled(
            format!("Delete '{label}'?"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "y to confirm • any other key to cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .alignment(Alignment::Center);
    f.render_widget(msg, inner);
}

fn draw_chain_picker(f: &mut Frame, area: Rect, picker: &mut hjkl_picker::Picker) {
    picker.refresh();
    let w = area.width.min(40);
    let h = area.height.min(14);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let overlay = Rect::new(x, y, w, h);

    let block = Block::default()
        .title(format!(" {} ", picker.title()))
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(overlay);
    f.render_widget(Clear, overlay);
    f.render_widget(block, overlay);

    let entries = picker.visible_entries();
    let selected = picker.selected;
    let rows: Vec<Line> = entries
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            if i == selected {
                Line::from(Span::styled(
                    format!("> {label}"),
                    Style::default().bg(Color::DarkGray),
                ))
            } else {
                Line::from(Span::raw(format!("  {label}")))
            }
        })
        .collect();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);

    let query = picker.query.text();
    let query_para = Paragraph::new(query)
        .block(Block::default().borders(Borders::BOTTOM))
        .style(Style::default().fg(Color::White));
    f.render_widget(query_para, chunks[0]);
    f.render_widget(Paragraph::new(rows), chunks[1]);
}

// ── Chain picker source ────────────────────────────────────────────────────

struct ChainPickerSource {
    chains: Vec<ChainId>,
}

impl PickerLogic for ChainPickerSource {
    fn title(&self) -> &str {
        "chain"
    }

    fn item_count(&self) -> usize {
        self.chains.len()
    }

    fn label(&self, idx: usize) -> String {
        self.chains
            .get(idx)
            .map(|c| format!("{} ({})", c.display_name(), c.ticker()))
            .unwrap_or_default()
    }

    fn match_text(&self, idx: usize) -> String {
        self.label(idx)
    }

    fn has_preview(&self) -> bool {
        false
    }

    fn select(&self, _idx: usize) -> PickerAction {
        PickerAction::None
    }

    fn enumerate(
        &mut self,
        _query: Option<&str>,
        _cancel: Arc<AtomicBool>,
    ) -> Option<JoinHandle<()>> {
        None
    }
}

impl ChainPickerSource {
    fn all() -> Self {
        Self {
            chains: vec![
                ChainId::Bitcoin,
                ChainId::Ethereum,
                ChainId::Monero,
                ChainId::Litecoin,
                ChainId::Dogecoin,
                ChainId::BitcoinCash,
                ChainId::BscMainnet,
                ChainId::NavCoin,
                ChainId::BitcoinTestnet,
            ],
        }
    }
}

pub fn open_chain_picker(state: &mut AddressBookState) {
    let source = ChainPickerSource::all();
    state.chain_picker = Some(hjkl_picker::Picker::new(Box::new(source)));
    state.mode = Mode::ChainPicker;
}
