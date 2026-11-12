//! Keyboard shortcuts: the defaults come from the command table, the user's changes are kept in
//! the preferences as overrides (key slot to key text, empty for "no key"). Every command has two
//! slots: its first key (slot id = command id, the one menus and tooltips show) and a second key
//! (slot id = command id + [`SECOND`]), so F2 starts a job as well as Alt+S. Conflicts are
//! detected when a key is assigned. A keymap travels as a `.dbhk` file.
//!
//! Number pad [KEY-03]: egui 0.37.0 (egui-winit 0.37.0 on winit 0.30.13) gives a key press the
//! logical key and the physical key, and maps both `KeyCode::DigitN` and `KeyCode::NumpadN` to
//! `Key::NumN`; winit's `KeyEvent::location` (which says "numpad") is dropped by egui-winit.
//! With NumLock on, numpad 8 and the 8 above the letters therefore arrive as the same event and
//! cannot be told apart (eframe 0.37 has no hook before that conversion except a raw Windows
//! message hook, which needs unsafe code). With NumLock off the number pad sends arrows, Home,
//! PgUp, End and PgDn as the logical key and still `NumN` as the physical key, which no other key
//! does: those presses are what a `Numpad8` binding matches. Only the key each pad digit stands
//! for with NumLock off counts (8 is ArrowUp, 7 Home, 0 Insert): a digit-row key that types
//! punctuation (Shift+1 is `!`, the AZERTY 6 is `-`) also has a digit as its physical key.
//!
//! A key the user gave a command beats a default key: when a newer version adds a default (F2
//! for Start) that the user already uses for something else, the default is left out.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use egui::{Key, KeyboardShortcut, Modifiers};
use serde::{Deserialize, Serialize};

use crate::commands::{self, Cmd};
use crate::i18n::tr;

/// The user's changes to the default keys.
pub type Overrides = BTreeMap<String, String>;

pub const FILE_FORMAT: &str = "darkburn-hotkeys";
pub const FILE_EXTENSION: &str = "dbhk";
/// Added to a command id for its second key.
pub const SECOND: &str = "#2";
/// Key text of a number pad key pressed with NumLock off ("Numpad8").
pub const NUMPAD: &str = "Numpad";

thread_local! {
    // One snapshot per thread that draws an app (see `options::get`).
    // The overrides and, once asked for, the effective keys they give.
    static CURRENT: RefCell<(Overrides, Option<BTreeMap<&'static str, String>>)> = const { RefCell::new((Overrides::new(), None)) };
}

/// Refresh the snapshot the menus and the ribbon tooltips read.
pub fn publish(o: &Overrides) {
    CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != *o {
            *c = (o.clone(), None);
        }
    });
}

/// The key text now bound to command `c` (its default unless changed). Menus and tooltips show
/// the first key only.
pub fn key_for(c: &Cmd) -> String {
    CURRENT.with(|cur| {
        let mut cur = cur.borrow_mut();
        let (o, eff) = &mut *cur;
        // Commands that take arguments have no slot; they show their default.
        eff.get_or_insert_with(|| effective(o)).get(c.id).cloned().unwrap_or_else(|| key_in(o, c.id, default_key(c)))
    })
}

/// The key slot `id` has now (empty for none), as the key handler sees it.
pub fn key_of(o: &Overrides, id: &str) -> String {
    effective(o).get(id).cloned().unwrap_or_default()
}

/// The first key command `c` has now with overrides `o` (what [`key_for`] shows).
pub fn first_key(o: &Overrides, c: &Cmd) -> String {
    effective(o).get(c.id).cloned().unwrap_or_else(|| key_in(o, c.id, default_key(c)))
}

