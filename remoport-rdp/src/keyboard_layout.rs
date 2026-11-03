//! Keyboard layout detection, from RustConn's (`rustconn-core`, GPL-3.0-or-later).
//!
//! RDP sends key positions; the server turns them into characters with the
//! layout the client announces. A wrong layout types wrong characters with
//! nothing pointing at the cause, so the source of the answer is reported.
//!
//! 1. `XKB_DEFAULT_LAYOUT`
//! 2. `localectl status`, preferring `X11 Layout` over `VC Keymap`
//! 3. US English (`0x0409`)

use std::process::Command;

/// US English keyboard layout (fallback default)
pub const LAYOUT_US_ENGLISH: u32 = 0x0409;

/// Values `localectl` prints for a setting that is not configured.
const LOCALECTL_PLACEHOLDERS: [&str; 2] = ["(unset)", "n/a"];

/// The Windows keyboard layout identifier (KLID) for this machine's layout,
/// with a short note on where it came from.
#[must_use]
pub fn detect_keyboard_layout() -> (u32, String) {
    let from_env = std::env::var("XKB_DEFAULT_LAYOUT").ok();
    if let Some(list) = from_env.as_deref()
        && let Some((name, klid)) = first_known_layout(list)
    {
        return (klid, format!("{name} from XKB_DEFAULT_LAYOUT"));
    }
    let from_localectl = detect_from_localectl();
    if let Some(list) = from_localectl.as_deref()
        && let Some((name, klid)) = first_known_layout(list)
    {
        return (klid, format!("{name} from localectl"));
    }
    (
        LAYOUT_US_ENGLISH,
        "US English, no known layout was found".to_string(),
    )
}

/// Returns the first layout in a comma-separated list that maps to a KLID.
///
/// Both sources report the whole group list of a machine that toggles between
/// layouts, e.g. `us,ua`. The first entry is the primary one, but it may be a
/// layout [`xkb_name_to_klid`] does not know — and then a later entry is a
/// better answer than the US English fallback.
fn first_known_layout(list: &str) -> Option<(&str, u32)> {
    list.split(',')
        .map(str::trim)
        .find_map(|name| xkb_name_to_klid(name).map(|klid| (name, klid)))
}

/// Reads `localectl status` and returns the layout list it reports.
fn detect_from_localectl() -> Option<String> {
    let output = Command::new("localectl").arg("status").output().ok()?;

    if !output.status.success() {
        return None;
    }

    parse_localectl_status(&String::from_utf8_lossy(&output.stdout))
}

/// Extracts the layout list from `localectl status` output.
///
/// `X11 Layout` wins over `VC Keymap` wherever it appears, and the placeholders
/// systemd prints for an unset value are ignored. Reading the first of the two
/// lines instead is what silently sent US English to the server on any machine
/// whose console keymap was unconfigured — which is the normal state of a
/// desktop, and it is printed first.
///
/// The value is returned as printed and may be a comma-separated list.
fn parse_localectl_status(stdout: &str) -> Option<String> {
    let mut vc_keymap = None;

    for line in stdout.lines() {
        let trimmed = line.trim();

        if let Some(value) = trimmed.strip_prefix("X11 Layout:")
            && let Some(layout) = usable_localectl_value(value)
        {
            return Some(layout);
        }

        if vc_keymap.is_none()
            && let Some(value) = trimmed.strip_prefix("VC Keymap:")
        {
            vc_keymap = usable_localectl_value(value);
        }
    }

    vc_keymap
}

/// Trims a `localectl` field value, rejecting empties and placeholders.
fn usable_localectl_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || LOCALECTL_PLACEHOLDERS
            .iter()
            .any(|placeholder| value.eq_ignore_ascii_case(placeholder))
    {
        return None;
    }
    Some(value.to_string())
}

