//! Window plumbing for the full-bleed player.
//!
//! GPUI composes its whole window with DirectComposition *above* any child HWND, so mpv cannot be
//! a child of the app window. Instead three top-level windows are stacked over the app's client area:
//!
//! ```text
//!   overlay  - transparent GPUI window (per-pixel alpha) with every control; gets all mouse input
//!   video    - black popup that mpv renders into, directly below the overlay
//!   app      - the normal GPUI window (owner of both, keeps keyboard focus)
//! ```
//!
//! [`Follower`] keeps both popups glued to the owner (moves synchronously from a subclass of the
//! owner's window procedure), keeps the overlay from stealing activation, and hides the cursor.
use std::cell::Cell;
use std::rc::Rc;

#[derive(Default)]
struct Shared {
    alive: Cell<bool>,
    main: isize,
    overlay: isize,
    video: Cell<isize>,
    /// Overlay (and controls) shown.
    visible: Cell<bool>,
    /// Video popup shown (only once there is something to show).
    video_visible: Cell<bool>,
    cursor_hidden: Cell<bool>,
    /// Last applied screen rects, so syncing is cheap when nothing moved.
    placed: Cell<Option<[i32; 4]>>,
    placed_video: Cell<Option<[i32; 4]>>,
    /// A posted sync message is in flight.
    sync_pending: Cell<bool>,
}

pub struct Follower {
    shared: Rc<Shared>,
}

impl Follower {
    /// Takes over `overlay` (a GPUI window) and makes it follow `main`'s client area.
    pub fn new(main: isize, overlay: isize) -> Self {
        let shared = Rc::new(Shared { alive: Cell::new(true), main, overlay, visible: Cell::new(true), ..Default::default() });
        imp::install(&shared);
        let f = Self { shared };
        f.sync();
        f
    }

    pub fn set_video(&self, video: isize) {
        self.shared.video.set(video);
        self.shared.placed_video.set(None);
        self.sync();
    }

    pub fn set_video_visible(&self, v: bool) {
        if self.shared.video_visible.replace(v) != v {
            self.shared.placed_video.set(None);
            self.sync();
        }
    }

    pub fn set_visible(&self, v: bool) {
        if self.shared.visible.replace(v) != v {
            self.shared.placed.set(None);
            self.shared.placed_video.set(None);
            self.sync();
        }
    }

    /// Hides the cursor over the overlay until it is shown again (by mouse movement → `false`).
    pub fn set_cursor_hidden(&self, hidden: bool) {
        self.shared.cursor_hidden.set(hidden);
        if hidden {
            imp::hide_cursor_if_over(self.shared.overlay);
        }
    }

    /// Screen cursor position while it is over the overlay.
    pub fn cursor_inside(&self) -> Option<(i32, i32)> {
        imp::cursor_over(self.shared.overlay)
    }

    /// Re-applies position / size / visibility (cheap when nothing changed). Posted to the
    /// overlay's own message queue: resizing a GPUI window from inside a GPUI update would
    /// reach its resize handler while the app is borrowed, and the new size would be lost.
    pub fn sync(&self) {
        imp::request_sync(&self.shared);
    }

    // Window-frame actions for the overlay's own title-bar controls.
    pub fn minimize(&self) {
        imp::minimize(self.shared.main);
    }
    pub fn toggle_maximize(&self) {
        imp::toggle_maximize(self.shared.main);
    }
    pub fn is_maximized(&self) -> bool {
        imp::is_maximized(self.shared.main)
    }
    pub fn close(&self) {
        imp::close(self.shared.main);
    }
    /// Starts an OS window move (call from a mouse-down/drag in the overlay).
    pub fn begin_move(&self) {
        imp::begin_move(self.shared.main);
    }
    /// Starts an OS resize from the top edge.
    pub fn begin_resize_top(&self) {
        imp::begin_resize_top(self.shared.main);
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.shared.alive.set(false);
        imp::uninstall(&self.shared);
    }
}

/// Raw HWND of a GPUI window.
pub fn hwnd_of(window: &gpui_kit::Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(windows)]
mod imp {
    use super::Shared;
    use std::rc::Rc;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const ID: usize = 0x4d56_504c; // "MVPL"
    const WM_SYNC: u32 = WM_APP + 0x3a;

    pub fn request_sync(s: &Shared) {
        if s.alive.get() && !s.sync_pending.replace(true) {
            unsafe { PostMessageW(h(s.overlay), WM_SYNC, 0, 0) };
        }
    }

