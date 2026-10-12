//! The ONE key adapter: `egui::Key` → the macOS virtual key codes
//! [`crate::ui::popup::route_key`] already understands ([`popup::KEY_*`]
//! consts). The existing pure routing ([`crate::ui::popup::route_key`],
//! [`crate::engines::text_edit_keys`]) is reused unchanged: egui only feeds it
//! a [`popup::KeyInput`] whose `key_code` is a macOS `kVK_*` value, exactly
//! like `NSEvent.keyCode` would.

use crate::ui::popup::{self, KeyInput};

/// Letters are the standard `kVK_ANSI_*` codes; the subset `popup.rs` doesn't
/// name is listed here (the ones it does name are reused via the `KEY_*` consts).
const KEY_B: u16 = 11;
const KEY_D: u16 = 2;
const KEY_E: u16 = 14;
const KEY_G: u16 = 5;
const KEY_I: u16 = 34;
const KEY_M: u16 = 46;
const KEY_Q: u16 = 12;
const KEY_T: u16 = 17;
const KEY_U: u16 = 32;
const KEY_Y: u16 = 16;

/// The number-row digits (`kVK_ANSI_0` … `kVK_ANSI_9`).
const KEY_1: u16 = 18;
const KEY_2: u16 = 19;
const KEY_3: u16 = 20;
const KEY_4: u16 = 21;
const KEY_5: u16 = 23;
const KEY_6: u16 = 22;
const KEY_7: u16 = 26;
const KEY_8: u16 = 28;
const KEY_9: u16 = 25;

const KEY_DELETE: u16 = 51; // backspace
const KEY_FWD_DELETE: u16 = 117;
const KEY_LEFT: u16 = 123;
const KEY_RIGHT: u16 = 124;
const KEY_COMMA: u16 = 43;
const KEY_PERIOD: u16 = 47;
const KEY_SEMICOLON: u16 = 41;
const KEY_QUOTE: u16 = 39;
const KEY_BACKTICK: u16 = 50;
const KEY_OPEN_BRACKET: u16 = 33;
const KEY_CLOSE_BRACKET: u16 = 30;

/// `egui::Key` → the macOS virtual key code, or `None` for keys that have no
/// `NSEvent.keyCode` counterpart (synthetic clipboard keys, F-keys, and the
/// modifier keys that egui reports in `Modifiers` instead).
fn key_code(key: egui::Key) -> Option<u16> {
    use egui::Key as K;
    let code = match key {
        K::ArrowDown => popup::KEY_DOWN,
        K::ArrowUp => popup::KEY_UP,
        K::ArrowLeft => KEY_LEFT,
        K::ArrowRight => KEY_RIGHT,
        K::Escape => popup::KEY_ESC,
        K::Tab => popup::KEY_TAB,
        K::Backspace => KEY_DELETE,
        K::Enter => popup::KEY_RETURN,
        K::Space => popup::KEY_SPACE,
        K::Delete => KEY_FWD_DELETE,
        K::Home => popup::KEY_HOME,
        K::End => popup::KEY_END,
        K::PageUp => popup::KEY_PAGE_UP,
        K::PageDown => popup::KEY_PAGE_DOWN,
        K::Slash => popup::KEY_SLASH,
        K::Backslash => popup::KEY_BACKSLASH,
        K::Minus => popup::KEY_MINUS,
        K::Plus | K::Equals => popup::KEY_PLUS,
        K::Comma => KEY_COMMA,
        K::Period => KEY_PERIOD,
        K::Semicolon | K::Colon => KEY_SEMICOLON,
        K::Quote => KEY_QUOTE,
        K::Backtick => KEY_BACKTICK,
        K::OpenBracket => KEY_OPEN_BRACKET,
        K::CloseBracket => KEY_CLOSE_BRACKET,
        K::Pipe => popup::KEY_BACKSLASH,
        K::Questionmark => popup::KEY_SLASH,
        K::Num0 => popup::KEY_ZERO,
        K::Num1 => KEY_1,
        K::Num2 => KEY_2,
        K::Num3 => KEY_3,
        K::Num4 => KEY_4,
        K::Num5 => KEY_5,
        K::Num6 => KEY_6,
        K::Num7 => KEY_7,
        K::Num8 => KEY_8,
        K::Num9 => KEY_9,
        K::A => popup::KEY_A,
        K::B => KEY_B,
        K::C => popup::KEY_C,
        K::D => KEY_D,
        K::E => KEY_E,
        K::F => popup::KEY_F,
        K::G => KEY_G,
        K::H => popup::KEY_H,
        K::I => KEY_I,
        K::J => popup::KEY_J,
        K::K => popup::KEY_K,
        K::L => popup::KEY_L,
        K::M => KEY_M,
        K::N => popup::KEY_N,
        K::O => popup::KEY_O,
        K::P => popup::KEY_P,
        K::Q => KEY_Q,
        K::R => popup::KEY_R,
        K::S => popup::KEY_S,
        K::T => KEY_T,
        K::U => KEY_U,
        K::V => popup::KEY_V,
        K::W => popup::KEY_W,
        K::X => popup::KEY_X,
        K::Y => KEY_Y,
        K::Z => popup::KEY_Z,
        // Insert, F-keys, modifier keys and the synthetic Copy/Cut/Paste have
        // no `keyCode` the router reads.
        _ => return None,
    };
    Some(code)
}

