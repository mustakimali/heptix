#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
# Copyright (C) 2026 M Mustakim Ali
#
# Renders screenshots of the heptix settings editor for the README.
#
# It runs the real binary in a pseudo-terminal with made-up activity data (never
# your own stats), captures the final frame of each view, draws it as SVG and,
# if `resvg` is installed, converts that to PNG. Usage:
#
#   cargo build --release
#   nix shell nixpkgs#resvg nixpkgs#dejavu_fonts -c \
#       python3 tools/render_screenshots.py target/release/heptix docs
#
# (or any environment with resvg and the DejaVu Sans Mono font). The made-up data
# is random on every run; set HEPTIX_SHOT_SEED=<number> to repeat a picture.
import datetime, fcntl, html, os, pty, random, re, select, shutil, struct, subprocess, sys, tempfile, termios, time

COLS, ROWS = 82, 24
CELL_W, CELL_H, PAD, TITLE_H = 9.6, 20, 18, 36
FG, BG = "#d4d4d4", "#1e1e1e"
FONT = "DejaVu Sans Mono, Menlo, Consolas, Liberation Mono, monospace"
BASE16 = ["#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
          "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff"]
BLOCKS = {"█": 1.0, "▓": 0.75, "▒": 0.5, "░": 0.25}

SCREENS = [  # name, keys to press (each followed by a pause)
    ("settings", []),
    ("activity", ["\t"]),
    ("hours", ["\t", "\t"]),
    ("keys", ["\t", "\t", "\t"]),
]


