#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
# Copyright (C) 2026 M Mustakim Ali
#
# Packs bucklespring's wav/ folder into assets/bucklespring.pack, which heptix
# embeds in the binary. Usage: pack_bucklespring.py <bucklespring/wav> <out>
#
# Format (little-endian): u16 count, then per sample:
#   u8 evdev keycode, u8 stroke (0 press, 1 release), u32 sample rate,
#   u32 frame count, frame count * i16 mono samples.
# Trailing near-silence is trimmed to keep the binary small.
import glob, os, re, struct, sys, wave

THRESHOLD = 60  # ~ -55 dBFS
KEEP = 0.004    # seconds of tail kept after the last loud sample

def load(path):
    w = wave.open(path)
    assert w.getsampwidth() == 2, path
    ch, rate = w.getnchannels(), w.getframerate()
    raw = struct.unpack("<%dh" % (w.getnframes() * ch), w.readframes(w.getnframes()))
    mono = [sum(raw[i:i + ch]) // ch for i in range(0, len(raw), ch)]
    last = max((i for i, s in enumerate(mono) if abs(s) > THRESHOLD), default=0)
    return rate, mono[: min(len(mono), last + int(rate * KEEP))]

def main():
    src, out = sys.argv[1:3]
    entries = []
    for path in sorted(glob.glob(os.path.join(src, "*.wav"))):
        m = re.fullmatch(r"([0-9a-f]{2})-([01])\.wav", os.path.basename(path))
        if m:
            rate, mono = load(path)
            entries.append((int(m[1], 16), int(m[2]), rate, mono))
    blob = struct.pack("<H", len(entries))
    for code, stroke, rate, mono in entries:
        blob += struct.pack("<BBII", code, stroke, rate, len(mono))
        blob += struct.pack("<%dh" % len(mono), *mono)
    open(out, "wb").write(blob)
    print(len(entries), "samples,", len(blob), "bytes")

main()
