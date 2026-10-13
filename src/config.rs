// SPDX-License-Identifier: GPL-2.0-only
// heptix - the missing haptic feedback for Linux!
// Copyright (C) 2026 M Mustakim Ali
//
// This program is free software; you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 2 as
// published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program; if not, see <https://www.gnu.org/licenses/>.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Per-sound levels used in `auto` volume mode: the mix between sounds, before
/// the system-volume compensation.
pub const AUTO_TAP_VOLUME: u32 = 55;
pub const AUTO_SCROLL_VOLUME: u32 = 45;
pub const AUTO_KEYBOARD_VOLUME: u32 = 40;
pub const AUTO_MOUSE_VOLUME: u32 = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Master switch. When false heptix is idle: it reads no input, plays
    /// nothing and records nothing.
    pub enabled: bool,
    /// Preset name or path to a 16-bit PCM WAV.
    pub sound: String,
    /// Preset name, WAV path, or "off".
    pub scroll_sound: String,
    /// "auto" follows the system volume (heptix gets quieter as it rises);
    /// "custom" uses the three volumes below as they are.
    pub volume_mode: String,
    /// System volume (slider %) at which auto mode plays sounds at full level.
    pub auto_reference: u32,
    /// Tap volume, 0-100 (custom mode).
    pub volume: u32,
    /// Scroll tick volume, 0-100 (custom mode).
    pub scroll_volume: u32,
    pub max_ms: u64,
    pub scroll_ticks: i32,
    /// Mechanical keyboard sound profile, or "off".
    pub keyboard_sound: String,
    /// Keyboard volume, 0-100 (custom mode).
    pub keyboard_volume: u32,
    /// Mouse button sound: "tactile", "soft" or "off". The wheel uses `scroll_sound`.
    pub mouse_sound: String,
    /// Mouse volume, 0-100 (custom mode).
    pub mouse_volume: u32,
    /// Count keystrokes and touchpad actions (counters only) for the Activity view.
    pub stats: bool,
    /// Also count presses per key (needs `stats`).
    pub stats_keys: bool,
    /// Distance unit for scroll distance: "km" or "mi".
    pub distance_unit: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            sound: "thud".into(),
            scroll_sound: "crown".into(),
            volume_mode: "auto".into(),
            auto_reference: 50,
            volume: AUTO_TAP_VOLUME,
            scroll_volume: AUTO_SCROLL_VOLUME,
            max_ms: 200,
            scroll_ticks: 24,
            keyboard_sound: "bucklespring".into(),
            keyboard_volume: AUTO_KEYBOARD_VOLUME,
            mouse_sound: "recorded".into(),
            mouse_volume: AUTO_MOUSE_VOLUME,
            stats: true,
            stats_keys: true,
            distance_unit: "km".into(),
        }
    }
}

/// `$XDG_CONFIG_HOME/heptix/config.toml`, falling back to `~/.config/heptix/config.toml`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("heptix").join("config.toml")
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

impl Settings {
    pub fn is_auto_volume(&self) -> bool {
        self.volume_mode == "auto"
    }

    /// (tap, scroll, keyboard, mouse) volumes to load the sounds with.
    pub fn effective_volumes(&self) -> (u32, u32, u32, u32) {
        if self.is_auto_volume() {
            (AUTO_TAP_VOLUME, AUTO_SCROLL_VOLUME, AUTO_KEYBOARD_VOLUME, AUTO_MOUSE_VOLUME)
        } else {
            (self.volume, self.scroll_volume, self.keyboard_volume, self.mouse_volume)
        }
    }