/// Keys of commands whose registry entry (in another module) has none: the laser keys that
/// LightBurn has (Pause, Ctrl+Break) and the jog keys. The arrow keys with Ctrl+Alt jog X and Y,
/// Ctrl+Alt with the keys of the number pad's corners (Home, PgUp, End, PgDn) jog diagonally, and
/// Ctrl+Alt+] and Ctrl+Alt+[ jog Z.
const EXTRA_DEFAULTS: &[(&str, &str)] = &[
    ("job.pause", "Pause"),
    ("job.stop", "Ctrl+Pause"),
    ("laser.jog_left", "Ctrl+Alt+Left"),
    ("laser.jog_right", "Ctrl+Alt+Right"),
    ("laser.jog_up", "Ctrl+Alt+Up"),
    ("laser.jog_down", "Ctrl+Alt+Down"),
    ("laser.jog_up_left", "Ctrl+Alt+Home"),
    ("laser.jog_up_right", "Ctrl+Alt+PgUp"),
    ("laser.jog_down_left", "Ctrl+Alt+End"),
    ("laser.jog_down_right", "Ctrl+Alt+PgDn"),
    ("laser.jog_z_up", "Ctrl+Alt+]"),
    ("laser.jog_z_down", "Ctrl+Alt+["),
];

/// Default second keys: F2 starts the job [KEY-01], and the number pad (NumLock off) jogs the
/// way its keys point [KEY-03]. 5 has nothing; + and - are zoom.
const SECOND_DEFAULTS: &[(&str, &str)] = &[
    ("job.start", "F2"),
    ("laser.jog_up", "Numpad8"),
    ("laser.jog_down", "Numpad2"),
    ("laser.jog_left", "Numpad4"),
    ("laser.jog_right", "Numpad6"),
    ("laser.jog_up_left", "Numpad7"),
    ("laser.jog_up_right", "Numpad9"),
    ("laser.jog_down_left", "Numpad1"),
    ("laser.jog_down_right", "Numpad3"),
];

/// The default key of a command.
pub fn default_key(c: &Cmd) -> &'static str {
    if !c.key.is_empty() {
        return c.key;
    }
    EXTRA_DEFAULTS.iter().find(|(id, _)| *id == c.id).map_or("", |(_, k)| k)
}

/// The default second key of command `id`.
pub fn second_default(id: &str) -> &'static str {
    SECOND_DEFAULTS.iter().find(|(i, _)| *i == id).map_or("", |(_, k)| k)
}

/// The key text for `id` given overrides and the default.
pub fn key_in(o: &Overrides, id: &str, default: &str) -> String {
    o.get(id).cloned().unwrap_or_else(|| default.to_string())
}

/// Every command that can have a key: id, label, default key.
pub fn bindable() -> Vec<(&'static str, &'static str, &'static str)> {
    commands::registry().iter().filter(|c| c.args.is_empty()).map(|c| (c.id, c.label, default_key(c))).collect()
}

/// A place for one key of a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// The command id for the first key, the command id + [`SECOND`] for the second.
    pub id: &'static str,
    pub cmd: &'static str,
    pub default: &'static str,
}

/// Both slots of every bindable command, first then second.
pub fn slots() -> &'static [Slot] {
    static ALL: OnceLock<Vec<Slot>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v = Vec::new();
        for (id, _, default) in bindable() {
            v.push(Slot { id, cmd: id, default });
            // Made once per command for the life of the program.
            let second: &'static str = Box::leak(format!("{id}{SECOND}").into_boxed_str());
            v.push(Slot { id: second, cmd: id, default: second_default(id) });
        }
        v
    })
}

/// The slot with id `id`.
pub fn slot(id: &str) -> Option<Slot> {
    slots().iter().find(|s| s.id == id).copied()
}

/// The slot id of command `cmd`'s second key.
pub fn second_of(cmd: &str) -> String {
    format!("{cmd}{SECOND}")
}

/// The command a slot id belongs to.
pub fn command_of(id: &str) -> &str {
    id.strip_suffix(SECOND).unwrap_or(id)
}

/// What a key text means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    /// A key with its modifiers, matched on the logical key.
    Keys(KeyboardShortcut),
    /// A number pad digit pressed with NumLock off (the physical key; see the module notes).
    Numpad(Key),
}

/// Parse a key text: what the command table writes, or `Numpad0` to `Numpad9` without
/// `Numpad5` (which cannot be told from the digit 5, see [`PAD_KEYS`]).
pub fn parse_binding(text: &str) -> Option<Binding> {
    if let Some(digit) = text.strip_prefix(NUMPAD) {
        return Some(digit)
            .filter(|d| d.len() == 1 && d.as_bytes()[0].is_ascii_digit())
            .and_then(Key::from_name)
            .filter(|k| PAD_KEYS.iter().any(|(d, _)| d == k))
            .map(Binding::Numpad);
    }
    commands::parse_key(text).map(Binding::Keys)
}

