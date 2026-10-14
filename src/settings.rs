//! Persistent user settings (JSON in the OS config dir) and WiFi passwords (OS keychain).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "NexArm";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConnMethod {
    #[default]
    Serial,
    Wifi,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub lang: String,
    pub method: ConnMethod,
    pub serial_port: Option<String>,
    pub baud: u32,
    pub last_ssid: String,
    pub wifi_host: String,
    pub wifi_port: u16,
    /// Global motion acceleration from the System page (1..=254).
    pub move_acc: u8,
    pub sync: SyncSettings,
    pub engine: crate::engine::supervisor::EngineSettings,
}

/// Synchronizer page values (the original kept them in `sync_host_gui_config.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    pub center_pos: u16,
    pub selected_servo: u8,
    pub offsets: [i16; 6],
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self { center_pos: crate::sync::DEFAULT_CENTER_POS, selected_servo: 1, offsets: [0; 6] }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lang: "en".into(),
            method: ConnMethod::Serial,
            serial_port: None,
            baud: 1_000_000,
            last_ssid: String::new(),
            wifi_host: "192.168.4.1".into(),
            wifi_port: 8080,
            move_acc: 254,
            sync: SyncSettings::default(),
            engine: Default::default(),
        }
    }
}

impl Settings {
    /// Load settings, falling back to defaults when the file is missing or unreadable.
    pub fn load() -> Settings {
        std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::write(settings_path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// `<OS config dir>/NexArm` (or `$NEXARM_CONFIG_DIR`, which the UI tests use), created on demand.
pub fn config_dir() -> PathBuf {
    let dir = match std::env::var_os("NEXARM_CONFIG_DIR") {
        Some(p) => PathBuf::from(p),
        None => dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("NexArm"),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

/// Store the password for `ssid` in the OS keychain; an empty password removes it.
pub fn save_wifi_password(ssid: &str, password: &str) -> anyhow::Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, ssid)?;
    if password.is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    } else {
        entry.set_password(password)?;
        Ok(())
    }
}

pub fn load_wifi_password(ssid: &str) -> Option<String> {
    if ssid.is_empty() {
        return None;
    }
    keyring::Entry::new(KEYRING_SERVICE, ssid).ok()?.get_password().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_round_trip() {
        let s = Settings {
            lang: "en".into(),
            method: ConnMethod::Wifi,
            serial_port: Some("COM5".into()),
            baud: 115_200,
            last_ssid: "NexArm_AP".into(),
            wifi_host: "10.0.0.2".into(),
            wifi_port: 9000,
            move_acc: 100,
            sync: SyncSettings { center_pos: 1000, selected_servo: 4, offsets: [-2047, -1, 0, 1, 2047, 5] },
            engine: crate::engine::supervisor::EngineSettings { profile: "bench".into(), ..Default::default() },
        };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"lang":"en"}"#).unwrap();
        assert_eq!(s.lang, "en");
        assert_eq!(s.baud, 1_000_000);
        assert_eq!(s.wifi_port, 8080);
        assert_eq!(s.method, ConnMethod::Serial);
        assert_eq!(s.sync, SyncSettings::default());
        let s: Settings = serde_json::from_str(r#"{"sync":{"center_pos":100}}"#).unwrap();
        assert_eq!((s.sync.center_pos, s.sync.selected_servo), (100, 1));
    }

    /// Touches the real OS keychain, so it only runs on request (`--ignored`).
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let ssid = "nexarm-desktop-selftest";
        save_wifi_password(ssid, "selftest-value").unwrap();
        assert_eq!(load_wifi_password(ssid).as_deref(), Some("selftest-value"));
        save_wifi_password(ssid, "").unwrap();
        assert_eq!(load_wifi_password(ssid), None);
    }
}
