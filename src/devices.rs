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

//! Finds the input devices heptix listens to (touchpads, keyboards, mice) and
//! keeps finding them: new `/dev/input/event*` nodes are picked up as they
//! appear, so plugging in a mouse or a keyboard receiver just works.
//!
//! Double counting is handled here: a touchpad's own pointer node ("... Mouse")
//! is skipped, and the consumer in `main` drops repeats of the same event.

use crate::{keyboard, mouse, tap};
use evdev::Device;
use std::collections::HashSet;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How long a watcher waits for input before it checks whether it was told to stop.
const STOP_POLL_MS: i32 = 250;

/// Waits until `fd` is readable (or hung up), or `ms` passes. False on timeout.
pub fn wait_readable(fd: RawFd, ms: i32) -> bool {
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    unsafe { libc::poll(&mut p, 1, ms) > 0 }
}

/// Waits for events on a device, so its watcher can notice `stop`. True when
/// the device has something to read.
pub fn wait_for_events(dev: &Device, stop: &AtomicBool) -> Option<bool> {
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    Some(wait_readable(dev.as_raw_fd(), STOP_POLL_MS))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Msg {
    Pad(tap::Event),
    Key(keyboard::KeyEvent),
    Mouse(mouse::Event),
}

#[derive(Clone)]
pub struct Options {
    /// Watch only this node as the touchpad (`--device`) instead of auto-detecting.
    pub only_touchpad: Option<PathBuf>,
    pub keyboards: bool,
    pub mice: bool,
    pub max_ms: u64,
    pub scroll_ticks: i32,
    pub verbose: bool,
    pub hotplug: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Touchpad,
    Keyboard,
    Mouse,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Touchpad => "touchpad",
            Role::Keyboard => "keyboard",
            Role::Mouse => "mouse",
        }
    }
}

/// What heptix would do with a device, given the names of all touchpads present.
pub fn classify(d: &Device, path: &Path, opts: &Options, pad_names: &[String]) -> Option<Role> {
    if let Some(only) = &opts.only_touchpad {
        if only == path {
            return Some(Role::Touchpad);
        }
    } else if tap::is_touchpad(d) {
        return Some(Role::Touchpad);
    }
    if opts.keyboards && keyboard::is_keyboard(d) {
        return Some(Role::Keyboard);
    }
    if opts.mice && mouse::is_mouse(d) && !mouse::is_touchpad_sibling(d.name().unwrap_or(""), pad_names) {
        return Some(Role::Mouse);
    }
    None
}

/// Every readable device with the role heptix would give it, for `--list`.
pub fn list(opts: &Options) -> Vec<(PathBuf, String, Role)> {
    let found = sorted_devices();
    let pad_names = touchpad_names(&found);
    found
        .iter()
        .filter_map(|(path, d)| {
            classify(d, path, opts, &pad_names).map(|r| (path.clone(), d.name().unwrap_or("?").to_string(), r))
        })
        .collect()
}

/// Same key and direction from two different devices within this window is one event
/// seen twice (a remapper exposing the same keyboard again).
const KEY_DUPLICATE: Duration = Duration::from_millis(8);

/// Drops a key event that a *different* device just reported identically. Repeats from one
/// device are never dropped: two real presses of one key can arrive together when the
/// machine is busy, and they must both count.
#[derive(Default)]
pub struct KeyDupes {
    last: Option<(u16, bool, usize, Instant)>,
}

impl KeyDupes {
    pub fn accept(&mut self, device: usize, ev: keyboard::KeyEvent, now: Instant) -> bool {
        let duplicate = self
            .last
            .is_some_and(|(code, down, from, at)| code == ev.code && down == ev.down && from != device && now.duration_since(at) < KEY_DUPLICATE);
        if !duplicate {
            self.last = Some((ev.code, ev.down, device, now));
        }
        !duplicate
    }
}

struct Shared {
    tx: Sender<Msg>,
    key_dupes: Mutex<KeyDupes>,
    next_device: AtomicUsize,
    opts: Options,
    watched: Mutex<HashSet<PathBuf>>,
    stop: Arc<AtomicBool>,
}

/// Keeps the device watchers alive. Dropping it stops them all and closes the devices.
pub struct Handle {
    /// How many devices were attached when it started.
    pub attached: usize,
    stop: Arc<AtomicBool>,
}

impl Handle {
    /// A handle that watches nothing, with the flag that shows when it was dropped.
    #[cfg(test)]
    pub fn fake() -> (Handle, Arc<AtomicBool>) {
        let stop = Arc::new(AtomicBool::new(false));
        (Handle { attached: 0, stop: Arc::clone(&stop) }, stop)
    }

