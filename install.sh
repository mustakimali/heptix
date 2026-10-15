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

# heptix installer. Two ways to let heptix read your keyboard and touchpad:
#
#   service mode (default, recommended): a small reader service runs as its own
#     user and serves the events to your heptix. Needs sudo once. You are not
#     added to the 'input' group, so your other programs cannot read your keyboard.
#   direct mode (not recommended): heptix reads /dev/input itself, which needs
#     you in the 'input' group. Any program you run can then read all keyboard
#     input on the machine.
#
# Usage: install.sh [--service | --direct] [--yes] [--uninstall] [--remove-input-group] [--no-verify]
# Testing: HEPTIX_DRY_RUN=1 prints what would be done; HEPTIX_LOCAL_DIR=<dir>
# (containing 'heptix' and 'packaging/') installs from there instead of downloading.
set -eu

REPO="mustakimali/heptix"
BIN_DIR="$HOME/.local/bin"
UNIT_DIR="$HOME/.config/systemd/user"
SYS_BIN="/usr/lib/heptix/heptix"
SYS_LINK="/usr/local/bin/heptix"
READER_UNIT="/etc/systemd/system/heptix-reader.service"
CLIENT_UNIT="/etc/systemd/user/heptix.service"
SYSUSERS="/etc/sysusers.d/heptix.conf"

MODE="${HEPTIX_MODE:-}"
ASSUME_YES=0
UNINSTALL=0
REMOVE_GROUP=0
NO_VERIFY="${HEPTIX_NO_VERIFY:-0}"
DRY="${HEPTIX_DRY_RUN:-0}"

usage() {
  cat <<'EOF'
Usage: install.sh [--service | --direct] [--yes] [--uninstall] [--remove-input-group] [--no-verify]

  --service    reader service (default, recommended): needs sudo, keeps you out of the 'input' group
  --direct     heptix reads the devices itself (not recommended): needs the 'input' group
  --yes        do not ask questions; take the defaults
  --uninstall  remove heptix (keeps your settings and stats)
  --remove-input-group  take you out of the 'input' group without asking (when you are in it
               and heptix does not need it: after switching to service mode, or on --uninstall)
  --no-verify  do not check the release's signature (it is checked when the GitHub CLI,
               gh, is installed and logged in; otherwise the checksum is all there is)
EOF
}

for arg in "$@"; do
  case "$arg" in
    --service) MODE=service ;;
    --direct) MODE=direct ;;
    --yes | -y) ASSUME_YES=1 ;;
    --uninstall) UNINSTALL=1 ;;
    --remove-input-group) REMOVE_GROUP=1 ;;
    --no-verify) NO_VERIFY=1 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
  esac
done

say() { printf '%s\n' "$*"; }
warn() { printf '%s\n' "$*" >&2; }
die() { warn "$*"; exit 1; }

# Runs a command, or only prints it in a dry run.
run() { if [ "$DRY" = 1 ]; then printf '+ %s\n' "$*"; else "$@"; fi; }

if [ "$(id -u)" = 0 ]; then SUDO=""; else SUDO="sudo"; fi
as_root() { if [ -n "$SUDO" ]; then run "$SUDO" "$@"; else run "$@"; fi; }

# write_file <path>: stdin becomes the file (or is shown as written, in a dry run).
write_file() { if [ "$DRY" = 1 ]; then printf '+ write %s\n' "$1"; cat >/dev/null; else cat >"$1"; fi; }

have_tty() { ( : </dev/tty ) 2>/dev/null; }

# A program that started us (a full-screen tool, a shell integration) may have left
# the terminal raw, where Enter arrives as a carriage return that `read` never
# accepts (it shows up as ^M). So before any prompt is printed, the terminal is put
# into normal line mode, and afterwards it is put back as it was.
tty_sane() {
  TTY_SAVED=$(stty -g </dev/tty 2>/dev/null) || TTY_SAVED=""
  stty icanon icrnl echo </dev/tty 2>/dev/null || true
}
tty_restore() {
  if [ -n "${TTY_SAVED:-}" ]; then stty "$TTY_SAVED" </dev/tty 2>/dev/null || true; fi
}
# read_tty <var>: one line from the terminal into <var>; call between tty_sane and tty_restore.
read_tty() {
  _line=""
  read -r _line </dev/tty || _line=""
  eval "$1=\$_line"
}

# ask <prompt> <Y|N>: yes/no with a default; no question without a terminal or with --yes.
ask() {
  if [ "$ASSUME_YES" = 1 ] || ! have_tty; then [ "$2" = Y ]; return; fi
  tty_sane
  printf '%s ' "$1" >/dev/tty
  read_tty ans
  tty_restore
  case "$ans" in "") [ "$2" = Y ] ;; [Yy]*) return 0 ;; *) return 1 ;; esac
}

