//! Remote screen: the phone mirrors the MangaVox main window (`GET /api/screen/frame`, a JPEG of the
//! client area) and controls it (`POST /api/screen/input`).
//!
//! Capture: `PrintWindow(PW_CLIENTONLY | PW_RENDERFULLCONTENT)` into a DIB section, which renders
//! GPUI's DirectComposition content even when the window is covered or off screen. When that comes
//! back all black (old drivers, some GPU setups) the client rect is copied from the screen instead,
//! which only shows the window when it is visible and on top. A minimized window is reported (409),
//! never restored.
//!
//! Input is posted to the window as mouse / keyboard messages, so the PC's cursor never moves and no
//! other window gets it. GPUI reads modifier keys with `GetKeyState`, which posted messages do not
//! change, so keys with ctrl / shift / alt are dispatched in-process through the UI bridge
//! (`Window::dispatch_keystroke`) instead.
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

/// The error text [`capture_hwnd`] returns for a minimized window (the route answers 409).
pub const MINIMIZED: &str = "MangaVox is minimized on the PC";

const DEFAULT_W: u32 = 900;
const MAX_W: u32 = 1600;
const DEFAULT_Q: u8 = 60;
/// Frames this fresh are served again to bursts of requests (several tabs, retries).
const CACHE_FOR: Duration = Duration::from_millis(60);

pub fn routes() -> Router {
    Router::new().route("/api/screen/frame", get(frame)).route("/api/screen/input", post(input))
}

#[derive(Deserialize)]
struct FrameQuery {
    w: Option<u32>,
    q: Option<u8>,
}

/// One phone gesture. x / y are 0..1 of the client area.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Input {
    Tap { x: f64, y: f64 },
    /// Long press: a right click.
    Long { x: f64, y: f64 },
    /// `dy` in wheel notches, positive scrolls down (fractions allowed).
    Scroll { x: f64, y: f64, dy: f64 },
    Drag { x: f64, y: f64, x2: f64, y2: f64 },
    Key {
        key: String,
        #[serde(default)]
        ctrl: bool,
        #[serde(default)]
        shift: bool,
        #[serde(default)]
        alt: bool,
    },
    Text { s: String },
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

struct Cached {
    at: Instant,
    hwnd: isize,
    w: u32,
    q: u8,
    jpeg: Arc<Vec<u8>>,
    size: (u32, u32),
}

/// The last frame.
static CACHE: Mutex<Option<Cached>> = Mutex::new(None);
/// Held (by the blocking capture itself) while a capture runs, so only one runs at a time.
static CAPTURING: LazyLock<Arc<tokio::sync::Mutex<()>>> = LazyLock::new(Default::default);
/// PrintWindow waits for the UI thread; while it is busy (a page loading) the phone gets the
/// previous frame after this long instead of hanging.
const CAPTURE_WAIT: Duration = Duration::from_millis(1500);

/// The last frame of `hwnd`: one made with `w` / `q` within `max_age`, or any when `max_age` is None.
fn cached(hwnd: isize, w: u32, q: u8, max_age: Option<Duration>) -> Option<(Arc<Vec<u8>>, (u32, u32))> {
    let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let c = cache.as_ref().filter(|c| c.hwnd == hwnd)?;
    match max_age {
        Some(age) if c.w != w || c.q != q || c.at.elapsed() >= age => None,
        _ => Some((c.jpeg.clone(), c.size)),
    }
}

fn jpeg_response((jpeg, (fw, fh)): (Arc<Vec<u8>>, (u32, u32)), stale: bool) -> Response {
    let mut res = (
        [
            (header::CONTENT_TYPE, "image/jpeg".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::HeaderName::from_static("x-frame-size"), format!("{fw}x{fh}")),
        ],
        jpeg.as_ref().clone(),
    )
        .into_response();
    if stale {
        res.headers_mut().insert("x-frame-stale", header::HeaderValue::from_static("1"));
    }
    res
}

