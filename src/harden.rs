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

//! Self-protection for the client process, which holds the raw key events.

use std::io;

/// Makes the process hard to inspect, and refuses to run if someone already is.
///
/// - Non-dumpable: other programs of the same user can neither attach a
///   debugger nor read its memory, and its private `/proc/<pid>` entries become root-owned.
/// - `no_new_privs`: nothing it starts can gain privileges. It applies to the
///   calling thread and everything it creates afterwards, and `/proc/<pid>/status`
///   shows the main thread's value, so call this from the main thread first.
/// - No core dumps, which would write its memory to disk.
pub fn harden_process() -> io::Result<()> {
    unsafe {
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(io::Error::last_os_error());
        }
        let no_core = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        libc::setrlimit(libc::RLIMIT_CORE, &no_core);
    }
    // A tracer that attached before we got here (for instance by starting us
    // under a debugger) already controls the process.
    let status = std::fs::read_to_string("/proc/self/status")?;
    let tracer = status.lines().find_map(|l| l.strip_prefix("TracerPid:")).map(|v| v.trim()).unwrap_or("0");
    if tracer != "0" {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("this process is being traced (by pid {tracer})")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardening_sets_the_flags_and_hides_the_proc_entries() {
        // Runs in a child so the test process itself stays inspectable.
        let me = std::process::id();
        if std::env::var_os("HEPTIX_HARDEN_CHILD").is_some() {
            let before_owner = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata("/proc/self/environ").unwrap());
            harden_process().unwrap();
            // no_new_privs belongs to the calling thread (tests run on their own thread).
            let status = std::fs::read_to_string("/proc/thread-self/status").unwrap();
            assert!(status.lines().any(|l| l.strip_prefix("NoNewPrivs:").map(str::trim) == Some("1")));
            let dumpable = unsafe { libc::prctl(libc::PR_GET_DUMPABLE) };
            assert_eq!(dumpable, 0);
            // The private /proc entries stop belonging to us. (To the host's root, which is
            // uid 0 normally and an unmapped id inside a user namespace such as a build sandbox.)
            let owner = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata("/proc/self/environ").unwrap());
            assert_ne!(owner, before_owner, "environ must no longer belong to the process's own user");
            return;
        }
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(exe)
            .args(["--exact", "harden::tests::hardening_sets_the_flags_and_hides_the_proc_entries", "--nocapture"])
            .env("HEPTIX_HARDEN_CHILD", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "child failed (pid {me}): {}", String::from_utf8_lossy(&out.stderr));
    }
}
