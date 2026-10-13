// SPDX-License-Identifier: GPL-2.0-only
// heptix - the missing haptic feedback for Linux!
// Copyright (C) 2026 M Mustakim Ali
//
// This program is free software; you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 2 as
// published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program; if not, see <https://www.gnu.org/licenses/>.

//! Text heatmaps for the Activity view and `heptix --stats`. Everything here
//! is a pure function from stats to lines, so it can be tested without a terminal.

use crate::stats::{civil_from_days, weekday, Counts, Metric, Stats};
use evdev::KeyCode;

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// 256-colour ramp: level 0 (empty) to 4 (most).
const GREENS: [u8; 5] = [238, 22, 28, 34, 46];
const GLYPHS: [&str; 5] = ["·", "░", "▒", "▓", "█"];

/// Groups digits: 48213 -> "48,213".
pub fn fmt_num(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Millimetres as a readable distance in `unit` ("km" or "mi").
pub fn fmt_distance(mm: u64, unit: &str) -> String {
    let m = mm as f64 / 1000.0;
    if unit == "mi" {
        let miles = m / 1609.344;
        if miles >= 0.1 {
            format!("{miles:.2} mi")
        } else {
            format!("{:.0} ft", m * 3.28084)
        }
    } else if m >= 1000.0 {
        format!("{:.2} km", m / 1000.0)
    } else if m >= 1.0 {
        format!("{m:.1} m")
    } else {
        format!("{:.0} cm", m * 100.0)
    }
}

fn fmt_metric(value: u32, metric: Metric, unit: &str) -> String {
    if metric == Metric::Distance {
        fmt_distance(value as u64, unit)
    } else {
        fmt_num(value as u64)
    }
}

pub fn fmt_day(day: i32) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{} {d} {} {y}", DAYS[weekday(day)], MONTHS[m as usize - 1])
}

/// Quartile cut-offs of the non-zero values, so one huge day does not wash out the rest.
fn thresholds(values: impl Iterator<Item = u32>) -> [u32; 3] {
    let mut v: Vec<u32> = values.filter(|x| *x > 0).collect();
    if v.is_empty() {
        return [0; 3];
    }
    v.sort_unstable();
    [v[v.len() / 4], v[v.len() / 2], v[v.len() * 3 / 4]]
}

fn level(x: u32, t: [u32; 3]) -> usize {
    if x == 0 {
        0
    } else if x < t[0] {
        1
    } else if x < t[1] {
        2
    } else if x < t[2] {
        3
    } else {
        4
    }
}

/// Two characters wide.
fn cell(level: usize, color: bool, selected: bool) -> String {
    match (color, selected) {
        (true, false) => format!("\x1b[38;5;{}m██\x1b[0m", GREENS[level]),
        (true, true) => format!("\x1b[48;5;{}m\x1b[1;97m[]\x1b[0m", GREENS[level]),
        (false, false) => format!("{0}{0}", GLYPHS[level]),
        (false, true) => "[]".into(),
    }
}

fn legend(color: bool) -> String {
    let cells: String = (0..5).map(|l| cell(l, color, false)).collect();
    format!("Less {cells} More")
}

/// GitHub-style contribution calendar: weeks as columns, Monday to Sunday as rows.
pub fn calendar(stats: &Stats, today: i32, weeks: usize, metric: Metric, cursor: Option<i32>, color: bool) -> Vec<String> {
    let weeks = weeks.max(1);
    let start = today - weekday(today) as i32 - 7 * (weeks as i32 - 1);
    let t = thresholds((start..=today).map(|d| stats.metric_day(d, metric)));

    let mut header = vec![' '; weeks * 2 + 3];
    let (mut last_end, mut last_month) = (0usize, 0u32);
    for c in 0..weeks {
        let (_, m, _) = civil_from_days(start + 7 * c as i32);
        if m != last_month && c * 2 >= last_end {
            for (k, ch) in MONTHS[m as usize - 1].chars().enumerate() {
                header[c * 2 + k] = ch;
            }
            last_end = c * 2 + 4;
            last_month = m;
        }
    }
    let mut lines = vec![format!("    {}", header.into_iter().collect::<String>().trim_end())];
    for (r, name) in DAYS.iter().enumerate() {
        let mut line = format!("{name} ");
        for c in 0..weeks {
            let day = start + 7 * c as i32 + r as i32;
            if day > today {
                line += "  ";
            } else {
                line += &cell(level(stats.metric_day(day, metric), t), color, cursor == Some(day));
            }
        }
        lines.push(line);
    }
    lines.push(format!("    {}", legend(color)));
    lines
}

