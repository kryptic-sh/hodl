//! Crossterm key events in, hjkl input out.
//!
//! hodl reads the terminal through crossterm, while hjkl's `-tui` crates take
//! ttyr's key type. The forms and pickers only need hjkl's engine-agnostic
//! [`Input`] and the [`Picker`] methods, so the conversion happens here, at
//! hodl's own boundary, instead of through `hjkl-engine-tui` and
//! `hjkl-picker-tui`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hjkl_form::{Field, Form, Input, Key};
use hjkl_picker::{Picker, PickerEvent};

/// `'formatoptions'` for every text field: continue comments, never wrap typed
/// text. hjkl's default gained `t` in 0.45, which breaks a line at
/// `'textwidth'` as it is typed. A single-line field keeps only its first line,
/// so a password or label typed past the margin with a space in it was cut
/// there without a word.
const FIELD_FORMATOPTIONS: &str = "ro";

/// Feed one key to a form.
pub fn form_key(form: &mut Form, key: KeyEvent) {
    // Set on every key rather than once: `TextFieldEditor::set_text` rebuilds
    // the field's editor with hjkl's default options.
    for field in &mut form.fields {
        if let Field::SingleLineText(text) | Field::MultiLineText(text) = field {
            let settings = text.editor.settings_mut();
            if settings.formatoptions != FIELD_FORMATOPTIONS {
                settings.formatoptions = FIELD_FORMATOPTIONS.to_string();
            }
        }
    }
    form.handle_input(to_input(key));
}

/// Convert a crossterm key event to hjkl's [`Input`].
///
/// Keys the forms do not model (`F(_)`, `Insert`, media keys, ...) become
/// [`Key::Null`], which the forms ignore.
fn to_input(key: KeyEvent) -> Input {
    let k = match key.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Enter => Key::Enter,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Tab => Key::Tab,
        KeyCode::Esc => Key::Esc,
        _ => Key::Null,
    };
    Input {
        key: k,
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
    }
}

/// Drive a picker with one key.
///
/// `Esc` / `C-c` cancel, `Enter` accepts, `Down` / `C-n` and `Up` / `C-p` move
/// the selection. Any other key goes to the picker's source first and, when the
/// source does not claim it, to the query field.
pub fn picker_key(picker: &mut Picker, key: KeyEvent) -> PickerEvent {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => return picker.cancel(),
        KeyCode::Char('c') if ctrl => return picker.cancel(),
        KeyCode::Enter => return picker.accept(),
        KeyCode::Down => {
            picker.select_next();
            return PickerEvent::None;
        }
        KeyCode::Up => {
            picker.select_prev();
            return PickerEvent::None;
        }
        KeyCode::Char('n') if ctrl => {
            picker.select_next();
            return PickerEvent::None;
        }
        KeyCode::Char('p') if ctrl => {
            picker.select_prev();
            return PickerEvent::None;
        }
        _ => {}
    }

    let input = to_input(key);
    if let Some(action) = picker.handle_source_key(input) {
        return PickerEvent::Select(action);
    }
    picker.handle_query_input(input);
    PickerEvent::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn char_keeps_its_modifiers() {
        let input = to_input(key(
            KeyCode::Char('W'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert_eq!(input.key, Key::Char('W'));
        assert!(input.ctrl);
        assert!(!input.alt);
        assert!(input.shift);
    }

    #[test]
    fn named_keys_map_to_their_engine_keys() {
        for (code, want) in [
            (KeyCode::Backspace, Key::Backspace),
            (KeyCode::Delete, Key::Delete),
            (KeyCode::Enter, Key::Enter),
            (KeyCode::Left, Key::Left),
            (KeyCode::Right, Key::Right),
            (KeyCode::Up, Key::Up),
            (KeyCode::Down, Key::Down),
            (KeyCode::Home, Key::Home),
            (KeyCode::End, Key::End),
            (KeyCode::Tab, Key::Tab),
            (KeyCode::Esc, Key::Esc),
        ] {
            assert_eq!(to_input(key(code, KeyModifiers::ALT)).key, want);
        }
    }

    /// Type `text` into a fresh form holding one text field, and read it back.
    fn type_into_field(rows: u16, text: &str) -> String {
        use hjkl_form::{FieldMeta, TextFieldEditor};

        let field = TextFieldEditor::with_meta(FieldMeta::new("field"), rows);
        let field = if rows <= 1 {
            Field::SingleLineText(field)
        } else {
            Field::MultiLineText(field)
        };
        let mut form = Form::new().with_field(field);
        form_key(&mut form, key(KeyCode::Char('i'), KeyModifiers::NONE));
        for c in text.chars() {
            form_key(&mut form, key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        form_key(&mut form, key(KeyCode::Esc, KeyModifiers::NONE));
        match &form.fields[0] {
            Field::SingleLineText(f) | Field::MultiLineText(f) => f.text(),
            _ => unreachable!("the form holds one text field"),
        }
    }

    #[test]
    fn typed_text_does_not_wrap_at_textwidth() {
        let text = "correct horse battery staple ".repeat(5);
        assert_eq!(type_into_field(1, &text), text);
        assert_eq!(type_into_field(4, &text), text);
    }

    #[test]
    fn unmodelled_keys_are_null() {
        assert_eq!(
            to_input(key(KeyCode::F(5), KeyModifiers::NONE)).key,
            Key::Null
        );
        assert_eq!(
            to_input(key(KeyCode::Insert, KeyModifiers::NONE)).key,
            Key::Null
        );
    }
}
