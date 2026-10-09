use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode, PropType};
use std::io;
use std::time::{Duration, Instant};

/// A touchpad reports finger position plus BTN_TOOL_FINGER, and is not a touchscreen.
pub fn is_touchpad(d: &Device) -> bool {
    let keys = d.supported_keys();
    let abs = d.supported_absolute_axes();
    let finger = keys.is_some_and(|k| k.contains(KeyCode::BTN_TOOL_FINGER));
    let xy = abs.is_some_and(|a| {
        a.contains(AbsoluteAxisCode::ABS_X) && a.contains(AbsoluteAxisCode::ABS_Y)
    });
    finger && xy && !d.properties().contains(PropType::DIRECT)
}

/// Fraction of the pad's width a finger may travel and still count as a tap.
const MOVE_FRACTION: i32 = 40; // 1/40 = 2.5%

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Tap,
    /// One scroll "detent" of a two-finger scroll.
    Scroll,
}

/// Shortest gap between scroll ticks, so a fast fling does not become a buzz.
const MIN_SCROLL_GAP: Duration = Duration::from_millis(18);

struct Touch {
    start: Instant,
    x0: Option<i32>,
    y0: Option<i32>,
    moved: bool,
    clicked: bool,
}

/// Blocks, reporting each tap (a short touch that barely moves), each physical
/// button press, and, if `scroll_ticks` > 0, a `Scroll` for every
/// 1/`scroll_ticks` of the pad height that two fingers travel.
pub fn watch(
    mut dev: Device,
    max_ms: u64,
    scroll_ticks: i32,
    mut on_event: impl FnMut(Event),
) -> io::Result<()> {
    let limit = Duration::from_millis(max_ms);
    let (mut x_slop, mut y_slop, mut scroll_step) = (100, 100, 100);
    if let Ok(state) = dev.get_abs_state() {
        let range = |axis: AbsoluteAxisCode| {
            let i = state[axis.0 as usize];
            (i.maximum - i.minimum).max(1) / MOVE_FRACTION
        };
        x_slop = range(AbsoluteAxisCode::ABS_X);
        y_slop = range(AbsoluteAxisCode::ABS_Y);
        let i = state[AbsoluteAxisCode::ABS_Y.0 as usize];
        scroll_step = ((i.maximum - i.minimum) / scroll_ticks.max(1)).max(1);
    }

    let debug = std::env::var_os("HEPTIX_DEBUG").is_some();
    let mut touch: Option<Touch> = None;
    let mut fingers = 0u8;
    // Last position and distance travelled since the last tick, per axis (x, y).
    let mut last: [Option<i32>; 2] = [None; 2];
    let mut travel = [0i32; 2];
    let mut last_tick = Instant::now() - MIN_SCROLL_GAP;
    loop {
        for ev in dev.fetch_events()? {
            match ev.destructure() {
                EventSummary::Key(
                    _,
                    KeyCode::BTN_TOOL_FINGER
                    | KeyCode::BTN_TOOL_DOUBLETAP
                    | KeyCode::BTN_TOOL_TRIPLETAP,
                    1,
                ) => {
                    let EventSummary::Key(_, code, _) = ev.destructure() else { unreachable!() };
                    fingers = match code {
                        KeyCode::BTN_TOOL_FINGER => 1,
                        KeyCode::BTN_TOOL_DOUBLETAP => 2,
                        _ => 3,
                    };
                    last = [None; 2];
                    travel = [0; 2];
                }
                EventSummary::Key(_, KeyCode::BTN_TOUCH, 1) => {
                    touch = Some(Touch {
                        start: Instant::now(),
                        x0: None,
                        y0: None,
                        moved: false,
                        clicked: false,
                    });
                }
                EventSummary::Key(_, KeyCode::BTN_TOUCH, 0) => {
                    if let Some(t) = touch.take() {
                        if debug {
                            eprintln!(
                                "release: {:?} moved={} clicked={} slop=({x_slop},{y_slop})",
                                t.start.elapsed(), t.moved, t.clicked
                            );
                        }
                        if !t.moved && !t.clicked && t.start.elapsed() <= limit {
                            on_event(Event::Tap);
                        }
                    }
                }
                EventSummary::Key(_, KeyCode::BTN_LEFT | KeyCode::BTN_RIGHT | KeyCode::BTN_MIDDLE, 1) => {
                    if let Some(t) = touch.as_mut() {
                        t.clicked = true; // the release must not also count as a tap
                    }
                    on_event(Event::Tap);
                }
                EventSummary::AbsoluteAxis(_, axis, v) => {
                    if scroll_ticks > 0 && fingers >= 2 {
                        let i = match axis {
                            AbsoluteAxisCode::ABS_X => Some(0),
                            AbsoluteAxisCode::ABS_Y => Some(1),
                            _ => None,
                        };
                        if let Some(i) = i {
                            if let Some(prev) = last[i].replace(v) {
                                travel[i] += v - prev;
                            }
                            if travel[i].abs() >= scroll_step {
                                travel[i] %= scroll_step;
                                if last_tick.elapsed() >= MIN_SCROLL_GAP {
                                    last_tick = Instant::now();
                                    on_event(Event::Scroll);
                                }
                            }
                        }
                    }
                    if let Some(t) = touch.as_mut() {
                        let (first, slop) = match axis {
                            AbsoluteAxisCode::ABS_X => (&mut t.x0, x_slop),
                            AbsoluteAxisCode::ABS_Y => (&mut t.y0, y_slop),
                            _ => continue,
                        };
                        let origin = *first.get_or_insert(v);
                        if (v - origin).abs() > slop {
                            if debug {
                                eprintln!("moved: {axis:?} {origin} -> {v}");
                            }
                            t.moved = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
