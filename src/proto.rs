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

//! The wire protocol between the reader (which reads the input devices) and
//! the client (which plays sounds and keeps stats), over a unix socket.
//!
//! A frame is one length byte followed by that many payload bytes; the first
//! payload byte is the message type. Everything is little-endian. Frames are
//! tiny and the stream is local, so there is no versioned handshake beyond
//! `Hello` and no compression or encryption.

use crate::devices::Msg;
use crate::keyboard::KeyEvent;
use crate::mouse;
use crate::tap;
use std::io::{self, Read, Write};
use std::path::PathBuf;

pub const VERSION: u8 = 1;
const MAX_PATH: usize = 64;

// Reader to client.
const T_HELLO: u8 = 0x01;
const T_KEY: u8 = 0x10;
const T_BUTTON: u8 = 0x11;
const T_WHEEL: u8 = 0x12;
const T_PAD_TAP: u8 = 0x20;
const T_PAD_CLICK: u8 = 0x21;
const T_PAD_SCROLL: u8 = 0x22;
const T_PAD_DISTANCE: u8 = 0x23;
// Client to reader.
const T_SUBSCRIBE: u8 = 0x80;

#[derive(Debug, PartialEq)]
pub enum ProtoError {
    Short,
    Unknown(u8),
    BadPath,
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoError::Short => write!(f, "truncated frame"),
            ProtoError::Unknown(t) => write!(f, "unknown message type {t:#x}"),
            ProtoError::BadPath => write!(f, "invalid device path"),
        }
    }
}

/// What the client asks the reader to send.
#[derive(Clone, Debug, PartialEq)]
pub struct Subscribe {
    pub keyboards: bool,
    pub mice: bool,
    /// Longest touch (ms) still counted as a tap.
    pub max_ms: u32,
    pub scroll_ticks: i32,
    /// Use this node as the touchpad instead of auto-detecting.
    pub only_touchpad: Option<PathBuf>,
}

/// What the reader sends the client.
#[derive(Debug, PartialEq)]
pub enum ToClient {
    Hello(u8),
    Event(Msg),
}

fn frame(payload: &[u8]) -> Vec<u8> {
    debug_assert!(payload.len() <= 255);
    let mut out = Vec::with_capacity(payload.len() + 1);
    out.push(payload.len() as u8);
    out.extend_from_slice(payload);
    out
}

pub fn hello() -> Vec<u8> {
    frame(&[T_HELLO, VERSION])
}

pub fn event(msg: &Msg) -> Vec<u8> {
    match *msg {
        Msg::Key(KeyEvent { code, down }) => {
            let c = code.to_le_bytes();
            frame(&[T_KEY, c[0], c[1], down as u8])
        }
        Msg::Mouse(mouse::Event::Button { code, down }) => {
            let c = code.to_le_bytes();
            frame(&[T_BUTTON, c[0], c[1], down as u8])
        }
        Msg::Mouse(mouse::Event::Wheel) => frame(&[T_WHEEL]),
        Msg::Pad(tap::Event::Tap) => frame(&[T_PAD_TAP]),
        Msg::Pad(tap::Event::Click) => frame(&[T_PAD_CLICK]),
        Msg::Pad(tap::Event::Scroll) => frame(&[T_PAD_SCROLL]),
        Msg::Pad(tap::Event::Distance(um)) => {
            let u = um.to_le_bytes();
            frame(&[T_PAD_DISTANCE, u[0], u[1], u[2], u[3]])
        }
    }
}

pub fn subscribe(s: &Subscribe) -> Vec<u8> {
    let mut p = vec![T_SUBSCRIBE, s.keyboards as u8 | (s.mice as u8) << 1];
    p.extend_from_slice(&s.max_ms.to_le_bytes());
    p.extend_from_slice(&s.scroll_ticks.to_le_bytes());
    let path = s.only_touchpad.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = &path.as_bytes()[..path.len().min(MAX_PATH)];
    p.push(bytes.len() as u8);
    p.extend_from_slice(bytes);
    frame(&p)
}

/// Reads one frame's payload (type byte first).
pub fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 1];
    r.read_exact(&mut len)?;
    let mut payload = vec![0u8; len[0] as usize];
    r.read_exact(&mut payload)?;
    if payload.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty frame"));
    }
    Ok(payload)
}

