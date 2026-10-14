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

mod client;
mod config;
mod devices;
mod harden;
mod heatmap;
mod info;
mod keyboard;
mod mouse;
mod peer;
mod player;
mod proto;
mod reader;
mod sound;
mod stats;
mod sysvol;
mod tap;
mod tui;
mod verify;

use config::Settings;
use std::path::{Path, PathBuf};
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
  --run               Run using the saved settings (reads input through the heptix reader
                      service if it is installed, otherwise directly)
  --client            Run, reading input only through the heptix reader service
  --direct            Run, reading the input devices directly (needs the 'input' group;
                      not recommended, see the README)
  --reader            Run the reader service (installed as a system service, not by hand)
  --reader-socket <P> Socket path (default /run/heptix/events.sock)
  --reader-uid <UID>  Reader: serve only this user id (default: the user of the active seat
                      session, or those listed in /etc/heptix/allow-uids). Client: the
                      user id the reader runs as (default: the 'heptix' user)
  --reader-insecure   Reader, testing only: skip checking the client (needs --reader-uid)
  --reader-unsafe-install  Reader, testing only: serve from a binary that is not root-owned
  --enabled <BOOL>    Master switch: false keeps heptix idle (no input read, no sound, no stats)
  --tui               Open the settings editor even with other options
  --config <PATH>     Use this config file
  --list              List detected touchpads, keyboards and mice and exit
  --info              Show the version, file and socket paths, running mode and service state, and exit
  --device <PATH>     Use this /dev/input/eventN as the touchpad instead of auto-detecting
  --sound <NAME|FILE> Tap sound: haptic, tock, tick, thud, crown, wheel_of_time, or a WAV/CAF file
  --scroll-sound <X>  Two-finger scroll sound: any of the above, or off
  --scroll-ticks <N>  Scroll ticks per pad height (higher = denser)
  --keyboard-sound <X>  Keyboard sound: thock, clicky, linear, buckling, bucklespring
                      (recorded), a sample directory, or off (default bucklespring)
  --mouse-sound <X>   Mouse button sound: recorded, tactile, soft, or off (default recorded)
  --volume-mode <M>   auto (follow the system volume) or custom (use the volumes below)
  --volume <0-100>    Tap volume (custom mode)
  --scroll-volume <0-100>  Scroll tick volume (custom mode)
  --keyboard-volume <0-100>  Keyboard volume (custom mode)
  --mouse-volume <0-100>  Mouse volume (custom mode)
  --auto-reference <10-100>  Auto mode: system volume % up to which sounds play at full level
  --max-ms <MS>       Longest touch still counted as a tap
  --stats             Print your activity stats (keys, taps, scrolling) and exit
  --stats-reset       Delete all stored activity stats and exit
  --preview           Play the tap and scroll sounds once and exit
  --verbose           Print every detected tap/scroll (key presses are never printed)
  -h, --help          Show this help
";

/// Where input events come from; dropping it disconnects or stops the watchers.
#[allow(dead_code)]
enum Source {
    Direct(devices::Handle),
    Reader(client::Handle),
}

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

