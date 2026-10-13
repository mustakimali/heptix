# Changelog

All notable changes to heptix. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.8.0] - 2026-10-14

### Added
- Reader/client split: `heptix --reader` reads the input devices (as a
  dedicated user) and streams events over a unix socket to `heptix --client`,
  which plays sounds, keeps stats and shows the editor. The reader opens
  devices only while a client is connected and releases them when it leaves.
  `--run` uses the reader when its socket exists and reads devices directly
  otherwise; `--client` and `--direct` force one.
- The reader serves only a verified client: started from the very same
  root-owned heptix binary, by the user's `systemd --user` as `heptix.service`,
  in the reader's user and PID namespaces, not being traced, with
  `no_new_privs` set and non-dumpable. It serves only the user who owns the
  active session on the seat (or those in `/etc/heptix/allow-uids` on headless
  machines), re-checks the client every two seconds, and refuses to run from a
  binary that is not root-owned. The client connects only to a socket served by
  the `heptix` user in a root-owned directory.
- The client process hardens itself (non-dumpable, `no_new_privs`, no core
  dumps) and refuses to run while being traced.
- Service-mode installation: `install.sh` offers **service mode** (default,
  recommended: a root-installed reader service running as the `heptix` user, you
  are not added to the `input` group) or **direct mode** (not recommended: your
  user needs the `input` group, so any program you run can read your keyboard).
  `--service`, `--direct`, `--yes` and `--uninstall` options; offers to remove you
  from the `input` group; removes a previous per-user install, which would shadow
  the new unit. `HEPTIX_DRY_RUN=1` prints the steps without doing them.
- `packaging/` with the reader system unit (no network, private namespace,
  system-call filter, only the input devices), the client user unit and the
  `sysusers.d` entry; they ship in the release tarball. `make install` now builds
  and runs the installer, which asks for the mode (`MODE=service|direct` and
  `YES=1` skip the question); `make uninstall` removes either mode. A NixOS
  module (`services.heptix.enable`).
- `tools/docker-test/run.sh` tests the installer end to end in Debian, Ubuntu,
  Fedora and Arch containers running real systemd (and checks that Alpine, which
  has no systemd, is refused cleanly): service-mode install, the client passing the reader's
  verification, refusals of other programs and users, reader restart and
  reconnect, uninstall, direct mode, and switching between the modes.
- NixOS: the installer offers direct mode (works immediately) or the module
  instead of refusing; the module has `services.heptix.allowUids` for headless
  machines and a NixOS VM test (`nix build .#checks.x86_64-linux.nixos-module`).
- `install.sh` prompts work even if the terminal was left in raw mode by the
  program that started it (a full-screen tool or shell integration): Enter used
  to show as `^M` and never be accepted. The terminal is put into normal line
  mode before each prompt and restored afterwards.
- The reader's install-integrity check accepts sticky directories such as
  `/nix/store` (root:nixbld, mode 1775), where entries can only be replaced by
  their owners; every component on the path must still be root's.
- The `sysusers.d` entry also creates the `input` group when a minimal system
  lacks it (the reader unit failed to start without it).
- The editor's tab bar shows how the service gets its input: reader service
  running, not running, or direct mode.
- README: install modes, the trust model of the reader and what it does not
  stop, and the reader's lockdown.

## [0.7.0] - 2026-10-13

### Changed
- Activity view: the stats under the heatmap are now a table (keys, taps,
  clicks, scroll ticks, scroll distance) with rows for the selected day, today,
  this week, this month and all time. The column for the heatmap's metric is
  bold. A footer line shows the busiest day, the streak and when recording
  began. Rows are dropped on short terminals. `heptix --stats` prints the same
  table.
- Settings editor: options are grouped under Sounds, Volume, Touchpad and
  Activity headers that can be folded (`Enter`, or `←/→`), with a help line for
  the selected item, and the list scrolls on short terminals. Volume rows that
  do nothing in the current mode are hidden instead of greyed out. The
  "Activity stats" row is now "Record activity".
- `config.toml` has matching section comments (no change to the keys).
- Keys view header alignment and the Hours view footer capitalisation fixed.
- README screenshots are rendered from made-up data by
  `tools/render_screenshots.py` (random pattern each run).

## [0.6.0] - 2026-10-12

### Added
- Global disable: `enabled = false` (the first setting; **Enabled** is the first
  row in the editor, and `--enabled false` works on the command line) puts
  heptix in an idle state: no input devices are opened, nothing is played and
  no stats are recorded. The service stays running, so turning it back on and
  saving applies at once.
- Mouse sounds: a different click for button down and up (`mouse_sound` =
  `recorded` (default, a real CC0 recording of a click and its release),
  `tactile`, `soft` or `off`, and `mouse_volume`), and one scroll
  tick per wheel notch, rate limited. Options `--mouse-sound` and
  `--mouse-volume`; Mouse sound and Mouse volume rows in the editor, grouped
  with the other sound and volume settings.
- Hotplug for all devices: keyboards, mice and touchpads connected after
  start-up are picked up automatically, and again after being unplugged.
- Mouse clicks and wheel ticks count in the activity stats as clicks and scroll
  ticks.

