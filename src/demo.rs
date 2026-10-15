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

//! `heptix --demo`: simulated typing, clicking and tapping, for hearing the sounds and
//! showing them off. It feeds made-up events through the same sound code the real
//! events use, and draws a keyboard, a mouse and a touchpad that light up as they are
//! used. Nothing is read from your devices and nothing is recorded: this module has no
//! access to the stats store.

use crate::config::Settings;
use crate::heatmap::KEYBOARD_ROWS;
use crate::keyboard::{KeyEvent, Keyboard};
use crate::mouse::Mouse;
use crate::player::Player;
use crate::tui::{term_width, RawMode};
use crate::sysvol;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

const KEY_BACKSPACE: u16 = 14;
const KEY_ENTER: u16 = 28;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;
const BTN_LEFT: u16 = 272;
const BTN_RIGHT: u16 = 273;

/// One simulated input event.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sim {
    Key { code: u16, down: bool },
    Button { code: u16, down: bool },
    Wheel { up: bool },
    Tap,
    PadClick,
    PadScroll,
}

/// Events of one stretch of activity, in milliseconds from its start.
type Timed = Vec<(u64, Sim)>;

struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Rng {
        // Small seeds start with small values: churn them before first use.
        let mut r = Rng(seed | 1);
        for _ in 0..8 {
            r.next();
        }
        r
    }

    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// 0..1
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / 16_777_216.0
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.unit() * (hi - lo)
    }

    fn ms(&mut self, lo: u64, hi: u64) -> u64 {
        self.range(lo as f32, hi as f32) as u64
    }

    fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
}

// --- typing -------------------------------------------------------------------------

const SENTENCES: [&str; 8] = [
    "The quick brown fox jumps over the lazy dog.",
    "heptix makes every key feel like a mechanical switch.",
    "Did you hear that? Nothing is recorded, this is only a demo.",
    "Thanks for the update, I will review the pull request after lunch.",
    "Remember to buy milk, eggs and coffee.",
    "let total = items.len() + 1;",
    "Meeting moved to 3:30, bring the notes from last week.",
    "It sounds better with the volume turned up a little!",
];

/// The key and whether Shift is held to type a character (US layout).
fn key_for(c: char) -> Option<(u16, bool)> {
    let row = |chars: &str, first: u16| chars.find(c.to_ascii_lowercase()).map(|i| first + i as u16);
    if c.is_ascii_alphabetic() {
        let code = row("qwertyuiop", 16).or_else(|| row("asdfghjkl", 30)).or_else(|| row("zxcvbnm", 44))?;
        return Some((code, c.is_ascii_uppercase()));
    }
    if let Some(i) = "1234567890".find(c) {
        return Some((2 + i as u16, false));
    }
    Some(match c {
        ' ' => (57, false),
        '.' => (52, false),
        ',' => (51, false),
        '/' => (53, false),
        ';' => (39, false),
        '\'' => (40, false),
        '-' => (12, false),
        '=' => (13, false),
        '?' => (53, true),
        '!' => (2, true),
        ':' => (39, true),
        '"' => (40, true),
        '_' => (12, true),
        '+' => (13, true),
        '(' => (10, true),
        ')' => (11, true),
        _ => return None,
    })
}

/// The character a key types, with or without Shift.
fn char_for(code: u16, shift: bool) -> Option<char> {
    (' '..='~').find(|&c| key_for(c) == Some((code, shift)))
}

/// A neighbouring letter on the same keyboard row, for a believable typo.
fn neighbour(c: char, rng: &mut Rng) -> char {
    for row in ["qwertyuiop", "asdfghjkl", "zxcvbnm"] {
        if let Some(i) = row.find(c) {
            let j = if i == 0 || (i + 1 < row.len() && rng.chance(0.5)) { i + 1 } else { i - 1 };
            return row.as_bytes()[j] as char;
        }
    }
    c
}