async fn frame(Query(fq): Query<FrameQuery>) -> Response {
    let w = fq.w.unwrap_or(DEFAULT_W).clamp(160, MAX_W);
    let q = fq.q.unwrap_or(DEFAULT_Q).clamp(30, 90);
    let Some(hwnd) = main_hwnd().await else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "The MangaVox window was not found");
    };
    if let Some(f) = cached(hwnd, w, q, Some(CACHE_FOR)) {
        return jpeg_response(f, false);
    }
    let busy = || match cached(hwnd, w, q, None) {
        Some(f) => jpeg_response(f, true),
        None => err(StatusCode::SERVICE_UNAVAILABLE, "MangaVox is busy, try again"),
    };
    let deadline = tokio::time::Instant::now() + CAPTURE_WAIT;
    let Ok(guard) = tokio::time::timeout_at(deadline, CAPTURING.clone().lock_owned()).await else {
        return busy();
    };
    // Another request may have captured while this one waited.
    if let Some(f) = cached(hwnd, w, q, Some(CACHE_FOR)) {
        return jpeg_response(f, false);
    }
    let job = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let (jpeg, size) = capture_hwnd(hwnd, w, q)?;
        let jpeg = Arc::new(jpeg);
        *CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Cached { at: Instant::now(), hwnd, w, q, jpeg: jpeg.clone(), size });
        Ok::<_, String>((jpeg, size))
    });
    match tokio::time::timeout_at(deadline, job).await {
        Err(_) => busy(),
        Ok(Ok(Ok(f))) => jpeg_response(f, false),
        Ok(Ok(Err(e))) if e == MINIMIZED => err(StatusCode::CONFLICT, e),
        Ok(Ok(Err(e))) => {
            // The window may have been recreated; look it up again next time.
            MAIN.store(0, Ordering::Relaxed);
            err(StatusCode::INTERNAL_SERVER_ERROR, e)
        }
        Ok(Err(_)) => err(StatusCode::INTERNAL_SERVER_ERROR, "Capture failed"),
    }
}

async fn input(Json(inp): Json<Input>) -> Response {
    // Modifier combos go through GPUI directly (see the module docs).
    if let Input::Key { key, ctrl, shift, alt } = &inp {
        if *ctrl || *shift || *alt {
            let Some(name) = keystroke_name(key) else {
                return err(StatusCode::BAD_REQUEST, format!("Unknown key: {key}"));
            };
            let mut s = String::new();
            for (on, m) in [(*ctrl, "ctrl-"), (*alt, "alt-"), (*shift, "shift-")] {
                if on {
                    s.push_str(m);
                }
            }
            s.push_str(&name);
            let res = crate::bridge::on_ui(move |window, cx| match gpui_kit::Keystroke::parse(&s) {
                Ok(k) => {
                    window.dispatch_keystroke(k, cx);
                    Ok(())
                }
                Err(e) => Err(format!("{e:?}")),
            })
            .await;
            return match res {
                Ok(Ok(())) => Json(json!({})).into_response(),
                Ok(Err(e)) => err(StatusCode::BAD_REQUEST, e),
                Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, e),
            };
        }
    }
    let Some(hwnd) = main_hwnd().await else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "The MangaVox window was not found");
    };
    match tokio::task::spawn_blocking(move || post_input(hwnd, &inp)).await {
        Ok(Ok(())) => Json(json!({})).into_response(),
        Ok(Err(e)) if e == MINIMIZED => err(StatusCode::CONFLICT, e),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, e),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "Input failed"),
    }
}

/// GPUI keystroke name for a contract key name (or a single letter / digit).
fn keystroke_name(key: &str) -> Option<String> {
    let k = key.to_ascii_lowercase();
    let named = [
        "enter", "escape", "backspace", "tab", "left", "right", "up", "down", "space", "pageup", "pagedown", "home", "end",
        "delete",
    ];
    if named.contains(&k.as_str()) || (k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric())) {
        Some(k)
    } else {
        None
    }
}

/// The main window's HWND (cached).
static MAIN: AtomicIsize = AtomicIsize::new(0);

async fn main_hwnd() -> Option<isize> {
    let cur = MAIN.load(Ordering::Relaxed);
    if cur != 0 && imp::is_window(cur) {
        return Some(cur);
    }
    let from_ui = tokio::time::timeout(Duration::from_secs(3), crate::bridge::on_ui(|window, _| hwnd_of(window))).await;
    let found = match from_ui {
        Ok(Ok(Some(h))) => Some(h),
        _ => {
            let pid = std::process::id();
            find_window(|p, title| p == pid && title.ends_with("MangaVox"))
        }
    };
    if let Some(h) = found {
        MAIN.store(h, Ordering::Relaxed);
    }
    found
}