/// Reassembles frames from a byte stream that may arrive in pieces.
#[derive(Default)]
pub struct FrameBuf {
    buf: Vec<u8>,
}

impl FrameBuf {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame's payload, if one has fully arrived.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        let len = *self.buf.first()? as usize;
        if self.buf.len() < 1 + len {
            return None;
        }
        let payload = self.buf[1..1 + len].to_vec();
        self.buf.drain(..1 + len);
        Some(payload)
    }
}

pub fn write_frame(w: &mut impl Write, frame: &[u8]) -> io::Result<()> {
    w.write_all(frame)
}

fn u16_at(p: &[u8], i: usize) -> Result<u16, ProtoError> {
    p.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or(ProtoError::Short)
}

fn u32_at(p: &[u8], i: usize) -> Result<u32, ProtoError> {
    p.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or(ProtoError::Short)
}

pub fn decode_to_client(p: &[u8]) -> Result<ToClient, ProtoError> {
    let down = |i: usize| p.get(i).map(|b| *b != 0).ok_or(ProtoError::Short);
    Ok(match *p.first().ok_or(ProtoError::Short)? {
        T_HELLO => ToClient::Hello(*p.get(1).ok_or(ProtoError::Short)?),
        T_KEY => ToClient::Event(Msg::Key(KeyEvent { code: u16_at(p, 1)?, down: down(3)? })),
        T_BUTTON => ToClient::Event(Msg::Mouse(mouse::Event::Button { code: u16_at(p, 1)?, down: down(3)? })),
        T_WHEEL => ToClient::Event(Msg::Mouse(mouse::Event::Wheel)),
        T_PAD_TAP => ToClient::Event(Msg::Pad(tap::Event::Tap)),
        T_PAD_CLICK => ToClient::Event(Msg::Pad(tap::Event::Click)),
        T_PAD_SCROLL => ToClient::Event(Msg::Pad(tap::Event::Scroll)),
        T_PAD_DISTANCE => ToClient::Event(Msg::Pad(tap::Event::Distance(u32_at(p, 1)?))),
        other => return Err(ProtoError::Unknown(other)),
    })
}

