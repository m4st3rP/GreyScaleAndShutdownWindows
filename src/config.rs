//! Persistent configuration stored in %APPDATA%\grayscale-timer\config.json.
//!
//! On first run a default file is created there so the user can open it in
//! Notepad (via the tray menu) and edit it.  All fields are optional in the
//! JSON: missing fields keep their default values thanks to
//! `#[serde(default)]`.

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

// ── Per-notification entry ────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Notification {
    /// How many minutes before the shutdown time this balloon tip fires.
    pub minutes_before: u32,
    /// Text shown in the balloon tip body.
    pub message: String,
}

// ── Main config ───────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// 24-hour time at which the greyscale filter is enabled, e.g. `"22:00"`.
    pub grayscale_time: String,
    /// 24-hour time at which the greyscale filter is disabled, e.g. `"07:00"`.
    pub grayscale_disable_time: String,
    /// Whether the greyscale timer is active.
    pub grayscale_enabled: bool,

    /// 24-hour time at which the computer shuts down, e.g. `"23:30"`.
    pub shutdown_time: String,
    /// Whether the shutdown timer is active.
    pub shutdown_enabled: bool,

    /// If true, activate the greyscale filter immediately when the programme
    /// starts (handy when placed in the Windows Start-up folder).
    pub activate_on_start: bool,

    /// Balloon-tip warnings to show before the shutdown.
    /// Each entry fires once per day, independently.
    pub notifications: Vec<Notification>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            grayscale_time:    "22:00".to_string(),
            grayscale_disable_time: "07:00".to_string(),
            grayscale_enabled: true,
            shutdown_time:     "23:00".to_string(),
            shutdown_enabled:  false,
            activate_on_start: false,
            notifications: vec![
                Notification {
                    minutes_before: 30,
                    message: "Computer shuts down in 30 minutes.".to_string(),
                },
                Notification {
                    minutes_before: 10,
                    message: "Computer shuts down in 10 minutes.".to_string(),
                },
                Notification {
                    minutes_before: 5,
                    message: "Computer shuts down in 5 minutes.".to_string(),
                },
            ],
        }
    }
}

impl Config {
    /// Full path to the config file.
    pub fn config_path() -> PathBuf {
        let programdata = std::env::var("PROGRAMDATA").unwrap_or_else(|_| "C:\\ProgramData".to_string());
        Path::new(&programdata)
            .join("grayscale-timer")
            .join("config.json")
    }

    /// Load from disk, or create a default file and return its defaults.
    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                if let Ok(cfg) = serde_json::from_str::<Config>(&contents) {
                    return cfg;
                }
            }
        }
        let default = Config::default();
        default.save(); // write template on first run
        default
    }

    /// Persist to disk (silently ignores I/O errors).
    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}