# offer_leave_input_group <why>: membership of 'input' lets any program you run read your
# keyboard, so when heptix does not need it, offer to drop it. Never silent: the default
# is to keep it (you may need it for something else), and --remove-input-group says yes.
offer_leave_input_group() {
  id -nG "${USER:-$(id -un)}" 2>/dev/null | tr ' ' '\n' | grep -qx input || return 0
  say
  say "You are in the 'input' group, which lets any program you run read your keyboard."
  say "$1"
  if [ -e /etc/NIXOS ]; then
    say "On NixOS the group comes from your configuration: remove \"input\" from"
    say "users.users.${USER:-you}.extraGroups, rebuild, then log out and in."
    return 0
  fi
  if [ "$REMOVE_GROUP" = 1 ] || ask "Remove yourself from the 'input' group now? [y/N]" N; then
    as_root gpasswd -d "${USER:-$(id -un)}" input
    say "Log out and back in for this to take effect."
  else
    say "To remove it later: sudo gpasswd -d ${USER:-$(id -un)} input   (then log out and in)"
  fi
}

[ "$(uname -s)" = Linux ] || die "heptix only supports Linux."
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) die "Unsupported CPU: $(uname -m)" ;;
esac
command -v systemctl >/dev/null || die "systemd is required for the background service."

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
  echo "No audio player found; installing $pkg (may ask for your password) ..."
  # shellcheck disable=SC2086
  if ! as_root $install_cmd "$pkg"; then
    echo "Could not install $pkg. Install pw-cat, paplay or aplay yourself, or heptix will be silent." >&2
  fi
}

# The first release whose tarballs carry an attestation; older ones never will.
FIRST_SIGNED=0.9.3

# version_ge <a> <b>: whether dotted version a is at least b (numbers only).
version_ge() {
  _a=$1; _b=$2
  for _i in 1 2 3; do
    _x=$(printf '%s' "$_a" | cut -d. -f$_i); _y=$(printf '%s' "$_b" | cut -d. -f$_i)
    _x=${_x:-0}; _y=${_y:-0}
    if [ "$_x" -gt "$_y" ]; then return 0; fi
    if [ "$_x" -lt "$_y" ]; then return 1; fi
  done
  return 0
}

# release_version: the version being installed ("0.9.3"), from the tag GitHub redirects
# "latest" to. Empty when unknown. HEPTIX_RELEASE_VERSION overrides it (for tests).
release_version() {
  if [ -n "${HEPTIX_RELEASE_VERSION:-}" ]; then printf '%s' "$HEPTIX_RELEASE_VERSION"; return; fi
  [ -z "${HEPTIX_BASE_URL:-}" ] || return 0
  curl -fsSI -o /dev/null -w '%{redirect_url}' "https://github.com/$REPO/releases/latest/download/$1" 2>/dev/null |
    sed -n 's#.*/download/v\([0-9][0-9.]*\)/.*#\1#p'
}

# verify_signature <tarball> <asset name>: checks the release was built by this repository's
# release workflow (a GitHub build attestation), with the GitHub CLI when it is installed and
# logged in. Never required: without gh it says so and goes on with the checksum alone.
# But once gh runs, a release that should be signed (0.9.3 and later) must verify: a missing
# attestation is exactly what a replaced tarball looks like. Only older releases, which
# were never signed, and a download location you chose yourself, are let through.
verify_signature() {
  if [ "$NO_VERIFY" = 1 ]; then
    say "Skipping the signature check (--no-verify)."
    return 0
  fi
  if ! command -v gh >/dev/null 2>&1; then
    say "Signature not checked: install the GitHub CLI (gh) and log in to verify the release."
    return 0
  fi
  if ! gh auth status >/dev/null 2>&1; then
    say "Signature not checked: log in with 'gh auth login' to verify the release."
    return 0
  fi
  if out=$(gh attestation verify "$1" --repo "$REPO" --signer-workflow "$REPO/.github/workflows/release.yml" 2>&1); then
    say "Signature verified: built by the $REPO release workflow."
    return 0
  fi
  case "$out" in
    *"HTTP 404"*)
      version=$(release_version "$2")
      if [ -z "${HEPTIX_RELEASE_VERSION:-}" ] && [ -n "${HEPTIX_BASE_URL:-}" ]; then
        say "Signature not checked: no attestation for a download from a location you chose."
        return 0
      fi
      if [ -n "$version" ] && ! version_ge "$version" "$FIRST_SIGNED"; then
        say "Signature not checked: release $version has no attestation (releases before $FIRST_SIGNED are not signed)."
        return 0
      fi
      warn "$out"
      die "This release should carry a signature but has none: not installing. (--no-verify skips the check.)"
      ;;
  esac
  warn "$out"
  die "The release failed signature verification: not installing. (--no-verify skips the check.)"
}

