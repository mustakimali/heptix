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

mod config;
mod heatmap;
mod keyboard;
mod player;
mod sound;
mod stats;
mod sysvol;
mod tap;
mod tui;

use config::Settings;
use evdev::Device;
use std::path::PathBuf;
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

const USAGE: &str = "heptix - the missing haptic feedback for Linux!

USAGE:
  heptix            Open the settings editor (TUI)
  heptix --run      Run in the background mode using the saved settings
  heptix [OPTIONS]  Any option also runs it; options override the saved settings

Settings live in $XDG_CONFIG_HOME/heptix/config.toml (default ~/.config/heptix/config.toml),
created on first use.

OPTIONS:
  --run               Run using the saved settings
  --tui               Open the settings editor even with other options
  --config <PATH>     Use this config file
  --list              List detected touchpads and exit
  --device <PATH>     Watch this /dev/input/eventN instead of auto-detecting
  --sound <NAME|FILE> Tap sound: haptic, tock, tick, thud, crown, wheel_of_time, or a WAV/CAF file
  --scroll-sound <X>  Two-finger scroll sound: any of the above, or off
  --scroll-ticks <N>  Scroll ticks per pad height (higher = denser)
  --keyboard-sound <X>  Keyboard sound: thock, clicky, linear, buckling, bucklespring
                      (recorded), a sample directory, or off (default bucklespring)
  --volume-mode <M>   auto (follow the system volume) or custom (use the volumes below)
  --volume <0-100>    Tap volume (custom mode)
  --scroll-volume <0-100>  Scroll tick volume (custom mode)
  --keyboard-volume <0-100>  Keyboard volume (custom mode)
  --auto-reference <10-100>  Auto mode: system volume % up to which sounds play at full level
  --max-ms <MS>       Longest touch still counted as a tap
  --stats             Print your activity stats (keys, taps, scrolling) and exit
  --stats-reset       Delete all stored activity stats and exit
  --preview           Play the tap and scroll sounds once and exit
  --verbose           Print every detected tap/scroll (key presses are never printed)
  -h, --help          Show this help
";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// Lets the main loop flush stats on `systemctl stop/restart` and Ctrl-C.
fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}

enum Msg {
    Pad(tap::Event),
    Key(keyboard::KeyEvent),
}

struct Cli {
    tui: bool,
    list: bool,
    preview: bool,
    stats: bool,
    stats_reset: bool,
    verbose: bool,
    device: Option<PathBuf>,
    config: PathBuf,
    overrides: Vec<(String, String)>,
}

fn parse_args() -> Cli {
    let mut c = Cli {
        tui: false,
        list: false,
        preview: false,
        stats: false,
        stats_reset: false,
        verbose: false,
        device: None,
        config: config::default_path(),
        overrides: Vec::new(),
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    // No arguments on a terminal opens the editor; a service has no terminal.
    c.tui = args.is_empty() && unsafe { libc::isatty(0) == 1 };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next().unwrap_or_else(|| {
                eprintln!("{name} needs a value");
                exit(2)
            })
        };
        match arg.as_str() {
            "--run" => {}
            "--tui" => c.tui = true,
            "--list" => c.list = true,
            "--preview" => c.preview = true,
            "--stats" => c.stats = true,
            "--stats-reset" => c.stats_reset = true,
            "--verbose" => c.verbose = true,
            "--config" => c.config = value("--config").into(),
            "--device" => c.device = Some(value("--device").into()),
            "--sound" | "--scroll-sound" | "--scroll-ticks" | "--volume" | "--scroll-volume" | "--max-ms"
            | "--keyboard-sound" | "--keyboard-volume" | "--volume-mode" | "--auto-reference" => {
                let v = value(&arg);
                c.overrides.push((arg.trim_start_matches("--").replace('-', "_"), v));
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                exit(0)
            }
            other => {
                eprintln!("unknown option: {other}\n\n{USAGE}");
                exit(2)
            }
        }
    }
    c
}

