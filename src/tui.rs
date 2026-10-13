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

//! Minimal settings editor: raw-mode terminal, ANSI drawing, no TUI crate.

use crate::config::Settings;
use crate::heatmap;
use crate::keyboard::{self, Keyboard};
use crate::mouse::{self, Mouse};
use crate::player::Player;
use crate::sound::{self, SOUNDS};
use crate::stats::{self, Metric, Stats};
use crate::sysvol;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const ROWS: [&str; 16] = [
    "Enabled",
    "Tap sound",
    "Scroll sound",
    "Keyboard sound",
    "Mouse sound",
    "Volume mode",
    "Tap volume",
    "Scroll volume",
    "Keyboard volume",
    "Mouse volume",
    "Auto reference",
    "Max tap length",
    "Scroll density",
    "Record activity",
    "Per-key counts",
    "Distance unit",
];

const R_ENABLED: usize = 0;
const R_TAP_SOUND: usize = 1;
const R_SCROLL_SOUND: usize = 2;
const R_KEYBOARD_SOUND: usize = 3;
const R_MOUSE_SOUND: usize = 4;
const R_VOLUME_MODE: usize = 5;
const R_TAP_VOLUME: usize = 6;
const R_SCROLL_VOLUME: usize = 7;
const R_KEYBOARD_VOLUME: usize = 8;
const R_MOUSE_VOLUME: usize = 9;
const R_AUTO_REFERENCE: usize = 10;
const R_MAX_MS: usize = 11;
const R_DENSITY: usize = 12;
const R_STATS: usize = 13;
const R_STATS_KEYS: usize = 14;
const R_UNIT: usize = 15;