/// Weekday by hour of day, summed over the last `weeks` weeks.
pub fn hour_grid(stats: &Stats, today: i32, weeks: usize, metric: Metric, color: bool) -> Vec<String> {
    let from = today - 7 * weeks as i32 + 1;
    let grid = stats.hour_grid(from, today, metric);
    let t = thresholds(grid.iter().flatten().copied());
    let mut header = String::from("    ");
    for h in (0..24).step_by(3) {
        header += &format!("{h:<6}");
    }
    let mut lines = vec![header.trim_end().to_string()];
    for (r, name) in DAYS.iter().enumerate() {
        let mut line = format!("{name} ");
        for n in grid[r] {
            line += &cell(level(n, t), color, false);
        }
        lines.push(line);
    }
    lines
}

/// Name of a key for display ("A", "Space", "L-Shift"...).
pub fn key_label(code: u16) -> String {
    let name = format!("{:?}", KeyCode::new(code));
    let Some(name) = name.strip_prefix("KEY_") else {
        return format!("#{code}");
    };
    let special = match name {
        "SPACE" => "Space",
        "ENTER" => "Enter",
        "KPENTER" => "KP Enter",
        "BACKSPACE" => "Bksp",
        "TAB" => "Tab",
        "CAPSLOCK" => "Caps",
        "ESC" => "Esc",
        "LEFTSHIFT" => "L-Shift",
        "RIGHTSHIFT" => "R-Shift",
        "LEFTCTRL" => "L-Ctrl",
        "RIGHTCTRL" => "R-Ctrl",
        "LEFTALT" => "L-Alt",
        "RIGHTALT" => "R-Alt",
        "LEFTMETA" => "L-Super",
        "RIGHTMETA" => "R-Super",
        "MINUS" => "-",
        "EQUAL" => "=",
        "LEFTBRACE" => "[",
        "RIGHTBRACE" => "]",
        "BACKSLASH" => "\\",
        "SEMICOLON" => ";",
        "APOSTROPHE" => "'",
        "GRAVE" => "`",
        "COMMA" => ",",
        "DOT" => ".",
        "SLASH" => "/",
        _ => "",
    };
    if !special.is_empty() {
        return special.to_string();
    }
    if name.len() <= 3 {
        return name.to_string();
    }
    let lower = name.to_lowercase();
    let mut chars = lower.chars();
    chars.next().map(|f| f.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn bar(n: u32, max: u32, width: usize) -> String {
    let filled = if max == 0 { 0 } else { (n as u64 * width as u64).div_ceil(max as u64) as usize };
    format!("{}{}", "█".repeat(filled.min(width)), " ".repeat(width - filled.min(width)))
}

/// Most and least pressed keys side by side.
pub fn key_bars(stats: &Stats, from: i32, to: i32) -> Vec<String> {
    let totals = stats.key_totals(from, to);
    if totals.is_empty() {
        return vec!["No key counts in this range yet.".into()];
    }
    let max = totals[0].1;
    let top_n = totals.len().div_ceil(2).min(8);
    let bot_n = (totals.len() - top_n).min(8);
    let entry = |(code, n): (u16, u32)| format!("{:<8} {} {:>8}", key_label(code), bar(n, max, 12), fmt_num(n as u64));
    let mut lines = vec![format!("{:<30}  {}", "Most pressed", "Least pressed")];
    for i in 0..top_n {
        let left = entry(totals[i]);
        let right = if i < bot_n { entry(totals[totals.len() - 1 - i]) } else { String::new() };
        lines.push(format!("{left}  {right}"));
    }
    lines
}

/// US-QWERTY keyboard (physical positions) coloured by how often each key was pressed.
pub fn keyboard(stats: &Stats, from: i32, to: i32, color: bool) -> Vec<String> {
    // (evdev code, label, width). Every row is 45 columns wide.
    type Row = &'static [(u16, &'static str, usize)];
    const ROWS: [Row; 5] = [
        &[(41, "`", 3), (2, "1", 3), (3, "2", 3), (4, "3", 3), (5, "4", 3), (6, "5", 3), (7, "6", 3), (8, "7", 3), (9, "8", 3), (10, "9", 3), (11, "0", 3), (12, "-", 3), (13, "=", 3), (14, "Bksp", 6)],
        &[(15, "Tab", 5), (16, "Q", 3), (17, "W", 3), (18, "E", 3), (19, "R", 3), (20, "T", 3), (21, "Y", 3), (22, "U", 3), (23, "I", 3), (24, "O", 3), (25, "P", 3), (26, "[", 3), (27, "]", 3), (43, "\\", 4)],
        &[(58, "Caps", 6), (30, "A", 3), (31, "S", 3), (32, "D", 3), (33, "F", 3), (34, "G", 3), (35, "H", 3), (36, "J", 3), (37, "K", 3), (38, "L", 3), (39, ";", 3), (40, "'", 3), (28, "Enter", 6)],
        &[(42, "Shift", 7), (44, "Z", 3), (45, "X", 3), (46, "C", 3), (47, "V", 3), (48, "B", 3), (49, "N", 3), (50, "M", 3), (51, ",", 3), (52, ".", 3), (53, "/", 3), (54, "Shift", 8)],
        &[(29, "Ctrl", 5), (125, "Sup", 4), (56, "Alt", 4), (57, "Space", 23), (100, "Alt", 4), (97, "Ctrl", 5)],
    ];
    let totals = stats.key_totals(from, to);
    let count = |code: u16| totals.iter().find(|(c, _)| *c == code).map_or(0, |(_, n)| *n);
    let t = thresholds(totals.iter().map(|(_, n)| *n));
    ROWS.iter()
        .map(|row| {
            row.iter()
                .map(|&(code, label, width)| {
                    let l = level(count(code), t);
                    let inner = width - 1;
                    if color {
                        let (bg, fg) = if l == 0 { (236, 245) } else if l >= 3 { (GREENS[l], 16) } else { (GREENS[l], 255) };
                        format!("\x1b[48;5;{bg}m\x1b[38;5;{fg}m{label:^inner$}\x1b[0m ")
                    } else {
                        let text = format!("{label}{}", GLYPHS[l]);
                        format!("{text:<width$}")
                    }
                })
                .collect::<String>()
        })
        .collect()
}