/// Types `text` at a human pace: rolling over from key to key, holding Shift for capitals,
/// pausing at spaces and commas, and now and then hitting a wrong key and correcting it.
fn typing(rng: &mut Rng, text: &str) -> Timed {
    let mut out: Timed = Vec::new();
    // When Shift is needed: from just before the key goes down to just before it comes up.
    let mut shifted: Vec<(f32, f32)> = Vec::new();
    let wpm = rng.range(55.0, 115.0);
    let base = 60_000.0 / (wpm * 5.0);
    let mut t = 60.0_f32;
    let press = |out: &mut Timed, shifted: &mut Vec<(f32, f32)>, rng: &mut Rng, c: char, t: f32| {
        let Some((code, shift)) = key_for(c) else { return };
        let hold = rng.range(40.0, 95.0);
        if shift {
            // Released well before the next key can go down (the quickest gap is 70 ms).
            shifted.push((t - 40.0, t + (hold - 10.0).min(50.0)));
        }
        out.push((t as u64, Sim::Key { code, down: true }));
        out.push(((t + hold) as u64, Sim::Key { code, down: false }));
    };
    for c in text.chars() {
        if c.is_ascii_lowercase() && rng.chance(0.035) {
            let wrong = neighbour(c, rng);
            press(&mut out, &mut shifted, rng, wrong, t);
            t += base * 1.3 + rng.range(120.0, 260.0);
            let hold = rng.range(40.0, 90.0);
            out.push((t as u64, Sim::Key { code: KEY_BACKSPACE, down: true }));
            out.push(((t + hold) as u64, Sim::Key { code: KEY_BACKSPACE, down: false }));
            t += base * 1.1;
        }
        press(&mut out, &mut shifted, rng, c, t);
        t += base * rng.range(0.7, 1.4);
        match c {
            ' ' => t += base * rng.range(0.0, 0.6),
            ',' | '.' | '?' | '!' => t += rng.range(120.0, 400.0),
            _ if rng.chance(0.02) => t += rng.range(400.0, 900.0),
            _ => {}
        }
    }
    if rng.chance(0.6) {
        t += rng.range(150.0, 400.0);
        out.push((t as u64, Sim::Key { code: KEY_ENTER, down: true }));
        out.push(((t + 70.0) as u64, Sim::Key { code: KEY_ENTER, down: false }));
    }
    // Capitals and symbols that follow each other share one Shift hold.
    let mut hold: Option<(f32, f32)> = None;
    for (start, end) in shifted.into_iter().chain(std::iter::once((f32::MAX, f32::MAX))) {
        match hold {
            Some((from, to)) if start <= to => hold = Some((from, to.max(end))),
            Some((from, to)) => {
                out.push((from.max(0.0) as u64, Sim::Key { code: KEY_LEFTSHIFT, down: true }));
                out.push((to as u64, Sim::Key { code: KEY_LEFTSHIFT, down: false }));
                hold = Some((start, end));
            }
            None => hold = Some((start, end)),
        }
    }
    out.sort_by_key(|(at, _)| *at);
    out
}

// --- mouse and touchpad -------------------------------------------------------------

fn click(out: &mut Timed, rng: &mut Rng, code: u16, at: u64) -> u64 {
    let hold = rng.ms(55, 120);
    out.push((at, Sim::Button { code, down: true }));
    out.push((at + hold, Sim::Button { code, down: false }));
    at + hold
}

/// A few clicks, double clicks and wheel bursts.
fn mouse(rng: &mut Rng) -> Timed {
    let mut out = Vec::new();
    let mut t = 100;
    for _ in 0..rng.ms(2, 6) {
        match rng.next() % 5 {
            0 | 1 => {
                click(&mut out, rng, BTN_LEFT, t);
            }
            2 => {
                let end = click(&mut out, rng, BTN_LEFT, t);
                let gap = rng.ms(80, 140);
                t = click(&mut out, rng, BTN_LEFT, end + gap);
            }
            3 => {
                click(&mut out, rng, BTN_RIGHT, t);
            }
            _ => {
                let up = rng.chance(0.5);
                for _ in 0..rng.ms(3, 11) {
                    out.push((t, Sim::Wheel { up }));
                    t += rng.ms(30, 80);
                }
            }
        }
        t += rng.ms(250, 900);
    }
    out
}

