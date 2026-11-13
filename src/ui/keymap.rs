//! Keyboard positions to RDP scancodes (PC/AT set 1).
//!
//! RDP sends where a key is, not what it types: the remote PC applies its own
//! keyboard layout. So a key is looked up by its physical position, named by
//! egui after the US layout, and the remote layout (detected from this
//! machine's) turns it into the right character, `ñ` included.

use egui::Key;

pub use remoport_core::session::input::Scancode;

pub const SHIFT: Scancode = (0x2A, false);
pub const CTRL: Scancode = (0x1D, false);
pub const ALT: Scancode = (0x38, false);
pub const C: Scancode = (0x2E, false);
pub const X: Scancode = (0x2D, false);
pub const V: Scancode = (0x2F, false);
/// Keys egui on Linux never reports, sent from the session's Keys menu.
pub const WINDOWS: Scancode = (0x5B, true);
#[allow(dead_code)]
pub const WINDOWS_RIGHT: Scancode = (0x5C, true);
#[allow(dead_code)]
pub const MENU: Scancode = (0x5D, true);
pub const ESCAPE: Scancode = (0x01, false);
pub const TAB: Scancode = (0x0F, false);
pub const F4: Scancode = (0x3E, false);
pub const PRINT_SCREEN: Scancode = (0x37, true);
pub const R: Scancode = (0x13, false);
pub const E: Scancode = (0x12, false);
pub const D: Scancode = (0x20, false);

/// Key combinations for the PC that this computer would keep for itself
/// (Alt+Tab, Alt+F4) or that egui does not report (the Windows key).
pub const COMBINATIONS: &[(&str, &[Scancode])] = &[
    ("Windows key (Start)", &[WINDOWS]),
    ("Win+R · Run", &[WINDOWS, R]),
    ("Win+E · File Explorer", &[WINDOWS, E]),
    ("Win+D · Show desktop", &[WINDOWS, D]),
    ("Alt+Tab · Switch apps", &[ALT, TAB]),
    ("Ctrl+Shift+Esc · Task Manager", &[CTRL, SHIFT, ESCAPE]),
    ("Alt+F4 · Close the window", &[ALT, F4]),
    ("Print Screen", &[PRINT_SCREEN]),
];

/// A combination as key presses in order, then releases in reverse.
pub fn strokes(keys: &[Scancode]) -> Vec<(Scancode, bool)> {
    keys.iter()
        .map(|&key| (key, true))
        .chain(keys.iter().rev().map(|&key| (key, false)))
        .collect()
}