/// The printable character for letter/digit keys (lowercase; the caller applies
/// Shift). `None` for every other key.
fn key_char(key: egui::Key) -> Option<char> {
    use egui::Key as K;
    Some(match key {
        K::A => 'a',
        K::B => 'b',
        K::C => 'c',
        K::D => 'd',
        K::E => 'e',
        K::F => 'f',
        K::G => 'g',
        K::H => 'h',
        K::I => 'i',
        K::J => 'j',
        K::K => 'k',
        K::L => 'l',
        K::M => 'm',
        K::N => 'n',
        K::O => 'o',
        K::P => 'p',
        K::Q => 'q',
        K::R => 'r',
        K::S => 's',
        K::T => 't',
        K::U => 'u',
        K::V => 'v',
        K::W => 'w',
        K::X => 'x',
        K::Y => 'y',
        K::Z => 'z',
        K::Num0 => '0',
        K::Num1 => '1',
        K::Num2 => '2',
        K::Num3 => '3',
        K::Num4 => '4',
        K::Num5 => '5',
        K::Num6 => '6',
        K::Num7 => '7',
        K::Num8 => '8',
        K::Num9 => '9',
        K::Space => ' ',
        _ => return None,
    })
}

/// Map one egui key press to the [`KeyInput`] the existing router consumes.
///
/// `text` is the typed character when available (egui's `Event::Text`); when
/// absent the letter/digit character is derived from `key` + Shift, which is
/// enough for the router's letter jumps and the Ctrl+B prefix commands.
///
/// Returns `None` for keys with no `keyCode` counterpart (F-keys, modifier
/// keys, synthetic clipboard keys) so the caller treats them as unmapped.
pub fn key_input(key: egui::Key, m: egui::Modifiers, text: Option<char>) -> Option<KeyInput> {
    let key_code = key_code(key)?;
    let shift = m.shift;
    let chars = text.or_else(|| {
        key_char(key).map(|ch| if shift { ch.to_ascii_uppercase() } else { ch })
    });
    Some(KeyInput {
        key_code,
        chars,
        // `mac_cmd` is the literal ⌘ key (macOS only); `command` would also be
        // true for Ctrl, which the router must keep separate from Command.
        cmd: m.mac_cmd,
        ctrl: m.ctrl,
        opt: m.alt,
        shift,
        esc_streak: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    fn no_mods() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn maps_letters_to_kvk_codes() {
        assert_eq!(key_input(Key::A, no_mods(), None).unwrap().key_code, 0);
        assert_eq!(key_input(Key::B, no_mods(), None).unwrap().key_code, 11);
        assert_eq!(key_input(Key::Z, no_mods(), None).unwrap().key_code, 6);
        assert_eq!(key_input(Key::L, no_mods(), None).unwrap().key_code, 37);
    }

    #[test]
    fn maps_arrows_and_special_keys() {
        assert_eq!(key_input(Key::ArrowDown, no_mods(), None).unwrap().key_code, popup::KEY_DOWN);
        assert_eq!(key_input(Key::ArrowUp, no_mods(), None).unwrap().key_code, popup::KEY_UP);
        assert_eq!(key_input(Key::Escape, no_mods(), None).unwrap().key_code, popup::KEY_ESC);
        assert_eq!(key_input(Key::Enter, no_mods(), None).unwrap().key_code, popup::KEY_RETURN);
    }

    #[test]
    fn derives_chars_and_shift() {
        let shifted = Modifiers { shift: true, ..Modifiers::default() };
        assert_eq!(key_input(Key::L, no_mods(), None).unwrap().chars, Some('l'));
        assert_eq!(key_input(Key::L, shifted, None).unwrap().chars, Some('L'));
        assert_eq!(key_input(Key::Num0, no_mods(), None).unwrap().chars, Some('0'));
    }

    #[test]
    fn prefers_explicit_text() {
        assert_eq!(key_input(Key::L, no_mods(), Some('x')).unwrap().chars, Some('x'));
    }

    #[test]
    fn reports_modifiers() {
        let m = Modifiers { mac_cmd: true, ctrl: true, alt: true, shift: true, ..Modifiers::default() };
        let ki = key_input(Key::V, m, None).unwrap();
        assert!(ki.cmd && ki.ctrl && ki.opt && ki.shift);
    }

    #[test]
    fn unmapped_keys_are_none() {
        assert!(key_input(Key::F1, no_mods(), None).is_none());
        assert!(key_input(Key::Insert, no_mods(), None).is_none());
        assert!(key_input(Key::Copy, no_mods(), None).is_none());
    }
}