/// Taps, presses of the pad's button and two-finger scrolls.
fn touchpad(rng: &mut Rng) -> Timed {
    let mut out = Vec::new();
    let mut t = 100;
    for _ in 0..rng.ms(3, 8) {
        match rng.next() % 10 {
            0..=4 => out.push((t, Sim::Tap)),
            5 | 6 => out.push((t, Sim::PadClick)),
            _ => {
                for _ in 0..rng.ms(5, 15) {
                    out.push((t, Sim::PadScroll));
                    t += rng.ms(18, 45);
                }
            }
        }
        t += rng.ms(200, 800);
    }
    out
}

/// Feeds stretches of activity, one after another, forever.
struct Script {
    rng: Rng,
    queue: VecDeque<(Instant, Sim)>,
    next_phase: Instant,
    phase: &'static str,
    speed: f32,
}

impl Script {
    fn new(seed: u32, now: Instant) -> Script {
        Script { rng: Rng::new(seed), queue: VecDeque::new(), next_phase: now, phase: "starting", speed: 1.0 }
    }

    /// Starts the next stretch when the last one is over and the pause after it has passed.
    fn refill(&mut self, now: Instant) {
        if !self.queue.is_empty() || now < self.next_phase {
            return;
        }
        let pick = self.rng.unit();
        let (name, events) = if pick < 0.5 {
            let text = SENTENCES[self.rng.next() as usize % SENTENCES.len()];
            ("typing", typing(&mut self.rng, text))
        } else if pick < 0.75 {
            ("touchpad", touchpad(&mut self.rng))
        } else {
            ("mouse", mouse(&mut self.rng))
        };
        self.phase = name;
        let scale = |ms: u64| Duration::from_secs_f32(ms as f32 / 1000.0 / self.speed);
        let last = events.last().map_or(0, |(at, _)| *at);
        self.queue = events.into_iter().map(|(at, ev)| (now + scale(at), ev)).collect();
        let pause = self.rng.ms(400, 1500);
        self.next_phase = now + scale(last + pause);
    }

    /// Events that are due.
    fn due(&mut self, now: Instant) -> Vec<Sim> {
        let mut out = Vec::new();
        while self.queue.front().is_some_and(|(at, _)| *at <= now) {
            out.extend(self.queue.pop_front().map(|(_, ev)| ev));
        }
        out
    }
}

// --- sound --------------------------------------------------------------------------

/// The same sound code the real events go through.
pub struct Audio {
    pub keyboard: Option<Keyboard>,
    pub mouse: Option<Mouse>,
    pub tap: Arc<Vec<u8>>,
    pub scroll: Option<Arc<Vec<u8>>>,
    pub player: Player,
}

impl Audio {
    fn play(&mut self, ev: Sim) {
        match ev {
            Sim::Key { code, down } => {
                if let Some(k) = self.keyboard.as_mut() {
                    k.handle(KeyEvent { code, down }, &self.player);
                }
            }
            Sim::Button { code, down } => {
                if let Some(m) = self.mouse.as_mut() {
                    m.handle_button(code, down, &self.player);
                }
            }
            Sim::Wheel { .. } | Sim::PadScroll => {
                if let Some(p) = &self.scroll {
                    self.player.play(p);
                }
            }
            Sim::Tap | Sim::PadClick => self.player.play(&self.tap),
        }
    }
}

// --- screen -------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Heat {
    Idle,
    /// Released a moment ago.
    Warm,
    /// Held, or released just now.
    Hot,
}

const HOT_FOR: Duration = Duration::from_millis(120);
const WARM_FOR: Duration = Duration::from_millis(450);

fn heat(down: bool, since: Option<Instant>, now: Instant) -> Heat {
    if down {
        return Heat::Hot;
    }
    match since.map(|t| now.saturating_duration_since(t)) {
        Some(d) if d < HOT_FOR => Heat::Hot,
        Some(d) if d < WARM_FOR => Heat::Warm,
        _ => Heat::Idle,
    }
}