/// What each number pad digit sends with NumLock off. Pad 5 sends a key egui does not know, so
/// egui reports it as the digit and it cannot be told from the 5 above the letters.
const PAD_KEYS: [(Key, Key); 9] = [
    (Key::Num0, Key::Insert),
    (Key::Num1, Key::End),
    (Key::Num2, Key::ArrowDown),
    (Key::Num3, Key::PageDown),
    (Key::Num4, Key::ArrowLeft),
    (Key::Num6, Key::ArrowRight),
    (Key::Num7, Key::Home),
    (Key::Num8, Key::ArrowUp),
    (Key::Num9, Key::PageUp),
];

/// The physical number pad key of a key event, when the logical key shows NumLock is off: the
/// key that pad digit sends then (8 is ArrowUp, 7 Home). Any other pair is not a pad key, also
/// when the physical key is a digit: Shift+1 (`!`) or the AZERTY 6 (`-`) on the digit row.
pub fn numpad_key(key: Key, physical: Option<Key>) -> Option<Key> {
    let physical = physical?;
    PAD_KEYS.iter().any(|&(digit, sends)| digit == physical && sends == key).then_some(physical)
}

/// The key text of a number pad key ([`numpad_key`]).
pub fn numpad_text(key: Key) -> String {
    format!("{NUMPAD}{}", key.name())
}

/// The effective key of each slot: every first key (empty when it has none) and the second keys
/// that are set or were changed. A default key that the user gave another slot is left out, so
/// a key set up before a version added that default keeps doing what the user chose.
pub fn effective(o: &Overrides) -> BTreeMap<&'static str, String> {
    let chosen: Vec<Binding> = o.iter().filter(|(id, k)| !k.is_empty() && slot(id).is_some()).filter_map(|(_, k)| parse_binding(k)).collect();
    slots()
        .iter()
        .filter_map(|s| {
            let key = match o.get(s.id) {
                Some(k) => k.clone(),
                None if !chosen.is_empty() && parse_binding(s.default).is_some_and(|b| chosen.contains(&b)) => String::new(),
                None => s.default.to_string(),
            };
            (s.id == s.cmd || !key.is_empty() || o.contains_key(s.id)).then_some((s.id, key))
        })
        .collect()
}

/// Shortcut table for the key handler: commands with a key, those with more modifiers first so
/// Ctrl+Shift+Z is not taken for Ctrl+Z. Number pad keys are in [`numpad_table`].
pub fn table(o: &Overrides) -> Vec<(KeyboardShortcut, &'static str)> {
    let mut v: Vec<_> = effective(o)
        .into_iter()
        .filter_map(|(id, key)| match parse_binding(&key) {
            Some(Binding::Keys(k)) => slot(id).map(|s| (k, s.cmd)),
            _ => None,
        })
        .collect();
    let weight = |k: &KeyboardShortcut| k.modifiers.command as u8 + k.modifiers.shift as u8 + k.modifiers.alt as u8;
    v.sort_by_key(|(k, id)| (std::cmp::Reverse(weight(k)), *id));
    v
}

/// The number pad keys (NumLock off) and their commands.
pub fn numpad_table(o: &Overrides) -> Vec<(Key, &'static str)> {
    effective(o)
        .into_iter()
        .filter_map(|(id, key)| match parse_binding(&key) {
            Some(Binding::Numpad(k)) => slot(id).map(|s| (k, s.cmd)),
            _ => None,
        })
        .collect()
}

/// Keys bound to more than one slot: (key text, the slot ids).
pub fn conflicts(o: &Overrides) -> Vec<(String, Vec<&'static str>)> {
    let eff = effective(o);
    let mut by_key: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    for (id, key) in &eff {
        match parse_binding(key) {
            Some(Binding::Keys(k)) => by_key.entry(format!("{:?}", (k.modifiers, k.logical_key))).or_default().push(id),
            Some(Binding::Numpad(k)) => by_key.entry(format!("numpad {k:?}")).or_default().push(id),
            None => {}
        }
    }
    by_key.into_values().filter(|ids| ids.len() > 1).map(|ids| (eff[ids[0]].clone(), ids)).collect()
}

