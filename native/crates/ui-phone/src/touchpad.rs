//! Touchpad: a paired phone moves the PC's mouse pointer, clicks, scrolls and types
//! (`POST /api/mouse`). Unlike the remote screen (`screen.rs`, which posts messages to the
//! MangaVox window only) this drives the real pointer and keyboard of the PC through
//! `SendInput`, so it works in every app. Like every `/api` route it needs a paired phone's cookie
//! and the CSRF header (`auth::guard`).
//!
//! The phone sends a batch of events at most once a frame (~16 ms), with one request in flight at
//! a time and the finger movement of that frame summed into one `move`. A batch becomes one
//! `SendInput` call, so its events reach Windows in order and are never interleaved with another
//! batch. Values are clamped here, whatever the phone sends: a batch can move the pointer a few
//! screens at most, scroll a few dozen notches and type a few hundred characters.
//!
//! Windows does not let input reach windows of elevated (administrator) apps or the secure
//! desktop (UAC prompt, lock screen); `SendInput` drops it there without an error.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

/// Events in one request.
pub const MAX_EVENTS: usize = 64;
/// Largest pointer move of one event, in pixels on each axis.
pub const MAX_MOVE: f64 = 4000.0;
/// Largest scroll of one event, in wheel notches on each axis.
pub const MAX_SCROLL: f64 = 30.0;
/// Characters typed by one request (all its text events together).
pub const MAX_TEXT: usize = 500;
/// Clicks of one click event (2 = double click).
pub const MAX_CLICKS: u8 = 3;
/// One wheel notch (`WHEEL_DELTA`).
const WHEEL: f64 = 120.0;
/// A button held down from the phone is let go when the phone sends nothing for this long (it
/// went to sleep or lost Wi-Fi mid-drag), so the PC is never left with a stuck mouse button.
const HOLD_TIMEOUT: Duration = Duration::from_secs(10);

pub fn routes() -> Router {
    Router::new().route("/api/mouse", post(handle))
}

/// `{"e": [event, ...]}`.
#[derive(Debug, Deserialize)]
struct Batch {
    #[serde(default)]
    e: Vec<Event>,
}

/// One touchpad event. Moves are relative, in pixels (fractions carry over to the next move);
/// scrolls are in wheel notches, positive `dy` scrolls down and positive `dx` right.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Event {
    Move { dx: f64, dy: f64 },
    Click {
        #[serde(default)]
        b: Button,
        #[serde(default = "one")]
        n: u8,
    },
    /// Press and hold (drag with the Left button on the phone).
    Down {
        #[serde(default)]
        b: Button,
    },
    Up {
        #[serde(default)]
        b: Button,
    },
    Scroll {
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
    },
    Text { s: String },
    Key { key: String },
}

fn one() -> u8 {
    1
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

impl Button {
    fn index(self) -> usize {
        self as usize
    }
    const ALL: [Button; 3] = [Button::Left, Button::Right, Button::Middle];
}

/// One `SendInput` entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Relative pointer move in pixels.
    Move(i32, i32),
    Press(Button),
    Release(Button),
    /// Vertical wheel in `WHEEL_DELTA` units, positive = away from the user (scrolls up).
    Wheel(i32),
    /// Horizontal wheel, positive = right.
    HWheel(i32),
    /// A virtual key pressed or let go (`ext` = extended key).
    Key { vk: u16, ext: bool, up: bool },
    /// A UTF-16 unit typed (`KEYEVENTF_UNICODE`), pressed or let go.
    Unit { unit: u16, up: bool },
}

/// What is left over between batches: sub-pixel / sub-unit remainders (so slow, careful finger
/// moves are not rounded away) and the buttons the phone holds down.
#[derive(Debug, Default)]
pub struct Carry {
    x: f64,
    y: f64,
    wheel: f64,
    hwheel: f64,
    held: [bool; 3],
}

fn finite(v: f64, max: f64) -> f64 {
    if v.is_finite() {
        v.clamp(-max, max)
    } else {
        0.0
    }
}

