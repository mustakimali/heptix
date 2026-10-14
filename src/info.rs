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

//! `heptix --info`: where everything is and what is running. Read-only: it creates and
//! changes nothing, and never connects to the reader's socket (that would show up in its
//! log as a refused client).

use crate::{config, stats};
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REV: &str = env!("HEPTIX_REV");
const BUILD_DATE: &str = env!("HEPTIX_BUILD_DATE");

#[derive(Default)]
struct Report {
    text: String,
    problems: Vec<String>,
}

impl Report {
    fn section(&mut self, title: &str) {
        self.text += &format!("\n{title}\n");
    }

    fn row(&mut self, key: &str, value: impl Display) {
        self.text += &format!("  {key:<14}{value}\n");
    }

    fn problem(&mut self, what: impl Into<String>) {
        self.problems.push(what.into());
    }
}

pub fn print(config_path: &Path, socket: &Path) {
    let mut r = Report::default();
    let uid = unsafe { libc::getuid() };
    let settings = fs::read_to_string(config_path).map(|t| config::Settings::parse(&t).0).unwrap_or_else(|_| config::Settings::parse("").0);

    r.text += &format!("heptix {VERSION}\n");
    r.section("Build");
    r.row("version", VERSION);
    r.row("revision", REV);
    r.row("build date", BUILD_DATE);
    r.row("binary", std::env::current_exe().map_or("unknown".into(), |p| p.display().to_string()));

    let user_unit = unit(true, "heptix.service");
    let reader_unit = unit(false, "heptix-reader.service");
    let mode = mode(user_unit.as_ref(), socket);

    r.section("Running mode");
    r.row("mode", &mode.text);
    r.row("client", unit_line(user_unit.as_ref(), "heptix.service (user)"));
    if let Some(u) = &user_unit {
        if u.get("LoadState").is_some_and(|s| s == "loaded") {
            r.row("unit file", u.get("FragmentPath").map_or("?", String::as_str));
            r.row("command", exec_args(u));
            if u.get("ActiveState").is_some_and(|s| s != "active") {
                r.problem("the heptix user service is not running (systemctl --user start heptix)");
            }
        }
    }

    r.section("Reader service");
    r.row("service", unit_line(reader_unit.as_ref(), "heptix-reader.service"));
    if let Some(u) = &reader_unit {
        if u.get("LoadState").is_some_and(|s| s == "loaded") {
            r.row("unit file", u.get("FragmentPath").map_or("?", String::as_str));
            r.row("runs as", u.get("User").filter(|s| !s.is_empty()).map_or("?", String::as_str));
        }
    }
    r.row("socket", socket_line(socket));
    if let Some(n) = connected_clients(socket) {
        r.row("clients", format!("{n} connected"));
    }
    let allow = fs::read_to_string("/etc/heptix/allow-uids").ok();
    r.row(
        "allow-uids",
        allow.as_deref().map_or("none (serves the user of the active seat session)".to_string(), |t| {
            t.split_whitespace().collect::<Vec<_>>().join(" ")
        }),
    );
    let seat = active_seat_uid();
    r.row(
        "active seat",
        match seat {
            Some(s) if s == uid => format!("uid {s} (you)"),
            Some(s) => format!("uid {s} (not you)"),
            None => "no active session".into(),
        },
    );
    if mode.service {
        if reader_unit.as_ref().and_then(|u| u.get("ActiveState")).is_none_or(|s| s != "active") {
            r.problem("the reader service is not running (sudo systemctl start heptix-reader)");
        } else if !socket.exists() {
            r.problem(format!("the reader is running but {} does not exist", socket.display()));
        }
        let allowed = allow.as_deref().is_some_and(|t| t.split_whitespace().any(|w| w == uid.to_string()));
        if seat.is_some_and(|s| s != uid) && !allowed {
            r.problem("the reader serves the user of the active session, which is not you");
        }
    }

    r.section("You");
    r.row("user", format!("{} (uid {uid})", user_name(uid).unwrap_or_else(|| "?".into())));
    r.row("input group", if in_group("input") { "member (direct mode possible; service mode does not need it)" } else { "not a member (service mode only)" });

    r.section("Files");
    r.row("settings", file_line(config_path));
    let stats_file = stats::default_path();
    r.row("stats", file_line(&stats_file));
    r.row("stats (live)", file_line(&stats::live_path(&stats_file)));
    for backup in stats::backups(&stats_file) {
        r.row("stats backup", file_line(&backup));
    }
    match stats::load(&stats_file) {
        Ok(s) if !s.is_empty() || s.lifetime.total() > 0 => {
            let days: std::collections::BTreeSet<i32> = s.hours.keys().map(|(d, _)| *d).collect();
            r.row(
                "recorded",
                format!(
                    "{} days kept{}, {} key presses in total",
                    days.len(),
                    s.since.map_or(String::new(), |d| format!(" (since {})", stats::date_string(d))),
                    s.lifetime.keys
                ),
            );
        }
        Ok(_) => r.row("recorded", "nothing yet"),
        Err(e) => r.row("recorded", format!("cannot read: {e}")),
    }

    r.section("Settings in effect");
    r.row("enabled", settings.enabled);
    if !settings.enabled {
        r.problem("heptix is switched off (enabled = false in the settings)");
    }
    r.row("stats", if settings.stats { if settings.stats_keys { "on, with per-key counts" } else { "on, without per-key counts" } } else { "off" });
    r.row("volume", format!("{} (auto reference {}%)", settings.volume_mode, settings.auto_reference));
    r.row("sounds", format!("tap {}, scroll {}, keyboard {}, mouse {}", settings.sound, settings.scroll_sound, settings.keyboard_sound, settings.mouse_sound));

    r.section("Status");
    if r.problems.is_empty() {
        r.text += "  everything looks fine\n";
    } else {
        for p in std::mem::take(&mut r.problems) {
            r.text += &format!("  - {p}\n");
        }
    }
    r.text += "\n(heptix --list shows the detected devices, heptix --stats your activity)\n";
    print!("{}", r.text);
}