# Leaves the files to install in $STAGE: the 'heptix' binary and 'packaging/'.
obtain() {
  if [ -n "${HEPTIX_LOCAL_DIR:-}" ]; then
    STAGE="$HEPTIX_LOCAL_DIR"
    return
  fi
  name="heptix-$arch-unknown-linux-musl.tar.gz"
  # HEPTIX_BASE_URL overrides the download location (used for testing).
  url="${HEPTIX_BASE_URL:-https://github.com/$REPO/releases/latest/download}/$name"
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  say "Downloading $name ..."
  curl -fsSL "$url" -o "$tmp/$name"
  curl -fsSL "$url.sha256" -o "$tmp/$name.sha256"
  (cd "$tmp" && sha256sum -c "$name.sha256" >/dev/null) || die "Checksum mismatch."
  verify_signature "$tmp/$name" "$name"
  tar -xzf "$tmp/$name" -C "$tmp"
  STAGE="$tmp"
}

choose_mode() {
  [ -n "$MODE" ] && return
  if [ "$ASSUME_YES" = 1 ] || ! have_tty; then MODE=service; return; fi
  tty_sane
  cat >/dev/tty <<'EOF'

How should heptix read your keyboard and touchpad?

  1) Service mode (recommended)
     A small reader service runs as its own user (installed with sudo). You are
     NOT added to the 'input' group, so your other programs cannot read your
     keyboard. Only the verified heptix process receives the events.

  2) Direct mode (not recommended)
     heptix reads /dev/input itself, which needs the 'input' group. Then ANY
     program you run can read all keyboard input on this machine, including
     what is typed at the login and lock screens.

EOF
  printf 'Choose [1]: ' >/dev/tty
  read_tty ans
  tty_restore
  [ -n "$ans" ] || ans=1
  case "$ans" in 2) MODE=direct ;; *) MODE=service ;; esac
}

# A per-user install of an earlier version would shadow the new user unit and
# run in direct mode, so it goes first.
remove_user_install() {
  if [ -e "$UNIT_DIR/heptix.service" ] || [ -e "$BIN_DIR/heptix" ]; then
    say "Removing the previous per-user install ..."
    run systemctl --user disable --now heptix.service 2>/dev/null || true
    run rm -f "$UNIT_DIR/heptix.service" "$BIN_DIR/heptix"
    run systemctl --user daemon-reload
  fi
}

service_install() {
  [ -f "$STAGE/packaging/system/heptix-reader.service" ] || die "This release has no service files. Use --direct, or install a newer release."
  if [ -n "$SUDO" ] && ! command -v sudo >/dev/null 2>&1; then die "Service mode needs sudo (or run this as root)."; fi
  say "Installing the heptix reader service (needs sudo) ..."
  remove_user_install

  as_root install -d -m 755 -o root -g root /usr/lib/heptix
  as_root install -m 755 -o root -g root "$STAGE/heptix" "$SYS_BIN"
  as_root ln -sf "$SYS_BIN" "$SYS_LINK"
  as_root install -d -m 755 /etc/sysusers.d /etc/systemd/system /etc/systemd/user
  as_root install -m 644 "$STAGE/packaging/sysusers/heptix.conf" "$SYSUSERS"
  as_root systemd-sysusers "$SYSUSERS"
  as_root install -m 644 "$STAGE/packaging/system/heptix-reader.service" "$READER_UNIT"
  as_root install -m 644 "$STAGE/packaging/user/heptix.service" "$CLIENT_UNIT"
  as_root systemctl daemon-reload
  as_root systemctl enable heptix-reader.service
  as_root systemctl restart heptix-reader.service

  ensure_audio_player

  if [ "$(id -u)" = 0 ]; then
    warn "Running as root: as your normal user, run: systemctl --user enable --now heptix.service"
  else
    run systemctl --user daemon-reload
    run systemctl --user enable heptix.service
    run systemctl --user restart heptix.service
  fi
  if [ "$DRY" != 1 ] && ! systemctl is-active --quiet heptix-reader.service; then
    warn "The reader service is not running. See: journalctl -u heptix-reader.service"
  fi

  offer_leave_input_group "Service mode does not need it."
  EDITOR_BIN="$SYS_LINK"
}