    /// Parses the TOML subset we write (`key = value`, `#` comments). Bad lines
    /// are skipped and reported in the returned warnings.
    pub fn parse(text: &str) -> (Settings, Vec<String>) {
        let mut s = Settings::default();
        let mut warnings = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, raw)) = line.split_once('=') else {
                warnings.push(format!("line {}: expected key = value", n + 1));
                continue;
            };
            let raw = raw.trim();
            let value = match raw.strip_prefix('"') {
                Some(rest) => rest.split('"').next().unwrap_or("").to_string(),
                None => raw.split('#').next().unwrap_or("").trim().to_string(),
            };
            let ok = match key.trim() {
                "sound" => {
                    s.sound = value;
                    true
                }
                "scroll_sound" => {
                    s.scroll_sound = value;
                    true
                }
                "keyboard_sound" => {
                    s.keyboard_sound = value;
                    true
                }
                "keyboard_volume" => value.parse().map(|v: u32| s.keyboard_volume = v.min(100)).is_ok(),
                "volume_mode" => match value.as_str() {
                    "auto" | "custom" => {
                        s.volume_mode = value;
                        true
                    }
                    _ => false,
                },
                "auto_reference" => value.parse().map(|v: u32| s.auto_reference = v.clamp(10, 100)).is_ok(),
                "mouse_sound" => match value.as_str() {
                    "off" | "recorded" | "tactile" | "soft" => {
                        s.mouse_sound = value;
                        true
                    }
                    _ => false,
                },
                "mouse_volume" => value.parse().map(|v: u32| s.mouse_volume = v.min(100)).is_ok(),
                "enabled" => parse_bool(&value).map(|v| s.enabled = v).is_some(),
                "stats" => parse_bool(&value).map(|v| s.stats = v).is_some(),
                "stats_keys" => parse_bool(&value).map(|v| s.stats_keys = v).is_some(),
                "distance_unit" => match value.as_str() {
                    "km" | "mi" => {
                        s.distance_unit = value;
                        true
                    }
                    _ => false,
                },
                "scroll_volume" => value.parse().map(|v: u32| s.scroll_volume = v.min(100)).is_ok(),
                "volume" => value.parse().map(|v: u32| s.volume = v.min(100)).is_ok(),
                "max_ms" => value.parse().map(|v: u64| s.max_ms = v.clamp(50, 1000)).is_ok(),
                "scroll_ticks" => value.parse().map(|v: i32| s.scroll_ticks = v.clamp(1, 200)).is_ok(),
                other => {
                    warnings.push(format!("line {}: unknown key '{other}'", n + 1));
                    continue;
                }
            };
            if !ok {
                warnings.push(format!("line {}: bad value for '{}'", n + 1, key.trim()));
            }
        }
        (s, warnings)
    }

    pub fn to_toml(&self) -> String {
        format!(
            "# heptix - the missing haptic feedback for Linux! Settings. Run `heptix` with no arguments to edit them interactively.\n\
             \n\
             # ── General ──────────────────────────────────────────────────────────────\n\
             # Master switch. Set to false to turn heptix off entirely: it reads no input, plays\n\
             # nothing and records nothing (it stays running, idle, until you turn it back on).\n\
             enabled = {}\n\
             \n\
             # ── Sounds ───────────────────────────────────────────────────────────────\n\
             # Tap and button-press sound: haptic, tock, tick, thud, crown, wheel_of_time, or a path to a 16-bit WAV\n\
             sound = \"{}\"\n\
             # Two-finger scroll sound: same choices, or \"off\"\n\
             scroll_sound = \"{}\"\n\
             # Mechanical keyboard sound (default bucklespring; set \"off\" to disable): off, thock, clicky, linear, buckling, bucklespring (recorded Model M),\n\
             # or a directory of <hex keycode>-0.wav / -1.wav files. Only the fact that a key went\n\
             # down or up is used to pick a sound; the service runs with no network access.\n\
             keyboard_sound = \"{}\"\n\
             # Mouse button sound (a different click for button down and up): off, recorded (real\n\
             # recording), tactile, soft (synthesised).\n\
             # The wheel plays one scroll_sound tick per notch. Only buttons and the wheel are used.\n\
             mouse_sound = \"{}\"\n\
             \n\
             # ── Volume ───────────────────────────────────────────────────────────────\n\
             # Volume mode: auto = follow the system volume (heptix gets quieter as you turn the\n\
             # system volume up, so it is never too loud); custom = use the volumes below.\n\
             volume_mode = \"{}\"\n\
             # Custom mode: tap volume, 0-100\n\
             volume = {}\n\
             # Custom mode: scroll tick volume, 0-100\n\
             scroll_volume = {}\n\
             # Custom mode: keyboard volume, 0-100\n\
             keyboard_volume = {}\n\
             # Custom mode: mouse volume, 0-100\n\
             mouse_volume = {}\n\
             # Auto mode: system volume (%) at and below which sounds play at full level, 10-100\n\
             auto_reference = {}\n\
             \n\
             # ── Touchpad ─────────────────────────────────────────────────────────────\n\
             # Longest touch (ms) still counted as a tap\n\
             max_ms = {}\n\
             # Scroll ticks per pad height (higher = denser)\n\
             scroll_ticks = {}\n\
             \n\
             # ── Activity ─────────────────────────────────────────────────────────────\n\
             # Activity stats: count keys and touchpad actions (counters only: per hour, and per key\n\
             # per day) for the Activity view in `heptix`. Stored in ~/.local/state/heptix/stats.tsv,\n\
             # never sent anywhere (the service runs with no network access). Set false to stop.\n\
             stats = {}\n\
             # Also count presses per key (needs stats)\n\
             stats_keys = {}\n\
             # Scroll distance unit: km or mi\n\
             distance_unit = \"{}\"\n",
            self.enabled,
            self.sound,
            self.scroll_sound,
            self.keyboard_sound,
            self.mouse_sound,
            self.volume_mode,
            self.volume,
            self.scroll_volume,
            self.keyboard_volume,
            self.mouse_volume,
            self.auto_reference,
            self.max_ms,
            self.scroll_ticks,
            self.stats,
            self.stats_keys,
            self.distance_unit
        )
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, self.to_toml())
    }

    /// Loads the config, writing a default file first if none exists.
    pub fn load_or_create(path: &Path) -> (Settings, Vec<String>) {
        match fs::read_to_string(path) {
            Ok(text) => Settings::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let s = Settings::default();
                let warn = match s.save(path) {
                    Ok(()) => vec![format!("created default config at {}", path.display())],
                    Err(e) => vec![format!("cannot create {}: {e}", path.display())],
                };
                (s, warn)
            }
            Err(e) => (Settings::default(), vec![format!("{}: {e}", path.display())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = Settings { enabled: false, sound: "/a b/c.wav".into(), scroll_sound: "off".into(), volume: 35, scroll_volume: 20, max_ms: 300, scroll_ticks: 40, keyboard_sound: "clicky".into(), keyboard_volume: 70, stats: false, stats_keys: false, distance_unit: "mi".into(), volume_mode: "custom".into(), auto_reference: 70, mouse_sound: "soft".into(), mouse_volume: 65 };
        let (back, warn) = Settings::parse(&s.to_toml());
        assert_eq!(back, s);
        assert!(warn.is_empty());
    }

    #[test]
    fn bad_input_warns_and_keeps_defaults() {
        let (s, warn) = Settings::parse("volume = loud\nnope = 1\nvolume = 500\n");
        assert_eq!(s.volume, 100);
        assert_eq!(warn.len(), 2);
    }

    #[test]
    fn volume_mode_defaults_to_auto_and_locks_the_per_sound_volumes() {
        let mut s = Settings::default();
        assert!(s.is_auto_volume());
        s.volume = 10;
        s.scroll_volume = 20;
        s.keyboard_volume = 30;
        assert_eq!(s.effective_volumes(), (AUTO_TAP_VOLUME, AUTO_SCROLL_VOLUME, AUTO_KEYBOARD_VOLUME, AUTO_MOUSE_VOLUME));
        s.volume_mode = "custom".into();
        s.mouse_volume = 40;
        assert_eq!(s.effective_volumes(), (10, 20, 30, 40));
        let (p, warn) = Settings::parse("volume_mode = \"loud\"\nauto_reference = 5\n");
        assert_eq!(warn.len(), 1);
        assert!(p.is_auto_volume());
        assert_eq!(p.auto_reference, 10);
    }

    #[test]
    fn enabled_defaults_to_true_and_is_written_first() {
        assert!(Settings::default().enabled);
        assert!(!Settings::parse("enabled = false\n").0.enabled);
        assert!(!Settings::parse("enabled = off\n").0.enabled);
        let text = Settings::default().to_toml();
        let first = text.lines().find(|l| !l.starts_with('#') && !l.is_empty()).unwrap();
        assert_eq!(first, "enabled = true");
    }

    #[test]
    fn mouse_sound_defaults_to_recorded_and_rejects_unknown_names() {
        assert_eq!(Settings::default().mouse_sound, "recorded");
        assert_eq!(Settings::parse("mouse_sound = \"off\"\n").0.mouse_sound, "off");
        let (s, warn) = Settings::parse("mouse_sound = \"squeaky\"\nmouse_volume = 300\n");
        assert_eq!(s.mouse_sound, "recorded");
        assert_eq!(s.mouse_volume, 100);
        assert_eq!(warn.len(), 1);
    }

    #[test]
    fn stats_are_on_by_default_and_can_be_switched_off() {
        let d = Settings::default();
        assert!(d.stats && d.stats_keys);
        assert_eq!(d.distance_unit, "km");
        let (s, warn) = Settings::parse("stats = off\nstats_keys = false\ndistance_unit = \"mi\"\n");
        assert!(!s.stats && !s.stats_keys && warn.is_empty());
        assert_eq!(s.distance_unit, "mi");
        let (s, warn) = Settings::parse("stats = maybe\ndistance_unit = \"furlongs\"\n");
        assert!(s.stats);
        assert_eq!(s.distance_unit, "km");
        assert_eq!(warn.len(), 2);
    }

    #[test]
    fn keyboard_sound_defaults_to_bucklespring() {
        assert_eq!(Settings::default().keyboard_sound, "bucklespring");
        assert_eq!(Settings::parse("volume = 10\n").0.keyboard_sound, "bucklespring");
        assert_eq!(Settings::parse("keyboard_sound = \"off\"\n").0.keyboard_sound, "off");
    }

    #[test]
    fn missing_file_is_created_with_defaults() {
        let dir = std::env::temp_dir().join(format!("heptix-test-{}", std::process::id()));
        let path = dir.join("nested").join("config.toml");
        let (s, _) = Settings::load_or_create(&path);
        assert_eq!(s, Settings::default());
        assert!(path.exists());
        let _ = fs::remove_dir_all(dir);
    }
}