struct Mode {
    text: String,
    /// Whether the reader service is in use (as opposed to reading devices directly).
    service: bool,
}

/// How the background service gets its input, from its unit's command line.
fn mode(user_unit: Option<&HashMap<String, String>>, socket: &Path) -> Mode {
    let Some(u) = user_unit.filter(|u| u.get("LoadState").is_some_and(|s| s == "loaded")) else {
        return Mode { text: "no user service installed (running in the foreground only)".into(), service: false };
    };
    let args = exec_args(u);
    if args.contains("--direct") {
        Mode { text: "direct: heptix reads the input devices itself (needs the input group)".into(), service: false }
    } else if args.contains("--client") {
        Mode { text: "service: the client gets events from the reader service".into(), service: true }
    } else if args.contains("--run") && socket.exists() {
        Mode { text: "automatic, using the reader service (its socket exists)".into(), service: true }
    } else if args.contains("--run") {
        Mode { text: "automatic, reading the devices directly (no reader socket)".into(), service: false }
    } else {
        Mode { text: format!("unknown command line: {args}"), service: false }
    }
}

/// `systemctl show` of one unit; None without systemd.
fn unit(user: bool, name: &str) -> Option<HashMap<String, String>> {
    let mut cmd = Command::new("systemctl");
    if user {
        cmd.arg("--user");
    }
    let out = cmd
        .args(["show", name, "-p", "LoadState,ActiveState,SubState,MainPID,ActiveEnterTimestamp,FragmentPath,ExecStart,User"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect())
}

fn unit_line(unit: Option<&HashMap<String, String>>, name: &str) -> String {
    let Some(u) = unit else { return format!("{name}: systemd is not available") };
    if u.get("LoadState").is_none_or(|s| s != "loaded") {
        return format!("{name}: not installed");
    }
    let state = u.get("ActiveState").map_or("?", String::as_str);
    let sub = u.get("SubState").map_or("?", String::as_str);
    let mut line = format!("{state} ({sub})");
    if state == "active" {
        if let Some(pid) = u.get("MainPID").filter(|p| *p != "0") {
            line += &format!(", pid {pid}");
        }
        if let Some(since) = u.get("ActiveEnterTimestamp").filter(|s| !s.is_empty()) {
            line += &format!(", since {since}");
        }
    }
    line
}

/// The command line from systemd's `ExecStart={ path=... ; argv[]=... ; ... }`.
fn exec_args(u: &HashMap<String, String>) -> String {
    u.get("ExecStart")
        .and_then(|e| e.split("argv[]=").nth(1))
        .and_then(|a| a.split(" ; ").next())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn file_line(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(m) => {
            let age = m.modified().ok().and_then(|t| SystemTime::now().duration_since(t).ok()).map_or(String::new(), |d| format!(", changed {} ago", age(d.as_secs())));
            format!("{} ({}{age})", path.display(), size(m.len()))
        }
        Err(_) => format!("{} (not there)", path.display()),
    }
}

fn socket_line(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(m) if m.file_type().is_socket() => {
            format!("{} (socket, mode {:o}, owner {})", path.display(), m.mode() & 0o7777, user_name(m.uid()).unwrap_or_else(|| m.uid().to_string()))
        }
        Ok(_) => format!("{} (exists but is not a socket)", path.display()),
        Err(_) => format!("{} (not there)", path.display()),
    }
}