/// Rows that do nothing in the current volume mode.
fn locked(s: &Settings, row: usize) -> bool {
    match row {
        R_TAP_VOLUME | R_SCROLL_VOLUME | R_KEYBOARD_VOLUME | R_MOUSE_VOLUME => s.is_auto_volume(),
        R_AUTO_REFERENCE => !s.is_auto_volume(),
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Group {
    Sounds,
    Volume,
    Touchpad,
    Activity,
}

const GROUPS: [(Group, &str); 4] =
    [(Group::Sounds, "Sounds"), (Group::Volume, "Volume"), (Group::Touchpad, "Touchpad"), (Group::Activity, "Activity")];

/// A line the cursor can be on: a setting, or a group header (folds the group).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Item {
    Row(usize),
    Header(Group),
}

/// Settings shown under a group. Volume rows that do nothing in the current
/// mode are left out, not greyed out.
fn group_rows(g: Group, s: &Settings) -> Vec<usize> {
    match g {
        Group::Sounds => vec![R_TAP_SOUND, R_SCROLL_SOUND, R_KEYBOARD_SOUND, R_MOUSE_SOUND],
        Group::Volume if s.is_auto_volume() => vec![R_VOLUME_MODE, R_AUTO_REFERENCE],
        Group::Volume => vec![R_VOLUME_MODE, R_TAP_VOLUME, R_SCROLL_VOLUME, R_KEYBOARD_VOLUME, R_MOUSE_VOLUME],
        Group::Touchpad => vec![R_MAX_MS, R_DENSITY],
        Group::Activity => vec![R_STATS, R_STATS_KEYS, R_UNIT],
    }
}

/// Everything currently on screen, top to bottom.
fn visible_items(s: &Settings, folded: &[bool; 4]) -> Vec<Item> {
    let mut items = vec![Item::Row(R_ENABLED)];
    for (g, _) in GROUPS {
        items.push(Item::Header(g));
        if !folded[g as usize] {
            items.extend(group_rows(g, s).into_iter().map(Item::Row));
        }
    }
    items
}

const PRIVACY: &str = "Counters only: never key order, timing or text. The service has no network access.";

/// Two lines explaining the selected item.
fn help(item: Item) -> [&'static str; 2] {
    match item {
        Item::Header(Group::Sounds) => ["Which sound plays for each kind of input.", "Enter or ←/→ folds this group."],
        Item::Header(Group::Volume) => ["How loud the sounds are.", "Enter or ←/→ folds this group."],
        Item::Header(Group::Touchpad) => ["Touchpad tap and scroll detection.", "Enter or ←/→ folds this group."],
        Item::Header(Group::Activity) => ["Counting what you type, tap and scroll, for the Activity views.", PRIVACY],
        Item::Row(R_ENABLED) => {
            ["Master switch. Off: nothing is read, played or recorded (stays running, idle).", "Turn it back on and press s to apply."]
        }
        Item::Row(R_TAP_SOUND) => ["Sound for touchpad taps and physical touchpad clicks.", ""],
        Item::Row(R_SCROLL_SOUND) => ["Tick for two-finger scrolling and the mouse wheel; off silences both.", ""],
        Item::Row(R_KEYBOARD_SOUND) => ["Mechanical keyboard sound. Only key down/up picks a click; nothing is logged.", ""],
        Item::Row(R_MOUSE_SOUND) => ["Mouse buttons: a different click for button down and up.", "The wheel uses the scroll sound."],
        Item::Row(R_VOLUME_MODE) => ["auto: quieter as the system volume rises, so never too loud.", "custom: set each volume yourself."],
        Item::Row(R_AUTO_REFERENCE) => ["System volume (%) up to which sounds play at full level.", "Above it heptix gets quieter, so the output stays the same."],
        Item::Row(R_TAP_VOLUME) => ["Volume of tap and click sounds (custom mode).", ""],
        Item::Row(R_SCROLL_VOLUME) => ["Volume of scroll ticks (custom mode).", ""],
        Item::Row(R_KEYBOARD_VOLUME) => ["Volume of keyboard sounds (custom mode).", ""],
        Item::Row(R_MOUSE_VOLUME) => ["Volume of mouse button sounds (custom mode).", ""],
        Item::Row(R_MAX_MS) => ["Longest touch (ms) still counted as a tap.", ""],
        Item::Row(R_DENSITY) => ["Scroll ticks per pad height; higher is denser.", ""],
        Item::Row(R_STATS) => ["Count keys, taps, clicks and scrolling (see the Activity view).", PRIVACY],
        Item::Row(R_STATS_KEYS) => ["Also count presses per key, for the Keys view. Needs Record activity.", PRIVACY],
        Item::Row(R_UNIT) => ["Unit for scroll distance: kilometres or miles.", ""],
        Item::Row(_) => ["", ""],
    }
}

struct RawMode(libc::termios);

impl RawMode {
    fn enter() -> io::Result<Self> {
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return Err(io::Error::last_os_error());
            }
            let saved = t;
            libc::cfmakeraw(&mut t);
            t.c_cc[libc::VMIN] = 0;
            t.c_cc[libc::VTIME] = 1; // reads time out after 100 ms
            libc::tcsetattr(0, libc::TCSANOW, &t);
            print!("\x1b[?1049h\x1b[?25l");
            let _ = io::stdout().flush();
            Ok(RawMode(saved))
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) };
        print!("\x1b[?25h\x1b[?1049l");
        let _ = io::stdout().flush();
    }
}

#[derive(PartialEq)]
enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    BackTab,
    Char(char),
    Other,
}

fn read_key() -> Option<Key> {
    let mut b = [0u8; 1];
    if io::stdin().read(&mut b).ok()? == 0 {
        return None;
    }
    Some(match b[0] {
        0x1b => {
            let mut seq = [0u8; 2];
            let n = io::stdin().read(&mut seq).unwrap_or(0);
            match (n, seq) {
                (2, [b'[', b'A']) => Key::Up,
                (2, [b'[', b'B']) => Key::Down,
                (2, [b'[', b'C']) => Key::Right,
                (2, [b'[', b'D']) => Key::Left,
                (2, [b'[', b'Z']) => Key::BackTab,
                _ => Key::Other,
            }
        }
        b'\r' | b'\n' => Key::Enter,
        3 => Key::Char('q'), // Ctrl-C
        c => Key::Char(c as char),
    })
}

fn options(row: usize, current: &str) -> Vec<String> {
    let mut v: Vec<String> = if row == R_KEYBOARD_SOUND {
        let mut v = vec!["off".to_string()];
        v.extend(keyboard::SOUNDS.iter().map(|s| s.to_string()));
        v
    } else if row == R_MOUSE_SOUND {
        let mut v = vec!["off".to_string()];
        v.extend(mouse::SOUNDS.iter().map(|s| s.to_string()));
        v
    } else {
        SOUNDS.iter().map(|s| s.to_string()).collect()
    };
    if row == R_SCROLL_SOUND {
        v.push("off".into());
    }
    if !v.iter().any(|o| o == current) {
        v.insert(0, current.to_string()); // keep a custom WAV path selectable
    }
    v
}

