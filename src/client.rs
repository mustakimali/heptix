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

//! The client side of the reader socket: connects, checks that the server is
//! the heptix reader, subscribes and feeds the events into the same channel
//! the direct device watchers use.

use crate::devices::Msg;
use crate::peer;
use crate::proto::{self, FrameBuf, Subscribe, ToClient};
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// The reader runs as this user; nothing else may serve us events.
pub const READER_USER: &str = "heptix";
pub const DEFAULT_SOCKET: &str = "/run/heptix/events.sock";

pub struct Config {
    pub socket: PathBuf,
    /// The user id the reader must run as; `None` means the `heptix` user.
    pub expected_uid: Option<u32>,
}

/// Keeps the connection alive; dropping it disconnects and stops reconnecting.
pub struct Handle {
    stop: Arc<AtomicBool>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Connects in the background, and reconnects whenever the reader goes away.
pub fn spawn(cfg: Config, sub: Subscribe, tx: Sender<Msg>) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    thread::spawn(move || {
        let (mut backoff, mut last_error) = (1u64, String::new());
        while !thread_stop.load(Ordering::Relaxed) {
            let connected_once = match session(&cfg, &sub, &tx, &thread_stop) {
                Ok(()) => true,
                Err(e) => {
                    let text = e.to_string();
                    if text != last_error {
                        eprintln!("reader: {text} (retrying)");
                        last_error = text;
                    }
                    false
                }
            };
            if connected_once {
                backoff = 1;
                last_error.clear();
            }
            for _ in 0..backoff * 10 {
                if thread_stop.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            backoff = (backoff * 2).min(5);
        }
    });
    Handle { stop }
}

/// The socket's directory must not let an ordinary user plant a fake reader.
fn check_socket_dir(path: &Path, expected_uid: u32) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let meta = std::fs::metadata(dir)?;
    if meta.uid() != 0 && meta.uid() != expected_uid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is owned by uid {}", dir.display(), meta.uid())));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is writable by others", dir.display())));
    }
    Ok(())
}

