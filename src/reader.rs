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

//! The reader service: reads the input devices and streams events over a unix
//! socket to one verified client at a time.
//!
//! It runs as a dedicated user in the `input` group, so that no other program
//! of the person at the keyboard needs that group. Who may connect is decided
//! by an `Authorizer`; everything else is refused before a byte is sent.

use crate::devices::{self, Msg};
use crate::mouse;
use crate::peer::{self, PeerInfo};
use crate::proto::{self, Subscribe};
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How long a new connection has to send its subscription.
const HANDSHAKE: Duration = Duration::from_secs(3);
/// A client that cannot take events for this long is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// How often an established connection is re-checked.
const RECHECK: Duration = Duration::from_secs(2);

/// Decides who is served.
pub trait Authorizer: Send + Sync + 'static {
    /// Called once per connection, before anything is read from it.
    fn check(&self, peer: &PeerInfo) -> Result<(), String>;

    /// Called every few seconds for a connected client; false ends the session.
    fn recheck(&self, _peer: &PeerInfo) -> bool {
        true
    }
}

/// Serves a single user id. Used with `--reader-uid` and in tests.
pub struct AllowUid(pub u32);

impl Authorizer for AllowUid {
    fn check(&self, peer: &PeerInfo) -> Result<(), String> {
        if peer.uid == self.0 {
            Ok(())
        } else {
            Err(format!("uid {} is not the allowed uid {}", peer.uid, self.0))
        }
    }
}

/// Starts the devices for a subscription and keeps them alive while the handle lives.
pub type Source = dyn Fn(Subscribe, Sender<Msg>) -> devices::Handle + Send + Sync;

/// Binds the socket (replacing a stale one) so any local user can try to connect;
/// the authorizer decides who is served.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
    Ok(listener)
}

/// Counts refusals so a flood of connection attempts cannot flood the log.
#[derive(Default)]
struct RefusalLog(AtomicU64);

impl RefusalLog {
    fn note(&self, peer: &PeerInfo, why: &str) {
        let n = self.0.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 5 || n.is_multiple_of(100) {
            eprintln!("refused connection from pid {} uid {} ({n} so far): {why}", peer.pid, peer.uid);
        }
    }
}

struct Shared {
    auth: Arc<dyn Authorizer>,
    source: Arc<Source>,
    /// The one live session: its id and a handle to shut it down.
    session: Mutex<Option<(u64, UnixStream)>>,
    next_id: AtomicU64,
    refusals: RefusalLog,
    stop: Arc<AtomicBool>,
}

/// Accepts connections until `stop` is set. Blocks.
pub fn serve(listener: UnixListener, auth: Arc<dyn Authorizer>, source: Arc<Source>, stop: Arc<AtomicBool>) {
    let shared = Arc::new(Shared {
        auth,
        source,
        session: Mutex::new(None),
        next_id: AtomicU64::new(1),
        refusals: RefusalLog::default(),
        stop,
    });
    let fd = std::os::fd::AsRawFd::as_raw_fd(&listener);
    while !shared.stop.load(Ordering::Relaxed) {
        if !devices::wait_readable(fd, 250) {
            continue;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = Arc::clone(&shared);
                thread::spawn(move || handle(stream, &shared));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => eprintln!("accept: {e}"),
        }
    }
    // Close the live session so its devices are released.
    let live = shared.session.lock().unwrap().take();
    if let Some((_, s)) = live {
        let _ = s.shutdown(std::net::Shutdown::Both);
    }
}

