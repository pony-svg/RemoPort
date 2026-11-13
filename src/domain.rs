//! Domain model: saved PCs, application preferences, and display rules.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Light or dark look, chosen in Settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Appearance {
    Dark,
    #[default]
    Light,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum NavTab {
    Favorites,
    #[default]
    Devices,
    Apps,
    Settings,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    Grid,
    List,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum SortOption {
    #[default]
    DeviceName,
    TimeLastConnected,
    Manual,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResolutionOption {
    #[default]
    Native,
    Fhd1080p,
    Qhd1440p,
    Uhd4k,
    Custom(u32, u32),
}

impl ResolutionOption {
    /// The desktop's size when it stays the same whatever the window.
    pub fn fixed_size(&self) -> Option<(u16, u16)> {
        match *self {
            Self::Native => None,
            Self::Fhd1080p => Some((1920, 1080)),
            Self::Qhd1440p => Some((2560, 1440)),
            Self::Uhd4k => Some((3840, 2160)),
            Self::Custom(width, height) => Some((
                u16::try_from(width).unwrap_or(u16::MAX),
                u16::try_from(height).unwrap_or(u16::MAX),
            )),
        }
    }

    /// Whether the desktop counts this screen's points instead of its pixels:
    /// with full resolution off, a smaller desktop that this computer enlarges.
    pub fn uses_points(&self, hidpi_full_resolution: bool) -> bool {
        !hidpi_full_resolution && self.fixed_size().is_none()
    }

    /// The display scale for a PC with none set: this screen's, unless this
    /// computer is already doing the enlarging.
    pub fn automatic_scale(&self, hidpi_full_resolution: bool, screen_scale: u32) -> u32 {
        if self.uses_points(hidpi_full_resolution) {
            100
        } else {
            screen_scale
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ColorQuality {
    High32Bit,
    Medium16Bit,
}

impl ColorQuality {
    pub fn bits(self) -> u32 {
        match self {
            Self::High32Bit => 32,
            Self::Medium16Bit => 16,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AudioPlaybackMode {
    LocalDevice,
    RemotePC,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectedFolder {
    pub id: Uuid,
    pub path: String,
    /// The PC may change what is in the folder. Off unless the user turns it
    /// on; folders saved before this existed come back read-only.
    #[serde(default)]
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceBookmark {
    pub id: Uuid,
    pub pc_name: String,
    pub friendly_name: String,
    pub group_name: String,
    pub username: String,
    pub resolution: ResolutionOption,
    pub color_quality: ColorQuality,
    pub fit_to_window: bool,
    /// How many of this computer's screens the session uses, one window each.
    #[serde(default = "one_screen")]
    pub screens: u8,
    /// The display scale for this PC as a percentage; without one it follows
    /// this computer's screen.
    #[serde(default)]
    pub scale_percent: Option<u16>,
    pub hidpi_full_resolution: bool,
    pub audio_playback: AudioPlaybackMode,
    pub redirect_microphones: bool,
    pub redirect_cameras: bool,
    /// Shares the clipboard's text with this PC. Off until the user turns it
    /// on; files saved before Remoport had a clipboard come back with it off.
    #[serde(default)]
    pub share_clipboard: bool,
    pub redirect_printers: bool,
    pub redirect_smartcards: bool,
    pub redirect_webauthn: bool,
    pub redirected_folders: Vec<RedirectedFolder>,
    pub is_favorite: bool,
    pub last_connected: Option<DateTime<Utc>>,
    /// Lets this PC sign in on its own screen when it does not offer Network
    /// Level Authentication. Off unless the user turns it on for this PC.
    #[serde(default)]
    pub allow_without_nla: bool,
    /// The RD Gateway this PC is reached through (`host` or `host:port`);
    /// empty to connect directly.
    #[serde(default)]
    pub gateway: String,
    /// The gateway's own account; empty to sign in to it with the PC's.
    #[serde(default)]
    pub gateway_username: String,
    /// Signs in with Kerberos rather than NTLM.
    #[serde(default)]
    pub kerberos: bool,
    /// The KDC for Kerberos; empty to find it.
    #[serde(default)]
    pub kdc: String,
}

fn one_screen() -> u8 {
    1
}

impl DeviceBookmark {
    pub fn new(pc_name: String, friendly_name: String, group_name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            pc_name,
            friendly_name,
            group_name: if group_name.is_empty() {
                "Saved PCs".to_string()
            } else {
                group_name
            },
            username: "".to_string(),
            resolution: ResolutionOption::Native,
            color_quality: ColorQuality::High32Bit,
            fit_to_window: true,
            screens: 1,
            scale_percent: None,
            hidpi_full_resolution: true,
            audio_playback: AudioPlaybackMode::LocalDevice,
            redirect_microphones: false,
            redirect_cameras: false,
            share_clipboard: false,
            redirect_printers: false,
            redirect_smartcards: false,
            redirect_webauthn: false,
            redirected_folders: Vec::new(),
            is_favorite: false,
            last_connected: None,
            allow_without_nla: false,
            gateway: String::new(),
            gateway_username: String::new(),
            kerberos: false,
            kdc: String::new(),
        }
    }

    pub fn display_title(&self) -> &str {
        if !self.friendly_name.trim().is_empty() {
            &self.friendly_name
        } else {
            &self.pc_name
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAppItem {
    pub id: Uuid,
    pub name: String,
    pub workspace_name: String,
    pub icon_name: String,
    pub is_favorite: bool,
    pub last_used: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAccount {
    pub id: Uuid,
    pub display_name: String,
    pub email: String,
    pub is_active: bool,
}

/// Everything Remoport remembers between runs, saved as JSON in the state
/// file. Fields added later fall back to their defaults when an older file is
/// read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppStateData {
    pub bookmarks: Vec<DeviceBookmark>,
    pub remote_apps: Vec<RemoteAppItem>,
    pub user_account: UserAccount,
    pub view_mode: ViewMode,
    pub sort_option: SortOption,
    pub sort_ascending: bool,
    pub appearance: Appearance,
    /// Resolution a newly added PC starts with (Settings → New PCs).
    pub default_resolution: ResolutionOption,
    pub active_tab: NavTab,
    /// Groups the user folded away on the Devices page.
    pub collapsed_groups: Vec<String>,
}

impl Default for AppStateData {
    fn default() -> Self {
        Self {
            bookmarks: Vec::new(),
            remote_apps: Vec::new(),
            user_account: UserAccount {
                id: Uuid::new_v4(),
                display_name: "Local profile".to_string(),
                email: "Not signed in".to_string(),
                is_active: true,
            },
            view_mode: ViewMode::default(),
            sort_option: SortOption::default(),
            sort_ascending: true,
            appearance: Appearance::default(),
            default_resolution: ResolutionOption::default(),
            active_tab: NavTab::default(),
            collapsed_groups: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_shared_with_a_pc_unless_the_user_turns_it_on() {
        let pc = DeviceBookmark::new("pc".into(), String::new(), String::new());
        assert!(!pc.share_clipboard && !pc.redirect_microphones && !pc.redirect_webauthn);
        // A PC saved before the clipboard existed had it ticked by default:
        // it comes back off.
        let mut old = serde_json::to_value(&pc).expect("serialize");
        let fields = old.as_object_mut().expect("object");
        fields.remove("share_clipboard");
        fields.insert("redirect_clipboard".into(), true.into());
        let loaded: DeviceBookmark = serde_json::from_value(old).expect("deserialize");
        assert!(!loaded.share_clipboard);
    }

    #[test]
    fn full_resolution_off_only_matters_without_a_fixed_size() {
        let native = ResolutionOption::Native;
        let fixed = ResolutionOption::Fhd1080p;
        assert_eq!(fixed.fixed_size(), Some((1920, 1080)));
        assert_eq!(native.fixed_size(), None);
        assert!(native.uses_points(false));
        assert!(!native.uses_points(true));
        assert!(!fixed.uses_points(false));
        assert_eq!(native.automatic_scale(true, 125), 125);
        assert_eq!(native.automatic_scale(false, 125), 100);
        assert_eq!(fixed.automatic_scale(false, 125), 125);
    }

    #[test]
    fn a_pc_saved_before_follows_this_screens_scale() {
        let pc = DeviceBookmark::new("pc".into(), String::new(), String::new());
        let mut old = serde_json::to_value(&pc).expect("to JSON");
        old.as_object_mut()
            .expect("an object")
            .remove("scale_percent");
        let read: DeviceBookmark = serde_json::from_value(old).expect("from JSON");
        assert_eq!(read.scale_percent, None);
    }

    #[test]
    fn folders_saved_before_are_shared_read_only() {
        let old = r#"{"id":"6f1b8a2e-3c4d-4e5f-8a9b-0c1d2e3f4a5b","path":"/home/me/Documents","read_only":false}"#;
        let folder: RedirectedFolder = serde_json::from_str(old).expect("deserialize");
        assert!(!folder.writable);
    }
}