direct_install() {
  warn
  warn "Direct mode (not recommended): heptix will read /dev/input itself, so you need the"
  warn "'input' group. Any program you run can then read all keyboard input on this machine."
  warn "Service mode avoids this. Re-run with --service to use it."
  warn
  say "Installing heptix for your user (direct mode) ..."
  run mkdir -p "$BIN_DIR" "$UNIT_DIR"
  run install -m 755 "$STAGE/heptix" "$BIN_DIR/heptix"

  write_file "$UNIT_DIR/heptix.service" <<UNIT
[Unit]
Description=heptix - the missing haptic feedback for Linux
After=pipewire.service

[Service]
ExecStart=$BIN_DIR/heptix --direct
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

  run systemctl --user daemon-reload
  run systemctl --user enable heptix.service >/dev/null 2>&1

  ensure_audio_player

  # Reading /dev/input needs the 'input' group.
  if ! id -nG | tr ' ' '\n' | grep -qx input; then
    if [ -e /etc/NIXOS ]; then
      say
      say "Add yourself to the input group in your NixOS config, rebuild, then log out and in:"
      say "  users.users.$USER.extraGroups = [ \"input\" ];"
    else
      say
      if getent group input >/dev/null; then
        say "Adding $USER to the 'input' group (needs sudo) ..."
        if as_root usermod -aG input "$USER"; then
          say "Log out and back in for this to take effect."
        else
          warn "Could not add you to 'input'. Run: sudo usermod -aG input $USER"
        fi
      else
        warn "No 'input' group on this system; make sure your user can read /dev/input/event*."
      fi
    fi
    say "heptix will start automatically at your next login."
  else
    run systemctl --user restart heptix.service
    say "heptix is running."
  fi
  case ":$PATH:" in *":$BIN_DIR:"*) ;; *) say "Note: add $BIN_DIR to your PATH to run 'heptix' yourself." ;; esac
  EDITOR_BIN="$BIN_DIR/heptix"
}

uninstall() {
  found=0
  for f in "$UNIT_DIR/heptix.service" "$BIN_DIR/heptix" "$READER_UNIT" "$SYS_BIN"; do
    if [ -e "$f" ]; then found=1; fi
  done
  say "Removing heptix ..."
  run systemctl --user disable --now heptix.service 2>/dev/null || true
  run rm -f "$UNIT_DIR/heptix.service" "$BIN_DIR/heptix"
  if [ -e "$READER_UNIT" ] || [ -e "$SYS_BIN" ]; then
    as_root systemctl disable --now heptix-reader.service 2>/dev/null || true
    as_root rm -f "$READER_UNIT" "$CLIENT_UNIT" "$SYSUSERS" "$SYS_LINK" "$SYS_BIN"
    as_root rmdir /usr/lib/heptix 2>/dev/null || true
    as_root systemctl daemon-reload
    say "The 'heptix' system user was left in place; remove it with: sudo userdel heptix"
  fi
  run systemctl --user daemon-reload
  say "Your settings (~/.config/heptix) and stats (~/.local/state/heptix) were kept."
  if [ "$found" = 1 ]; then
    offer_leave_input_group "Nothing heptix installs needs it; only keep it if another program does."
  fi
}

if [ "$UNINSTALL" = 1 ]; then
  uninstall
  exit 0
fi

nixos_module_help() {
  cat <<'EOF'

On NixOS the reader service is installed through the flake's module (a script
cannot change a declarative system). Add this to your configuration and rebuild:

  inputs.heptix.url = "github:mustakimali/heptix";
  # in your configuration:
  imports = [ inputs.heptix.nixosModules.default ];
  services.heptix.enable = true;

(On a machine without a graphical session, add services.heptix.allowUids = [ 1000 ];
with your user id.) Then remove any per-user install with: install.sh --uninstall
EOF
}

if [ -e /etc/NIXOS ]; then
  case "$MODE" in
    service)
      warn "Service mode cannot be installed by this script on NixOS."
      nixos_module_help
      exit 1
      ;;
    direct) ;;
    *)
      if [ "$ASSUME_YES" = 1 ] || ! have_tty; then
        warn "NixOS: the reader service needs the NixOS module, so installing in direct mode for now."
        MODE=direct
      else
        tty_sane
        cat >/dev/tty <<'EOF'

This is NixOS, where a script cannot install the reader service. How should
heptix read your keyboard and touchpad?

  1) Direct mode (works now; not recommended)
     heptix reads /dev/input itself, which needs the 'input' group. Then ANY
     program you run can read all keyboard input on this machine, including
     what is typed at the login and lock screens.

  2) NixOS module (recommended)
     Add heptix to your NixOS configuration; it sets up the reader service, and
     you are not added to the 'input' group. Shows what to add.

EOF
        printf 'Choose [1]: ' >/dev/tty
        read_tty ans
        tty_restore
        [ -n "$ans" ] || ans=1
        case "$ans" in
          2) nixos_module_help; exit 0 ;;
          *) MODE=direct ;;
        esac
      fi
      ;;
  esac
fi

choose_mode
obtain
case "$MODE" in
  service) service_install ;;
  direct) direct_install ;;
  *) die "Unknown mode '$MODE' (use service or direct)." ;;
esac

# First run: open the settings editor (stdin is the script when piped from curl).
if [ "$DRY" != 1 ] && have_tty; then
  exec "$EDITOR_BIN" </dev/tty >/dev/tty
fi
say "Run 'heptix' to change settings."
