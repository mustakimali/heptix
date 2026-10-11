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

const ROWS: [&str; 13] = [
    "Tap sound",
    "Scroll sound",
    "Keyboard sound",
    "Volume mode",
    "Tap volume",
    "Scroll volume",
    "Keyboard volume",
    "Auto reference",
    "Max tap length",
    "Scroll density",
    "Activity stats",
    "Per-key counts",
    "Distance unit",
];

const R_TAP_SOUND: usize = 0;
const R_SCROLL_SOUND: usize = 1;
const R_KEYBOARD_SOUND: usize = 2;
const R_VOLUME_MODE: usize = 3;
const R_TAP_VOLUME: usize = 4;
const R_SCROLL_VOLUME: usize = 5;
const R_KEYBOARD_VOLUME: usize = 6;
const R_AUTO_REFERENCE: usize = 7;
const R_MAX_MS: usize = 8;
const R_DENSITY: usize = 9;
const R_STATS: usize = 10;
const R_STATS_KEYS: usize = 11;
const R_UNIT: usize = 12;

/// Rows that do nothing in the current volume mode.
fn locked(s: &Settings, row: usize) -> bool {
    match row {
        R_TAP_VOLUME | R_SCROLL_VOLUME | R_KEYBOARD_VOLUME => s.is_auto_volume(),
        R_AUTO_REFERENCE => !s.is_auto_volume(),
        _ => false,
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
        R_TAP_SOUND | R_SCROLL_SOUND | R_KEYBOARD_SOUND => {
            let cur = match row {
                R_TAP_SOUND => &mut s.sound,
                R_SCROLL_SOUND => &mut s.scroll_sound,
                _ => &mut s.keyboard_sound,
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
        R_AUTO_REFERENCE => s.auto_reference = step(s.auto_reference, dir * 5, 10, 100, |v| v as i64, |i| i as u32),
        R_MAX_MS => s.max_ms = step(s.max_ms, dir * 25, 50, 1000, |v| v as i64, |i| i as u64),
        R_DENSITY => s.scroll_ticks = step(s.scroll_ticks, dir * 2, 2, 200, |v| v as i64, |i| i as i32),
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
        R_TAP_SOUND => s.sound.clone(),
        R_SCROLL_SOUND => s.scroll_sound.clone(),
        R_KEYBOARD_SOUND => s.keyboard_sound.clone(),
        R_VOLUME_MODE => if s.is_auto_volume() { "auto (follows system volume)" } else { "custom" }.to_string(),
        R_TAP_VOLUME if s.is_auto_volume() => "auto".into(),
        R_SCROLL_VOLUME if s.is_auto_volume() => "auto".into(),
        R_KEYBOARD_VOLUME if s.is_auto_volume() => "auto".into(),
        R_TAP_VOLUME => format!("{}%", s.volume),
        R_SCROLL_VOLUME => format!("{}%", s.scroll_volume),
        R_KEYBOARD_VOLUME => format!("{}%", s.keyboard_volume),
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

    fn row(&self, s: &Settings, row: usize) {
        match row {
            R_SCROLL_SOUND | R_SCROLL_VOLUME | R_DENSITY => self.scroll(s),
            R_KEYBOARD_SOUND | R_KEYBOARD_VOLUME => self.typing(s),
            R_MAX_MS | R_STATS | R_STATS_KEYS | R_UNIT => {}
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

fn draw_settings(s: &Settings, sel: usize, dirty: bool, path: &Path, msg: &str) {
    let mut out = String::from("\x1b[2J\x1b[H");
    out += &tab_bar(Tab::Settings);
    out += "\r\n\r\n";
    for (i, name) in ROWS.iter().enumerate() {
        let line = format!(" {name:<16} < {} >", value_text(s, i));
        let dim = if locked(s, i) { "\x1b[2m" } else { "" };
        if i == sel {
            out += &format!("{dim}\x1b[7m{line}\x1b[0m\r\n");
        } else {
            out += &format!("{dim}{line}\x1b[0m\r\n");
        }
    }
    out += " \x1b[2mAuto volume plays quieter as your system volume rises; custom sets the levels.\r\n";
    out += " Activity stats keep counters only (no key order, timing or text), and the service\r\n";
    out += " runs with its network access blocked, so nothing can be sent anywhere.\x1b[0m\r\n";
    out += "\r\n \x1b[2m↑/↓ select  ←/→ change  space/p preview  Tab view  s save & apply  q quit\x1b[0m\r\n\r\n";
    out += &format!(" {}{}\r\n {}\r\n", path.display(), if dirty { "  [unsaved]" } else { "" }, msg);
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
    lines.push(String::new());
    let pad = |l: String| format!(" {l}");
    match tab {
        Tab::Activity => {
            lines.extend(heatmap::calendar(&v.stats, today, weeks, v.metric, Some(v.cursor), color).into_iter().map(pad));
            lines.push(String::new());
            lines.push(format!(" {}", heatmap::day_detail(&v.stats, v.cursor, unit)));
            lines.push(format!(" {}", heatmap::scroll_distance_line(&v.stats, today, unit)));
            lines.push(format!(" {}", heatmap::headline(&v.stats, today, cal_start, v.metric, unit)));
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
    lines.push(String::new());
    lines.push(match tab {
        Tab::Activity => " \x1b[2m←/→ week  ↑/↓ day  m metric  r range  u km/mi  e rec on/off  X wipe  q quit\x1b[0m".into(),
        Tab::Hours => " \x1b[2mm metric  r range  u km/mi  e rec on/off  X wipe  s save & apply  q quit\x1b[0m".into(),
        _ => " \x1b[2mr range  e rec on/off  X wipe  s save & apply  q quit   (US-layout key positions)\x1b[0m".into(),
    });
    lines.push(" \x1b[2mCounters only: no key order, timing or text. Kept in a private file on this machine;\x1b[0m".into());
    lines.push(" \x1b[2mthe service runs with its network access blocked, so nothing can leave it.\x1b[0m".into());
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
    let (mut sel, mut dirty) = (0usize, false);
    let mut msg = notes.join("; ");
    let mut quit_armed = false;
    let mut redraw = true;
    let mut tab = Tab::Settings;
    let mut view = View::load();
    loop {
        if redraw {
            match tab {
                Tab::Settings => draw_settings(&s, sel, dirty, path, &msg),
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
            Key::Up | Key::Char('k') => sel = (sel + ROWS.len() - 1) % ROWS.len(),
            Key::Down | Key::Char('j') => sel = (sel + 1) % ROWS.len(),
            Key::Left | Key::Char('h') | Key::Right | Key::Char('l') => {
                let dir = if matches!(key, Key::Left | Key::Char('h')) { -1 } else { 1 };
                if adjust(&mut s, sel, dir) {
                    dirty = true;
                    msg.clear();
                    preview.row(&s, sel);
                } else if locked(&s, sel) {
                    msg = if sel == R_AUTO_REFERENCE {
                        "auto reference only applies in auto mode".into()
                    } else {
                        "volumes are automatic: set Volume mode to custom to change them".into()
                    };
                }
            }
            Key::Char(' ') | Key::Enter => preview.row(&s, sel),
            Key::Char('p') => {
                preview.tap(&s);
                thread::sleep(Duration::from_millis(350));
                preview.scroll(&s);
                thread::sleep(Duration::from_millis(350));
                preview.typing(&s);
            }
            _ => {}
        }
    }
}
