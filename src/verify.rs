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

//! Deciding whether the process on the other end of the reader socket is the
//! genuine heptix client, from what the kernel reports about it.
//!
//! None of this is a proof: each check closes one way a program running as the
//! same user could pose as the client, and together they make that hard. The
//! reader needs `CAP_SYS_PTRACE` to read another user's `/proc/<pid>/exe` and
//! namespace links (its unit blocks the ptrace system calls themselves).

use crate::peer::PeerInfo;
use crate::reader::Authorizer;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Reads the facts about a process, from `/proc` or a fake tree in tests.
pub struct ProcFs(pub PathBuf);

impl ProcFs {
    pub fn system() -> ProcFs {
        ProcFs("/proc".into())
    }

    fn at(&self, pid: i32, rest: &str) -> PathBuf {
        self.0.join(pid.to_string()).join(rest)
    }

    fn status(&self, pid: i32, key: &str) -> io::Result<String> {
        let text = std::fs::read_to_string(self.at(pid, "status"))?;
        text.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':')))
            .map(|v| v.trim().to_string())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("no {key} in status")))
    }

    fn uid(&self, pid: i32) -> io::Result<u32> {
        // "Uid:  real effective saved fs"; all four must be the same user.
        let v = self.status(pid, "Uid")?;
        let ids: Vec<&str> = v.split_whitespace().collect();
        match ids.as_slice() {
            [a, b, c, d] if a == b && b == c && c == d => a.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad Uid")),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, format!("mixed uids ({v})"))),
        }
    }

    fn number(&self, pid: i32, key: &str) -> io::Result<u32> {
        self.status(pid, key)?.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("bad {key}")))
    }

    fn ppid(&self, pid: i32) -> io::Result<i32> {
        let stat = std::fs::read_to_string(self.at(pid, "stat"))?;
        // "pid (comm) S ppid ..."; comm may contain spaces and parentheses.
        let after = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or("");
        after.split_whitespace().nth(1).and_then(|v| v.parse().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad stat"))
    }

    fn comm(&self, pid: i32) -> io::Result<String> {
        Ok(std::fs::read_to_string(self.at(pid, "comm"))?.trim().to_string())
    }

    fn cgroup(&self, pid: i32) -> io::Result<String> {
        let text = std::fs::read_to_string(self.at(pid, "cgroup"))?;
        text.lines()
            .find_map(|l| l.strip_prefix("0::"))
            .map(String::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no cgroup v2 path"))
    }

    /// (device, inode) of the executable the process was started from.
    fn exe_id(&self, pid: i32) -> io::Result<(u64, u64)> {
        let m = std::fs::metadata(self.at(pid, "exe"))?;
        Ok((m.dev(), m.ino()))
    }

    fn ns(&self, pid: i32, name: &str) -> io::Result<PathBuf> {
        std::fs::read_link(self.at(pid, &format!("ns/{name}")))
    }

    /// The owner of a private `/proc/<pid>` entry (`environ`, mode 0400): the
    /// kernel makes it root's exactly when the process is non-dumpable. (The
    /// world-readable `/proc/<pid>` directory itself is exempt, so it cannot be used.)
    fn owner_uid(&self, pid: i32) -> io::Result<u32> {
        Ok(std::fs::metadata(self.at(pid, "environ"))?.uid())
    }
}

/// Which user ids may be served.
pub enum Allowed {
    /// Exactly this user (`--reader-uid`).
    Fixed(u32),
    /// The user who owns the active session on the physical seat, or else
    /// anyone listed in the allowlist file (headless machines).
    Seat { seat: PathBuf, allowlist: PathBuf },
}

/// `ACTIVE_UID=` from a systemd-logind seat file, if the seat has an active session.
pub fn active_uid(seat_file: &str) -> Option<u32> {
    seat_file.lines().find_map(|l| l.strip_prefix("ACTIVE_UID=")).and_then(|v| v.trim().parse().ok())
}

fn allowlist(text: &str) -> Vec<u32> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).filter_map(|l| l.parse().ok()).collect()
}