fn handle(stream: UnixStream, shared: &Shared) {
    let peer = match peer::peer_info(&stream) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot identify a connecting process: {e}");
            return;
        }
    };
    if let Err(why) = shared.auth.check(&peer) {
        shared.refusals.note(&peer, &why);
        return;
    }
    let _ = stream.set_read_timeout(Some(HANDSHAKE));
    let sub = match proto::read_frame(&mut &stream).map_err(|e| e.to_string()).and_then(|p| proto::decode_subscribe(&p).map_err(|e| e.to_string())) {
        Ok(s) => s,
        Err(why) => {
            shared.refusals.note(&peer, &format!("bad subscription: {why}"));
            return;
        }
    };
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    // One session at a time: a new verified client replaces the old one (a restart).
    let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
    let Ok(shutdown) = stream.try_clone() else { return };
    if let Some((_, old)) = shared.session.lock().unwrap().replace((id, shutdown)) {
        let _ = old.shutdown(std::net::Shutdown::Both);
    }
    let result = session(&stream, &peer, sub, shared);
    if let Err(e) = result {
        if e.kind() != io::ErrorKind::BrokenPipe && e.kind() != io::ErrorKind::UnexpectedEof {
            eprintln!("session {id} ended: {e}");
        }
    }
    let mut slot = shared.session.lock().unwrap();
    if slot.as_ref().is_some_and(|(sid, _)| *sid == id) {
        *slot = None;
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn session(stream: &UnixStream, peer: &PeerInfo, sub: Subscribe, shared: &Shared) -> io::Result<()> {
    let mut out = stream;
    proto::write_frame(&mut out, &proto::hello())?;
    let (tx, rx) = mpsc::channel::<Msg>();
    // Devices are opened only now, for a verified and subscribed client, and
    // closed when this function returns and the handle drops.
    let _devices = (shared.source)(sub, tx);

    // The client sends nothing after subscribing, so a read ending means it left.
    let alive = Arc::new(AtomicBool::new(true));
    {
        let (alive, mut reader) = (Arc::clone(&alive), stream.try_clone()?);
        thread::spawn(move || {
            let mut sink = [0u8; 64];
            while reader.read(&mut sink).is_ok_and(|n| n > 0) {}
            alive.store(false, Ordering::Relaxed);
        });
    }

    let mut mouse_filter = mouse::Filter::default();
    let mut last_check = Instant::now();
    while alive.load(Ordering::Relaxed) && !shared.stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(msg) => {
                let now = Instant::now();
                let keep = match msg {
                    // Duplicate keyboards are filtered where the events are read (devices.rs).
                    Msg::Key(_) => true,
                    Msg::Mouse(ev) => mouse_filter.accept(ev, now),
                    Msg::Pad(_) => true,
                };
                if keep {
                    proto::write_frame(&mut out, &proto::event(&msg))?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if last_check.elapsed() >= RECHECK {
            last_check = Instant::now();
            if !shared.auth.recheck(peer) {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "client no longer passes the checks"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::KeyEvent;
    use crate::proto::ToClient;

    fn temp_socket(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("heptix-reader-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("events.sock")
    }

    fn me() -> u32 {
        unsafe { libc::getuid() }
    }

    struct Fixture {
        path: std::path::PathBuf,
        stop: Arc<AtomicBool>,
        subs: Arc<Mutex<Vec<Subscribe>>>,
        /// Stop flags of the device handles that were created, in order.
        handles: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
    }

    impl Fixture {
        /// A reader whose "devices" send the given messages as soon as they are opened.
        fn start(name: &str, auth: impl Authorizer, script: Vec<Msg>) -> Fixture {
            let path = temp_socket(name);
            let listener = bind(&path).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let subs = Arc::new(Mutex::new(Vec::new()));
            let handles = Arc::new(Mutex::new(Vec::new()));
            let (s2, h2) = (Arc::clone(&subs), Arc::clone(&handles));
            let source: Arc<Source> = Arc::new(move |sub, tx| {
                s2.lock().unwrap().push(sub);
                for m in &script {
                    tx.send(*m).unwrap();
                }
                let (handle, flag) = devices::Handle::fake();
                h2.lock().unwrap().push(flag);
                // Keep the channel open like real watchers do, until the handle drops.
                let alive = Arc::clone(&handle.stop_flag());
                thread::spawn(move || {
                    while !alive.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(20));
                    }
                    drop(tx);
                });
                handle
            });
            let stop2 = Arc::clone(&stop);
            thread::spawn(move || serve(listener, Arc::new(auth), source, stop2));
            Fixture { path, stop, subs, handles }
        }

        fn connect(&self, sub: &Subscribe) -> UnixStream {
            let mut s = UnixStream::connect(&self.path).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            io::Write::write_all(&mut s, &proto::subscribe(sub)).unwrap();
            s
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = std::fs::remove_dir_all(self.path.parent().unwrap());
        }
    }

    fn sub() -> Subscribe {
        Subscribe { keyboards: true, mice: true, max_ms: 200, scroll_ticks: 24, only_touchpad: None }
    }

    fn next(s: &mut UnixStream) -> io::Result<ToClient> {
        let p = proto::read_frame(s)?;
        proto::decode_to_client(&p).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }

    #[test]
    fn an_allowed_client_gets_hello_then_events_in_order() {
        let script = vec![
            Msg::Key(KeyEvent { code: 30, down: true }),
            Msg::Key(KeyEvent { code: 30, down: false }),
            Msg::Mouse(mouse::Event::Wheel),
        ];
        let fx = Fixture::start("ok", AllowUid(me()), script.clone());
        let mut c = fx.connect(&sub());
        assert_eq!(next(&mut c).unwrap(), ToClient::Hello(proto::VERSION));
        for m in script {
            assert_eq!(next(&mut c).unwrap(), ToClient::Event(m));
        }
        assert_eq!(fx.subs.lock().unwrap().as_slice(), &[sub()]);
    }

    #[test]
    fn a_client_that_is_not_allowed_gets_nothing_and_no_devices_are_opened() {
        let fx = Fixture::start("refuse", AllowUid(me() + 1), vec![Msg::Mouse(mouse::Event::Wheel)]);
        let mut c = fx.connect(&sub());
        assert!(next(&mut c).is_err(), "connection must close without data");
        assert!(fx.subs.lock().unwrap().is_empty());
    }

    #[test]
    fn a_bad_subscription_is_refused_before_any_device_is_opened() {
        let fx = Fixture::start("badsub", AllowUid(me()), vec![]);
        let mut c = fx.connect(&Subscribe { only_touchpad: Some("/etc/shadow".into()), ..sub() });
        assert!(next(&mut c).is_err());
        assert!(fx.subs.lock().unwrap().is_empty());
        // Garbage instead of a frame.
        let mut g = UnixStream::connect(&fx.path).unwrap();
        g.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        io::Write::write_all(&mut g, &[3, 0x55, 1, 2]).unwrap();
        assert!(next(&mut g).is_err());
        assert!(fx.subs.lock().unwrap().is_empty());
    }

    #[test]
    fn quick_repeated_presses_of_one_key_are_all_forwarded() {
        // Real presses can reach the reader together when the machine is busy.
        let press = Msg::Key(KeyEvent { code: 57, down: true });
        let fx = Fixture::start("repeat", AllowUid(me()), vec![press, press, press, Msg::Mouse(mouse::Event::Wheel)]);
        let mut c = fx.connect(&sub());
        assert_eq!(next(&mut c).unwrap(), ToClient::Hello(proto::VERSION));
        for _ in 0..3 {
            assert_eq!(next(&mut c).unwrap(), ToClient::Event(press));
        }
        assert_eq!(next(&mut c).unwrap(), ToClient::Event(Msg::Mouse(mouse::Event::Wheel)));
    }

    #[test]
    fn devices_are_released_when_the_client_leaves() {
        let fx = Fixture::start("leave", AllowUid(me()), vec![]);
        let mut c = fx.connect(&sub());
        assert_eq!(next(&mut c).unwrap(), ToClient::Hello(proto::VERSION));
        drop(c);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !fx.handles.lock().unwrap().first().is_some_and(|f| f.load(Ordering::Relaxed)) {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(fx.handles.lock().unwrap()[0].load(Ordering::Relaxed), "the device handle must be dropped");
    }

    #[test]
    fn a_new_verified_client_replaces_the_old_one() {
        let fx = Fixture::start("replace", AllowUid(me()), vec![]);
        let mut first = fx.connect(&sub());
        assert_eq!(next(&mut first).unwrap(), ToClient::Hello(proto::VERSION));
        let mut second = fx.connect(&sub());
        assert_eq!(next(&mut second).unwrap(), ToClient::Hello(proto::VERSION));
        assert!(next(&mut first).is_err(), "the old session must be closed");
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !fx.handles.lock().unwrap().first().is_some_and(|f| f.load(Ordering::Relaxed)) {
            thread::sleep(Duration::from_millis(20));
        }
        let handles = fx.handles.lock().unwrap();
        assert_eq!(handles.len(), 2);
        assert!(handles[0].load(Ordering::Relaxed), "the old session's devices must be released");
        assert!(!handles[1].load(Ordering::Relaxed));
    }

    #[test]
    fn rechecks_can_end_a_session() {
        struct Revoked(AtomicBool);
        impl Authorizer for Revoked {
            fn check(&self, _: &PeerInfo) -> Result<(), String> {
                Ok(())
            }
            fn recheck(&self, _: &PeerInfo) -> bool {
                !self.0.load(Ordering::Relaxed)
            }
        }
        let auth = Arc::new(Revoked(AtomicBool::new(false)));
        struct Shim(Arc<Revoked>);
        impl Authorizer for Shim {
            fn check(&self, p: &PeerInfo) -> Result<(), String> {
                self.0.check(p)
            }
            fn recheck(&self, p: &PeerInfo) -> bool {
                self.0.recheck(p)
            }
        }
        let fx = Fixture::start("recheck", Shim(Arc::clone(&auth)), vec![]);
        let mut c = fx.connect(&sub());
        assert_eq!(next(&mut c).unwrap(), ToClient::Hello(proto::VERSION));
        auth.0.store(true, Ordering::Relaxed);
        c.set_read_timeout(Some(Duration::from_secs(6))).unwrap();
        assert!(next(&mut c).is_err(), "revoked client must be disconnected within the recheck interval");
    }
}
