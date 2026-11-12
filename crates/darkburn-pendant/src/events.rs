//! Turning the stream of held-key sets into presses, releases and Fn chords.

use crate::protocol::{Key, KeySet};

/// Something that happened on the pendant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendantEvent {
    /// The receiver was found and opened. The text names it for messages.
    Connected(String),
    /// The open receiver went away (unplugged, or a read failed). Every held key has had its
    /// [`PendantEvent::KeyUp`] before this.
    Disconnected(String),
    /// The receiver could not be opened. Sent once per new reason while the pendant is on and
    /// no receiver is open, so the app can show it (for example the Linux permission message).
    Unavailable(crate::PendantError),
    /// A key went down on its own (Fn not held). Fn itself also arrives as a `KeyDown`, and so
    /// do Stop and Pause whatever else is held: see [`never_chords`].
    KeyDown(Key),
    /// A key went down while Fn was held: Fn+key. Comes instead of `KeyDown` for that key.
    /// Never for Stop or Pause.
    Chord(Key),
    /// A key came up (after a `KeyDown` or a `Chord`).
    KeyUp(Key),
}

/// Stop and Pause are a press on their own even while Fn is held: a thumb resting on Fn must
/// never turn Stop into a chord that does nothing.
pub fn never_chords(key: Key) -> bool {
    matches!(key, Key::Stop | Key::Pause)
}

/// Remembers which keys are held and turns each new report into events.
///
/// A report is the full set of keys held now, so a press is a key that is in the new set and not
/// in the last one, and a release the other way round. Each press gives exactly one action
/// event: [`PendantEvent::Chord`] when Fn is held (or goes down in the same report),
/// [`PendantEvent::KeyDown`] otherwise. Stop and Pause always give a `KeyDown`.
#[derive(Clone, Debug, Default)]
pub struct KeyTracker {
    held: KeySet,
}

impl KeyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The keys held after the last report.
    pub fn held(&self) -> KeySet {
        self.held
    }

    /// Takes the next report's held set. Releases come first, then Fn, then the other presses
    /// in code order.
    pub fn update(&mut self, now: KeySet) -> Vec<PendantEvent> {
        let mut out = Vec::new();
        out.extend(self.held.minus(now).iter().map(PendantEvent::KeyUp));
        let pressed = now.minus(self.held);
        if pressed.contains(Key::Fn) {
            out.push(PendantEvent::KeyDown(Key::Fn));
        }
        let fn_held = now.contains(Key::Fn);
        out.extend(
            pressed.iter().filter(|k| *k != Key::Fn).map(
                |k| {
                    if fn_held && !never_chords(k) { PendantEvent::Chord(k) } else { PendantEvent::KeyDown(k) }
                },
            ),
        );
        self.held = now;
        out
    }

    /// Lets go of every held key, as when the receiver goes away.
    pub fn release_all(&mut self) -> Vec<PendantEvent> {
        self.update(KeySet::EMPTY)
    }
}

#[cfg(test)]
mod tests {
    use super::PendantEvent::*;
    use super::*;
    use crate::protocol::decode_input;

    fn feed(t: &mut KeyTracker, report: [u8; 6]) -> Vec<PendantEvent> {
        t.update(decode_input(&report).unwrap().held)
    }

    #[test]
    fn press_and_release() {
        let mut t = KeyTracker::new();
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x08, 0x00, 0x00, 0x08]), vec![KeyDown(Key::Start)]);
        // The same set again (a repeat or the idle tick while held) gives nothing.
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x08, 0x00, 0x00, 0x08]), vec![]);
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x00, 0x00, 0x00, 0x10]), vec![KeyUp(Key::Start)]);
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x00, 0x00, 0x00, 0x10]), vec![]);
    }

    #[test]
    fn fn_then_key_is_a_chord() {
        let mut t = KeyTracker::new();
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x0b, 0x00, 0x00, 0x05]), vec![KeyDown(Key::Fn)]);
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x0b, 0x07, 0x00, 0x00]), vec![Chord(Key::GoToOrigin)]);
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x0b, 0x00, 0x00, 0x05]), vec![KeyUp(Key::GoToOrigin)]);
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x00, 0x00, 0x00, 0x10]), vec![KeyUp(Key::Fn)]);
    }

    #[test]
    fn fn_and_key_in_one_report_is_a_chord() {
        let mut t = KeyTracker::new();
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x10, 0x0b, 0x00, 0x00]), vec![KeyDown(Key::Fn), Chord(Key::ZPlusW)]);
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x00, 0x00, 0x00, 0x00]), vec![KeyUp(Key::Fn), KeyUp(Key::ZPlusW)]);
    }

    #[test]
    fn printed_fn_functions_all_chord() {
        for k in [Key::GoToOrigin, Key::ZCalibrate, Key::ZMinusW, Key::ZPlusW] {
            let mut t = KeyTracker::new();
            t.update([Key::Fn].into_iter().collect());
            assert_eq!(t.update([Key::Fn, k].into_iter().collect()), vec![Chord(k)]);
        }
    }

    /// Fn held (or going down in the same report) must not swallow Stop or Pause.
    #[test]
    fn stop_and_pause_are_plain_presses_with_fn_held() {
        for k in [Key::Stop, Key::Pause] {
            let mut t = KeyTracker::new();
            t.update([Key::Fn].into_iter().collect());
            assert_eq!(t.update([Key::Fn, k].into_iter().collect()), vec![KeyDown(k)], "Fn held, then {k:?}");
            let mut t = KeyTracker::new();
            assert_eq!(t.update([Key::Fn, k].into_iter().collect()), vec![KeyDown(Key::Fn), KeyDown(k)], "Fn and {k:?} in one report");
        }
        // The report of the finding: 04 10 0b 0a 00 chk, Fn already held.
        let mut t = KeyTracker::new();
        feed(&mut t, [0x04, 0x10, 0x0b, 0x00, 0x00, 0x05]);
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x0b, 0x0a, 0x00, 0x00]), vec![KeyDown(Key::Stop)]);
    }

    #[test]
    fn key_held_before_fn_stays_a_plain_press() {
        let mut t = KeyTracker::new();
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x0f, 0x00, 0x00, 0x00]), vec![KeyDown(Key::Up)]);
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x0f, 0x0b, 0x00, 0x00]), vec![KeyDown(Key::Fn)]);
    }

    #[test]
    fn three_keys_and_partial_release() {
        let mut t = KeyTracker::new();
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x0f, 0x11, 0x0d, 0x00]), vec![KeyDown(Key::FastStep), KeyDown(Key::Up), KeyDown(Key::Left)]);
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x11, 0x00, 0x00, 0x00]), vec![KeyUp(Key::FastStep), KeyUp(Key::Up)]);
        assert_eq!(t.held().iter().collect::<Vec<_>>(), vec![Key::Left]);
        // One key replaced by another in the same report.
        assert_eq!(feed(&mut t, [0x04, 0x10, 0x13, 0x00, 0x00, 0x00]), vec![KeyUp(Key::Left), KeyDown(Key::Right)]);
    }

    #[test]
    fn release_all_lets_go_of_everything() {
        let mut t = KeyTracker::new();
        feed(&mut t, [0x04, 0x10, 0x0b, 0x12, 0x00, 0x00]);
        assert_eq!(t.release_all(), vec![KeyUp(Key::Fn), KeyUp(Key::Down)]);
        assert!(t.held().is_empty());
        assert_eq!(t.release_all(), vec![]);
    }
}