struct Screen {
    rng: Rng,
    keys: HashMap<u16, (bool, Instant)>,
    shift: bool,
    typed: String,
    buttons: HashMap<u16, (bool, Instant)>,
    wheel: Option<(Instant, bool)>,
    tap: Option<(Instant, usize, usize)>,
    pad_click: Option<Instant>,
    pad_scroll: Option<(Instant, u32)>,
}

impl Screen {
    fn new(seed: u32) -> Screen {
        Screen {
            rng: Rng::new(seed),
            keys: HashMap::new(),
            shift: false,
            typed: String::new(),
            buttons: HashMap::new(),
            wheel: None,
            tap: None,
            pad_click: None,
            pad_scroll: None,
        }
    }

    fn apply(&mut self, ev: Sim, now: Instant) {
        match ev {
            Sim::Key { code, down } => {
                self.keys.insert(code, (down, now));
                if code == KEY_LEFTSHIFT || code == KEY_RIGHTSHIFT {
                    self.shift = down;
                } else if down {
                    match code {
                        KEY_BACKSPACE => {
                            self.typed.pop();
                        }
                        KEY_ENTER => self.typed.clear(),
                        _ => self.typed.extend(char_for(code, self.shift)),
                    }
                }
            }
            Sim::Button { code, down } => {
                self.buttons.insert(code, (down, now));
            }
            Sim::Wheel { up } => self.wheel = Some((now, up)),
            Sim::Tap => self.tap = Some((now, self.rng.next() as usize % PAD_W, self.rng.next() as usize % PAD_H)),
            Sim::PadClick => self.pad_click = Some(now),
            Sim::PadScroll => {
                let n = self.pad_scroll.map_or(0, |(_, n)| n) + 1;
                self.pad_scroll = Some((now, n));
            }
        }
    }
}

const PAD_W: usize = 22;
const PAD_H: usize = 4;
const KEYBOARD_W: usize = 45;
const MOUSE_W: usize = 11;

fn paint(text: &str, heat: Heat, color: bool) -> String {
    let style = match (heat, color) {
        (Heat::Idle, true) => "\x1b[48;5;236m\x1b[38;5;250m",
        (Heat::Warm, true) => "\x1b[48;5;71m\x1b[38;5;16m",
        (Heat::Hot, true) => "\x1b[1m\x1b[48;5;208m\x1b[38;5;16m",
        (Heat::Hot, false) => "\x1b[7m",
        _ => return text.to_string(),
    };
    format!("{style}{text}\x1b[0m")
}

fn keyboard_lines(s: &Screen, now: Instant, color: bool) -> Vec<String> {
    KEYBOARD_ROWS
        .iter()
        .map(|row| {
            row.iter()
                .map(|&(code, label, width)| {
                    let (down, since) = s.keys.get(&code).map_or((false, None), |(d, t)| (*d, Some(*t)));
                    let inner = width - 1;
                    format!("{} ", paint(&format!("{label:^inner$}"), heat(down, since, now), color))
                })
                .collect()
        })
        .collect()
}

fn mouse_lines(s: &Screen, now: Instant, color: bool) -> Vec<String> {
    let button = |code: u16, label: &str| {
        let (down, since) = s.buttons.get(&code).map_or((false, None), |(d, t)| (*d, Some(*t)));
        paint(&format!(" {label} "), heat(false || down, since, now), color)
    };
    let (wheel_heat, wheel_glyph) = match s.wheel {
        Some((t, up)) if heat(false, Some(t), now) != Heat::Idle => (heat(false, Some(t), now), if up { "▲" } else { "▼" }),
        _ => (Heat::Idle, "│"),
    };
    vec![
        " ╭───┬───╮ ".to_string(),
        format!(" │{}│{}│ ", button(BTN_LEFT, "L"), button(BTN_RIGHT, "R")),
        " ├───┴───┤ ".to_string(),
        format!(" │   {}   │ ", paint(wheel_glyph, wheel_heat, color)),
        " │       │ ".to_string(),
        " ╰───────╯ ".to_string(),
    ]
}

