//! Persisted app settings (`~/.config/Notes/config.json`).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub autostart_server: bool,
    pub markdown_mirror: bool,
    /// Which section the app opens on: "notes", "pinned" or "archived".
    /// Trash is deliberately not offered — opening into deleted notes is
    /// never what someone means by a home view.
    pub default_view: String,
    /// Days a note may sit in the trash before it's permanently swept on the
    /// next launch. `0` means "keep forever". Defaults to 30 days, matching what
    /// most notes apps do; a user who wants trash kept forever picks "Never".
    pub trash_retention_days: u32,
    /// Note-list sort field: "updated", "created", "title", "type" or "color".
    pub sort_field: String,
    /// Direction of `sort_field`. Ascending means oldest-first for dates,
    /// A→Z for titles, text-before-checklist for type.
    pub sort_ascending: bool,
    /// Closing the window hides it instead of quitting while the server is
    /// running, so the phone can keep syncing. No tray icon — relaunching the
    /// app raises the hidden window.
    pub run_in_background: bool,
    /// Client mode: the remote WebDAV server this machine syncs *to*, for a
    /// vault hosted elsewhere (another PC, a NAS, a hosted DAV account). Empty
    /// means client mode is off and this machine only hosts its own server.
    pub remote_url: String,
    pub remote_username: String,
    pub remote_password: String,
    /// Minutes between automatic syncs. `0` means manual only.
    pub sync_interval_minutes: u32,

    /// Saved per-network server configurations ("Home", "Work", "On road").
    /// When the active network changes and a profile's network matches, its
    /// server settings are applied automatically.
    pub server_profiles: Vec<ServerProfile>,
}

/// One saved server configuration, remembered together with the network it
/// was created on so it can be re-applied when that network comes back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerProfile {
    pub name: String,
    /// Active connection name (NetworkManager) or SSID when it was saved.
    pub network: String,
    pub bind: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

