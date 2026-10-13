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

//! Mechanical keyboard sounds, synthesised.
//!
//! Each switch profile is a set of short "hits" (a band of filtered noise plus
//! decaying resonances) for the key-down and key-up strokes. At start-up we
//! render a few variants per key class (normal, wide, space) and at play time
//! pick one at random, nudging pitch and level per key so no two presses are
//! identical.
//!
//! A sample pack directory can be used instead of a profile: it holds
//! `<hex keycode>-0.wav` (press) and `<hex keycode>-1.wav` (release), as in
//! bucklespring's `wav` folder. Keys without a file use a random other key's.
//!
//! Privacy: only the key code and direction of an event are used, to choose a
//! sample. Nothing is logged, stored or sent anywhere.

use crate::player::Player;
use crate::sound::{self, RATE};
use evdev::{Device, EventSummary, KeyCode};
use std::f32::consts::TAU;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// Recorded IBM Model M samples embedded in the binary (see `assets/README.md`).
pub const RECORDED: &str = "bucklespring";

/// Every built-in keyboard sound, for menus.
pub const SOUNDS: &[&str] = &["thock", "clicky", "linear", "buckling", RECORDED];

static BUCKLESPRING: &[u8] = include_bytes!("../assets/bucklespring.pack");

/// Pre-rendered variants per class and stroke.
pub(crate) const VARIANTS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyEvent {
    pub code: u16,
    pub down: bool,
}

/// A keyboard has letters, space and enter; mice, buttons and touchpads do not.
pub fn is_keyboard(d: &Device) -> bool {
    d.supported_keys().is_some_and(|k| {
        [KeyCode::KEY_A, KeyCode::KEY_Z, KeyCode::KEY_SPACE, KeyCode::KEY_ENTER].iter().all(|c| k.contains(*c))
    })
}

/// Blocks, reporting key presses and releases (not auto-repeat).
pub fn watch(mut dev: Device, stop: &AtomicBool, mut on_key: impl FnMut(KeyEvent)) -> io::Result<()> {
    loop {
        match crate::devices::wait_for_events(&dev, stop) {
            None => return Ok(()),
            Some(false) => continue,
            Some(true) => {}
        }
        for ev in dev.fetch_events()? {
            if let EventSummary::Key(_, code, value @ (0 | 1)) = ev.destructure() {
                if code.0 < 0x100 {
                    on_key(KeyEvent { code: code.0, down: value == 1 });
                }
            }
        }
    }
}

/// One impact: filtered noise plus ringing resonances, `at` seconds in.
pub(crate) struct Hit {
    pub(crate) at: f32,
    /// Band low Hz, band high Hz, decay seconds, amplitude.
    pub(crate) noise: (f32, f32, f32, f32),
    /// Hz, decay seconds, amplitude.
    pub(crate) tones: &'static [(f32, f32, f32)],
}

pub(crate) struct Profile {
    pub(crate) press: &'static [Hit],
    pub(crate) press_ms: f32,
    pub(crate) release: &'static [Hit],
    pub(crate) release_ms: f32,
    /// Release peak relative to the press.
    pub(crate) release_level: f32,
}

// Lubed linear in a padded case: deep, rounded, short high-end.
static THOCK: Profile = Profile {
    press: &[Hit {
        at: 0.0,
        noise: (400.0, 2400.0, 0.004, 0.7),
        tones: &[(240.0, 0.040, 1.0), (410.0, 0.024, 0.55), (650.0, 0.014, 0.3), (1900.0, 0.010, 0.10), (3200.0, 0.007, 0.07)],
    }],
    press_ms: 120.0,
    release: &[Hit {
        at: 0.0,
        noise: (900.0, 3500.0, 0.002, 0.5),
        tones: &[(310.0, 0.018, 0.5), (520.0, 0.010, 0.3), (2300.0, 0.006, 0.08)],
    }],
    release_ms: 60.0,
    release_level: 0.5,
};