fn touchpad_lines(s: &Screen, now: Instant, color: bool) -> Vec<String> {
    let tap_heat = s.tap.map(|(t, _, _)| heat(false, Some(t), now)).unwrap_or(Heat::Idle);
    let scrolling = s.pad_scroll.filter(|(t, _)| now.saturating_duration_since(*t) < Duration::from_millis(160));
    let mut lines = vec![format!("╭{}╮", "─".repeat(PAD_W))];
    for y in 0..PAD_H {
        let mut cells: Vec<String> = vec![" ".to_string(); PAD_W];
        if let (Some((_, x, ty)), true) = (s.tap, tap_heat != Heat::Idle) {
            if ty == y {
                cells[x] = paint(if tap_heat == Heat::Hot { "●" } else { "•" }, tap_heat, color);
            }
        }
        if let Some((_, n)) = scrolling {
            // Two fingers sliding down the pad.
            let fy = (n as usize / 2) % PAD_H;
            if fy == y {
                cells[PAD_W / 2 - 2] = paint("●", Heat::Hot, color);
                cells[PAD_W / 2 + 1] = paint("●", Heat::Hot, color);
            }
        }
        lines.push(format!("│{}│", cells.concat()));
    }
    lines.push(format!("├{}┤", "─".repeat(PAD_W)));
    let clicked = heat(false, s.pad_click, now);
    lines.push(format!("│{}│", paint(&format!("{:^PAD_W$}", "click"), clicked, color)));
    lines.push(format!("╰{}╯", "─".repeat(PAD_W)));
    lines
}

/// The whole screen as lines (without line ends), for a terminal `width` columns wide.
fn render(s: &Screen, now: Instant, width: usize, color: bool, phase: &str, speed: f32) -> Vec<String> {
    let dim = |t: &str| if color { format!("\x1b[2m{t}\x1b[0m") } else { t.to_string() };
    let bold = |t: &str| if color { format!("\x1b[1m{t}\x1b[0m") } else { t.to_string() };
    if width < KEYBOARD_W + 4 {
        return vec![bold("heptix demo"), "The terminal is too narrow; make it at least 50 columns wide.".into()];
    }
    let mut lines = vec![
        format!("{}  {}", bold("heptix demo"), dim("simulated input")),
        dim("Nothing is read from your devices and nothing is recorded in your stats."),
        String::new(),
    ];
    lines.extend(keyboard_lines(s, now, color));
    let shown: String = {
        let tail: Vec<char> = s.typed.chars().rev().take(KEYBOARD_W - 4).collect();
        tail.into_iter().rev().collect()
    };
    lines.push(String::new());
    lines.push(format!("{} {shown}{}", dim(">"), dim("▏")));
    lines.push(String::new());
    lines.push(format!("{}{}{}", dim(&format!("{:<11}", " Mouse")), " ".repeat(6), dim("Touchpad")));
    let (mouse, pad) = (mouse_lines(s, now, color), touchpad_lines(s, now, color));
    for i in 0..mouse.len().max(pad.len()) {
        let m = mouse.get(i).cloned().unwrap_or_else(|| " ".repeat(MOUSE_W));
        let p = pad.get(i).map_or("", String::as_str);
        lines.push(format!("{m}{}{p}", " ".repeat(6)));
    }
    lines.push(String::new());
    lines.push(dim(&format!("q quit   + / - speed ({speed:.1}x)   now: {phase}")));
    lines.into_iter().map(|l| format!("  {l}")).collect()
}

// --- the loop -----------------------------------------------------------------------