fn step<T: Copy + PartialOrd>(v: T, delta: i64, lo: T, hi: T, as_i: fn(T) -> i64, from_i: fn(i64) -> T) -> T {
    let n = (as_i(v) + delta).clamp(as_i(lo), as_i(hi));
    from_i(n)
}

/// Changes the selected row; returns false if the row is locked or already at its limit.
fn adjust(s: &mut Settings, row: usize, dir: i64) -> bool {
    if locked(s, row) {
        return false;
    }
    let before = s.clone();
    match row {
        R_TAP_SOUND | R_SCROLL_SOUND | R_KEYBOARD_SOUND | R_MOUSE_SOUND => {
            let cur = match row {
                R_TAP_SOUND => &mut s.sound,
                R_SCROLL_SOUND => &mut s.scroll_sound,
                R_KEYBOARD_SOUND => &mut s.keyboard_sound,
                _ => &mut s.mouse_sound,
            };
            let opts = options(row, cur);
            let i = opts.iter().position(|o| o == cur).unwrap_or(0) as i64;
            let n = (i + dir).rem_euclid(opts.len() as i64) as usize;
            *cur = opts[n].clone();
        }
        R_VOLUME_MODE => s.volume_mode = if s.is_auto_volume() { "custom".into() } else { "auto".into() },
        R_TAP_VOLUME => s.volume = step(s.volume, dir * 5, 0, 100, |v| v as i64, |i| i as u32),
        R_SCROLL_VOLUME => s.scroll_volume = step(s.scroll_volume, dir * 5, 0, 100, |v| v as i64, |i| i as u32),
        R_KEYBOARD_VOLUME => s.keyboard_volume = step(s.keyboard_volume, dir * 5, 0, 100, |v| v as i64, |i| i as u32),
        R_MOUSE_VOLUME => s.mouse_volume = step(s.mouse_volume, dir * 5, 0, 100, |v| v as i64, |i| i as u32),
        R_AUTO_REFERENCE => s.auto_reference = step(s.auto_reference, dir * 5, 10, 100, |v| v as i64, |i| i as u32),
        R_MAX_MS => s.max_ms = step(s.max_ms, dir * 25, 50, 1000, |v| v as i64, |i| i as u64),
        R_DENSITY => s.scroll_ticks = step(s.scroll_ticks, dir * 2, 2, 200, |v| v as i64, |i| i as i32),
        R_ENABLED => s.enabled = !s.enabled,
        R_STATS => s.stats = !s.stats,
        R_STATS_KEYS => s.stats_keys = !s.stats_keys,
        R_UNIT => s.distance_unit = if s.distance_unit == "km" { "mi".into() } else { "km".into() },
        _ => {}
    }
    *s != before
}

fn on_off(v: bool) -> String {
    if v { "on" } else { "off" }.to_string()
}

fn value_text(s: &Settings, row: usize) -> String {
    match row {
        R_ENABLED => if s.enabled { "on" } else { "off: nothing is played, read or recorded" }.to_string(),
        R_TAP_SOUND => s.sound.clone(),
        R_SCROLL_SOUND => s.scroll_sound.clone(),
        R_KEYBOARD_SOUND => s.keyboard_sound.clone(),
        R_MOUSE_SOUND => s.mouse_sound.clone(),
        R_VOLUME_MODE => if s.is_auto_volume() { "auto (follows system volume)" } else { "custom" }.to_string(),
        R_TAP_VOLUME if s.is_auto_volume() => "auto".into(),
        R_SCROLL_VOLUME if s.is_auto_volume() => "auto".into(),
        R_KEYBOARD_VOLUME if s.is_auto_volume() => "auto".into(),
        R_MOUSE_VOLUME if s.is_auto_volume() => "auto".into(),
        R_TAP_VOLUME => format!("{}%", s.volume),
        R_SCROLL_VOLUME => format!("{}%", s.scroll_volume),
        R_KEYBOARD_VOLUME => format!("{}%", s.keyboard_volume),
        R_MOUSE_VOLUME => format!("{}%", s.mouse_volume),
        R_AUTO_REFERENCE if s.is_auto_volume() => format!("{}% system volume", s.auto_reference),
        R_AUTO_REFERENCE => "n/a".into(),
        R_MAX_MS => format!("{} ms", s.max_ms),
        R_DENSITY => format!("{} per pad height", s.scroll_ticks),
        R_STATS => on_off(s.stats),
        R_STATS_KEYS => on_off(s.stats_keys),
        _ => if s.distance_unit == "mi" { "miles" } else { "kilometres" }.to_string(),
    }
}