/// Whole units of `acc + v`, keeping the fraction in `acc`.
fn take_whole(acc: &mut f64, v: f64) -> i32 {
    *acc += v;
    let whole = acc.trunc();
    *acc -= whole;
    whole as i32
}

/// The virtual key for a key name the phone sends, and whether it is an extended key.
pub fn vk_of(key: &str) -> Option<(u16, bool)> {
    Some(match key.to_ascii_lowercase().as_str() {
        "enter" => (0x0D, false),
        "escape" => (0x1B, false),
        "backspace" => (0x08, false),
        "tab" => (0x09, false),
        "space" => (0x20, false),
        "pageup" => (0x21, true),
        "pagedown" => (0x22, true),
        "end" => (0x23, true),
        "home" => (0x24, true),
        "left" => (0x25, true),
        "up" => (0x26, true),
        "right" => (0x27, true),
        "down" => (0x28, true),
        "delete" => (0x2E, true),
        "volumemute" => (0xAD, true),
        "volumedown" => (0xAE, true),
        "volumeup" => (0xAF, true),
        "medianext" => (0xB0, true),
        "mediaprev" => (0xB1, true),
        "playpause" => (0xB3, true),
        _ => return None,
    })
}

fn key_tap(out: &mut Vec<Action>, vk: u16, ext: bool) {
    out.push(Action::Key { vk, ext, up: false });
    out.push(Action::Key { vk, ext, up: true });
}

/// Turns a batch into `SendInput` entries, clamping every value. Errors (400) on a batch that is
/// too long, types too much or names an unknown key; nothing of such a batch is sent.
pub fn plan(events: &[Event], carry: &mut Carry) -> Result<Vec<Action>, String> {
    if events.len() > MAX_EVENTS {
        return Err(format!("At most {MAX_EVENTS} events per request"));
    }
    let typed: usize = events.iter().map(|e| if let Event::Text { s } = e { s.chars().count() } else { 0 }).sum();
    if typed > MAX_TEXT {
        return Err(format!("At most {MAX_TEXT} characters at a time"));
    }
    let mut out = Vec::new();
    for e in events {
        match e {
            Event::Move { dx, dy } => {
                let x = take_whole(&mut carry.x, finite(*dx, MAX_MOVE));
                let y = take_whole(&mut carry.y, finite(*dy, MAX_MOVE));
                if x != 0 || y != 0 {
                    out.push(Action::Move(x, y));
                }
            }
            Event::Click { b, n } => {
                // A click ends a hold of the same button.
                carry.held[b.index()] = false;
                for _ in 0..(*n).clamp(1, MAX_CLICKS) {
                    out.push(Action::Press(*b));
                    out.push(Action::Release(*b));
                }
            }
            Event::Down { b } => {
                if !carry.held[b.index()] {
                    carry.held[b.index()] = true;
                    out.push(Action::Press(*b));
                }
            }
            Event::Up { b } => {
                if carry.held[b.index()] {
                    carry.held[b.index()] = false;
                    out.push(Action::Release(*b));
                }
            }
            Event::Scroll { dx, dy } => {
                // Phone: +dy scrolls down. Windows: a positive wheel delta scrolls up.
                let v = take_whole(&mut carry.wheel, -finite(*dy, MAX_SCROLL) * WHEEL);
                if v != 0 {
                    out.push(Action::Wheel(v));
                }
                let h = take_whole(&mut carry.hwheel, finite(*dx, MAX_SCROLL) * WHEEL);
                if h != 0 {
                    out.push(Action::HWheel(h));
                }
            }
            Event::Text { s } => {
                for c in s.chars() {
                    match c {
                        '\n' => key_tap(&mut out, 0x0D, false),
                        '\t' => key_tap(&mut out, 0x09, false),
                        // Other control characters (and \r of \r\n) are dropped.
                        c if c.is_control() => {}
                        c => {
                            let mut buf = [0u16; 2];
                            for unit in c.encode_utf16(&mut buf) {
                                out.push(Action::Unit { unit: *unit, up: false });
                                out.push(Action::Unit { unit: *unit, up: true });
                            }
                        }
                    }
                }
            }
            Event::Key { key } => {
                let (vk, ext) = vk_of(key).ok_or_else(|| format!("Unknown key: {}", key.chars().take(24).collect::<String>()))?;
                key_tap(&mut out, vk, ext);
            }
        }
    }
    Ok(out)
}

