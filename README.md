# heptix

**The missing haptic feedback for Linux!**

Plays a short sound when you tap or press the touchpad, and a tick as you
scroll with two fingers. Also plays mechanical keyboard sounds as you type
(Model M by default; set `keyboard_sound = "off"` to disable). Works on X11 and Wayland (reads input devices via evdev).

![heptix settings editor](screenshot.png)

## Install

**Quick install** (latest release):

```sh
curl -fsSL https://raw.githubusercontent.com/mustakimali/heptix/main/install.sh | sh
```

Installs the latest release to `~/.local/bin`, runs it as a systemd user
service, and opens the settings editor. Needs systemd and an audio server
(PipeWire, PulseAudio or ALSA). If no audio player (`pw-cat`, `paplay` or
`aplay`) is installed, the script installs the one matching your sound server
with `apt`, `dnf`, `pacman` or `zypper` (asks for sudo).

**From source** (needs a Rust toolchain):

```sh
git clone https://github.com/mustakimali/heptix
cd heptix
make install
```

Builds and installs to `~/.local/bin` and starts the systemd user service.
Pass options with `make install ARGS="--keyboard-sound thock --volume 80"`.

Reading the touchpad needs the `input` group. The script adds you with `sudo`
(log out and in afterwards). On NixOS add `"input"` to
`users.users.<you>.extraGroups` instead.

## Dependencies

The release binary is a single static file with all sounds built in. At runtime
it needs:

- Linux with systemd (for the background service).
- An audio player on `PATH`: `pw-cat` (PipeWire), `paplay` (PulseAudio) or
  `aplay` (ALSA). The installer adds one if missing.
- Membership in the `input` group, to read `/dev/input/event*`.

Building from source also needs a Rust toolchain.

## Privacy

**heptix cannot send your data anywhere.** The background service runs in
lockdown mode: systemd blocks all network access for it
(`RestrictAddressFamilies=AF_UNIX`, `IPAddressDeny=any`, `NoNewPrivileges=yes`
in the unit file). It cannot open an IPv4 or IPv6 socket at all, only local
unix sockets such as your audio server. So there are no analytics, no crash
reports, no update checks and no phoning home, and the code has no networking
dependencies to begin with. (A copy you start by hand, such as the settings
editor, is not under the unit's restrictions, but it contains no network code
either.)

What heptix records, because activity stats are on by default:

- **Counters only**: totals per hour (keystrokes, touchpad taps, clicks,
  scroll ticks, and how far your fingers travelled while scrolling), and how
  often each key was pressed per day.
- **Never**: which keys you pressed in what order, the time between keys, the
  text you typed, window titles or anything else. Counts can still show
  habits, such as when you are at the computer.
- **Where**: `~/.local/state/heptix/stats.tsv`, readable only by you, kept for
  53 weeks plus all-time totals. It never leaves your machine.
- **Control**: turn it off with `stats = false` (or in the editor), stop only
  the per-key counts with `stats_keys = false`, and delete everything with
  `heptix --stats-reset` or `X` in the Activity view.

Keyboard sound (also on by default) only reacts to a key going down or up to
pick a click. `--verbose` never prints key presses. Set `keyboard_sound = "off"`
and `stats = false` and heptix stops reading keyboards altogether.

## Use

- `heptix` opens the settings editor (sound, volumes, tap length, scroll
  density; changes preview as you make them, `s` saves and restarts the service).
- The editor also has **Activity**, **Hours** and **Keys** views (`Tab` to
  switch): a GitHub-style heatmap of your keystrokes, touchpad taps, clicks,
  scroll ticks or scroll distance by day, a weekday-by-hour grid of when you
  are active, your most and least used keys (plus a keyboard heatmap), and how
  far you have scrolled today, this week and in total (km or miles, `u` to
  switch). `heptix --stats` prints a text summary. See Privacy for what is stored.
- `heptix --run` runs with the saved settings; other options override them
  (`heptix --help`).
- Settings: `~/.config/heptix/config.toml` (created on first run).
- Keyboard sounds (on by default, `bucklespring`): set `keyboard_sound` to
  `off`, `thock`, `clicky`, `linear`, `buckling` (synthesised Model M-style
  spring ping) or `bucklespring` (or use the editor). Separate down/up strokes, different sounds
  for space and wide keys, and slight per-key pitch and level variation.
  A key event only picks which click to play (see Privacy).
  `bucklespring` plays recorded IBM Model M samples built into the binary
  (from [bucklespring](https://github.com/zevv/bucklespring), GPL-2.0).
  To use your own recordings, set `keyboard_sound` to a directory of
  `<hex keycode>-0.wav` (press) and `-1.wav` (release) files.
- Sounds: `thud` (default tap), `haptic`, `tock`, `tick`, `crown` (default
  scroll), `wheel_of_time` (a recorded picker-wheel tick, good for scrolling), or your own 16-bit WAV / linear-PCM CAF file path.

## Build

```sh
cargo build --release     # or: nix run github:mustakimali/heptix
make install              # install + start the service from source (see Install)
```

Uninstall: `systemctl --user disable --now heptix; rm ~/.local/bin/heptix`

Linux only. Releases are published automatically when `version` in
`Cargo.toml` is bumped on the default branch.

## License

GPL-2.0-only. Releases up to and including 0.2.0 were published under the MIT license.