// Blue-style click jacket: a sharp leaf click, then the bottom-out a few ms later.
static CLICKY: Profile = Profile {
    press: &[
        Hit { at: 0.0, noise: (2500.0, 9000.0, 0.0007, 1.0), tones: &[(4600.0, 0.005, 0.5), (6800.0, 0.003, 0.3)] },
        Hit { at: 0.0015, noise: (2000.0, 6000.0, 0.0005, 0.45), tones: &[(3900.0, 0.003, 0.3)] },
        Hit {
            at: 0.007,
            noise: (500.0, 3500.0, 0.003, 0.7),
            tones: &[(300.0, 0.025, 0.8), (520.0, 0.014, 0.45), (2400.0, 0.008, 0.15)],
        },
    ],
    press_ms: 110.0,
    release: &[
        Hit { at: 0.0, noise: (2500.0, 8000.0, 0.0006, 0.7), tones: &[(4200.0, 0.004, 0.35), (6200.0, 0.003, 0.2)] },
        Hit { at: 0.003, noise: (800.0, 3000.0, 0.002, 0.35), tones: &[(360.0, 0.015, 0.35)] },
    ],
    release_ms: 60.0,
    release_level: 0.6,
};

// Light, poppy linear: higher and brighter than thock.
static LINEAR: Profile = Profile {
    press: &[Hit {
        at: 0.0,
        noise: (800.0, 3500.0, 0.003, 0.55),
        tones: &[(330.0, 0.028, 0.9), (560.0, 0.016, 0.5), (900.0, 0.009, 0.25), (2200.0, 0.008, 0.18), (3800.0, 0.005, 0.1)],
    }],
    press_ms: 100.0,
    release: &[Hit {
        at: 0.0,
        noise: (1200.0, 4500.0, 0.0018, 0.45),
        tones: &[(420.0, 0.014, 0.45), (700.0, 0.008, 0.25), (2800.0, 0.005, 0.08)],
    }],
    release_ms: 50.0,
    release_level: 0.5,
};

// IBM Model M-style buckling spring: the spring snaps with a long metallic
// ping, then the hammer bottoms out on the steel plate and plastic case.
static BUCKLING: Profile = Profile {
    press: &[
        Hit {
            at: 0.0,
            noise: (2000.0, 9000.0, 0.001, 0.8),
            tones: &[(2300.0, 0.045, 0.55), (3450.0, 0.035, 0.4), (5200.0, 0.020, 0.25), (7100.0, 0.012, 0.12)],
        },
        Hit {
            at: 0.004,
            noise: (600.0, 3000.0, 0.004, 0.8),
            tones: &[(210.0, 0.035, 0.9), (380.0, 0.022, 0.5), (820.0, 0.012, 0.3)],
        },
    ],
    press_ms: 150.0,
    release: &[
        Hit { at: 0.0, noise: (2000.0, 7000.0, 0.0012, 0.6), tones: &[(2600.0, 0.030, 0.35), (4100.0, 0.020, 0.2)] },
        Hit { at: 0.003, noise: (800.0, 3500.0, 0.002, 0.5), tones: &[(300.0, 0.018, 0.45), (700.0, 0.010, 0.2)] },
    ],
    release_ms: 80.0,
    release_level: 0.6,
};

#[derive(Clone, Copy)]
pub(crate) enum Class {
    Normal,
    /// Shift, enter, backspace, tab, caps and other stabilised keys.
    Wide,
    Space,
}

const CLASSES: [Class; 3] = [Class::Normal, Class::Wide, Class::Space];

impl Class {
    /// Bigger keys ring lower.
    fn freq(self) -> f32 {
        [1.0, 0.85, 0.7][self as usize]
    }

    fn tail(self) -> f32 {
        [1.0, 1.15, 1.3][self as usize]
    }

    /// Stabiliser rattle level.
    fn rattle(self) -> f32 {
        [0.0, 0.2, 0.4][self as usize]
    }
}

fn class_of(code: u16) -> Class {
    const WIDE: [KeyCode; 13] = [
        KeyCode::KEY_ENTER,
        KeyCode::KEY_KPENTER,
        KeyCode::KEY_BACKSPACE,
        KeyCode::KEY_TAB,
        KeyCode::KEY_CAPSLOCK,
        KeyCode::KEY_LEFTSHIFT,
        KeyCode::KEY_RIGHTSHIFT,
        KeyCode::KEY_LEFTCTRL,
        KeyCode::KEY_RIGHTCTRL,
        KeyCode::KEY_LEFTALT,
        KeyCode::KEY_RIGHTALT,
        KeyCode::KEY_LEFTMETA,
        KeyCode::KEY_RIGHTMETA,
    ];
    if code == KeyCode::KEY_SPACE.0 {
        Class::Space
    } else if WIDE.iter().any(|k| k.0 == code) {
        Class::Wide
    } else {
        Class::Normal
    }
}