/// "busiest Tue 14:00-15:00" for the hour grid, if there is any activity.
pub fn peak_hour(stats: &Stats, today: i32, weeks: usize, metric: Metric, unit: &str) -> Option<String> {
    let grid = stats.hour_grid(today - 7 * weeks as i32 + 1, today, metric);
    let (mut best, mut at) = (0u32, (0usize, 0usize));
    for (r, row) in grid.iter().enumerate() {
        for (h, &n) in row.iter().enumerate() {
            if n > best {
                (best, at) = (n, (r, h));
            }
        }
    }
    (best > 0).then(|| format!("Busiest {} {:02}:00-{:02}:00 ({})", DAYS[at.0], at.1, (at.1 + 1) % 24, fmt_metric(best, metric, unit)))
}

const LABEL_W: usize = 16;
const NUM_W: usize = 9;
const DIST_W: usize = 11;
/// Width of the table: label column, four counters and the distance.
#[cfg(test)]
const TABLE_W: usize = LABEL_W + 4 * NUM_W + DIST_W;

fn first_of_month(day: i32) -> i32 {
    let (_, _, d) = civil_from_days(day);
    day - (d as i32 - 1)
}

/// Which column the metric highlights (None for "everything").
fn metric_column(metric: Metric) -> Option<usize> {
    match metric {
        Metric::Keys => Some(0),
        Metric::Taps => Some(1),
        Metric::Clicks => Some(2),
        Metric::Scrolls => Some(3),
        Metric::Distance => Some(4),
        Metric::All => None,
    }
}

fn table_row(label: &str, c: &Counts, unit: &str, highlight: Option<usize>, styled: bool) -> String {
    let cells = [
        (fmt_num(c.keys as u64), NUM_W),
        (fmt_num(c.taps as u64), NUM_W),
        (fmt_num(c.clicks as u64), NUM_W),
        (fmt_num(c.scrolls as u64), NUM_W),
        (fmt_distance(c.scroll_mm as u64, unit), DIST_W),
    ];
    let mut line = format!("{label:<LABEL_W$}");
    for (i, (text, w)) in cells.iter().enumerate() {
        let cell = format!("{text:>w$}", w = *w);
        line += &if styled && highlight == Some(i) { format!("\x1b[1m{cell}\x1b[0m") } else { cell };
    }
    line
}