/// Applies command-line overrides through the same parser the config file uses.
fn apply_overrides(s: &mut Settings, overrides: &[(String, String)]) {
    if overrides.is_empty() {
        return;
    }
    let mut text = s.to_toml();
    for (k, v) in overrides {
        text += &format!("{k} = \"{v}\"\n");
    }
    let (merged, warnings) = Settings::parse(&text);
    for w in warnings {
        eprintln!("option: {w}");
    }
    *s = merged;
}

fn find_touchpads(only: Option<PathBuf>) -> Vec<(PathBuf, Device)> {
    if let Some(path) = only {
        return match Device::open(&path) {
            Ok(d) => vec![(path, d)],
            Err(e) => {
                eprintln!("cannot open {}: {e}", path.display());
                exit(1)
            }
        };
    }
    evdev::enumerate().filter(|(_, d)| tap::is_touchpad(d)).collect()
}

fn load_sound(spec: &str, volume: u32) -> Vec<u8> {
    sound::load(spec, volume).unwrap_or_else(|e| {
        eprintln!("{e}");
        exit(2)
    })
}

fn main() {
    let cli = parse_args();
    let (mut settings, notes) = Settings::load_or_create(&cli.config);

    if cli.tui {
        if let Err(e) = tui::run(&cli.config, settings, &notes) {
            eprintln!("tui: {e}");
            exit(1);
        }
        return;
    }
    for n in &notes {
        eprintln!("{n}");
    }
    apply_overrides(&mut settings, &cli.overrides);

    if cli.stats_reset {
        match stats::wipe(&stats::default_path()) {
            Ok(true) => println!("activity stats deleted"),
            Ok(false) => println!("no activity stats stored"),
            Err(e) => {
                eprintln!("cannot delete stats: {e}");
                exit(1);
            }
        }
        return;
    }
    if cli.stats {
        let stats = stats::load(&stats::default_path()).unwrap_or_else(|e| {
            eprintln!("cannot read stats: {e}");
            exit(1)
        });
        let color = unsafe { libc::isatty(1) == 1 } && std::env::var_os("NO_COLOR").is_none();
        println!("{}", heatmap::summary(&stats, stats::Local::now().day, &settings.distance_unit, color, settings.stats).join("\n"));
        return;
    }

    let pads = find_touchpads(cli.device.clone());
    if cli.list {
        for (path, d) in &pads {
            println!("{}\t{}", path.display(), d.name().unwrap_or("?"));
        }
        return;
    }

    let (tap_volume, scroll_volume, keyboard_volume) = settings.effective_volumes();
    let tap_pcm = Arc::new(load_sound(&settings.sound, tap_volume));
    let scroll_on = settings.scroll_sound != "off";
    let scroll_pcm = scroll_on.then(|| Arc::new(load_sound(&settings.scroll_sound, scroll_volume)));
    let keyboard_on = settings.keyboard_sound != "off";
    let keyboard = keyboard_on.then(|| {
        keyboard::Keyboard::new(&settings.keyboard_sound, keyboard_volume).unwrap_or_else(|e| {
            eprintln!("{e}");
            exit(2)
        })
    });
    let player = player::Player::start();

    if cli.preview {
        if settings.is_auto_volume() {
            if let Some(r) = sysvol::query() {
                player.set_master(sysvol::compensation(r.amp, settings.auto_reference));
            }
        }
        player.play(&tap_pcm);
        thread::sleep(std::time::Duration::from_millis(400));
        if let Some(p) = &scroll_pcm {
            for _ in 0..8 {
                player.play(p);
                thread::sleep(std::time::Duration::from_millis(60));
            }
        }
        thread::sleep(std::time::Duration::from_millis(400));
        if let Some(mut k) = keyboard {
            k.demo(&player);
            thread::sleep(std::time::Duration::from_millis(400));
        }
        return;
    }

    let stats_on = settings.stats;
    // Counting keystrokes needs the keyboards even when their sound is off.
    let keyboard_wanted = keyboard_on || stats_on;
    let keyboards: Vec<(PathBuf, Device)> =
        if keyboard_wanted { evdev::enumerate().filter(|(_, d)| keyboard::is_keyboard(d)).collect() } else { Vec::new() };
    if pads.is_empty() && keyboards.is_empty() {
        eprintln!(
            "no touchpad{} found (or no permission). Readable devices need membership in the \
             'input' group; see README.",
            if keyboard_wanted { " or keyboard" } else { "" }
        );
        exit(1);
    }
    if keyboard_wanted && keyboards.is_empty() {
        eprintln!("keyboard sound or stats are on but no keyboard was found (or no permission)");
    }

    let (tx, rx) = mpsc::channel::<Msg>();
    for (path, dev) in pads {
        eprintln!("watching {} ({})", path.display(), dev.name().unwrap_or("?"));
        let tx = tx.clone();
        let (max_ms, verbose) = (settings.max_ms, cli.verbose);
        let ticks = if scroll_on || stats_on { settings.scroll_ticks } else { 0 };
        thread::spawn(move || {
            if let Err(e) = tap::watch(dev, max_ms, ticks, |event| {
                if verbose {
                    println!("{event:?}");
                }
                let _ = tx.send(Msg::Pad(event));
            }) {
                eprintln!("{}: {e}", path.display());
            }
        });
    }
    for (path, dev) in keyboards {
        eprintln!("watching {} ({})", path.display(), dev.name().unwrap_or("?"));
        let tx = tx.clone();
        thread::spawn(move || {
            if let Err(e) = keyboard::watch(dev, |ev| {
                let _ = tx.send(Msg::Key(ev));
            }) {
                eprintln!("{}: {e}", path.display());
            }
        });
    }
    drop(tx);
    install_signal_handlers();
    // Auto volume: keep heptix quiet when the system volume is high.
    let volume_watcher = settings.is_auto_volume().then(|| {
        let (player, reference) = (player.clone(), settings.auto_reference);
        sysvol::watch(move |amp| player.set_master(sysvol::compensation(amp, reference)))
    });
    let mut keyboard = keyboard;
    let mut store = stats_on.then(|| stats::Store::open(stats::default_path()));
    let per_key = settings.stats_keys;
    // Ends when every watcher thread has died (e.g. device unplugged), or on SIGTERM/SIGINT.
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(msg) => {
                if let Some(w) = &volume_watcher {
                    w.touch();
                }
                match msg {
                    Msg::Pad(ev) => {
                        match ev {
                            tap::Event::Tap | tap::Event::Click => player.play(&tap_pcm),
                            tap::Event::Scroll => {
                                if let Some(p) = &scroll_pcm {
                                    player.play(p);
                                }
                            }
                            tap::Event::Distance(_) => {}
                        }
                        if let Some(s) = store.as_mut() {
                            match ev {
                                tap::Event::Tap => s.record(stats::Kind::Tap, per_key),
                                tap::Event::Click => s.record(stats::Kind::Click, per_key),
                                tap::Event::Scroll => s.record(stats::Kind::Scroll, per_key),
                                tap::Event::Distance(um) => s.record_distance_um(um),
                            }
                        }
                    }
                    Msg::Key(ev) => {
                        if let Some(k) = keyboard.as_mut() {
                            k.handle(ev, &player);
                        }
                        if ev.down {
                            if let Some(s) = store.as_mut() {
                                s.record(stats::Kind::Key(ev.code), per_key);
                            }
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(s) = store.as_mut() {
            s.flush(false);
        }
        if STOP.load(Ordering::Relaxed) {
            break;
        }
    }
    if let Some(s) = store.as_mut() {
        s.flush(true);
    }
}
