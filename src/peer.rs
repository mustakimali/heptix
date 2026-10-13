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

//! Who is on the other end of a unix socket, as told by the kernel.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;

/// `SO_PEERPIDFD` (Linux 6.5): a pidfd for the peer, immune to PID reuse.
const SO_PEERPIDFD: libc::c_int = 77;

#[derive(Debug)]
pub struct PeerInfo {
    pub pid: i32,
    pub uid: u32,
    /// Pins the peer process; `None` on kernels without `SO_PEERPIDFD`.
    pub pidfd: Option<OwnedFd>,
}

/// The pid a pidfd refers to, from `/proc/self/fdinfo`.
fn pid_of_pidfd(fd: &OwnedFd) -> Option<i32> {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd())).ok()?;
    info.lines().find_map(|l| l.strip_prefix("Pid:")).and_then(|v| v.trim().parse().ok()).filter(|p| *p > 0)
}

/// Credentials of the process that connected (or that this stream connected to).
pub fn peer_info(stream: &UnixStream) -> io::Result<PeerInfo> {
    let fd = stream.as_raw_fd();
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pidfd: libc::c_int = -1;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let got = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, SO_PEERPIDFD, (&mut pidfd as *mut libc::c_int).cast(), &mut len) } == 0;
    let pidfd = got.then(|| unsafe { OwnedFd::from_raw_fd(pidfd) });
    // Prefer the pid the pidfd pins; fall back to the one in the credentials.
    let pid = pidfd.as_ref().and_then(pid_of_pidfd).unwrap_or(cred.pid);
    Ok(PeerInfo { pid, uid: cred.uid, pidfd })
}

/// The numeric user id of a named user, if it exists.
pub fn lookup_uid(name: &str) -> Option<u32> {
    let cname = std::ffi::CString::new(name).ok()?;
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 4096];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwnam_r(cname.as_ptr(), &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut out) };
    (rc == 0 && !out.is_null()).then_some(pwd.pw_uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_info_of_a_local_socketpair_is_this_process() {
        let (a, _b) = UnixStream::pair().unwrap();
        let info = peer_info(&a).unwrap();
        assert_eq!(info.pid, std::process::id() as i32);
        assert_eq!(info.uid, unsafe { libc::getuid() });
    }

    #[test]
    fn root_exists_and_nonsense_does_not() {
        assert_eq!(lookup_uid("root"), Some(0));
        assert_eq!(lookup_uid("no-such-user-heptix-test"), None);
    }
}
