//! Minimal libmpv binding loaded at runtime (no import library needed). Video renders into a
//! borderless black popup owned by the app window; the GPUI controls live in a transparent
//! overlay window stacked above it (see `overlay.rs`), so the video is never cut or covered.
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _, Result};

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

/// mpv options, tuned for streaming over the local torrent HTTP server (range requests):
/// start on the first decodable data, keep a big forward cache plus a back-buffer so seeking
/// backwards (and forwards inside what is already downloaded) never touches the network.
/// Unknown options (older libmpv builds) are skipped.
const OPTIONS: &[(&str, &str)] = &[
    // Output / decoding.
    ("vo", "gpu-next,gpu"),
    ("hwdec", "auto-safe"),
    ("background", "color"),
    ("background-color", "#000000"),
    ("keepaspect", "yes"),
    // Embedding: no mpv UI, input or window behaviour of its own.
    ("keep-open", "yes"),
    ("idle", "yes"),
    ("osc", "no"),
    ("osd-level", "0"),
    ("input-default-bindings", "no"),
    ("input-vo-keyboard", "no"),
    ("input-cursor", "no"),
    ("cursor-autohide", "no"),
    ("window-dragging", "no"),
    ("ytdl", "no"),
    ("terminal", "no"),
    ("audio-client-name", "MangaVox"),
    ("audio-fallback-to-null", "yes"),
    ("volume-max", "130"),
    // Cache: start fast, seek instantly inside buffered ranges.
    ("cache", "yes"),
    ("cache-secs", "120"),
    ("demuxer-max-bytes", "256MiB"),
    ("demuxer-max-back-bytes", "128MiB"),
    ("demuxer-readahead-secs", "30"),
    ("demuxer-seekable-cache", "yes"),
    ("force-seekable", "yes"),
    ("cache-pause", "yes"),
    ("cache-pause-initial", "no"),
    ("cache-pause-wait", "1.5"),
    ("stream-buffer-size", "512KiB"),
    ("network-timeout", "90"),
    // Exact seeks for absolute targets; scrubbing uses keyframe seeks explicitly.
    ("hr-seek", "default"),
    ("hr-seek-framedrop", "yes"),
    // Subtitles: clean, readable defaults.
    ("sub-auto", "fuzzy"),
    ("slang", "en,eng"),
    ("sub-font", "Segoe UI Semibold"),
    ("sub-font-size", "46"),
    ("sub-border-size", "2.4"),
    ("sub-shadow-offset", "1"),
    ("sub-shadow-color", "#80000000"),
    ("sub-margin-y", "40"),
];

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
    /// Creates the (hidden) black video popup owned by `owner` and an initialised mpv rendering into it.
    pub fn create(owner: isize) -> Result<Self> {
        let (api, lib_path) = load_api()?;
        let surface = create_surface(owner)?;
        unsafe {
            let handle = (api.create)();
            if handle.is_null() {
                destroy_surface(surface);
                return Err(anyhow!("mpv_create returned null"));
            }
            // From here Drop cleans up both mpv and the popup on any error.
            let mpv = Self { api, handle, surface, lib_path };
            mpv.set_option("wid", &surface.to_string())?;
            for (k, v) in OPTIONS {
                // Older builds may lack an option; none of these are essential.
                let _ = mpv.set_option(k, v);
            }
            // Debugging aid: full mpv log to a file.
            if let Some(path) = std::env::var_os("MANGAVOX_MPV_LOG") {
                let _ = mpv.set_option("log-file", &path.to_string_lossy());
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

    /// Exact seek (frame accurate).
    pub fn seek_absolute(&self, secs: f64) {
        self.seek(secs, true);
    }

    /// `exact = false` jumps to the nearest keyframe: much cheaper, used for live scrubbing.
    pub fn seek(&self, secs: f64, exact: bool) {
        let flags = if exact { "absolute+exact" } else { "absolute+keyframes" };
        let _ = self.command(&["seek", &format!("{:.3}", secs.max(0.0)), flags]);
    }

    /// Buffered (seekable without network) ranges in seconds.
    pub fn cache_ranges(&self) -> Vec<(f64, f64)> {
        self.get("demuxer-cache-state").map(|j| crate::logic::cache_ranges(&j)).unwrap_or_default()
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

    /// The video popup's HWND.
    pub fn surface(&self) -> isize {
        self.surface
    }

    pub fn library_path(&self) -> String {
        self.lib_path.display().to_string()
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // Hide first so the popup never lingers while mpv shuts down.
        hide_surface(self.surface);
        unsafe { (self.api.terminate_destroy)(self.handle) };
        destroy_surface(self.surface);
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const CLASS: &str = "MangaVoxVideo\0";

    unsafe extern "system" fn wnd_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match msg {
            // Never take mouse input or activation (the overlay above handles both).
            WM_NCHITTEST => HTTRANSPARENT as LRESULT,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            _ => DefWindowProcW(h, msg, w, l),
        }
    }

    fn register() -> *const u16 {
        static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
        let mut first = false;
        let class = NAME.get_or_init(|| {
            first = true;
            CLASS.encode_utf16().collect()
        });
        if first {
            unsafe {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(std::ptr::null()),
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                // Black brush: nothing ever flashes grey before the first frame.
                hbrBackground: GetStockObject(BLACK_BRUSH) as HBRUSH,
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            RegisterClassExW(&wc);
            }
        }
        class.as_ptr()
    }

    /// A hidden borderless popup owned by the app window; the overlay positions and shows it.
    pub fn create_surface(owner: isize) -> anyhow::Result<isize> {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                register(),
                std::ptr::null(),
                WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                owner as HWND,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(anyhow::anyhow!("CreateWindowExW failed"));
        }
        Ok(hwnd as isize)
    }

    pub fn hide_surface(h: isize) {
        unsafe { ShowWindow(h as HWND, SW_HIDE) };
    }

    pub fn destroy_surface(h: isize) {
        unsafe { DestroyWindow(h as HWND) };
    }
}

#[cfg(windows)]
use win::{create_surface, destroy_surface, hide_surface};

#[cfg(not(windows))]
fn create_surface(_: isize) -> Result<isize> {
    Err(anyhow!("the video player is Windows-only for now"))
}
#[cfg(not(windows))]
fn hide_surface(_: isize) {}
#[cfg(not(windows))]
fn destroy_surface(_: isize) {}
