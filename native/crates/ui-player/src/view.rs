//! The player page. The video (mpv popup) fills the whole window; every control is drawn by GPUI
//! in a transparent overlay window stacked above it (see `overlay.rs`), Netflix / Stremio style:
//! soft gradients over the picture, auto-hiding with the cursor, animated scrubber and menus.
//!
//! The app window keeps keyboard focus (keys are handled here); the overlay handles the mouse
//! and forwards window-level actions (fullscreen, back, next) to the app window. Kit controls
//! never take focus on a mouse press, so every click in the overlay hands focus back to the
//! player root explicitly (`keep_focus`) and the shortcuts keep working.
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_media::types::{SkipSegment, SkipType, StremioSubtitle};
use mv_ui::nav::{navigate, PlayRequest, Route};
use mv_ui::theme::{duration, easing, icon as icon_px, radius, ramp, size, Tokens};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    appear, bg, btn, copy_button, disclosure, error_view, glyphs, icon, icon_button, motion, ph, progress_bar, progress_ring_sized, shimmer_text, skeleton, spinner, ButtonKind, Hue, Ph,
    RampExt as _, MONO_FONT,
};

use crate::backend::{self, Ident, NextInfo, Resolved};
use crate::logic::{self, PlaybackPrefs, BOTTOM_H, MENU_RIGHT, TOP_H};
use crate::mpv::{Event, Mpv, Track, TrackKind};
use crate::overlay::{self, Follower};
use crate::ui::{self, ctl, ctl_box, ctl_shell, frame_btn, glass_panel, jump_ctl, light, shade, text_ctl, track_row, video_bg};

const TICK: Duration = Duration::from_millis(33);
const POLL_EVERY: u32 = 4; // ticks (~130 ms)
const TRACKS_EVERY: u32 = 8; // polls (~1 s)
const SAVE_EVERY: Duration = Duration::from_secs(10);
/// Controls fade in / out (the kit's slow transition).
const FADE: Duration = duration::SLOW;
/// Scrubber grows on hover.
const GROW: Duration = Duration::from_millis(150);
/// Centre play / pause / volume indicator.
const FLASH: Duration = Duration::from_millis(700);
/// Live seeks while scrubbing are at most this frequent.
const SCRUB_SEEK_EVERY: Duration = Duration::from_millis(90);
const TRACKS_W: f32 = 580.0;
const SPEED_W: f32 = 240.0;
const UP_NEXT_W: f32 = 400.0;
/// Past this share of the episode the next one is looked up (and warmed when the
/// "Pre-download next episode" setting is on).
const PREFETCH_AT: f64 = 0.75;
/// Transport glyphs (play, rewind, forward) and the rest of the bar.
const TRANSPORT_PX: f32 = icon_px::HERO;
const BAR_PX: f32 = icon_px::XL;

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// Waiting for a torrent / source to be ready.
    Connecting,
    /// mpv created, file requested.
    Loading,
    Playing,
    Error(String),
}

/// Bottom-bar menus. Audio and subtitles share one menu (Netflix's two-column picker).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Tracks,
    Speed,
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Seek,
    Volume,
}

/// The following episode's stream lookup.
#[derive(Clone, Debug, PartialEq)]
enum NextState {
    Idle,
    Looking,
    Ready(PlayRequest),
    Failed,
}

/// Something fetched in the background for display.
#[derive(Clone, Debug, PartialEq)]
enum Fetch<T> {
    Idle,
    Loading,
    Done(T),
}