### Changed
- `heptix --list` now lists keyboards and mice too, with a role column.
- A touchpad's own pointer node and repeated events are filtered out so
  nothing is played or counted twice.
- The service no longer exits when no input device can be opened at start-up;
  it keeps waiting for devices to appear.
- Auto volume mode plays taps, clicks and scroll ticks slightly louder (55 and
  45, were 45 and 35). The custom-mode defaults for `volume` and
  `scroll_volume` are raised to match.

## [0.5.0] - 2026-10-11

### Added
- Auto volume mode (`volume_mode = "auto"`, the default): sounds get quieter as
  the system volume rises, so heptix is never too loud at full system volume.
  The system volume is read via `wpctl` or `pactl` while sounds play. New
  settings `volume_mode` and `auto_reference`, and options `--volume-mode` and
  `--auto-reference`.

### Changed
- All volume settings are grouped together in the settings editor and the
  config file. In auto mode the per-sound volumes are locked at the built-in
  mix; set `volume_mode = "custom"` to use `volume`, `scroll_volume` and
  `keyboard_volume` as before. Existing configs switch to auto mode.

## [0.4.0] - 2026-10-11

### Added
- Activity stats, on by default: counters for keystrokes, touchpad taps,
  clicks, scroll ticks and scroll distance (finger travel, from the touchpad's
  reported resolution), plus per-key counts per day. Stored in
  `~/.local/state/heptix/stats.tsv` (mode 0600), 53 weeks plus all-time totals.
  Settings `stats`, `stats_keys` and `distance_unit` (`km` or `mi`).
- Settings editor views **Activity** (GitHub-style calendar heatmap, scroll
  distance today / this week / all time), **Hours** (weekday by hour of day)
  and **Keys** (most and least used keys, keyboard heatmap); `Tab` switches,
  `m` metric, `r` range, `u` km/mi, `e` recording on/off, `X` wipe.
- `heptix --stats` prints a text summary; `heptix --stats-reset` deletes all
  stored stats.
- The service flushes stats on stop or restart (SIGTERM/SIGINT).
- `install.sh` installs a missing audio player (`pw-cat`, `paplay` or `aplay`,
  matching the running sound server) using `apt`, `dnf`, `pacman` or `zypper`.

### Changed
- The direct-mode user unit runs with an explicit `--direct`.
- Physical touchpad button presses are now reported as clicks, separate from taps.
- Privacy section rewritten: counters are stored when stats are on, and the
  network lockdown of the service is spelled out.

## [0.3.0] - 2026-10-10

### Changed
- Keyboard sound is now on by default, using `bucklespring`. Existing config
  files that already contain `keyboard_sound` keep their value; set
  `keyboard_sound = "off"` to disable.
- Relicensed from MIT to GPL-2.0-only, so GPL-licensed sound samples (such as
  bucklespring's) can be bundled. Releases up to 0.2.0 remain available under MIT.
- Every source file now carries a GPL notice and SPDX identifier.

### Added
- `wheel_of_time` built-in sound (a recorded picker-wheel tick, intended for
  scrolling), embedded in the binary.
- `buckling` keyboard sound: a synthesised IBM Model M-style buckling spring
  with a metallic spring ping and steel-plate bottom-out.
- `bucklespring` keyboard sound: the recorded IBM Model M samples from
  bucklespring, embedded in the binary (adds about 5.5 MB). See `assets/README.md`.
- Recorded keyboard sounds: set `keyboard_sound` to a directory of
  `<hex keycode>-0.wav` (press) and `<hex keycode>-1.wav` (release) files, the
  layout of bucklespring's `wav` folder. Keys without a file borrow another
  key's sample.

## [0.2.0] - 2026-10-10

### Added
- Mechanical keyboard sounds, off by default. Profiles `thock`, `clicky` and
  `linear` are synthesised, with separate key-down and key-up strokes, deeper
  sounds for space and wide keys (with stabiliser rattle), a fixed pitch offset
  per key, and random per-press variation.
- Settings `keyboard_sound` and `keyboard_volume`, options `--keyboard-sound`
  and `--keyboard-volume`, and matching rows in the settings editor with a
  typing preview.
- Privacy note in the editor, config file and README: keystrokes are never
  logged, stored or sent anywhere, and `--verbose` never prints key presses.
- Network lockdown for the systemd unit (`RestrictAddressFamilies=AF_UNIX`,
  `IPAddressDeny=any`, `NoNewPrivileges=yes`).

### Changed
- The audio player now mixes overlapping sounds instead of queuing them, so
  rapid taps, scroll ticks and typing no longer wait on earlier sounds.

## [0.1.0] - 2026-10-09

### Added
- Initial release: touchpad tap, button-press and two-finger scroll sounds on
  X11 and Wayland, read via evdev.
- Built-in sounds `haptic`, `tock`, `tick`, `thud` and `crown`, plus custom
  16-bit WAV and linear-PCM CAF files.
- Settings editor (TUI) with live preview, and a config file at
  `~/.config/heptix/config.toml`.
- `install.sh` installer and systemd user service; Nix flake.
- Automatic GitHub releases when the version in `Cargo.toml` is bumped.
- README screenshot of the settings editor.

### Fixed
- `install.sh` tolerates a system without an `input` group, and accepts a
  `HEPTIX_BASE_URL` override for testing.