struct Preview {
    player: Player,
    /// Latest system output amplitude (f32 bits), kept fresh by the watcher.
    amp: Arc<AtomicU32>,
    watcher: sysvol::Watcher,
}

impl Preview {
    fn new() -> Preview {
        let amp = Arc::new(AtomicU32::new(0));
        let sink = Arc::clone(&amp);
        let watcher = sysvol::watch(move |a| sink.store(a.to_bits(), Ordering::Relaxed));
        Preview { player: Player::start(), amp, watcher }
    }

    /// Plays previews at the level the service would use in the current mode.
    fn level(&self, s: &Settings) {
        self.watcher.touch();
        let gain = if s.is_auto_volume() {
            sysvol::compensation(f32::from_bits(self.amp.load(Ordering::Relaxed)), s.auto_reference)
        } else {
            1.0
        };
        self.player.set_master(gain);
    }

    fn tap(&self, s: &Settings) {
        self.level(s);
        if let Ok(p) = sound::load(&s.sound, s.effective_volumes().0) {
            self.player.play(&Arc::new(p));
        }
    }

    /// A short run of scroll ticks spaced like a moderate scroll at this density.
    fn scroll(&self, s: &Settings) {
        if s.scroll_sound == "off" {
            return;
        }
        self.level(s);
        if let Ok(p) = sound::load(&s.scroll_sound, s.effective_volumes().1) {
            let (player, pcm) = (self.player.clone(), Arc::new(p));
            let gap = Duration::from_millis((1500 / s.scroll_ticks.max(1) as u64).max(20));
            thread::spawn(move || {
                for _ in 0..8 {
                    player.play(&pcm);
                    thread::sleep(gap);
                }
            });
        }
    }

    /// A short burst of typing.
    fn typing(&self, s: &Settings) {
        self.level(s);
        if let Ok(mut k) = Keyboard::new(&s.keyboard_sound, s.effective_volumes().2) {
            let player = self.player.clone();
            thread::spawn(move || k.demo(&player));
        }
    }

    /// A few clicks: left, right, middle and a double click.
    fn clicking(&self, s: &Settings) {
        self.level(s);
        if let Ok(mut m) = Mouse::new(&s.mouse_sound, s.effective_volumes().3) {
            let player = self.player.clone();
            thread::spawn(move || m.demo(&player));
        }
    }

    fn row(&self, s: &Settings, row: usize) {
        match row {
            R_MOUSE_SOUND | R_MOUSE_VOLUME => self.clicking(s),
            R_SCROLL_SOUND | R_SCROLL_VOLUME | R_DENSITY => self.scroll(s),
            R_KEYBOARD_SOUND | R_KEYBOARD_VOLUME => self.typing(s),
            R_ENABLED | R_MAX_MS | R_STATS | R_STATS_KEYS | R_UNIT => {}
            _ => self.tap(s),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Settings,
    Activity,
    Hours,
    Keys,
}

const TABS: [(Tab, &str); 4] = [(Tab::Settings, "Settings"), (Tab::Activity, "Activity"), (Tab::Hours, "Hours"), (Tab::Keys, "Keys")];
const RANGES: [usize; 4] = [4, 12, 26, 53];

impl Tab {
    fn shift(self, by: usize) -> Tab {
        let i = TABS.iter().position(|(t, _)| *t == self).unwrap_or(0);
        TABS[(i + by) % TABS.len()].0
    }
}

/// What the Activity, Hours and Keys tabs show.
struct View {
    stats: Stats,
    loaded: Instant,
    metric: Metric,
    range: usize,
    cursor: i32,
    wipe_armed: bool,
}

impl View {
    fn load() -> View {
        View {
            stats: stats::load(&stats::default_path()).unwrap_or_default(),
            loaded: Instant::now(),
            metric: Metric::Keys,
            range: 2,
            cursor: stats::Local::now().day,
            wipe_armed: false,
        }
    }

    fn reload(&mut self) {
        self.stats = stats::load(&stats::default_path()).unwrap_or_default();
        self.loaded = Instant::now();
    }
}

fn term_rows() -> usize {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_row > 0 {
            ws.ws_row as usize
        } else {
            24
        }
    }
}

fn term_width() -> usize {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            ws.ws_col as usize
        } else {
            80
        }
    }
}