/// Counters for the selected day, today, this week (Monday to today), this
/// month and all time, under an underlined header. The column for the
/// heatmap's metric is bold. Every line is `TABLE_W` columns wide.
pub fn stats_table(stats: &Stats, today: i32, selected: Option<i32>, show_month: bool, metric: Metric, unit: &str, styled: bool) -> Vec<String> {
    let highlight = metric_column(metric);
    let heads = ["Keys", "Taps", "Clicks", "Ticks", "Scrolled"];
    let widths = [NUM_W, NUM_W, NUM_W, NUM_W, DIST_W];
    let mut head = format!("{}{:<LABEL_W$}", if styled { "\x1b[4m" } else { "" }, "");
    for (i, (h, w)) in heads.iter().zip(widths).enumerate() {
        let cell = format!("{h:>w$}");
        head += &if styled && highlight == Some(i) { format!("\x1b[1m{cell}\x1b[22m") } else { cell };
    }
    if styled {
        head += "\x1b[0m";
    }

    let mut lines = vec![head];
    if let Some(day) = selected {
        let label = format!("▸ {}", fmt_day_short(day));
        lines.push(table_row(&label, &stats.day_counts(day), unit, highlight, styled));
    }
    lines.push(table_row("  Today", &stats.day_counts(today), unit, highlight, styled));
    let week_start = today - weekday(today) as i32;
    lines.push(table_row("  This week", &stats.range_counts(week_start, today), unit, highlight, styled));
    if show_month {
        lines.push(table_row("  This month", &stats.range_counts(first_of_month(today), today), unit, highlight, styled));
    }
    lines.push(table_row("  All time", &stats.lifetime, unit, highlight, styled));
    lines
}

/// "Sun 11 Oct" (no year), for the table's selected-day row.
fn fmt_day_short(day: i32) -> String {
    let (_, m, d) = civil_from_days(day);
    format!("{} {d} {}", DAYS[weekday(day)], MONTHS[m as usize - 1])
}

/// Busiest day, streak and when recording began, under the table.
pub fn table_footer(stats: &Stats, today: i32, from: i32, metric: Metric, unit: &str) -> String {
    let mut parts = Vec::new();
    if let Some((day, n)) = stats.busiest_day(from, today, metric) {
        parts.push(format!("busiest {} ({} {})", fmt_day(day), fmt_metric(n, metric, unit), metric.name().to_lowercase()));
    }
    let streak = stats.streak(today);
    if streak > 1 {
        parts.push(format!("{streak}-day streak"));
    }
    if let Some(since) = stats.since {
        let (y, m, d) = civil_from_days(since);
        parts.push(format!("since {d} {} {y}", MONTHS[m as usize - 1]));
    }
    if parts.is_empty() {
        "No activity recorded yet.".into()
    } else {
        let mut text = parts.join(" · ");
        if let Some(first) = text.get(..1) {
            text = first.to_uppercase() + &text[1..];
        }
        text
    }
}

/// Headline numbers for a range.
pub fn headline(stats: &Stats, today: i32, from: i32, metric: Metric, unit: &str) -> String {
    let counts: Counts = stats.range_counts(from, today);
    let total = metric.pick(&counts);
    let mut parts = vec![format!("{} {}", fmt_metric(total, metric, unit), metric.name().to_lowercase())];
    if let Some((day, n)) = stats.busiest_day(from, today, metric) {
        parts.push(format!("busiest {} ({})", fmt_day(day), fmt_metric(n, metric, unit)));
    }
    let streak = stats.streak(today);
    if streak > 1 {
        parts.push(format!("{streak}-day streak"));
    }
    parts.join(" · ")
}