fn hwnd_of(window: &gpui_kit::Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// How the last frame was captured ("print" = PrintWindow, "screen" = copied from the screen).
pub fn last_method() -> &'static str {
    match imp::LAST_METHOD.load(Ordering::Relaxed) {
        1 => "print",
        2 => "screen",
        _ => "none",
    }
}

pub use imp::{capture_hwnd, find_window, post_input, process_path};

#[cfg(windows)]
mod imp {
    use super::{Input, MINIMIZED};
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::time::Duration;

    use image::codecs::jpeg::JpegEncoder;
    use image::imageops::FilterType;
    use image::RgbImage;
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::Storage::Xps::{PrintWindow, PW_CLIENTONLY};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_VSC};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    pub static LAST_METHOD: AtomicU8 = AtomicU8::new(0);

    const PW_RENDERFULLCONTENT: u32 = 2;

    fn h(v: isize) -> HWND {
        v as HWND
    }

    pub fn is_window(hwnd: isize) -> bool {
        unsafe { IsWindow(h(hwnd)) != 0 }
    }

    fn client_size(hwnd: isize) -> Result<(i32, i32), String> {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if unsafe { GetClientRect(h(hwnd), &mut r) } == 0 {
            return Err("The MangaVox window is gone".into());
        }
        Ok((r.right - r.left, r.bottom - r.top))
    }

    /// True when every sampled pixel is pure black (a failed DirectComposition capture). A dark
    /// theme is never exactly 0,0,0 everywhere.
    fn is_black(bgra: &[u8]) -> bool {
        let px = bgra.len() / 4;
        let step = (px / 4096).max(1);
        (0..px).step_by(step).all(|i| bgra[i * 4] == 0 && bgra[i * 4 + 1] == 0 && bgra[i * 4 + 2] == 0)
    }

    /// The window's client area as a JPEG at most `max_w` wide, and the client size in physical pixels.
    pub fn capture_hwnd(hwnd: isize, max_w: u32, q: u8) -> Result<(Vec<u8>, (u32, u32)), String> {
        if !is_window(hwnd) {
            return Err("The MangaVox window is gone".into());
        }
        if unsafe { IsIconic(h(hwnd)) } != 0 {
            return Err(MINIMIZED.into());
        }
        let (w, hgt) = client_size(hwnd)?;
        if w <= 0 || hgt <= 0 {
            return Err(MINIMIZED.into());
        }
        let bgra = unsafe { grab(hwnd, w, hgt)? };
        let (w, hgt) = (w as u32, hgt as u32);
        let mut rgb = Vec::with_capacity((w * hgt * 3) as usize);
        for p in bgra.chunks_exact(4) {
            rgb.extend_from_slice(&[p[2], p[1], p[0]]);
        }
        let mut img = RgbImage::from_raw(w, hgt, rgb).ok_or("Bad frame")?;
        let max_w = max_w.max(1);
        if w > max_w {
            let nh = ((hgt as u64 * max_w as u64) / w as u64).max(1) as u32;
            img = image::imageops::resize(&img, max_w, nh, FilterType::Triangle);
        }
        let mut out = Vec::with_capacity(img.width() as usize * img.height() as usize / 4);
        JpegEncoder::new_with_quality(&mut out, q.clamp(1, 100)).encode_image(&img).map_err(|e| e.to_string())?;
        Ok((out, (w, hgt)))
    }

    /// The window is in front (it or one of its own popups has focus) and a grid of points across
    /// its client area all hit it, i.e. no other app's window covers it.
    unsafe fn unobscured(hwnd: isize, origin: POINT, w: i32, hgt: i32) -> bool {
        let ours = |other: HWND| !other.is_null() && GetAncestor(other, GA_ROOTOWNER) == h(hwnd);
        if !ours(GetForegroundWindow()) {
            return false;
        }
        const N: i32 = 6;
        (0..N).all(|i| {
            (0..N).all(|j| {
                let p = POINT { x: origin.x + (w - 1) * i / (N - 1), y: origin.y + (hgt - 1) * j / (N - 1) };
                ours(WindowFromPoint(p))
            })
        })
    }

    /// Top-down BGRA pixels of the client area: PrintWindow first, then the screen.
    unsafe fn grab(hwnd: isize, w: i32, hgt: i32) -> Result<Vec<u8>, String> {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("GetDC failed".into());
        }
        let mem = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -hgt;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if dib.is_null() || bits.is_null() {
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("Could not allocate the frame".into());
        }
        let old = SelectObject(mem, dib);
        let len = (w as usize) * (hgt as usize) * 4;
        let px = std::slice::from_raw_parts(bits as *const u8, len);

        let mut ok = PrintWindow(h(hwnd), mem, PW_CLIENTONLY | PW_RENDERFULLCONTENT) != 0;
        GdiFlush();
        ok = ok && !is_black(px);
        let mut method = 1;
        if !ok {
            let mut pt = POINT { x: 0, y: 0 };
            ClientToScreen(h(hwnd), &mut pt);
            // A screen copy shows whatever covers the window: only take it when nothing does, so
            // the phone never sees other apps.
            ok = unobscured(hwnd, pt, w, hgt) && BitBlt(mem, 0, 0, w, hgt, screen, pt.x, pt.y, SRCCOPY | CAPTUREBLT) != 0;
            GdiFlush();
            method = 2;
        }
        let out = px.to_vec();
        SelectObject(mem, old);
        DeleteObject(dib);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if !ok {
            return Err("Could not capture the window".into());
        }
        LAST_METHOD.store(method, Ordering::Relaxed);
        Ok(out)
    }

    fn lp(x: i32, y: i32) -> LPARAM {
        (((y as u16 as u32) << 16) | (x as u16 as u32)) as LPARAM
    }

    fn post(hwnd: isize, msg: u32, wp: WPARAM, l: LPARAM) -> Result<(), String> {
        if unsafe { PostMessageW(h(hwnd), msg, wp, l) } == 0 {
            return Err("The MangaVox window is gone".into());
        }
        Ok(())
    }

    fn pause(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// Posts one phone gesture to the window. Modifiers on `Input::Key` are ignored here (posted
    /// messages cannot change GetKeyState); the HTTP route dispatches those in-process.
    pub fn post_input(hwnd: isize, input: &Input) -> Result<(), String> {
        if !is_window(hwnd) {
            return Err("The MangaVox window is gone".into());
        }
        if unsafe { IsIconic(h(hwnd)) } != 0 {
            return Err(MINIMIZED.into());
        }
        let (cw, ch) = client_size(hwnd)?;
        let to_px = |x: f64, y: f64| -> (i32, i32) {
            let x = (x.clamp(0.0, 1.0) * (cw - 1).max(0) as f64).round() as i32;
            let y = (y.clamp(0.0, 1.0) * (ch - 1).max(0) as f64).round() as i32;
            (x, y)
        };
        let lbutton: WPARAM = 0x0001; // MK_LBUTTON
        let rbutton: WPARAM = 0x0002; // MK_RBUTTON
        match input {
            Input::Tap { x, y } => {
                let (x, y) = to_px(*x, *y);
                post(hwnd, WM_MOUSEMOVE, 0, lp(x, y))?;
                // Let hover-revealed controls paint before the press.
                pause(40);
                post(hwnd, WM_LBUTTONDOWN, lbutton, lp(x, y))?;
                pause(30);
                post(hwnd, WM_LBUTTONUP, 0, lp(x, y))?;
            }
            Input::Long { x, y } => {
                let (x, y) = to_px(*x, *y);
                post(hwnd, WM_MOUSEMOVE, 0, lp(x, y))?;
                pause(40);
                post(hwnd, WM_RBUTTONDOWN, rbutton, lp(x, y))?;
                pause(30);
                post(hwnd, WM_RBUTTONUP, 0, lp(x, y))?;
            }
            Input::Scroll { x, y, dy } => {
                let (x, y) = to_px(*x, *y);
                post(hwnd, WM_MOUSEMOVE, 0, lp(x, y))?;
                let mut pt = POINT { x, y };
                unsafe { ClientToScreen(h(hwnd), &mut pt) };
                let delta = (-dy.clamp(-30.0, 30.0) * WHEEL_DELTA as f64).round() as i32;
                if delta != 0 {
                    let wp = ((delta as i16 as u16 as usize) << 16) as WPARAM;
                    post(hwnd, WM_MOUSEWHEEL, wp, lp(pt.x, pt.y))?;
                }
            }
            Input::Drag { x, y, x2, y2 } => {
                let (ax, ay) = to_px(*x, *y);
                let (bx, by) = to_px(*x2, *y2);
                post(hwnd, WM_MOUSEMOVE, 0, lp(ax, ay))?;
                pause(30);
                post(hwnd, WM_LBUTTONDOWN, lbutton, lp(ax, ay))?;
                const STEPS: i32 = 10;
                for i in 1..=STEPS {
                    pause(16);
                    let px = ax + (bx - ax) * i / STEPS;
                    let py = ay + (by - ay) * i / STEPS;
                    post(hwnd, WM_MOUSEMOVE, lbutton, lp(px, py))?;
                }
                pause(30);
                post(hwnd, WM_LBUTTONUP, 0, lp(bx, by))?;
            }
            Input::Key { key, .. } => {
                let (vk, ext) = vk_of(key).ok_or_else(|| format!("Unknown key: {key}"))?;
                let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) } & 0xff;
                let base = 1 | ((scan as isize) << 16) | if ext { 1 << 24 } else { 0 };
                post(hwnd, WM_KEYDOWN, vk as WPARAM, base)?;
                pause(20);
                post(hwnd, WM_KEYUP, vk as WPARAM, base | (1 << 30) | (1 << 31))?;
            }
            Input::Text { s } => {
                for unit in s.encode_utf16() {
                    post(hwnd, WM_CHAR, unit as WPARAM, 1)?;
                }
            }
        }
        Ok(())
    }

    /// Virtual-key code and "extended key" flag.
    fn vk_of(key: &str) -> Option<(u32, bool)> {
        let k = key.to_ascii_lowercase();
        Some(match k.as_str() {
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
            s if s.len() == 1 && s.chars().all(|c| c.is_ascii_alphanumeric()) => (s.to_ascii_uppercase().as_bytes()[0] as u32, false),
            _ => return None,
        })
    }

    /// The full exe path of a process.
    pub fn process_path(pid: u32) -> Option<String> {
        unsafe {
            let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if p.is_null() {
                return None;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
            CloseHandle(p);
            ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    struct Search<'a> {
        pred: &'a dyn Fn(u32, &str) -> bool,
        found: isize,
    }

    unsafe extern "system" fn enum_cb(hwnd: HWND, l: LPARAM) -> BOOL {
        let s = &mut *(l as *mut Search);
        if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
            return 1;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        if (s.pred)(pid, &title) {
            s.found = hwnd as isize;
            return 0;
        }
        1
    }

    /// The first visible top-level window for which `pred(pid, title)` holds.
    pub fn find_window(pred: impl Fn(u32, &str) -> bool) -> Option<isize> {
        let mut s = Search { pred: &pred, found: 0 };
        unsafe { EnumWindows(Some(enum_cb), &mut s as *mut Search as LPARAM) };
        (s.found != 0).then_some(s.found)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Input;
    pub static LAST_METHOD: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
    pub fn is_window(_: isize) -> bool {
        false
    }
    pub fn capture_hwnd(_: isize, _: u32, _: u8) -> Result<(Vec<u8>, (u32, u32)), String> {
        Err("Screen mirroring needs Windows".into())
    }
    pub fn post_input(_: isize, _: &Input) -> Result<(), String> {
        Err("Remote control needs Windows".into())
    }
    pub fn process_path(_: u32) -> Option<String> {
        None
    }
    pub fn find_window(_: impl Fn(u32, &str) -> bool) -> Option<isize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inputs() {
        let i: Input = serde_json::from_str(r#"{"t":"scroll","x":0.5,"y":0.5,"dy":-2}"#).unwrap();
        assert!(matches!(i, Input::Scroll { dy, .. } if dy == -2.0));
        let i: Input = serde_json::from_str(r#"{"t":"key","key":"enter","ctrl":true}"#).unwrap();
        assert!(matches!(i, Input::Key { ctrl: true, shift: false, .. }));
        let i: Input = serde_json::from_str(r#"{"t":"text","s":"hi"}"#).unwrap();
        assert!(matches!(i, Input::Text { .. }));
        assert!(serde_json::from_str::<Input>(r#"{"t":"nope"}"#).is_err());
    }

    #[test]
    fn key_names() {
        assert_eq!(keystroke_name("Enter").as_deref(), Some("enter"));
        assert_eq!(keystroke_name("a").as_deref(), Some("a"));
        assert_eq!(keystroke_name("f13"), None);
    }
}