fn tab_bar(tab: Tab) -> String {
    let mut out = String::from("\x1b[1mheptix\x1b[0m  ");
    for (t, name) in TABS {
        if t == tab {
            out += &format!("\x1b[7m {name} \x1b[0m ");
        } else {
            out += &format!("\x1b[2m {name} \x1b[0m ");
        }
    }
    out += "\x1b[2m(Tab)\x1b[0m";
    out
}

/// Lines around the settings list: tab bar, blank, blank, 2 help lines, hints, status.
const SETTINGS_CHROME: usize = 7;

#[allow(clippy::too_many_arguments)]
fn draw_settings(s: &Settings, items: &[Item], sel: Item, top: &mut usize, folded: &[bool; 4], dirty: bool, path: &Path, msg: &str) {
    let avail = term_rows().saturating_sub(SETTINGS_CHROME).max(5);
    let pos = items.iter().position(|i| *i == sel).unwrap_or(0);
    if pos < *top {
        *top = pos;
    }
    if pos >= *top + avail {
        *top = pos + 1 - avail;
    }
    *top = (*top).min(items.len().saturating_sub(avail));

    let mut out = String::from("\x1b[2J\x1b[H");
    out += &tab_bar(Tab::Settings);
    out += "\r\n\r\n";
    for item in items.iter().skip(*top).take(avail) {
        let (line, indent_dim) = match *item {
            Item::Header(g) => {
                let name = GROUPS[g as usize].1;
                (format!(" {} {name}", if folded[g as usize] { "▸" } else { "▾" }), false)
            }
            Item::Row(R_ENABLED) => (format!(" {:<16} < {} >", ROWS[R_ENABLED], value_text(s, R_ENABLED)), false),
            Item::Row(r) => (format!("     {:<16} < {} >", ROWS[r], value_text(s, r)), !s.enabled),
        };
        let dim = if indent_dim { "\x1b[2m" } else { "" };
        let bold = if matches!(item, Item::Header(_)) { "\x1b[1m" } else { "" };
        if *item == sel {
            out += &format!("{dim}\x1b[7m{line}\x1b[0m\r\n");
        } else {
            out += &format!("{dim}{bold}{line}\x1b[0m\r\n");
        }
    }
    let more = (*top > 0, *top + avail < items.len());
    out += match more {
        (true, true) => " \x1b[2m↑↓ more\x1b[0m\r\n",
        (true, false) => " \x1b[2m↑ more\x1b[0m\r\n",
        (false, true) => " \x1b[2m↓ more\x1b[0m\r\n",
        _ => "\r\n",
    };
    if !s.enabled && sel != Item::Row(R_ENABLED) {
        out += " \x1b[33mheptix is OFF:\x1b[0m\x1b[2m nothing is played, read or recorded.\x1b[0m\r\n\r\n";
    } else {
        let [a, b] = help(sel);
        out += &format!(" \x1b[2m{a}\x1b[0m\r\n \x1b[2m{b}\x1b[0m\r\n");
    }
    out += " \x1b[2m↑/↓ select  ←/→ change  Enter fold/preview  Tab view  s save  q quit\x1b[0m\r\n";
    let status = if msg.is_empty() { path.display().to_string() } else { msg.to_string() };
    out += &format!(" {status}{}", if dirty { "  [unsaved]" } else { "" });
    print!("{out}");
    let _ = io::stdout().flush();
}

