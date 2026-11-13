//! Y and N answer a message box that asks a yes or no question: Y gives the primary action, N
//! the closing one. A box opts in; one that confirms something risky may take N alone, so a
//! single letter can only ever say no to it.

use super::Action;
use egui::{Key, Modifiers};

/// Which letter keys answer a box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnswerKeys {
    /// Neither letter (the default: forms and boxes with more than two answers).
    #[default]
    Off,
    /// Y says yes (when the yes answer is enabled), N says no.
    YesNo,
    /// Only N, which says no.
    NoOnly,
}

/// The answer a Y or N press gives this frame, the key used up. Nothing while a text field has
/// the keyboard: there the letter is typed.
pub fn answer_key(ctx: &egui::Context, keys: AnswerKeys, yes_enabled: bool) -> Action {
    if keys == AnswerKeys::Off || ctx.text_edit_focused() {
        return Action::None;
    }
    if keys == AnswerKeys::YesNo && yes_enabled && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Y)) {
        Action::Ok
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::N)) {
        Action::Cancel
    } else {
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key, mods: Modifiers, keys: AnswerKeys, yes_enabled: bool, typing: bool) -> Action {
        let ctx = egui::Context::default();
        let event = egui::Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: mods };
        let input = egui::RawInput { events: vec![event], ..Default::default() };
        let mut got = Action::None;
        let field = egui::Id::unique("answer_keys_field");
        if typing {
            // A text field that has the keyboard (its state marks it as a text edit).
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let mut text = String::new();
                let r = ui.add(egui::TextEdit::singleline(&mut text).id(field));
                r.request_focus();
            });
            out.textures_delta.clear();
        }
        let mut out = ctx.run_ui(input, |ui| {
            if typing {
                let mut text = String::new();
                ui.add(egui::TextEdit::singleline(&mut text).id(field));
            }
            got = answer_key(ui.ctx(), keys, yes_enabled);
        });
        out.textures_delta.clear();
        got
    }

    #[test]
    fn y_says_yes_and_n_says_no_only_when_the_box_asks_for_them() {
        assert_eq!(press(Key::Y, Modifiers::NONE, AnswerKeys::YesNo, true, false), Action::Ok);
        assert_eq!(press(Key::N, Modifiers::NONE, AnswerKeys::YesNo, true, false), Action::Cancel);
        assert_eq!(press(Key::Y, Modifiers::NONE, AnswerKeys::YesNo, false, false), Action::None, "a disabled yes");
        assert_eq!(press(Key::Y, Modifiers::NONE, AnswerKeys::NoOnly, true, false), Action::None, "Y never says yes to a risky box");
        assert_eq!(press(Key::N, Modifiers::NONE, AnswerKeys::NoOnly, true, false), Action::Cancel);
        assert_eq!(press(Key::Y, Modifiers::NONE, AnswerKeys::Off, true, false), Action::None);
        assert_eq!(press(Key::N, Modifiers::NONE, AnswerKeys::Off, true, false), Action::None);
        assert_eq!(press(Key::Y, Modifiers::COMMAND, AnswerKeys::YesNo, true, false), Action::None, "Ctrl+Y is another key");
        assert_eq!(press(Key::Z, Modifiers::NONE, AnswerKeys::YesNo, true, false), Action::None);
    }

    #[test]
    fn letters_typed_into_a_field_are_not_answers() {
        assert_eq!(press(Key::Y, Modifiers::NONE, AnswerKeys::YesNo, true, true), Action::None);
        assert_eq!(press(Key::N, Modifiers::NONE, AnswerKeys::YesNo, true, true), Action::None);
    }
}