impl Default for ServerProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            network: String::new(),
            bind: "127.0.0.1".to_string(),
            port: 8080,
            username: String::new(),
            password: String::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: crate::store::default_data_dir(),
            // Loopback by default: exposing notes on the LAN is opt-in.
            bind: "127.0.0.1".to_string(),
            port: 8080,
            username: "noteuser".to_string(),
            password: String::new(),
            autostart_server: false,
            markdown_mirror: true,
            default_view: "notes".to_string(),
            trash_retention_days: 30,
            sort_field: "updated".to_string(),
            sort_ascending: false,
            run_in_background: false,
            remote_url: String::new(),
            remote_username: String::new(),
            remote_password: String::new(),
            sync_interval_minutes: 0,
            server_profiles: Vec::new(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        crate::store::config_dir().join("config.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        // The file holds the WebDAV password in plaintext, so keep it owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// The retention choices offered in settings, paired as (label, days).
    /// `0` days is "Never" (keep forever).
    pub const TRASH_RETENTION_OPTIONS: [(&'static str, u32); 4] = [
        ("Never", 0),
        ("After 7 days", 7),
        ("After 30 days", 30),
        ("After 90 days", 90),
    ];

    /// Dropdown index for the configured retention window. An unrecognised value
    /// (e.g. hand-edited config) falls back to "Never".
    pub fn trash_retention_index(&self) -> u32 {
        Self::TRASH_RETENTION_OPTIONS
            .iter()
            .position(|(_, d)| *d == self.trash_retention_days)
            .unwrap_or(0) as u32
    }

    /// Maps a dropdown index back to a retention window in days.
    pub fn trash_retention_days_for(index: u32) -> u32 {
        Self::TRASH_RETENTION_OPTIONS
            .get(index as usize)
            .map(|(_, d)| *d)
            .unwrap_or(0)
    }

    /// Sidebar row index for the configured default view.
    pub fn default_view_index(&self) -> i32 {
        match self.default_view.as_str() {
            "pinned" => 1,
            "archived" => 2,
            _ => 0,
        }
    }

    /// A client for the configured remote, or `None` when client mode is off.
    pub fn remote_client(&self) -> Option<crate::webdav::Client> {
        let url = self.remote_url.trim();
        (!url.is_empty()).then(|| {
            crate::webdav::Client::new(url, &self.remote_username, &self.remote_password)
        })
    }

    /// The automatic-sync choices offered in settings, paired as (label, minutes).
    pub const SYNC_INTERVAL_OPTIONS: [(&'static str, u32); 4] = [
        ("Manually only", 0),
        ("Every 5 minutes", 5),
        ("Every 15 minutes", 15),
        ("Every hour", 60),
    ];

    /// Dropdown index for the configured sync interval; an unrecognised value
    /// (hand-edited config) falls back to manual.
    pub fn sync_interval_index(&self) -> u32 {
        Self::SYNC_INTERVAL_OPTIONS
            .iter()
            .position(|(_, m)| *m == self.sync_interval_minutes)
            .unwrap_or(0) as u32
    }

    /// Maps a dropdown index back to an interval in minutes.
    pub fn sync_interval_for(index: u32) -> u32 {
        Self::SYNC_INTERVAL_OPTIONS
            .get(index as usize)
            .map(|(_, m)| *m)
            .unwrap_or(0)
    }

    /// Snapshot of the current server fields as a named profile.
    pub fn profile_from_current(&self, name: &str, network: &str) -> ServerProfile {
        ServerProfile {
            name: name.to_string(),
            network: network.to_string(),
            bind: self.bind.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self.password.clone(),
        }
    }

    /// Copies a profile's server fields into the live config. Returns whether
    /// anything actually changed (callers restart the server only then).
    pub fn apply_profile(&mut self, profile: &ServerProfile) -> bool {
        let changed = self.bind != profile.bind
            || self.port != profile.port
            || self.username != profile.username
            || self.password != profile.password;
        self.bind = profile.bind.clone();
        self.port = profile.port;
        self.username = profile.username.clone();
        self.password = profile.password.clone();
        changed
    }

    /// The saved profile recorded on the given network, if any.
    pub fn profile_for_network(&self, network: &str) -> Option<&ServerProfile> {
        (!network.is_empty())
            .then(|| self.server_profiles.iter().find(|p| p.network == network))
            .flatten()
    }

    pub fn server_config(&self) -> crate::server::ServerConfig {
        crate::server::ServerConfig {
            bind: self.bind.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self.password.clone(),
            root: self.data_dir.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_maps_to_sidebar_rows() {
        let view = |v: &str| {
            Config {
                default_view: v.into(),
                ..Default::default()
            }
            .default_view_index()
        };
        assert_eq!(view("notes"), 0);
        assert_eq!(view("pinned"), 1);
        assert_eq!(view("archived"), 2);
        // Trash isn't offered, and anything unrecognised must not strand the
        // user in an empty section.
        assert_eq!(view("trash"), 0);
        assert_eq!(view("nonsense"), 0);
    }

    #[test]
    fn trash_retention_index_round_trips() {
        for (i, (_, days)) in Config::TRASH_RETENTION_OPTIONS.iter().enumerate() {
            let cfg = Config {
                trash_retention_days: *days,
                ..Default::default()
            };
            assert_eq!(cfg.trash_retention_index(), i as u32);
            assert_eq!(Config::trash_retention_days_for(i as u32), *days);
        }
        // A hand-edited value that's not on the menu falls back to "Never" (0).
        let odd = Config {
            trash_retention_days: 13,
            ..Default::default()
        };
        assert_eq!(odd.trash_retention_index(), 0);
        // An out-of-range dropdown index is also "Never", never a panic.
        assert_eq!(Config::trash_retention_days_for(99), 0);
    }

    #[test]
    fn profiles_round_trip_and_apply() {
        let mut cfg = Config {
            bind: "0.0.0.0".into(),
            port: 9999,
            username: "casa".into(),
            password: "secret".into(),
            ..Default::default()
        };
        let home = cfg.profile_from_current("Home", "MyWifi");
        cfg.server_profiles.push(home.clone());

        // Serde round trip keeps the profile.
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.server_profiles, vec![home.clone()]);

        // Applying over different values changes them and reports the change.
        let mut other = Config::default();
        assert!(other.apply_profile(&home));
        assert_eq!(other.port, 9999);
        assert_eq!(other.username, "casa");
        // Applying again is a no-op.
        assert!(!other.apply_profile(&home));

        // Network lookup: exact match only, empty never matches.
        assert!(cfg.profile_for_network("MyWifi").is_some());
        assert!(cfg.profile_for_network("Other").is_none());
        assert!(cfg.profile_for_network("").is_none());
    }

    #[test]
    fn a_config_missing_new_keys_still_loads() {
        // Older config files won't have `default_view`; serde(default) on the
        // struct must fill it rather than failing the whole load.
        let old = r#"{"data_dir":"/tmp/x","port":8080,"username":"u"}"#;
        let cfg: Config = serde_json::from_str(old).unwrap();
        assert_eq!(cfg.default_view, "notes");
        assert_eq!(cfg.port, 8080);
    }
}