/// The Up Next card.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct UpNext {
    shown: bool,
    /// Cancelled for this pass through the end of the episode.
    dismissed: bool,
    /// Countdown seconds left (auto-play on).
    left: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct TorrentStats {
    peers: u32,
    speed: f64,
    progress: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Snap {
    pos: f64,
    dur: f64,
    paused: bool,
    volume: f64,
    muted: bool,
    speed: f64,
    buffering: bool,
    /// mpv's `cache-buffering-state` (0..100) while stalled.
    cache_pct: f64,
    /// Playback reached the end (mpv keeps the last frame and pauses).
    eof: bool,
}

pub struct PlayerView {
    req: PlayRequest,
    ident: Ident,
    focus: FocusHandle,
    /// The app window (owner of the video popup and the overlay).
    main: AnyWindowHandle,
    main_hwnd: Option<isize>,
    overlay: Option<AnyWindowHandle>,
    follower: Option<Follower>,
    fullscreen: bool,
    /// Overlay size (window pixels), written by a canvas each layout.
    ov_bounds: Rc<Cell<Bounds<Pixels>>>,
    seek_bounds: Rc<Cell<Bounds<Pixels>>>,
    vol_bounds: Rc<Cell<Bounds<Pixels>>>,
    mpv: Option<Mpv>,
    phase: Phase,
    started: Instant,
    info_hash: Option<String>,
    /// The source's info hash before the torrent is added (stats while connecting only).
    pending_hash: Option<String>,
    torrent: Option<TorrentStats>,
    snap: Snap,
    /// Buffered (instantly seekable) ranges.
    ranges: Vec<(f64, f64)>,
    tracks: Vec<Track>,
    /// Last subtitle track that was on, for the `c` toggle.
    last_sid: Option<i64>,
    skip: Vec<SkipSegment>,
    skip_requested: bool,
    subs_requested: bool,
    /// Intros already skipped automatically (start, deciseconds), so Undo does not loop.
    auto_skipped: Vec<i64>,
    /// Saved position playback resumes from; its "Resuming at" toast waits for the first frame,
    /// because mpv cannot seek (Start over) before the file is loaded.
    resume_note: Option<f64>,
    /// Settings → Playback.
    prefs: PlaybackPrefs,
    /// Backdrop for the loader and the Up Next card.
    artwork: Option<String>,
    next: NextState,
    /// Play the next episode as soon as its lookup finishes.
    next_wanted: bool,
    /// The next episode's torrent has been warmed.
    next_warm: bool,
    next_info: Fetch<Option<NextInfo>>,
    up_next: UpNext,
    menu: Option<Menu>,
    /// Bumped on every menu open so its entrance animation replays.
    menu_serial: u32,
    /// The keyboard shortcut sheet (`?`).
    help: bool,
    drag: Option<Drag>,
    /// Scrub handle position while dragging.
    scrub: Option<f64>,
    /// Last live seek sent while scrubbing.
    scrub_sent: Option<(Instant, f64)>,
    /// Seek target shown until mpv catches up.
    pending_seek: Option<(f64, Instant)>,
    /// Mouse x over the seek bar (hover tooltip).
    hover_x: Option<f32>,
    seek_hot: bool,
    seek_hot_at: Instant,
    ticks: u32,
    polls: u32,
    last_poll: Instant,
    last_save: Instant,
    played_logged: bool,
    complete_logged: bool,
    /// Centre indicator: glyph (asset path), label, start.
    flash: Option<(Option<&'static str>, String, Instant)>,
    // Overlay visibility.
    last_input: Instant,
    controls: bool,
    controls_at: Instant,
    cursor_hidden: bool,
    last_cursor: Option<(i32, i32)>,
    mouse_in_bar: bool,
    /// Mouse-down point on the top bar (drag to move the window).
    title_drag: Option<Point<Pixels>>,
    /// The video popup is shown once there is a picture to show.
    video_shown: bool,
    /// Set once Back was requested, so repeated clicks / keys don't pop twice.
    leaving: bool,
}

actions!(player, [StayOnPlayer]);

/// Marks the player's key bindings as registered (once per app).
struct PlayerKeys;
impl Global for PlayerKeys {}

/// Tab / Shift+Tab on the player. The kit root binds them to "focus next", and the next Tab stops
/// in the app window are the shell's title-bar buttons, hidden behind the video: focus would land
/// on a control nobody can see, Space would then click it, and every player shortcut would stop
/// working until the next click. WCAG 2.4.11 Focus Not Obscured:
/// <https://www.w3.org/WAI/WCAG22/Understanding/focus-not-obscured-minimum.html>
///
/// Ctrl+K too: the shell's command palette would open in the app window, under the video, take
/// keyboard focus and silence every player shortcut until Escape. The shell skips keys an action
/// handled, so binding it here keeps it on the player (shell's `SHORTCUTS` table, app/src/keys.rs).
fn bind_keys(cx: &mut App) {
    if cx.has_global::<PlayerKeys>() {
        return;
    }
    cx.set_global(PlayerKeys);
    cx.bind_keys([
        KeyBinding::new("tab", StayOnPlayer, Some("Player")),
        KeyBinding::new("shift-tab", StayOnPlayer, Some("Player")),
        KeyBinding::new("ctrl-k", StayOnPlayer, Some("Player")),
    ]);
}

/// Opens the player for `Route::Player`.
pub fn open_view(req: PlayRequest, window: &mut Window, cx: &mut App) -> AnyView {
    bind_keys(cx);
    cx.new(|cx| PlayerView::new(req, window, cx)).into()
}

impl PlayerView {
    fn new(req: PlayRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let ident = backend::ident(&req);
        let pending_hash = backend::source_hash(&req);
        let now = Instant::now();
        let mut this = Self {
            req,
            ident,
            focus,
            main: window.window_handle(),
            main_hwnd: overlay::hwnd_of(window),
            overlay: None,
            follower: None,
            fullscreen: window.is_fullscreen(),
            ov_bounds: Rc::new(Cell::new(Bounds::default())),
            seek_bounds: Rc::new(Cell::new(Bounds::default())),
            vol_bounds: Rc::new(Cell::new(Bounds::default())),
            mpv: None,
            phase: Phase::Connecting,
            started: now,
            info_hash: None,
            pending_hash,
            torrent: None,
            snap: Snap { volume: 100.0, speed: 1.0, ..Default::default() },
            ranges: vec![],
            tracks: vec![],
            last_sid: None,
            skip: vec![],
            skip_requested: false,
            subs_requested: false,
            auto_skipped: vec![],
            resume_note: None,
            prefs: PlaybackPrefs::default(),
            artwork: None,
            next: NextState::Idle,
            next_wanted: false,
            next_warm: false,
            next_info: Fetch::Idle,
            up_next: UpNext::default(),
            menu: None,
            menu_serial: 0,
            help: false,
            drag: None,
            scrub: None,
            scrub_sent: None,
            pending_seek: None,
            hover_x: None,
            seek_hot: false,
            seek_hot_at: now,
            ticks: 0,
            polls: 0,
            last_poll: now,
            last_save: now,
            played_logged: false,
            complete_logged: false,
            flash: None,
            last_input: now,
            controls: true,
            controls_at: now - FADE,
            cursor_hidden: false,
            last_cursor: None,
            mouse_in_bar: false,
            title_drag: None,
            video_shown: false,
            leaving: false,
        };
        this.begin(window, cx);
        this.fetch_artwork(cx);
        this.open_overlay(window, cx);
        cx.on_release(|this, cx| {
            this.follower = None;
            if let Some(h) = this.overlay.take() {
                let _ = h.update(cx, |_, window, _| window.remove_window());
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if this.update_in(cx, |this, window, cx| this.tick(window, cx)).is_err() {
                break;
            }
        })
        .detach();
        this
    }

    /// Opens the transparent controls window over the app window (after this update, so the
    /// overlay's first render can read the player).
    fn open_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(main_hwnd) = self.main_hwnd else { return };
        let bounds = window.bounds();
        let display_id = window.display(cx).map(|d| d.id());
        let player = cx.entity().downgrade();
        cx.defer(move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: false,
                show: true,
                kind: WindowKind::PopUp,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                display_id,
                window_background: WindowBackgroundAppearance::Transparent,
                // The overlay is never the active window; animate at full rate anyway.
                inactive_frame_interval: None,
                ..Default::default()
            };
            let mut ov_hwnd = None;
            let p = player.clone();
            let Ok(handle) = cx.open_window(options, |w, cx| {
                ov_hwnd = overlay::hwnd_of(w);
                cx.new(|cx| OverlayRoot::new(p, w, cx))
            }) else {
                return;
            };
            let handle: AnyWindowHandle = handle.into();
            let kept = player
                .update(cx, |this, cx| {
                    if this.leaving {
                        return false;
                    }
                    let Some(ov) = ov_hwnd else { return false };
                    let f = Follower::new(main_hwnd, ov);
                    if let Some(m) = &this.mpv {
                        f.set_video(m.surface());
                    }
                    f.set_video_visible(this.mpv.is_some() && !matches!(this.phase, Phase::Error(_)));
                    this.follower = Some(f);
                    this.overlay = Some(handle);
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !kept {
                let _ = handle.update(cx, |_, w, _| w.remove_window());
            }
        });
    }

    /// Runs `f` with the app window (fullscreen, navigation…) after the current event.
    fn with_main(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static) {
        let (main, this) = (self.main, cx.entity().downgrade());
        cx.defer(move |cx| {
            let _ = main.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| f(this, window, cx));
            });
        });
    }

    /// Keyboard focus back on the player root (the shortcuts live there). Kit controls do not
    /// move focus on a mouse press, so nothing else would put it back after a click.
    fn keep_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.leaving && !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
    }

    /// Resolves the source (torrent metadata etc.) then starts mpv.
    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.phase = Phase::Connecting;
        self.started = Instant::now();
        self.set_video_shown(false);
        let req = self.req.clone();
        let ident = self.ident.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (resume, prefs) = bg::blocking(move || (backend::load_resume(&ident), backend::load_prefs())).await.unwrap_or_default();
            let resolved = bg::run(async move { backend::resolve(&req).await }).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| this.on_resolved(resolved, resume, prefs, window, cx));
        })
        .detach();
    }

    /// Loader / Up Next backdrop (anime too, through its MAL id).
    fn fetch_artwork(&mut self, cx: &mut Context<Self>) {
        let req = self.req.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(Some(url)) = bg::run(backend::artwork(req)).await {
                let _ = this.update(cx, |this, cx| {
                    this.artwork = Some(url);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn on_resolved(&mut self, resolved: anyhow::Result<Resolved>, resume: Option<f64>, prefs: PlaybackPrefs, _window: &mut Window, cx: &mut Context<Self>) {
        if self.leaving {
            return;
        }
        self.prefs = prefs;
        let r = match resolved {
            Ok(r) => r,
            Err(e) => {
                self.fail(format!("{e:#}"));
                cx.notify();
                return;
            }
        };
        self.info_hash = r.info_hash.clone();
        let created = match self.main_hwnd {
            Some(owner) => Mpv::create(owner),
            None => Err(anyhow::anyhow!("no native window handle")),
        };
        match created {
            Ok(m) => {
                // Settings → Playback → Subtitle language picks the embedded track mpv selects.
                let _ = m.set("slang", &logic::slang(&self.prefs.subtitle_language));
                self.resume_note = resume;
                if let Some(t) = resume {
                    let _ = m.set("start", &format!("{t:.1}"));
                }
                if let Err(e) = m.load_str(&r.target) {
                    self.fail(format!("{e:#}"));
                } else {
                    self.phase = Phase::Loading;
                }
                if let Some(f) = &self.follower {
                    f.set_video(m.surface());
                }
                self.mpv = Some(m);
            }
            Err(e) => self.fail(format!("{e:#}")),
        }
        if matches!(self.phase, Phase::Error(_)) {
            self.mpv = None;
        }
        self.sync_video_popup();
        cx.notify();
    }

    /// Playback failed: log the raw error, show the friendly error screen.
    fn fail(&mut self, raw: String) {
        log_error("playback failed", &raw);
        self.phase = Phase::Error(raw);
    }

    /// "Resuming at 12:34" with Start over (YouTube / Netflix offer the same choice).
    fn resume_toast(&mut self, at: f64, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        toast::show(
            cx,
            Toast::info(format!("Resuming at {}", logic::format_time(at))).icon(glyphs::HISTORY).action("Start over", move |_, cx| {
                let _ = this.update(cx, |p, cx| p.seek_to(0.0, cx));
            }),
        );
        self.reveal();
    }

    /// `video_shown` switches the UI from the loader to the controls. The popup itself is shown
    /// as soon as mpv exists (the loader covers it): mpv only starts presenting reliably into a
    /// window that is visible when its swapchain is created.
    fn set_video_shown(&mut self, shown: bool) {
        self.video_shown = shown;
        self.sync_video_popup();
    }

    fn sync_video_popup(&self) {
        if let Some(f) = &self.follower {
            f.set_video_visible(self.mpv.is_some() && !matches!(self.phase, Phase::Error(_)));
        }
    }

    /// Brings the controls back (with a toast or a state change worth seeing).
    fn reveal(&mut self) {
        self.last_input = Instant::now();
        self.set_controls(true);
    }

    /// Big centre indicator (play / pause / volume / seek).
    fn flash(&mut self, glyph: Option<&'static str>, label: impl Into<String>) {
        self.flash = Some((glyph, label.into(), Instant::now()));
    }

    /// Shows / hides the controls, keeping the fade continuous if it is mid-way.
    fn set_controls(&mut self, visible: bool) {
        if self.controls == visible {
            return;
        }
        let o = logic::fade(self.controls, self.controls_at.elapsed(), FADE).clamp(0.0, 1.0);
        // Start the new fade from the current opacity (ease-out cubic inverse).
        let from = if visible { o } else { 1.0 - o };
        let t = 1.0 - (1.0 - from).cbrt();
        self.controls = visible;
        self.controls_at = Instant::now() - FADE.mul_f32(t.clamp(0.0, 1.0));
        if !visible {
            self.hover_x = None;
            self.mouse_in_bar = false;
        }
    }

    fn set_seek_hot(&mut self, hot: bool) {
        if self.seek_hot != hot {
            let g = logic::fade(self.seek_hot, self.seek_hot_at.elapsed(), GROW);
            let from = if hot { g } else { 1.0 - g };
            let t = 1.0 - (1.0 - from).cbrt();
            self.seek_hot = hot;
            self.seek_hot_at = Instant::now() - GROW.mul_f32(t.clamp(0.0, 1.0));
        }
    }

    fn shown_pos(&self) -> f64 {
        logic::shown_position(self.scrub, self.pending_seek.map(|(t, _)| t), self.snap.pos)
    }

    // ── per-frame work ──

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ticks = self.ticks.wrapping_add(1);
        let fs = window.is_fullscreen();
        if fs != self.fullscreen {
            self.fullscreen = fs;
            cx.notify();
        }
        if let Some(f) = &self.follower {
            f.sync();
        }
        if !self.ticks.is_multiple_of(POLL_EVERY) {
            return;
        }
        self.polls = self.polls.wrapping_add(1);
        let dt = self.last_poll.elapsed().as_secs_f64().min(1.0);
        self.last_poll = Instant::now();
        let mut changed = false;

        // Torrent stats (connecting screen and buffering).
        if self.polls.is_multiple_of(4) {
            if let (Some(h), Some(engine)) = (self.info_hash.as_ref().or(self.pending_hash.as_ref()), backend::engine_if_started()) {
                let stats = engine.stats(h).ok().map(|s| TorrentStats { peers: s.peers, speed: s.download_speed, progress: s.progress });
                if stats != self.torrent {
                    self.torrent = stats;
                    changed = true;
                }
            }
        }
        self.update_overlay_state(cx, &mut changed);

        let Some(mpv) = &self.mpv else {
            if self.phase == Phase::Connecting && self.polls.is_multiple_of(2) {
                changed = true; // slow-start hint
            }
            if changed {
                cx.notify();
            }
            return;
        };

        let (mut loaded, mut ended, mut failed) = (false, false, None);
        for ev in mpv.poll_events() {
            match ev {
                Event::FileLoaded => loaded = true,
                Event::Ended => ended = true,
                Event::Failed(m) => failed = Some(m),
            }
        }

        let paused_for_cache = mpv.get_bool("paused-for-cache").unwrap_or(false);
        let seeking = mpv.get_bool("seeking").unwrap_or(false);
        let snap = Snap {
            pos: mpv.get_f64("time-pos").unwrap_or(self.snap.pos),
            dur: mpv.get_f64("duration").unwrap_or(self.snap.dur),
            paused: mpv.get_bool("pause").unwrap_or(false),
            volume: mpv.get_f64("volume").unwrap_or(self.snap.volume),
            muted: mpv.get_bool("mute").unwrap_or(false),
            speed: mpv.get_f64("speed").unwrap_or(1.0),
            buffering: paused_for_cache || (self.phase == Phase::Loading && !loaded) || (seeking && self.pending_seek.is_some_and(|(_, at)| at.elapsed() > Duration::from_millis(700))),
            cache_pct: if paused_for_cache { mpv.get_f64("cache-buffering-state").unwrap_or(0.0) } else { 0.0 },
            eof: ended || mpv.get_bool("eof-reached").unwrap_or(false),
        };
        if self.polls.is_multiple_of(TRACKS_EVERY) || loaded {
            let t = mpv.tracks();
            if t != self.tracks {
                self.tracks = t;
                changed = true;
            }
        }
        if self.polls.is_multiple_of(2) || loaded {
            let r = mpv.cache_ranges();
            if r != self.ranges {
                self.ranges = r;
                changed = true;
            }
        }
        if snap != self.snap {
            self.snap = snap;
            changed = true;
        }
        if let Some((t, at)) = self.pending_seek {
            if logic::seek_settled(t, self.snap.pos, at.elapsed()) && !seeking {
                self.pending_seek = None;
                changed = true;
            }
        }

        if let Some(msg) = failed {
            self.fail(msg);
            self.set_video_shown(false);
            changed = true;
        } else if loaded || (self.phase == Phase::Loading && self.snap.dur > 0.0) {
            self.phase = Phase::Playing;
            changed = true;
        }
        // Reveal the picture once playback really runs (the loader stays until then).
        if !self.video_shown && self.phase == Phase::Playing && (!self.snap.buffering || self.snap.pos > 0.0) {
            self.set_video_shown(true);
            changed = true;
            // Now the picture is up and seeking works, so Start over does what it says.
            if let Some(t) = self.resume_note.take() {
                self.resume_toast(t, cx);
            }
        }
        if loaded {
            self.on_loaded(cx);
        }
        if ended {
            self.save("complete");
            self.complete_logged = true;
            changed = true;
        }

        if self.snap.dur > 0.0 && self.skip.is_empty() && !self.skip_requested {
            self.fetch_skip(cx);
        }
        if self.last_save.elapsed() >= SAVE_EVERY && self.snap.dur > 0.0 && !self.snap.paused {
            self.save("progress");
        }
        if self.phase == Phase::Playing {
            self.auto_skip(cx);
            if self.update_up_next(dt, window, cx) {
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Auto-hide of controls and cursor.
    fn update_overlay_state(&mut self, cx: &mut Context<Self>, changed: &mut bool) {
        // Cursor polling catches movement even if no mouse event reached GPUI.
        let cursor = self.follower.as_ref().and_then(|f| f.cursor_inside());
        if cursor.is_some() && cursor != self.last_cursor && self.last_cursor.is_some() {
            self.activity(cx);
        }
        self.last_cursor = cursor;
        let visible = logic::controls_visible(self.last_input.elapsed(), self.pinned());
        if visible != self.controls {
            self.set_controls(visible);
            *changed = true;
        } else if !visible && !self.cursor_hidden && self.controls_at.elapsed() > FADE {
            self.cursor_hidden = true;
            if let Some(f) = &self.follower {
                f.set_cursor_hidden(true);
            }
        }
    }

    /// Controls stay up while paused, loading, scrubbing, in a menu or hovering a bar.
    fn pinned(&self) -> bool {
        self.phase != Phase::Playing || self.snap.paused || self.menu.is_some() || self.help || self.drag.is_some() || self.mouse_in_bar
    }

    /// Loading / buffering indicator over the video.
    fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Error(_)) && (self.mpv.is_none() || self.phase == Phase::Loading || self.snap.buffering)
    }

    fn skip_segment(&self) -> Option<SkipSegment> {
        (self.phase == Phase::Playing).then(|| logic::active_segment(&self.skip, self.snap.pos)).flatten()
    }

    fn up_next_visible(&self) -> bool {
        self.up_next.shown && !self.up_next.dismissed && !matches!(self.phase, Phase::Error(_)) && !self.leaving
    }

    /// The next-episode lookup has been asked for and is still running.
    fn next_busy(&self) -> bool {
        self.next_wanted && self.next == NextState::Looking
    }

    /// The request came from a title's source list, so going back offers other sources.
    fn can_switch_source(&self) -> bool {
        !matches!(logic::classify_source(&self.req.source), logic::SourceKind::File(_))
            && (self.req.imdb_id.as_deref().is_some_and(|s| !s.is_empty()) || self.req.mal_id.as_deref().is_some_and(|s| !s.is_empty()))
    }

    fn activity(&mut self, cx: &mut Context<Self>) {
        self.last_input = Instant::now();
        if self.cursor_hidden {
            self.cursor_hidden = false;
            if let Some(f) = &self.follower {
                f.set_cursor_hidden(false);
            }
        }
        if !self.controls {
            self.set_controls(true);
            cx.notify();
        }
    }

    fn on_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.played_logged {
            self.played_logged = true;
            let (ident, dur) = (self.ident.clone(), self.snap.dur);
            bg::runtime().spawn_blocking(move || {
                let _ = backend::save_event(&ident, "play", 0.0, dur);
            });
        }
        self.fetch_skip(cx);
        self.fetch_subtitles(cx);
        self.fetch_next_info(cx);
    }

    fn fetch_skip(&mut self, cx: &mut Context<Self>) {
        if self.skip_requested || self.snap.dur <= 0.0 {
            return;
        }
        let (Some(mal), Some(ep)) = (self.req.mal_id.as_deref().and_then(|m| m.parse::<u64>().ok()), self.req.episode) else {
            self.skip_requested = true;
            return;
        };
        self.skip_requested = true;
        let dur = self.snap.dur;
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(segs)) = bg::run(async move { backend::skip_segments(mal, ep, dur).await }).await {
                let _ = this.update(cx, |this, cx| {
                    this.skip = segs;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn fetch_subtitles(&mut self, cx: &mut Context<Self>) {
        if self.subs_requested {
            return;
        }
        self.subs_requested = true;
        let Some(imdb) = self.req.imdb_id.clone().filter(|s| !s.is_empty()) else { return };
        let (season, episode) = (self.req.season, self.req.episode);
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(subs)) = bg::run(async move { backend::subtitles(imdb, season, episode).await }).await {
                let _ = this.update(cx, |this, cx| {
                    this.add_subtitles(&subs);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Title and still of the next episode for the Up Next card.
    fn fetch_next_info(&mut self, cx: &mut Context<Self>) {
        if self.next_info != Fetch::Idle || !backend::has_next(&self.req) {
            return;
        }
        self.next_info = Fetch::Loading;
        let req = self.req.clone();
        cx.spawn(async move |this, cx| {
            let info = bg::run(backend::next_info(req)).await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.next_info = Fetch::Done(info);
                cx.notify();
            });
        })
        .detach();
    }

    /// Adds external subtitles (max 3 per language); `auto` keeps mpv's own language preference.
    fn add_subtitles(&mut self, subs: &[StremioSubtitle]) {
        let Some(mpv) = &self.mpv else { return };
        let mut per_lang: std::collections::HashMap<&str, usize> = Default::default();
        for s in subs {
            let n = per_lang.entry(s.lang.as_str()).or_default();
            if *n >= 3 {
                continue;
            }
            *n += 1;
            let _ = mpv.sub_add(&s.url, &format!("{} {}", logic::language_name(&s.lang), n), &s.lang);
        }
        self.tracks = mpv.tracks();
    }

    /// Persist progress (`progress` becomes `complete` past 90 %).
    fn save(&mut self, kind: &str) {
        self.last_save = Instant::now();
        let (pos, dur) = (self.snap.pos, self.snap.dur);
        if dur <= 0.0 {
            return;
        }
        let kind = if kind == "complete" || logic::is_complete(pos, dur) { "complete" } else { "progress" };
        if kind == "complete" {
            self.complete_logged = true;
        }
        let ident = self.ident.clone();
        bg::runtime().spawn_blocking(move || {
            let _ = backend::save_event(&ident, kind, pos.min(dur), dur);
        });
    }

    // ── Settings-driven behaviour: skip intro, Up Next, pre-download ──

    /// Settings → "Skip intro automatically": jumps past an opening once, with Undo.
    fn auto_skip(&mut self, cx: &mut Context<Self>) {
        if !self.prefs.skip_intro || self.drag.is_some() || self.pending_seek.is_some() {
            return;
        }
        let Some(seg) = self.skip_segment().filter(|s| logic::is_intro(s.kind)) else { return };
        let key = (seg.start * 10.0) as i64;
        if self.auto_skipped.contains(&key) {
            return;
        }
        self.auto_skipped.push(key);
        let from = self.snap.pos;
        self.seek_to(seg.end, cx);
        let this = cx.entity().downgrade();
        toast::show(
            cx,
            Toast::info("Skipped intro").icon(ph::SKIP_FORWARD).action("Undo", move |_, cx| {
                let _ = this.update(cx, |p, cx| p.seek_to(from, cx));
            }),
        );
    }

    /// Shows the Up Next card near the end, runs the auto-play countdown and warms the next
    /// episode. True when something on screen changed.
    fn update_up_next(&mut self, dt: f64, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.leaving || !backend::has_next(&self.req) {
            return false;
        }
        let dur = self.snap.dur;
        let Some(start) = logic::up_next_start(dur, self.prefs.countdown, logic::outro_start(&self.skip)) else { return false };
        let pos = self.shown_pos();
        if pos >= dur * PREFETCH_AT {
            self.ensure_next(self.prefs.pre_download_next, window, cx);
        }
        if pos < start && !self.snap.eof {
            // Seeking back before the credits resets the card.
            let was = self.up_next != UpNext::default();
            self.up_next = UpNext::default();
            return was;
        }
        if self.up_next.dismissed {
            return false;
        }
        // The lookup already failed (the last episode, or no source for the next one): no card
        // counting down to an episode that will not play. N and the Next button still retry.
        if self.next == NextState::Failed && !self.next_wanted {
            let was = self.up_next.shown;
            self.up_next.dismissed = true;
            return was;
        }
        let mut changed = false;
        if !self.up_next.shown {
            self.up_next = UpNext { shown: true, dismissed: false, left: self.prefs.countdown as f64 };
            self.ensure_next(self.prefs.pre_download_next, window, cx);
            changed = true;
        }
        if self.prefs.auto_play && !self.next_wanted {
            // The countdown waits while paused (except on the last frame) and while a menu or the
            // shortcut sheet is open, so the episode never changes mid-choice (WCAG 2.2.1).
            let choosing = self.menu.is_some() || self.help;
            if !choosing && (!self.snap.paused || self.snap.eof) {
                self.up_next.left -= dt;
                changed = true;
            }
            if self.up_next.left <= 0.0 {
                self.play_next(window, cx);
            }
        }
        changed
    }

    /// Starts the next-episode lookup if it has not run (or retries a failed one that is wanted).
    /// `warm` also pre-downloads its torrent.
    fn ensure_next(&mut self, warm: bool, window: &mut Window, cx: &mut Context<Self>) {
        match &self.next {
            NextState::Ready(next) => {
                if warm && !self.next_warm {
                    self.next_warm = true;
                    backend::prefetch(next);
                }
                return;
            }
            NextState::Looking => {
                self.next_warm |= warm;
                return;
            }
            NextState::Failed if !self.next_wanted => return,
            NextState::Failed | NextState::Idle => {}
        }
        self.next = NextState::Looking;
        self.next_warm |= warm;
        let req = self.req.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = bg::run(backend::next_episode(req)).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| this.on_next(res, window, cx));
        })
        .detach();
    }

    fn on_next(&mut self, res: anyhow::Result<PlayRequest>, window: &mut Window, cx: &mut Context<Self>) {
        if self.leaving {
            return;
        }
        match res {
            Ok(next) => {
                if self.next_warm {
                    backend::prefetch(&next);
                }
                self.next = NextState::Ready(next.clone());
                if self.next_wanted {
                    self.go_next(next, window, cx);
                }
            }
            Err(e) => {
                self.next = NextState::Failed;
                if self.next_wanted {
                    self.next_wanted = false;
                    // Stop the countdown from retrying every frame; the toast offers Retry.
                    self.up_next.dismissed = true;
                    let this = cx.entity().downgrade();
                    let ep = self.req.episode.map(|e| e + 1).unwrap_or(0);
                    log_error("next episode", &e);
                    toast::show(
                        cx,
                        Toast::error(format!("Couldn't find a source for episode {ep}")).action("Retry", move |_, cx| {
                            let _ = this.update(cx, |p, cx| p.with_main(cx, |p, window, cx| p.play_next(window, cx)));
                        }),
                    );
                }
            }
        }
        cx.notify();
    }

    // ── actions ──

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            // At the end of the file (keep-open) pressing play restarts.
            if self.snap.dur > 0.0 && self.snap.pos >= self.snap.dur - 0.5 && self.snap.paused {
                m.seek_absolute(0.0);
            }
            self.snap.paused = !self.snap.paused;
            m.set_pause(self.snap.paused);
            let paused = self.snap.paused;
            self.flash(Some(if paused { ph::PAUSE.fill() } else { ph::PLAY.fill() }), "");
            cx.notify();
        }
    }

    fn seek_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        if self.mpv.is_none() {
            return;
        }
        // Relative to what the UI shows, so repeated presses accumulate before mpv catches up.
        let t = logic::seek_target(self.shown_pos(), delta, self.snap.dur);
        self.seek_to(t, cx);
        let label = if delta < 0.0 { format!("-{}s", -delta as i64) } else { format!("+{}s", delta as i64) };
        self.flash(Some(if delta < 0.0 { ph::ARROW_COUNTER_CLOCKWISE.bold() } else { ph::ARROW_CLOCKWISE.bold() }), label);
    }

    fn seek_to(&mut self, t: f64, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            m.seek(t, true);
            self.pending_seek = Some((t, Instant::now()));
            cx.notify();
        }
    }

    fn set_volume(&mut self, v: f64, cx: &mut Context<Self>) {
        let v = logic::step_volume(v, 0.0);
        if let Some(m) = &self.mpv {
            let _ = m.set("volume", &format!("{v:.0}"));
            if v > 0.0 && self.snap.muted {
                let _ = m.set("mute", "no");
                self.snap.muted = false;
            }
        }
        self.snap.volume = v;
        cx.notify();
    }

    fn volume_flash(&mut self) {
        let v = if self.snap.muted { 0.0 } else { self.snap.volume };
        self.flash(Some(volume_glyph(self.snap.muted, v).fill()), format!("{v:.0}%"));
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        // Volume dragged to 0 already shows as muted ("Unmute"); unmuting brings the sound back
        // instead of muting silence (YouTube does the same).
        if self.mpv.is_some() && !self.snap.muted && self.snap.volume <= 0.0 {
            self.set_volume(50.0, cx);
            self.volume_flash();
            return;
        }
        if let Some(m) = &self.mpv {
            self.snap.muted = !self.snap.muted;
            let _ = m.set("mute", if self.snap.muted { "yes" } else { "no" });
            self.volume_flash();
            cx.notify();
        }
    }

    /// Recaps: background music off (the "Narration only" track) or back on.
    fn toggle_music(&mut self, cx: &mut Context<Self>) {
        let Some((full, solo)) = logic::music_tracks(&self.tracks) else { return };
        let off = self.tracks.iter().any(|t| t.id == solo && t.kind == TrackKind::Audio && t.selected);
        self.select_track(TrackKind::Audio, Some(if off { full } else { solo }), cx);
        self.flash(Some(ph::MUSIC_NOTES.fill()), if off { "Music on" } else { "Music off · narration only" });
    }

    fn set_speed(&mut self, s: f64, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            let _ = m.set("speed", &format!("{s}"));
            self.snap.speed = s;
            self.menu = None;
            cx.notify();
        }
    }

    /// Shift+< / Shift+>: one step down or up the speed list.
    fn step_speed(&mut self, up: bool, cx: &mut Context<Self>) {
        let s = logic::speed_step(self.snap.speed, up);
        self.set_speed(s, cx);
        self.flash(Some(ph::GAUGE.bold()), if (s - 1.0).abs() < 0.01 { "Normal speed".to_string() } else { logic::format_rate(s) });
    }

    fn select_track(&mut self, kind: TrackKind, id: Option<i64>, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            let (prop, val) = match (kind, id) {
                (TrackKind::Audio, Some(i)) => ("aid", i.to_string()),
                (TrackKind::Audio, None) => ("aid", "no".into()),
                (TrackKind::Sub, Some(i)) => ("sid", i.to_string()),
                (TrackKind::Sub, None) => ("sid", "no".into()),
            };
            let _ = m.set(prop, &val);
            if kind == TrackKind::Sub && id.is_some() {
                self.last_sid = id;
            }
            self.tracks = m.tracks();
            cx.notify();
        }
    }

    /// `c`: subtitles off, or back on with the last (or first) track (YouTube).
    fn toggle_subtitles(&mut self, cx: &mut Context<Self>) {
        if self.mpv.is_none() {
            return;
        }
        let subs: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Sub).collect();
        if let Some(cur) = subs.iter().find(|t| t.selected).map(|t| t.id) {
            self.last_sid = Some(cur);
            self.select_track(TrackKind::Sub, None, cx);
            self.flash(Some(glyphs::SUBTITLES.regular()), "Subtitles off");
            return;
        }
        let pick = self.last_sid.and_then(|id| subs.iter().find(|t| t.id == id)).or(subs.first()).map(|t| (t.id, logic::track_label(t)));
        match pick {
            Some((id, label)) => {
                self.select_track(TrackKind::Sub, Some(id), cx);
                self.flash(Some(glyphs::SUBTITLES.fill()), label);
            }
            None => self.flash(Some(ph::SUBTITLES_SLASH.regular()), "No subtitles for this video"),
        }
        cx.notify();
    }

    fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.help = false;
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            self.menu_serial = self.menu_serial.wrapping_add(1);
            Some(menu)
        };
        cx.notify();
    }

    fn toggle_help(&mut self, cx: &mut Context<Self>) {
        self.help = !self.help;
        self.menu = None;
        cx.notify();
    }

    fn toggle_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.toggle_fullscreen();
        self.fullscreen = !self.fullscreen;
        cx.notify();
    }

    fn skip_now(&mut self, cx: &mut Context<Self>) {
        if let Some(seg) = self.skip_segment() {
            self.seek_to(seg.end, cx);
        }
    }

    /// `,` / `.`: one frame back or forward (pauses), YouTube style.
    fn frame_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            let _ = m.command(&[if forward { "frame-step" } else { "frame-back-step" }]);
            self.snap.paused = true;
            cx.notify();
        }
    }

    /// Saves progress and hides the video + overlay before the page goes away.
    fn leave(&mut self) {
        self.save("progress");
        if let Some(m) = &self.mpv {
            m.set_pause(true);
        }
        if let Some(f) = &self.follower {
            f.set_cursor_hidden(false);
            f.set_visible(false);
        }
    }

    fn exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.leaving, true) {
            return;
        }
        if window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.leave();
        // Player toasts (Undo a skip, Start over) mean nothing once the player is gone.
        toast::dismiss(cx);
        window.defer(cx, |window, cx| navigate(Route::Back, window, cx));
    }

    /// Plays the following episode: at once when its lookup is done, else when it finishes.
    fn play_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.leaving || !backend::has_next(&self.req) {
            return;
        }
        if let NextState::Ready(next) = &self.next {
            let next = next.clone();
            self.go_next(next, window, cx);
            return;
        }
        if self.next_wanted {
            return;
        }
        self.next_wanted = true;
        if self.next != NextState::Looking {
            self.ensure_next(self.prefs.pre_download_next, window, cx);
        }
        // The Up Next card's Play now already shows a spinner.
        if !self.up_next_visible() {
            toast::show(cx, Toast::info("Finding the next episode…").icon(ph::SKIP_FORWARD));
        }
        self.reveal();
        cx.notify();
    }

    /// Replaces this player with `next`.
    fn go_next(&mut self, next: PlayRequest, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.leaving, true) {
            return;
        }
        // Same torrent (season pack): keep it alive for the next player.
        if self.info_hash.as_ref().is_some_and(|h| next.source.to_ascii_lowercase().contains(&h.to_ascii_lowercase())) {
            self.info_hash = None;
        }
        self.leave();
        toast::dismiss(cx);
        // Back first so history stays `detail → player`.
        window.defer(cx, move |window, cx| {
            navigate(Route::Back, window, cx);
            navigate(Route::Player { request: next }, window, cx);
        });
    }

    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mpv = None;
        self.begin(window, cx);
        cx.notify();
    }

    /// Keyboard (from the app window, which keeps focus; the overlay forwards here too).
    /// YouTube's documented player keys: <https://support.google.com/youtube/answer/7631406>
    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        if m.control || m.alt || m.platform {
            return;
        }
        self.activity(cx);
        let key = k.key.as_str();
        let ch = k.key_char.as_deref().unwrap_or("");
        // Punctuation is matched on the character typed when there is one, so layouts with their
        // own `<` key (German, Nordic: Shift+< is `>`) get the right direction; without it, as the
        // character or Shift + the US base key.
        let is = |c: &str, base: &str| if ch.is_empty() { key == c || (m.shift && key == base) } else { ch == c };
        if is("?", "/") {
            self.toggle_help(cx);
            return;
        }
        if is("<", ",") {
            self.step_speed(false, cx);
            return;
        }
        if is(">", ".") {
            self.step_speed(true, cx);
            return;
        }
        match key {
            "space" | "k" => self.toggle_pause(cx),
            "left" | "j" => self.seek_by(-10.0, cx),
            "right" | "l" => self.seek_by(10.0, cx),
            "up" => {
                self.set_volume(self.snap.volume + 5.0, cx);
                self.volume_flash();
            }
            "down" => {
                self.set_volume(self.snap.volume - 5.0, cx);
                self.volume_flash();
            }
            "f" => self.toggle_fullscreen(window, cx),
            "m" => self.toggle_mute(cx),
            "n" => self.play_next(window, cx),
            "s" => self.skip_now(cx),
            "c" => self.toggle_subtitles(cx),
            // Unshifted only: Shift+, / Shift+. are other characters on many layouts.
            "," if !m.shift => self.frame_step(false, cx),
            "." if !m.shift => self.frame_step(true, cx),
            "home" => self.seek_to(0.0, cx),
            "end" if self.snap.dur > 0.0 => self.seek_to((self.snap.dur - 1.0).max(0.0), cx),
            d if !m.shift && d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                if let Some(t) = logic::percent_target(u32::from(d.as_bytes()[0] - b'0'), self.snap.dur) {
                    self.seek_to(t, cx);
                    self.flash(None, logic::format_time(t));
                }
            }
            "escape" => {
                if self.help {
                    self.help = false;
                    cx.notify();
                } else if self.menu.is_some() {
                    self.menu = None;
                    cx.notify();
                } else if window.is_fullscreen() {
                    self.toggle_fullscreen(window, cx);
                } else {
                    self.exit(window, cx);
                }
            }
            _ => {}
        }
    }

    // ── mouse (overlay) ──

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let pos = ev.position;
        self.activity(cx);
        if let Some(start) = self.title_drag {
            let d = pos - start;
            if ev.pressed_button == Some(MouseButton::Left) && (d.x.as_f32().abs() > 4.0 || d.y.as_f32().abs() > 4.0) {
                self.title_drag = None;
                if let Some(f) = &self.follower {
                    f.begin_move();
                }
                return;
            }
        }
        let ob = self.ov_bounds.get();
        let y = (pos.y - ob.origin.y).as_f32();
        let in_bar = self.controls && (y < TOP_H || y > ob.size.height.as_f32() - BOTTOM_H);
        let hover = (self.controls && self.seek_bounds.get().contains(&pos)).then_some(pos.x.as_f32());
        if hover != self.hover_x || in_bar != self.mouse_in_bar {
            self.hover_x = hover;
            self.mouse_in_bar = in_bar;
            cx.notify();
        }
        self.drag_to(pos.x.as_f32(), cx);
    }

    fn drag_to(&mut self, x: f32, cx: &mut Context<Self>) {
        match self.drag {
            Some(Drag::Seek) => {
                let b = self.seek_bounds.get();
                let f = logic::fraction_in(x, b.origin.x.as_f32(), b.size.width.as_f32());
                let t = f * self.snap.dur;
                self.scrub = Some(t);
                // Live preview: cheap keyframe seeks, throttled.
                let due = self.scrub_sent.is_none_or(|(at, last)| at.elapsed() >= SCRUB_SEEK_EVERY && (last - t).abs() > 0.5);
                if due {
                    if let Some(m) = &self.mpv {
                        m.seek(t, false);
                    }
                    self.scrub_sent = Some((Instant::now(), t));
                }
                cx.notify();
            }
            Some(Drag::Volume) => {
                let b = self.vol_bounds.get();
                let f = logic::fraction_in(x, b.origin.x.as_f32(), b.size.width.as_f32());
                self.set_volume(f * 130.0, cx);
            }
            None => {}
        }
    }

    fn drag_end(&mut self, cx: &mut Context<Self>) {
        self.title_drag = None;
        if self.drag == Some(Drag::Seek) {
            if let Some(t) = self.scrub.take() {
                // Exact seek on release.
                self.seek_to(t, cx);
            }
            self.scrub_sent = None;
            if self.hover_x.is_none() {
                self.set_seek_hot(false);
            }
        }
        self.drag = None;
    }

    fn on_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let dy = ev.delta.pixel_delta(px(20.)).y.as_f32();
        if dy.abs() < 0.5 || self.mpv.is_none() || self.menu.is_some() || self.help {
            return;
        }
        self.activity(cx);
        self.set_volume(self.snap.volume + if dy > 0.0 { 5.0 } else { -5.0 }, cx);
        self.volume_flash();
    }

    // ── rendering ──

    fn menu_size(&self, menu: Menu) -> (f32, f32) {
        let (w, h) = match menu {
            Menu::Tracks => {
                let count = |k| self.tracks.iter().filter(|t| t.kind == k).count();
                (TRACKS_W, logic::menu_height(count(TrackKind::Audio).max(count(TrackKind::Sub) + 1).max(2)))
            }
            Menu::Speed => (SPEED_W, logic::menu_height(logic::SPEEDS.len())),
        };
        // Short windows: shrink to the room above the bar (the lists scroll) instead of running
        // under the top bar and losing the column heads.
        (w, logic::menu_fit(h, self.ov_bounds.get().size.height.as_f32()))
    }

    /// The overlay window's content. `in_overlay` is false for the fallback drawn in the app
    /// window (which already has the shell's toast layer).
    fn render_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>, in_overlay: bool) -> AnyElement {
        let fade_t = self.controls_at.elapsed();
        let o = logic::fade(self.controls, fade_t, FADE);
        let error = matches!(self.phase, Phase::Error(_));
        let mut animating = fade_t < FADE || self.seek_hot_at.elapsed() < GROW;
        if let Some((_, _, at)) = &self.flash {
            if at.elapsed() < FLASH {
                animating = true;
            } else {
                self.flash = None;
            }
        }
        if animating {
            window.request_animation_frame();
        }
        let menu = self.menu.filter(|_| self.controls && !error);
        let up_next = self.up_next_visible();
        // Up Next replaces "Skip outro" (Netflix swaps Skip Credits for Next Episode).
        let skip = self.skip_segment().filter(|_| !up_next);
        let ov_bounds = self.ov_bounds.clone();
        let loading_screen = !error && !self.video_shown;
        let bars = o > 0.01;
        // Toasts sit above the bottom bar while it shows.
        let toast_lift = if error { 0.0 } else { 96.0 * o };

        div()
            .id("player-overlay")
            .relative()
            .size_full()
            .overflow_hidden()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| this.on_mouse_move(ev, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _w, cx| this.drag_end(cx)))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _w, cx| this.on_wheel(ev, cx)))
            // Mouse back button: same as the back arrow.
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, _w, cx| {
                    cx.stop_propagation();
                    this.with_main(cx, |this, window, cx| this.exit(window, cx));
                }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _w, cx| {
                if !*hovered {
                    this.mouse_in_bar = false;
                    this.hover_x = None;
                    this.set_seek_hot(false);
                    cx.notify();
                }
            }))
            .child(canvas(move |b, _, _| ov_bounds.set(b), |_, _, _, _| {}).absolute().size_full())
            // Click target for the whole video: pause, double-click fullscreen, close menus.
            .child(div().id("stage").absolute().top_0().left_0().size_full().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                    if this.help || this.menu.is_some() {
                        this.help = false;
                        this.menu = None;
                        cx.notify();
                    } else if matches!(this.phase, Phase::Error(_)) {
                    } else if ev.click_count == 2 {
                        this.toggle_pause(cx); // undo the first click's pause
                        this.flash = None;
                        this.with_main(cx, |this, window, cx| this.toggle_fullscreen(window, cx));
                    } else if ev.click_count == 1 && this.mpv.is_some() {
                        this.toggle_pause(cx);
                    }
                }),
            ))
            // Above the stage so the loader's own button gets its clicks; empty loader space
            // still reaches the stage (no hitbox there).
            .when(loading_screen, |d| d.child(self.loading_screen(cx)))
            .when(!loading_screen && self.busy(), |d| d.child(self.buffering_badge()))
            .children(self.flash_view())
            .children(self.error_screen(cx))
            .when(bars, |d| d.child(self.top_bar(o, cx)))
            .when(bars && !error, |d| d.child(self.bottom_bar(o, cx)))
            .children(skip.map(|s| self.skip_button(s, o, cx)))
            .when(up_next && self.video_shown, |d| d.child(self.up_next_card(o, cx)))
            // After Skip and Up Next: they sit where the menu opens, and the menu the viewer just
            // opened must be on top and take the clicks.
            .children(menu.map(|m| self.menu_view(m, cx)))
            .when(self.help, |d| d.child(self.help_sheet(cx)))
            .when(in_overlay, |d| d.child(div().absolute().top_0().left_0().right_0().bottom(px(toast_lift)).child(toast::host(cx))))
            .into_any_element()
    }

    /// Full-screen loader before the first frame: dimmed artwork, title, live status, and after a
    /// long wait a way out ("Try another source"). Apple HIG Loading:
    /// <https://developer.apple.com/design/human-interface-guidelines/loading>
    fn loading_screen(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let torrent = matches!(logic::classify_source(&self.req.source), logic::SourceKind::Torrent(_));
        let status = if self.mpv.is_none() {
            if torrent {
                "Connecting to peers"
            } else {
                "Opening stream"
            }
        } else if self.snap.cache_pct > 0.0 {
            "Buffering"
        } else {
            "Starting playback"
        };
        let episode = logic::episode_label(&self.req.title, self.req.season, self.req.episode);
        let progress = if self.snap.cache_pct > 0.0 { Some((self.snap.cache_pct / 100.0) as f32) } else { None };
        let slow = logic::slow_start(self.started.elapsed(), torrent, self.torrent.map(|t| t.peers));
        let calm = motion::reduced();
        let backdrop = self.artwork.clone().map(|url| {
            let el = img(url).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover).with_fallback(|| div().into_any_element());
            if calm {
                el.opacity(0.6).into_any_element()
            } else {
                el.with_animation("backdrop-in", Animation::new(Duration::from_millis(900)).with_easing(easing::standard), |el, d| el.opacity(0.6 * d)).into_any_element()
            }
        });
        let hint = slow.then(|| {
            let (title, body) = if torrent && self.torrent.is_none_or(|t| t.peers == 0) {
                ("No one is sharing this file right now", "It may never start. Another source usually plays.")
            } else {
                ("This is taking longer than usual", "The source is slow to respond.")
            };
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .mt(px(14.))
                .max_w(px(460.))
                // Clicks on the hint never pause / play the video underneath.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).text_center().child(title))
                .child(div().ramp(ramp::CAPTION).text_color(Tokens::text_2()).text_center().child(body))
                .when(self.can_switch_source(), |d| {
                    d.child(
                        btn("load-switch", "Try another source", ButtonKind::Primary)
                            .icon(ph::SWAP)
                            .tooltip("Go back and pick a different source")
                            .mt(px(8.))
                            .on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.exit(window, cx)))),
                    )
                })
        });
        let el = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(video_bg())
            .children(backdrop)
            // Soft vignette so text stays readable over any backdrop.
            .child(div().absolute().top_0().left_0().size_full().bg(linear_gradient(180., linear_color_stop(shade(0.45), 0.), linear_color_stop(shade(0.88), 1.))))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .child(spinner("load-spin", Tokens::accent(), 48.))
                    .child(div().mt(px(10.)).max_w(relative(0.7)).truncate().ramp(ramp::TITLE).text_color(Tokens::text_1()).child(self.req.title.clone()))
                    .children(episode.map(|e| div().ramp(ramp::BODY_LARGE).text_color(Tokens::text_2()).child(e)))
                    .child(shimmer_text(status).ramp(ramp::BODY).text_color(Tokens::text_2()).mt(px(2.)))
                    .children(self.torrent.map(stat_chips))
                    .child(div().mt(px(10.)).w(px(280.)).child(progress_bar("load-progress", progress.unwrap_or(0.), true)))
                    .children(hint),
            );
        appear("loader-in", el)
    }

    /// Compact buffering indicator over a playing video.
    fn buffering_badge(&self) -> impl IntoElement {
        let label = if self.snap.cache_pct > 0.0 { format!("Buffering {:.0}%", self.snap.cache_pct) } else { "Buffering".to_string() };
        div().absolute().top_0().left_0().size_full().flex().items_center().justify_center().child(appear(
            "buffer-in",
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.))
                .px(px(30.))
                .py(px(24.))
                .rounded(px(radius::XL))
                .bg(Tokens::glass())
                .child(spinner("buffer-spin", Tokens::accent(), 40.))
                .child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child(label))
                .children(self.torrent.map(stat_chips)),
        ))
    }

    /// Fading centre indicator for play / pause / seek / volume (it grows a little as it fades,
    /// except under reduced motion).
    fn flash_view(&self) -> Option<impl IntoElement> {
        let (glyph, label, at) = self.flash.as_ref()?;
        let p = logic::ease_out(at.elapsed(), FLASH);
        let o = (1.0 - p).clamp(0.0, 1.0);
        let s = if motion::reduced() { 84.0 } else { 78.0 + 18.0 * p };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .opacity(o)
                .children(glyph.map(|g| div().size(px(s)).rounded_full().bg(Tokens::glass()).flex().items_center().justify_center().child(icon(g, Tokens::text_1(), s * 0.42))))
                .when(!label.is_empty(), |d| {
                    d.child(div().px(px(12.)).py(px(4.)).rounded(px(radius::MD)).bg(Tokens::glass()).ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child(label.clone()))
                }),
        )
    }

    /// Playback failure in plain words with a way forward; the raw error sits under Details.
    /// NN/g error messages: <https://www.nngroup.com/articles/error-message-guidelines/>
    fn error_screen(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let Phase::Error(raw) = &self.phase else { return None };
        let fe = logic::friendly_error(raw, matches!(logic::classify_source(&self.req.source), logic::SourceKind::File(_)));
        let switch = fe.other_source && self.can_switch_source();
        let retry = btn("err-retry", "Try again", if switch { ButtonKind::Secondary } else { ButtonKind::Primary })
            .icon(glyphs::RETRY)
            .on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.retry(window, cx))));
        let other = if switch {
            btn("err-switch", "Try another source", ButtonKind::Primary).icon(ph::SWAP).tooltip("Go back and pick a different source")
        } else {
            btn("err-back", "Back", ButtonKind::Secondary).icon(glyphs::BACK).kbd("escape")
        }
        .on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.exit(window, cx))));
        let (primary, secondary) = if switch { (other.into_any_element(), retry.into_any_element()) } else { (retry.into_any_element(), other.into_any_element()) };
        let details = div()
            .flex()
            .items_start()
            .gap(px(8.))
            .max_w(px(520.))
            .child(div().flex_1().min_w_0().font_family(MONO_FONT).text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(raw.clone()))
            .child(copy_button("err-copy", raw.clone()));
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .bg(video_bg())
                .child(appear(
                    "error-in",
                    error_view(fe.body).title(fe.title).action(primary).secondary(secondary).max_w(px(600.)).build().child(div().mt(px(6.)).child(disclosure("err-details", "Details").content(details))),
                )),
        )
    }

    fn top_bar(&self, o: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let episode = logic::episode_label(&self.req.title, self.req.season, self.req.episode);
        let windowed = !self.fullscreen;
        let maximized = self.follower.as_ref().is_some_and(|f| f.is_maximized());
        let slide = if motion::reduced() { 0.0 } else { -(1.0 - o) * 14.0 };
        div()
            .id("top-bar")
            .absolute()
            .top(px(slide))
            .left_0()
            .w_full()
            .h(px(TOP_H + 30.))
            .opacity(o)
            .flex()
            .items_start()
            .pl(px(18.))
            .pt(px(14.))
            .gap(px(12.))
            .bg(linear_gradient(180., linear_color_stop(shade(0.7), 0.), linear_color_stop(shade(0.), 1.)))
            // Empty top-bar space moves the window (drag) / maximizes it (double-click).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                    // Fullscreen: no window to move, let the click reach the video (pause).
                    if !windowed {
                        return;
                    }
                    cx.stop_propagation();
                    if ev.click_count == 2 {
                        this.title_drag = None;
                        if let Some(f) = &this.follower {
                            f.toggle_maximize();
                        }
                    } else {
                        this.title_drag = Some(ev.position);
                    }
                }),
            )
            .child(
                ctl("back", glyphs::BACK.bold(), BAR_PX, "Back", Some("escape"))
                    .bg(Tokens::glass())
                    .border_1()
                    .border_color(Tokens::glass_border())
                    .on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.exit(window, cx)))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(44.))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(div().truncate().ramp(ramp::SUBTITLE).text_color(Tokens::text_1()).child(self.req.title.clone()))
                    .children(episode.map(|e| div().ramp(ramp::BODY).text_color(Tokens::text_2()).child(e))),
            )
            .when(windowed, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .mt(px(-14.))
                        .child(frame_btn("win-min", IconName::WindowMinimize, false, "Minimize").on_click(cx.listener(|this, _, _w, _cx| {
                            if let Some(f) = &this.follower {
                                f.minimize();
                            }
                        })))
                        .child(
                            frame_btn("win-max", if maximized { IconName::WindowRestore } else { IconName::WindowMaximize }, false, if maximized { "Restore down" } else { "Maximize" }).on_click(
                                cx.listener(|this, _, _w, _cx| {
                                    if let Some(f) = &this.follower {
                                        f.toggle_maximize();
                                    }
                                }),
                            ),
                        )
                        .child(frame_btn("win-close", IconName::WindowClose, true, "Close").on_click(cx.listener(|this, _, _w, _cx| {
                            if let Some(f) = &this.follower {
                                f.close();
                            }
                        }))),
                )
            })
            // Thin strip along the top edge resizes the window (the overlay covers the real edge).
            .when(windowed && !maximized, |d| {
                d.child(
                    div()
                        .id("resize-top")
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h(px(5.))
                        .cursor(CursorStyle::ResizeUpDown)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _w, cx| {
                                cx.stop_propagation();
                                if let Some(f) = &this.follower {
                                    f.begin_resize_top();
                                }
                            }),
                        ),
                )
            })
    }

    fn bottom_bar(&self, o: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = &self.snap;
        let ready = self.mpv.is_some() && self.video_shown;
        let shown_pos = self.shown_pos();
        let time_text = if snap.dur > 0.0 {
            format!("{} / {}", logic::format_time(shown_pos), logic::format_time(snap.dur))
        } else {
            logic::format_time(shown_pos)
        };
        let muted = snap.muted || snap.volume <= 0.0;
        let left = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                ctl("play", if snap.paused { ph::PLAY.fill() } else { ph::PAUSE.fill() }, TRANSPORT_PX, if snap.paused { "Play" } else { "Pause" }, Some("space"))
                    .on_click(cx.listener(|this, _, _w, cx| this.toggle_pause(cx))),
            )
            .child(jump_ctl("rew", ph::ARROW_COUNTER_CLOCKWISE.bold(), TRANSPORT_PX, "Back 10 seconds", "j").on_click(cx.listener(|this, _, _w, cx| this.seek_by(-10.0, cx))))
            .child(jump_ctl("fwd", ph::ARROW_CLOCKWISE.bold(), TRANSPORT_PX, "Forward 10 seconds", "l").on_click(cx.listener(|this, _, _w, cx| this.seek_by(10.0, cx))))
            .child(ctl("mute", volume_glyph(snap.muted, snap.volume).fill(), BAR_PX, if muted { "Unmute" } else { "Mute" }, Some("m")).on_click(cx.listener(|this, _, _w, cx| this.toggle_mute(cx))))
            .child(self.volume_slider(cx))
            .child(div().ml(px(14.)).ramp(ramp::BODY).text_color(Tokens::text_2()).child(time_text));
        let menu = self.menu;
        let subs_on = self.tracks.iter().any(|t| t.kind == TrackKind::Sub && t.selected);
        let speed = snap.speed;
        let normal_speed = (speed - 1.0).abs() < 0.01;
        let right = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .when(backend::has_next(&self.req), |d| {
                d.child(if self.next_busy() {
                    ctl_shell("next", ctl_box(BAR_PX), "Finding the next episode…", None, false).child(spinner("next-spin", Tokens::text_1(), BAR_PX))
                } else {
                    ctl("next", ph::SKIP_FORWARD.fill(), BAR_PX, "Next episode", Some("n")).on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.play_next(window, cx))))
                })
            })
            .when_some(logic::music_tracks(&self.tracks), |d, (_, solo)| {
                let off = self.tracks.iter().any(|t| t.id == solo && t.kind == TrackKind::Audio && t.selected);
                d.child(
                    ctl("music", if off { ph::MUSIC_NOTES.regular() } else { ph::MUSIC_NOTES.fill() }, BAR_PX, if off { "Turn the music on" } else { "Turn the music off (narration only)" }, None)
                        .when(off, |c| c.opacity(0.55))
                        .on_click(cx.listener(|this, _, _w, cx| this.toggle_music(cx))),
                )
            })
            .child(
                ctl_shell("tracks", ctl_box(BAR_PX), "Audio & subtitles", None, menu == Some(Menu::Tracks))
                    .child(icon(glyphs::SUBTITLES.path(if subs_on { mv_ui::Weight::Fill } else { mv_ui::Weight::Bold }), Tokens::text_1(), BAR_PX))
                    .on_click(cx.listener(|this, _, _w, cx| this.toggle_menu(Menu::Tracks, cx))),
            )
            .child(
                if normal_speed {
                    ctl_shell("speed", ctl_box(BAR_PX), "Playback speed", None, menu == Some(Menu::Speed)).child(icon(ph::GAUGE.bold(), Tokens::text_1(), BAR_PX))
                } else {
                    text_ctl("speed", logic::format_rate(speed), BAR_PX, format!("Playback speed: {}", logic::format_rate(speed)), menu == Some(Menu::Speed))
                }
                .on_click(cx.listener(|this, _, _w, cx| this.toggle_menu(Menu::Speed, cx))),
            )
            .child(
                ctl("fs", if self.fullscreen { ph::CORNERS_IN.bold() } else { ph::CORNERS_OUT.bold() }, BAR_PX, if self.fullscreen { "Exit full screen" } else { "Full screen" }, Some("f"))
                    .on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.toggle_fullscreen(window, cx)))),
            );
        let slide = if motion::reduced() { 0.0 } else { -(1.0 - o) * 14.0 };
        div()
            .id("bottom-bar")
            .absolute()
            .bottom(px(slide))
            .left_0()
            .w_full()
            .h(px(BOTTOM_H + 40.))
            .opacity(o)
            .px(px(24.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .justify_end()
            .gap(px(8.))
            .bg(linear_gradient(0., linear_color_stop(shade(0.85), 0.), linear_color_stop(shade(0.), 1.)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(ready, |d| d.child(div().flex().items_center().child(self.scrubber(cx))).child(div().flex().items_center().justify_between().h(px(56.)).child(left).child(right)))
    }

    fn scrubber(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dur = self.snap.dur;
        let pos = self.shown_pos();
        let frac = |t: f64| if dur > 0.0 { (t / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let g = logic::fade(self.seek_hot || self.drag == Some(Drag::Seek), self.seek_hot_at.elapsed(), GROW);
        let track_h = 4.0 + 3.0 * g;
        let thumb = 6.0 + 10.0 * g;
        let b = self.seek_bounds.get();
        let hover = self.hover_x.filter(|_| dur > 0.0).map(|x| logic::fraction_in(x, b.origin.x.as_f32(), b.size.width.as_f32()));
        let bounds = self.seek_bounds.clone();
        div()
            .id("seek")
            .relative()
            .flex_1()
            .h(px(28.))
            .flex()
            .items_center()
            .cursor_pointer()
            .on_hover(cx.listener(|this, hovered: &bool, _w, cx| {
                if this.drag != Some(Drag::Seek) {
                    this.set_seek_hot(*hovered);
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                    cx.stop_propagation();
                    if this.snap.dur > 0.0 {
                        this.drag = Some(Drag::Seek);
                        this.set_seek_hot(true);
                        this.drag_to(ev.position.x.as_f32(), cx);
                    }
                }),
            )
            .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(track_h))
                    .rounded(px(track_h / 2.))
                    // 3:1 against the dark gradient (WCAG 1.4.11 non-text contrast).
                    .bg(Tokens::track())
                    // Buffered ranges.
                    .children(self.ranges.iter().filter(|_| dur > 0.0).map(|&(s, e)| {
                        div().absolute().top_0().h_full().left(relative(frac(s))).w(relative((frac(e) - frac(s)).max(0.0))).rounded(px(track_h / 2.)).bg(light(0.3))
                    }))
                    // Hover preview fill.
                    .children(hover.map(|f| div().absolute().top_0().left_0().h_full().w(relative(f as f32)).rounded(px(track_h / 2.)).bg(light(0.22))))
                    // Intro / recap / credits markers.
                    .when(dur > 0.0, |d| {
                        d.children(self.skip.iter().map(|s| {
                            let color = match s.kind {
                                SkipType::Ed | SkipType::MixedEd => Tokens::hue(Hue::Amber).alpha(0.5),
                                _ => Tokens::hue(Hue::Cyan).alpha(0.45),
                            };
                            div().absolute().top_0().h_full().left(relative(frac(s.start))).w(relative(frac(s.end - s.start))).bg(color)
                        }))
                    })
                    .child(div().absolute().top_0().left_0().h_full().w(relative(frac(pos))).rounded(px(track_h / 2.)).bg(Tokens::accent()))
                    .child(
                        div()
                            .absolute()
                            .top(px((track_h - thumb) / 2.))
                            .left(relative(frac(pos)))
                            .ml(px(-thumb / 2.))
                            .size(px(thumb))
                            .rounded_full()
                            .bg(Tokens::accent())
                            .opacity(0.35 + 0.65 * g),
                    ),
            )
            .children(hover.map(|f| {
                div()
                    .absolute()
                    .bottom(px(30.))
                    .left(relative(f as f32))
                    .ml(px(-38.))
                    .w(px(76.))
                    .py(px(5.))
                    .rounded(px(radius::MD))
                    .bg(Tokens::glass_strong())
                    .border_1()
                    .border_color(Tokens::glass_border())
                    .text_center()
                    .ramp(ramp::BODY_STRONG)
                    .text_color(Tokens::text_1())
                    .child(logic::format_time(f * dur))
            }))
    }

    fn volume_slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let v = if self.snap.muted { 0.0 } else { self.snap.volume };
        let frac = (v / 130.0).clamp(0.0, 1.0) as f32;
        let bounds = self.vol_bounds.clone();
        div()
            .id("vol")
            .w(px(92.))
            .h(px(28.))
            .flex()
            .items_center()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                    cx.stop_propagation();
                    this.drag = Some(Drag::Volume);
                    this.drag_to(ev.position.x.as_f32(), cx);
                }),
            )
            .tooltip(mv_ui::tip(format!("Volume {v:.0}%")))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(Tokens::track())
                    .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(relative(frac))
                            .rounded(px(2.))
                            // Over 100% is amplified and may distort.
                            .bg(if self.snap.volume > 100.0 { Tokens::warning() } else { Tokens::text_1() }),
                    )
                    .child(div().absolute().top(px(-4.)).left(relative(frac)).ml(px(-6.)).size(px(12.)).rounded_full().bg(Tokens::text_1())),
            )
    }

    fn menu_view(&self, menu: Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let (w, h) = self.menu_size(menu);
        let list = |id: &'static str| div().id(id).flex_1().min_h_0().overflow_y_scroll().px(px(8.)).pb(px(8.)).flex().flex_col();
        let empty = |text: &'static str| div().px(px(10.)).py(px(8.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(text);
        let body = match menu {
            Menu::Tracks => {
                let mut audio = list("audio-list");
                let tracks: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Audio).collect();
                if tracks.is_empty() {
                    audio = audio.child(empty("No audio tracks yet"));
                }
                for t in tracks {
                    let id = t.id;
                    audio = audio.child(
                        track_row(SharedString::from(format!("a{id}")), Some(ui::flag(logic::flag_country(&t.lang, &t.title))), logic::track_label(t), t.selected)
                            .on_click(cx.listener(move |this, _, _w, cx| this.select_track(TrackKind::Audio, Some(id), cx))),
                    );
                }
                let subs: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Sub).collect();
                let off = !subs.iter().any(|t| t.selected);
                let mut sub_list = list("sub-list")
                    .child(track_row("s-off", Some(ui::flag_glyph(ph::SUBTITLES_SLASH)), "Off", off).on_click(cx.listener(|this, _, _w, cx| this.select_track(TrackKind::Sub, None, cx))));
                for t in subs {
                    let id = t.id;
                    sub_list = sub_list.child(
                        track_row(SharedString::from(format!("s{id}")), Some(ui::flag(logic::flag_country(&t.lang, &t.title))), logic::track_label(t), t.selected)
                            .on_click(cx.listener(move |this, _, _w, cx| this.select_track(TrackKind::Sub, Some(id), cx))),
                    );
                }
                let col = || div().flex_1().min_w_0().h_full().flex().flex_col();
                div()
                    .size_full()
                    .flex()
                    .child(col().child(ui::menu_head("Audio", ph::WAVEFORM)).child(audio))
                    .child(div().w(px(1.)).my(px(14.)).bg(Tokens::glass_border()))
                    .child(col().child(ui::menu_head("Subtitles", glyphs::SUBTITLES)).child(sub_list))
            }
            Menu::Speed => {
                let mut speeds = div().id("speed-list").flex_1().min_h_0().overflow_y_scroll().px(px(8.)).pb(px(8.)).flex().flex_col();
                for s in logic::SPEEDS {
                    let selected = (self.snap.speed - s).abs() < 0.01;
                    let label = if s == 1.0 { "Normal".to_string() } else { logic::format_rate(s) };
                    speeds = speeds.child(track_row(SharedString::from(format!("sp{s}")), None, label, selected).on_click(cx.listener(move |this, _, _w, cx| this.set_speed(s, cx))));
                }
                div().size_full().flex().flex_col().child(ui::menu_head("Playback speed", ph::GAUGE)).child(speeds)
            }
        };
        let panel = glass_panel()
            .id("menu")
            .absolute()
            .right(px(MENU_RIGHT))
            .bottom(px(BOTTOM_H - 18.))
            .w(px(w))
            .h(px(h))
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(body);
        if motion::reduced() {
            panel.into_any_element()
        } else {
            panel
                .with_animation(SharedString::from(format!("menu-{}", self.menu_serial)), Animation::new(Duration::from_millis(200)).with_easing(ease_out_quint()), |el, d| el.opacity(d).mb(px((1.0 - d) * 10.0)))
                .into_any_element()
        }
    }

    /// Netflix-style outlined "Skip intro" pill; fills white on hover.
    fn skip_button(&self, seg: SkipSegment, o: f32, cx: &mut Context<Self>) -> impl IntoElement {
        use mv_ui::Interactive as _;
        let bottom = logic::skip_bottom(false) + (logic::skip_bottom(true) - logic::skip_bottom(false)) * o;
        let el = div()
            .id("skip")
            .absolute()
            .right(px(40.))
            .bottom(px(bottom))
            .h(px(46.))
            .px(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .rounded(px(radius::MD))
            .border_1()
            .border_color(light(0.65))
            .bg(Tokens::glass_strong())
            .ramp(ramp::BODY_STRONG)
            .text_color(Tokens::text_1())
            .role(Role::Button)
            .cursor_pointer()
            .group("skip")
            .hover(|s| s.bg(Tokens::text_1()).text_color(Tokens::base()))
            .active(|s| s.bg(light(0.8)).text_color(Tokens::base()))
            .focus_ring()
            .tooltip(mv_ui::tip_kbd(logic::skip_label(seg.kind), "s"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(logic::skip_label(seg.kind))
            .child(icon(ph::SKIP_FORWARD.fill(), Tokens::text_1(), icon_px::LG).group_hover("skip", |s| s.text_color(Tokens::base())))
            .on_click(cx.listener(|this, _, _w, cx| this.skip_now(cx)));
        if motion::reduced() {
            el.into_any_element()
        } else {
            el.with_animation(
                SharedString::from(format!("skip-{}", seg.start as i64)),
                Animation::new(Duration::from_millis(260)).with_easing(ease_out_quint()),
                |el, d| el.opacity(d).mr(px((1.0 - d) * -24.0)),
            )
            .into_any_element()
        }
    }

    /// Up Next: the next episode's still and name, a countdown ring when auto-play is on, a filled
    /// Play now and Cancel. Google Cast autoplay checklist:
    /// <https://developers.google.com/cast/docs/design_checklist/cast-autoplay>
    fn up_next_card(&self, o: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let bottom = logic::skip_bottom(false) + (logic::skip_bottom(true) - logic::skip_bottom(false)) * o;
        let next_ep = self.req.episode.map(|e| e + 1).unwrap_or(1);
        let ep_label = match self.req.season {
            Some(s) => format!("S{s} · E{next_ep}"),
            None => format!("Episode {next_ep}"),
        };
        let (title, still, loading) = match &self.next_info {
            Fetch::Done(Some(i)) => (i.title.clone(), i.still.clone().or_else(|| self.artwork.clone()), false),
            Fetch::Done(None) | Fetch::Idle => (None, self.artwork.clone(), false),
            Fetch::Loading => (None, None, true),
        };
        let countdown = self.prefs.auto_play && !self.next_busy();
        let total = self.prefs.countdown.max(1) as f64;
        let left = self.up_next.left.max(0.0);
        let thumb_w = 144.0;
        let thumb_h = 81.0;
        let thumb = div()
            .relative()
            .flex_none()
            .w(px(thumb_w))
            .h(px(thumb_h))
            .rounded(px(radius::MD))
            .overflow_hidden()
            .bg(Tokens::surface_2())
            .map(|d| match (loading, still) {
                (true, _) => d.child(skeleton(px(thumb_w), px(thumb_h), radius::MD)),
                (false, Some(url)) => d.child(
                    img(url)
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .with_loading(move || skeleton(px(thumb_w), px(thumb_h), radius::MD).into_any_element())
                        .with_fallback(|| div().size_full().flex().items_center().justify_center().child(icon(ph::FILM_STRIP.duotone(), Tokens::text_3(), icon_px::HERO)).into_any_element()),
                ),
                (false, None) => d.child(div().size_full().flex().items_center().justify_center().child(icon(ph::FILM_STRIP.duotone(), Tokens::text_3(), icon_px::HERO))),
            })
            // Play glyph over the still, like the next-episode tiles on Netflix.
            .child(div().absolute().top_0().left_0().size_full().flex().items_center().justify_center().child(
                div().size(px(36.)).rounded_full().bg(Tokens::glass()).flex().items_center().justify_center().child(icon(ph::PLAY.fill(), Tokens::text_1(), icon_px::LG)),
            ));
        let info = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .child(div().ramp(ramp::CAPTION).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(format!("UP NEXT · {ep_label}")))
            .map(|d| match (&title, loading) {
                (Some(t), _) => d.child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).line_clamp(2).text_ellipsis().child(t.clone())),
                (None, true) => d.child(div().mt(px(4.)).child(mv_ui::skeleton_line(relative(0.9)))).child(div().mt(px(6.)).child(mv_ui::skeleton_line(relative(0.6)))),
                (None, false) => d.child(div().ramp(ramp::BODY_STRONG).text_color(Tokens::text_1()).child(logic::next_title(&self.req.title, next_ep - 1))),
            });
        let ring = countdown.then(|| {
            div()
                .id("up-next-ring")
                .relative()
                .flex_none()
                .size(px(32.))
                .tooltip(mv_ui::tip(format!("Plays in {} s", left.ceil() as u64)))
                .child(progress_ring_sized((left / total) as f32, 32.))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(size::XS))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(Tokens::text_1())
                        .child(format!("{}", left.ceil() as u64)),
                )
        });
        let card = glass_panel()
            .id("up-next")
            .absolute()
            .right(px(40.))
            .bottom(px(bottom))
            .w(px(UP_NEXT_W))
            .p(px(12.))
            .gap(px(12.))
            .role(Role::Dialog)
            .aria_label(SharedString::from(format!("Up next: {ep_label}")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().flex().gap(px(12.)).child(thumb).child(info))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        btn("up-next-play", "Play now", ButtonKind::Primary)
                            .icon(ph::PLAY.fill())
                            .kbd("n")
                            .loading(self.next_busy())
                            .on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.play_next(window, cx)))),
                    )
                    .child(btn("up-next-cancel", "Cancel", ButtonKind::Ghost).on_click(cx.listener(|this, _, _w, cx| {
                        this.up_next.dismissed = true;
                        this.next_wanted = false;
                        cx.notify();
                    })))
                    .child(div().flex_1())
                    .children(ring),
            );
        if motion::reduced() {
            card.into_any_element()
        } else {
            card.with_animation("up-next-in", Animation::new(duration::SLOW).with_easing(easing::enter), |el, d| el.opacity(d).mr(px((1.0 - d) * -24.0))).into_any_element()
        }
    }

    /// `?`: every shortcut on one sheet with key caps (YouTube's Shift+? list).
    fn help_sheet(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let group = |title: &'static str, rows: Vec<Div>| div().flex().flex_col().gap(px(2.)).child(div().mb(px(4.)).ramp(ramp::CAPTION).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(title)).children(rows);
        let left = div()
            .flex()
            .flex_col()
            .flex_1()
            .gap(px(16.))
            .child(group(
                "PLAYBACK",
                vec![
                    ui::shortcut_row("Play or pause", &["space", "k"]),
                    ui::shortcut_row("Back 10 seconds", &["left", "j"]),
                    ui::shortcut_row("Forward 10 seconds", &["right", "l"]),
                    ui::shortcut_range("Jump to 0% … 90%", "0", "9"),
                    ui::shortcut_row("Start / end", &["home", "end"]),
                    ui::shortcut_row("Frame back / forward", &[",", "."]),
                    ui::shortcut_row("Slower / faster", &["<", ">"]),
                ],
            ));
        let right = div()
            .flex()
            .flex_col()
            .flex_1()
            .gap(px(16.))
            .child(group("SOUND", vec![ui::shortcut_row("Volume up / down", &["up", "down"]), ui::shortcut_row("Mute", &["m"])]))
            .child(group(
                "VIEW",
                vec![
                    ui::shortcut_row("Full screen", &["f"]),
                    ui::shortcut_row("Subtitles on / off", &["c"]),
                    ui::shortcut_row("Skip intro, recap or outro", &["s"]),
                    ui::shortcut_row("Next episode", &["n"]),
                    ui::shortcut_row("Close menu / back", &["escape"]),
                    ui::shortcut_row("This list", &["?"]),
                ],
            ));
        div()
            .id("help-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(Tokens::overlay())
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _w, cx| {
                    cx.stop_propagation();
                    this.help = false;
                    cx.notify();
                }),
            )
            .child(appear(
                "help-in",
                glass_panel()
                    .id("help")
                    .w(px(640.))
                    .p(px(24.))
                    .gap(px(18.))
                    .role(Role::Dialog)
                    .aria_label("Keyboard shortcuts")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(icon(ph::KEYBOARD.regular(), Tokens::text_2(), icon_px::LG))
                            .child(div().flex_1().ramp(ramp::SUBTITLE).text_color(Tokens::text_1()).child("Keyboard shortcuts"))
                            .child(icon_button("help-close", glyphs::CLOSE, "Close").kbd("escape").on_click(cx.listener(|this, _, _w, cx| {
                                this.help = false;
                                cx.notify();
                            }))),
                    )
                    .child(div().flex().gap(px(32.)).child(left).child(right)),
            ))
    }
}

