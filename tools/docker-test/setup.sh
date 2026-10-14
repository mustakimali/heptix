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

# Installs systemd and what the checks need, and creates the login user (uid 1000).
set -eu
if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y --no-install-recommends systemd systemd-sysv dbus dbus-user-session \
    sudo procps iproute2 python3 alsa-utils ca-certificates util-linux make
  rm -rf /var/lib/apt/lists/*
  userdel -r ubuntu 2>/dev/null || true   # Ubuntu images ship a user with uid 1000
elif command -v dnf >/dev/null 2>&1; then
  dnf install -y systemd dbus-broker sudo procps-ng iproute python3 alsa-utils util-linux shadow-utils passwd make
  dnf clean all
elif command -v pacman >/dev/null 2>&1; then
  pacman -Syu --noconfirm --needed systemd dbus sudo procps-ng iproute2 python alsa-utils util-linux shadow make
  rm -rf /var/cache/pacman/pkg/*
else
  echo "no supported package manager" >&2
  exit 1
fi
useradd -m -u 1000 -s /bin/bash tester
mkdir -p /etc/sudoers.d
echo 'tester ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/tester
