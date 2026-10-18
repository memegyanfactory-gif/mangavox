//! Minimal libmpv binding loaded at runtime (no import library needed). Video renders into a
//! borderless owned popup positioned over the player area (GPUI composes above child HWNDs).
//! GPUI controls drawn over the video are made visible by cutting holes in the popup's region.
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _, Result};
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

type Handle = *mut c_void;

struct Api {
    _lib: libloading::Library,
    create: unsafe extern "C" fn() -> Handle,
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    command: unsafe extern "C" fn(Handle, *mut *const c_char) -> c_int,
    terminate_destroy: unsafe extern "C" fn(Handle),
    get_property_string: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    free: unsafe extern "C" fn(*mut c_void),
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut RawEvent,
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
}

/// Prefix of `mpv_event`.
#[repr(C)]
struct RawEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

/// Prefix of `mpv_event_end_file`.
#[repr(C)]
struct RawEndFile {
    reason: c_int,
    error: c_int,
}

fn candidates() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(p) = std::env::var_os("MANGAVOX_LIBMPV") {
        v.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("libmpv-2.dll"));
            v.push(dir.join("mpv-2.dll"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        v.push(local.join("Programs").join("Stremio").join("libmpv-2.dll"));
    }
    v
}

fn load_api() -> Result<(Api, PathBuf)> {
    for path in candidates() {
        if !path.exists() {
            continue;
        }
        unsafe {
            let lib = libloading::Library::new(&path).with_context(|| format!("loading {}", path.display()))?;
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get($name).map_err(|e| anyhow!("missing {}: {e}", String::from_utf8_lossy($name)))?
                };
            }
            let api = Api {
                create: sym!(b"mpv_create\0"),
                set_option_string: sym!(b"mpv_set_option_string\0"),
                initialize: sym!(b"mpv_initialize\0"),
                command: sym!(b"mpv_command\0"),
                terminate_destroy: sym!(b"mpv_terminate_destroy\0"),
                get_property_string: sym!(b"mpv_get_property_string\0"),
                set_property_string: sym!(b"mpv_set_property_string\0"),
                free: sym!(b"mpv_free\0"),
                wait_event: sym!(b"mpv_wait_event\0"),
                error_string: sym!(b"mpv_error_string\0"),
                _lib: lib,
            };
            return Ok((api, path));
        }
    }
    Err(anyhow!("libmpv-2.dll not found (set MANGAVOX_LIBMPV)"))
}

pub struct Mpv {
    api: Api,
    handle: Handle,
    surface: isize,
    lib_path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Sub,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: i64,
    pub kind: TrackKind,
    pub lang: String,
    pub title: String,
    pub selected: bool,
    pub external: bool,
}

/// Player events the UI cares about.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    FileLoaded,
    /// Playback reached the end of the file.
    Ended,
    /// Loading or decoding failed.
    Failed(String),
}

