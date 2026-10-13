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

use std::f32::consts::TAU;
use std::fs;

pub const RATE: u32 = 48_000;
/// Recorded sounds embedded in the binary (see `assets/README.md`).
static WHEEL_OF_TIME: &[u8] = include_bytes!("../assets/wheel_of_time.caf");

/// Every built-in sound, for menus.
pub const SOUNDS: &[&str] = &["haptic", "tock", "tick", "thud", "crown", "wheel_of_time"];

/// Signed 16-bit little-endian mono at `RATE`, ready to write to an audio sink.
pub type Pcm = Vec<u8>;

fn render(ms: f32, mut f: impl FnMut(f32, f32) -> f32) -> Vec<f32> {
    let n = (RATE as f32 * ms / 1000.0) as usize;
    let mut noise: u32 = 0x1234_5678;
    (0..n)
        .map(|i| {
            noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = (noise >> 16) as f32 / 32768.0 - 1.0;
            f(i as f32 / RATE as f32, white)
        })
        .collect()
}

pub fn to_pcm(samples: &[f32], volume: u32) -> Pcm {
    let amp = 32767.0 * 0.9 * (volume as f32 / 100.0);
    samples
        .iter()
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * amp) as i16).to_le_bytes())
        .collect()
}

pub fn preset(name: &str, volume: u32) -> Option<Pcm> {
    let samples = match name {
        // MacBook-style haptic: a short, low "thump" with a faint tick on
        // top. Laptop speakers are weak below ~150 Hz, so the pitch sits
        // just above that to stay audible.
        "haptic" => render(30.0, |t, white| {
            let attack = (t * 1000.0).min(1.0);
            let freq = 110.0 + 90.0 * (-t * 350.0).exp();
            let thump = (t * freq * TAU).sin() * (-t * 170.0).exp();
            let tick = white * (-t * 2500.0).exp() * 0.3;
            (thump + tick) * attack
        }),
        // Soft keyboard-style tock.
        "tock" => render(40.0, |t, white| {
            let tick = white * (-t * 1200.0).exp() * 0.5;
            let freq = 520.0 + 700.0 * (-t * 250.0).exp();
            let body = (t * freq * TAU).sin() * (-t * 160.0).exp();
            tick + body * 0.8
        }),
        // Light, crisp tick.
        "tick" => render(12.0, |t, white| {
            let ping = (t * 2600.0 * TAU).sin() * (-t * 700.0).exp();
            white * (-t * 2000.0).exp() * 0.4 + ping * 0.6
        }),
        // Mechanical watch-crown detent: a metallic double snap. The first is
        // the click with a short ring (two inharmonic partials); the second,
        // quieter and ~3.5 ms later, is the pawl dropping into the next tooth.
        "crown" => {
            let mut prev = 0.0f32;
            render(18.0, move |t, white| {
                let hp = (white - prev) * 0.5;
                prev = white;
                let snap = |dt: f32, f1: f32, f2: f32| {
                    if dt < 0.0 {
                        return 0.0;
                    }
                    let ring = (dt * f1 * TAU).sin() * (-dt * 650.0).exp() * 0.5
                        + (dt * f2 * TAU).sin() * (-dt * 900.0).exp() * 0.3;
                    (hp * (-dt * 2200.0).exp() * 0.9 + ring) * (dt * 20000.0).min(1.0)
                };
                snap(t, 2350.0, 4150.0) + 0.55 * snap(t - 0.0035, 2900.0, 5000.0)
            })
        }
        // Heavy, deep thud.
        "thud" => render(60.0, |t, white| {
            let attack = (t * 800.0).min(1.0);
            let freq = 70.0 + 60.0 * (-t * 200.0).exp();
            let body = (t * freq * TAU).sin() * (-t * 90.0).exp();
            (body + white * (-t * 1500.0).exp() * 0.2) * attack
        }),
        _ => return None,
    };
    Some(to_pcm(&samples, volume))
}

/// Loads a 16-bit PCM WAV of any channel count and sample rate.
pub fn from_wav(path: &str, volume: u32) -> Result<Pcm, String> {
    let b = fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let (mono, rate) = decode_wav(&b, path)?;
    Ok(finish(&mono, rate, volume))
}

