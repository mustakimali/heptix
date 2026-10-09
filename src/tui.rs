//! Minimal settings editor: raw-mode terminal, ANSI drawing, no TUI crate.

use crate::config::Settings;
use crate::player::Player;
use crate::sound::{self, PRESETS};
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const ROWS: [&str; 6] = ["Tap sound", "Scroll sound", "Tap volume", "Scroll volume", "Max tap length", "Scroll density"];

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
                _ => Key::Other,
            }
        }
        b'\r' | b'\n' => Key::Enter,
        3 => Key::Char('q'), // Ctrl-C
        c => Key::Char(c as char),
    })
}

fn options(row: usize, current: &str) -> Vec<String> {
    let mut v: Vec<String> = PRESETS.iter().map(|s| s.to_string()).collect();
    if row == 1 {
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

fn adjust(s: &mut Settings, row: usize, dir: i64) {
    match row {
        0 | 1 => {
            let cur = if row == 0 { &mut s.sound } else { &mut s.scroll_sound };
            let opts = options(row, cur);
            let i = opts.iter().position(|o| o == cur).unwrap_or(0) as i64;
            let n = (i + dir).rem_euclid(opts.len() as i64) as usize;
            *cur = opts[n].clone();
        }
        2 => s.volume = step(s.volume, dir * 5, 0, 100, |v| v as i64, |i| i as u32),
        3 => s.scroll_volume = step(s.scroll_volume, dir * 5, 0, 100, |v| v as i64, |i| i as u32),
        4 => s.max_ms = step(s.max_ms, dir * 25, 50, 1000, |v| v as i64, |i| i as u64),
        _ => s.scroll_ticks = step(s.scroll_ticks, dir * 2, 2, 200, |v| v as i64, |i| i as i32),
    }
}

fn value_text(s: &Settings, row: usize) -> String {
    match row {
        0 => s.sound.clone(),
        1 => s.scroll_sound.clone(),
        2 => format!("{}%", s.volume),
        3 => format!("{}%", s.scroll_volume),
        4 => format!("{} ms", s.max_ms),
        _ => format!("{} per pad height", s.scroll_ticks),
    }
}

struct Preview {
    player: Player,
}

impl Preview {
    fn tap(&self, s: &Settings) {
        if let Ok(p) = sound::load(&s.sound, s.volume) {
            self.player.play(&Arc::new(p));
        }
    }

    /// A short run of scroll ticks spaced like a moderate scroll at this density.
    fn scroll(&self, s: &Settings) {
        if s.scroll_sound == "off" {
            return;
        }
        if let Ok(p) = sound::load(&s.scroll_sound, s.scroll_volume) {
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

    fn row(&self, s: &Settings, row: usize) {
        match row {
            1 | 3 | 5 => self.scroll(s),
            4 => {}
            _ => self.tap(s),
        }
    }
}

fn draw(s: &Settings, sel: usize, dirty: bool, path: &Path, msg: &str) {
    let mut out = String::from("\x1b[2J\x1b[H");
    out += "\x1b[1mheptix\x1b[0m - the missing haptic feedback for Linux!\r\n\x1b[2msettings\x1b[0m\r\n\r\n";
    for (i, name) in ROWS.iter().enumerate() {
        let line = format!(" {name:<16} < {} >", value_text(s, i));
        if i == sel {
            out += &format!("\x1b[7m{line}\x1b[0m\r\n");
        } else {
            out += &format!("{line}\r\n");
        }
    }
    out += "\r\n \x1b[2m↑/↓ select   ←/→ change (plays preview)   space preview\r\n";
    out += " p preview tap + scroll   s save & apply   q quit\x1b[0m\r\n\r\n";
    out += &format!(" {}{}\r\n {}\r\n", path.display(), if dirty { "  [unsaved]" } else { "" }, msg);
    print!("{out}");
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
    let preview = Preview { player: Player::start() };
    let (mut sel, mut dirty) = (0usize, false);
    let mut msg = notes.join("; ");
    let mut quit_armed = false;
    let mut redraw = true;
    loop {
        if redraw {
            draw(&s, sel, dirty, path, &msg);
        }
        redraw = false;
        let Some(key) = read_key() else { continue };
        redraw = true;
        if key != Key::Char('q') {
            quit_armed = false;
        }
        match key {
            Key::Up | Key::Char('k') => sel = (sel + ROWS.len() - 1) % ROWS.len(),
            Key::Down | Key::Char('j') => sel = (sel + 1) % ROWS.len(),
            Key::Left | Key::Char('h') | Key::Right | Key::Char('l') => {
                let dir = if matches!(key, Key::Left | Key::Char('h')) { -1 } else { 1 };
                adjust(&mut s, sel, dir);
                dirty = true;
                msg.clear();
                preview.row(&s, sel);
            }
            Key::Char(' ') | Key::Enter => preview.row(&s, sel),
            Key::Char('p') => {
                preview.tap(&s);
                thread::sleep(Duration::from_millis(350));
                preview.scroll(&s);
            }
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
            _ => {}
        }
    }
}