    #[cfg(test)]
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Attaches every matching device now, and keeps watching for new ones if
/// `opts.hotplug`, until the returned handle is dropped.
pub fn start(tx: Sender<Msg>, opts: Options) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared {
        tx,
        opts,
        watched: Mutex::new(HashSet::new()),
        stop: Arc::clone(&stop),
        key_dupes: Mutex::new(KeyDupes::default()),
        next_device: AtomicUsize::new(0),
    });
    let attached = scan(&shared);
    if let Some(only) = &shared.opts.only_touchpad {
        if !shared.watched.lock().unwrap().contains(only) {
            eprintln!("cannot open {} (missing, or no permission)", only.display());
        }
    }
    if shared.opts.hotplug {
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            let stop = Arc::clone(&shared.stop);
            watch_dir(Path::new("/dev/input"), SETTLE, RETRY, &stop, || {
                scan(&shared);
            })
        });
    }
    Handle { attached, stop }
}

/// Natural order, so event2 comes before event10.
fn sorted_devices() -> Vec<(PathBuf, Device)> {
    let mut found: Vec<(PathBuf, Device)> = evdev::enumerate().collect();
    found.sort_by_key(|(p, _)| (p.as_os_str().len(), p.clone()));
    found
}

fn touchpad_names(found: &[(PathBuf, Device)]) -> Vec<String> {
    found.iter().filter(|(_, d)| tap::is_touchpad(d)).filter_map(|(_, d)| d.name().map(String::from)).collect()
}

/// Attaches all not-yet-watched matching devices; returns how many.
fn scan(shared: &Arc<Shared>) -> usize {
    let found = sorted_devices();
    let pad_names = touchpad_names(&found);
    let mut attached = 0;
    for (path, dev) in found {
        if shared.watched.lock().unwrap().contains(&path) {
            continue;
        }
        if let Some(role) = classify(&dev, &path, &shared.opts, &pad_names) {
            attach(shared, role, path, dev);
            attached += 1;
        }
    }
    attached
}

fn attach(shared: &Arc<Shared>, role: Role, path: PathBuf, dev: Device) {
    shared.watched.lock().unwrap().insert(path.clone());
    eprintln!("watching {} ({}) as {}", path.display(), dev.name().unwrap_or("?"), role.name());
    let shared = Arc::clone(shared);
    thread::spawn(move || {
        let (tx, verbose) = (shared.tx.clone(), shared.opts.verbose);
        let stop = Arc::clone(&shared.stop);
        let device_id = shared.next_device.fetch_add(1, Ordering::Relaxed);
        let result = match role {
            Role::Touchpad => tap::watch(dev, shared.opts.max_ms, shared.opts.scroll_ticks, &stop, |event| {
                if verbose {
                    println!("{event:?}");
                }
                let _ = tx.send(Msg::Pad(event));
            }),
            Role::Keyboard => keyboard::watch(dev, &stop, |ev| {
                if shared.key_dupes.lock().unwrap().accept(device_id, ev, Instant::now()) {
                    let _ = tx.send(Msg::Key(ev));
                }
            }),
            Role::Mouse => mouse::watch(dev, &stop, |ev| {
                if verbose {
                    println!("{ev:?}");
                }
                let _ = tx.send(Msg::Mouse(ev));
            }),
        };
        match result {
            Err(e) if e.raw_os_error() == Some(libc::ENODEV) => eprintln!("{} disconnected", path.display()),
            Err(e) => eprintln!("{}: {e}", path.display()),
            Ok(()) => {}
        }
        // Forget it, so the same node is picked up again if the device returns.
        shared.watched.lock().unwrap().remove(&path);
    });
}

/// True if an inotify buffer mentions an `event*` node.
fn mentions_event_node(buf: &[u8]) -> bool {
    let mut off = 0;
    while off + 16 <= buf.len() {
        let len = u32::from_ne_bytes([buf[off + 12], buf[off + 13], buf[off + 14], buf[off + 15]]) as usize;
        let name = &buf[(off + 16).min(buf.len())..(off + 16 + len).min(buf.len())];
        if name.starts_with(b"event") {
            return true;
        }
        off += 16 + len;
    }
    false
}

/// Reads and discards whatever inotify already has queued.
fn drain(fd: libc::c_int) {
    let mut buf = [0u8; 4096];
    loop {
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut p, 1, 0) } <= 0 || unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) } <= 0 {
            return;
        }
    }
}

/// Wait after a change before rescanning, so sibling nodes can appear.
const SETTLE: Duration = Duration::from_millis(300);
/// Second rescan after this, because udev grants the `input` group access a
/// moment after a node is created.
const RETRY: Duration = Duration::from_millis(1500);