/// `/dev/input/event<digits>` and nothing else, so a client cannot point the
/// reader at an arbitrary file.
fn valid_device_path(path: &str) -> bool {
    path.strip_prefix("/dev/input/event").is_some_and(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
}

pub fn decode_subscribe(p: &[u8]) -> Result<Subscribe, ProtoError> {
    match p.first() {
        Some(&T_SUBSCRIBE) => {}
        Some(&other) => return Err(ProtoError::Unknown(other)),
        None => return Err(ProtoError::Short),
    }
    let flags = *p.get(1).ok_or(ProtoError::Short)?;
    let max_ms = u32_at(p, 2)?;
    let scroll_ticks = u32_at(p, 6)? as i32;
    let n = *p.get(10).ok_or(ProtoError::Short)? as usize;
    if n > MAX_PATH {
        return Err(ProtoError::BadPath);
    }
    let only_touchpad = if n == 0 {
        None
    } else {
        let raw = p.get(11..11 + n).ok_or(ProtoError::Short)?;
        let path = std::str::from_utf8(raw).map_err(|_| ProtoError::BadPath)?;
        if !valid_device_path(path) {
            return Err(ProtoError::BadPath);
        }
        Some(PathBuf::from(path))
    };
    Ok(Subscribe {
        keyboards: flags & 1 != 0,
        mice: flags & 2 != 0,
        max_ms: max_ms.clamp(50, 1000),
        scroll_ticks: scroll_ticks.clamp(0, 200),
        only_touchpad,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(m: Msg) {
        let f = event(&m);
        assert_eq!(f[0] as usize, f.len() - 1);
        assert_eq!(decode_to_client(&f[1..]).unwrap(), ToClient::Event(m));
    }

    #[test]
    fn every_event_roundtrips() {
        for m in [
            Msg::Key(KeyEvent { code: 57, down: true }),
            Msg::Key(KeyEvent { code: 0x2ff, down: false }),
            Msg::Mouse(mouse::Event::Button { code: 0x110, down: true }),
            Msg::Mouse(mouse::Event::Wheel),
            Msg::Pad(tap::Event::Tap),
            Msg::Pad(tap::Event::Click),
            Msg::Pad(tap::Event::Scroll),
            Msg::Pad(tap::Event::Distance(123_456)),
        ] {
            roundtrip(m);
        }
        assert_eq!(decode_to_client(&hello()[1..]).unwrap(), ToClient::Hello(VERSION));
    }

    #[test]
    fn subscribe_roundtrips_and_clamps() {
        let s = Subscribe { keyboards: true, mice: false, max_ms: 200, scroll_ticks: 24, only_touchpad: Some("/dev/input/event13".into()) };
        let f = subscribe(&s);
        assert_eq!(decode_subscribe(&f[1..]).unwrap(), s);
        let none = Subscribe { keyboards: false, mice: true, max_ms: 5, scroll_ticks: 9999, only_touchpad: None };
        let back = decode_subscribe(&subscribe(&none)[1..]).unwrap();
        assert_eq!((back.max_ms, back.scroll_ticks, back.mice, back.keyboards), (50, 200, true, false));
    }

    #[test]
    fn subscribe_rejects_paths_that_are_not_event_nodes() {
        for bad in ["/etc/shadow", "/dev/input/event", "/dev/input/event1/../../x", "/dev/input/mouse0", "relative", "/dev/input/event12x"] {
            let f = subscribe(&Subscribe { keyboards: false, mice: false, max_ms: 200, scroll_ticks: 0, only_touchpad: Some(bad.into()) });
            assert_eq!(decode_subscribe(&f[1..]), Err(ProtoError::BadPath), "{bad}");
        }
    }

    #[test]
    fn malformed_frames_are_errors_not_panics() {
        assert_eq!(decode_to_client(&[]), Err(ProtoError::Short));
        assert_eq!(decode_to_client(&[T_KEY, 1]), Err(ProtoError::Short));
        assert_eq!(decode_to_client(&[0x77]), Err(ProtoError::Unknown(0x77)));
        assert_eq!(decode_subscribe(&[T_SUBSCRIBE, 0, 0]), Err(ProtoError::Short));
        assert_eq!(decode_subscribe(&[T_KEY]), Err(ProtoError::Unknown(T_KEY)));
        // Declared path longer than the frame.
        let mut f = vec![T_SUBSCRIBE, 0, 0, 0, 0, 0, 0, 0, 0, 0, 30];
        f.extend(b"abc");
        assert_eq!(decode_subscribe(&f), Err(ProtoError::Short));
        // A length byte larger than the data available.
        assert!(read_frame(&mut &[5u8, 1, 2][..]).is_err());
        assert!(read_frame(&mut &[0u8][..]).is_err());
    }

    #[test]
    fn frame_buf_reassembles_split_frames() {
        let mut bytes = event(&Msg::Key(KeyEvent { code: 30, down: true }));
        bytes.extend(event(&Msg::Mouse(mouse::Event::Wheel)));
        let mut fb = FrameBuf::default();
        let mut got = Vec::new();
        for chunk in bytes.chunks(2) {
            fb.push(chunk);
            while let Some(f) = fb.next_frame() {
                got.push(decode_to_client(&f).unwrap());
            }
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[1], ToClient::Event(Msg::Mouse(mouse::Event::Wheel)));
        assert!(fb.next_frame().is_none());
    }

    #[test]
    fn frames_stream_back_to_back() {
        let mut bytes = hello();
        bytes.extend(event(&Msg::Mouse(mouse::Event::Wheel)));
        bytes.extend(event(&Msg::Key(KeyEvent { code: 30, down: true })));
        let mut r = &bytes[..];
        let kinds: Vec<ToClient> = (0..3).map(|_| decode_to_client(&read_frame(&mut r).unwrap()).unwrap()).collect();
        assert_eq!(kinds[0], ToClient::Hello(VERSION));
        assert_eq!(kinds[2], ToClient::Event(Msg::Key(KeyEvent { code: 30, down: true })));
        assert!(read_frame(&mut r).is_err());
    }
}