/// The scancode of the key at `key`'s position on a US keyboard, if it has one.
pub fn scancode(key: Key) -> Option<Scancode> {
    let plain = |code: u8| Some((code, false));
    let extended = |code: u8| Some((code, true));
    match key {
        Key::Escape => plain(0x01),
        Key::Num1 => plain(0x02),
        Key::Num2 => plain(0x03),
        Key::Num3 => plain(0x04),
        Key::Num4 => plain(0x05),
        Key::Num5 => plain(0x06),
        Key::Num6 => plain(0x07),
        Key::Num7 => plain(0x08),
        Key::Num8 => plain(0x09),
        Key::Num9 => plain(0x0A),
        Key::Num0 => plain(0x0B),
        Key::Minus => plain(0x0C),
        Key::Equals | Key::Plus => plain(0x0D),
        Key::Backspace => plain(0x0E),
        Key::Tab => plain(0x0F),
        Key::Q => plain(0x10),
        Key::W => plain(0x11),
        Key::E => plain(0x12),
        Key::R => plain(0x13),
        Key::T => plain(0x14),
        Key::Y => plain(0x15),
        Key::U => plain(0x16),
        Key::I => plain(0x17),
        Key::O => plain(0x18),
        Key::P => plain(0x19),
        Key::OpenBracket => plain(0x1A),
        Key::CloseBracket => plain(0x1B),
        Key::Enter => plain(0x1C),
        Key::A => plain(0x1E),
        Key::S => plain(0x1F),
        Key::D => plain(0x20),
        Key::F => plain(0x21),
        Key::G => plain(0x22),
        Key::H => plain(0x23),
        Key::J => plain(0x24),
        Key::K => plain(0x25),
        Key::L => plain(0x26),
        Key::Semicolon | Key::Colon => plain(0x27),
        Key::Quote => plain(0x28),
        Key::Backtick => plain(0x29),
        Key::Backslash | Key::Pipe => plain(0x2B),
        Key::Z => plain(0x2C),
        Key::X => plain(0x2D),
        Key::C => plain(0x2E),
        Key::V => plain(0x2F),
        Key::B => plain(0x30),
        Key::N => plain(0x31),
        Key::M => plain(0x32),
        Key::Comma => plain(0x33),
        Key::Period => plain(0x34),
        Key::Slash | Key::Questionmark => plain(0x35),
        Key::Space => plain(0x39),
        Key::F1 => plain(0x3B),
        Key::F2 => plain(0x3C),
        Key::F3 => plain(0x3D),
        Key::F4 => plain(0x3E),
        Key::F5 => plain(0x3F),
        Key::F6 => plain(0x40),
        Key::F7 => plain(0x41),
        Key::F8 => plain(0x42),
        Key::F9 => plain(0x43),
        Key::F10 => plain(0x44),
        Key::F11 => plain(0x57),
        Key::F12 => plain(0x58),
        Key::Home => extended(0x47),
        Key::ArrowUp => extended(0x48),
        Key::PageUp => extended(0x49),
        Key::ArrowLeft => extended(0x4B),
        Key::ArrowRight => extended(0x4D),
        Key::End => extended(0x4F),
        Key::ArrowDown => extended(0x50),
        Key::PageDown => extended(0x51),
        Key::Insert => extended(0x52),
        Key::Delete => extended(0x53),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_follow_the_us_rows() {
        assert_eq!(scancode(Key::Q), Some((0x10, false)));
        assert_eq!(scancode(Key::A), Some((0x1E, false)));
        assert_eq!(scancode(Key::Z), Some((0x2C, false)));
        assert_eq!(scancode(Key::Num1), Some((0x02, false)));
        assert_eq!(scancode(Key::Num0), Some((0x0B, false)));
    }

    #[test]
    fn navigation_keys_are_extended() {
        for key in [
            Key::ArrowUp,
            Key::ArrowLeft,
            Key::Home,
            Key::Delete,
            Key::PageDown,
        ] {
            assert!(
                scancode(key).is_some_and(|(_, extended)| extended),
                "{key:?}"
            );
        }
        assert_eq!(scancode(Key::Enter), Some((0x1C, false)));
    }

    #[test]
    fn shifted_names_share_their_key() {
        // egui may name a key by what it types; the position is the same.
        assert_eq!(scancode(Key::Colon), scancode(Key::Semicolon));
        assert_eq!(scancode(Key::Plus), scancode(Key::Equals));
        assert_eq!(scancode(Key::Questionmark), scancode(Key::Slash));
        assert_eq!(scancode(Key::Pipe), scancode(Key::Backslash));
    }

    #[test]
    fn the_spanish_n_key_is_the_us_semicolon_position() {
        // On a Spanish or Latin American layout `ñ` sits where US has `;`.
        assert_eq!(scancode(Key::Semicolon), Some((0x27, false)));
    }

    #[test]
    fn clipboard_letters_match_their_keys() {
        assert_eq!(scancode(Key::C), Some(C));
        assert_eq!(scancode(Key::X), Some(X));
        assert_eq!(scancode(Key::V), Some(V));
    }

    #[test]
    fn combinations_press_in_order_and_release_in_reverse() {
        assert_eq!(
            strokes(&[CTRL, SHIFT, ESCAPE]),
            vec![
                (CTRL, true),
                (SHIFT, true),
                (ESCAPE, true),
                (ESCAPE, false),
                (SHIFT, false),
                (CTRL, false),
            ]
        );
        assert!(COMBINATIONS.iter().all(|(_, keys)| !keys.is_empty()));
    }
}