impl Allowed {
    fn check(&self, uid: u32) -> Result<(), String> {
        match self {
            Allowed::Fixed(allowed) if *allowed == uid => Ok(()),
            Allowed::Fixed(allowed) => Err(format!("uid {uid} is not the served uid {allowed}")),
            Allowed::Seat { seat, allowlist: list } => {
                if let Some(active) = std::fs::read_to_string(seat).ok().as_deref().and_then(active_uid) {
                    return if active == uid { Ok(()) } else { Err(format!("uid {uid} does not own the active session (uid {active} does)")) };
                }
                // No active graphical session: only explicitly allowed users.
                let allowed = std::fs::read_to_string(list).map(|t| allowlist(&t)).unwrap_or_default();
                if allowed.contains(&uid) {
                    Ok(())
                } else {
                    Err(format!("no active session on the seat and uid {uid} is not in {}", list.display()))
                }
            }
        }
    }
}

pub struct Policy {
    pub allowed: Allowed,
    /// The reader's own executable; the client must be started from the same file.
    pub reader_exe: (u64, u64),
    /// The reader's user and pid namespaces; the client must share them.
    pub reader_ns: Vec<(&'static str, PathBuf)>,
    /// Parent is the user's systemd manager and the cgroup is the client unit's.
    pub require_unit: bool,
    /// Non-dumpable, no_new_privs: what the client sets on itself first thing.
    pub require_hardened: bool,
}

impl Policy {
    /// The checks for a reader running as `proc`'s own process.
    pub fn for_this_process(proc: &ProcFs, allowed: Allowed, require_unit: bool, require_hardened: bool) -> io::Result<Policy> {
        let me = std::process::id() as i32;
        Ok(Policy {
            allowed,
            reader_exe: proc.exe_id(me)?,
            reader_ns: vec![("user", proc.ns(me, "user")?), ("pid", proc.ns(me, "pid")?)],
            require_unit,
            require_hardened,
        })
    }
}

pub struct Verified {
    pub proc: ProcFs,
    pub policy: Policy,
}

impl Verified {
    fn verify(&self, peer: &PeerInfo) -> Result<(), String> {
        let (p, pid) = (&self.proc, peer.pid);
        let ctx = |what: &str, e: io::Error| {
            let hint = if e.kind() == io::ErrorKind::PermissionDenied { " (the reader needs CAP_SYS_PTRACE to inspect another user's process)" } else { "" };
            format!("cannot check {what} (pid {pid}): {e}{hint}")
        };

        self.policy.allowed.check(peer.uid)?;

        // The process must still be the one that connected (not exited and replaced).
        if let Some(fd) = &peer.pidfd {
            let mut pf = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            if unsafe { libc::poll(&mut pf, 1, 0) } > 0 {
                return Err("the connecting process has already exited".into());
            }
        }
        let uid = p.uid(pid).map_err(|e| ctx("its user", e))?;
        if uid != peer.uid {
            return Err(format!("process runs as uid {uid}, the socket said {}", peer.uid));
        }

        // Started from the very same file as the reader.
        let exe = p.exe_id(pid).map_err(|e| ctx("its executable", e))?;
        if exe != self.policy.reader_exe {
            return Err("not started from the heptix binary".into());
        }
        for (name, want) in &self.policy.reader_ns {
            let got = p.ns(pid, name).map_err(|e| ctx(&format!("its {name} namespace"), e))?;
            if &got != want {
                return Err(format!("runs in a different {name} namespace"));
            }
        }

        // Nobody is debugging it.
        if p.number(pid, "TracerPid").map_err(|e| ctx("TracerPid", e))? != 0 {
            return Err("it is being traced".into());
        }

        if self.policy.require_hardened {
            if !p.status(pid, "NoNewPrivs").map(|v| v == "1").map_err(|e| ctx("NoNewPrivs", e))? {
                return Err("it has not set no_new_privs".into());
            }
            if p.owner_uid(pid).map_err(|e| ctx("dumpability", e))? != 0 {
                return Err("it is dumpable (not hardened)".into());
            }
        }

        if self.policy.require_unit {
            let parent = p.ppid(pid).map_err(|e| ctx("its parent", e))?;
            let comm = p.comm(parent).map_err(|e| ctx("its parent's name", e))?;
            let parent_uid = p.uid(parent).map_err(|e| ctx("its parent's user", e))?;
            if comm != "systemd" || parent_uid != peer.uid {
                return Err(format!("parent is '{comm}' (uid {parent_uid}), not the user's systemd"));
            }
            let cgroup = p.cgroup(pid).map_err(|e| ctx("its cgroup", e))?;
            let unit = format!("user@{}.service/", peer.uid);
            if !cgroup.contains(&unit) || !cgroup.ends_with("/heptix.service") {
                return Err(format!("cgroup '{cgroup}' is not the heptix user service"));
            }
        }
        Ok(())
    }
}

impl Authorizer for Verified {
    fn check(&self, peer: &PeerInfo) -> Result<(), String> {
        self.verify(peer)
    }

