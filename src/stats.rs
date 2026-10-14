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

pub fn date_string(day: i32) -> String {
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

/// Which of the two stats files is being written.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    History,
    Live,
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

    /// Everything in one text, as a single legacy-style file.
    #[cfg(test)]
    pub fn to_text(&self) -> String {
        self.render(Part::Live, i32::MIN)
    }

    /// One of the two files: the history holds the days before `boundary`, the live
    /// file the days from `boundary` on plus the all-time totals.
    fn render(&self, part: Part, boundary: i32) -> String {
        let mut out = String::from(
            "# heptix activity stats: counters only (totals per hour, presses per key per day).\n\
             # h <date>T<hour> <keys> <taps> <clicks> <scroll ticks> <scroll mm>\n\
             # k <date> <keycode> <count>\n\
             # t <keys> <taps> <clicks> <scroll ticks> <scroll mm>   (all-time totals)\n\
             # s <date>   (first day recorded)\n\
             # Days before today are in stats.tsv, recent days and the totals in stats.live.tsv.\n",
        );
        let wanted = |day: i32| (day < boundary) == (part == Part::History);
        if part == Part::Live {
            let l = &self.lifetime;
            out += &format!("t {} {} {} {} {}\n", l.keys, l.taps, l.clicks, l.scrolls, l.scroll_mm);
        }
        if let Some(day) = self.since {
            out += &format!("s {}\n", date_string(day));
        }
        for (&(day, hour), c) in self.hours.iter().filter(|((d, _), _)| wanted(*d)) {
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
        for (&(day, code), n) in self.keys.iter().filter(|((d, _), _)| wanted(*d)) {
            out += &format!("k {} {code} {n}\n", date_string(day));
        }
        out
    }

    /// Parses one stats file. Returns the stats and how many lines were skipped.
    #[cfg(test)]
    pub fn parse(text: &str) -> (Stats, usize) {
        let mut s = Stats::default();
        let (skipped, has_totals) = s.parse_into(text);
        s.finish(has_totals);
        (s, skipped)
    }

    /// Reads the history and the live file together. A day present in the live file
    /// replaces that day in the history, so a save interrupted between the two writes
    /// never counts anything twice.
    pub fn parse_split(history: &str, live: &str) -> Stats {
        let mut s = Stats::default();
        let (_, history_totals) = s.parse_into(history);
        let mut l = Stats::default();
        let (_, live_totals) = l.parse_into(live);
        let days: std::collections::BTreeSet<i32> =
            l.hours.keys().map(|(d, _)| *d).chain(l.keys.keys().map(|(d, _)| *d)).collect();
        s.hours.retain(|(d, _), _| !days.contains(d));
        s.keys.retain(|(d, _), _| !days.contains(d));
        s.hours.extend(l.hours);
        s.keys.extend(l.keys);
        if live_totals {
            s.lifetime = l.lifetime;
        }
        s.since = match (s.since, l.since) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        s.finish(history_totals || live_totals);
        s
    }

    /// Returns (lines skipped, whether a totals line was present).
    fn parse_into(&mut self, text: &str) -> (usize, bool) {
        let mut skipped = 0;
        let mut has_totals = false;
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            has_totals |= line.starts_with("t ");
            if self.parse_line(line).is_none() {
                skipped += 1;
            }
        }
        (skipped, has_totals)
    }

    /// Fills in what a file without totals or a first-day line leaves out.
    fn finish(&mut self, has_totals: bool) {
        if !has_totals {
            self.lifetime = self.range_counts(i32::MIN, i32::MAX);
        }
        if self.since.is_none() {
            self.since = self.hours.keys().next().map(|(d, _)| *d);
        }
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

/// The file holding recent days and the totals, next to the history file.
pub fn live_path(path: &Path) -> PathBuf {
    path.with_extension("live.tsv")
}

/// Where copies of an old single-file `stats.tsv` are kept: `stats.tsv.bak`, then `.bak.1`, ...
pub fn backup_paths(path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    (0..20).map(|n| match n {
        0 => path.with_extension("tsv.bak"),
        n => path.with_extension(format!("tsv.bak.{n}")),
    })
}

/// The copies of an old single-file stats.tsv that exist.
pub fn backups(path: &Path) -> Vec<PathBuf> {
    backup_paths(path).filter(|p| p.exists()).collect()
}

/// True when `path` is an old single stats.tsv: it has no live file next to it yet.
fn is_unsplit(path: &Path) -> bool {
    path.exists() && !live_path(path).exists()
}

/// Makes sure a verified copy of `path` exists before it is split. An existing copy with the
/// same content counts; one with different content (an older backup) is left alone and the
/// next free name is used, so the current data is always what gets copied.
fn back_up(path: &Path) -> io::Result<()> {
    let data = fs::read(path)?;
    for copy in backup_paths(path) {
        match fs::read(&copy) {
            Ok(old) if old == data => return Ok(()),
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                write_atomic(&copy, &data)?;
                if fs::read(&copy)? != data {
                    let _ = fs::remove_file(&copy);
                    return Err(io::Error::other("the copy does not match the original"));
                }
                return Ok(());
            }
            Err(_) => continue,
        }
    }
    Err(io::Error::other("too many old backups next to stats.tsv"))
}

fn read_or_empty(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// Reads the history and the live file; missing files are empty stats.
pub fn load(path: &Path) -> io::Result<Stats> {
    Ok(Stats::parse_split(&read_or_empty(path)?, &read_or_empty(&live_path(path))?))
}

/// Writes atomically (temp file, then rename), readable only by the user.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let tmp = path.with_extension("tsv.tmp");
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(data)?;
    f.flush()?;
    drop(f);
    fs::rename(&tmp, path)
}

/// Writes the small live file: the days from `boundary` on and the totals.
fn save_live(path: &Path, stats: &Stats, boundary: i32) -> io::Result<()> {
    write_atomic(&live_path(path), stats.render(Part::Live, boundary).as_bytes())
}

/// Writes both files, history first: if a crash lands in between, the days that moved
/// are in both and the live file wins when they are read.
pub fn save(path: &Path, stats: &Stats, boundary: i32) -> io::Result<()> {
    write_atomic(path, stats.render(Part::History, boundary).as_bytes())?;
    save_live(path, stats, boundary)
}

/// Deletes the stats files (and the backup of an older layout). Returns whether any existed.
pub fn wipe(path: &Path) -> io::Result<bool> {
    let mut any = false;
    for p in [live_path(path), path.to_path_buf()].into_iter().chain(backup_paths(path)) {
        match fs::remove_file(&p) {
            Ok(()) => any = true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(any)
}

/// The service's live counters, flushed to disk periodically.
pub struct Store {
    path: PathBuf,
    stats: Stats,
    dirty: bool,
    /// An old single stats.tsv whose copy could not be made yet: nothing is saved until it can.
    unbacked: bool,
    /// Days before this are in the history file as last written (None: not written yet).
    history_boundary: Option<i32>,
    /// A day inside the history was changed (a clock that went back), so rewrite it.
    history_dirty: bool,
    on_disk: bool,
    last_save: Instant,
    last_check: Instant,
    /// Sub-millimetre remainder of scroll distance, in micrometres.
    um_carry: u32,
}

impl Store {
    pub fn open(path: PathBuf) -> Store {
        let (stats, on_disk) = match load(&path) {
            Ok(s) => {
                let exists = path.exists() || live_path(&path).exists();
                (s, exists)
            }
            Err(e) => {
                eprintln!("stats: {}: {e}", path.display());
                (Stats::default(), false)
            }
        };
        // An old single stats.tsv: keep a copy right away, before anything can touch it.
        let unbacked = is_unsplit(&path)
            && back_up(&path).map_err(|e| eprintln!("stats: cannot back up {}: {e}", path.display())).is_err();
        let now = Instant::now();
        Store {
            path,
            stats,
            dirty: false,
            unbacked,
            history_boundary: None,
            history_dirty: false,
            on_disk,
            last_save: now,
            last_check: now,
            um_carry: 0,
        }
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
        // Every event counts: duplicate keyboards are filtered where events are read, and two real
        // presses of one key can arrive together when the machine is busy.
        let now = Local::now();
        self.stats.record(now, kind, per_key);
        self.dirty = true;
        if self.history_boundary.is_some_and(|b| now.day < b) {
            self.history_dirty = true;
        }
    }

    /// Saves when due (or `force`). Notices a wipe done by the editor or
    /// `--stats-reset` and drops its in-memory counters instead of writing
    /// them back.
    pub fn flush(&mut self, force: bool) {
        if self.on_disk && self.last_check.elapsed() >= WIPE_CHECK_EVERY {
            self.last_check = Instant::now();
            // Both gone: an old single stats.tsv has no live file yet, and is not a wipe.
            if !self.path.exists() && !live_path(&self.path).exists() {
                // Wiped: drop the counters, and any history left behind so it does not come back.
                let _ = wipe(&self.path);
                self.stats = Stats::default();
                self.dirty = false;
                self.on_disk = false;
                self.history_boundary = None;
                self.history_dirty = false;
            }
        }
        if !self.dirty || !(force || self.last_save.elapsed() >= SAVE_EVERY) {
            return;
        }
        // Never split an old file without a copy: retry the copy, and keep the file as it is.
        if self.unbacked {
            match back_up(&self.path) {
                Ok(()) => self.unbacked = false,
                Err(e) => {
                    eprintln!("stats: cannot back up {}, not saving: {e}", self.path.display());
                    self.last_save = Instant::now();
                    return;
                }
            }
        }
        let today = Local::now().day;
        self.stats.prune(today);
        // Days before yesterday are final and live in the history file, which is rewritten
        // only when that set changes (a new day, or after a long sleep). Every other save
        // writes just the small live file.
        let boundary = today - 1;
        let history_due = self.history_boundary != Some(boundary) || self.history_dirty;
        let result = if history_due {
            save(&self.path, &self.stats, boundary)
        } else {
            save_live(&self.path, &self.stats, boundary)
        };
        match result {
            Ok(()) => {
                if history_due {
                    self.history_boundary = Some(boundary);
                    self.history_dirty = false;
                }
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
        save(&path, &s, 4).unwrap();
        assert_eq!(load(&path).unwrap(), s);
        for file in [path.clone(), live_path(&path)] {
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(!path.with_extension("tsv.tmp").exists());
        assert!(!live_path(&path).with_extension("tsv.tmp").exists());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    /// Days 1..=10 with a few keys each, plus a total that is not just their sum.
    fn ten_days() -> Stats {
        days_ending(10)
    }

    fn days_ending(last: i32) -> Stats {
        let mut s = Stats::default();
        for d in last - 9..=last {
            s.record(at(d, 8), Kind::Key(30), true);
            s.record(at(d, 9), Kind::Key(48), true);
            s.record(at(d, 9), Kind::Tap, true);
        }
        s.lifetime.keys += 1000;
        s
    }

    #[test]
    fn days_before_the_boundary_go_to_the_history_and_the_rest_to_the_live_file() {
        let path = temp("split");
        let s = ten_days();
        save(&path, &s, 9).unwrap();
        let history = fs::read_to_string(&path).unwrap();
        let live = fs::read_to_string(live_path(&path)).unwrap();
        let day = |d| date_string(d);
        assert!(history.contains(&format!("h {}T08", day(8))));
        assert!(!history.contains(&format!("h {}T08", day(9))));
        assert!(!history.lines().any(|l| l.starts_with("t ")), "the totals change every save: live file only");
        assert!(live.contains(&format!("h {}T08", day(9))) && live.contains(&format!("h {}T08", day(10))));
        assert!(!live.contains(&format!("h {}T08", day(8))));
        assert!(live.lines().any(|l| l.starts_with("t ")));
        assert_eq!(load(&path).unwrap(), s);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_day_in_both_files_is_counted_once_and_the_live_file_wins() {
        // A crash between the two writes: the history already holds days 8 and 9, the old
        // live file still holds them too (with slightly older counts).
        let path = temp("both");
        let s = ten_days();
        save(&path, &s, 10).unwrap();
        let mut older = s.clone();
        older.hours.get_mut(&(9, 9)).unwrap().taps = 0;
        write_atomic(&live_path(&path), older.render(Part::Live, 9).as_bytes()).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back.hours.len(), s.hours.len(), "no day twice");
        assert_eq!(back.day_counts(9).taps, 0, "the live file's version of the day wins");
        assert_eq!(back.day_counts(8), s.day_counts(8));
        assert_eq!(back.lifetime, s.lifetime);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn days_left_in_the_live_file_by_a_long_sleep_move_to_the_history_on_the_next_save() {
        // The machine slept from day 3 to day 10: the live file still holds days 2 and 3.
        let path = temp("sleep");
        let s = ten_days();
        let mut slept = Stats::default();
        slept.hours = s.hours.range((1, 0)..=(3, 23)).map(|(k, v)| (*k, *v)).collect();
        slept.keys = s.keys.range((1, 0)..=(3, u16::MAX)).map(|(k, v)| (*k, *v)).collect();
        slept.lifetime = slept.range_counts(0, 100);
        slept.since = Some(1);
        save(&path, &slept, 2).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains(&format!("h {}T08", date_string(2))));
        // Awake on day 10: loaded as is, nothing lost, then the next save re-splits.
        let mut awake = load(&path).unwrap();
        assert_eq!(awake, slept);
        awake.record(at(10, 8), Kind::Key(30), true);
        save(&path, &awake, 9).unwrap();
        let history = fs::read_to_string(&path).unwrap();
        let live = fs::read_to_string(live_path(&path)).unwrap();
        for d in 1..=3 {
            assert!(history.contains(&format!("h {}T08", date_string(d))), "day {d} moved to the history");
            assert!(!live.contains(&format!("h {}T08", date_string(d))));
        }
        assert_eq!(load(&path).unwrap(), awake);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn an_old_single_file_is_split_on_the_first_save_and_backed_up_once() {
        let path = temp("legacy");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let today = Local::now().day;
        let old = days_ending(today - 3); // recent, so the save does not prune it
        let old_text = old.to_text();
        fs::write(&path, &old_text).unwrap();
        assert_eq!(load(&path).unwrap(), old);

        let mut store = Store::open(path.clone());
        let backup = path.with_extension("tsv.bak");
        assert_eq!(fs::read_to_string(&backup).unwrap(), old_text, "backed up before anything else");
        // The wipe check runs before the first save, as it does in the service: the old file
        // has no live file next to it yet, and that is not a wipe.
        store.last_check = Instant::now() - Duration::from_secs(5);
        store.flush(false);
        assert!(path.exists(), "an old single file is not a wipe");
        assert_eq!(load(&path).unwrap(), old);
        store.record(Kind::Tap, true);
        store.last_check = Instant::now() - Duration::from_secs(5);
        store.flush(true);
        assert!(live_path(&path).exists());
        let after = load(&path).unwrap();
        assert_eq!(after.lifetime.keys, old.lifetime.keys, "the totals survive the split");
        assert_eq!(after.lifetime.taps, old.lifetime.taps + 1);
        assert_eq!(after.since, old.since);
        assert_eq!(after.range_counts(today - 20, today - 3), old.range_counts(today - 20, today - 3));

        // The backup is made once: later saves leave it alone.
        fs::write(&backup, "kept").unwrap();
        store.record(Kind::Tap, true);
        store.flush(true);
        assert_eq!(fs::read_to_string(&backup).unwrap(), "kept");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn an_older_backup_is_not_overwritten_and_the_current_data_is_copied_next_to_it() {
        let path = temp("two-backups");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bak = path.with_extension("tsv.bak");
        fs::write(&bak, "from an earlier upgrade").unwrap();
        let text = days_ending(Local::now().day - 3).to_text();
        fs::write(&path, &text).unwrap();
        let _store = Store::open(path.clone());
        assert_eq!(fs::read_to_string(&bak).unwrap(), "from an earlier upgrade");
        assert_eq!(fs::read_to_string(path.with_extension("tsv.bak.1")).unwrap(), text);
        // Opening again does not make a third copy of the same data.
        let _again = Store::open(path.clone());
        assert!(!path.with_extension("tsv.bak.2").exists());
        // And the wipe removes all of them.
        assert!(wipe(&path).unwrap());
        assert!(!bak.exists() && !path.with_extension("tsv.bak.1").exists());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn an_old_file_is_not_split_until_its_copy_can_be_made() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root ignores directory permissions
        }
        let path = temp("no-copy");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = days_ending(Local::now().day - 3).to_text();
        fs::write(&path, &text).unwrap();
        let dir = path.parent().unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o500)).unwrap();

        let mut store = Store::open(path.clone());
        store.record(Kind::Tap, true);
        store.flush(true);
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "the original is untouched");
        assert!(!live_path(&path).exists());

        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        store.last_save = Instant::now() - SAVE_EVERY;
        store.flush(true);
        assert_eq!(fs::read_to_string(path.with_extension("tsv.bak")).unwrap(), text);
        assert!(live_path(&path).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ordinary_saves_write_only_the_live_file() {
        let path = temp("live-only");
        let mut store = Store::open(path.clone());
        store.record(Kind::Key(30), true);
        store.flush(true);
        assert!(path.exists() && live_path(&path).exists());
        fs::remove_file(&path).unwrap();
        store.record(Kind::Key(30), true);
        store.flush(true);
        assert!(!path.exists(), "the history is not rewritten while the day has not changed");
        assert_eq!(load(&path).unwrap().lifetime.keys, 2);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_change_to_a_day_inside_the_history_rewrites_it() {
        let path = temp("clock");
        let mut store = Store::open(path.clone());
        store.record(Kind::Key(30), true);
        store.flush(true);
        fs::remove_file(&path).unwrap();
        // A clock that went back: this press lands on a day the history owns.
        store.stats.record(at(Local::now().day - 5, 3), Kind::Key(30), true);
        store.history_dirty = true;
        store.dirty = true;
        store.flush(true);
        assert!(path.exists());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn wiping_removes_every_file() {
        let path = temp("wipe-all");
        save(&path, &ten_days(), 5).unwrap();
        fs::write(path.with_extension("tsv.bak"), "old").unwrap();
        assert!(wipe(&path).unwrap());
        assert!(!path.exists() && !live_path(&path).exists() && !path.with_extension("tsv.bak").exists());
        assert!(!wipe(&path).unwrap());
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
    fn store_counts_presses_that_arrive_together() {
        let mut store = Store::open(temp("burst"));
        for _ in 0..5 {
            store.record(Kind::Key(30), true);
        }
        assert_eq!(store.stats.day_counts(Local::now().day).keys, 5);
    }
}
