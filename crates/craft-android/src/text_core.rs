//! Typed text on Android, as pure logic (no JNI, no egui) so it can be tested on any host.
//!
//! Typed characters can reach the app by three routes, and none of them is reliable on every
//! device: winit's own translation of key events (which depends on the input device's key
//! character map), the character `MainActivity` resolves with Android's own APIs, and text a soft
//! keyboard commits through the IME. [`text_for_key`] turns what Java reports about a key press into
//! text, and [`TextMerger`] lets the routes coexist without typing a character twice.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// `KeyCharacterMap.COMBINING_ACCENT`: set on the code of a dead key (an accent waiting for a letter).
const COMBINING_ACCENT: u32 = 0x8000_0000;

/// How long a character from one route waits for its twin from another.
const MATCH_WINDOW: Duration = Duration::from_millis(400);

/// The text a key press types, or `None` if it types nothing (a shortcut, an arrow, Enter, ...).
///
/// * `unicode`: `KeyEvent.getUnicodeChar()` for the event as it was (all modifiers).
/// * `unicode_no_alt`: the same with Alt stripped from the modifiers. Some keyboard layouts report
///   nothing for plain letters unless Alt is held; this is what makes typing work without it.
/// * If Android reports no character at all, a plain US-QWERTY character is used for letters,
///   digits, punctuation and the numeric keypad.
///
/// Presses with Ctrl or Meta held are shortcuts, not text.
pub fn text_for_key(key_code: i32, unicode: i32, unicode_no_alt: i32, shift: bool, caps_lock: bool, ctrl_or_meta: bool) -> Option<String> {
    if ctrl_or_meta {
        return None;
    }
    [unicode, unicode_no_alt]
        .into_iter()
        .find_map(printable)
        .or_else(|| qwerty_fallback(key_code, shift, caps_lock))
        .map(String::from)
}

/// The character for a Java key code, unless it's a dead key, a control character or nothing.
fn printable(code: i32) -> Option<char> {
    if code <= 0 || (code as u32) & COMBINING_ACCENT != 0 {
        return None;
    }
    char::from_u32(code as u32).filter(|c| !c.is_control())
}

/// US-QWERTY, by Android key code (`KeyEvent.KEYCODE_*`).
fn qwerty_fallback(key_code: i32, shift: bool, caps_lock: bool) -> Option<char> {
    Some(match key_code {
        29..=54 => {
            let letter = (b'a' + (key_code - 29) as u8) as char;
            if shift != caps_lock {
                letter.to_ascii_uppercase()
            } else {
                letter
            }
        }
        7..=16 => {
            let digit = key_code - 7;
            if shift {
                [')', '!', '@', '#', '$', '%', '^', '&', '*', '('][digit as usize]
            } else {
                char::from_digit(digit as u32, 10)?
            }
        }
        144..=153 => char::from_digit((key_code - 144) as u32, 10)?,
        154 => '/',
        155 => '*',
        156 => '-',
        157 => '+',
        158 => '.',
        62 => ' ',
        55 => if shift { '<' } else { ',' },
        56 => if shift { '>' } else { '.' },
        68 => if shift { '~' } else { '`' },
        69 => if shift { '_' } else { '-' },
        70 => if shift { '+' } else { '=' },
        71 => if shift { '{' } else { '[' },
        72 => if shift { '}' } else { ']' },
        73 => if shift { '|' } else { '\\' },
        74 => if shift { ':' } else { ';' },
        75 => if shift { '"' } else { '\'' },
        76 => if shift { '?' } else { '/' },
        _ => return None,
    })
}

/// Text from the soft keyboard's IME, without control characters (a newline is Enter, which arrives
/// as a key press, and would otherwise be typed into single-line fields).
pub fn sanitize_ime_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// Lets the same typed character arrive by two routes (winit, and Java) and be typed once.
///
/// Each route reports its characters here. The first to report a character is let through and
/// remembered for a moment; when the other route reports the same character, it is a duplicate
/// and is dropped. If only one route works, everything it reports gets through.
#[derive(Default)]
pub struct TextMerger {
    /// Characters winit typed that Java hasn't (yet) repeated.
    winit: VecDeque<(String, Instant)>,
    /// Characters Java typed that winit hasn't (yet) repeated.
    java: VecDeque<(String, Instant)>,
}

impl TextMerger {
    /// Text winit generated: `true` to type it, `false` if Java already typed it.
    pub fn from_winit(&mut self, text: &str, now: Instant) -> bool {
        Self::offer(&mut self.winit, &mut self.java, text, now)
    }

    /// Text Java captured: `true` to type it, `false` if winit already typed it.
    pub fn from_java(&mut self, text: &str, now: Instant) -> bool {
        Self::offer(&mut self.java, &mut self.winit, text, now)
    }

