//! winit key → Linux evdev keycode translation (M1 T6).
//!
//! The core's input funnel speaks **evdev codes**, because that is what the
//! Wayland protocol carries and what xkb keymaps are indexed by. winit speaks
//! `KeyCode`, its own physical-key enum. Something has to be the table, and it
//! lives here — at the edge, in the backend that knows about winit, so no evdev
//! knowledge leaks into the core and no winit type does either.
//!
//! # What is covered, and what happens to the rest
//!
//! The standard typing set: letters, digits, the punctuation on a US keyboard,
//! modifiers, space/enter/backspace/tab/escape, and the arrows. That is the set
//! a terminal needs (M1's acceptance is "foot echoes typed input"), and it is
//! deliberately not the whole keyboard.
//!
//! **Unmapped keys are dropped and counted, never fatal.** A media key or a
//! keyboard with an exotic layout must not take the compositor down, and a
//! panic here would be a self-inflicted denial of service triggered by pressing
//! the wrong key. The counter ([`KeyTranslator::dropped`]) keeps the gap visible
//! instead of silent — when it starts moving in real use, that is the evidence
//! for widening the table. The full evdev set arrives with libinput in M2, which
//! delivers evdev codes directly and makes this table unnecessary rather than
//! bigger.

use winit::keyboard::KeyCode;

/// Translate a winit physical key to a Linux evdev keycode.
///
/// Returns `None` for keys outside the covered set (see the module docs). The
/// values are from `linux/input-event-codes.h`.
pub fn evdev_keycode(key: KeyCode) -> Option<u32> {
    // Written as a flat match rather than a lookup table: it is the same length,
    // it reads like the header file it mirrors, and a missing arm is visible.
    let code = match key {
        // Letters (evdev orders these by keyboard row, not alphabetically).
        KeyCode::KeyA => 30,
        KeyCode::KeyB => 48,
        KeyCode::KeyC => 46,
        KeyCode::KeyD => 32,
        KeyCode::KeyE => 18,
        KeyCode::KeyF => 33,
        KeyCode::KeyG => 34,
        KeyCode::KeyH => 35,
        KeyCode::KeyI => 23,
        KeyCode::KeyJ => 36,
        KeyCode::KeyK => 37,
        KeyCode::KeyL => 38,
        KeyCode::KeyM => 50,
        KeyCode::KeyN => 49,
        KeyCode::KeyO => 24,
        KeyCode::KeyP => 25,
        KeyCode::KeyQ => 16,
        KeyCode::KeyR => 19,
        KeyCode::KeyS => 31,
        KeyCode::KeyT => 20,
        KeyCode::KeyU => 22,
        KeyCode::KeyV => 47,
        KeyCode::KeyW => 17,
        KeyCode::KeyX => 45,
        KeyCode::KeyY => 21,
        KeyCode::KeyZ => 44,

        // Digit row.
        KeyCode::Digit1 => 2,
        KeyCode::Digit2 => 3,
        KeyCode::Digit3 => 4,
        KeyCode::Digit4 => 5,
        KeyCode::Digit5 => 6,
        KeyCode::Digit6 => 7,
        KeyCode::Digit7 => 8,
        KeyCode::Digit8 => 9,
        KeyCode::Digit9 => 10,
        KeyCode::Digit0 => 11,

        // Punctuation, in evdev's order.
        KeyCode::Minus => 12,
        KeyCode::Equal => 13,
        KeyCode::BracketLeft => 26,
        KeyCode::BracketRight => 27,
        KeyCode::Semicolon => 39,
        KeyCode::Quote => 40,
        KeyCode::Backquote => 41,
        KeyCode::Backslash => 43,
        KeyCode::Comma => 51,
        KeyCode::Period => 52,
        KeyCode::Slash => 53,

        // Editing and whitespace.
        KeyCode::Escape => 1,
        KeyCode::Backspace => 14,
        KeyCode::Tab => 15,
        KeyCode::Enter => 28,
        KeyCode::Space => 57,
        KeyCode::Delete => 111,

        // Modifiers. Both sides, because a keymap distinguishes them.
        KeyCode::ControlLeft => 29,
        KeyCode::ShiftLeft => 42,
        KeyCode::ShiftRight => 54,
        KeyCode::AltLeft => 56,
        KeyCode::CapsLock => 58,
        KeyCode::ControlRight => 97,
        KeyCode::AltRight => 100,
        KeyCode::SuperLeft => 125,
        KeyCode::SuperRight => 126,

        // Navigation.
        KeyCode::Home => 102,
        KeyCode::ArrowUp => 103,
        KeyCode::PageUp => 104,
        KeyCode::ArrowLeft => 105,
        KeyCode::ArrowRight => 106,
        KeyCode::End => 107,
        KeyCode::ArrowDown => 108,
        KeyCode::PageDown => 109,
        KeyCode::Insert => 110,

        // Everything else: function keys, media keys, the numeric keypad, and
        // whatever a given keyboard invents. Dropped and counted by the caller.
        _ => return None,
    };
    Some(code)
}