struct Cli {
    reader: bool,
    client: bool,
    direct: bool,
    reader_socket: Option<PathBuf>,
    reader_uid: Option<u32>,
    reader_insecure: bool,
    reader_unsafe_install: bool,
    tui: bool,
    list: bool,
    info: bool,
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
        reader: false,
        client: false,
        direct: false,
        reader_socket: None,
        reader_uid: None,
        reader_insecure: false,
        reader_unsafe_install: false,
        tui: false,
        list: false,
        info: false,
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
            "--reader" => c.reader = true,
            "--client" => c.client = true,
            "--direct" => c.direct = true,
            "--reader-insecure" => c.reader_insecure = true,
            "--reader-unsafe-install" => c.reader_unsafe_install = true,
            "--reader-socket" => c.reader_socket = Some(value("--reader-socket").into()),
            "--reader-uid" => {
                let v = value("--reader-uid");
                c.reader_uid = Some(v.parse().unwrap_or_else(|_| {
                    eprintln!("--reader-uid needs a number");
                    exit(2)
                }));
            }
            "--list" => c.list = true,
            "--info" => c.info = true,
            "--preview" => c.preview = true,
            "--stats" => c.stats = true,
            "--stats-reset" => c.stats_reset = true,
            "--verbose" => c.verbose = true,
            "--config" => c.config = value("--config").into(),
            "--device" => c.device = Some(value("--device").into()),
            "--sound" | "--scroll-sound" | "--scroll-ticks" | "--volume" | "--scroll-volume" | "--max-ms"
            | "--keyboard-sound" | "--keyboard-volume" | "--volume-mode" | "--auto-reference" | "--mouse-sound" | "--mouse-volume" | "--enabled" => {
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

fn load_sound(spec: &str, volume: u32) -> Vec<u8> {
    sound::load(spec, volume).unwrap_or_else(|e| {
        eprintln!("{e}");
        exit(2)
    })
}

/// The reader service: serves input events to the verified client and nothing else.
fn run_reader(cli: &Cli) -> ! {
    let socket = cli.reader_socket.clone().unwrap_or_else(|| client::DEFAULT_SOCKET.into());
    let auth: Arc<dyn reader::Authorizer> = if cli.reader_insecure {
        let Some(uid) = cli.reader_uid else {
            eprintln!("--reader-insecure needs --reader-uid <UID>");
            exit(2)
        };
        eprintln!("WARNING: --reader-insecure: the client is NOT verified; for testing only");
        Arc::new(reader::AllowUid(uid))
    } else {
        if !cli.reader_unsafe_install {
            if let Err(why) = verify::check_install(Path::new("/proc/self/exe")) {
                eprintln!("refusing to serve: {why}. Install heptix as root (see the README), or use --reader-unsafe-install for testing.");
                exit(1)
            }
        }
        let proc = verify::ProcFs::system();
        let allowed = match cli.reader_uid {
            Some(uid) => verify::Allowed::Fixed(uid),
            None => verify::Allowed::Seat { seat: "/run/systemd/seats/seat0".into(), allowlist: "/etc/heptix/allow-uids".into() },
        };
        let policy = verify::Policy::for_this_process(&proc, allowed, true, true).unwrap_or_else(|e| {
            eprintln!("cannot describe this process: {e}");
            exit(1)
        });
        Arc::new(verify::Verified { proc, policy })
    };
    let listener = reader::bind(&socket).unwrap_or_else(|e| {
        eprintln!("cannot listen on {}: {e}", socket.display());
        exit(1)
    });
    install_signal_handlers();
    eprintln!("heptix reader listening on {}", socket.display());
    let source: Arc<reader::Source> = Arc::new(|sub, tx| {
        devices::start(
            tx,
            devices::Options {
                only_touchpad: sub.only_touchpad,
                keyboards: sub.keyboards,
                mice: sub.mice,
                max_ms: sub.max_ms as u64,
                scroll_ticks: sub.scroll_ticks,
                verbose: false,
                hotplug: true,
            },
        )
    });
    let stop = Arc::new(AtomicBool::new(false));
    let watcher_stop = Arc::clone(&stop);
    thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            thread::sleep(std::time::Duration::from_millis(200));
        }
        watcher_stop.store(true, Ordering::Relaxed);
    });
    reader::serve(listener, auth, source, stop);
    let _ = std::fs::remove_file(&socket);
    exit(0)
}

