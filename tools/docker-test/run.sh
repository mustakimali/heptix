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

# Tests install.sh (service mode, then direct mode, then uninstall) inside a
# container that runs real systemd. Needs docker and a static heptix binary:
#
#   rustup target add x86_64-unknown-linux-musl
#   cargo build --release --target x86_64-unknown-linux-musl
#   tools/docker-test/run.sh target/x86_64-unknown-linux-musl/release/heptix [distro]
#
# distro: debian (default), ubuntu, fedora, arch, or alpine (no systemd: checks
# that the installer refuses cleanly).
#
# The container gets SYS_ADMIN (systemd as PID 1), SYS_PTRACE (the reader needs it in
# its bounding set) and NET_ADMIN (systemd 255+ silently skips PrivateNetwork=yes
# without it), which Docker drops by default, and no seccomp/AppArmor profile; it
# is not privileged, so it sees none of your input devices.
set -eu
BIN="${1:?usage: run.sh <path to a static heptix binary> [debian|ubuntu|fedora|arch|alpine]}"
DISTRO="${2:-debian}"
case "$DISTRO" in
  debian) BASE=debian:12 ;;
  ubuntu) BASE=ubuntu:24.04 ;;
  fedora) BASE=fedora:latest ;;
  arch) BASE=archlinux:latest ;;
  alpine) BASE=alpine:latest ;;
  *) echo "unknown distro: $DISTRO" >&2; exit 2 ;;
esac
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
NAME="heptix-install-test-$DISTRO"
IMAGE="heptix-install-test-$DISTRO"

stage=$(mktemp -d)
trap 'docker rm -f "$NAME" >/dev/null 2>&1 || true; rm -rf "$stage"' EXIT
cp "$BIN" "$stage/heptix"
cp -r "$ROOT/packaging" "$stage/packaging"
mkdir "$stage/tools"
cp "$ROOT/tools/test-signature.sh" "$stage/tools/"
cp "$ROOT/install.sh" "$ROOT/Makefile" "$HERE/checks.sh" "$HERE/uinput_key.py" "$HERE/evcount.py" "$stage/"
chmod -R a+rX "$stage"

if [ "$DISTRO" = alpine ]; then
  # No systemd here: the installer must say so and change nothing.
  out=$(docker run --rm -v "$stage":/stage:ro "$BASE" sh -c 'cd /tmp && HEPTIX_LOCAL_DIR=/stage sh /stage/install.sh --service --yes' 2>&1) && rc=0 || rc=$?
  echo "$out"
  if [ "$rc" -ne 0 ] && echo "$out" | grep -q 'systemd is required'; then
    echo "ok   - the installer refuses cleanly without systemd"
    exit 0
  fi
  echo "FAIL - expected a clean refusal (exit $rc)"
  exit 1
fi

docker build -q --build-arg BASE="$BASE" -t "$IMAGE" "$HERE" >/dev/null
docker rm -f "$NAME" >/dev/null 2>&1 || true

set -- --cgroupns=host \
  --cap-add SYS_ADMIN --cap-add SYS_PTRACE --cap-add NET_ADMIN --cap-add AUDIT_CONTROL \
  --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
  --tmpfs /run --tmpfs /run/lock --tmpfs /tmp \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw -v "$stage":/stage:ro
# A virtual keyboard (/dev/uinput) lets the checks send real input events through
# the reader. It only ever presses F24. Set HEPTIX_NO_UINPUT=1 to skip it.
if [ -e /dev/uinput ] && [ -z "${HEPTIX_NO_UINPUT:-}" ]; then
  set -- "$@" --device /dev/uinput --device-cgroup-rule 'c 13:* rmw'
fi
# Extra docker arguments, for example HEPTIX_DOCKER_ARGS="--cpus 0.3" to imitate a slow machine.
# shellcheck disable=SC2086
docker run -d --name "$NAME" "$@" ${HEPTIX_DOCKER_ARGS:-} "$IMAGE" >/dev/null
docker exec "$NAME" sh /stage/checks.sh
