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

//! Activity counters: how much you type, tap and scroll.
//!
//! Only counters are kept: totals per hour, and per key per day. Never which
//! keys came in what order, never the time between keys, never any text. The
//! file lives in the user's state directory with mode 0600 and is written only
//! by the background service; the settings editor just reads it.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Data older than this (53 weeks) is dropped on save.
pub const RETENTION_DAYS: i32 = 371;

/// How often changed counters are written to disk.
const SAVE_EVERY: Duration = Duration::from_secs(30);
/// How often the service checks whether the file was wiped behind its back.
const WIPE_CHECK_EVERY: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Counts {
    pub keys: u32,
    pub taps: u32,
    pub clicks: u32,
    /// Scroll ticks (detents).
    pub scrolls: u32,
    /// Finger travel during two-finger scrolling, in millimetres.
    pub scroll_mm: u32,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.keys = self.keys.saturating_add(o.keys);
        self.taps = self.taps.saturating_add(o.taps);
        self.clicks = self.clicks.saturating_add(o.clicks);
        self.scrolls = self.scrolls.saturating_add(o.scrolls);
        self.scroll_mm = self.scroll_mm.saturating_add(o.scroll_mm);
    }

    /// Everything that counts as an action (distance is a length, not an action).
    pub fn total(&self) -> u32 {
        self.keys.saturating_add(self.taps).saturating_add(self.clicks).saturating_add(self.scrolls)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Metric {
    Keys,
    Taps,
    Clicks,
    Scrolls,
    /// Finger travel while scrolling.
    Distance,
    All,
}

impl Metric {
    pub fn name(self) -> &'static str {
        match self {
            Metric::Keys => "Keystrokes",
            Metric::Taps => "Touchpad taps",
            Metric::Clicks => "Touchpad clicks",
            Metric::Scrolls => "Scroll ticks",
            Metric::Distance => "Scroll distance",
            Metric::All => "Everything",
        }
    }

    pub fn pick(self, c: &Counts) -> u32 {
        match self {
            Metric::Keys => c.keys,
            Metric::Taps => c.taps,
            Metric::Clicks => c.clicks,
            Metric::Scrolls => c.scrolls,
            Metric::Distance => c.scroll_mm,
            Metric::All => c.total(),
        }
    }

    pub fn next(self) -> Metric {
        match self {
            Metric::Keys => Metric::Taps,
            Metric::Taps => Metric::Clicks,
            Metric::Clicks => Metric::Scrolls,
            Metric::Scrolls => Metric::Distance,
            Metric::Distance => Metric::All,
            Metric::All => Metric::Keys,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Key(u16),
    Tap,
    Click,
    Scroll,
    /// Finger travel in whole millimetres.
    DistanceMm(u32),
}

/// A local date (days since 1970-01-01) and hour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Local {
    pub day: i32,
    pub hour: u8,
}

impl Local {
    pub fn now() -> Local {
        unsafe {
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            Local {
                day: days_from_civil(tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32),
                hour: tm.tm_hour as u8,
            }
        }
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400) as u32;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i32 - 719_468
}

/// Inverse of `days_from_civil`: (year, month, day).
pub fn civil_from_days(z: i32) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0 = Monday ... 6 = Sunday.
pub fn weekday(day: i32) -> usize {
    (day + 3).rem_euclid(7) as usize
}

fn date_string(day: i32) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_date(s: &str) -> Option<i32> {
    let mut it = s.split('-');
    let (y, m, d) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

#[derive(Default, Clone, PartialEq, Debug)]
pub struct Stats {
    /// Totals per (day, hour).
    pub hours: BTreeMap<(i32, u8), Counts>,
    /// Presses per (day, key code).
    pub keys: BTreeMap<(i32, u16), u32>,
    /// Running totals since stats began; unaffected by pruning.
    pub lifetime: Counts,
    /// First day anything was recorded.
    pub since: Option<i32>,
}

impl Stats {
    pub fn record(&mut self, at: Local, kind: Kind, per_key: bool) {
        let mut delta = Counts::default();
        match kind {
            Kind::Key(code) => {
                delta.keys = 1;
                if per_key {
                    let n = self.keys.entry((at.day, code)).or_default();
                    *n = n.saturating_add(1);
                }
            }
            Kind::Tap => delta.taps = 1,
            Kind::Click => delta.clicks = 1,
            Kind::Scroll => delta.scrolls = 1,
            Kind::DistanceMm(mm) => delta.scroll_mm = mm,
        }
        self.hours.entry((at.day, at.hour)).or_default().add(&delta);
        self.lifetime.add(&delta);
        self.since.get_or_insert(at.day);
    }

    /// Drops everything older than the retention window.
    pub fn prune(&mut self, today: i32) {
        let cutoff = today - RETENTION_DAYS;
        self.hours = self.hours.split_off(&(cutoff, 0));
        self.keys = self.keys.split_off(&(cutoff, 0));
    }

    pub fn is_empty(&self) -> bool {
        self.hours.is_empty() && self.keys.is_empty()
    }

    pub fn range_counts(&self, from: i32, to: i32) -> Counts {
        let mut sum = Counts::default();
        if from <= to {
            for c in self.hours.range((from, 0)..=(to, 23)).map(|(_, c)| c) {
                sum.add(c);
            }
        }
        sum
    }

    pub fn day_counts(&self, day: i32) -> Counts {
        self.range_counts(day, day)
    }

    pub fn metric_day(&self, day: i32, metric: Metric) -> u32 {
        metric.pick(&self.day_counts(day))
    }

    /// Weekday (Mon=0) by hour of day, summed over `from..=to`.
    pub fn hour_grid(&self, from: i32, to: i32, metric: Metric) -> [[u32; 24]; 7] {
        let mut grid = [[0u32; 24]; 7];
        if from <= to {
            for (&(day, hour), c) in self.hours.range((from, 0)..=(to, 23)) {
                let cell = &mut grid[weekday(day)][hour as usize % 24];
                *cell = cell.saturating_add(metric.pick(c));
            }
        }
        grid
    }

    /// Presses per key over `from..=to`, most pressed first.
    pub fn key_totals(&self, from: i32, to: i32) -> Vec<(u16, u32)> {
        let mut totals: BTreeMap<u16, u32> = BTreeMap::new();
        if from <= to {
            for (&(_, code), n) in self.keys.range((from, 0)..=(to, u16::MAX)) {
                let t = totals.entry(code).or_default();
                *t = t.saturating_add(*n);
            }
        }
        let mut v: Vec<(u16, u32)> = totals.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    /// Consecutive days with any activity, ending today (or yesterday if today is still empty).
    pub fn streak(&self, today: i32) -> u32 {
        let mut day = if self.day_counts(today).total() > 0 { today } else { today - 1 };
        let mut n = 0;
        while self.day_counts(day).total() > 0 {
            n += 1;
            day -= 1;
        }
        n
    }

    /// The day with the highest value in `from..=to`.
    pub fn busiest_day(&self, from: i32, to: i32, metric: Metric) -> Option<(i32, u32)> {
        (from..=to).map(|d| (d, self.metric_day(d, metric))).filter(|(_, n)| *n > 0).max_by_key(|(d, n)| (*n, -*d))
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# heptix activity stats: counters only (totals per hour, presses per key per day).\n\
             # h <date>T<hour> <keys> <taps> <clicks> <scroll ticks> <scroll mm>\n\
             # k <date> <keycode> <count>\n\
             # t <keys> <taps> <clicks> <scroll ticks> <scroll mm>   (all-time totals)\n\
             # s <date>   (first day recorded)\n",
        );
        let l = &self.lifetime;
        out += &format!("t {} {} {} {} {}\n", l.keys, l.taps, l.clicks, l.scrolls, l.scroll_mm);
        if let Some(day) = self.since {
            out += &format!("s {}\n", date_string(day));
        }
        for (&(day, hour), c) in &self.hours {
            out += &format!(
                "h {}T{hour:02} {} {} {} {} {}\n",
                date_string(day),
                c.keys,
                c.taps,
                c.clicks,
                c.scrolls,
                c.scroll_mm
            );
        }
        for (&(day, code), n) in &self.keys {
            out += &format!("k {} {code} {n}\n", date_string(day));
        }
        out
    }

    /// Parses `to_text` output. Returns the stats and how many lines were skipped.
    pub fn parse(text: &str) -> (Stats, usize) {
        let mut s = Stats::default();
        let mut skipped = 0;
        let mut has_totals = false;
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            has_totals |= line.starts_with("t ");
            if s.parse_line(line).is_none() {
                skipped += 1;
            }
        }
        if !has_totals {
            s.lifetime = s.range_counts(i32::MIN, i32::MAX);
        }
        if s.since.is_none() {
            s.since = s.hours.keys().next().map(|(d, _)| *d);
        }
        (s, skipped)
    }

    fn parse_line(&mut self, line: &str) -> Option<()> {
        let f: Vec<&str> = line.split_whitespace().collect();
        let counts = |v: &[&str]| -> Option<Counts> {
            let n = |i: usize| v.get(i).map_or(Some(0), |x| x.parse().ok());
            Some(Counts { keys: n(0)?, taps: n(1)?, clicks: n(2)?, scrolls: n(3)?, scroll_mm: n(4)? })
        };
        match f.as_slice() {
            ["h", when, rest @ ..] if (4..=5).contains(&rest.len()) => {
                let (date, hour) = when.split_once('T')?;
                let hour: u8 = hour.parse().ok().filter(|h| *h < 24)?;
                self.hours.entry((parse_date(date)?, hour)).or_default().add(&counts(rest)?);
            }
            ["t", rest @ ..] if (4..=5).contains(&rest.len()) => self.lifetime = counts(rest)?,
            ["s", date] => self.since = Some(parse_date(date)?),
            ["k", date, code, n] => {
                let n: u32 = n.parse().ok()?;
                let slot = self.keys.entry((parse_date(date)?, code.parse().ok()?)).or_default();
                *slot = slot.saturating_add(n);
            }
            _ => return None,
        }
        Some(())
    }
}

/// `$XDG_STATE_HOME/heptix/stats.tsv`, falling back to `~/.local/state/heptix/stats.tsv`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("heptix").join("stats.tsv")
}

