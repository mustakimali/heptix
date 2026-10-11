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

//! Follows the system volume so heptix can stay quiet when the system volume
//! is high ("auto" volume mode).
//!
//! The level is read from `pactl` (PulseAudio and PipeWire's pulse layer) or
//! `wpctl` (PipeWire). Both report the slider position, and the real amplitude
//! is its cube. Nothing runs while heptix is idle: a worker re-reads the
//! volume when sounds are played, at most once a second.

use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Linear amplitude (0..) of the default output, and whether it is muted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub amp: f32,
    pub muted: bool,
}

/// Gain (0..=1) that keeps heptix's output level constant once the system
/// volume rises above `reference_pct` (a slider position, like the desktop's).
/// At or below the reference there is no change: a quiet system stays quiet.
pub fn compensation(amp: f32, reference_pct: u32) -> f32 {
    let reference = (reference_pct as f32 / 100.0).powi(3);
    if amp <= reference || amp <= 0.0 {
        1.0
    } else {
        reference / amp
    }
}

fn slider_to_amp(slider: f32) -> f32 {
    slider.max(0.0).powi(3)
}

/// Largest channel percentage in `pactl get-sink-volume` output, as a 0..1+ slider.
fn parse_pactl_volume(text: &str) -> Option<f32> {
    text.split_whitespace()
        .filter_map(|w| w.strip_suffix('%')?.parse::<f32>().ok())
        .reduce(f32::max)
        .map(|p| p / 100.0)
}

fn parse_pactl_mute(text: &str) -> bool {
    text.trim().to_ascii_lowercase().ends_with("yes")
}

/// `Volume: 0.45` or `Volume: 0.45 [MUTED]` from `wpctl get-volume`.
fn parse_wpctl(text: &str) -> Option<(f32, bool)> {
    let rest = text.trim().strip_prefix("Volume:")?;
    let slider = rest.split_whitespace().next()?.parse::<f32>().ok()?;
    Some((slider, rest.contains("MUTED")))
}

fn run(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Reads the default output's volume; `None` if no supported tool answers.
pub fn query() -> Option<Reading> {
    if let Some(volume) = run("pactl", &["get-sink-volume", "@DEFAULT_SINK@"]) {
        let slider = parse_pactl_volume(&volume)?;
        let muted = run("pactl", &["get-sink-mute", "@DEFAULT_SINK@"]).is_some_and(|m| parse_pactl_mute(&m));
        return Some(Reading { amp: slider_to_amp(slider), muted });
    }
    let (slider, muted) = parse_wpctl(&run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?)?;
    Some(Reading { amp: slider_to_amp(slider), muted })
}

/// Keeps a callback updated with the system volume. See the module docs.
pub struct Watcher {
    tx: Sender<()>,
    last: std::cell::Cell<Instant>,
}

/// Starts the worker and reads the volume once right away. `on_change` gets
/// the amplitude whenever it differs from the last one (not while muted).
pub fn watch(on_change: impl Fn(f32) + Send + 'static) -> Watcher {
    let (tx, rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut last: Option<f32> = None;
        let mut refresh = || {
            if let Some(r) = query().filter(|r| !r.muted) {
                if last != Some(r.amp) {
                    last = Some(r.amp);
                    on_change(r.amp);
                }
            }
        };
        refresh();
        while rx.recv().is_ok() {
            while rx.try_recv().is_ok() {}
            refresh();
        }
    });
    Watcher { tx, last: std::cell::Cell::new(Instant::now()) }
}

impl Watcher {
    /// Call when a sound is about to play: asks for a fresh reading, at most once a second.
    pub fn touch(&self) {
        if self.last.get().elapsed() >= Duration::from_secs(1) {
            self.last.set(Instant::now());
            let _ = self.tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compensation_is_flat_below_the_reference_and_falls_above_it() {
        let at = |pct: f32| slider_to_amp(pct / 100.0);
        assert_eq!(compensation(at(20.0), 50), 1.0);
        assert_eq!(compensation(at(50.0), 50), 1.0);
        assert!((compensation(at(100.0), 50) - 0.125).abs() < 1e-6);
        // Output level (gain * system amplitude) stays constant above the reference.
        let out = |pct: f32| compensation(at(pct), 50) * at(pct);
        assert!((out(60.0) - out(100.0)).abs() < 1e-6);
        assert_eq!(compensation(0.0, 50), 1.0);
        assert!(compensation(at(80.0), 50) > compensation(at(100.0), 50));
    }

    #[test]
    fn pactl_output_parses() {
        let text = "Volume: front-left: 29491 /  45% / -20.00 dB,   front-right: 31457 /  48% / -18.37 dB\n        balance 0.04\n";
        assert_eq!(parse_pactl_volume(text), Some(0.48));
        assert_eq!(parse_pactl_volume("nonsense"), None);
        assert!(parse_pactl_mute("Mute: yes\n"));
        assert!(!parse_pactl_mute("Mute: no\n"));
    }

    #[test]
    fn wpctl_output_parses() {
        assert_eq!(parse_wpctl("Volume: 0.69\n"), Some((0.69, false)));
        assert_eq!(parse_wpctl("Volume: 0.45 [MUTED]\n"), Some((0.45, true)));
        assert_eq!(parse_wpctl("garbage"), None);
    }
}