def palette(n):
    if n < 16:
        return BASE16[n]
    if n < 232:
        n -= 16
        levels = [0, 95, 135, 175, 215, 255]
        return "#%02x%02x%02x" % (levels[n // 36], levels[n // 6 % 6], levels[n % 6])
    v = 8 + 10 * (n - 232)
    return "#%02x%02x%02x" % (v, v, v)


class Cell:
    __slots__ = ("ch", "fg", "bg", "bold", "dim", "ul", "rev")

    def __init__(self, ch=" ", fg=None, bg=None, bold=False, dim=False, ul=False, rev=False):
        self.ch, self.fg, self.bg, self.bold, self.dim, self.ul, self.rev = ch, fg, bg, bold, dim, ul, rev


class Screen:
    """Just enough of a terminal for the escape codes heptix writes."""

    def __init__(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.grid = [[Cell() for _ in range(cols)] for _ in range(rows)]
        self.r = self.c = 0
        self.style = dict(fg=None, bg=None, bold=False, dim=False, ul=False, rev=False)

    def sgr(self, params):
        p = [int(x) if x else 0 for x in params.split(";")] if params else [0]
        i = 0
        while i < len(p):
            n = p[i]
            if n == 0:
                self.style = dict(fg=None, bg=None, bold=False, dim=False, ul=False, rev=False)
            elif n == 1:
                self.style["bold"] = True
            elif n == 2:
                self.style["dim"] = True
            elif n == 22:
                self.style["bold"] = self.style["dim"] = False
            elif n == 4:
                self.style["ul"] = True
            elif n == 24:
                self.style["ul"] = False
            elif n == 7:
                self.style["rev"] = True
            elif n == 27:
                self.style["rev"] = False
            elif 30 <= n <= 37:
                self.style["fg"] = palette(n - 30)
            elif 90 <= n <= 97:
                self.style["fg"] = palette(n - 90 + 8)
            elif n == 39:
                self.style["fg"] = None
            elif n == 49:
                self.style["bg"] = None
            elif n in (38, 48) and i + 2 < len(p) and p[i + 1] == 5:
                self.style["fg" if n == 38 else "bg"] = palette(p[i + 2])
                i += 2
            i += 1

    def put(self, ch):
        if self.c >= self.cols:
            self.c = 0
            self.newline()
        self.grid[self.r][self.c] = Cell(ch, **self.style)
        self.c += 1

    def newline(self):
        self.r += 1
        if self.r >= self.rows:
            self.grid.pop(0)
            self.grid.append([Cell() for _ in range(self.cols)])
            self.r = self.rows - 1

    def feed(self, text):
        i = 0
        while i < len(text):
            ch = text[i]
            if ch == "\x1b":
                m = re.match(r"\[([0-9;?]*)([A-Za-z])", text[i + 1:])
                if not m:
                    i += 2
                    continue
                params, cmd = m.groups()
                if cmd == "m":
                    self.sgr(params)
                elif cmd == "H":
                    self.r = self.c = 0
                elif cmd == "J" and params == "2":
                    self.grid = [[Cell() for _ in range(self.cols)] for _ in range(self.rows)]
                i += 1 + m.end()
            elif ch == "\r":
                self.c = 0
                i += 1
            elif ch == "\n":
                self.newline()
                i += 1
            else:
                self.put(ch)
                i += 1

    def used_rows(self):
        last = 0
        for r, row in enumerate(self.grid):
            if any(c.ch != " " or c.bg or c.rev for c in row):
                last = r + 1
        return last


def run_view(binary, state_dir, config, keys):
    pid, fd = pty.fork()
    if pid == 0:
        env = dict(os.environ, XDG_STATE_HOME=state_dir, TERM="xterm-256color", LANG="C.UTF-8")
        env.pop("NO_COLOR", None)
        os.execve(binary, [binary, "--tui", "--config", config], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

    def read(t):
        out, end = b"", time.time() + t
        while time.time() < end:
            if select.select([fd], [], [], 0.1)[0]:
                try:
                    out += os.read(fd, 65536)
                except OSError:
                    break
        return out.decode("utf8", "replace")

    data = read(1.2)
    for k in keys:
        os.write(fd, k.encode())
        data += read(0.7)
    os.write(fd, b"qq")
    read(0.3)
    os.waitpid(pid, 0)
    return data.split("\x1b[2J")[-1]


def to_svg(screen):
    rows = max(screen.used_rows(), 1)
    w = COLS * CELL_W + 2 * PAD
    h = TITLE_H + rows * CELL_H + 2 * PAD
    o = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0f}" height="{h:.0f}" viewBox="0 0 {w:.0f} {h:.0f}">']
    o.append(f'<rect width="{w:.0f}" height="{h:.0f}" rx="10" fill="{BG}" stroke="#3a3a3a"/>')
    for i, c in enumerate(("#ff5f56", "#ffbd2e", "#27c93f")):
        o.append(f'<circle cx="{20 + i * 20}" cy="{TITLE_H / 2:.0f}" r="6" fill="{c}"/>')
    o.append(f'<text x="{w / 2:.0f}" y="{TITLE_H / 2 + 5:.0f}" fill="#8a8a8a" font-family="{FONT}" font-size="13" text-anchor="middle">heptix</text>')
    for r in range(rows):
        y0 = TITLE_H + PAD + r * CELL_H
        row = screen.grid[r]
        resolved = []
        for c in row:
            fg, bg = c.fg or FG, c.bg
            if c.rev:
                fg, bg = (c.bg or BG), (c.fg or FG)
            resolved.append((fg, bg))
        # backgrounds
        x = 0
        while x < COLS:
            bg = resolved[x][1]
            if bg is None:
                x += 1
                continue
            e = x
            while e < COLS and resolved[e][1] == bg:
                e += 1
            o.append(f'<rect x="{PAD + x * CELL_W:.1f}" y="{y0}" width="{(e - x) * CELL_W:.1f}" height="{CELL_H}" fill="{bg}"/>')
            x = e
        # block characters as rectangles (crisp, font independent)
        for x, c in enumerate(row):
            if c.ch in BLOCKS:
                o.append(f'<rect x="{PAD + x * CELL_W:.1f}" y="{y0 + 1}" width="{CELL_W:.1f}" height="{CELL_H - 2}" fill="{resolved[x][0]}" fill-opacity="{BLOCKS[c.ch] * (0.6 if c.dim else 1):.2f}"/>')
        # text runs
        x = 0
        while x < COLS:
            c = row[x]
            if c.ch in BLOCKS or c.ch == " ":
                x += 1
                continue
            key = (resolved[x][0], c.bold, c.dim, c.ul)
            e = x
            while e < COLS and row[e].ch not in BLOCKS and (resolved[e][0], row[e].bold, row[e].dim, row[e].ul) == key:
                e += 1
            text = "".join(cc.ch for cc in row[x:e]).rstrip()
            n = len(text)
            if n:
                attrs = f'fill="{key[0]}"' + (' font-weight="bold"' if key[1] else "") + (' fill-opacity="0.6"' if key[2] else "")
                o.append(
                    f'<text x="{PAD + x * CELL_W:.1f}" y="{y0 + 15}" {attrs} font-family="{FONT}" font-size="16" '
                    f'xml:space="preserve" textLength="{n * CELL_W:.1f}" lengthAdjust="spacing">{html.escape(text)}</text>'
                )
                if key[3]:
                    o.append(f'<rect x="{PAD + x * CELL_W:.1f}" y="{y0 + 18}" width="{n * CELL_W:.1f}" height="1" fill="{key[0]}"/>')
            x = e
    o.append("</svg>")
    return "\n".join(o)


def make_stats(path, seed):
    """Made-up activity: weekday-heavy typing, some taps, clicks and scrolling."""
    rnd = random.Random(seed)
    today = datetime.date.today()
    keycodes = [57] * 10 + [18] * 7 + [30, 20, 24, 23, 49, 31, 19, 32, 38, 35, 28, 14] * 3 + [44, 45, 47, 16, 17, 2, 3, 41, 1, 54, 42]
    lines, perkey, total = [], {}, [0] * 5
    for d in range(230):
        day = today - datetime.timedelta(days=d)
        wd = day.weekday()
        base = 0 if (wd >= 5 and rnd.random() < 0.5) else rnd.randint(300, 3200)
        for h in range(24):
            if d == 0 and h > datetime.datetime.now().hour:
                continue
            w = 1.0 if 9 <= h <= 18 else (0.3 if 19 <= h <= 23 else 0.02)
            if wd >= 5:
                w *= 0.5
            k = int(base * w / 6 * rnd.random() * 2)
            if k == 0:
                continue
            taps, clicks, ticks = int(k * 0.08), int(k * 0.03), int(k * 0.1)
            mm = int(ticks * rnd.randint(8, 30))
            lines.append(f"h {day.isoformat()}T{h:02d} {k} {taps} {clicks} {ticks} {mm}")
            for i, v in enumerate((k, taps, clicks, ticks, mm)):
                total[i] += v
            for _ in range(min(k, 40)):
                key = (day.isoformat(), rnd.choice(keycodes))
                perkey[key] = perkey.get(key, 0) + max(1, k // 40)
    lines += [f"k {d} {c} {n}" for (d, c), n in perkey.items()]
    lines.append("t " + " ".join(map(str, total)))
    lines.append("s " + (today - datetime.timedelta(days=229)).isoformat())
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    os.chmod(path, 0o600)


def main():
    binary, out = os.path.abspath(sys.argv[1]), sys.argv[2]
    zoom = sys.argv[3] if len(sys.argv) > 3 else "2"
    os.makedirs(out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="heptix-shots-")
    # A fresh random pattern each run; set HEPTIX_SHOT_SEED to reproduce one.
    seed = int(os.environ.get("HEPTIX_SHOT_SEED") or time.time_ns() % 10**9)
    print("seed:", seed)
    make_stats(os.path.join(work, "heptix", "stats.tsv"), seed)
    config = os.path.join(work, "config.toml")
    # Let the binary write its default config first, so no "created config" notice shows.
    subprocess.run([binary, "--list", "--config", config], env=dict(os.environ, XDG_STATE_HOME=work), capture_output=True)
    resvg = shutil.which("resvg")
    fonts = None
    if resvg:
        found = subprocess.run(["fc-match", "-f", "%{file}", "DejaVu Sans Mono"], capture_output=True, text=True).stdout
        fonts = os.path.dirname(found) if found else None
    for name, keys in SCREENS:
        frame = run_view(binary, work, config, keys)
        screen = Screen(ROWS, COLS)
        screen.feed(frame.replace(config, "~/.config/heptix/config.toml"))
        svg_path = os.path.join(out, f"{name}.svg")
        with open(svg_path, "w") as f:
            f.write(to_svg(screen))
        if resvg:
            cmd = [resvg, "--zoom", zoom, "--font-family", "DejaVu Sans Mono", "--monospace-family", "DejaVu Sans Mono"]
            if fonts:
                cmd += ["--use-fonts-dir", fonts]
            subprocess.run(cmd + [svg_path, os.path.join(out, f"{name}.png")], check=True)
            os.remove(svg_path)
            print("wrote", os.path.join(out, f"{name}.png"))
        else:
            print("wrote", svg_path, "(install resvg to get PNGs)")
    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