/// To `~/.mangavox/logs/main.log` (the raw error the UI words for people).
fn log_error(what: &str, e: &dyn std::fmt::Display) {
    mv_core::logging::global().warn("player", &format!("{what}: {e:#}"));
}

/// Speaker glyph for the volume level.
fn volume_glyph(muted: bool, volume: f64) -> Ph {
    if muted || volume <= 0.0 {
        ph::SPEAKER_X
    } else if volume < 50.0 {
        ph::SPEAKER_LOW
    } else {
        ph::SPEAKER_HIGH
    }
}

/// Peers / speed / progress chips.
fn stat_chips(s: TorrentStats) -> impl IntoElement {
    div()
        .flex()
        .gap(px(8.))
        .mt(px(6.))
        .child(ui::stat_chip(ph::USERS, format!("{} peers", s.peers)))
        .child(ui::stat_chip(ph::ARROW_DOWN, logic::format_speed(s.speed)))
        .when(s.progress > 0.0, |d| d.child(ui::stat_chip(glyphs::DOWNLOAD, format!("{:.0}%", (s.progress * 100.0).min(100.0)))))
}

impl Drop for PlayerView {
    fn drop(&mut self) {
        // Final progress save, then let mpv + the popup drop with the struct.
        if let Some(m) = &self.mpv {
            let pos = m.get_f64("time-pos").unwrap_or(self.snap.pos);
            let dur = m.get_f64("duration").unwrap_or(self.snap.dur);
            if dur > 0.0 {
                let kind = if logic::is_complete(pos, dur) { "complete" } else { "progress" };
                let ident = self.ident.clone();
                bg::runtime().spawn_blocking(move || {
                    let _ = backend::save_event(&ident, kind, pos.min(dur), dur);
                });
            }
        }
        if let Some(h) = self.info_hash.take() {
            backend::release_torrent(h);
        }
    }
}