/// A fixed ±4% pitch offset per key, so each key keeps its own character.
fn key_pitch(code: u16) -> f32 {
    let h = (code as u32).wrapping_mul(2_654_435_761) >> 24;
    1.0 + 0.04 * (h as f32 / 127.5 - 1.0)
}

pub(crate) struct Rng(u32);

impl Rng {
    pub(crate) fn seeded() -> Self {
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
        Rng(nanos | 1)
    }

    pub(crate) fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// 0..1
    pub(crate) fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / 16_777_216.0
    }

    /// -1..1
    pub(crate) fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

fn add_hit(out: &mut [f32], hit: &Hit, freq: f32, tail: f32, rng: &mut Rng) {
    let start = (hit.at * RATE as f32) as usize;
    let (lo, hi, noise_tau, noise_amp) = hit.noise;
    let pole = |fc: f32| 1.0 - (-TAU * fc / RATE as f32).exp();
    let (a_lo, a_hi) = (pole(lo), pole(hi));
    let (mut s_lo, mut s_hi) = (0.0f32, 0.0f32);
    let phases: Vec<f32> = hit.tones.iter().map(|_| rng.unit() * TAU).collect();
    for (k, o) in out.iter_mut().enumerate().skip(start) {
        let dt = (k - start) as f32 / RATE as f32;
        let white = rng.signed();
        s_hi += a_hi * (white - s_hi);
        s_lo += a_lo * (white - s_lo);
        let mut v = (s_hi - s_lo) * 3.0 * noise_amp * (-dt / noise_tau).exp();
        for (&(hz, tau, amp), &phase) in hit.tones.iter().zip(&phases) {
            v += (TAU * hz * freq * dt + phase).sin() * (-dt / (tau * tail)).exp() * amp;
        }
        *o += v * (dt * 20_000.0).min(1.0);
    }
}

/// Renders one stroke, normalised to `peak`.
pub(crate) fn render(hits: &[Hit], ms: f32, class: Class, press: bool, peak: f32, rng: &mut Rng) -> Vec<f32> {
    let n = (RATE as f32 * ms * class.tail() / 1000.0) as usize;
    let mut out = vec![0.0f32; n];
    // Every variant gets its own slight detune and noise.
    let freq = class.freq() * (1.0 + 0.03 * rng.signed());
    for hit in hits {
        add_hit(&mut out, hit, freq, class.tail(), rng);
    }
    if press && class.rattle() > 0.0 {
        let mut at = 0.005;
        for _ in 0..3 {
            let rattle = Hit { at, noise: (1500.0, 5000.0, 0.0015, class.rattle()), tones: &[] };
            add_hit(&mut out, &rattle, freq, 1.0, rng);
            at += 0.005 + rng.unit() * 0.004;
        }
    }
    let max = out.iter().fold(1e-6f32, |m, x| m.max(x.abs()));
    let fade = (RATE as f32 * 0.003) as usize;
    for (i, x) in out.iter_mut().enumerate() {
        *x *= peak / max * ((n - i).min(fade) as f32 / fade as f32);
    }
    out
}

/// Recorded samples per key code.
struct Pack {
    press: HashMap<u16, Arc<Vec<u8>>>,
    release: HashMap<u16, Arc<Vec<u8>>>,
}

/// Decodes the embedded pack: see `tools/pack_bucklespring.py` for the format.
fn embedded_pack() -> Pack {
    let mut pack = Pack { press: HashMap::new(), release: HashMap::new() };
    let b = BUCKLESPRING;
    let le32 = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let count = u16::from_le_bytes([b[0], b[1]]);
    let mut pos = 2;
    for _ in 0..count {
        let (code, stroke) = (b[pos] as u16, b[pos + 1]);
        let (rate, frames) = (le32(pos + 2), le32(pos + 6) as usize);
        pos += 10;
        let mono: Vec<f32> = b[pos..pos + frames * 2]
            .as_chunks::<2>().0.iter()
            .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
            .collect();
        pos += frames * 2;
        let map = if stroke == 0 { &mut pack.press } else { &mut pack.release };
        map.insert(code, Arc::new(sound::finish(&mono, rate, 100)));
    }
    pack
}