/// Connected clients of the socket, from the kernel's table of unix sockets.
fn connected_clients(path: &Path) -> Option<usize> {
    let table = fs::read_to_string("/proc/net/unix").ok()?;
    let want = path.to_str()?;
    Some(
        table
            .lines()
            .skip(1)
            .filter(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                // Num RefCount Protocol Flags Type St Inode Path; St 03 is connected
                f.len() >= 8 && f[5] == "03" && f[7] == want
            })
            .count(),
    )
}

fn active_seat_uid() -> Option<u32> {
    let text = fs::read_to_string("/run/systemd/seats/seat0").ok()?;
    text.lines().find_map(|l| l.strip_prefix("ACTIVE_UID=")).and_then(|v| v.trim().parse().ok())
}

fn user_name(uid: u32) -> Option<String> {
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() {
        return None;
    }
    Some(unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }.to_string_lossy().into_owned())
}

fn in_group(name: &str) -> bool {
    let Ok(cname) = std::ffi::CString::new(name) else { return false };
    let grp = unsafe { libc::getgrnam(cname.as_ptr()) };
    if grp.is_null() {
        return false;
    }
    let gid = unsafe { (*grp).gr_gid };
    let n = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    let mut groups = vec![0 as libc::gid_t; n.max(0) as usize];
    let n = unsafe { libc::getgroups(n, groups.as_mut_ptr()) };
    groups.truncate(n.max(0) as usize);
    groups.contains(&gid)
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / 1024.0 / 1024.0),
    }
}

fn age(secs: u64) -> String {
    match secs {
        s if s < 90 => format!("{s} s"),
        s if s < 90 * 60 => format!("{} min", s / 60),
        s if s < 48 * 3600 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_with(exec: &str) -> HashMap<String, String> {
        HashMap::from([
            ("LoadState".to_string(), "loaded".to_string()),
            ("ExecStart".to_string(), format!("{{ path=/x/heptix ; argv[]={exec} ; ignore_errors=no ; start_time=[n/a] }}")),
        ])
    }

    #[test]
    fn the_command_line_is_read_from_systemds_exec_start() {
        assert_eq!(exec_args(&unit_with("/x/heptix --client")), "/x/heptix --client");
        assert_eq!(exec_args(&HashMap::new()), "");
    }

    #[test]
    fn the_mode_follows_the_user_units_command_line() {
        let none = Path::new("/nonexistent/heptix.sock");
        assert!(mode(Some(&unit_with("/x/heptix --client")), none).service);
        assert!(!mode(Some(&unit_with("/x/heptix --direct")), none).service);
        assert!(!mode(Some(&unit_with("/x/heptix --run")), none).service, "no socket: direct");
        assert!(!mode(None, none).service);
        let present = std::env::temp_dir().join(format!("heptix-info-{}.sock", std::process::id()));
        let _l = std::os::unix::net::UnixListener::bind(&present).unwrap();
        assert!(mode(Some(&unit_with("/x/heptix --run")), &present).service, "with a socket: the reader");
        let _ = fs::remove_file(&present);
    }

    #[test]
    fn units_that_are_missing_or_stopped_are_described() {
        assert!(unit_line(None, "x").contains("not available"));
        let mut u = unit_with("/x/heptix --client");
        u.insert("LoadState".into(), "not-found".into());
        assert!(unit_line(Some(&u), "x").contains("not installed"));
        let u = HashMap::from([
            ("LoadState".to_string(), "loaded".to_string()),
            ("ActiveState".to_string(), "active".to_string()),
            ("SubState".to_string(), "running".to_string()),
            ("MainPID".to_string(), "42".to_string()),
        ]);
        assert_eq!(unit_line(Some(&u), "x"), "active (running), pid 42");
    }

    #[test]
    fn sizes_and_ages_read_naturally() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(3236), "3.2 KB");
        assert_eq!(age(40), "40 s");
        assert_eq!(age(600), "10 min");
        assert_eq!(age(5 * 3600), "5 h");
        assert_eq!(age(3 * 86_400), "3 days");
    }
}
