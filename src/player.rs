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

use crate::sound::RATE;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Mixing granularity: 5 ms of audio per write.
const CHUNK: usize = RATE as usize / 200;
/// How far ahead of the wall clock we let written audio get. Bounds the delay
/// before a new sound can start while another is still playing.
const LOOKAHEAD: Duration = Duration::from_millis(10);

/// Candidate raw-PCM sinks, tried in order. Each reads s16le mono 48 kHz on stdin.
fn candidates() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("pw-cat", vec!["--playback", "--raw", "--latency", "20ms", "--rate", "48000", "--channels", "1", "--format", "s16", "-"]),
        ("paplay", vec!["--raw", "--rate=48000", "--channels=1", "--format=s16le"]),
        ("aplay", vec!["-q", "-t", "raw", "-r", "48000", "-c", "1", "-f", "S16_LE"]),
    ]
}

fn spawn_sink() -> Option<Child> {
    for (bin, args) in candidates() {
        if let Ok(child) = Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            return Some(child);
        }
    }
    None
}

struct Voice {
    pcm: Arc<Vec<u8>>,
    pos: f32,
    step: f32,
    gain: f32,
}

impl Voice {
    fn len(&self) -> usize {
        self.pcm.len() / 2
    }

    fn at(&self, i: usize) -> f32 {
        i16::from_le_bytes([self.pcm[2 * i], self.pcm[2 * i + 1]]) as f32 / 32768.0
    }
}

/// Mixes the next `CHUNK` samples of every voice and drops the finished ones.
fn mix(voices: &mut Vec<Voice>, master: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(CHUNK * 2);
    for _ in 0..CHUNK {
        let mut acc = 0.0f32;
        for v in voices.iter_mut() {
            let (n, i) = (v.len(), v.pos as usize);
            if i >= n {
                continue;
            }
            let a = v.at(i);
            let b = if i + 1 < n { v.at(i + 1) } else { a };
            acc += (a + (b - a) * v.pos.fract()) * v.gain;
            v.pos += v.step;
        }
        out.extend((((acc * master).clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    voices.retain(|v| (v.pos as usize) < v.len());
    out
}

fn write_sink(sink: &mut Option<Child>, data: &[u8], warned: &mut bool) {
    for _ in 0..2 {
        if sink.is_none() {
            *sink = spawn_sink();
            if sink.is_none() {
                if !*warned {
                    eprintln!("no audio player found (tried pw-cat, paplay, aplay)");
                    *warned = true;
                }
                return;
            }
        }
        let ok = sink
            .as_mut()
            .and_then(|c| c.stdin.as_mut())
            .is_some_and(|w| w.write_all(data).and_then(|_| w.flush()).is_ok());
        if ok {
            return;
        }
        if let Some(mut dead) = sink.take() {
            let _ = dead.kill();
            let _ = dead.wait();
        }
    }
}

#[derive(Clone)]
pub struct Player {
    tx: Sender<Voice>,
    /// Master gain (f32 bits) applied to everything mixed; see `set_master`.
    master: Arc<AtomicU32>,
}

impl Player {
    /// Keeps one audio stream open so each sound has low latency instead of
    /// paying process-spawn cost per play. Overlapping sounds are mixed, so
    /// fast typing does not queue up behind earlier clicks.
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel::<Voice>();
        let master = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let thread_master = Arc::clone(&master);
        thread::spawn(move || {
            // Open the stream now and prime it with silence, so the first sound
            // is not clipped by stream start-up.
            let mut sink = spawn_sink();
            if let Some(w) = sink.as_mut().and_then(|c| c.stdin.as_mut()) {
                let _ = w.write_all(&vec![0u8; RATE as usize / 10 * 2]);
            }
            let (mut voices, mut warned) = (Vec::new(), false);
            let mut deadline = Instant::now();
            loop {
                if voices.is_empty() {
                    match rx.recv() {
                        Ok(v) => voices.push(v),
                        Err(_) => break,
                    }
                    deadline = Instant::now();
                }
                while let Ok(v) = rx.try_recv() {
                    voices.push(v);
                }
                write_sink(&mut sink, &mix(&mut voices, f32::from_bits(thread_master.load(Ordering::Relaxed))), &mut warned);
                deadline = deadline.max(Instant::now()) + Duration::from_millis(5);
                let ahead = deadline.saturating_duration_since(Instant::now());
                if ahead > LOOKAHEAD {
                    thread::sleep(ahead - LOOKAHEAD);
                }
            }
        });
        Player { tx, master }
    }

    /// Scales everything played from now on (1.0 = unchanged).
    pub fn set_master(&self, gain: f32) {
        self.master.store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn play(&self, pcm: &Arc<Vec<u8>>) {
        self.play_with(pcm, 1.0, 1.0);
    }

    /// `gain` scales the level; `rate` > 1 plays faster (higher pitch).
    pub fn play_with(&self, pcm: &Arc<Vec<u8>>, gain: f32, rate: f32) {
        let _ = self.tx.send(Voice { pcm: Arc::clone(pcm), pos: 0.0, step: rate, gain });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(samples: &[i16], gain: f32) -> Voice {
        let pcm: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        Voice { pcm: Arc::new(pcm), pos: 0.0, step: 1.0, gain }
    }

    fn first_sample(out: &[u8]) -> i16 {
        i16::from_le_bytes([out[0], out[1]])
    }

    #[test]
    fn overlapping_voices_sum() {
        let mut one = vec![voice(&[8000; 10], 1.0)];
        let mut two = vec![voice(&[8000; 10], 1.0), voice(&[8000; 10], 0.5)];
        let a = first_sample(&mix(&mut one, 1.0));
        let b = first_sample(&mix(&mut two, 1.0));
        assert!(b > a + a / 3, "{a} {b}");
    }

    #[test]
    fn master_gain_scales_the_mix() {
        let full = first_sample(&mix(&mut vec![voice(&[8000; 10], 1.0)], 1.0));
        let half = first_sample(&mix(&mut vec![voice(&[8000; 10], 1.0)], 0.5));
        assert!((full as f32 * 0.5 - half as f32).abs() <= 1.0, "{full} {half}");
    }

    #[test]
    fn finished_voices_are_dropped_and_output_is_silent_after() {
        let mut v = vec![voice(&[1000; 10], 1.0)];
        let out = mix(&mut v, 1.0);
        assert!(v.is_empty());
        assert_eq!(out.len(), CHUNK * 2);
        assert_eq!(i16::from_le_bytes([out[40], out[41]]), 0);
    }
}
