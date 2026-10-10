#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# heptix - the missing haptic feedback for Linux!
# Copyright (C) 2026 M Mustakim Ali
#
# This program is free software; you can redistribute it and/or modify
# it under the terms of the GNU General Public License version 2 as
# published by the Free Software Foundation.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program; if not, see <https://www.gnu.org/licenses/>.

# heptix installer: downloads the latest release, installs it as a systemd
# user service, then opens the settings editor.
set -eu

REPO="mustakimali/heptix"
BIN_DIR="$HOME/.local/bin"
UNIT_DIR="$HOME/.config/systemd/user"

[ "$(uname -s)" = Linux ] || { echo "heptix only supports Linux." >&2; exit 1; }
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) echo "Unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac
command -v systemctl >/dev/null || { echo "systemd is required for the background service." >&2; exit 1; }

# heptix plays sound through pw-cat, paplay or aplay; try to install the one
# matching the running sound server if none is present.
ensure_audio_player() {
  for p in pw-cat paplay aplay; do command -v "$p" >/dev/null 2>&1 && return 0; done
  if [ -e /etc/NIXOS ]; then
    echo "No audio player found. On NixOS, add pipewire to your system packages." >&2
    return 0
  fi
  if pgrep -x pipewire >/dev/null 2>&1; then server=pipewire
  elif pgrep -x pulseaudio >/dev/null 2>&1; then server=pulse
  else server=alsa; fi
  if command -v apt-get >/dev/null 2>&1; then
    install_cmd="apt-get install -y"
    case $server in pipewire) pkg=pipewire-bin ;; pulse) pkg=pulseaudio-utils ;; *) pkg=alsa-utils ;; esac
  elif command -v dnf >/dev/null 2>&1; then
    install_cmd="dnf install -y"
    case $server in pipewire) pkg=pipewire-utils ;; pulse) pkg=pulseaudio-utils ;; *) pkg=alsa-utils ;; esac
  elif command -v pacman >/dev/null 2>&1; then
    install_cmd="pacman -S --needed --noconfirm"
    case $server in pipewire) pkg=pipewire ;; pulse) pkg=libpulse ;; *) pkg=alsa-utils ;; esac
  elif command -v zypper >/dev/null 2>&1; then
    install_cmd="zypper --non-interactive install"
    case $server in pipewire) pkg=pipewire-tools ;; pulse) pkg=pulseaudio-utils ;; *) pkg=alsa-utils ;; esac
  else
    echo "No audio player found (pw-cat, paplay or aplay). Install one for your sound server." >&2
    return 0
  fi
  sudo_cmd=""
  [ "$(id -u)" = 0 ] || sudo_cmd=sudo
  echo "No audio player found; installing $pkg (may ask for your password) ..."
  # shellcheck disable=SC2086
  if ! $sudo_cmd $install_cmd "$pkg"; then
    echo "Could not install $pkg. Install pw-cat, paplay or aplay yourself, or heptix will be silent." >&2
  fi
}

name="heptix-$arch-unknown-linux-musl.tar.gz"
# HEPTIX_BASE_URL overrides the download location (used for testing).
url="${HEPTIX_BASE_URL:-https://github.com/$REPO/releases/latest/download}/$name"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $name ..."
curl -fsSL "$url" -o "$tmp/$name"
curl -fsSL "$url.sha256" -o "$tmp/$name.sha256"
(cd "$tmp" && sha256sum -c "$name.sha256" >/dev/null) || { echo "Checksum mismatch." >&2; exit 1; }
tar -xzf "$tmp/$name" -C "$tmp"

mkdir -p "$BIN_DIR" "$UNIT_DIR"
install -m 755 "$tmp/heptix" "$BIN_DIR/heptix"

cat > "$UNIT_DIR/heptix.service" <<UNIT
[Unit]
Description=heptix - the missing haptic feedback for Linux
After=pipewire.service

[Service]
ExecStart=$BIN_DIR/heptix --run
Restart=on-failure
RestartSec=2
# No network: only local unix sockets (the audio server) are allowed.
RestrictAddressFamilies=AF_UNIX
IPAddressDeny=any
NoNewPrivileges=yes
# User units get a minimal PATH; heptix needs pw-cat, paplay or aplay.
Environment=PATH=/run/current-system/sw/bin:/etc/profiles/per-user/%u/bin:/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=default.target
UNIT

systemctl --user daemon-reload
systemctl --user enable heptix.service >/dev/null 2>&1

ensure_audio_player

# Reading /dev/input needs the 'input' group.
if ! id -nG | tr ' ' '\n' | grep -qx input; then
  if [ -e /etc/NIXOS ]; then
    echo
    echo "Add yourself to the input group in your NixOS config, rebuild, then log out and in:"
    echo "  users.users.$USER.extraGroups = [ \"input\" ];"
  else
    echo
    if getent group input >/dev/null; then
      echo "Adding $USER to the 'input' group (needs sudo) ..."
      if sudo usermod -aG input "$USER"; then
        echo "Log out and back in for this to take effect."
      else
        echo "Could not add you to 'input'. Run: sudo usermod -aG input $USER" >&2
      fi
    else
      echo "No 'input' group on this system; make sure your user can read /dev/input/event*." >&2
    fi
  fi
  echo "heptix will start automatically at your next login."
else
  systemctl --user restart heptix.service
  echo "heptix is running."
fi

case ":$PATH:" in *":$BIN_DIR:"*) ;; *) echo "Note: add $BIN_DIR to your PATH to run 'heptix' yourself." ;; esac

# First run: open the settings editor (stdin is the script when piped from curl).
if ( : </dev/tty ) 2>/dev/null; then
  exec "$BIN_DIR/heptix" </dev/tty >/dev/tty
fi
echo "Run 'heptix' to change settings."