/// Releases for every held button (the hold timed out).
fn release_held(carry: &mut Carry) -> Vec<Action> {
    let mut out = Vec::new();
    for b in Button::ALL {
        if std::mem::take(&mut carry.held[b.index()]) {
            out.push(Action::Release(b));
        }
    }
    out
}

struct Shared {
    carry: Carry,
    last: Instant,
}

static STATE: Mutex<Option<Shared>> = Mutex::new(None);
static WATCHDOG: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------- test sink

static DRY_RUN: AtomicBool = AtomicBool::new(false);
static SENT: Mutex<Vec<Action>> = Mutex::new(Vec::new());

/// Tests: from now on nothing reaches Windows; what would have been sent is kept for
/// [`take_sent_for_tests`] (the real mouse never moves in tests).
#[doc(hidden)]
pub fn dry_run_for_tests() {
    DRY_RUN.store(true, Ordering::SeqCst);
}

#[doc(hidden)]
pub fn take_sent_for_tests() -> Vec<Action> {
    std::mem::take(&mut *SENT.lock().unwrap_or_else(|e| e.into_inner()))
}

fn send(actions: &[Action]) -> Result<(), String> {
    if actions.is_empty() {
        return Ok(());
    }
    if DRY_RUN.load(Ordering::SeqCst) || cfg!(test) {
        SENT.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(actions);
        return Ok(());
    }
    imp::send(actions)
}

// ---------------------------------------------------------------- route

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