/// Text report for `heptix --stats`.
pub fn summary(stats: &Stats, today: i32, unit: &str, color: bool, recording: bool) -> Vec<String> {
    let mut lines = vec![format!("heptix activity stats (recording is {})", if recording { "on" } else { "off" })];
    if stats.is_empty() && stats.lifetime.total() == 0 {
        lines.push("Nothing recorded yet.".into());
        return lines;
    }
    let from = today - 7 * 26 + 1;
    lines.push(String::new());
    lines.push(format!("Keystrokes, last 26 weeks: {}", headline(stats, today, from, Metric::Keys, unit)));
    lines.extend(calendar(stats, today, 26, Metric::Keys, None, color));
    lines.push(String::new());
    lines.extend(stats_table(stats, today, None, true, Metric::Keys, unit, color));
    lines.push(String::new());
    lines.extend(key_bars(stats, from, today));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{days_from_civil, Kind, Local};

    fn sample() -> (Stats, i32) {
        let today = days_from_civil(2026, 10, 11);
        let mut s = Stats::default();
        for d in 0..60 {
            for _ in 0..(d % 7 + 1) * 10 {
                s.record(Local { day: today - d, hour: (d % 24) as u8 }, Kind::Key(30 + (d % 5) as u16), true);
            }
        }
        s.record(Local { day: today, hour: 9 }, Kind::DistanceMm(1500), false);
        (s, today)
    }

    fn visible(s: &str) -> String {
        let mut out = String::new();
        let mut esc = false;
        for c in s.chars() {
            match (esc, c) {
                (false, '\x1b') => esc = true,
                (true, 'm') => esc = false,
                (false, c) => out.push(c),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn numbers_and_distances_format() {
        assert_eq!(fmt_num(0), "0");
        assert_eq!(fmt_num(999), "999");
        assert_eq!(fmt_num(48_213), "48,213");
        assert_eq!(fmt_num(1_234_567), "1,234,567");
        assert_eq!(fmt_distance(400, "km"), "40 cm");
        assert_eq!(fmt_distance(12_340, "km"), "12.3 m");
        assert_eq!(fmt_distance(1_234_000, "km"), "1.23 km");
        assert_eq!(fmt_distance(30_000, "mi"), "98 ft");
        assert_eq!(fmt_distance(1_609_344, "mi"), "1.00 mi");
    }

    #[test]
    fn calendar_has_header_seven_rows_and_legend_within_width() {
        let (s, today) = sample();
        for weeks in [4, 12, 26, 53] {
            let lines = calendar(&s, today, weeks, Metric::Keys, None, false);
            assert_eq!(lines.len(), 9, "{weeks}");
            assert!(lines[1].starts_with("Mon "));
            assert!(lines[7].starts_with("Sun "));
            for l in &lines[1..8] {
                assert_eq!(visible(l).chars().count(), 4 + weeks * 2, "{weeks}");
            }
        }
        let with_color = calendar(&s, today, 12, Metric::Keys, Some(today), true);
        assert!(with_color[7].contains("[]")); // today is a Sunday: selected cell
        assert!(visible(&with_color[0]).contains("Oct"));
    }

    #[test]
    fn empty_stats_render_without_panicking() {
        let s = Stats::default();
        let today = days_from_civil(2026, 10, 11);
        assert_eq!(calendar(&s, today, 26, Metric::All, None, true).len(), 9);
        assert_eq!(hour_grid(&s, today, 12, Metric::Keys, false).len(), 8);
        assert_eq!(keyboard(&s, today - 30, today, true).len(), 5);
        assert_eq!(key_bars(&s, today - 30, today).len(), 1);
        assert_eq!(summary(&s, today, "km", false, true).len(), 2);
    }

    #[test]
    fn thresholds_split_values_into_quartile_levels() {
        let t = thresholds([1, 2, 3, 4, 5, 6, 7, 8].into_iter());
        assert_eq!(level(0, t), 0);
        let levels: Vec<usize> = (1..=8).map(|x| level(x, t)).collect();
        assert_eq!(levels.iter().filter(|l| **l == 4).count(), 2);
        assert!(levels.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(level(5, thresholds([5].into_iter())), 4);
    }

    #[test]
    fn hour_grid_marks_the_active_hour() {
        let (s, today) = sample();
        let lines = hour_grid(&s, today, 12, Metric::Keys, false);
        assert!(lines[0].contains("21"));
        assert_eq!(lines.len(), 8);
        assert!(lines[1..].iter().all(|l| visible(l).chars().count() == 4 + 48));
    }

    #[test]
    fn key_labels_are_readable() {
        assert_eq!(key_label(KeyCode::KEY_A.0), "A");
        assert_eq!(key_label(KeyCode::KEY_SPACE.0), "Space");
        assert_eq!(key_label(KeyCode::KEY_LEFTSHIFT.0), "L-Shift");
        assert_eq!(key_label(KeyCode::KEY_F5.0), "F5");
        assert_eq!(key_label(KeyCode::KEY_HOME.0), "Home");
        assert_eq!(key_label(KeyCode::KEY_COMMA.0), ",");
        assert!(key_label(0x2ff).starts_with('#') || !key_label(0x2ff).is_empty());
    }

    #[test]
    fn key_bars_split_most_and_least_without_overlap() {
        let (s, today) = sample();
        let lines = key_bars(&s, today - 59, today);
        assert!(lines[0].contains("Most pressed") && lines[0].contains("Least pressed"));
        assert_eq!(lines.len(), 1 + 3); // 5 distinct keys: 3 most, 2 least
        let top = s.key_totals(today - 59, today)[0].0;
        assert!(lines[1].starts_with(&key_label(top)), "{lines:?}");
    }

    #[test]
    fn keyboard_has_five_rows_of_equal_width() {
        let (s, today) = sample();
        let lines = keyboard(&s, today - 59, today, false);
        assert_eq!(lines.len(), 5);
        assert!(lines.iter().all(|l| l.chars().count() == 45), "{lines:?}");
        let colored = keyboard(&s, today - 59, today, true);
        assert!(colored.iter().all(|l| visible(l).chars().count() == 45));
    }

    #[test]
    fn table_has_header_selected_today_week_month_and_all_time_rows_of_equal_width() {
        let (s, today) = sample();
        let lines = stats_table(&s, today, Some(today - 1), true, Metric::Keys, "km", true);
        assert_eq!(lines.len(), 6);
        assert!(visible(&lines[0]).trim_start().starts_with("Keys"));
        assert!(visible(&lines[0]).trim_end().ends_with("Scrolled"));
        assert!(visible(&lines[1]).starts_with("▸ Sat 10 Oct"));
        for (line, label) in lines[2..].iter().zip(["Today", "This week", "This month", "All time"]) {
            assert!(visible(line).contains(label), "{label}");
        }
        for l in &lines {
            assert_eq!(visible(l).chars().count(), TABLE_W, "{}", visible(l));
        }
        // Without the selected day or the month, down to three rows.
        assert_eq!(stats_table(&s, today, None, false, Metric::Keys, "km", true).len(), 4);
    }

    #[test]
    fn table_numbers_are_right() {
        let (s, today) = sample();
        let lines = stats_table(&s, today, Some(today), true, Metric::Keys, "km", true);
        let today_row = visible(&lines[2]);
        assert!(today_row.contains("10"), "{today_row}"); // sample: today (a Sunday) = (0 % 7 + 1) * 10 keys
        assert!(today_row.trim_end().ends_with("1.5 m"));
        // This week = Monday..today, this month = from the 1st, all time = lifetime.
        let week: u32 = (0..=6).map(|d| s.day_counts(today - d).keys).sum();
        assert!(visible(&lines[3]).contains(&fmt_num(week as u64)));
        let month: u32 = (0..=10).map(|d| s.day_counts(today - d).keys).sum();
        assert!(visible(&lines[4]).contains(&fmt_num(month as u64)));
        assert!(visible(&lines[5]).contains(&fmt_num(s.lifetime.keys as u64)));
        assert_eq!(first_of_month(today), today - 10);
    }

    #[test]
    fn table_highlights_the_metric_column_only() {
        let (s, today) = sample();
        let bold = |m| stats_table(&s, today, None, false, m, "km", true)[1].matches("\x1b[1m").count();
        assert_eq!(bold(Metric::Keys), 1);
        assert_eq!(bold(Metric::Distance), 1);
        assert_eq!(bold(Metric::All), 0);
        let keys_row = &stats_table(&s, today, None, false, Metric::Keys, "km", true)[1];
        let taps_row = &stats_table(&s, today, None, false, Metric::Taps, "km", true)[1];
        assert_ne!(keys_row, taps_row);
        // Plain output (piped, or NO_COLOR) carries no escape codes at all.
        let plain = stats_table(&s, today, Some(today), true, Metric::Keys, "km", false);
        assert!(plain.iter().all(|l| !l.contains('\x1b')));
        assert_eq!(plain.iter().map(|l| l.chars().count()).collect::<Vec<_>>(), vec![TABLE_W; plain.len()]);
    }

    #[test]
    fn table_footer_names_busiest_day_streak_and_start() {
        let (s, today) = sample();
        let text = table_footer(&s, today, today - 59, Metric::Keys, "km");
        assert!(text.starts_with("Busiest "), "{text}");
        assert!(text.contains("-day streak") && text.contains("since "), "{text}");
        assert!(text.chars().count() < 79, "fits an 80-column screen: {text}");
        assert_eq!(table_footer(&Stats::default(), today, today - 5, Metric::Keys, "km"), "No activity recorded yet.");
    }

    #[test]
    fn summary_lists_calendar_distance_and_keys() {
        let (s, today) = sample();
        let lines = summary(&s, today, "km", false, true);
        let text = lines.join("\n");
        assert!(text.contains("recording is on") && text.contains("Scrolled") && text.contains("Most pressed"));
        assert!(text.contains("All time") && text.contains("This week"));
    }
}
