use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NavTab {
    Home,
    Devices,
    Apps,
    Settings,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    List,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SortOption {
    DeviceName,
    TimeLastConnected,
    Manual,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResolutionOption {
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
    pub redirect_clipboard: bool,
    pub redirect_printers: bool,
    pub redirect_smartcards: bool,
    pub redirect_webauthn: bool,
    pub redirected_folders: Vec<RedirectedFolder>,
    pub is_favorite: bool,
    pub last_connected: Option<DateTime<Utc>>,
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
            redirect_clipboard: true,
            redirect_printers: false,
            redirect_smartcards: false,
            redirect_webauthn: true,
            redirected_folders: Vec::new(),
            is_favorite: false,
            last_connected: None,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStateData {
    pub bookmarks: Vec<DeviceBookmark>,
    pub remote_apps: Vec<RemoteAppItem>,
    pub user_account: UserAccount,
    pub view_mode: ViewMode,
    pub sort_option: SortOption,
    pub sort_ascending: bool,
}

impl Default for AppStateData {
    fn default() -> Self {
        let mut sample_pc1 = DeviceBookmark::new(
            "workstation.example.com".to_string(),
            "Office workstation".to_string(),
            "Work PCs".to_string(),
        );
        sample_pc1.is_favorite = true;
        sample_pc1.last_connected = Some(Utc::now());

        let mut sample_pc2 = DeviceBookmark::new(
            "build01.example.com".to_string(),
            "Build server".to_string(),
            "Work PCs".to_string(),
        );
        sample_pc2.last_connected = Some(Utc::now());

        let sample_apps = vec![
            RemoteAppItem {
                id: Uuid::new_v4(),
                name: "Terminal".to_string(),
                workspace_name: "Example workspace".to_string(),
                icon_name: "terminal".to_string(),
                is_favorite: true,
                last_used: Some(Utc::now()),
            },
            RemoteAppItem {
                id: Uuid::new_v4(),
                name: "Files".to_string(),
                workspace_name: "Example workspace".to_string(),
                icon_name: "folder".to_string(),
                is_favorite: false,
                last_used: None,
            },
            RemoteAppItem {
                id: Uuid::new_v4(),
                name: "Web browser".to_string(),
                workspace_name: "Example workspace".to_string(),
                icon_name: "browser".to_string(),
                is_favorite: false,
                last_used: None,
            },
        ];

        Self {
            bookmarks: vec![sample_pc1, sample_pc2],
            remote_apps: sample_apps,
            user_account: UserAccount {
                id: Uuid::new_v4(),
                display_name: "Local profile".to_string(),
                email: "Not signed in".to_string(),
                is_active: true,
            },
            view_mode: ViewMode::Grid,
            sort_option: SortOption::DeviceName,
            sort_ascending: true,
        }
    }
}