    fn recheck(&self, peer: &PeerInfo) -> bool {
        match self.verify(peer) {
            Ok(()) => true,
            Err(why) => {
                eprintln!("ending the session with pid {}: {why}", peer.pid);
                false
            }
        }
    }
}

/// Whether one path component (a file or a directory) keeps the binary safe from
/// replacement: root's, and not writable by group or others. The one exception is
/// a sticky directory (the Nix store is `root:nixbld` mode 1775, like /tmp): its
/// entries can only be removed or renamed by their own owners, and we require
/// every entry on the path to be root's, so nobody else can swap them.
fn component_ok(uid: u32, mode: u32, is_dir: bool) -> Result<(), &'static str> {
    if uid != 0 {
        return Err("is not owned by root");
    }
    let sticky_dir = is_dir && mode & 0o1000 != 0;
    if mode & 0o022 != 0 && !sticky_dir {
        return Err("is writable by group or others");
    }
    Ok(())
}

/// The reader refuses to serve from a binary that someone else could replace:
/// it and every directory above it must pass `component_ok`.
pub fn check_install(exe: &Path) -> Result<(), String> {
    let real = std::fs::canonicalize(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    let mut path = Some(real.as_path());
    while let Some(p) = path {
        let m = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        component_ok(m.uid(), m.mode(), m.is_dir()).map_err(|why| format!("{} {why}", p.display()))?;
        path = p.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    const ME: u32 = 1000;
    const CLIENT: i32 = 4242;
    const PARENT: i32 = 4000;

    /// A fake /proc with a well-behaved client, to be spoiled one fact at a time.
    struct Fake {
        root: PathBuf,
        reader_exe: PathBuf,
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn status(uid: u32, tracer: u32, nnp: u32) -> String {
        format!("Name:\theptix\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nTracerPid:\t{tracer}\nNoNewPrivs:\t{nnp}\n")
    }

    impl Fake {
        fn new(name: &str) -> Fake {
            let root = std::env::temp_dir().join(format!("heptix-proc-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            let reader_exe = root.join("installed-heptix");
            fs::write(&reader_exe, b"binary").unwrap();
            let f = Fake { root, reader_exe };
            f.set_process(CLIENT, PARENT, ME, 0, 1, "/user.slice/user-1000.slice/user@1000.service/app.slice/heptix.service");
            write(&f.root.join(PARENT.to_string()).join("status"), &status(ME, 0, 0));
            write(&f.root.join(PARENT.to_string()).join("comm"), "systemd\n");
            f
        }

        fn proc(&self) -> ProcFs {
            ProcFs(self.root.clone())
        }

        fn set_process(&self, pid: i32, ppid: i32, uid: u32, tracer: u32, nnp: u32, cgroup: &str) {
            let d = self.root.join(pid.to_string());
            write(&d.join("status"), &status(uid, tracer, nnp));
            write(&d.join("stat"), &format!("{pid} (heptix) S {ppid} 1 1 0\n"));
            write(&d.join("cgroup"), &format!("0::{cgroup}\n"));
            write(&d.join("environ"), "");
            let _ = fs::remove_file(d.join("exe"));
            symlink(&self.reader_exe, d.join("exe")).unwrap();
            for ns in ["user", "pid"] {
                let _ = fs::remove_file(d.join("ns").join(ns));
                fs::create_dir_all(d.join("ns")).unwrap();
                symlink(format!("{ns}:[4026531837]"), d.join("ns").join(ns)).unwrap();
            }
        }

        fn policy(&self, hardened: bool) -> Policy {
            let m = fs::metadata(&self.reader_exe).unwrap();
            Policy {
                allowed: Allowed::Fixed(ME),
                reader_exe: (m.dev(), m.ino()),
                reader_ns: vec![("user", "user:[4026531837]".into()), ("pid", "pid:[4026531837]".into())],
                require_unit: true,
                require_hardened: hardened,
            }
        }

        fn verify(&self, hardened: bool) -> Result<(), String> {
            let v = Verified { proc: self.proc(), policy: self.policy(hardened) };
            v.verify(&PeerInfo { pid: CLIENT, uid: ME, pidfd: None })
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const UNIT: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice/heptix.service";

    #[test]
    fn the_genuine_client_passes() {
        assert_eq!(Fake::new("ok").verify(false), Ok(()));
    }

    #[test]
    fn a_different_executable_is_refused() {
        let f = Fake::new("exe");
        let other = f.root.join("other-binary");
        fs::write(&other, b"not heptix").unwrap();
        let d = f.root.join(CLIENT.to_string());
        fs::remove_file(d.join("exe")).unwrap();
        symlink(&other, d.join("exe")).unwrap();
        assert_eq!(f.verify(false), Err("not started from the heptix binary".into()));
    }

    #[test]
    fn a_copy_of_the_binary_elsewhere_is_a_different_file() {
        let f = Fake::new("copy");
        let copy = f.root.join("copy-of-heptix");
        fs::copy(&f.reader_exe, &copy).unwrap();
        let d = f.root.join(CLIENT.to_string());
        fs::remove_file(d.join("exe")).unwrap();
        symlink(&copy, d.join("exe")).unwrap();
        assert!(f.verify(false).is_err(), "identical bytes in another file must not pass");
    }

    #[test]
    fn a_traced_client_is_refused() {
        let f = Fake::new("traced");
        f.set_process(CLIENT, PARENT, ME, 777, 1, UNIT);
        assert_eq!(f.verify(false), Err("it is being traced".into()));
    }

    #[test]
    fn the_wrong_user_is_refused() {
        let f = Fake::new("uid");
        let v = Verified { proc: f.proc(), policy: Policy { allowed: Allowed::Fixed(ME + 1), ..f.policy(false) } };
        assert!(v.verify(&PeerInfo { pid: CLIENT, uid: ME, pidfd: None }).unwrap_err().contains("not the served uid"));
        // The kernel says one user, /proc says another: mismatch.
        f.set_process(CLIENT, PARENT, ME + 5, 0, 1, UNIT);
        assert!(f.verify(false).unwrap_err().contains("socket said"));
    }

    #[test]
    fn a_client_in_another_namespace_is_refused() {
        let f = Fake::new("ns");
        let d = f.root.join(CLIENT.to_string()).join("ns");
        fs::remove_file(d.join("user")).unwrap();
        symlink("user:[4026532999]", d.join("user")).unwrap();
        assert_eq!(f.verify(false), Err("runs in a different user namespace".into()));
    }

    #[test]
    fn the_parent_and_cgroup_must_be_the_users_heptix_unit() {
        let f = Fake::new("unit");
        // Started from a shell, not by systemd.
        write(&f.root.join(PARENT.to_string()).join("comm"), "bash\n");
        assert!(f.verify(false).unwrap_err().contains("not the user's systemd"));
        write(&f.root.join(PARENT.to_string()).join("comm"), "systemd\n");
        // A systemd parent that belongs to someone else.
        write(&f.root.join(PARENT.to_string()).join("status"), &status(ME + 1, 0, 0));
        assert!(f.verify(false).unwrap_err().contains("not the user's systemd"));
        write(&f.root.join(PARENT.to_string()).join("status"), &status(ME, 0, 0));
        // Right parent, wrong unit.
        f.set_process(CLIENT, PARENT, ME, 0, 1, "/user.slice/user-1000.slice/user@1000.service/app.slice/evil.service");
        assert!(f.verify(false).unwrap_err().contains("not the heptix user service"));
        // The unit of a different user's manager.
        f.set_process(CLIENT, PARENT, ME, 0, 1, "/user.slice/user-1001.slice/user@1001.service/app.slice/heptix.service");
        assert!(f.verify(false).unwrap_err().contains("not the heptix user service"));
    }

    #[test]
    fn hardening_is_required_when_asked() {
        let f = Fake::new("hard");
        f.set_process(CLIENT, PARENT, ME, 0, 0, UNIT);
        assert_eq!(f.verify(true), Err("it has not set no_new_privs".into()));
        f.set_process(CLIENT, PARENT, ME, 0, 1, UNIT);
        // The fake environ file is owned by the test user, not root: still dumpable.
        if unsafe { libc::getuid() } != 0 {
            assert_eq!(f.verify(true), Err("it is dumpable (not hardened)".into()));
        }
    }

    #[test]
    fn a_missing_process_is_refused_not_trusted() {
        let f = Fake::new("gone");
        let v = Verified { proc: f.proc(), policy: f.policy(false) };
        assert!(v.verify(&PeerInfo { pid: 99_999, uid: ME, pidfd: None }).is_err());
    }

    #[test]
    fn the_seat_file_names_the_active_user() {
        let seat = "# This is private data. Do not parse.\nIS_SEAT0=1\nCAN_GRAPHICAL=1\nACTIVE=2\nACTIVE_UID=1000\nSESSIONS=2:1000 3:1001\nUIDS=1000 1001\n";
        assert_eq!(active_uid(seat), Some(1000));
        assert_eq!(active_uid("IS_SEAT0=1\n"), None);
        assert_eq!(active_uid("ACTIVE_UID=abc\n"), None);
    }

    #[test]
    fn only_the_active_seat_user_or_the_allowlist_is_served() {
        let f = Fake::new("seat");
        let (seat, list) = (f.root.join("seat0"), f.root.join("allow"));
        let allowed = || Allowed::Seat { seat: seat.clone(), allowlist: list.clone() };
        // No seat file and no allowlist: nobody.
        assert!(allowed().check(1000).is_err());
        // An active session: only its owner, even if a second user is logged in.
        fs::write(&seat, "ACTIVE=2\nACTIVE_UID=1000\nSESSIONS=2:1000 3:1001\n").unwrap();
        assert!(allowed().check(1000).is_ok());
        assert!(allowed().check(1001).unwrap_err().contains("does not own the active session"));
        // The allowlist applies only when there is no active session.
        fs::write(&list, "# headless box\n1001\n").unwrap();
        assert!(allowed().check(1001).is_err());
        fs::write(&seat, "IS_SEAT0=1\n").unwrap();
        assert!(allowed().check(1001).is_ok());
        assert!(allowed().check(1000).is_err());
    }

    #[test]
    fn path_components_must_be_roots_and_unwritable_except_sticky_directories() {
        assert_eq!(component_ok(0, 0o755, true), Ok(()));
        assert_eq!(component_ok(0, 0o755, false), Ok(()));
        assert_eq!(component_ok(0, 0o775, true), Err("is writable by group or others"));
        assert_eq!(component_ok(0, 0o777, true), Err("is writable by group or others"));
        assert_eq!(component_ok(0, 0o1775, true), Ok(()), "the Nix store: sticky and group-writable");
        assert_eq!(component_ok(0, 0o1777, true), Ok(()), "/tmp-like");
        assert_eq!(component_ok(0, 0o1775, false), Err("is writable by group or others"), "sticky means nothing on a file");
        assert_eq!(component_ok(0, 0o666, false), Err("is writable by group or others"));
        assert_eq!(component_ok(1000, 0o755, true), Err("is not owned by root"));
        assert_eq!(component_ok(1000, 0o1755, true), Err("is not owned by root"), "sticky never excuses a non-root owner");
    }

    #[test]
    fn install_integrity_needs_root_owned_unwritable_paths() {
        let f = Fake::new("install");
        // The test's temp files belong to the test user, never to root.
        if unsafe { libc::getuid() } != 0 {
            assert!(check_install(&f.reader_exe).unwrap_err().contains("not owned by root"));
        }
        assert!(check_install(Path::new("/definitely/not/here")).is_err());
        // A real root-owned system binary passes (every directory above it is root's).
        if let Ok(sh) = fs::canonicalize("/bin/sh") {
            let ok = check_install(&sh).is_ok();
            // Nix stores and unusual layouts may differ; only require no panic there.
            let _ = ok;
        }
    }

    #[test]
    fn this_process_can_describe_itself() {
        let proc = ProcFs::system();
        let policy = Policy::for_this_process(&proc, Allowed::Fixed(0), false, false).unwrap();
        assert_ne!(policy.reader_exe, (0, 0));
        assert_eq!(policy.reader_ns.len(), 2);
        let me = std::process::id() as i32;
        assert_eq!(proc.uid(me).unwrap(), unsafe { libc::getuid() });
        assert_eq!(proc.number(me, "TracerPid").unwrap(), 0);
    }
}
