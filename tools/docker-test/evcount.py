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

# Counts key-down events of one key on an input device, independently of heptix, so the
# end-to-end test can tell a loss inside heptix from one in the kernel or the test setup.
#   evcount.py <device> <keycode> <seconds>     prints "<count>" when the time is up
import os, select, struct, sys, time

device, code, seconds = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
fd = os.open(device, os.O_RDONLY | os.O_NONBLOCK)
size = struct.calcsize("llHHi")
count = dropped = 0
end = time.time() + seconds
while time.time() < end:
    if select.select([fd], [], [], 0.1)[0]:
        try:
            data = os.read(fd, size * 64)
        except BlockingIOError:
            continue
        for i in range(0, len(data) - size + 1, size):
            _, _, etype, ecode, value = struct.unpack("llHHi", data[i : i + size])
            if etype == 1 and ecode == code and value == 1:
                count += 1
            if etype == 0 and ecode == 3:  # SYN_DROPPED
                dropped += 1
print(count, dropped, flush=True)