/// Reads the stats file; a missing file is empty stats.
pub fn load(path: &Path) -> io::Result<Stats> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Stats::parse(&text).0),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Stats::default()),
        Err(e) => Err(e),
    }
}

/// Writes atomically (temp file, then rename), readable only by the user.
pub fn save(path: &Path, stats: &Stats) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let tmp = path.with_extension("tsv.tmp");
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(stats.to_text().as_bytes())?;
    f.flush()?;
    drop(f);
    fs::rename(&tmp, path)
}

/// Deletes all stored stats. Returns whether a file existed.
pub fn wipe(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// The service's live counters, flushed to disk periodically.
pub struct Store {
    path: PathBuf,
    stats: Stats,
    dirty: bool,
    on_disk: bool,
    last_save: Instant,
    last_check: Instant,
    last_key: Option<(u16, Instant)>,
    /// Sub-millimetre remainder of scroll distance, in micrometres.
    um_carry: u32,
}

impl Store {
    pub fn open(path: PathBuf) -> Store {
        let (stats, on_disk) = match load(&path) {
            Ok(s) => {
                let exists = path.exists();
                (s, exists)
            }
            Err(e) => {
                eprintln!("stats: {}: {e}", path.display());
                (Stats::default(), false)
            }
        };
        let now = Instant::now();
        Store { path, stats, dirty: false, on_disk, last_save: now, last_check: now, last_key: None, um_carry: 0 }
    }

    /// Adds finger travel in micrometres, keeping the sub-millimetre rest.
    pub fn record_distance_um(&mut self, um: u32) {
        let total = self.um_carry.saturating_add(um);
        self.um_carry = total % 1000;
        if total >= 1000 {
            self.record(Kind::DistanceMm(total / 1000), false);
        }
    }

    pub fn record(&mut self, kind: Kind, per_key: bool) {
        if let Kind::Key(code) = kind {
            // A remapper can expose one keyboard twice; drop the duplicate.
            let now = Instant::now();
            if self.last_key.is_some_and(|(c, t)| c == code && now.duration_since(t) < Duration::from_millis(8)) {
                return;
            }
            self.last_key = Some((code, now));
        }
        self.stats.record(Local::now(), kind, per_key);
        self.dirty = true;
    }

    /// Saves when due (or `force`). Notices a wipe done by the editor or
    /// `--stats-reset` and drops its in-memory counters instead of writing
    /// them back.
    pub fn flush(&mut self, force: bool) {
        if self.on_disk && self.last_check.elapsed() >= WIPE_CHECK_EVERY {
            self.last_check = Instant::now();
            if !self.path.exists() {
                self.stats = Stats::default();
                self.dirty = false;
                self.on_disk = false;
            }
        }
        if !self.dirty || !(force || self.last_save.elapsed() >= SAVE_EVERY) {
            return;
        }
        self.stats.prune(Local::now().day);
        match save(&self.path, &self.stats) {
            Ok(()) => {
                self.dirty = false;
                self.on_disk = true;
                self.last_save = Instant::now();
            }
            Err(e) => {
                eprintln!("stats: cannot save {}: {e}", self.path.display());
                self.last_save = Instant::now(); // retry later, do not spam
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn at(day: i32, hour: u8) -> Local {
        Local { day, hour }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("heptix-stats-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("stats.tsv")
    }

    #[test]
    fn civil_dates_roundtrip_and_weekday() {
        for day in -800..40_000 {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(weekday(0), 3); // Thursday
        assert_eq!(weekday(days_from_civil(2026, 10, 11)), 6); // Sunday
        assert_eq!(weekday(days_from_civil(2026, 10, 12)), 0); // Monday
    }

    #[test]
    fn record_counts_hours_and_keys() {
        let mut s = Stats::default();
        let day = days_from_civil(2026, 10, 11);
        s.record(at(day, 9), Kind::Key(30), true);
        s.record(at(day, 9), Kind::Key(30), true);
        s.record(at(day, 10), Kind::Key(48), true);
        s.record(at(day, 10), Kind::Tap, true);
        s.record(at(day, 10), Kind::Click, true);
        s.record(at(day, 10), Kind::Scroll, true);
        assert_eq!(s.day_counts(day), Counts { keys: 3, taps: 1, clicks: 1, scrolls: 1, scroll_mm: 0 });
        assert_eq!(s.lifetime.keys, 3);
        assert_eq!(s.since, Some(day));
        assert_eq!(s.key_totals(day, day), vec![(30, 2), (48, 1)]);
    }

    #[test]
    fn per_key_off_keeps_only_hour_totals() {
        let mut s = Stats::default();
        s.record(at(100, 1), Kind::Key(30), false);
        assert_eq!(s.day_counts(100).keys, 1);
        assert!(s.keys.is_empty());
    }

    #[test]
    fn text_roundtrip_and_bad_lines_are_skipped() {
        let mut s = Stats::default();
        s.record(at(20_000, 23), Kind::Key(57), true);
        s.record(at(20_001, 0), Kind::Scroll, true);
        let (back, skipped) = Stats::parse(&s.to_text());
        assert_eq!((back, skipped), (s.clone(), 0));
        let (_, skipped) = Stats::parse(&format!("{}garbage\nh 2026-13-40T99 1 2 3 4\nk x y z\n", s.to_text()));
        assert_eq!(skipped, 3);
    }

    #[test]
    fn distance_accumulates_and_survives_roundtrip_and_pruning() {
        let mut s = Stats::default();
        s.record(at(1000, 3), Kind::DistanceMm(1500), false);
        s.record(at(1001, 3), Kind::DistanceMm(500), false);
        assert_eq!(s.day_counts(1000).scroll_mm, 1500);
        assert_eq!(s.lifetime.scroll_mm, 2000);
        assert_eq!(Counts { scroll_mm: 9, ..Counts::default() }.total(), 0);
        s.prune(1000 + RETENTION_DAYS + 1);
        assert_eq!(s.lifetime.scroll_mm, 2000);
        assert_eq!(s.range_counts(0, 5000).scroll_mm, 500);
        let (back, skipped) = Stats::parse(&s.to_text());
        assert_eq!((back, skipped), (s, 0));
    }

    #[test]
    fn old_four_counter_lines_still_parse_and_totals_are_rebuilt() {
        let (s, skipped) = Stats::parse("h 2026-10-11T09 5 1 2 3\n");
        assert_eq!(skipped, 0);
        assert_eq!(s.lifetime, Counts { keys: 5, taps: 1, clicks: 2, scrolls: 3, scroll_mm: 0 });
        assert_eq!(s.since, Some(days_from_civil(2026, 10, 11)));
    }

    #[test]
    fn store_carries_sub_millimetre_distance() {
        let mut store = Store::open(temp("um"));
        for _ in 0..4 {
            store.record_distance_um(600);
        }
        assert_eq!(store.stats.lifetime.scroll_mm, 2);
        assert_eq!(store.um_carry, 400);
    }

    #[test]
    fn prune_drops_old_data() {
        let mut s = Stats::default();
        s.record(at(1000, 1), Kind::Key(1), true);
        s.record(at(1000 + RETENTION_DAYS, 1), Kind::Key(1), true);
        s.prune(1000 + RETENTION_DAYS + 5);
        assert_eq!(s.hours.len(), 1);
        assert_eq!(s.keys.len(), 1);
    }

    #[test]
    fn hour_grid_sums_by_weekday_and_hour() {
        let mut s = Stats::default();
        let mon = days_from_civil(2026, 10, 12);
        s.record(at(mon, 9), Kind::Key(1), true);
        s.record(at(mon + 7, 9), Kind::Key(1), true);
        s.record(at(mon + 1, 9), Kind::Tap, true);
        let g = s.hour_grid(mon, mon + 7, Metric::Keys);
        assert_eq!(g[0][9], 2);
        assert_eq!(g[1][9], 0);
        assert_eq!(s.hour_grid(mon, mon + 7, Metric::All)[1][9], 1);
    }

    #[test]
    fn streak_and_busiest_day() {
        let mut s = Stats::default();
        for d in [10, 11, 12, 14] {
            s.record(at(d, 8), Kind::Key(1), true);
        }
        s.record(at(11, 9), Kind::Key(1), true);
        assert_eq!(s.streak(14), 1);
        assert_eq!(s.streak(13), 3); // today (13) empty: counts back from yesterday
        assert_eq!(s.streak(9), 0);
        assert_eq!(s.busiest_day(10, 14, Metric::Keys), Some((11, 2)));
    }

    #[test]
    fn save_is_private_and_atomic() {
        let path = temp("save");
        let mut s = Stats::default();
        s.record(at(5, 5), Kind::Key(5), true);
        save(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap(), s);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(!path.with_extension("tsv.tmp").exists());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_file_loads_empty() {
        assert!(load(&temp("missing")).unwrap().is_empty());
    }

    #[test]
    fn store_flushes_and_honours_a_wipe() {
        let path = temp("store");
        let mut store = Store::open(path.clone());
        store.record(Kind::Key(30), true);
        store.flush(true);
        assert!(path.exists());
        assert_eq!(load(&path).unwrap().hours.len(), 1);

        // Wiped behind the service's back: its memory is dropped, not rewritten.
        assert!(wipe(&path).unwrap());
        store.last_check = Instant::now() - Duration::from_secs(5);
        store.flush(true);
        assert!(!path.exists());
        assert!(store.stats.is_empty());

        store.record(Kind::Tap, true);
        store.flush(true);
        let again = load(&path).unwrap();
        assert_eq!(again.day_counts(Local::now().day).taps, 1);
        assert_eq!(again.day_counts(Local::now().day).keys, 0);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn store_drops_duplicate_key_events() {
        let mut store = Store::open(temp("dupe"));
        store.record(Kind::Key(30), true);
        store.record(Kind::Key(30), true);
        assert_eq!(store.stats.day_counts(Local::now().day).keys, 1);
    }
}