/// Maps an XKB layout name to a Windows keyboard layout identifier (KLID).
///
/// Covers the most common layouts. Returns `None` for unknown layouts.
///
/// # Arguments
///
/// * `name` - XKB layout name (e.g. "de", "fr", "us")
#[must_use]
pub fn xkb_name_to_klid(name: &str) -> Option<u32> {
    // Map of XKB layout names to Windows KLIDs
    // Reference: https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/default-input-locales-for-windows-language-packs
    match name {
        "us" => Some(0x0409),
        "gb" | "uk" => Some(0x0809),
        "de" => Some(0x0407),
        "fr" => Some(0x040C),
        "es" => Some(0x040A),
        "it" => Some(0x0410),
        "pt" => Some(0x0816),
        "br" => Some(0x0416),
        "nl" => Some(0x0413),
        "be" => Some(0x080C), // French - Belgium
        "ch" => Some(0x0807), // German - Switzerland
        "at" => Some(0x0C07), // German - Austria
        "se" => Some(0x041D),
        "no" => Some(0x0414),
        "dk" => Some(0x0406),
        "fi" => Some(0x040B),
        "pl" => Some(0x0415),
        "cz" => Some(0x0405),
        "sk" => Some(0x041B),
        "hu" => Some(0x040E),
        "ro" => Some(0x0418),
        "bg" => Some(0x0402),
        "hr" => Some(0x041A),
        "si" => Some(0x0424),
        "rs" | "sr" => Some(0x081A),
        "ru" => Some(0x0419),
        "ua" => Some(0x0422),
        "by" => Some(0x0423),
        "tr" => Some(0x041F),
        "gr" | "el" => Some(0x0408),
        "il" | "he" => Some(0x040D),
        "ar" => Some(0x0401),
        "jp" => Some(0x0411),
        "kr" | "ko" => Some(0x0412),
        "cn" | "zh" => Some(0x0804),
        "tw" => Some(0x0404),
        "th" => Some(0x041E),
        "in" => Some(0x0439), // Hindi
        "ie" => Some(0x1809), // Irish English
        "is" => Some(0x040F),
        "ee" => Some(0x0425),
        "lt" => Some(0x0427),
        "lv" => Some(0x0426),
        "latam" => Some(0x080A), // Latin American Spanish
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xkb_name_to_klid_common_layouts() {
        assert_eq!(xkb_name_to_klid("us"), Some(0x0409));
        assert_eq!(xkb_name_to_klid("de"), Some(0x0407));
        assert_eq!(xkb_name_to_klid("fr"), Some(0x040C));
        assert_eq!(xkb_name_to_klid("gb"), Some(0x0809));
        assert_eq!(xkb_name_to_klid("ru"), Some(0x0419));
        assert_eq!(xkb_name_to_klid("ua"), Some(0x0422));
        assert_eq!(xkb_name_to_klid("jp"), Some(0x0411));
    }

    #[test]
    fn test_xkb_name_to_klid_unknown() {
        assert_eq!(xkb_name_to_klid("unknown_layout"), None);
        assert_eq!(xkb_name_to_klid(""), None);
    }

    #[test]
    fn test_xkb_name_to_klid_aliases() {
        // UK alias
        assert_eq!(xkb_name_to_klid("uk"), Some(0x0809));
        // Serbian aliases
        assert_eq!(xkb_name_to_klid("rs"), Some(0x081A));
        assert_eq!(xkb_name_to_klid("sr"), Some(0x081A));
    }

    #[test]
    fn test_detect_keyboard_layout_returns_valid() {
        let (klid, source) = detect_keyboard_layout();
        // Should always return a valid KLID (at minimum the US fallback)
        assert!(klid > 0);
        assert!(!source.is_empty());
    }

    /// The defect this parser was rewritten for.
    ///
    /// `localectl status` prints `VC Keymap` above `X11 Layout`, and on a
    /// desktop the console keymap is normally unconfigured. Reading whichever
    /// of the two lines came first therefore answered `(unset)`, which maps to
    /// no KLID, and the German machine got US English sent to the server with
    /// nothing but a debug line about "detection failed" to show for it.
    #[test]
    fn an_unset_console_keymap_does_not_mask_the_graphical_layout() {
        let status = "\
   System Locale: LANG=de_DE.UTF-8
       VC Keymap: (unset)
      X11 Layout: de
       X11 Model: pc105
";
        assert_eq!(parse_localectl_status(status).as_deref(), Some("de"));
    }

    #[test]
    fn the_older_placeholder_spelling_is_rejected_too() {
        // systemd printed `n/a` before it printed `(unset)`.
        let status = "    VC Keymap: n/a\n   X11 Layout: fr\n";
        assert_eq!(parse_localectl_status(status).as_deref(), Some("fr"));
    }

    #[test]
    fn a_console_keymap_is_used_when_there_is_no_graphical_one() {
        // A headless or console-only machine reports no X11 layout at all.
        let status = "   System Locale: LANG=C\n       VC Keymap: pl\n";
        assert_eq!(parse_localectl_status(status).as_deref(), Some("pl"));
    }

    #[test]
    fn both_sources_unset_yields_nothing_to_go_on() {
        let status = "       VC Keymap: (unset)\n      X11 Layout: (unset)\n";
        assert_eq!(parse_localectl_status(status), None);
    }

    #[test]
    fn the_group_list_is_returned_whole() {
        // The real output of a machine toggling between two layouts.
        let status = "\
   System Locale: LANG=uk_UA.UTF-8
       VC Keymap: (unset)
      X11 Layout: us,ua
     X11 Variant: ,
";
        assert_eq!(parse_localectl_status(status).as_deref(), Some("us,ua"));
    }

    #[test]
    fn the_primary_layout_wins_when_it_is_known() {
        assert_eq!(first_known_layout("us,ua"), Some(("us", 0x0409)));
        assert_eq!(first_known_layout("ua,us"), Some(("ua", 0x0422)));
    }

    #[test]
    fn an_unknown_primary_falls_through_to_the_next_group() {
        // Better than answering US English for a machine that named a layout.
        assert_eq!(first_known_layout("apl, de"), Some(("de", 0x0407)));
        assert_eq!(first_known_layout("apl,epo"), None);
        assert_eq!(first_known_layout(""), None);
    }
}