/// Plays the demo until `q` (or Ctrl-C, or a termination signal).
pub fn run(settings: &Settings, mut audio: Audio) -> io::Result<()> {
    let raw = RawMode::enter()?;
    // Auto volume, as in the real service: quieter when the system volume is high.
    let volume_watcher = settings.is_auto_volume().then(|| {
        let (player, reference) = (audio.player.clone(), settings.auto_reference);
        sysvol::watch(move |amp| player.set_master(sysvol::compensation(amp, reference)))
    });
    let color = std::env::var_os("NO_COLOR").is_none();
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
    let mut script = Script::new(seed, Instant::now());
    let mut screen = Screen::new(seed ^ 0x9e37_79b9);
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    loop {
        let now = Instant::now();
        for ev in script.due(now) {
            if let Some(w) = &volume_watcher {
                w.touch();
            }
            audio.play(ev);
            screen.apply(ev, now);
        }
        script.refill(now);
        if last_draw.elapsed() >= Duration::from_millis(30) {
            last_draw = now;
            let frame = render(&screen, now, term_width(), color, script.phase, script.speed);
            print!("\x1b[H{}\x1b[K\x1b[J", frame.join("\x1b[K\r\n"));
            io::stdout().flush()?;
        }
        if crate::STOP.load(Ordering::Relaxed) {
            break;
        }
        match wait_for_key(8) {
            Some(b'q') | Some(3) => break,
            Some(b'+') | Some(b'=') => script.speed = (script.speed + 0.25).min(3.0),
            Some(b'-') | Some(b'_') => script.speed = (script.speed - 0.25).max(0.5),
            _ => {}
        }
    }
    drop(raw);
    Ok(())
}

