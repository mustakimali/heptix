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

//! Mouse sounds: a different tactile click for button down and button up, and
//! one scroll tick per wheel notch.
//!
//! Only button and wheel events are used. Pointer movement events are read
//! from the device and discarded immediately; nothing about them is kept.

use crate::keyboard::{render, Class, Hit, Profile, Rng, VARIANTS};
use crate::player::Player;
use crate::sound;
use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode, RelativeAxisCode};
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub const SOUNDS: &[&str] = &["recorded", "tactile", "soft"];

/// A real recording of a mouse button going down and coming up (see `assets/README.md`).
static RECORDED_DOWN: &[u8] = include_bytes!("../assets/mouse-click-down.wav");
static RECORDED_UP: &[u8] = include_bytes!("../assets/mouse-click-up.wav");

/// Wheel units per notch in the kernel's high-resolution scroll axes.
const NOTCH: i32 = 120;
/// Shortest gap between wheel ticks, so a fast fling does not become a buzz.
const MIN_TICK_GAP: Duration = Duration::from_millis(18);
/// Same button and direction closer together than this is one event seen twice.
const DUPLICATE_WINDOW: Duration = Duration::from_millis(8);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Button { code: u16, down: bool },
    /// One wheel notch (vertical or horizontal).
    Wheel,
}

/// A mouse has relative motion and buttons; touchpads and keyboards do not.
pub fn is_mouse(d: &Device) -> bool {
    let rel = d
        .supported_relative_axes()
        .is_some_and(|a| a.contains(RelativeAxisCode::REL_X) && a.contains(RelativeAxisCode::REL_Y));
    let button = d.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_LEFT));
    let tablet = d.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_X));
    rel && button && !tablet
}

/// "SYNA2393:00 06CB:7A13 Touchpad" -> "SYNA2393:00 06CB:7A13".
fn touchpad_stem(name: &str) -> Option<&str> {
    let stem = name.strip_suffix("Touchpad").or_else(|| name.strip_suffix("TouchPad"))?.trim_end();
    (stem.len() >= 4).then_some(stem)
}

/// True if `mouse_name` is the pointer node of one of these touchpads (for
/// example "SYNA2393:00 06CB:7A13 Mouse"), which would report the same clicks.
pub fn is_touchpad_sibling(mouse_name: &str, touchpad_names: &[String]) -> bool {
    touchpad_names.iter().filter_map(|t| touchpad_stem(t)).any(|stem| mouse_name.starts_with(stem))
}

/// Turns wheel movement into ticks: one per notch, rate limited.
#[derive(Default)]
pub struct Wheel {
    acc: [i32; 2],
    last: Option<Instant>,
}