/// Why an assignment was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    UnknownCommand,
    BadKey,
    /// Another slot (id) already has this key.
    Taken(&'static str),
}

/// The slot that already uses `key`, ignoring `except`.
pub fn owner_of(o: &Overrides, key: &str, except: &str) -> Option<&'static str> {
    let want = parse_binding(key)?;
    effective(o).into_iter().find(|(id, k)| *id != except && parse_binding(k) == Some(want)).map(|(id, _)| id)
}

/// Give slot `id` (a command id, or a command id + [`SECOND`]) the key `key` ("" clears it).
/// Without `take`, a key another slot uses is refused; with it, the other slot loses the key.
pub fn assign(o: &mut Overrides, id: &str, key: &str, take: bool) -> Result<(), Refused> {
    let Some(s) = slot(id) else {
        return Err(Refused::UnknownCommand);
    };
    if !key.is_empty() && parse_binding(key).is_none() {
        return Err(Refused::BadKey);
    }
    if !key.is_empty()
        && let Some(other) = owner_of(o, key, id)
    {
        if !take {
            return Err(Refused::Taken(other));
        }
        o.insert(other.to_string(), String::new());
    }
    if key == s.default {
        o.remove(id);
    } else {
        o.insert(id.to_string(), key.to_string());
    }
    Ok(())
}

/// Back to the default key: a command id resets both its keys, a second-key id only that one.
pub fn reset(o: &mut Overrides, id: &str) {
    o.remove(id);
    if !id.ends_with(SECOND) {
        o.remove(&second_of(id));
    }
}

/// The text of a key press, as the command table writes it (`Ctrl+Shift+Z`, `Del`, `PgUp`).
pub fn key_text(m: Modifiers, key: Key) -> String {
    let mut s = String::new();
    if m.command {
        s += "Ctrl+";
    }
    if m.alt {
        s += "Alt+";
    }
    if m.shift {
        s += "Shift+";
    }
    s += match key {
        Key::Delete => "Del",
        Key::PageUp => "PgUp",
        Key::PageDown => "PgDn",
        Key::Period => ".",
        Key::Comma => ",",
        Key::Equals => "=",
        Key::Minus => "-",
        Key::Plus => "+",
        Key::Backtick => "`",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        other => other.name(),
    };
    s
}

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    version: u32,
    keys: BTreeMap<String, String>,
}

/// A `.dbhk` file holding the effective key of every command (and the second keys in use).
pub fn to_file(o: &Overrides) -> String {
    let keys = effective(o).into_iter().map(|(id, k)| (id.to_string(), k)).collect();
    serde_json::to_string_pretty(&File { format: FILE_FORMAT.into(), version: 1, keys }).unwrap_or_default()
}