fn load_pack(dir: &str) -> Result<Pack, String> {
    let mut pack = Pack { press: HashMap::new(), release: HashMap::new() };
    for code in 0..0x100u16 {
        for (stroke, map) in [(0, &mut pack.press), (1, &mut pack.release)] {
            for sub in ["", "wav/"] {
                let path = Path::new(dir).join(format!("{sub}{code:02x}-{stroke}.wav"));
                if path.exists() {
                    let pcm = sound::from_wav(&path.to_string_lossy(), 100)?;
                    map.insert(code, Arc::new(pcm));
                    break;
                }
            }
        }
    }
    if pack.press.is_empty() {
        return Err(format!("{dir}: no <hex keycode>-0.wav files found (bucklespring layout)"));
    }
    Ok(pack)
}

pub struct Keyboard {
    pack: Option<Pack>,
    /// `[class][variant]`
    press: Vec<Vec<Arc<Vec<u8>>>>,
    release: Vec<Vec<Arc<Vec<u8>>>>,
    volume: f32,
    rng: Rng,
    /// The previous event's code, direction and time, to drop duplicates when
    /// a remapper exposes the same keyboard twice.
    last: Option<(u16, bool, Instant)>,
}

impl Keyboard {
    pub fn new(profile: &str, volume: u32) -> Result<Self, String> {
        if profile == RECORDED || profile.contains('/') {
            let pack = if profile == RECORDED { embedded_pack() } else { load_pack(profile)? };
            let (press, release) = (Vec::new(), Vec::new());
            return Ok(Keyboard { pack: Some(pack), press, release, volume: volume as f32 / 100.0, rng: Rng::seeded(), last: None });
        }
        let p = match profile {
            "thock" => &THOCK,
            "clicky" => &CLICKY,
            "linear" => &LINEAR,
            "buckling" => &BUCKLING,
            _ => return Err(format!("unknown keyboard sound '{profile}' (choose off, {}, or a sample directory)", SOUNDS.join(", "))),
        };
        let mut rng = Rng::seeded();
        let mut bank = |hits: &[Hit], ms: f32, press: bool, peak: f32| -> Vec<Vec<Arc<Vec<u8>>>> {
            CLASSES
                .iter()
                .map(|&class| {
                    (0..VARIANTS)
                        .map(|_| Arc::new(sound::to_pcm(&render(hits, ms, class, press, peak, &mut rng), 100)))
                        .collect()
                })
                .collect()
        };
        let press = bank(p.press, p.press_ms, true, 1.0);
        let release = bank(p.release, p.release_ms, false, p.release_level);
        Ok(Keyboard { pack: None, press, release, volume: volume as f32 / 100.0, rng: Rng::seeded(), last: None })
    }

    /// The recorded press sample for a key (bucklespring), for loudness tests.
    #[cfg(test)]
    pub(crate) fn recorded_press(&self, code: u16) -> Option<Arc<Vec<u8>>> {
        self.pack.as_ref()?.press.get(&code).cloned()
    }

    pub fn handle(&mut self, ev: KeyEvent, player: &Player) {
        let now = Instant::now();
        if let Some((code, down, at)) = self.last {
            if code == ev.code && down == ev.down && now.duration_since(at) < Duration::from_millis(8) {
                return;
            }
        }
        self.last = Some((ev.code, ev.down, now));
        let (pcm, rate) = match &self.pack {
            Some(pack) => {
                let map = if ev.down { &pack.press } else { &pack.release };
                let pcm = map.get(&ev.code).or_else(|| {
                    // No sample for this key: borrow another one.
                    let n = self.rng.next() as usize % map.len().max(1);
                    map.values().nth(n)
                });
                let Some(pcm) = pcm else { return };
                (pcm, 1.0 + 0.015 * self.rng.signed())
            }
            None => {
                let bank = if ev.down { &self.press } else { &self.release };
                let pcm = &bank[class_of(ev.code) as usize][self.rng.next() as usize % VARIANTS];
                (pcm, key_pitch(ev.code) * (1.0 + 0.02 * self.rng.signed()))
            }
        };
        let gain = self.volume * (0.85 + 0.15 * self.rng.unit());
        player.play_with(pcm, gain, rate);
    }

