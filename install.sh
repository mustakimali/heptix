#!/bin/sh
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
