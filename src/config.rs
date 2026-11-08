// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistent configuration (JSON) and profiles.

use crate::schedule::Entry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProfileValues {
    pub black_red: Option<u8>,
    pub black_green: Option<u8>,
    pub black_blue: Option<u8>,
    pub brightness: Option<u8>,
    pub contrast: Option<u8>,
    pub volume: Option<u8>,
    pub red: Option<u8>,
    pub green: Option<u8>,
    pub blue: Option<u8>,
    pub color_preset: Option<u16>,
    pub input: Option<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    /// stable monitor key (EDID) -> values; the key "*" applies to any monitor
    pub monitors: BTreeMap<String, ProfileValues>,
    /// software color filter (applies to the whole screen)
    pub fx: Option<FxValues>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FxValues {
    pub saturation: u8,
    pub mode: String,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub temp: u8,
}

impl Profile {
    /// (brightness, contrast, saturation) shown on the profile card; -1 = not set.
    pub fn levels(&self) -> (i32, i32, i32) {
        let first = self.monitors.values().next();
        let b = first.and_then(|v| v.brightness).map(|v| v as i32).unwrap_or(-1);
        let c = first.and_then(|v| v.contrast).map(|v| v as i32).unwrap_or(-1);
        let s = self.fx.as_ref().map(|f| f.saturation as i32).unwrap_or(-1);
        (b, c, s)
    }
}

/// Reads `levels`, accepting the old `[brightness, contrast]` form.
fn de_levels<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, BTreeMap<u8, u8>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Item {
        New(BTreeMap<String, u8>),
        Old((u8, u8)),
    }
    let raw = BTreeMap::<String, Item>::deserialize(d)?;
    Ok(raw
        .into_iter()
        .map(|(k, v)| match v {
            Item::New(m) => (k, m.into_iter().filter_map(|(c, v)| Some((c.parse::<u8>().ok()?, v))).collect()),
            Item::Old((b, c)) => (k, BTreeMap::from([(0x10, b), (0x12, c)])),
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub language: String, // "auto" | "pt-BR" | "en" | "es" | "de"
    pub theme: String,    // "system" | "light" | "dark" | "black"
    #[serde(default = "default_theme_color")]
    pub theme_color: String, // color of the Dark theme: "blue" | "violet" | "default"
    pub start_with_windows: bool,
    /// Linux: keep running (hidden) when the window is closed, so shortcuts keep working
    pub background: bool,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub show_osd: bool,
    pub sync_all: bool,
    pub step: u8,
    /// sliders with steps (dotted) of `step`
    pub discrete: bool,
    pub apply_on_start: bool,
    /// schedule times shown as 12-hour (AM/PM) instead of 24-hour
    pub clock_12h: bool,
    /// active saved profile (None = Free profile)
    pub active_profile: Option<String>,
    /// last state of the Free profile (captured when a saved profile is activated)
    pub free: Option<Profile>,
    /// allows editing saved profiles (changes are written to the active profile)
    pub edit_profiles: bool,
    pub link_fx: bool,
    pub link_hw: bool,
    /// shortcut indicator: position (0..=6) and size (%)
    pub osd_pos: u8,
    pub osd_scale: u8,
    /// last known controls per monitor key (VCP code -> percent: brightness, contrast, RGB gains and
    /// black levels), restored on launch
    #[serde(deserialize_with = "de_levels")]
    pub levels: BTreeMap<String, BTreeMap<u8, u8>>,
    /// mild, fixed transparency for the indicator and the mini panel
    pub transparent: bool,
    /// shows the monitor volume in the mini panel
    pub mini_audio: bool,
    /// animation direction of the indicator and mini panel (0 right, 1 down, 2 up, 3 left)
    pub anim_dir: u8,
    pub selected_monitor: Option<String>,
    pub profiles: Vec<Profile>,
    pub schedule: Vec<Entry>,
    pub hotkeys: BTreeMap<String, String>,
    /// software color filter (applied to the whole screen)
    pub fx_saturation: u8,
    pub fx_mode: String,
    pub fx_r: u8,
    pub fx_g: u8,
    pub fx_b: u8,
    pub fx_temp: u8,
}

pub const HOTKEY_ACTIONS: &[&str] = &[
    "brightness_up",
    "brightness_down",
    "contrast_up",
    "contrast_down",
    "saturation_up",
    "saturation_down",
    "temperature_up",
    "temperature_down",
    "volume_up",
    "volume_down",
    "volume_mute",
    "mini",
    "show",
];

pub fn default_hotkeys() -> BTreeMap<String, String> {
    [
        ("brightness_up", "Ctrl+Alt+Up"),
        ("brightness_down", "Ctrl+Alt+Down"),
        ("contrast_up", "Ctrl+Alt+Right"),
        ("contrast_down", "Ctrl+Alt+Left"),
        ("saturation_up", "Ctrl+Alt+PageUp"),
        ("saturation_down", "Ctrl+Alt+PageDown"),
        ("temperature_up", "Ctrl+Alt+Home"),
        ("temperature_down", "Ctrl+Alt+End"),
        ("volume_up", "Ctrl+Alt+Plus"),
        ("volume_down", "Ctrl+Alt+Minus"),
        ("volume_mute", "Ctrl+Alt+0"),
        ("mini", "Ctrl+Alt+M"),
        ("show", "Ctrl+Alt+S"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

fn default_theme_color() -> String {
    "blue".into()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            language: "auto".into(),
            theme: "system".into(),
            theme_color: default_theme_color(),
            start_with_windows: false,
            background: false,
            start_minimized: false,
            close_to_tray: true,
            show_osd: true,
            sync_all: false,
            step: 5,
            discrete: true,
            apply_on_start: false,
            clock_12h: false,
            active_profile: None,
            free: None,
            edit_profiles: false,
            link_fx: false,
            link_hw: false,
            osd_pos: 4,
            osd_scale: 100,
            levels: BTreeMap::new(),
            transparent: false,
            mini_audio: false,
            anim_dir: 1,
            selected_monitor: None,
            profiles: Vec::new(),
            schedule: Vec::new(),
            hotkeys: default_hotkeys(),
            fx_saturation: 50,
            fx_mode: "normal".into(),
            fx_r: 100,
            fx_g: 100,
            fx_b: 100,
            fx_temp: 50,
        }
    }
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("Solua");
    // migrate the folder of the old name (Soluna) once
    let old = base.join("Soluna");
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &dir);
    }
    dir
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

impl Config {
    pub fn load() -> Config {
        let mut cfg: Config = std::fs::read(config_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // fill in missing shortcuts (newer versions) without overwriting the user's
        for (k, v) in default_hotkeys() {
            cfg.hotkeys.entry(k).or_insert(v);
        }
        cfg.step = cfg.step.clamp(1, 25);
        cfg.osd_pos = cfg.osd_pos.min(6);
        cfg.osd_scale = cfg.osd_scale.clamp(60, 150);
        cfg.anim_dir = cfg.anim_dir.min(3);
        cfg
    }

    pub fn save(&self) {
        let dir = config_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let tmp = dir.join("config.json.tmp");
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, config_path());
            }
        }
    }

    /// Backup: all of the user's settings as JSON.
    pub fn export_json(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec_pretty(self).map_err(|e| e.to_string())
    }

    /// Reads a backup (missing values get the default).
    pub fn from_backup(bytes: &[u8]) -> Result<Config, String> {
        let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if !v.is_object() {
            return Err("not a Solua backup".into());
        }
        let mut cfg: Config = serde_json::from_value(v).map_err(|e| e.to_string())?;
        for (k, d) in default_hotkeys() {
            cfg.hotkeys.entry(k).or_insert(d);
        }
        cfg.step = cfg.step.clamp(1, 25);
        cfg.osd_pos = cfg.osd_pos.min(6);
        cfg.osd_scale = cfg.osd_scale.clamp(60, 150);
        cfg.anim_dir = cfg.anim_dir.min(3);
        Ok(cfg)
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_and_defaults() {
        let mut c = Config::default();
        c.profiles.push(Profile {
            name: "Noite".into(),
            monitors: [("DELA0B1-1".to_string(), ProfileValues { brightness: Some(20), ..Default::default() })].into(),
            ..Default::default()
        });
        let json = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.profiles, c.profiles);
        // partial JSON uses defaults
        let partial: Config = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(partial.theme, "dark");
        assert!(partial.close_to_tray);
        assert_eq!(partial.hotkeys.get("show").map(String::as_str), Some("Ctrl+Alt+S"));
        assert_eq!(partial.fx_saturation, 50);
    }

    #[test]
    fn profile_lookup_case_insensitive() {
        let mut c = Config::default();
        c.profiles.push(Profile { name: "Cinema".into(), ..Default::default() });
        assert!(c.profile("cinema").is_some());
        assert!(c.profile("x").is_none());
    }

    #[test]
    fn backup_roundtrip() {
        let mut c = Config::default();
        c.theme = "black".into();
        c.profiles.push(Profile { name: "A".into(), ..Default::default() });
        c.active_profile = Some("A".into());
        let bytes = c.export_json().unwrap();
        let back = Config::from_backup(&bytes).unwrap();
        assert_eq!(back.theme, "black");
        assert_eq!(back.active_profile.as_deref(), Some("A"));
        assert!(Config::from_backup(b"[1,2]").is_err());
        assert!(Config::from_backup(b"nope").is_err());
        assert_eq!(Config::from_backup(b"{}").unwrap().step, 5);
    }

    #[test]
    fn levels_keep_all_controls_and_read_the_old_form() {
        let mut c = Config::default();
        c.levels.insert("M1".into(), BTreeMap::from([(0x10, 40), (0x12, 60), (0x16, 90), (0x6C, 55)]));
        let back = Config::from_backup(&c.export_json().unwrap()).unwrap();
        assert_eq!(back.levels, c.levels);
        let old = Config::from_backup(br#"{"levels":{"M1":[30,70]}}"#).unwrap();
        assert_eq!(old.levels["M1"], BTreeMap::from([(0x10, 30), (0x12, 70)]));
    }
}