    fn offer(own: &mut VecDeque<(String, Instant)>, other: &mut VecDeque<(String, Instant)>, text: &str, now: Instant) -> bool {
        for queue in [&mut *own, &mut *other] {
            queue.retain(|(_, at)| now.saturating_duration_since(*at) <= MATCH_WINDOW);
        }
        if let Some(i) = other.iter().position(|(t, _)| t == text) {
            other.remove(i); // the twin of something already typed
            false
        } else {
            own.push_back((text.to_owned(), now));
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: i32 = 29;
    const SHIFTED_A: i32 = 'A' as i32;

    #[test]
    fn android_reported_characters_win() {
        assert_eq!(text_for_key(A, 'a' as i32, 'a' as i32, false, false, false).as_deref(), Some("a"));
        assert_eq!(text_for_key(A, SHIFTED_A, SHIFTED_A, true, false, false).as_deref(), Some("A"));
        assert_eq!(text_for_key(62, ' ' as i32, ' ' as i32, false, false, false).as_deref(), Some(" "));
        assert_eq!(text_for_key(A, 'é' as i32, 'a' as i32, false, false, false).as_deref(), Some("é"), "AltGr-style characters are kept");
    }

    #[test]
    fn letters_type_without_alt_when_only_the_alt_lookup_answers() {
        // The reported bug: the key character map only answers when Alt is held.
        assert_eq!(text_for_key(A, 0, 'a' as i32, false, false, false).as_deref(), Some("a"));
    }

    #[test]
    fn a_key_android_has_no_character_for_falls_back_to_qwerty() {
        assert_eq!(text_for_key(A, 0, 0, false, false, false).as_deref(), Some("a"));
        assert_eq!(text_for_key(A, 0, 0, true, false, false).as_deref(), Some("A"));
        assert_eq!(text_for_key(A, 0, 0, false, true, false).as_deref(), Some("A"), "caps lock");
        assert_eq!(text_for_key(A, 0, 0, true, true, false).as_deref(), Some("a"), "shift cancels caps lock");
        assert_eq!(text_for_key(54, 0, 0, false, false, false).as_deref(), Some("z"));
        assert_eq!(text_for_key(7, 0, 0, false, false, false).as_deref(), Some("0"));
        assert_eq!(text_for_key(16, 0, 0, true, false, false).as_deref(), Some("("));
        assert_eq!(text_for_key(8, 0, 0, true, false, false).as_deref(), Some("!"));
        assert_eq!(text_for_key(62, 0, 0, false, false, false).as_deref(), Some(" "));
        assert_eq!(text_for_key(69, 0, 0, true, false, false).as_deref(), Some("_"));
        assert_eq!(text_for_key(153, 0, 0, false, false, false).as_deref(), Some("9"), "numeric keypad");
    }

    #[test]
    fn shortcuts_and_control_keys_type_nothing() {
        assert_eq!(text_for_key(A, 'a' as i32, 'a' as i32, false, false, true), None, "Ctrl+A / Cmd+A");
        assert_eq!(text_for_key(66, 10, 10, false, false, false), None, "Enter is a key, not text");
        assert_eq!(text_for_key(61, 9, 9, false, false, false), None, "Tab");
        assert_eq!(text_for_key(67, 0, 0, false, false, false), None, "Backspace");
        assert_eq!(text_for_key(19, 0, 0, false, false, false), None, "arrow");
    }

    #[test]
    fn a_dead_key_types_nothing_by_itself() {
        let dead_acute = (0x8000_0000_u32 | 0xB4) as i32;
        assert_eq!(text_for_key(75, dead_acute, dead_acute, false, false, false).as_deref(), Some("'"), "falls to the plain key");
        assert_eq!(printable(dead_acute), None);
    }

    #[test]
    fn ime_text_loses_control_characters() {
        assert_eq!(sanitize_ime_text("hello"), "hello");
        assert_eq!(sanitize_ime_text("a\nb\tc"), "abc");
        assert_eq!(sanitize_ime_text("héllo wörld"), "héllo wörld");
        assert_eq!(sanitize_ime_text("\n"), "");
    }

    #[test]
    fn a_character_from_both_routes_is_typed_once_in_either_order() {
        let t0 = Instant::now();
        let mut m = TextMerger::default();
        assert!(m.from_winit("a", t0));
        assert!(!m.from_java("a", t0), "Java's twin of winit's character is dropped");
        assert!(m.from_java("b", t0));
        assert!(!m.from_winit("b", t0), "winit's twin of Java's character is dropped");
    }

    #[test]
    fn a_repeated_character_is_typed_each_time() {
        let t0 = Instant::now();
        let mut m = TextMerger::default();
        // "ll" with both routes working, interleaved or batched
        assert!(m.from_java("l", t0));
        assert!(m.from_java("l", t0));
        assert!(!m.from_winit("l", t0));
        assert!(!m.from_winit("l", t0));
        // and nothing is left over to swallow a later, genuine "l"
        assert!(m.from_winit("l", t0));
    }

    #[test]
    fn if_only_one_route_works_everything_gets_through() {
        let t0 = Instant::now();
        let mut m = TextMerger::default();
        for (i, c) in ["h", "e", "l", "l", "o"].iter().enumerate() {
            assert!(m.from_java(c, t0 + Duration::from_millis(i as u64 * 50)), "{c}");
        }
        let mut m = TextMerger::default();
        for (i, c) in ["h", "e", "l", "l", "o"].iter().enumerate() {
            assert!(m.from_winit(c, t0 + Duration::from_millis(i as u64 * 50)), "{c}");
        }
    }

    #[test]
    fn a_twin_that_arrives_too_late_is_not_matched() {
        let t0 = Instant::now();
        let mut m = TextMerger::default();
        assert!(m.from_java("a", t0));
        assert!(m.from_winit("a", t0 + Duration::from_millis(900)), "a later 'a' is a new keypress");
    }

    #[test]
    fn different_characters_never_cancel_each_other() {
        let t0 = Instant::now();
        let mut m = TextMerger::default();
        assert!(m.from_java("a", t0));
        assert!(m.from_winit("b", t0));
        assert!(m.from_winit("a", t0) == false);
    }
}