impl Wheel {
    /// `axis` 0 = vertical, 1 = horizontal; `units` in 1/120 notch.
    /// Returns true when a tick should play.
    pub fn feed(&mut self, axis: usize, units: i32, now: Instant) -> bool {
        let acc = &mut self.acc[axis];
        if *acc != 0 && (*acc > 0) != (units > 0) {
            *acc = 0; // turned around: forget the partial notch
        }
        *acc += units;
        if acc.abs() < NOTCH {
            return false;
        }
        *acc %= NOTCH; // drop the whole notches, keep the remainder
        if self.last.is_some_and(|t| now.duration_since(t) < MIN_TICK_GAP) {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// Blocks, reporting button presses/releases and wheel ticks.
pub fn watch(mut dev: Device, stop: &AtomicBool, mut on_event: impl FnMut(Event)) -> io::Result<()> {
    let hires = dev.supported_relative_axes().is_some_and(|a| {
        a.contains(RelativeAxisCode::REL_WHEEL_HI_RES) || a.contains(RelativeAxisCode::REL_HWHEEL_HI_RES)
    });
    let mut wheel = Wheel::default();
    loop {
        match crate::devices::wait_for_events(&dev, stop) {
            None => return Ok(()),
            Some(false) => continue,
            Some(true) => {}
        }
        for ev in dev.fetch_events()? {
            match ev.destructure() {
                EventSummary::Key(_, code, value @ (0 | 1)) if (0x110..=0x117).contains(&code.0) => {
                    on_event(Event::Button { code: code.0, down: value == 1 });
                }
                EventSummary::RelativeAxis(_, axis, v) => {
                    // Devices with hi-res axes also send the legacy ones for the same movement.
                    let (index, units) = match axis {
                        RelativeAxisCode::REL_WHEEL_HI_RES if hires => (0, v),
                        RelativeAxisCode::REL_HWHEEL_HI_RES if hires => (1, v),
                        RelativeAxisCode::REL_WHEEL if !hires => (0, v * NOTCH),
                        RelativeAxisCode::REL_HWHEEL if !hires => (1, v * NOTCH),
                        _ => continue,
                    };
                    if wheel.feed(index, units, Instant::now()) {
                        on_event(Event::Wheel);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Drops events that arrive twice (a remapper exposing the same mouse
/// again, or a touchpad's pointer node) and wheel ticks that are too close.
#[derive(Default)]
pub struct Filter {
    button: Option<(u16, bool, Instant)>,
    wheel: Option<Instant>,
}

impl Filter {
    pub fn accept(&mut self, ev: Event, now: Instant) -> bool {
        match ev {
            Event::Button { code, down } => {
                if self.button.is_some_and(|(c, d, t)| c == code && d == down && now.duration_since(t) < DUPLICATE_WINDOW) {
                    return false;
                }
                self.button = Some((code, down, now));
                true
            }
            Event::Wheel => {
                if self.wheel.is_some_and(|t| now.duration_since(t) < MIN_TICK_GAP) {
                    return false;
                }
                self.wheel = Some(now);
                true
            }
        }
    }
}

// Microswitch: a sharp click on the way down, a lighter, higher tick on release.
static TACTILE: Profile = Profile {
    press: &[
        Hit { at: 0.0, noise: (2500.0, 9000.0, 0.0008, 0.9), tones: &[(4200.0, 0.006, 0.6), (6300.0, 0.004, 0.35)] },
        Hit {
            at: 0.002,
            noise: (500.0, 2500.0, 0.003, 0.5),
            tones: &[(700.0, 0.018, 0.7), (1250.0, 0.010, 0.35), (2600.0, 0.006, 0.12)],
        },
    ],
    press_ms: 55.0,
    release: &[
        Hit { at: 0.0, noise: (3000.0, 9000.0, 0.0006, 0.6), tones: &[(4800.0, 0.004, 0.4), (7000.0, 0.003, 0.2)] },
        Hit { at: 0.0015, noise: (800.0, 3000.0, 0.0025, 0.3), tones: &[(900.0, 0.010, 0.3)] },
    ],
    release_ms: 35.0,
    release_level: 0.55,
};

// Quiet, rounded rubber-dome feel.
static SOFT: Profile = Profile {
    press: &[Hit {
        at: 0.0,
        noise: (800.0, 3500.0, 0.003, 0.55),
        tones: &[(520.0, 0.020, 0.9), (900.0, 0.012, 0.45), (1900.0, 0.006, 0.15)],
    }],
    press_ms: 50.0,
    release: &[Hit { at: 0.0, noise: (1200.0, 4500.0, 0.002, 0.4), tones: &[(680.0, 0.012, 0.45), (1300.0, 0.008, 0.2)] }],
    release_ms: 30.0,
    release_level: 0.5,
};

/// Right and middle buttons sound a little lower, side buttons a little higher.
/// The recording is of a middle button, so it is the reference there.
fn button_pitch(code: u16, recorded: bool) -> f32 {
    let (left, right, middle, other) = if recorded { (1.05, 1.0, 1.0, 1.1) } else { (1.0, 0.94, 0.88, 1.08) };
    match code {
        c if c == KeyCode::BTN_LEFT.0 => left,
        c if c == KeyCode::BTN_RIGHT.0 => right,
        c if c == KeyCode::BTN_MIDDLE.0 => middle,
        _ => other,
    }
}

/// Both recordings scaled by the same factor (so the release keeps its natural
/// level relative to the press), with a short fade-out, as playable PCM.
/// Pre-rendered variants of one stroke.
type Bank = Vec<Arc<Vec<u8>>>;

fn recorded_bank() -> Result<(Bank, Bank), String> {
    let (mut down, rate_down) = sound::decode_wav(RECORDED_DOWN, "mouse-click-down.wav")?;
    let (mut up, rate_up) = sound::decode_wav(RECORDED_UP, "mouse-click-up.wav")?;
    let peak = down.iter().chain(up.iter()).fold(1e-6f32, |m, x| m.max(x.abs()));
    for (samples, rate) in [(&mut down, rate_down), (&mut up, rate_up)] {
        let fade = (rate as f32 * 0.003) as usize;
        let n = samples.len();
        for (i, x) in samples.iter_mut().enumerate() {
            *x *= 1.0 / peak * ((n - i).min(fade.max(1)) as f32 / fade.max(1) as f32);
        }
    }
    let one = |samples: &[f32], rate| Arc::new(sound::finish(samples, rate, 100));
    let (down, up) = (one(&down, rate_down), one(&up, rate_up));
    Ok((vec![down; VARIANTS], vec![up; VARIANTS]))
}

pub struct Mouse {
    recorded: bool,
    down: Vec<Arc<Vec<u8>>>,
    up: Vec<Arc<Vec<u8>>>,
    volume: f32,
    rng: Rng,
}

impl Mouse {
    pub fn new(profile: &str, volume: u32) -> Result<Self, String> {
        if profile == "recorded" {
            let (down, up) = recorded_bank()?;
            return Ok(Mouse { recorded: true, down, up, volume: volume as f32 / 100.0, rng: Rng::seeded() });
        }
        let p = match profile {
            "tactile" => &TACTILE,
            "soft" => &SOFT,
            _ => return Err(format!("unknown mouse sound '{profile}' (choose off, {})", SOUNDS.join(", "))),
        };
        let mut rng = Rng::seeded();
        let mut bank = |hits: &[Hit], ms: f32, press: bool, peak: f32| -> Vec<Arc<Vec<u8>>> {
            (0..VARIANTS)
                .map(|_| Arc::new(sound::to_pcm(&render(hits, ms, Class::Normal, press, peak, &mut rng), 100)))
                .collect()
        };
        let down = bank(p.press, p.press_ms, true, 1.0);
        let up = bank(p.release, p.release_ms, false, p.release_level);
        Ok(Mouse { recorded: false, down, up, volume: volume as f32 / 100.0, rng: Rng::seeded() })
    }

    pub fn handle_button(&mut self, code: u16, down: bool, player: &Player) {
        let bank = if down { &self.down } else { &self.up };
        let pcm = &bank[self.rng.next() as usize % VARIANTS];
        // One recording only, so vary it a little more to keep repeats from sounding identical.
        let spread = if self.recorded { 0.035 } else { 0.02 };
        let rate = button_pitch(code, self.recorded) * (1.0 + spread * self.rng.signed());
        let gain = self.volume * (0.85 + 0.15 * self.rng.unit());
        player.play_with(pcm, gain, rate);
    }

    /// Left, right, middle and a quick double click. Blocks until done.
    pub fn demo(&mut self, player: &Player) {
        let (left, right, middle) = (KeyCode::BTN_LEFT.0, KeyCode::BTN_RIGHT.0, KeyCode::BTN_MIDDLE.0);
        for (code, hold, gap) in [(left, 70, 300), (right, 70, 300), (middle, 70, 350), (left, 55, 90), (left, 55, 0)] {
            self.handle_button(code, true, player);
            thread::sleep(Duration::from_millis(hold));
            self.handle_button(code, false, player);
            thread::sleep(Duration::from_millis(gap));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(pcm: &[u8]) -> i16 {
        pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]).abs()).max().unwrap()
    }

    #[test]
    fn down_and_up_are_different_sounds_and_up_is_quieter() {
        for name in SOUNDS {
            let m = Mouse::new(name, 100).unwrap();
            for v in 0..VARIANTS {
                assert_ne!(m.down[v], m.up[v], "{name}");
                assert!(peak(&m.up[v]) < peak(&m.down[v]), "{name}");
                assert!(peak(&m.down[v]) > 20_000, "{name}");
            }
            if *name != "recorded" {
                assert_ne!(m.down[0], m.down[1], "{name}: variants differ");
            }
        }
        assert!(Mouse::new("nope", 50).is_err());
    }

    #[test]
    fn recorded_clicks_are_real_audio_of_a_sensible_length() {
        let m = Mouse::new("recorded", 100).unwrap();
        // 44.1 kHz recordings, ~75 ms, resampled to 48 kHz mono s16.
        for pcm in [&m.down[0], &m.up[0]] {
            let ms = pcm.len() as f32 / 2.0 / 48.0;
            assert!((40.0..120.0).contains(&ms), "{ms} ms");
        }
        assert!(m.recorded);
        assert!(button_pitch(KeyCode::BTN_LEFT.0, true) > button_pitch(KeyCode::BTN_RIGHT.0, true));
    }

    #[test]
    fn wheel_ticks_once_per_notch_and_handles_hires_remainders() {
        let mut w = Wheel::default();
        let t = Instant::now();
        let at = |ms: u64| t + Duration::from_millis(ms);
        assert!(w.feed(0, 120, at(0)));
        // A smooth wheel: 8 x 15 units = 1 notch.
        let ticks = (0..8).filter(|i| w.feed(0, 15, at(100 + *i * 5))).count();
        assert_eq!(ticks, 1);
        // Turning around drops the partial notch.
        assert!(!w.feed(0, 100, at(300)));
        assert!(!w.feed(0, -100, at(310)));
        assert!(w.feed(0, -20, at(320)));
        // Horizontal is tracked separately.
        assert!(w.feed(1, 120, at(400)));
    }

    #[test]
    fn wheel_fling_is_rate_limited() {
        let mut w = Wheel::default();
        let t = Instant::now();
        // One notch every 2 ms for 100 ms: far more than 1 per 18 ms must not play.
        let ticks = (0..50).filter(|i| w.feed(0, 120, t + Duration::from_millis(i * 2))).count();
        assert!((5..=6).contains(&ticks), "{ticks}");
    }

    #[test]
    fn filter_drops_duplicate_buttons_and_close_wheel_ticks() {
        let mut f = Filter::default();
        let t = Instant::now();
        let left = |down| Event::Button { code: KeyCode::BTN_LEFT.0, down };
        assert!(f.accept(left(true), t));
        assert!(!f.accept(left(true), t + Duration::from_millis(3)), "same event from a second node");
        assert!(f.accept(left(false), t + Duration::from_millis(4)), "release is a different event");
        assert!(f.accept(left(true), t + Duration::from_millis(60)), "a real second click");
        assert!(f.accept(Event::Wheel, t));
        assert!(!f.accept(Event::Wheel, t + Duration::from_millis(5)));
        assert!(f.accept(Event::Wheel, t + Duration::from_millis(30)));
    }

    #[test]
    fn touchpad_pointer_nodes_are_recognised_as_siblings() {
        let pads = vec!["SYNA2393:00 06CB:7A13 Touchpad".to_string()];
        assert!(is_touchpad_sibling("SYNA2393:00 06CB:7A13 Mouse", &pads));
        assert!(!is_touchpad_sibling("Compx 2.4G Receiver Mouse", &pads));
        assert!(!is_touchpad_sibling("Logitech USB Mouse", &[]));
        assert!(!is_touchpad_sibling("Touchpad Mouse", &["Touchpad".to_string()]), "no stem, no match");
    }
}
