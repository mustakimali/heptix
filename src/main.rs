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
mod keyboard;
mod player;
mod sound;
mod tap;
mod tui;

use config::Settings;
use evdev::Device;
use std::path::PathBuf;
use std::process::exit;
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
  --keyboard-volume <0-100>  Keyboard volume
  --volume <0-100>    Tap volume
  --scroll-volume <0-100>  Scroll tick volume
  --max-ms <MS>       Longest touch still counted as a tap
  --preview           Play the tap and scroll sounds once and exit
  --verbose           Print every detected tap/scroll (key presses are never printed)
  -h, --help          Show this help
";

enum Msg {
    Pad(tap::Event),
    Key(keyboard::KeyEvent),
}

struct Cli {
    tui: bool,
    list: bool,
    preview: bool,
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
            "--verbose" => c.verbose = true,
            "--config" => c.config = value("--config").into(),
            "--device" => c.device = Some(value("--device").into()),
            "--sound" | "--scroll-sound" | "--scroll-ticks" | "--volume" | "--scroll-volume" | "--max-ms"
            | "--keyboard-sound" | "--keyboard-volume" => {
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

    let pads = find_touchpads(cli.device.clone());
    if cli.list {
        for (path, d) in &pads {
            println!("{}\t{}", path.display(), d.name().unwrap_or("?"));
        }
        return;
    }

    let tap_pcm = Arc::new(load_sound(&settings.sound, settings.volume));
    let scroll_on = settings.scroll_sound != "off";
    let scroll_pcm = scroll_on.then(|| Arc::new(load_sound(&settings.scroll_sound, settings.scroll_volume)));
    let keyboard_on = settings.keyboard_sound != "off";
    let keyboard = keyboard_on.then(|| {
        keyboard::Keyboard::new(&settings.keyboard_sound, settings.keyboard_volume).unwrap_or_else(|e| {
            eprintln!("{e}");
            exit(2)
        })
    });
    let player = player::Player::start();

    if cli.preview {
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

    let keyboards: Vec<(PathBuf, Device)> =
        if keyboard_on { evdev::enumerate().filter(|(_, d)| keyboard::is_keyboard(d)).collect() } else { Vec::new() };
    if pads.is_empty() && keyboards.is_empty() {
        eprintln!(
            "no touchpad{} found (or no permission). Readable devices need membership in the \
             'input' group; see README.",
            if keyboard_on { " or keyboard" } else { "" }
        );
        exit(1);
    }
    if keyboard_on && keyboards.is_empty() {
        eprintln!("keyboard sound is on but no keyboard was found (or no permission)");
    }

    let (tx, rx) = mpsc::channel::<Msg>();
    for (path, dev) in pads {
        eprintln!("watching {} ({})", path.display(), dev.name().unwrap_or("?"));
        let tx = tx.clone();
        let (max_ms, verbose) = (settings.max_ms, cli.verbose);
        let ticks = if scroll_on { settings.scroll_ticks } else { 0 };
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
    let mut keyboard = keyboard;
    // Ends when every watcher thread has died (e.g. device unplugged).
    for msg in rx {
        match msg {
            Msg::Pad(tap::Event::Tap) => player.play(&tap_pcm),
            Msg::Pad(tap::Event::Scroll) => {
                if let Some(p) = &scroll_pcm {
                    player.play(p);
                }
            }
            Msg::Key(ev) => {
                if let Some(k) = keyboard.as_mut() {
                    k.handle(ev, &player);
                }
            }
        }
    }
}
