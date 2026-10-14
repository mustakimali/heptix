#!/usr/bin/env python3
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

# A virtual keyboard for the end-to-end test (needs /dev/uinput). It declares the
# keys that make heptix treat it as a keyboard (letters, space, enter) but only
# ever presses F24, which nothing uses, so it is harmless on a machine with a desktop.
#
#   uinput_key.py <name>   creates the device, prints its event number on the first
#                          line, then reads commands on stdin: "press N" (N presses
#                          of F24) or "quit".
import fcntl, os, struct, sys, time

UI_SET_EVBIT = 0x40045564
UI_SET_KEYBIT = 0x40045565
UI_DEV_SETUP = 0x405C5503
UI_DEV_CREATE = 0x5501
UI_DEV_DESTROY = 0x5502
UI_GET_SYSNAME = 0x8040552C
EV_SYN, EV_KEY = 0, 1
KEY_A, KEY_Z, KEY_SPACE, KEY_ENTER, KEY_F24 = 30, 44, 57, 28, 194


def emit(fd, etype, code, value):
    os.write(fd, struct.pack("llHHi", 0, 0, etype, code, value))


def main():
    name = (sys.argv[1] if len(sys.argv) > 1 else "heptix-test-keyboard").encode()
    fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_SYN)
    for key in (KEY_A, KEY_Z, KEY_SPACE, KEY_ENTER, KEY_F24):
        fcntl.ioctl(fd, UI_SET_KEYBIT, key)
    # struct uinput_setup: input_id (bustype, vendor, product, version), name[80], ff_effects_max
    setup = struct.pack("HHHH80sI", 0x03, 0x1234, 0x5678, 1, name, 0)
    fcntl.ioctl(fd, UI_DEV_SETUP, setup)
    fcntl.ioctl(fd, UI_DEV_CREATE)
    sysname = fcntl.ioctl(fd, UI_GET_SYSNAME, bytes(64)).split(b"\0")[0].decode()
    time.sleep(0.5)
    event = next(e for e in os.listdir(f"/sys/devices/virtual/input/{sysname}") if e.startswith("event"))
    print(event[len("event"):], flush=True)
    try:
        for line in sys.stdin:
            parts = line.split()
            if not parts or parts[0] == "quit":
                break
            if parts[0] == "press":
                for _ in range(int(parts[1])):
                    emit(fd, EV_KEY, KEY_F24, 1)
                    emit(fd, EV_SYN, 0, 0)
                    time.sleep(0.06)
                    emit(fd, EV_KEY, KEY_F24, 0)
                    emit(fd, EV_SYN, 0, 0)
                    time.sleep(0.06)
                print("pressed", flush=True)
    finally:
        fcntl.ioctl(fd, UI_DEV_DESTROY)
        os.close(fd)


main()