async fn handle(body: Result<Json<Batch>, axum::extract::rejection::JsonRejection>) -> Response {
    let Ok(Json(batch)) = body else {
        return err(StatusCode::BAD_REQUEST, "Bad touchpad input");
    };
    let res = {
        let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let st = g.get_or_insert_with(|| Shared { carry: Carry::default(), last: Instant::now() });
        st.last = Instant::now();
        // Sent while the lock is held, so two requests never interleave their input.
        plan(&batch.e, &mut st.carry).and_then(|actions| send(&actions).map(|()| st.carry.held.iter().any(|h| *h)))
    };
    match res {
        Ok(holding) => {
            if holding {
                start_watchdog();
            }
            Json(json!({})).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// While a button is held, lets it go once the phone has been quiet for [`HOLD_TIMEOUT`].
fn start_watchdog() {
    if WATCHDOG.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
            let Some(st) = g.as_mut() else { break };
            if !st.carry.held.iter().any(|h| *h) {
                break;
            }
            if st.last.elapsed() >= HOLD_TIMEOUT {
                let release = release_held(&mut st.carry);
                if let Err(e) = send(&release) {
                    eprintln!("[phone] touchpad: could not let go of a held button: {e}");
                }
                break;
            }
        }
        WATCHDOG.store(false, Ordering::SeqCst);
    });
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
mod imp {
    use super::{Action, Button};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

    fn mouse(dx: i32, dy: i32, data: i32, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        }
    }

    fn key(vk: u16, scan: u16, flags: u32) -> INPUT {
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
    }

    fn input(a: &Action) -> INPUT {
        match *a {
            Action::Move(dx, dy) => mouse(dx, dy, 0, MOUSEEVENTF_MOVE),
            Action::Press(b) => mouse(0, 0, 0, match b {
                Button::Left => MOUSEEVENTF_LEFTDOWN,
                Button::Right => MOUSEEVENTF_RIGHTDOWN,
                Button::Middle => MOUSEEVENTF_MIDDLEDOWN,
            }),
            Action::Release(b) => mouse(0, 0, 0, match b {
                Button::Left => MOUSEEVENTF_LEFTUP,
                Button::Right => MOUSEEVENTF_RIGHTUP,
                Button::Middle => MOUSEEVENTF_MIDDLEUP,
            }),
            Action::Wheel(d) => mouse(0, 0, d, MOUSEEVENTF_WHEEL),
            Action::HWheel(d) => mouse(0, 0, d, MOUSEEVENTF_HWHEEL),
            Action::Key { vk, ext, up } => {
                key(vk, 0, if ext { KEYEVENTF_EXTENDEDKEY } else { 0 } | if up { KEYEVENTF_KEYUP } else { 0 })
            }
            Action::Unit { unit, up } => key(0, unit, KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 }),
        }
    }

    pub fn send(actions: &[Action]) -> Result<(), String> {
        let inputs: Vec<INPUT> = actions.iter().map(input).collect();
        let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
        if (sent as usize) < inputs.len() {
            return Err("Windows blocked the input (the PC may be locked)".into());
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Action;
    pub fn send(_: &[Action]) -> Result<(), String> {
        Err("The touchpad needs Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<Event> {
        serde_json::from_str::<Batch>(s).unwrap().e
    }

    #[test]
    fn parses_events() {
        let e = parse(r#"{"e":[{"t":"move","dx":3.5,"dy":-2},{"t":"click"},{"t":"click","b":"right","n":2},{"t":"down","b":"left"},{"t":"up"},{"t":"scroll","dy":1.5},{"t":"text","s":"hi"},{"t":"key","key":"enter"}]}"#);
        assert_eq!(e.len(), 8);
        assert_eq!(e[0], Event::Move { dx: 3.5, dy: -2.0 });
        assert_eq!(e[1], Event::Click { b: Button::Left, n: 1 });
        assert_eq!(e[2], Event::Click { b: Button::Right, n: 2 });
        assert_eq!(e[4], Event::Up { b: Button::Left });
        assert_eq!(e[5], Event::Scroll { dx: 0.0, dy: 1.5 });
        assert!(parse(r#"{}"#).is_empty());
        for bad in [r#"{"e":[{"t":"nope"}]}"#, r#"{"e":[{"t":"move","dx":"x","dy":1}]}"#, r#"{"e":[{"t":"click","b":"side"}]}"#, r#"{"e":[{"t":"move","dx":NaN,"dy":0}]}"#, r#"{"e":[{"t":"click","n":300}]}"#, r#"{"e":[{"t":"text"}]}"#] {
            assert!(serde_json::from_str::<Batch>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn moves_are_clamped_and_keep_fractions() {
        let mut c = Carry::default();
        assert_eq!(plan(&[Event::Move { dx: 1e300, dy: -1e300 }], &mut c).unwrap(), vec![Action::Move(4000, -4000)]);
        assert_eq!(plan(&[Event::Move { dx: f64::INFINITY, dy: f64::NAN }], &mut c).unwrap(), vec![]);
        // 0.4 px four times: nothing, nothing, then 1 px with 0.2 left over... never lost.
        let mut c = Carry::default();
        let mut total = 0;
        for _ in 0..10 {
            for a in plan(&[Event::Move { dx: 0.4, dy: -0.4 }], &mut c).unwrap() {
                let Action::Move(x, y) = a else { panic!() };
                assert_eq!(x, -y);
                total += x;
            }
        }
        assert_eq!(total, 4, "ten 0.4 px moves are 4 px");
        // Several moves in one batch stay separate entries, in order.
        let a = plan(&[Event::Move { dx: 5.0, dy: 0.0 }, Event::Click { b: Button::Left, n: 1 }, Event::Move { dx: -2.0, dy: 7.0 }], &mut Carry::default()).unwrap();
        assert_eq!(a, vec![Action::Move(5, 0), Action::Press(Button::Left), Action::Release(Button::Left), Action::Move(-2, 7)]);
    }

    #[test]
    fn clicks_and_holds() {
        let mut c = Carry::default();
        let a = plan(&[Event::Click { b: Button::Right, n: 1 }], &mut c).unwrap();
        assert_eq!(a, vec![Action::Press(Button::Right), Action::Release(Button::Right)]);
        let a = plan(&[Event::Click { b: Button::Left, n: 9 }], &mut c).unwrap();
        assert_eq!(a.len(), 2 * MAX_CLICKS as usize, "clicks are capped");
        let a = plan(&[Event::Click { b: Button::Left, n: 0 }], &mut c).unwrap();
        assert_eq!(a.len(), 2, "at least one click");
        // Down twice presses once; up releases once; a stray up sends nothing.
        let a = plan(&[Event::Down { b: Button::Left }, Event::Down { b: Button::Left }, Event::Move { dx: 3.0, dy: 0.0 }, Event::Up { b: Button::Left }, Event::Up { b: Button::Left }], &mut c).unwrap();
        assert_eq!(a, vec![Action::Press(Button::Left), Action::Move(3, 0), Action::Release(Button::Left)]);
        // A hold that times out is let go.
        plan(&[Event::Down { b: Button::Middle }], &mut c).unwrap();
        assert_eq!(release_held(&mut c), vec![Action::Release(Button::Middle)]);
        assert!(release_held(&mut c).is_empty());
    }

    #[test]
    fn scrolls_are_clamped_and_signed() {
        let mut c = Carry::default();
        assert_eq!(plan(&[Event::Scroll { dx: 0.0, dy: 1.0 }], &mut c).unwrap(), vec![Action::Wheel(-120)], "+dy scrolls down");
        assert_eq!(plan(&[Event::Scroll { dx: 2.0, dy: 0.0 }], &mut c).unwrap(), vec![Action::HWheel(240)]);
        assert_eq!(plan(&[Event::Scroll { dx: 0.0, dy: -1e9 }], &mut c).unwrap(), vec![Action::Wheel(30 * 120)]);
        // Small smooth-scroll steps add up.
        let mut c = Carry::default();
        let total: i32 = (0..10).flat_map(|_| plan(&[Event::Scroll { dx: 0.0, dy: 0.05 }], &mut c).unwrap()).map(|a| if let Action::Wheel(d) = a { d } else { 0 }).sum();
        assert_eq!(total, -60);
    }

    #[test]
    fn text_and_keys() {
        let mut c = Carry::default();
        let a = plan(&[Event::Text { s: "a\u{1F600}\r\n\u{7}".into() }], &mut c).unwrap();
        assert_eq!(
            a,
            vec![
                Action::Unit { unit: 'a' as u16, up: false },
                Action::Unit { unit: 'a' as u16, up: true },
                Action::Unit { unit: 0xD83D, up: false },
                Action::Unit { unit: 0xD83D, up: true },
                Action::Unit { unit: 0xDE00, up: false },
                Action::Unit { unit: 0xDE00, up: true },
                Action::Key { vk: 0x0D, ext: false, up: false },
                Action::Key { vk: 0x0D, ext: false, up: true },
            ]
        );
        assert_eq!(plan(&[Event::Key { key: "Left".into() }], &mut c).unwrap(), vec![Action::Key { vk: 0x25, ext: true, up: false }, Action::Key { vk: 0x25, ext: true, up: true }]);
        assert!(plan(&[Event::Key { key: "f13".into() }], &mut c).is_err());
        assert!(plan(&[Event::Key { key: "lwin".into() }], &mut c).is_err(), "no Windows key: it opens things outside the app");
        // Too much text, or too many events: refused whole.
        assert!(plan(&[Event::Text { s: "x".repeat(MAX_TEXT + 1) }], &mut c).is_err());
        assert!(plan(&[Event::Text { s: "x".repeat(MAX_TEXT / 2 + 1) }, Event::Text { s: "x".repeat(MAX_TEXT / 2 + 1) }], &mut c).is_err());
        assert!(plan(&vec![Event::Click { b: Button::Left, n: 1 }; MAX_EVENTS + 1], &mut c).is_err());
        assert!(plan(&vec![Event::Move { dx: 1.0, dy: 1.0 }; MAX_EVENTS], &mut c).is_ok());
    }
}
