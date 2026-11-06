//! Minimal libmpv binding loaded at runtime (no import library needed). Video renders into a
//! borderless black popup owned by the app window; the GPUI controls live in a transparent
//! overlay window stacked above it (see `overlay.rs`), so the video is never cut or covered.
//!
//! The UI thread never makes a blocking libmpv call once a file is loading. Synchronous calls
//! (`mpv_get_property*`, `mpv_command`, `mpv_set_property*`, `mpv_terminate_destroy`) wait for
//! mpv's core thread, which can be busy for a long time (a `sub-add` downloading a subtitle over
//! the network, the video output initialising, a stalled stream), and the core can in turn wait
//! for the video window, whose parent belongs to the UI thread: the app froze and ignored clicks.
//! So: properties the player reads are *observed* (their values arrive as events and are read
//! from a cache), writes and commands are *async*, and mpv is destroyed on a background thread.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Result};

type Handle = *mut c_void;

struct Api {
    _lib: libloading::Library,
    create: unsafe extern "C" fn() -> Handle,
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    command_async: unsafe extern "C" fn(Handle, u64, *mut *const c_char) -> c_int,
    terminate_destroy: unsafe extern "C" fn(Handle),
    get_property_string: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property_async: unsafe extern "C" fn(Handle, u64, *const c_char, c_int, *mut c_void) -> c_int,
    observe_property: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    free: unsafe extern "C" fn(*mut c_void),
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut RawEvent,
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    /// Hooks and async reads (absent from very old builds: recaps' music then only switches
    /// between their two tracks instead of mixing them).
    hooks: Option<HookApi>,
}

#[derive(Clone, Copy)]
struct HookApi {
    hook_add: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    hook_continue: unsafe extern "C" fn(Handle, u64) -> c_int,
    get_property_async: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
}

/// `MPV_FORMAT_STRING`.
const FORMAT_STRING: c_int = 1;
/// Reply ids of the recap music mix's async requests (made while the `on_preloaded` hook waits).
const REPLY_TRACKS: u64 = 0x6d76_0001;
const REPLY_MIX: u64 = 0x6d76_0002;
/// The `mpv_hook_add` id of `on_preloaded`.
const HOOK_PRELOADED: u64 = 0x6d76_0010;

/// `mpv_event_hook`.
#[repr(C)]
struct RawHook {
    name: *const c_char,
    id: u64,
}

/// Properties the player reads, delivered as change events (`MPV_FORMAT_STRING`, so lists such as
/// `track-list` and `demuxer-cache-state` arrive as JSON, like `mpv_get_property_string`).
const OBSERVED: &[&str] = &[
    "time-pos",
    "duration",
    "pause",
    "volume",
    "mute",
    "speed",
    "paused-for-cache",
    "cache-buffering-state",
    "eof-reached",
    "seeking",
    "track-list",
    "demuxer-cache-state",
];

/// Prefix of `mpv_event_property`.
#[repr(C)]
struct RawProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