/// Decodes a 16-bit PCM WAV to mono samples in -1..1 and its sample rate.
/// `path` is only used in error messages.
pub fn decode_wav(b: &[u8], path: &str) -> Result<(Vec<f32>, u32), String> {
    let bad = |m: &str| format!("{path}: {m} (need 16-bit PCM WAV; convert with `ffmpeg -i in -c:a pcm_s16le out.wav`)");
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(bad("not a WAV file"));
    }
    let (mut channels, mut rate, mut data) = (0usize, 0u32, None);
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &b[pos + 8..(pos + 8 + size).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            let format = u16::from_le_bytes([body[0], body[1]]);
            let bits = u16::from_le_bytes([body[14], body[15]]);
            if format != 1 || bits != 16 {
                return Err(bad("unsupported encoding"));
            }
            channels = u16::from_le_bytes([body[2], body[3]]) as usize;
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
        } else if id == b"data" {
            data = Some(body);
        }
        pos += 8 + size + (size & 1);
    }
    let data = data.ok_or_else(|| bad("no audio data"))?;
    if channels == 0 || rate == 0 {
        return Err(bad("missing format"));
    }
    let mono: Vec<f32> = data
        .chunks_exact(2 * channels)
        .map(|frame| {
            let sum: f32 = frame
                .as_chunks::<2>().0.iter()
                .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
                .sum();
            sum / channels as f32
        })
        .collect();
    Ok((mono, rate))
}

/// Linear resample to the sink rate, then convert to PCM.
pub fn finish(mono: &[f32], rate: u32, volume: u32) -> Pcm {
    if mono.is_empty() {
        return Vec::new();
    }
    let n = (mono.len() as u64 * RATE as u64 / rate as u64) as usize;
    let out: Vec<f32> = (0..n)
        .map(|i| {
            let p = i as f32 * rate as f32 / RATE as f32;
            let (i0, frac) = (p as usize, p.fract());
            let a = mono[i0.min(mono.len() - 1)];
            let b = mono[(i0 + 1).min(mono.len() - 1)];
            a + (b - a) * frac
        })
        .collect();
    to_pcm(&out, volume)
}

/// Decodes an Apple CAF with linear PCM (16/24/32-bit integer or 32-bit float,
/// either endianness), mixing channels down to mono.
fn from_caf_bytes(b: &[u8], volume: u32) -> Result<Pcm, String> {
    let bad = |m: &str| format!("CAF: {m} (only linear PCM is supported)");
    if b.len() < 8 || &b[0..4] != b"caff" {
        return Err(bad("not a CAF file"));
    }
    let (mut fmt, mut data) = (None, None);
    let mut pos = 8;
    while pos + 12 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = i64::from_be_bytes(b[pos + 4..pos + 12].try_into().unwrap());
        let end = if size < 0 { b.len() } else { (pos + 12 + size as usize).min(b.len()) };
        let body = &b[pos + 12..end];
        match id {
            b"desc" if body.len() >= 32 => {
                let rate = f64::from_be_bytes(body[0..8].try_into().unwrap());
                let flags = u32::from_be_bytes(body[12..16].try_into().unwrap());
                let channels = u32::from_be_bytes(body[24..28].try_into().unwrap()) as usize;
                let bits = u32::from_be_bytes(body[28..32].try_into().unwrap()) as usize;
                if &body[8..12] != b"lpcm" {
                    return Err(bad("compressed audio"));
                }
                fmt = Some((rate as u32, flags, channels, bits));
            }
            b"data" if body.len() > 4 => data = Some(&body[4..]), // 4-byte edit count first
            _ => {}
        }
        pos = end;
    }
    let ((rate, flags, channels, bits), data) = fmt.zip(data).ok_or_else(|| bad("missing data"))?;
    let (float, little) = (flags & 1 != 0, flags & 2 != 0);
    let width = bits / 8;
    if channels == 0 || rate == 0 || !(2..=4).contains(&width) || (float && width != 4) {
        return Err(bad("unsupported sample format"));
    }
    let sample = |bytes: &[u8]| -> f32 {
        let mut v = [0u8; 4];
        if little {
            v[..width].copy_from_slice(bytes);
        } else {
            v[4 - width..].copy_from_slice(bytes);
            v.reverse();
        }
        // `v` is now little-endian padded at the top; sign-extend from `width` bytes.
        if float {
            return f32::from_le_bytes(v);
        }
        let raw = u32::from_le_bytes(v) << (32 - bits);
        (raw as i32) as f32 / 2_147_483_648.0
    };
    let mono: Vec<f32> = data
        .chunks_exact(width * channels)
        .map(|frame| frame.chunks_exact(width).map(sample).sum::<f32>() / channels as f32)
        .collect();
    Ok(finish(&mono, rate, volume))
}