    fn h(v: isize) -> HWND {
        v as HWND
    }

    /// Each subclass holds its own strong ref (released on removal / WM_NCDESTROY).
    pub fn install(s: &Rc<Shared>) {
        unsafe {
            // GPUI makes `PopUp` windows topmost, captioned and unowned: make it a plain
            // borderless popup owned by the app window instead (stays above it, minimizes with it).
            let ov = h(s.overlay);
            SetWindowLongPtrW(ov, GWLP_HWNDPARENT, s.main);
            let style = GetWindowLongPtrW(ov, GWL_STYLE) as u32;
            let new_style = (style & WS_VISIBLE) | WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
            SetWindowLongPtrW(ov, GWL_STYLE, new_style as isize);
            let ex = GetWindowLongPtrW(ov, GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(ov, GWL_EXSTYLE, ((ex & !WS_EX_TOPMOST & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) as isize);
            SetWindowPos(ov, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED);
            let corner = DWMWCP_DONOTROUND;
            DwmSetWindowAttribute(ov, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &corner as *const _ as _, 4);
            let border = DWMWA_COLOR_NONE;
            DwmSetWindowAttribute(ov, DWMWA_BORDER_COLOR as u32, &border as *const _ as _, 4);

            SetWindowSubclass(ov, Some(overlay_proc), ID, Rc::into_raw(s.clone()) as usize);
            SetWindowSubclass(h(s.main), Some(main_proc), ID + s.overlay as usize, Rc::into_raw(s.clone()) as usize);
        }
    }

    pub fn uninstall(s: &Rc<Shared>) {
        unsafe {
            for (hwnd, proc_, id) in [
                (s.overlay, overlay_proc as SubProc, ID),
                (s.main, main_proc as SubProc, ID + s.overlay as usize),
            ] {
                if IsWindow(h(hwnd)) != 0 && RemoveWindowSubclass(h(hwnd), Some(proc_), id) != 0 {
                    drop(Rc::from_raw(Rc::as_ptr(s)));
                }
            }
            if IsWindow(h(s.overlay)) != 0 {
                ShowWindow(h(s.overlay), SW_HIDE);
            }
        }
    }

    type SubProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;

    /// Borrow the shared state for the duration of a message (keeps it alive even if the
    /// follower is dropped while the message is being handled).
    unsafe fn borrow(data: usize) -> Rc<Shared> {
        let p = data as *const Shared;
        Rc::increment_strong_count(p);
        Rc::from_raw(p)
    }

    unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM, id: usize, data: usize) -> LRESULT {
        let s = borrow(data);
        match msg {
            // Clicking the overlay must not take focus from the app window (it handles the keys).
            WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
            WM_SYNC => {
                s.sync_pending.set(false);
                sync(&s);
                return 0;
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                if GetForegroundWindow() != h(s.main) {
                    SetForegroundWindow(h(s.main));
                }
            }
            WM_SETCURSOR if s.cursor_hidden.get() && (l & 0xffff) as u32 == HTCLIENT => {
                SetCursor(std::ptr::null_mut());
                return 1;
            }
            WM_NCDESTROY => {
                if RemoveWindowSubclass(hwnd, Some(overlay_proc), id) != 0 {
                    drop(Rc::from_raw(data as *const Shared));
                }
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }

    unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM, id: usize, data: usize) -> LRESULT {
        let s = borrow(data);
        if msg == WM_NCDESTROY {
            if RemoveWindowSubclass(hwnd, Some(main_proc), id) != 0 {
                drop(Rc::from_raw(data as *const Shared));
            }
            return DefSubclassProc(hwnd, msg, w, l);
        }
        let r = DefSubclassProc(hwnd, msg, w, l);
        if msg == WM_WINDOWPOSCHANGED {
            // Follow moves / resizes (posted: this can arrive from inside a GPUI update).
            request_sync(&s);
        }
        r
    }

    fn window_rect(w: HWND) -> [i32; 4] {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(w, &mut r) };
        [r.left, r.top, r.right - r.left, r.bottom - r.top]
    }

    fn client_rect(main: HWND) -> Option<[i32; 4]> {
        unsafe {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if GetClientRect(main, &mut r) == 0 {
                return None;
            }
            let mut pt = POINT { x: 0, y: 0 };
            ClientToScreen(main, &mut pt);
            Some([pt.x, pt.y, (r.right - r.left).max(1), (r.bottom - r.top).max(1)])
        }
    }

    pub fn sync(s: &Shared) {
        if !s.alive.get() {
            return;
        }
        unsafe {
            let main = h(s.main);
            if IsWindow(main) == 0 || IsIconic(main) != 0 {
                // Owned popups hide with a minimized owner on their own.
                return;
            }
            let Some(r) = client_rect(main) else { return };
            let ov = h(s.overlay);
            if s.visible.get() {
                // Compare with the real rect too: GPUI may move it itself (e.g. on DPI changes).
                if s.placed.get() != Some(r) || window_rect(ov) != r || IsWindowVisible(ov) == 0 {
                    SetWindowPos(ov, std::ptr::null_mut(), r[0], r[1], r[2], r[3], SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    s.placed.set(Some(r));
                }
            } else if IsWindowVisible(ov) != 0 {
                ShowWindow(ov, SW_HIDE);
                s.placed.set(None);
            }
            let video = s.video.get();
            if video != 0 && IsWindow(h(video)) != 0 {
                if s.visible.get() && s.video_visible.get() {
                    // Re-stack if anything got between the overlay and the video.
                    let stacked = GetWindow(ov, GW_HWNDNEXT) == h(video);
                    if s.placed_video.get() != Some(r) || window_rect(h(video)) != r || IsWindowVisible(h(video)) == 0 || !stacked {
                        // Directly below the overlay.
                        SetWindowPos(h(video), ov, r[0], r[1], r[2], r[3], SWP_NOACTIVATE | SWP_SHOWWINDOW);
                        s.placed_video.set(Some(r));
                    }
                } else if IsWindowVisible(h(video)) != 0 {
                    ShowWindow(h(video), SW_HIDE);
                    s.placed_video.set(None);
                }
            }
        }
    }

    pub fn cursor_over(overlay: isize) -> Option<(i32, i32)> {
        unsafe {
            let (mut pt, mut r) = (POINT { x: 0, y: 0 }, RECT { left: 0, top: 0, right: 0, bottom: 0 });
            if GetCursorPos(&mut pt) == 0 || IsWindowVisible(h(overlay)) == 0 || GetWindowRect(h(overlay), &mut r) == 0 {
                return None;
            }
            (pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom).then_some((pt.x, pt.y))
        }
    }

    pub fn hide_cursor_if_over(overlay: isize) {
        if cursor_over(overlay).is_some() {
            unsafe { SetCursor(std::ptr::null_mut()) };
        }
    }

    // Frame actions are posted, never sent: they run from inside GPUI event handlers, and the
    // app window's handlers must not be re-entered while GPUI is busy.
    fn syscommand(main: isize, cmd: u32) {
        unsafe { PostMessageW(h(main), WM_SYSCOMMAND, cmd as usize, 0) };
    }

    pub fn minimize(main: isize) {
        syscommand(main, SC_MINIMIZE);
    }

    pub fn is_maximized(main: isize) -> bool {
        unsafe { IsZoomed(h(main)) != 0 }
    }

    pub fn toggle_maximize(main: isize) {
        syscommand(main, if is_maximized(main) { SC_RESTORE } else { SC_MAXIMIZE });
    }

    pub fn close(main: isize) {
        unsafe { PostMessageW(h(main), WM_CLOSE, 0, 0) };
    }

    pub fn begin_move(main: isize) {
        unsafe { ReleaseCapture() };
        // SC_MOVE | HTCAPTION: the system's own drag-move loop (with snap layouts).
        syscommand(main, SC_MOVE | 2);
    }

    pub fn begin_resize_top(main: isize) {
        unsafe { ReleaseCapture() };
        syscommand(main, SC_SIZE | 3); // WMSZ_TOP
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Shared;
    use std::rc::Rc;
    pub fn install(_: &Rc<Shared>) {}
    pub fn uninstall(_: &Rc<Shared>) {}
    pub fn request_sync(_: &Shared) {}
    pub fn cursor_over(_: isize) -> Option<(i32, i32)> {
        None
    }
    pub fn hide_cursor_if_over(_: isize) {}
    pub fn minimize(_: isize) {}
    pub fn is_maximized(_: isize) -> bool {
        false
    }
    pub fn toggle_maximize(_: isize) {}
    pub fn close(_: isize) {}
    pub fn begin_move(_: isize) {}
    pub fn begin_resize_top(_: isize) {}
}