/// One connection, until it ends. `Ok` means we were connected and it ended normally.
fn session(cfg: &Config, sub: &Subscribe, tx: &Sender<Msg>, stop: &AtomicBool) -> io::Result<()> {
    let expected = cfg
        .expected_uid
        .or_else(|| peer::lookup_uid(READER_USER))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no '{READER_USER}' user on this system")))?;
    check_socket_dir(&cfg.socket, expected)?;
    let mut stream = UnixStream::connect(&cfg.socket)?;
    let server = peer::peer_info(&stream)?;
    if server.uid != expected {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the socket is served by uid {}, not the reader (uid {expected}); not connecting", server.uid),
        ));
    }
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let refused = |e: io::Error| match e.kind() {
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset => io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the reader refused the connection (it only serves the heptix user service; see its log)",
        ),
        _ => e,
    };
    stream.write_all(&proto::subscribe(sub)).map_err(refused)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;

    let (mut buf, mut chunk, mut greeted) = (FrameBuf::default(), [0u8; 256], false);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let n = match stream.read(&mut chunk) {
            Ok(0) => return if greeted { Ok(()) } else { Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the reader refused the connection")) },
            Ok(n) => n,
            // A timeout, or a signal handler that ran while we waited: just wait again.
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => continue,
            Err(e) => return Err(if greeted { e } else { refused(e) }),
        };
        buf.push(&chunk[..n]);
        while let Some(frame) = buf.next_frame() {
            match proto::decode_to_client(&frame).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))? {
                ToClient::Hello(v) if v == proto::VERSION => {
                    if !greeted {
                        eprintln!("reader: connected");
                    }
                    greeted = true;
                }
                ToClient::Hello(v) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("reader speaks protocol {v}, this is {}", proto::VERSION)));
                }
                ToClient::Event(msg) if greeted => {
                    if tx.send(msg).is_err() {
                        return Ok(()); // nobody is listening any more
                    }
                }
                ToClient::Event(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "event before hello")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices;
    use crate::keyboard::KeyEvent;
    use crate::mouse;
    use crate::reader::{self, AllowUid, Source};
    use std::sync::mpsc;
    use std::sync::Mutex;
    use std::time::Instant;

    fn me() -> u32 {
        unsafe { libc::getuid() }
    }

    fn sub() -> Subscribe {
        Subscribe { keyboards: true, mice: true, max_ms: 200, scroll_ticks: 24, only_touchpad: None }
    }

    /// A reader that pushes `script` to every subscriber.
    fn reader_with(name: &str, allow: u32, script: Vec<Msg>) -> (PathBuf, Arc<AtomicBool>) {
        let dir = std::env::temp_dir().join(format!("heptix-client-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.sock");
        let listener = reader::bind(&path).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let keep: Arc<Mutex<Vec<devices::Handle>>> = Arc::new(Mutex::new(Vec::new()));
        let source: Arc<Source> = Arc::new(move |_, tx| {
            for m in &script {
                tx.send(*m).unwrap();
            }
            let (handle, flag) = devices::Handle::fake();
            thread::spawn(move || {
                let _tx = tx; // keep the channel open until the handle drops
                while !flag.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(20));
                }
            });
            let _ = &keep;
            handle
        });
        let s2 = Arc::clone(&stop);
        thread::spawn(move || reader::serve(listener, Arc::new(AllowUid(allow)), source, s2));
        (path, stop)
    }

    #[test]
    fn events_from_the_reader_reach_the_channel() {
        let script = vec![Msg::Key(KeyEvent { code: 30, down: true }), Msg::Mouse(mouse::Event::Wheel)];
        let (path, stop) = reader_with("flow", me(), script.clone());
        let (tx, rx) = mpsc::channel();
        let _client = spawn(Config { socket: path, expected_uid: Some(me()) }, sub(), tx);
        for want in script {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), want);
        }
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn the_client_does_not_talk_to_a_server_that_is_not_the_reader() {
        // The server runs as us, but the client expects the reader to be a different user.
        let (path, stop) = reader_with("fake", me(), vec![Msg::Mouse(mouse::Event::Wheel)]);
        let (tx, rx) = mpsc::channel();
        let _client = spawn(Config { socket: path, expected_uid: Some(me() + 1) }, sub(), tx);
        assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err(), "no event may be accepted from a wrong server");
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn a_reader_that_refuses_us_delivers_nothing() {
        let (path, stop) = reader_with("refused", me() + 1, vec![Msg::Mouse(mouse::Event::Wheel)]);
        let (tx, rx) = mpsc::channel();
        let _client = spawn(Config { socket: path, expected_uid: Some(me()) }, sub(), tx);
        assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err());
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn it_reconnects_when_the_reader_comes_back() {
        let (path, stop) = reader_with("again", me(), vec![Msg::Mouse(mouse::Event::Wheel)]);
        let (tx, rx) = mpsc::channel();
        let _client = spawn(Config { socket: path.clone(), expected_uid: Some(me()) }, sub(), tx);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Msg::Mouse(mouse::Event::Wheel));
        // The reader restarts.
        stop.store(true, Ordering::Relaxed);
        thread::sleep(Duration::from_millis(600));
        let listener = reader::bind(&path).unwrap();
        let stop2 = Arc::new(AtomicBool::new(false));
        let source: Arc<Source> = Arc::new(|_, tx| {
            tx.send(Msg::Mouse(mouse::Event::Wheel)).unwrap();
            let (h, flag) = devices::Handle::fake();
            thread::spawn(move || {
                let _tx = tx;
                while !flag.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(20));
                }
            });
            h
        });
        let s2 = Arc::clone(&stop2);
        thread::spawn(move || reader::serve(listener, Arc::new(AllowUid(me())), source, s2));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = false;
        while Instant::now() < deadline && !got {
            got = rx.recv_timeout(Duration::from_millis(500)).is_ok();
        }
        stop2.store(true, Ordering::Relaxed);
        assert!(got, "the client must reconnect to the new reader");
    }

    #[test]
    fn a_world_writable_socket_directory_is_rejected() {
        let dir = std::env::temp_dir().join(format!("heptix-clientdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o777)).unwrap();
        assert!(check_socket_dir(&dir.join("s"), me()).is_err());
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert!(check_socket_dir(&dir.join("s"), me()).is_ok());
        assert!(check_socket_dir(&dir.join("s"), me() + 1).is_err(), "owned by someone else");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
