//! Persistent storage paths, permissions, and state JSON read/write.

use crate::domain::AppStateData;
use crate::logging;
use chrono::Utc;
use std::path::{Path, PathBuf};

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

/// Reads the saved state, or starts empty. A file that cannot be read
/// as state is moved aside rather than overwritten, so nothing is lost.
pub fn load() -> AppStateData {
    state_path().map_or_else(AppStateData::default, |path| load_from(&path))
}

pub fn load_from(path: &Path) -> AppStateData {
    let Ok(text) = std::fs::read_to_string(path) else {
        return AppStateData::default();
    };
    match serde_json::from_str(&text) {
        Ok(state) => state,
        Err(err) => {
            let aside = path.with_extension(format!(
                "json.unreadable-{}",
                Utc::now().format("%Y%m%d%H%M%S")
            ));
            logging::event(format_args!(
                "{} could not be read ({err}); moved it to {}",
                path.display(),
                aside.display()
            ));
            let _ = std::fs::rename(path, &aside);
            AppStateData::default()
        }
    }
}

/// Writes `json` (the serialized state) to [`state_path`], replacing
/// the previous file only once the new one is complete.
pub fn save(json: &str) -> std::io::Result<()> {
    match state_path() {
        Some(path) => save_to(&path, json),
        None => Ok(()),
    }
}

/// Readable only by the user (0600), replaced only once complete.
pub fn save_to(path: &Path, json: &str) -> std::io::Result<()> {
    remoport_rdp::write_private(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Appearance, DeviceBookmark, ViewMode};
    use uuid::Uuid;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("remoport-test-{name}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
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

        save_to(&path, &json).expect("save");
        let loaded = load_from(&path);

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
        let loaded = load_from(&dir.join("state.json"));
        assert!(loaded.bookmarks.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unreadable_file_is_moved_aside_not_lost() {
        let dir = scratch_dir("unreadable");
        let path = dir.join("state.json");
        std::fs::write(&path, "{ not json").expect("write");

        let loaded = load_from(&path);

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

        let loaded = load_from(&path);

        assert_eq!(loaded.view_mode, ViewMode::List);
        assert_eq!(loaded.appearance, Appearance::Light);
        assert!(path.exists(), "a readable file stays in place");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn current_state_json_snapshot_loads_without_format_drift() {
        let snapshot = r#"{
  "bookmarks": [
    {
      "id": "11111111-2222-4333-8444-555555555555",
      "pc_name": "workstation.example.com",
      "friendly_name": "Workstation",
      "group_name": "Work",
      "username": "DOMAIN\\user",
      "resolution": {
        "Custom": [
          1600,
          900
        ]
      },
      "color_quality": "High32Bit",
      "fit_to_window": false,
      "screens": 2,
      "scale_percent": 125,
      "hidpi_full_resolution": false,
      "audio_playback": "LocalDevice",
      "redirect_microphones": true,
      "redirect_cameras": false,
      "share_clipboard": true,
      "redirect_printers": false,
      "redirect_smartcards": false,
      "redirect_webauthn": false,
      "redirected_folders": [
        {
          "id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
          "path": "/home/user/Shared",
          "writable": true
        }
      ],
      "is_favorite": true,
      "last_connected": "2026-01-15T10:30:00Z",
      "allow_without_nla": false,
      "gateway": "gw.example.com",
      "gateway_username": "gwuser",
      "kerberos": true,
      "kdc": "kdc.example.com:88"
    }
  ],
  "remote_apps": [
    {
      "id": "22222222-3333-4444-8555-666666666666",
      "name": "Spreadsheet",
      "workspace_name": "Corp",
      "icon_name": "calc",
      "is_favorite": true,
      "last_used": "2026-01-10T08:00:00Z"
    }
  ],
  "user_account": {
    "id": "33333333-4444-4555-8666-777777777777",
    "display_name": "Local profile",
    "email": "Not signed in",
    "is_active": true
  },
  "view_mode": "List",
  "sort_option": "TimeLastConnected",
  "sort_ascending": false,
  "appearance": "Dark",
  "default_resolution": "Qhd1440p",
  "active_tab": "Favorites",
  "collapsed_groups": [
    "devices:Work"
  ]
}"#;
        let dir = scratch_dir("snapshot");
        let path = dir.join("state.json");
        std::fs::write(&path, snapshot).expect("write snapshot");

        let loaded = load_from(&path);
        assert_eq!(loaded.bookmarks.len(), 1);
        let pc = &loaded.bookmarks[0];
        assert_eq!(pc.pc_name, "workstation.example.com");
        assert_eq!(pc.friendly_name, "Workstation");
        assert_eq!(pc.group_name, "Work");
        assert_eq!(pc.username, "DOMAIN\\user");
        assert_eq!(pc.screens, 2);
        assert_eq!(pc.scale_percent, Some(125));
        assert!(!pc.fit_to_window);
        assert!(!pc.hidpi_full_resolution);
        assert!(pc.share_clipboard);
        assert!(pc.redirect_microphones);
        assert!(pc.is_favorite);
        assert_eq!(pc.gateway, "gw.example.com");
        assert_eq!(pc.gateway_username, "gwuser");
        assert!(pc.kerberos);
        assert_eq!(pc.kdc, "kdc.example.com:88");
        assert_eq!(pc.redirected_folders.len(), 1);
        assert!(pc.redirected_folders[0].writable);
        assert_eq!(loaded.appearance, Appearance::Dark);
        assert_eq!(loaded.view_mode, ViewMode::List);
        assert_eq!(
            serde_json::to_string_pretty(&loaded).expect("re-serialize"),
            snapshot
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