/// The next byte typed, waiting at most `ms` milliseconds.
fn wait_for_key(ms: i32) -> Option<u8> {
    let mut fd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    if unsafe { libc::poll(&mut fd, 1, ms) } <= 0 {
        return None;
    }
    let mut b = [0u8; 1];
    (io::stdin().read(&mut b).ok()? == 1).then_some(b[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible_width(s: &str) -> usize {
        let (mut n, mut in_escape) = (0, false);
        for c in s.chars() {
            match (in_escape, c) {
                (true, 'm') => in_escape = false,
                (true, _) => {}
                (false, '\x1b') => in_escape = true,
                (false, _) => n += 1,
            }
        }
        n
    }

    #[test]
    fn every_character_of_the_sentences_can_be_typed() {
        for s in SENTENCES {
            for c in s.chars() {
                assert!(key_for(c).is_some(), "no key for {c:?} in {s:?}");
            }
        }
    }

    #[test]
    fn keys_and_characters_map_back_and_forth() {
        assert_eq!(key_for('a'), Some((30, false)));
        assert_eq!(key_for('A'), Some((30, true)));
        assert_eq!(key_for('?'), Some((53, true)));
        assert_eq!(char_for(30, false), Some('a'));
        assert_eq!(char_for(30, true), Some('A'));
        assert_eq!(char_for(57, false), Some(' '));
        assert_eq!(char_for(KEY_ENTER, false), None);
    }

    #[test]
    fn typing_releases_every_key_it_presses_and_is_in_time_order() {
        for seed in 1..40 {
            let events = typing(&mut Rng::new(seed), "Hello, World? (yes)");
            assert!(events.windows(2).all(|w| w[0].0 <= w[1].0), "seed {seed}: out of order");
            let mut held: HashMap<u16, i32> = HashMap::new();
            for (_, ev) in &events {
                if let Sim::Key { code, down } = ev {
                    *held.entry(*code).or_default() += if *down { 1 } else { -1 };
                    assert!(held[code] >= 0, "seed {seed}: key {code} released before it was pressed");
                } else {
                    panic!("typing made {ev:?}");
                }
            }
            assert!(held.values().all(|n| *n == 0), "seed {seed}: a key stays down");
            assert!(events.iter().any(|(_, e)| matches!(e, Sim::Key { code: KEY_LEFTSHIFT, down: true })), "capitals need Shift");
        }
    }

    #[test]
    fn typing_the_same_seed_twice_gives_the_same_events() {
        assert_eq!(typing(&mut Rng::new(7), "hello"), typing(&mut Rng::new(7), "hello"));
        assert_ne!(typing(&mut Rng::new(7), "hello"), typing(&mut Rng::new(8), "hello"));
    }

    #[test]
    fn a_typo_is_followed_by_a_backspace_and_the_screen_ends_with_the_right_text() {
        let mut seen_backspace = false;
        for seed in 1..400 {
            let events = typing(&mut Rng::new(seed), "the quick brown fox jumps over the lazy dog");
            let mut screen = Screen::new(1);
            let now = Instant::now();
            for (_, ev) in &events {
                seen_backspace |= matches!(ev, Sim::Key { code: KEY_BACKSPACE, down: true });
                screen.apply(*ev, now);
            }
            // An Enter at the end sends the line; otherwise the text must be exactly what was meant.
            let sent = events.iter().any(|(_, e)| matches!(e, Sim::Key { code: KEY_ENTER, down: true }));
            if !sent {
                assert_eq!(screen.typed, "the quick brown fox jumps over the lazy dog", "seed {seed}");
            }
        }
        assert!(seen_backspace, "no typo in 400 tries");
    }

    #[test]
    fn capitals_and_punctuation_come_out_as_typed() {
        let text = "Meeting moved to 3:30, Bring (the) notes? \"Yes!\"";
        let mut clean = 0;
        for seed in 1..200 {
            let events = typing(&mut Rng::new(seed), text);
            let mut screen = Screen::new(1);
            let now = Instant::now();
            for (_, ev) in &events {
                screen.apply(*ev, now);
            }
            let touched_up = events.iter().any(|(_, e)| matches!(e, Sim::Key { code: KEY_BACKSPACE | KEY_ENTER, down: true }));
            if !touched_up {
                assert_eq!(screen.typed, text, "seed {seed}");
                clean += 1;
            }
        }
        assert!(clean > 20, "too few clean runs to mean anything");
    }

    #[test]
    fn shift_is_let_go_before_the_next_key_goes_down() {
        for seed in 1..100 {
            let events = typing(&mut Rng::new(seed), "Hello World");
            let mut shift_down = false;
            for (_, ev) in &events {
                match ev {
                    Sim::Key { code: KEY_LEFTSHIFT, down } => shift_down = *down,
                    Sim::Key { code, down: true } if shift_down => {
                        assert!(char_for(*code, true).is_some_and(|c| c.is_ascii_uppercase()), "seed {seed}: shift still down for {code}");
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn mouse_and_touchpad_stretches_only_use_their_own_devices() {
        for seed in 1..60 {
            for (_, ev) in mouse(&mut Rng::new(seed)) {
                assert!(matches!(ev, Sim::Button { .. } | Sim::Wheel { .. }), "{ev:?}");
            }
            for (_, ev) in touchpad(&mut Rng::new(seed)) {
                assert!(matches!(ev, Sim::Tap | Sim::PadClick | Sim::PadScroll), "{ev:?}");
            }
            let events = mouse(&mut Rng::new(seed));
            for code in [BTN_LEFT, BTN_RIGHT] {
                let downs = events.iter().filter(|(_, e)| *e == Sim::Button { code, down: true }).count();
                let ups = events.iter().filter(|(_, e)| *e == Sim::Button { code, down: false }).count();
                assert_eq!(downs, ups, "seed {seed}: button {code} left down");
            }
        }
    }

    #[test]
    fn the_script_starts_stretches_and_pauses_between_them() {
        let start = Instant::now();
        let mut script = Script::new(3, start);
        script.refill(start);
        assert!(!script.queue.is_empty());
        let first_phase = script.phase;
        let queued = script.queue.len();
        script.refill(start); // not over yet: nothing new is started
        assert_eq!(script.queue.len(), queued);
        let far = start + Duration::from_secs(120);
        assert_eq!(script.due(far).len(), queued);
        script.refill(far);
        assert!(!script.queue.is_empty(), "the next stretch starts after the pause (was {first_phase})");
    }

    #[test]
    fn a_faster_speed_squeezes_the_stretch_into_less_time() {
        let start = Instant::now();
        let end = |speed: f32| {
            let mut script = Script::new(5, start);
            script.speed = speed;
            script.refill(start);
            script.queue.back().map(|(at, _)| at.duration_since(start)).unwrap()
        };
        assert!(end(2.0) < end(1.0));
    }

    #[test]
    fn the_keyboard_lights_up_the_keys_in_use_and_they_cool_down() {
        let now = Instant::now();
        let mut s = Screen::new(1);
        let idle = keyboard_lines(&s, now, true);
        assert!(idle.iter().all(|l| visible_width(l) == KEYBOARD_W), "rows must line up");
        assert!(!idle.concat().contains("48;5;208"), "nothing is pressed yet");
        s.apply(Sim::Key { code: 16, down: true }, now); // Q
        let hot = keyboard_lines(&s, now, true);
        assert!(hot[1].contains("\x1b[1m\x1b[48;5;208m\x1b[38;5;16mQ \x1b[0m"), "{:?}", hot[1]);
        assert!(!hot[2].contains("48;5;208"), "other rows stay dark");
        s.apply(Sim::Key { code: 16, down: false }, now);
        assert!(keyboard_lines(&s, now + Duration::from_millis(60), true)[1].contains("48;5;208"), "still bright right after release");
        assert!(keyboard_lines(&s, now + Duration::from_millis(250), true)[1].contains("48;5;71"), "then it cools down");
        assert!(!keyboard_lines(&s, now + Duration::from_secs(2), true)[1].contains("48;5;71"), "and goes dark");
    }

    #[test]
    fn mouse_buttons_wheel_and_pad_light_up() {
        let now = Instant::now();
        let mut s = Screen::new(1);
        s.apply(Sim::Button { code: BTN_LEFT, down: true }, now);
        s.apply(Sim::Wheel { up: true }, now);
        s.apply(Sim::Tap, now);
        s.apply(Sim::PadClick, now);
        let mouse = mouse_lines(&s, now, true).join("\n");
        assert!(mouse.contains("208m\x1b[38;5;16m L "), "left button is lit: {mouse:?}");
        assert!(mouse.contains('▲'));
        let pad = touchpad_lines(&s, now, true).join("\n");
        assert!(pad.contains('●'), "the tap shows");
        assert!(pad.contains("208m\x1b[38;5;16m") && pad.contains("click"));
        s.apply(Sim::Button { code: BTN_LEFT, down: false }, now);
        s.apply(Sim::Wheel { up: false }, now);
        assert!(mouse_lines(&s, now + Duration::from_secs(3), true).join("\n").contains('│'), "the wheel rests again");
    }

    #[test]
    fn the_devices_have_a_fixed_size() {
        let now = Instant::now();
        let mut s = Screen::new(1);
        for ev in [Sim::Tap, Sim::PadScroll, Sim::PadClick, Sim::Wheel { up: false }] {
            s.apply(ev, now);
        }
        for color in [true, false] {
            assert!(mouse_lines(&s, now, color).iter().all(|l| visible_width(l) == MOUSE_W));
            assert!(touchpad_lines(&s, now, color).iter().all(|l| visible_width(l) == PAD_W + 2));
        }
    }

    #[test]
    fn without_color_the_hot_keys_are_reverse_video_and_no_palette_is_used() {
        let now = Instant::now();
        let mut s = Screen::new(1);
        s.apply(Sim::Key { code: 30, down: true }, now);
        let screen = render(&s, now, 100, false, "typing", 1.0).join("\n");
        assert!(screen.contains("\x1b[7m"));
        assert!(!screen.contains("48;5;") && !screen.contains("\x1b[2m"));
    }

    #[test]
    fn the_screen_says_what_it_is_and_shows_what_is_typed() {
        let now = Instant::now();
        let mut s = Screen::new(1);
        for ev in [
            Sim::Key { code: KEY_LEFTSHIFT, down: true },
            Sim::Key { code: 35, down: true },
            Sim::Key { code: 35, down: false },
            Sim::Key { code: KEY_LEFTSHIFT, down: false },
            Sim::Key { code: 23, down: true },
        ] {
            s.apply(ev, now);
        }
        let screen = render(&s, now, 100, false, "typing", 1.5).join("\n");
        assert!(screen.contains("nothing is recorded"));
        assert!(screen.contains("> Hi"), "{screen}");
        assert!(screen.contains("1.5x") && screen.contains("now: typing"));
        assert!(render(&s, now, 40, false, "typing", 1.0).join("\n").contains("too narrow"));
    }
}
