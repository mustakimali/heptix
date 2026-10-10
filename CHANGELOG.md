# Changelog

All notable changes to heptix. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/).

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
