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
#   tools/docker-test/run.sh target/x86_64-unknown-linux-musl/release/heptix
#
# The container gets SYS_ADMIN (systemd as PID 1) and SYS_PTRACE (the reader needs it
# in its bounding set, and Docker drops it by default), and no seccomp/AppArmor profile; it
# is not privileged, so it sees none of your input devices.
set -eu
BIN="${1:?usage: run.sh <path to a static heptix binary>}"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
NAME=heptix-install-test
IMAGE=heptix-install-test

stage=$(mktemp -d)
trap 'docker rm -f "$NAME" >/dev/null 2>&1 || true; rm -rf "$stage"' EXIT
cp "$BIN" "$stage/heptix"
cp -r "$ROOT/packaging" "$stage/packaging"
cp "$ROOT/install.sh" "$HERE/checks.sh" "$stage/"
chmod -R a+rX "$stage"

docker build -q -t "$IMAGE" "$HERE" >/dev/null
docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" --cgroupns=host \
  --cap-add SYS_ADMIN --cap-add SYS_PTRACE --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
  --tmpfs /run --tmpfs /run/lock --tmpfs /tmp \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw -v "$stage":/stage:ro \
  "$IMAGE" >/dev/null
docker exec "$NAME" sh /stage/checks.sh