/// The app window's player page: black, owns keyboard focus. Until the overlay window exists
/// (or if it cannot be created) the overlay content is drawn here instead.
impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fallback = self.overlay.is_none().then(|| self.render_overlay(window, cx, false));
        div()
            .id("player-root")
            .track_focus(&self.focus)
            .key_context("Player")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(video_bg())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            // With the overlay up, Tab stays here (see `bind_keys`). The fallback controls are
            // drawn in this window and visible, so there Tab moves between them as usual.
            .on_action(cx.listener(|this, _: &StayOnPlayer, window, cx| {
                if this.overlay.is_some() {
                    this.activity(cx);
                    this.keep_focus(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            // Any click here (fallback controls) keeps the shortcuts on the player root.
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| this.keep_focus(window, cx)))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.exit(window, cx);
                }),
            )
            .children(fallback)
    }
}

/// Root view of the transparent overlay window: renders the player's controls and its toasts.
struct OverlayRoot {
    player: WeakEntity<PlayerView>,
    focus: FocusHandle,
    _observe: Option<Subscription>,
}

impl OverlayRoot {
    fn new(player: WeakEntity<PlayerView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let observe = player.upgrade().map(|p| cx.observe(&p, |_, _, cx| cx.notify()));
        Self { player, focus, _observe: observe }
    }
}

impl Render for OverlayRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inner = self.player.upgrade().map(|p| p.update(cx, |p, cx| p.render_overlay(window, cx, true)));
        div()
            .id("overlay-root")
            .size_full()
            .track_focus(&self.focus)
            // Every click in the overlay hands keyboard focus back to the player in the app window.
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _w, cx| {
                if let Some(p) = this.player.upgrade() {
                    p.update(cx, |p, cx| p.with_main(cx, |p, window, cx| p.keep_focus(window, cx)));
                }
            }))
            // Keys only land here if the overlay ever gets focus; forward them to the player.
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                if let Some(p) = this.player.upgrade() {
                    let ev = ev.clone();
                    p.update(cx, |p, cx| p.with_main(cx, move |p, window, cx| p.on_key(&ev, window, cx)));
                }
            }))
            .children(inner)
    }
}
