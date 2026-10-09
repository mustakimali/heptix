# heptix

**The missing haptic feedback for Linux!**

Plays a short sound when you tap or press the touchpad, and a tick as you
scroll with two fingers. Works on X11 and Wayland (reads the touchpad via evdev).

![heptix settings editor](screenshot.png)

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/mustakimali/heptix/main/install.sh | sh
```

Installs the latest release to `~/.local/bin`, runs it as a systemd user
service, and opens the settings editor. Needs systemd and an audio server
(PipeWire, PulseAudio or ALSA).

Reading the touchpad needs the `input` group. The script adds you with `sudo`
(log out and in afterwards). On NixOS add `"input"` to
`users.users.<you>.extraGroups` instead.

## Use

- `heptix` opens the settings editor (sound, volumes, tap length, scroll
  density; changes preview as you make them, `s` saves and restarts the service).
- `heptix --run` runs with the saved settings; other options override them
  (`heptix --help`).
- Settings: `~/.config/heptix/config.toml` (created on first run).
- Sounds: `thud` (default tap), `haptic`, `tock`, `tick`, `crown` (default
  scroll), or your own 16-bit WAV / linear-PCM CAF file path.

## Build

```sh
cargo build --release     # or: nix run github:mustakimali/heptix
make install              # install + start the service from source
```

Uninstall: `systemctl --user disable --now heptix; rm ~/.local/bin/heptix`

Linux only. Releases are published automatically when `version` in
`Cargo.toml` is bumped on the default branch.

## License

MIT