fn main() {
    let cli = parse_args();
    if cli.reader {
        run_reader(&cli);
    }
    if cli.info {
        info::print(&cli.config, &cli.reader_socket.clone().unwrap_or_else(|| client::DEFAULT_SOCKET.into()));
        return;
    }
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

    let stats_on = settings.stats;
    let keyboard_on = settings.keyboard_sound != "off";
    let mouse_on = settings.mouse_sound != "off";
    // Counting keystrokes and clicks needs those devices even when their sound is off.
    let device_options = devices::Options {
        only_touchpad: cli.device.clone(),
        keyboards: keyboard_on || stats_on,
        mice: mouse_on || stats_on,
        max_ms: settings.max_ms,
        scroll_ticks: if settings.scroll_sound != "off" || stats_on { settings.scroll_ticks } else { 0 },
        verbose: cli.verbose,
        hotplug: true,
    };
    if cli.list {
        let all = devices::Options { keyboards: true, mice: true, ..device_options.clone() };
        for (path, name, role) in devices::list(&all) {
            println!("{}\t{}\t{}", path.display(), name, role.name());
        }
        return;
    }

    if !settings.enabled && !cli.preview {
        // Idle rather than exit, so the service stays up and a restart (after
        // turning it back on) simply works.
        eprintln!("heptix is turned off (enabled = false): not reading input, playing sounds or recording stats");
        install_signal_handlers();
        while !STOP.load(Ordering::Relaxed) {
            thread::sleep(std::time::Duration::from_millis(500));
        }
        return;
    }

    // From here on this process holds raw input events: make it hard to inspect, before
    // any other thread exists. (The reader's checks for the client rely on this.)
    if !cli.preview {
        if let Err(e) = harden::harden_process() {
            eprintln!("cannot harden the process: {e}");
            exit(1);
        }
    }

    let (tap_volume, scroll_volume, keyboard_volume, mouse_volume) = settings.effective_volumes();
    let tap_pcm = Arc::new(load_sound(&settings.sound, tap_volume));
    let scroll_on = settings.scroll_sound != "off";
    let scroll_pcm = scroll_on.then(|| Arc::new(load_sound(&settings.scroll_sound, scroll_volume)));
    let keyboard = keyboard_on.then(|| {
        keyboard::Keyboard::new(&settings.keyboard_sound, keyboard_volume).unwrap_or_else(|e| {
            eprintln!("{e}");
            exit(2)
        })
    });
    let mouse = mouse_on.then(|| {
        mouse::Mouse::new(&settings.mouse_sound, mouse_volume).unwrap_or_else(|e| {
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
        if let Some(mut m) = mouse {
            m.demo(&player);
            thread::sleep(std::time::Duration::from_millis(400));
        }
        return;
    }

    let (tx, rx) = mpsc::channel::<devices::Msg>();
    let socket = cli.reader_socket.clone().unwrap_or_else(|| client::DEFAULT_SOCKET.into());
    // `--run` uses the reader service when it is installed, and reads the devices
    // itself otherwise (the older setup); `--client` and `--direct` force one.
    let use_reader = cli.client || (!cli.direct && socket.exists());
    let _source = if use_reader {
        Source::Reader(client::spawn(
            client::Config { socket, expected_uid: cli.reader_uid },
            proto::Subscribe {
                keyboards: device_options.keyboards,
                mice: device_options.mice,
                max_ms: device_options.max_ms as u32,
                scroll_ticks: device_options.scroll_ticks,
                only_touchpad: device_options.only_touchpad.clone(),
            },
            tx,
        ))
    } else {
        let handle = devices::start(tx, device_options);
        if handle.attached == 0 {
            eprintln!(
                "no input devices opened yet (waiting for hotplug). If you expected some, check that you \
                 are in the 'input' group, or install the reader service; see README."
            );
        }
        Source::Direct(handle)
    };
    install_signal_handlers();
    // Auto volume: keep heptix quiet when the system volume is high.
    let volume_watcher = settings.is_auto_volume().then(|| {
        let (player, reference) = (player.clone(), settings.auto_reference);
        sysvol::watch(move |amp| player.set_master(sysvol::compensation(amp, reference)))
    });
    let (mut keyboard, mut mouse) = (keyboard, mouse);
    let mut mouse_filter = mouse::Filter::default();
    let mut store = stats_on.then(|| stats::Store::open(stats::default_path()));
    let per_key = settings.stats_keys;
    // Runs until SIGTERM/SIGINT; devices come and go underneath (hotplug). On the signal it
    // keeps going until the events that already arrived are processed (a quiet moment, or
    // two seconds at most), so they are in the stats the final flush writes.
    let mut stopping: Option<std::time::Instant> = None;
    let (mut events_seen, mut key_presses_seen) = (0u64, 0u64);
    loop {
        let wait = if stopping.is_some() { 150 } else { 500 };
        match rx.recv_timeout(std::time::Duration::from_millis(wait)) {
            Ok(msg) => {
                events_seen += 1;
                key_presses_seen += matches!(msg, devices::Msg::Key(ev) if ev.down) as u64;
                if let Some(w) = &volume_watcher {
                    w.touch();
                }
                match msg {
                    devices::Msg::Pad(ev) => {
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
                    devices::Msg::Key(ev) => {
                        if let Some(k) = keyboard.as_mut() {
                            k.handle(ev, &player);
                        }
                        if ev.down {
                            if let Some(s) = store.as_mut() {
                                s.record(stats::Kind::Key(ev.code), per_key);
                            }
                        }
                    }
                    devices::Msg::Mouse(ev) => {
                        // The same click can arrive from two nodes; count and play it once.
                        if mouse_filter.accept(ev, std::time::Instant::now()) {
                            match ev {
                                mouse::Event::Button { code, down } => {
                                    if let Some(m) = mouse.as_mut() {
                                        m.handle_button(code, down, &player);
                                    }
                                    if down {
                                        if let Some(s) = store.as_mut() {
                                            s.record(stats::Kind::Click, per_key);
                                        }
                                    }
                                }
                                mouse::Event::Wheel => {
                                    if let Some(p) = &scroll_pcm {
                                        player.play(p);
                                    }
                                    if let Some(s) = store.as_mut() {
                                        s.record(stats::Kind::Scroll, per_key);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) if stopping.is_some() => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(s) = store.as_mut() {
            s.flush(false);
        }
        if STOP.load(Ordering::Relaxed) {
            let since = *stopping.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() > std::time::Duration::from_secs(2) {
                break;
            }
        }
    }
    if let Some(s) = store.as_mut() {
        s.flush(true);
    }
    // Counts only, never which keys.
    eprintln!("stopping after {events_seen} events ({key_presses_seen} key presses)");
}