/// The overrides a `.dbhk` file describes (only where it differs from the defaults).
pub fn from_file(text: &str) -> Result<Overrides, String> {
    let f: File = serde_json::from_str(text).map_err(|e| format!("{} ({e})", tr("hotkeys.bad_file", "This is not a hotkey file")))?;
    if f.format != FILE_FORMAT {
        return Err(tr("hotkeys.bad_file", "This is not a hotkey file"));
    }
    let mut o = Overrides::new();
    for (id, key) in f.keys {
        // Commands this version does not know are skipped, so files travel between versions.
        if let Some(s) = slot(&id) {
            if !key.is_empty() && parse_binding(&key).is_none() {
                return Err(format!("{}: {key}", tr("hotkeys.bad_key", "Not a key")));
            }
            if key != s.default {
                o.insert(id, key);
            }
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_is_not_shared_with_an_app_on_another_thread() {
        let copy = commands::lookup("edit.copy").unwrap();
        publish(&Overrides::from([("edit.copy".to_string(), "Ctrl+Alt+C".to_string())]));
        std::thread::spawn(|| publish(&Overrides::new())).join().unwrap();
        assert_eq!(key_for(copy), "Ctrl+Alt+C", "another app's keys replaced this one's");
    }

    #[test]
    fn defaults_have_no_conflicts_and_all_parse() {
        for (id, _, key) in bindable() {
            if !key.is_empty() {
                assert!(commands::parse_key(key).is_some(), "{id}: {key}");
            }
        }
        assert!(conflicts(&Overrides::new()).is_empty(), "{:?}", conflicts(&Overrides::new()));
    }

    #[test]
    fn key_text_round_trips_through_the_parser() {
        for (id, _, key) in bindable() {
            // Alt+* is Shift+8 as far as egui can tell.
            if let Some(k) = commands::parse_key(key).filter(|_| !key.ends_with('*')) {
                assert_eq!(key_text(k.modifiers, k.logical_key), key, "{id}");
            }
        }
    }

    #[test]
    fn remapping_applies_and_default_is_kept_clean() {
        let mut o = Overrides::new();
        assign(&mut o, "edit.undo", "Ctrl+Alt+U", false).unwrap();
        assert_eq!(key_in(&o, "edit.undo", "Ctrl+Z"), "Ctrl+Alt+U");
        let t = table(&o);
        assert!(t.iter().any(|(k, id)| *id == "edit.undo" && commands::parse_key("Ctrl+Alt+U") == Some(*k)));
        assert!(!t.iter().any(|(k, id)| *id == "edit.undo" && commands::parse_key("Ctrl+Z") == Some(*k)));
        assign(&mut o, "edit.undo", "Ctrl+Z", false).unwrap();
        assert!(o.is_empty(), "going back to the default drops the override");
    }

    #[test]
    fn conflicts_are_detected_and_can_be_taken() {
        let mut o = Overrides::new();
        assert_eq!(assign(&mut o, "edit.redo", "Ctrl+Z", false), Err(Refused::Taken("edit.undo")));
        assert!(o.is_empty());
        assert_eq!(assign(&mut o, "edit.redo", "Ctrl+Z", true), Ok(()));
        assert_eq!(effective(&o)["edit.undo"], "");
        assert_eq!(effective(&o)["edit.redo"], "Ctrl+Z");
        assert!(conflicts(&o).is_empty());
        // A conflict made by hand (an imported file) between two keys the user chose is reported.
        let mut bad = Overrides::new();
        bad.insert("edit.redo".into(), "Ctrl+Shift+F12".into());
        bad.insert("edit.undo".into(), "Ctrl+Shift+F12".into());
        let c = conflicts(&bad);
        assert_eq!(c.len(), 1);
        assert!(c[0].1.contains(&"edit.undo") && c[0].1.contains(&"edit.redo"));
        // A key the user chose beats another command's default: the default is left out.
        let mut mine = Overrides::new();
        mine.insert("edit.redo".into(), "Ctrl+Z".into());
        assert!(conflicts(&mine).is_empty());
        assert_eq!(effective(&mine)["edit.undo"], "");
        assert_eq!(
            table(&mine).iter().filter(|(k, _)| commands::parse_key("Ctrl+Z") == Some(*k)).map(|(_, id)| *id).collect::<Vec<_>>(),
            ["edit.redo"]
        );
        assert_eq!(assign(&mut o, "no.such", "F5", false), Err(Refused::UnknownCommand));
        assert_eq!(assign(&mut o, "edit.undo", "Ctrl+Banana", false), Err(Refused::BadKey));
    }

    #[test]
    fn f2_is_a_second_start_key_and_menus_keep_the_first() {
        let o = Overrides::new();
        let start = commands::lookup("job.start").unwrap();
        assert_eq!(effective(&o)["job.start#2"], "F2");
        let f2 = commands::parse_key("F2").unwrap();
        let alt_s = commands::parse_key("Alt+S").unwrap();
        let t = table(&o);
        assert!(t.contains(&(f2, "job.start")), "F2 runs the command itself, not a slot");
        assert!(t.contains(&(alt_s, "job.start")));
        publish(&o);
        assert_eq!(key_for(start), "Alt+S", "menus and tooltips show the first key");
    }

    #[test]
    fn second_keys_are_set_cleared_and_reset_on_their_own() {
        let mut o = Overrides::new();
        assign(&mut o, "edit.undo#2", "Ctrl+Alt+Z", false).unwrap();
        assert!(table(&o).contains(&(commands::parse_key("Ctrl+Alt+Z").unwrap(), "edit.undo")));
        assert!(table(&o).contains(&(commands::parse_key("Ctrl+Z").unwrap(), "edit.undo")), "the first key still works");
        // A second key can clash with a first key and take it over.
        assert_eq!(assign(&mut o, "edit.redo#2", "Ctrl+Z", false), Err(Refused::Taken("edit.undo")));
        assert_eq!(assign(&mut o, "job.frame#2", "F2", false), Err(Refused::Taken("job.start#2")));
        assign(&mut o, "job.start#2", "", false).unwrap();
        assert!(!table(&o).iter().any(|(k, _)| commands::parse_key("F2") == Some(*k)), "a cleared second key does nothing");
        // A cleared default second key survives a file.
        assert_eq!(from_file(&to_file(&o)).unwrap(), o);
        reset(&mut o, "job.start#2");
        assert_eq!(effective(&o)["job.start#2"], "F2");
        assign(&mut o, "edit.undo", "Ctrl+Alt+F7", false).unwrap();
        reset(&mut o, "edit.undo");
        assert!(o.is_empty(), "resetting a command resets both its keys: {o:?}");
        assert_eq!(assign(&mut o, "edit.undo#3", "F7", false), Err(Refused::UnknownCommand));
    }

    #[test]
    fn number_pad_keys_are_bindings_of_their_own() {
        assert_eq!(parse_binding("Numpad8"), Some(Binding::Numpad(Key::Num8)));
        assert_eq!(parse_binding("8"), Some(Binding::Keys(KeyboardShortcut::new(Modifiers::NONE, Key::Num8))));
        assert_eq!(parse_binding("Numpad"), None);
        assert_eq!(parse_binding("Numpad12"), None);
        assert_eq!(parse_binding("NumpadA"), None);
        assert_eq!(parse_binding("Numpad5"), None, "pad 5 is the digit 5 to egui");
        // NumLock off: an arrow with the physical key of a pad digit. NumLock on, or the digit
        // row: the digit itself, which is not a number pad key.
        assert_eq!(numpad_key(Key::ArrowUp, Some(Key::Num8)), Some(Key::Num8));
        assert_eq!(numpad_key(Key::Home, Some(Key::Num7)), Some(Key::Num7));
        assert_eq!(numpad_key(Key::Num8, Some(Key::Num8)), None);
        assert_eq!(numpad_key(Key::ArrowUp, Some(Key::ArrowUp)), None);
        assert_eq!(numpad_key(Key::ArrowUp, None), None);
        // The pad digit must send its own key: 8 is ArrowUp, never Home.
        assert_eq!(numpad_key(Key::Home, Some(Key::Num8)), None);
        assert_eq!(numpad_key(Key::Insert, Some(Key::Num0)), Some(Key::Num0));
        // Digit-row keys that type punctuation have a digit as their physical key too: Shift+1
        // is `!`, Shift+7 is `/` on Nordic and German keyboards, the AZERTY 6 is `-` and its 4
        // is `'`. None of them is a pad key.
        for (key, digit) in [(Key::Exclamationmark, Key::Num1), (Key::Slash, Key::Num7), (Key::Minus, Key::Num6), (Key::Quote, Key::Num4)] {
            assert_eq!(numpad_key(key, Some(digit)), None, "{key:?} on {digit:?}");
        }
        assert_eq!(numpad_text(Key::Num8), "Numpad8");
        let pad = numpad_table(&Overrides::new());
        for (key, id) in [
            (Key::Num8, "laser.jog_up"),
            (Key::Num2, "laser.jog_down"),
            (Key::Num4, "laser.jog_left"),
            (Key::Num6, "laser.jog_right"),
            (Key::Num7, "laser.jog_up_left"),
            (Key::Num9, "laser.jog_up_right"),
            (Key::Num1, "laser.jog_down_left"),
            (Key::Num3, "laser.jog_down_right"),
        ] {
            assert!(pad.contains(&(key, id)), "{key:?} {id}");
        }
        assert_eq!(pad.len(), 8);
        assert!(!table(&Overrides::new()).iter().any(|(k, _)| k.logical_key == Key::Num8 && !k.modifiers.any()), "a pad key is not the digit 8");
        // A number pad key can be moved like any other.
        let mut o = Overrides::new();
        assert_eq!(assign(&mut o, "laser.jog_up", "Numpad8", false), Err(Refused::Taken("laser.jog_up#2")));
        assign(&mut o, "laser.jog_up#2", "Numpad0", false).unwrap();
        assert!(numpad_table(&o).contains(&(Key::Num0, "laser.jog_up")));
        assert_eq!(assign(&mut o, "laser.jog_up#2", "Numpad5", false), Err(Refused::BadKey));
        assert_eq!(from_file(&to_file(&o)).unwrap(), o);
    }

    #[test]
    fn z_and_diagonal_jog_keys_have_defaults() {
        let o = Overrides::new();
        let e = effective(&o);
        for (id, key) in [
            ("laser.jog_up_left", "Ctrl+Alt+Home"),
            ("laser.jog_up_right", "Ctrl+Alt+PgUp"),
            ("laser.jog_down_left", "Ctrl+Alt+End"),
            ("laser.jog_down_right", "Ctrl+Alt+PgDn"),
            ("laser.jog_z_up", "Ctrl+Alt+]"),
            ("laser.jog_z_down", "Ctrl+Alt+["),
        ] {
            assert_eq!(e[id], key, "{id}");
            assert!(table(&o).contains(&(commands::parse_key(key).unwrap(), id)), "{id}");
        }
    }

    /// Keys saved by an older version that a newer default also uses: the user's key wins, in
    /// the preferences and in an imported file, and the menus show what really happens.
    #[test]
    fn a_users_key_beats_a_newer_default_key() {
        let mut o = Overrides::new();
        o.insert("view.zoom_page".into(), "F2".into());
        o.insert("view.pan".into(), "Ctrl+Alt+Home".into());
        let f2 = commands::parse_key("F2").unwrap();
        let home = commands::parse_key("Ctrl+Alt+Home").unwrap();
        let t = table(&o);
        assert_eq!(t.iter().filter(|(k, _)| *k == f2).map(|(_, id)| *id).collect::<Vec<_>>(), ["view.zoom_page"], "F2 does not start a job");
        assert_eq!(t.iter().filter(|(k, _)| *k == home).map(|(_, id)| *id).collect::<Vec<_>>(), ["view.pan"], "Ctrl+Alt+Home does not jog");
        assert!(t.contains(&(commands::parse_key("Alt+S").unwrap(), "job.start")), "Start keeps its first key");
        assert!(conflicts(&o).is_empty(), "{:?}", conflicts(&o));
        assert_eq!(key_of(&o, "job.start#2"), "");
        assert_eq!(key_of(&o, "laser.jog_up_left"), "");
        // An older file that binds F2 does the same.
        let old = r#"{"format":"darkburn-hotkeys","version":1,"keys":{"view.zoom_page":"F2","job.start":"Alt+S"}}"#;
        let imported = from_file(old).unwrap();
        assert!(!table(&imported).contains(&(f2, "job.start")));
        assert!(table(&imported).contains(&(f2, "view.zoom_page")));
        // Menus show the key that works.
        publish(&o);
        assert_eq!(key_for(commands::lookup("laser.jog_up_left").unwrap()), "");
        assert_eq!(key_for(commands::lookup("view.zoom_page").unwrap()), "F2");
        // Giving the key back brings the default back.
        reset(&mut o, "view.zoom_page");
        assert!(table(&o).contains(&(f2, "job.start")));
        publish(&Overrides::new());
    }

    #[test]
    fn hotkey_files_round_trip() {
        let mut o = Overrides::new();
        assign(&mut o, "file.save", "F8", false).unwrap();
        assign(&mut o, "edit.select_all", "", false).unwrap();
        let text = to_file(&o);
        assert!(text.contains(FILE_FORMAT));
        assert_eq!(from_file(&text).unwrap(), o);
        assert!(from_file("{}").is_err());
        assert!(from_file(r#"{"format":"darkburn-hotkeys","version":1,"keys":{"file.save":"Ctrl+Banana"}}"#).is_err());
        let skip = from_file(r#"{"format":"darkburn-hotkeys","version":1,"keys":{"future.command":"F9"}}"#).unwrap();
        assert!(skip.is_empty());
    }
}