fn draw_stats(tab: Tab, s: &Settings, v: &mut View, dirty: bool, msg: &str) {
    let width = term_width();
    let today = stats::Local::now().day;
    let unit = s.distance_unit.as_str();
    let color = std::env::var_os("NO_COLOR").is_none();
    let weeks = match tab {
        Tab::Activity => RANGES[v.range].min(width.saturating_sub(8) / 2).max(1),
        _ => RANGES[v.range],
    };
    let from = today - 7 * weeks as i32 + 1;
    let cal_start = today - stats::weekday(today) as i32 - 7 * (weeks as i32 - 1);
    v.cursor = v.cursor.clamp(cal_start, today);

    let mut lines = vec![tab_bar(tab), String::new()];
    lines.push(if !s.stats {
        " \x1b[33mRecording is off.\x1b[0m Press e to turn it on, then s to save & apply.".into()
    } else if let Some(since) = v.stats.since {
        format!(" Recording on, since {}", heatmap::fmt_day(since))
    } else {
        " Recording on, nothing recorded yet".into()
    });
    let range_text = if tab == Tab::Activity && weeks < RANGES[v.range] {
        format!("{weeks} weeks (fits screen)")
    } else {
        format!("{weeks} weeks")
    };
    lines.push(if tab == Tab::Keys {
        format!(" Range < {range_text} >")
    } else {
        format!(" Metric < {} >   Range < {range_text} >", v.metric.name())
    });
    if tab != Tab::Activity {
        lines.push(String::new());
    }
    let pad = |l: String| format!(" {l}");
    match tab {
        Tab::Activity => {
            lines.extend(heatmap::calendar(&v.stats, today, weeks, v.metric, Some(v.cursor), color).into_iter().map(pad));
            lines.push(String::new());
            // Everything else on this view takes 19 lines; the table uses what is left.
            let data_rows = term_rows().saturating_sub(19).clamp(3, 5);
            let table = heatmap::stats_table(&v.stats, today, (data_rows >= 4).then_some(v.cursor), data_rows >= 5, v.metric, unit, true);
            lines.extend(table.into_iter().map(pad));
            lines.push(format!(" {}", heatmap::table_footer(&v.stats, today, cal_start, v.metric, unit)));
        }
        Tab::Hours => {
            lines.extend(heatmap::hour_grid(&v.stats, today, weeks, v.metric, color).into_iter().map(pad));
            lines.push(String::new());
            lines.push(match heatmap::peak_hour(&v.stats, today, weeks, v.metric, unit) {
                Some(p) => format!(" {p}"),
                None => " No activity in this range yet.".into(),
            });
        }
        _ => {
            lines.extend(heatmap::key_bars(&v.stats, from, today).into_iter().map(pad));
            lines.push(String::new());
            lines.extend(heatmap::keyboard(&v.stats, from, today, color).into_iter().map(pad));
        }
    }
    if tab != Tab::Activity {
        lines.push(String::new());
    }
    lines.push(match tab {
        Tab::Activity => " \x1b[2m←/→ week  ↑/↓ day  m metric  r range  u km/mi  e rec on/off  X wipe  q quit\x1b[0m".into(),
        Tab::Hours => " \x1b[2mm metric  r range  u km/mi  e rec on/off  X wipe  s save & apply  q quit\x1b[0m".into(),
        _ => " \x1b[2mr range  e rec on/off  X wipe  s save & apply  q quit   (US-layout key positions)\x1b[0m".into(),
    });
    lines.push(" \x1b[2mCounters only, never key order, timing or text. Service has no network access.\x1b[0m".into());
    lines.push(format!(" {}{}", msg, if dirty { "  [unsaved settings]" } else { "" }));
    print!("\x1b[2J\x1b[H{}", lines.join("\r\n"));
    let _ = io::stdout().flush();
}

fn apply_to_service() -> String {
    match Command::new("systemctl").args(["--user", "try-restart", "heptix.service"]).status() {
        Ok(st) if st.success() => "saved; background service restarted if it was running".into(),
        _ => "saved (could not restart a background service; restart it yourself if you use one)".into(),
    }
}