pub fn load(spec: &str, volume: u32) -> Result<Pcm, String> {
    if let Some(p) = preset(spec, volume) {
        return Ok(p);
    }
    if spec == "wheel_of_time" {
        return from_caf_bytes(WHEEL_OF_TIME, volume);
    }
    let lower = spec.to_lowercase();
    if lower.ends_with(".caf") {
        let bytes = fs::read(spec).map_err(|e| format!("{spec}: {e}"))?;
        return from_caf_bytes(&bytes, volume).map_err(|e| format!("{spec}: {e}"));
    }
    if spec.contains('/') || lower.ends_with(".wav") {
        return from_wav(spec, volume);
    }
    Err(format!("unknown sound '{spec}' (built-in: {}; or a .wav/.caf path)", SOUNDS.join(", ")))
}

/// A-weighted level in dB of 48 kHz mono s16 PCM (the first 50 ms), scaled by `gain`, so
/// that sounds can be compared by how loud they seem rather than by their peak.
#[cfg(test)]
pub fn a_weighted_db(pcm: &[u8], gain: f32) -> f32 {
    let x: Vec<f32> = pcm.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0 * gain).collect();
    let n = x.len().min(2400);
    let weight = |f: f32| {
        let f2 = f * f;
        let ra = 12194.0f32.powi(2) * f2 * f2
            / ((f2 + 20.6f32.powi(2)) * ((f2 + 107.7f32.powi(2)) * (f2 + 737.9f32.powi(2))).sqrt() * (f2 + 12194.0f32.powi(2)));
        ra / 0.7943 // 0 dB at 1 kHz
    };
    let mut power = 0.0f64;
    for k in 1..n / 2 {
        let f = k as f32 * RATE as f32 / n as f32;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().take(n).enumerate() {
            let phase = -2.0 * std::f64::consts::PI * (k * i) as f64 / n as f64;
            re += *v as f64 * phase.cos();
            im += *v as f64 * phase.sin();
        }
        power += (re * re + im * im) / (n * n) as f64 * (weight(f) as f64).powi(2);
    }
    10.0 * (power.max(1e-20) as f32).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_render_nonempty_and_scale_with_volume() {
        for name in SOUNDS.iter().filter(|n| **n != "wheel_of_time") {
            let loud = preset(name, 100).unwrap();
            let quiet = preset(name, 10).unwrap();
            assert!(!loud.is_empty());
            let peak = |p: &Pcm| p.as_chunks::<2>().0.iter().map(|s| i16::from_le_bytes([s[0], s[1]]).abs()).max().unwrap();
            assert!(peak(&loud) > peak(&quiet) * 5, "{name}");
        }
    }

    #[test]
    fn caf_24bit_little_endian_decodes() {
        let mut caf = b"caff\x00\x01\x00\x00desc".to_vec();
        caf.extend(32i64.to_be_bytes());
        caf.extend(48000f64.to_be_bytes());
        caf.extend(b"lpcm");
        for v in [2u32, 3, 1, 1, 24] {
            caf.extend(v.to_be_bytes());
        }
        caf.extend(b"data");
        caf.extend(((4 + 3 * 3) as i64).to_be_bytes());
        caf.extend(0u32.to_be_bytes()); // edit count
        for sample in [0i32, 4_194_304, -4_194_304] {
            caf.extend(&sample.to_le_bytes()[..3]);
        }
        let pcm = from_caf_bytes(&caf, 100).unwrap();
        let s: Vec<i16> = pcm.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(s.len(), 3);
        assert!(s[1] > 10_000 && s[2] < -10_000, "{s:?}");
    }

    #[test]
    fn wheel_of_time_is_embedded_and_scales_with_volume() {
        let loud = load("wheel_of_time", 100).unwrap();
        let quiet = load("wheel_of_time", 10).unwrap();
        let peak = |p: &Pcm| p.as_chunks::<2>().0.iter().map(|s| i16::from_le_bytes([s[0], s[1]]).abs()).max().unwrap();
        assert!(loud.len() > 1000);
        assert!(peak(&loud) > peak(&quiet) * 5);
    }

    #[test]
    fn unknown_sound_is_an_error() {
        assert!(load("nope", 50).is_err());
    }
}