/// The raw handle, moved to the thread that destroys it (the libmpv client API is thread-safe).
struct SendHandle(Handle);
unsafe impl Send for SendHandle {}

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
                command_async: sym!(b"mpv_command_async\0"),
                terminate_destroy: sym!(b"mpv_terminate_destroy\0"),
                get_property_string: sym!(b"mpv_get_property_string\0"),
                set_property_string: sym!(b"mpv_set_property_string\0"),
                set_property_async: sym!(b"mpv_set_property_async\0"),
                observe_property: sym!(b"mpv_observe_property\0"),
                free: sym!(b"mpv_free\0"),
                wait_event: sym!(b"mpv_wait_event\0"),
                error_string: sym!(b"mpv_error_string\0"),
                hooks: (|| {
                    Some(HookApi {
                        hook_add: *lib.get(b"mpv_hook_add\0").ok()?,
                        hook_continue: *lib.get(b"mpv_hook_continue\0").ok()?,
                        get_property_async: *lib.get(b"mpv_get_property_async\0").ok()?,
                    })
                })(),
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
    api: Arc<Api>,
    handle: Handle,
    surface: isize,
    lib_path: PathBuf,
    /// Last values of the observed properties (absent while unavailable).
    props: RefCell<HashMap<String, String>>,
    /// Recaps' music level for the files loaded from now on, once [`Mpv::watch_music`] ran.
    music: Cell<Option<f64>>,
    /// The `on_preloaded` hook mpv waits on while the music mix is set up, and the track pair
    /// whose mix was requested.
    hook: Cell<Option<u64>>,
    mixing: Cell<Option<(i64, i64)>>,
    /// The recap tracks (with music, narration only) mixed in the file playing now.
    mixed: Cell<Option<(i64, i64)>>,
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
            let mpv = Self {
                api: Arc::new(api),
                handle,
                surface,
                lib_path,
                props: RefCell::default(),
                music: Cell::new(None),
                hook: Cell::new(None),
                mixing: Cell::new(None),
                mixed: Cell::new(None),
            };
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
            for name in OBSERVED {
                let n = CString::new(*name)?;
                (mpv.api.observe_property)(mpv.handle, 0, n.as_ptr(), FORMAT_STRING);
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

    /// A property's value: from the observed cache (never blocks) for the properties in
    /// [`OBSERVED`], else a synchronous `mpv_get_property_string` (only safe before a file loads).
    /// `None` when unavailable.
    pub fn get(&self, name: &str) -> Option<String> {
        if OBSERVED.contains(&name) {
            return self.props.borrow().get(name).cloned();
        }
        self.get_now(name)
    }

    /// `mpv_get_property_string` (blocks until mpv's core answers).
    fn get_now(&self, name: &str) -> Option<String> {
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

    /// Sets a property without waiting for mpv (`mpv_set_property_async`). The cached value is
    /// updated at once so the next read already shows it; mpv's own change event follows.
    pub fn set(&self, name: &str, value: &str) -> Result<()> {
        let (n, v) = (CString::new(name)?, CString::new(value)?);
        let mut p = v.as_ptr();
        // mpv copies the value before returning.
        let rc = unsafe { (self.api.set_property_async)(self.handle, 0, n.as_ptr(), FORMAT_STRING, &mut p as *mut *const c_char as *mut c_void) };
        if rc < 0 {
            return Err(anyhow!("mpv set {name} failed ({})", self.err(rc)));
        }
        if OBSERVED.contains(&name) {
            self.props.borrow_mut().insert(name.to_string(), value.to_string());
        }
        Ok(())
    }

    /// Synchronous `mpv_set_property_string`: only before the first file is loaded, while mpv's
    /// core is idle (start position, languages), so it is applied before `loadfile`.
    pub fn set_now(&self, name: &str, value: &str) -> Result<()> {
        let (n, v) = (CString::new(name)?, CString::new(value)?);
        let rc = unsafe { (self.api.set_property_string)(self.handle, n.as_ptr(), v.as_ptr()) };
        if rc < 0 {
            Err(anyhow!("mpv set {name} failed ({})", self.err(rc)))
        } else {
            Ok(())
        }
    }

    /// `mpv_command_async` with string arguments: queued, never waits (`sub-add` of a subtitle
    /// URL downloads it, `loadfile` opens the stream). Errors only for malformed requests.
    pub fn command(&self, args: &[&str]) -> Result<()> {
        let owned: Vec<CString> = args.iter().map(|a| CString::new(*a)).collect::<std::result::Result<_, _>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let rc = unsafe { (self.api.command_async)(self.handle, 0, ptrs.as_mut_ptr()) };
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

    /// Audio and subtitle tracks from the observed `track-list` (one JSON value, instead of six
    /// blocking property reads per track every second).
    pub fn tracks(&self) -> Vec<Track> {
        self.props.borrow().get("track-list").map(|j| crate::logic::parse_tracks(j)).unwrap_or_default()
    }

    /// Plays the music of every MangaVox recap loaded from now on at `level` (0..=1): its two
    /// tracks are mixed before the file starts (mpv's `on_preloaded` hook). Mixing them in once a
    /// file plays makes mpv jump seconds ahead to where the second track's data resumes, so the
    /// mix is in place from the start and later changes only move its weights. False when this
    /// libmpv has no hooks (the music button then switches tracks).
    pub fn watch_music(&self, level: f64) -> bool {
        let Some(h) = self.api.hooks else { return false };
        if self.music.get().is_none() {
            let rc = unsafe { (h.hook_add)(self.handle, HOOK_PRELOADED, c"on_preloaded".as_ptr(), 0) };
            if rc < 0 {
                return false;
            }
        }
        self.music.set(Some(crate::logic::clamp_music(level)));
        true
    }

    /// The recap tracks (with music, narration only) mixed in the file playing now.
    pub fn music_mixed(&self) -> Option<(i64, i64)> {
        self.mixed.get()
    }

    /// The music at `level` from now on: the playing recap's mix gets the new weights (a short
    /// pause in the sound while mpv rebuilds it). False when nothing is mixed (no change made).
    pub fn set_music_level(&self, level: f64) -> bool {
        if self.music.get().is_some() {
            self.music.set(Some(crate::logic::clamp_music(level)));
        }
        let Some(pair) = self.mixed.get() else { return false };
        mv_core::logging::global().log_with(mv_core::logging::Level::Info, "player", "recap music level", serde_json::json!({ "level": crate::logic::clamp_music(level) }));
        self.set("file-local-options/lavfi-complex", &crate::logic::music_graph(pair, level)).is_ok()
    }

    /// Lets the file mpv holds at `on_preloaded` go on loading.
    fn release_hook(&self) {
        self.mixing.set(None);
        if let (Some(id), Some(h)) = (self.hook.take(), self.api.hooks) {
            unsafe { (h.hook_continue)(self.handle, id) };
        }
    }

    /// Whether a file is held at `on_preloaded` while its music mix is set up (the player then
    /// polls more often, so the file starts without a noticeable wait).
    pub fn hook_pending(&self) -> bool {
        self.hook.get().is_some()
    }

    /// `on_preloaded`: the file is open, no decoder runs yet. Asks for its track list (async: the
    /// UI thread never waits on mpv); the reply decides whether its music is mixed.
    fn on_preloaded(&self, id: u64) {
        self.release_hook();
        self.hook.set(Some(id));
        let (Some(h), Some(_)) = (self.api.hooks, self.music.get()) else { return self.release_hook() };
        let rc = unsafe { (h.get_property_async)(self.handle, REPLY_TRACKS, c"track-list".as_ptr(), FORMAT_STRING) };
        if rc < 0 {
            self.release_hook();
        }
    }

    /// The held file's track list: a MangaVox recap whose two tracks mix gets the mix (as a
    /// file-local option, so mpv drops it when the file ends); the hook is released once mpv has
    /// it, or at once for anything else.
    fn on_preloaded_tracks(&self, json: Option<String>) {
        if self.hook.get().is_none() {
            return;
        }
        let Some(json) = json else { return self.release_hook() };
        let tracks = crate::logic::parse_tracks(&json);
        self.props.borrow_mut().insert("track-list".into(), json);
        let level = self.music.get();
        let Some((pair, level)) = crate::logic::music_tracks(&tracks).filter(|p| crate::logic::music_mixable(&tracks, *p)).zip(level) else { return self.release_hook() };
        let (n, v) = (c"file-local-options/lavfi-complex", CString::new(crate::logic::music_graph(pair, level)).unwrap_or_default());
        let mut p = v.as_ptr();
        let rc = unsafe { (self.api.set_property_async)(self.handle, REPLY_MIX, n.as_ptr(), FORMAT_STRING, &mut p as *mut *const c_char as *mut c_void) };
        if rc < 0 {
            return self.release_hook();
        }
        self.mixing.set(Some(pair));
    }

    /// Drains pending events without blocking, updating the observed property cache.
    pub fn poll_events(&self) -> Vec<Event> {
        let mut out = vec![];
        for _ in 0..256 {
            let ev = unsafe { (self.api.wait_event)(self.handle, 0.0) };
            if ev.is_null() {
                break;
            }
            let (id, error, data, reply) = unsafe { ((*ev).event_id, (*ev).error, (*ev).data, (*ev).reply_userdata) };
            match id {
                0 => break, // MPV_EVENT_NONE
                // MPV_EVENT_START_FILE: the last file's mix went with it (a file-local option).
                6 => self.mixed.set(None),
                // MPV_EVENT_HOOK
                25 if !data.is_null() => {
                    let h = unsafe { &*(data as *const RawHook) };
                    self.on_preloaded(h.id);
                }
                // MPV_EVENT_GET_PROPERTY_REPLY: the held file's track list.
                3 if reply == REPLY_TRACKS => {
                    let value = (error >= 0 && !data.is_null())
                        .then(|| unsafe { &*(data as *const RawProperty) })
                        .filter(|p| p.format == FORMAT_STRING && !p.data.is_null())
                        .map(|p| unsafe { *(p.data as *const *const c_char) })
                        .filter(|s| !s.is_null())
                        .map(|s| unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned());
                    self.on_preloaded_tracks(value);
                }
                // MPV_EVENT_SET_PROPERTY_REPLY: the mix is set, the file may start.
                4 if reply == REPLY_MIX => {
                    let pair = self.mixing.get().filter(|_| error >= 0);
                    self.mixed.set(pair);
                    mv_core::logging::global().log_with(
                        mv_core::logging::Level::Info,
                        "player",
                        "recap music mixed",
                        serde_json::json!({ "tracks": pair.map(|(f, s)| [f, s]), "level": self.music.get(), "error": (error < 0).then(|| self.err(error)) }),
                    );
                    self.release_hook();
                }
                22 if !data.is_null() => {
                    // MPV_EVENT_PROPERTY_CHANGE
                    let p = unsafe { &*(data as *const RawProperty) };
                    if p.name.is_null() {
                        continue;
                    }
                    let name = unsafe { CStr::from_ptr(p.name) }.to_string_lossy().into_owned();
                    let value = (p.format == FORMAT_STRING && !p.data.is_null())
                        .then(|| unsafe { *(p.data as *const *const c_char) })
                        .filter(|s| !s.is_null())
                        .map(|s| unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned());
                    let mut props = self.props.borrow_mut();
                    match value {
                        Some(v) => {
                            props.insert(name, v);
                        }
                        None => {
                            props.remove(&name);
                        }
                    }
                }
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
        // `mpv_terminate_destroy` waits for mpv's threads, including the video output closing
        // its window inside our popup (which messages the UI thread): run it off the UI thread,
        // then have the UI thread destroy the popup through its own message queue.
        let (api, handle, surface) = (self.api.clone(), SendHandle(self.handle), self.surface);
        let spawned = std::thread::Builder::new().name("mpv-teardown".into()).spawn(move || {
            let h = handle;
            unsafe { (api.terminate_destroy)(h.0) };
            close_surface_later(surface);
        });
        if spawned.is_err() {
            unsafe { (self.api.terminate_destroy)(self.handle) };
            destroy_surface(self.surface);
        }
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

    /// From any thread: the popup closes (and is destroyed) on its own thread. Default
    /// `WM_CLOSE` handling destroys the window.
    pub fn close_surface_later(h: isize) {
        unsafe { PostMessageW(h as HWND, WM_CLOSE, 0, 0) };
    }
}

#[cfg(windows)]
use win::{close_surface_later, create_surface, destroy_surface, hide_surface};

#[cfg(not(windows))]
fn create_surface(_: isize) -> Result<isize> {
    Err(anyhow!("the video player is Windows-only for now"))
}
#[cfg(not(windows))]
fn hide_surface(_: isize) {}
#[cfg(not(windows))]
fn destroy_surface(_: isize) {}
#[cfg(not(windows))]
fn close_surface_later(_: isize) {}