/// Calls `rescan` whenever `dir` gets or changes an `event*` node. New nodes and the later permission
/// change (udev gives the `input` group access a moment after creation) both
/// trigger it, and sibling nodes of one device appear within milliseconds of
/// each other, so changes are debounced.
fn watch_dir(dir: &Path, settle: Duration, retry: Duration, stop: &AtomicBool, rescan: impl Fn()) {
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    let c_dir = CString::new(dir.as_os_str().as_encoded_bytes()).unwrap();
    if fd < 0 || unsafe { libc::inotify_add_watch(fd, c_dir.as_ptr(), libc::IN_CREATE | libc::IN_ATTRIB) } < 0 {
        eprintln!("hotplug disabled: cannot watch {}", dir.display());
        return;
    }
    let mut buf = [0u8; 4096];
    loop {
        if stop.load(Ordering::Relaxed) {
            unsafe { libc::close(fd) };
            return;
        }
        if !wait_readable(fd, STOP_POLL_MS) {
            continue;
        }
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if n <= 0 {
            return;
        }
        if !mentions_event_node(&buf[..n as usize]) {
            continue;
        }
        thread::sleep(settle);
        drain(fd);
        rescan();
        thread::sleep(retry);
        drain(fd);
        rescan();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &[u8]) -> Vec<u8> {
        let padded = (name.len() + 1).div_ceil(4) * 4;
        let mut b = vec![0u8; 16];
        b[12..16].copy_from_slice(&(padded as u32).to_ne_bytes());
        b.extend_from_slice(name);
        b.resize(16 + padded, 0);
        b
    }

    #[test]
    fn inotify_buffers_are_parsed_for_event_nodes() {
        assert!(mentions_event_node(&event(b"event12")));
        assert!(!mentions_event_node(&event(b"mouse0")));
        assert!(!mentions_event_node(&event(b"by-id")));
        let mut both = event(b"js0");
        both.extend(event(b"event3"));
        assert!(mentions_event_node(&both));
        assert!(!mentions_event_node(&[]));
        assert!(!mentions_event_node(&[0u8; 7]));
    }

    #[test]
    fn new_event_nodes_trigger_a_debounced_double_rescan_and_other_files_do_not() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!("heptix-hotplug-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let (watch_path, counter) = (dir.clone(), Arc::clone(&count));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let watcher = thread::spawn(move || {
            watch_dir(&watch_path, Duration::from_millis(40), Duration::from_millis(60), &thread_stop, || {
                counter.fetch_add(1, Ordering::SeqCst);
            })
        });
        thread::sleep(Duration::from_millis(100)); // let the watch start
        std::fs::write(dir.join("mouse0"), b"").unwrap();
        std::fs::write(dir.join("js0"), b"").unwrap();
        thread::sleep(Duration::from_millis(300));
        assert_eq!(count.load(Ordering::SeqCst), 0, "unrelated nodes must not rescan");
        // A burst of sibling nodes (as one device creates) is one debounced rescan pair.
        for n in ["event20", "event21", "event22"] {
            std::fs::write(dir.join(n), b"").unwrap();
        }
        thread::sleep(Duration::from_millis(500));
        assert_eq!(count.load(Ordering::SeqCst), 2);
        // The same node changing (udev permissions) triggers again.
        std::fs::set_permissions(dir.join("event20"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        thread::sleep(Duration::from_millis(500));
        assert_eq!(count.load(Ordering::SeqCst), 4);
        // Telling it to stop ends the watcher within a poll interval.
        stop.store(true, Ordering::Relaxed);
        watcher.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_dupes_drop_a_second_device_but_never_repeats_from_one_device() {
        let press = keyboard::KeyEvent { code: 30, down: true };
        let t = Instant::now();
        let at = |ms: u64| t + Duration::from_millis(ms);
        let mut d = KeyDupes::default();
        assert!(d.accept(0, press, at(0)));
        assert!(!d.accept(1, press, at(3)), "the same event from another device within 8 ms is a duplicate");
        assert!(d.accept(1, press, at(20)), "later it is a new press");
        // One device pressing the same key several times, all delivered together:
        let mut d = KeyDupes::default();
        for i in 0..5 {
            assert!(d.accept(0, press, at(i)), "press {i} from one device must count");
        }
        // A different key or direction is never a duplicate.
        let mut d = KeyDupes::default();
        assert!(d.accept(0, press, at(0)));
        assert!(d.accept(1, keyboard::KeyEvent { code: 31, down: true }, at(1)));
        assert!(d.accept(1, keyboard::KeyEvent { code: 30, down: false }, at(2)));
    }

    #[test]
    fn roles_have_names() {
        assert_eq!(Role::Mouse.name(), "mouse");
        assert_eq!(Role::Touchpad.name(), "touchpad");
    }
}
