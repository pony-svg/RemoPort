use crate::theme::Appearance;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

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
    pub fn display_label(&self) -> String {
        match self {
            Self::Native => "Default for this display".to_string(),
            Self::Fhd1080p => "1920 x 1080 (1080p)".to_string(),
            Self::Qhd1440p => "2560 x 1440 (1440p)".to_string(),
            Self::Uhd4k => "3840 x 2160 (4K UHD)".to_string(),
            Self::Custom(w, h) => format!("{} x {} (Custom)", w, h),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ColorQuality {
    High32Bit,
    Medium16Bit,
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
    pub read_only: bool,
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
    pub use_all_monitors: bool,
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
            use_all_monitors: false,
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

/// Everything Remoport remembers between runs, saved as JSON in
/// [`state_path`]. Fields added later fall back to their defaults when an
/// older file is read.
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

/// Remoport's configuration directory: `$REMOPORT_CONFIG_DIR`, else
/// `$XDG_CONFIG_HOME/remoport`, else `~/.config/remoport`.
pub fn config_dir() -> Option<PathBuf> {
    Some(match std::env::var_os("REMOPORT_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => match std::env::var_os("XDG_CONFIG_HOME") {
            Some(config) => PathBuf::from(config).join("remoport"),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".config/remoport"),
        },
    })
}

/// Where the state is kept, in [`config_dir`].
pub fn state_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("state.json"))
}

/// Where the certificates the user trusted are kept, in [`config_dir`].
pub fn trust_store_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("trusted_servers.json"))
}

/// Creates [`config_dir`] if needed and makes it private (0700): it holds the
/// list of PCs and the certificates the user trusted.
pub fn secure_config_dir() -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(dir) = config_dir() else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
}

impl AppStateData {
    /// Reads the saved state, or starts empty. A file that cannot be read
    /// as state is moved aside rather than overwritten, so nothing is lost.
    pub fn load() -> Self {
        state_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    fn load_from(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(state) => state,
            Err(err) => {
                let aside = path.with_extension(format!(
                    "json.unreadable-{}",
                    Utc::now().format("%Y%m%d%H%M%S")
                ));
                crate::logging::event(format_args!(
                    "{} could not be read ({err}); moved it to {}",
                    path.display(),
                    aside.display()
                ));
                let _ = std::fs::rename(path, &aside);
                Self::default()
            }
        }
    }

    /// Writes `json` (this state, serialized) to [`state_path`], replacing
    /// the previous file only once the new one is complete.
    pub fn save(json: &str) -> std::io::Result<()> {
        match state_path() {
            Some(path) => Self::save_to(&path, json),
            None => Ok(()),
        }
    }

    /// Readable only by the user (0600), replaced only once complete.
    fn save_to(path: &Path, json: &str) -> std::io::Result<()> {
        remoport_rdp::write_private(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("remoport-test-{name}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

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
    fn saved_state_reads_back_the_same() {
        let dir = scratch_dir("roundtrip");
        let path = dir.join("state.json");
        let mut state = AppStateData::default();
        state.bookmarks.push(DeviceBookmark::new(
            "pc.example.com".into(),
            "Office".into(),
            "Work".into(),
        ));
        state.appearance = Appearance::Dark;
        state.collapsed_groups.push("devices:Work".into());
        let json = serde_json::to_string_pretty(&state).expect("serialize");

        AppStateData::save_to(&path, &json).expect("save");
        let loaded = AppStateData::load_from(&path);

        assert_eq!(
            serde_json::to_string_pretty(&loaded).expect("serialize"),
            json
        );
        assert!(!path.with_extension("partial").exists());
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the state is private");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_starts_empty() {
        let dir = scratch_dir("missing");
        let loaded = AppStateData::load_from(&dir.join("state.json"));
        assert!(loaded.bookmarks.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unreadable_file_is_moved_aside_not_lost() {
        let dir = scratch_dir("unreadable");
        let path = dir.join("state.json");
        std::fs::write(&path, "{ not json").expect("write");

        let loaded = AppStateData::load_from(&path);

        assert!(loaded.bookmarks.is_empty());
        assert!(
            !path.exists(),
            "the broken file must not stay where it gets overwritten"
        );
        let kept: Vec<_> = std::fs::read_dir(&dir)
            .expect("list")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains("unreadable"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            std::fs::read_to_string(kept[0].path()).expect("read"),
            "{ not json"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_older_file_without_new_fields_still_loads() {
        let dir = scratch_dir("older");
        let path = dir.join("state.json");
        std::fs::write(&path, r#"{"bookmarks": [], "view_mode": "List"}"#).expect("write");

        let loaded = AppStateData::load_from(&path);

        assert_eq!(loaded.view_mode, ViewMode::List);
        assert_eq!(loaded.appearance, Appearance::Light);
        assert!(path.exists(), "a readable file stays in place");
        let _ = std::fs::remove_dir_all(dir);
    }
}