    /// Types "hello world" and enter at a human pace. Blocks until done.
    pub fn demo(&mut self, player: &Player) {
        for code in [35u16, 18, 38, 38, 24, 57, 17, 24, 19, 38, 32, 28] {
            self.handle(KeyEvent { code, down: true }, player);
            thread::sleep(Duration::from_millis(55 + (self.rng.unit() * 25.0) as u64));
            self.handle(KeyEvent { code, down: false }, player);
            thread::sleep(Duration::from_millis(60 + (self.rng.unit() * 70.0) as u64));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(pcm: &[u8]) -> i16 {
        pcm.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes([b[0], b[1]]).abs()).max().unwrap()
    }

    #[test]
    fn every_profile_renders_all_classes_with_quieter_release() {
        for name in SOUNDS.iter().filter(|n| **n != RECORDED) {
            let k = Keyboard::new(name, 100).unwrap();
            for class in 0..CLASSES.len() {
                for v in 0..VARIANTS {
                    let (p, r) = (&k.press[class][v], &k.release[class][v]);
                    assert!(p.len() > 1000 && r.len() > 1000, "{name}");
                    assert!(peak(r) < peak(p), "{name}");
                    assert!(peak(p) > 20_000, "{name}");
                }
            }
        }
    }

    #[test]
    fn embedded_bucklespring_pack_covers_the_main_keys() {
        let k = Keyboard::new(RECORDED, 100).unwrap();
        let pack = k.pack.as_ref().unwrap();
        for key in [KeyCode::KEY_A, KeyCode::KEY_SPACE, KeyCode::KEY_ENTER, KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_BACKSPACE] {
            assert!(pack.press[&key.0].len() > 2000, "{key:?}");
            assert!(pack.release.contains_key(&key.0), "{key:?}");
        }
    }

    #[test]
    fn variants_differ() {
        let k = Keyboard::new("thock", 50).unwrap();
        assert_ne!(k.press[0][0], k.press[0][1]);
    }

    #[test]
    fn unknown_profile_is_an_error() {
        assert!(Keyboard::new("nope", 50).is_err());
    }

    fn write_wav(path: &Path, samples: &[i16]) {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut b = b"RIFF".to_vec();
        b.extend((36 + data.len() as u32).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        for v in [1u16, 1] {
            b.extend(v.to_le_bytes());
        }
        b.extend(48000u32.to_le_bytes());
        b.extend(96000u32.to_le_bytes());
        for v in [2u16, 16] {
            b.extend(v.to_le_bytes());
        }
        b.extend(b"data");
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(data);
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn sample_pack_loads_and_bad_dir_errors() {
        let dir = std::env::temp_dir().join(format!("heptix-pack-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("wav")).unwrap();
        write_wav(&dir.join("wav/1e-0.wav"), &[1000; 100]);
        write_wav(&dir.join("wav/1e-1.wav"), &[500; 100]);
        let k = Keyboard::new(&dir.to_string_lossy(), 100).unwrap();
        let pack = k.pack.as_ref().unwrap();
        assert_eq!((pack.press.len(), pack.release.len()), (1, 1));
        assert!(Keyboard::new("/nonexistent/dir", 100).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn key_classes() {
        assert!(matches!(class_of(KeyCode::KEY_SPACE.0), Class::Space));
        assert!(matches!(class_of(KeyCode::KEY_LEFTSHIFT.0), Class::Wide));
        assert!(matches!(class_of(KeyCode::KEY_A.0), Class::Normal));
    }

    #[test]
    fn pitch_is_stable_per_key_and_bounded() {
        for code in 0..0x100u16 {
            let p = key_pitch(code);
            assert_eq!(p, key_pitch(code));
            assert!((0.96..=1.04).contains(&p));
        }
    }
}