impl Mpv {
    /// Creates the video popup over `rect` (owner-client device pixels) and an initialised mpv rendering into it.
    pub fn create(window: &Window, rect: (i32, i32, i32, i32)) -> Result<Self> {
        let parent = match HasWindowHandle::window_handle(window).map_err(|e| anyhow!("{e}"))?.as_raw() {
            RawWindowHandle::Win32(h) => h.hwnd.get(),
            _ => return Err(anyhow!("unsupported window type")),
        };
        let (api, lib_path) = load_api()?;
        let surface = create_child(parent, rect)?;
        unsafe {
            let handle = (api.create)();
            if handle.is_null() {
                destroy_child(surface);
                return Err(anyhow!("mpv_create returned null"));
            }
            // From here Drop cleans up both mpv and the popup on any error.
            let mpv = Self { api, handle, surface, lib_path };
            mpv.set_option("wid", &surface.to_string())?;
            for (k, v) in [
                ("hwdec", "auto-safe"),
                ("keep-open", "yes"),
                ("osc", "no"),
                ("osd-level", "0"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("input-cursor", "no"),
                ("cursor-autohide", "3000"),
                ("window-dragging", "no"),
                ("ytdl", "no"),
                ("volume-max", "130"),
                ("cache", "yes"),
                ("demuxer-max-bytes", "150MiB"),
                ("sub-auto", "fuzzy"),
                ("slang", "en,eng"),
            ] {
                // Older builds may lack an option; none of these are essential.
                let _ = mpv.set_option(k, v);
            }
            if (mpv.api.initialize)(mpv.handle) < 0 {
                return Err(anyhow!("mpv_initialize failed"));
            }
            Ok(mpv)
        }
    }

    fn set_option(&self, name: &str, value: &str) -> Result<()> {
        let (n, v) = (CString::new(name)?, CString::new(value)?);
        let rc = unsafe { (self.api.set_option_string)(self.handle, n.as_ptr(), v.as_ptr()) };
        if rc < 0 {
            Err(anyhow!("mpv option {name} failed ({})", self.err(rc)))
        } else {
            Ok(())
        }
    }

    fn err(&self, rc: c_int) -> String {
        unsafe {
            let p = (self.api.error_string)(rc);
            if p.is_null() {
                rc.to_string()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    /// `mpv_get_property_string`; `None` when unavailable.
    pub fn get(&self, name: &str) -> Option<String> {
        let n = CString::new(name).ok()?;
        unsafe {
            let p = (self.api.get_property_string)(self.handle, n.as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            (self.api.free)(p as *mut c_void);
            Some(s)
        }
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        self.get(name)?.trim().parse().ok()
    }

    pub fn get_bool(&self, name: &str) -> Option<bool> {
        self.get(name).map(|v| v == "yes")
    }

    pub fn set(&self, name: &str, value: &str) -> Result<()> {
        let (n, v) = (CString::new(name)?, CString::new(value)?);
        let rc = unsafe { (self.api.set_property_string)(self.handle, n.as_ptr(), v.as_ptr()) };
        if rc < 0 {
            Err(anyhow!("mpv set {name} failed ({})", self.err(rc)))
        } else {
            Ok(())
        }
    }

    /// `mpv_command` with string arguments.
    pub fn command(&self, args: &[&str]) -> Result<()> {
        let owned: Vec<CString> = args.iter().map(|a| CString::new(*a)).collect::<std::result::Result<_, _>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let rc = unsafe { (self.api.command)(self.handle, ptrs.as_mut_ptr()) };
        if rc < 0 {
            Err(anyhow!("mpv {} failed ({})", args.first().unwrap_or(&""), self.err(rc)))
        } else {
            Ok(())
        }
    }

    /// Loads a URL or path (replacing the current file).
    pub fn load_str(&self, target: &str) -> Result<()> {
        self.command(&["loadfile", target, "replace"])
    }

    pub fn load(&self, file: &Path) -> Result<()> {
        self.load_str(&file.to_string_lossy())
    }

    pub fn set_pause(&self, paused: bool) {
        let _ = self.set("pause", if paused { "yes" } else { "no" });
    }

    pub fn seek_absolute(&self, secs: f64) {
        let _ = self.command(&["seek", &format!("{:.3}", secs.max(0.0)), "absolute"]);
    }

    /// Adds an external subtitle (selected only if it matches the preferred language).
    pub fn sub_add(&self, url: &str, title: &str, lang: &str) -> Result<()> {
        self.command(&["sub-add", url, "auto", title, lang])
    }

    /// Audio and subtitle tracks from `track-list`.
    pub fn tracks(&self) -> Vec<Track> {
        let count = self.get_f64("track-list/count").unwrap_or(0.0) as usize;
        let mut out = vec![];
        for i in 0..count {
            let kind = match self.get(&format!("track-list/{i}/type")).as_deref() {
                Some("audio") => TrackKind::Audio,
                Some("sub") => TrackKind::Sub,
                _ => continue,
            };
            let Some(id) = self.get_f64(&format!("track-list/{i}/id")) else { continue };
            out.push(Track {
                id: id as i64,
                kind,
                lang: self.get(&format!("track-list/{i}/lang")).unwrap_or_default(),
                title: self.get(&format!("track-list/{i}/title")).unwrap_or_default(),
                selected: self.get_bool(&format!("track-list/{i}/selected")).unwrap_or(false),
                external: self.get_bool(&format!("track-list/{i}/external")).unwrap_or(false),
            });
        }
        out
    }

    /// Drains pending events without blocking.
    pub fn poll_events(&self) -> Vec<Event> {
        let mut out = vec![];
        for _ in 0..32 {
            let ev = unsafe { (self.api.wait_event)(self.handle, 0.0) };
            if ev.is_null() {
                break;
            }
            let (id, error, data) = unsafe { ((*ev).event_id, (*ev).error, (*ev).data) };
            match id {
                0 => break, // MPV_EVENT_NONE
                7 => {
                    // MPV_EVENT_END_FILE
                    if !data.is_null() {
                        let ef = unsafe { &*(data as *const RawEndFile) };
                        match ef.reason {
                            0 => out.push(Event::Ended),
                            4 => out.push(Event::Failed(self.err(if ef.error != 0 { ef.error } else { error }))),
                            _ => {}
                        }
                    }
                }
                8 => out.push(Event::FileLoaded), // MPV_EVENT_FILE_LOADED
                _ => {}
            }
        }
        out
    }

    pub fn surface(&self) -> isize {
        self.surface
    }

    pub fn library_path(&self) -> String {
        self.lib_path.display().to_string()
    }

    pub fn set_visible(&self, visible: bool) {
        show_surface(self.surface, visible);
    }

    /// Cuts `[x, y, w, h]` rects (popup device pixels) out of the video so GPUI shows through.
    pub fn set_holes(&self, holes: &[[i32; 4]]) {
        set_region(self.surface, holes);
    }

    /// Screen cursor position while it is over the video (for idle detection).
    pub fn cursor_over(&self) -> Option<(i32, i32)> {
        cursor_over(self.surface)
    }

    /// mpv's own video window (made on mpv's thread) would swallow mouse input over the video;
    /// disabled child windows pass it to the popup, whose HTTRANSPARENT hands it to the app.
    pub fn pass_mouse_through(&self) {
        disable_children(self.surface);
    }

    /// Hides the cursor until it moves, if it is over the video.
    pub fn hide_cursor(&self) {
        if self.cursor_over().is_some() {
            hide_cursor();
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // Hide first so the black popup never lingers while mpv shuts down.
        show_surface(self.surface, false);
        unsafe { (self.api.terminate_destroy)(self.handle) };
        destroy_child(self.surface);
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    /// GPUI draws with DirectComposition *above* child windows, so the video lives in a
    /// borderless popup owned by the app window, kept over the player area in screen space.
    /// (STATIC windows answer WM_NCHITTEST with HTTRANSPARENT, so mouse input falls through to the app.)
    pub fn create_child(parent: isize, rect: (i32, i32, i32, i32)) -> anyhow::Result<isize> {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                parent as HWND,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(anyhow::anyhow!("CreateWindowExW failed"));
        }
        move_surface(hwnd as isize, rect);
        Ok(hwnd as isize)
    }

    /// `rect` is in the owner's client coordinates (device pixels).
    pub fn move_surface(h: isize, (x, y, w, hgt): (i32, i32, i32, i32)) {
        unsafe {
            let owner = GetWindow(h as HWND, GW_OWNER);
            if owner.is_null() {
                return;
            }
            if IsIconic(owner) != 0 {
                return;
            }
            let mut pt = POINT { x, y };
            ClientToScreen(owner, &mut pt);
            SetWindowPos(h as HWND, std::ptr::null_mut(), pt.x, pt.y, w.max(1), hgt.max(1), SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    pub fn show_surface(h: isize, visible: bool) {
        unsafe { ShowWindow(h as HWND, if visible { SW_SHOWNA } else { SW_HIDE }) };
    }

    pub fn destroy_child(h: isize) {
        unsafe { DestroyWindow(h as HWND) };
    }

    /// Window region = everything minus `holes`; no holes restores the full window.
    pub fn set_region(h: isize, holes: &[[i32; 4]]) {
        use windows_sys::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_DIFF};
        unsafe {
            if holes.is_empty() {
                SetWindowRgn(h as HWND, std::ptr::null_mut(), 1);
                return;
            }
            let rgn = CreateRectRgn(0, 0, 32767, 32767);
            for &[x, y, w, hgt] in holes {
                let cut = CreateRectRgn(x, y, x + w, y + hgt);
                CombineRgn(rgn, rgn, cut, RGN_DIFF);
                DeleteObject(cut);
            }
            // The system owns `rgn` after this call.
            SetWindowRgn(h as HWND, rgn, 1);
        }
    }

    pub fn cursor_over(h: isize) -> Option<(i32, i32)> {
        use windows_sys::Win32::Foundation::RECT;
        unsafe {
            let (mut pt, mut r) = (POINT { x: 0, y: 0 }, RECT { left: 0, top: 0, right: 0, bottom: 0 });
            if GetCursorPos(&mut pt) == 0 || IsWindowVisible(h as HWND) == 0 || GetWindowRect(h as HWND, &mut r) == 0 {
                return None;
            }
            (pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom).then_some((pt.x, pt.y))
        }
    }

    pub fn disable_children(h: isize) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
        unsafe {
            let mut c = GetWindow(h as HWND, GW_CHILD);
            while !c.is_null() {
                if IsWindowEnabled(c) != 0 {
                    EnableWindow(c, 0);
                }
                c = GetWindow(c, GW_HWNDNEXT);
            }
        }
    }

    pub fn hide_cursor() {
        unsafe { SetCursor(std::ptr::null_mut()) };
    }
}

#[cfg(windows)]
use win::{create_child, cursor_over, destroy_child, disable_children, hide_cursor, set_region, show_surface};
#[cfg(windows)]
pub use win::move_surface;

#[cfg(not(windows))]
fn create_child(_: isize, _: (i32, i32, i32, i32)) -> Result<isize> {
    Err(anyhow!("the video player is Windows-only for now"))
}
#[cfg(not(windows))]
pub fn move_surface(_: isize, _: (i32, i32, i32, i32)) {}
#[cfg(not(windows))]
fn show_surface(_: isize, _: bool) {}
#[cfg(not(windows))]
fn destroy_child(_: isize) {}
#[cfg(not(windows))]
fn set_region(_: isize, _: &[[i32; 4]]) {}
#[cfg(not(windows))]
fn cursor_over(_: isize) -> Option<(i32, i32)> {
    None
}
#[cfg(not(windows))]
fn hide_cursor() {}
#[cfg(not(windows))]
fn disable_children(_: isize) {}