/// Translates winit keys and keeps count of the ones it cannot.
///
/// The count is the point: a silently-dropped key is indistinguishable from a
/// broken input path, and the difference matters when debugging "my keyboard
/// does nothing in Parhelion".
#[derive(Debug, Default)]
pub struct KeyTranslator {
    /// How many key events were dropped for want of a mapping.
    dropped: u64,
}

impl KeyTranslator {
    /// A fresh translator with an empty drop count.
    pub fn new() -> Self {
        KeyTranslator::default()
    }

    /// Translate `key`, counting it if there is no mapping.
    pub fn translate(&mut self, key: KeyCode) -> Option<u32> {
        let code = evdev_keycode(key);
        if code.is_none() {
            self.dropped += 1;
        }
        code
    }

    /// How many key events have been dropped so far.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codes are the real evdev values, spot-checked against
    /// `linux/input-event-codes.h` across every group in the table — a
    /// transposition inside one group would otherwise pass unnoticed until
    /// someone typed that key.
    #[test]
    fn known_keys_map_to_their_evdev_codes() {
        assert_eq!(evdev_keycode(KeyCode::KeyA), Some(30)); // KEY_A
        assert_eq!(evdev_keycode(KeyCode::KeyZ), Some(44)); // KEY_Z
        assert_eq!(evdev_keycode(KeyCode::Digit1), Some(2)); // KEY_1
        assert_eq!(evdev_keycode(KeyCode::Digit0), Some(11)); // KEY_0
        assert_eq!(evdev_keycode(KeyCode::Enter), Some(28)); // KEY_ENTER
        assert_eq!(evdev_keycode(KeyCode::Space), Some(57)); // KEY_SPACE
        assert_eq!(evdev_keycode(KeyCode::ShiftLeft), Some(42)); // KEY_LEFTSHIFT
        assert_eq!(evdev_keycode(KeyCode::ArrowUp), Some(103)); // KEY_UP
        assert_eq!(evdev_keycode(KeyCode::Slash), Some(53)); // KEY_SLASH
    }

    /// No two keys share a code. A duplicate would silently deliver the wrong
    /// character for one of them — the kind of bug that is obvious in use and
    /// invisible in review.
    #[test]
    fn the_table_has_no_duplicate_codes() {
        // Every key the table claims to cover, listed independently of the match
        // arms so the check does not simply mirror the implementation.
        let keys = [
            KeyCode::KeyA, KeyCode::KeyB, KeyCode::KeyC, KeyCode::KeyD, KeyCode::KeyE,
            KeyCode::KeyF, KeyCode::KeyG, KeyCode::KeyH, KeyCode::KeyI, KeyCode::KeyJ,
            KeyCode::KeyK, KeyCode::KeyL, KeyCode::KeyM, KeyCode::KeyN, KeyCode::KeyO,
            KeyCode::KeyP, KeyCode::KeyQ, KeyCode::KeyR, KeyCode::KeyS, KeyCode::KeyT,
            KeyCode::KeyU, KeyCode::KeyV, KeyCode::KeyW, KeyCode::KeyX, KeyCode::KeyY,
            KeyCode::KeyZ, KeyCode::Digit0, KeyCode::Digit1, KeyCode::Digit2,
            KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6,
            KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9, KeyCode::Minus,
            KeyCode::Equal, KeyCode::BracketLeft, KeyCode::BracketRight,
            KeyCode::Semicolon, KeyCode::Quote, KeyCode::Backquote, KeyCode::Backslash,
            KeyCode::Comma, KeyCode::Period, KeyCode::Slash, KeyCode::Escape,
            KeyCode::Backspace, KeyCode::Tab, KeyCode::Enter, KeyCode::Space,
            KeyCode::Delete, KeyCode::ControlLeft, KeyCode::ShiftLeft, KeyCode::ShiftRight,
            KeyCode::AltLeft, KeyCode::CapsLock, KeyCode::ControlRight, KeyCode::AltRight,
            KeyCode::SuperLeft, KeyCode::SuperRight, KeyCode::Home, KeyCode::ArrowUp,
            KeyCode::PageUp, KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::End,
            KeyCode::ArrowDown, KeyCode::PageDown, KeyCode::Insert,
        ];
        let mut codes: Vec<u32> = keys.iter().filter_map(|k| evdev_keycode(*k)).collect();
        assert_eq!(codes.len(), keys.len(), "every listed key has a mapping");
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), before, "no two keys share an evdev code");
    }

    /// An unmapped key is dropped and counted — not a panic, and not silence.
    #[test]
    fn unmapped_keys_are_counted_not_fatal() {
        let mut t = KeyTranslator::new();
        assert_eq!(t.translate(KeyCode::KeyA), Some(30));
        assert_eq!(t.dropped(), 0, "a mapped key is not counted as dropped");

        assert_eq!(t.translate(KeyCode::F13), None, "unmapped keys yield None");
        assert_eq!(t.translate(KeyCode::MediaPlayPause), None);
        assert_eq!(t.dropped(), 2, "and each one is counted");
    }
}