pub fn run(path: &Path, mut s: Settings, notes: &[String]) -> io::Result<()> {
    let _raw = RawMode::enter()?;
    let preview = Preview::new();
    let mut dirty = false;
    let mut sel = Item::Row(R_ENABLED);
    let (mut top, mut folded) = (0usize, [false; 4]);
    let mut msg = notes.join("; ");
    let mut quit_armed = false;
    let mut redraw = true;
    let mut tab = Tab::Settings;
    let mut view = View::load();
    loop {
        if redraw {
            match tab {
                Tab::Settings => {
                    let items = visible_items(&s, &folded);
                    if !items.contains(&sel) {
                        sel = Item::Row(R_ENABLED);
                    }
                    draw_settings(&s, &items, sel, &mut top, &folded, dirty, path, &msg)
                }
                _ => draw_stats(tab, &s, &mut view, dirty, &msg),
            }
        }
        redraw = false;
        let Some(key) = read_key() else {
            // Pick up what the background service has recorded since.
            if tab != Tab::Settings && view.loaded.elapsed() >= Duration::from_secs(3) {
                view.reload();
                redraw = true;
            }
            continue;
        };
        redraw = true;
        if key != Key::Char('q') {
            quit_armed = false;
        }
        if key != Key::Char('X') {
            view.wipe_armed = false;
        }
        if matches!(key, Key::Char('\t') | Key::BackTab) {
            tab = tab.shift(if key == Key::BackTab { TABS.len() - 1 } else { 1 });
            view.reload();
            msg.clear();
            continue;
        }
        if tab != Tab::Settings {
            let day = view.cursor;
            let mut handled = true;
            match key {
                Key::Left | Key::Char('h') if tab == Tab::Activity => view.cursor = day - 7,
                Key::Right | Key::Char('l') if tab == Tab::Activity => view.cursor = day + 7,
                Key::Up | Key::Char('k') if tab == Tab::Activity => view.cursor = day - 1,
                Key::Down | Key::Char('j') if tab == Tab::Activity => view.cursor = day + 1,
                Key::Char('m') => view.metric = view.metric.next(),
                Key::Char('r') => view.range = (view.range + 1) % RANGES.len(),
                Key::Char('u') => {
                    adjust(&mut s, R_UNIT, 1);
                    dirty = true;
                }
                Key::Char('e') => {
                    s.stats = !s.stats;
                    dirty = true;
                    msg = if s.stats { "recording on: s to save & apply".into() } else { "recording off: s to save & apply".into() };
                }
                Key::Char('X') => {
                    if view.wipe_armed {
                        msg = match stats::wipe(&stats::default_path()) {
                            Ok(_) => "all activity stats deleted".into(),
                            Err(e) => format!("could not delete stats: {e}"),
                        };
                        view.wipe_armed = false;
                        view.reload();
                    } else {
                        view.wipe_armed = true;
                        msg = "press X again to delete ALL stored activity stats".into();
                    }
                }
                _ => handled = false,
            }
            if handled {
                continue;
            }
        }
        match key {
            Key::Char('s') => {
                msg = match s.save(path) {
                    Ok(()) => {
                        dirty = false;
                        apply_to_service()
                    }
                    Err(e) => format!("save failed: {e}"),
                };
            }
            Key::Char('q') => {
                if !dirty || quit_armed {
                    return Ok(());
                }
                quit_armed = true;
                msg = "unsaved changes: press q again to discard, s to save".into();
            }
            _ if tab != Tab::Settings => {}
            Key::Up | Key::Char('k') | Key::Down | Key::Char('j') => {
                let items = visible_items(&s, &folded);
                let pos = items.iter().position(|i| *i == sel).unwrap_or(0);
                let next = if matches!(key, Key::Up | Key::Char('k')) { (pos + items.len() - 1) % items.len() } else { (pos + 1) % items.len() };
                sel = items[next];
            }
            Key::Left | Key::Char('h') | Key::Right | Key::Char('l') => {
                let dir = if matches!(key, Key::Left | Key::Char('h')) { -1 } else { 1 };
                match sel {
                    // ← folds a group, → opens it.
                    Item::Header(g) => folded[g as usize] = dir < 0,
                    Item::Row(row) => {
                        if adjust(&mut s, row, dir) {
                            dirty = true;
                            msg.clear();
                            preview.row(&s, row);
                        }
                    }
                }
            }
            Key::Char(' ') | Key::Enter => match sel {
                Item::Header(g) => folded[g as usize] = !folded[g as usize],
                Item::Row(row) => preview.row(&s, row),
            },
            Key::Char('p') => {
                preview.tap(&s);
                thread::sleep(Duration::from_millis(350));
                preview.scroll(&s);
                thread::sleep(Duration::from_millis(350));
                preview.typing(&s);
                thread::sleep(Duration::from_millis(1500));
                preview.clicking(&s);
            }
            _ => {}
        }
    }
}
