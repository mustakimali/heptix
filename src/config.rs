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

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Preset name or path to a 16-bit PCM WAV.
    pub sound: String,
    /// Preset name, WAV path, or "off".
    pub scroll_sound: String,
    /// Tap volume, 0-100.
    pub volume: u32,
    /// Scroll tick volume, 0-100.
    pub scroll_volume: u32,
    pub max_ms: u64,
    pub scroll_ticks: i32,
    /// Mechanical keyboard sound profile, or "off".
    pub keyboard_sound: String,
    /// Keyboard volume, 0-100.
    pub keyboard_volume: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            sound: "thud".into(),
            scroll_sound: "crown".into(),
            volume: 45,
            scroll_volume: 35,
            max_ms: 200,
            scroll_ticks: 24,
            keyboard_sound: "bucklespring".into(),
            keyboard_volume: 40,
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

impl Settings {
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
             # Tap and button-press sound: haptic, tock, tick, thud, crown, wheel_of_time, or a path to a 16-bit WAV\n\
             sound = \"{}\"\n\
             # Two-finger scroll sound: same choices, or \"off\"\n\
             scroll_sound = \"{}\"\n\
             # Tap volume, 0-100\n\
             volume = {}\n\
             # Scroll tick volume, 0-100\n\
             scroll_volume = {}\n\
             # Longest touch (ms) still counted as a tap\n\
             max_ms = {}\n\
             # Scroll ticks per pad height (higher = denser)\n\
             scroll_ticks = {}\n\
             # Mechanical keyboard sound (default bucklespring; set \"off\" to disable): off, thock, clicky, linear, buckling, bucklespring (recorded Model M),\n\
             # or a directory of <hex keycode>-0.wav / -1.wav files. Only the fact that a key went\n\
             # down or up is used to pick a sound; keystrokes are never logged, stored or sent anywhere.\n\
             keyboard_sound = \"{}\"\n\
             # Keyboard volume, 0-100\n\
             keyboard_volume = {}\n",
            self.sound,
            self.scroll_sound,
            self.volume,
            self.scroll_volume,
            self.max_ms,
            self.scroll_ticks,
            self.keyboard_sound,
            self.keyboard_volume
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
        let s = Settings { sound: "/a b/c.wav".into(), scroll_sound: "off".into(), volume: 35, scroll_volume: 20, max_ms: 300, scroll_ticks: 40, keyboard_sound: "clicky".into(), keyboard_volume: 70 };
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
