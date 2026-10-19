//! The player page. The video (mpv popup) fills the whole window; every control is drawn by GPUI
//! in a transparent overlay window stacked above it (see `overlay.rs`), Netflix / Stremio style:
//! soft gradients over the picture, auto-hiding with the cursor, animated scrubber and menus.
//!
//! The app window keeps keyboard focus (keys are handled here); the overlay handles the mouse
//! and forwards window-level actions (fullscreen, back, next) to the app window.
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_media::types::{SkipSegment, SkipType, StremioSubtitle};
use mv_ui::nav::{navigate, PlayRequest, Route};
use mv_ui::theme::{size, Tokens};
use mv_ui::{bg, button, icon, ButtonKind};

use crate::backend::{self, Ident, Resolved};
use crate::logic::{self, BOTTOM_H, MENU_HEAD_H, MENU_RIGHT, MENU_ROW_H, TOP_H};
use crate::mpv::{Event, Mpv, Track, TrackKind};
use crate::overlay::{self, Follower};

const TICK: Duration = Duration::from_millis(33);
const POLL_EVERY: u32 = 4; // ticks (~130 ms)
const TRACKS_EVERY: u32 = 8; // polls (~1 s)
const SAVE_EVERY: Duration = Duration::from_secs(10);
/// Controls fade in / out.
const FADE: Duration = Duration::from_millis(240);
/// Scrubber grows on hover.
const GROW: Duration = Duration::from_millis(150);
/// Centre play / pause / volume indicator.
const FLASH: Duration = Duration::from_millis(700);
/// Live seeks while scrubbing are at most this frequent.
const SCRUB_SEEK_EVERY: Duration = Duration::from_millis(90);
const TRACKS_W: f32 = 580.0;
const SPEED_W: f32 = 240.0;

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// Waiting for a torrent / source to be ready.
    Connecting,
    /// mpv created, file requested.
    Loading,
    Playing,
    Error(String),
}

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
    torrent: Option<TorrentStats>,
    snap: Snap,
    /// Buffered (instantly seekable) ranges.
    ranges: Vec<(f64, f64)>,
    tracks: Vec<Track>,
    skip: Vec<SkipSegment>,
    skip_requested: bool,
    subs_requested: bool,
    menu: Option<Menu>,
    /// Bumped on every menu open so its entrance animation replays.
    menu_serial: u32,
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
    last_save: Instant,
    played_logged: bool,
    complete_logged: bool,
    toast: Option<(String, Instant)>,
    flash: Option<(&'static str, String, Instant)>,
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
    next_busy: bool,
    /// Set once Back was requested, so repeated clicks / keys don't pop twice.
    leaving: bool,
}

/// Opens the player for `Route::Player`.
pub fn open_view(req: PlayRequest, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| PlayerView::new(req, window, cx)).into()
}

impl PlayerView {
    fn new(req: PlayRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let ident = backend::ident(&req);
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
            torrent: None,
            snap: Snap { volume: 100.0, speed: 1.0, ..Default::default() },
            ranges: vec![],
            tracks: vec![],
            skip: vec![],
            skip_requested: false,
            subs_requested: false,
            menu: None,
            menu_serial: 0,
            drag: None,
            scrub: None,
            scrub_sent: None,
            pending_seek: None,
            hover_x: None,
            seek_hot: false,
            seek_hot_at: now,
            ticks: 0,
            polls: 0,
            last_save: now,
            played_logged: false,
            complete_logged: false,
            toast: None,
            flash: None,
            last_input: now,
            controls: true,
            controls_at: now - FADE,
            cursor_hidden: false,
            last_cursor: None,
            mouse_in_bar: false,
            title_drag: None,
            video_shown: false,
            next_busy: false,
            leaving: false,
        };
        this.begin(window, cx);
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

    /// Resolves the source (torrent metadata etc.) then starts mpv.
    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.phase = Phase::Connecting;
        self.started = Instant::now();
        self.set_video_shown(false);
        let req = self.req.clone();
        let ident = self.ident.clone();
        cx.spawn_in(window, async move |this, cx| {
            let resume = bg::blocking(move || backend::load_resume(&ident)).await.ok().flatten();
            let resolved = bg::run(async move { backend::resolve(&req).await }).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| this.on_resolved(resolved, resume, window, cx));
        })
        .detach();
    }

    fn on_resolved(&mut self, resolved: anyhow::Result<Resolved>, resume: Option<f64>, _window: &mut Window, cx: &mut Context<Self>) {
        if self.leaving {
            return;
        }
        let r = match resolved {
            Ok(r) => r,
            Err(e) => {
                self.phase = Phase::Error(format!("{e:#}"));
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
                if let Some(t) = resume {
                    let _ = m.set("start", &format!("{t:.1}"));
                    self.toast(format!("Resuming at {}", logic::format_time(t)));
                }
                if let Err(e) = m.load_str(&r.target) {
                    self.phase = Phase::Error(format!("{e:#}"));
                } else {
                    self.phase = Phase::Loading;
                }
                if let Some(f) = &self.follower {
                    f.set_video(m.surface());
                }
                self.mpv = Some(m);
            }
            Err(e) => self.phase = Phase::Error(format!("{e:#}")),
        }
        if matches!(self.phase, Phase::Error(_)) {
            self.mpv = None;
        }
        self.sync_video_popup();
        cx.notify();
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

    /// Short message in the top bar (also reveals the controls).
    fn toast(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
        self.last_input = Instant::now();
        self.set_controls(true);
    }

    /// Big centre indicator (play / pause / volume / seek).
    fn flash(&mut self, icon: &'static str, label: impl Into<String>) {
        self.flash = Some((icon, label.into(), Instant::now()));
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
        if self.ticks % POLL_EVERY != 0 {
            return;
        }
        self.polls = self.polls.wrapping_add(1);
        let mut changed = false;

        // Torrent stats (connecting screen and buffering).
        if self.polls % 4 == 0 {
            if let (Some(h), Some(engine)) = (&self.info_hash, backend::engine_if_started()) {
                let stats = engine.stats(h).ok().map(|s| TorrentStats { peers: s.peers, speed: s.download_speed, progress: s.progress });
                if stats != self.torrent {
                    self.torrent = stats;
                    changed = true;
                }
            }
        }
        self.update_overlay_state(cx, &mut changed);

        let Some(mpv) = &self.mpv else {
            if self.phase == Phase::Connecting && self.polls % 2 == 0 {
                changed = true; // elapsed timer
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
        };
        if self.polls % TRACKS_EVERY == 0 || loaded {
            let t = mpv.tracks();
            if t != self.tracks {
                self.tracks = t;
                changed = true;
            }
        }
        if self.polls % 2 == 0 || loaded {
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
            self.phase = Phase::Error(msg);
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
        if let Some((_, at)) = &self.toast {
            if at.elapsed() > Duration::from_secs(3) {
                self.toast = None;
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
        self.phase != Phase::Playing || self.snap.paused || self.menu.is_some() || self.drag.is_some() || self.mouse_in_bar
    }

    /// Loading / buffering indicator over the video.
    fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Error(_)) && (self.mpv.is_none() || self.phase == Phase::Loading || self.snap.buffering)
    }

    fn skip_segment(&self) -> Option<SkipSegment> {
        (self.phase == Phase::Playing).then(|| logic::active_segment(&self.skip, self.snap.pos)).flatten()
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
            self.flash(if paused { "icons/pause.svg" } else { "icons/play.svg" }, "");
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
        self.flash(if delta < 0.0 { "icons/rotate-ccw.svg" } else { "icons/rotate-cw.svg" }, label);
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
        self.flash(volume_icon(self.snap.muted, v), format!("{v:.0}%"));
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            self.snap.muted = !self.snap.muted;
            let _ = m.set("mute", if self.snap.muted { "yes" } else { "no" });
            self.volume_flash();
            cx.notify();
        }
    }

    fn set_speed(&mut self, s: f64, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            let _ = m.set("speed", &format!("{s}"));
            self.snap.speed = s;
            self.menu = None;
            cx.notify();
        }
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
            self.tracks = m.tracks();
            cx.notify();
        }
    }

    fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            self.menu_serial = self.menu_serial.wrapping_add(1);
            Some(menu)
        };
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
        window.defer(cx, |window, cx| navigate(Route::Back, window, cx));
    }

    /// Looks up the next episode's stream, then replaces this player with it.
    fn play_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.next_busy || !backend::has_next(&self.req) {
            return;
        }
        self.next_busy = true;
        self.toast("Finding the next episode…".into());
        cx.notify();
        let req = self.req.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = bg::run(backend::next_episode(req)).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| {
                this.next_busy = false;
                match res {
                    Ok(next) => {
                        // Same torrent (season pack): keep it alive for the next player.
                        if this.info_hash.as_ref().is_some_and(|h| next.source.to_ascii_lowercase().contains(&h.to_ascii_lowercase())) {
                            this.info_hash = None;
                        }
                        this.leaving = true;
                        this.leave();
                        // Back first so history stays `detail → player`.
                        window.defer(cx, move |window, cx| {
                            navigate(Route::Back, window, cx);
                            navigate(Route::Player { request: next }, window, cx);
                        });
                    }
                    Err(e) => this.toast(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mpv = None;
        self.begin(window, cx);
        cx.notify();
    }

    /// Keyboard (from the app window, which keeps focus; the overlay forwards here too).
    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return;
        }
        self.activity(cx);
        match ev.keystroke.key.as_str() {
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
            "escape" => {
                if self.menu.is_some() {
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
        if dy.abs() < 0.5 || self.mpv.is_none() || self.menu.is_some() {
            return;
        }
        self.activity(cx);
        self.set_volume(self.snap.volume + if dy > 0.0 { 5.0 } else { -5.0 }, cx);
        self.volume_flash();
    }

    // ── rendering ──

    fn menu_size(&self, menu: Menu) -> (f32, f32) {
        match menu {
            Menu::Tracks => {
                let count = |k| self.tracks.iter().filter(|t| t.kind == k).count();
                (TRACKS_W, logic::menu_height(count(TrackKind::Audio).max(count(TrackKind::Sub) + 1).max(2)))
            }
            Menu::Speed => (SPEED_W, logic::menu_height(logic::SPEEDS.len())),
        }
    }

    /// The overlay window's content.
    fn render_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let fade_t = self.controls_at.elapsed();
        let mut o = logic::fade(self.controls, fade_t, FADE);
        let error = matches!(self.phase, Phase::Error(_));
        if error {
            o = 0.0;
        }
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
        let skip = self.skip_segment();
        let ov_bounds = self.ov_bounds.clone();
        let loading_screen = !error && !self.video_shown;

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
            .when(loading_screen, |d| d.child(self.loading_screen()))
            // Click target for the whole video: pause, double-click fullscreen, close menus.
            .child(div().id("stage").absolute().top_0().left_0().size_full().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                    if this.menu.take().is_some() {
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
            .when(!loading_screen && self.busy(), |d| d.child(self.buffering_badge()))
            .children(self.flash_view())
            .children(self.error_view(cx))
            .when(o > 0.01, |d| d.child(self.top_bar(o, cx)).child(self.bottom_bar(o, cx)))
            .children(menu.map(|m| self.menu_view(m, cx)))
            .children(skip.map(|s| self.skip_button(s, o, cx)))
            .into_any_element()
    }

    /// Full-screen loader before the first frame: dimmed backdrop, title, animated status.
    fn loading_screen(&self) -> impl IntoElement {
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
        let backdrop = self.req.imdb_id.as_deref().filter(|s| s.starts_with("tt")).map(|id| format!("https://images.metahub.space/background/medium/{id}/img"));
        let progress = if self.snap.cache_pct > 0.0 { Some((self.snap.cache_pct / 100.0) as f32) } else { None };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(rgb(0x000000))
            .children(backdrop.map(|url| {
                img(url).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover).with_animation(
                    "backdrop-in",
                    Animation::new(Duration::from_millis(900)).with_easing(ease_in_out),
                    |el, d| el.opacity(0.6 * d),
                )
            }))
            // Soft vignette so text stays readable over any backdrop.
            .child(div().absolute().top_0().left_0().size_full().bg(linear_gradient(
                180.,
                linear_color_stop(rgba(0x00000073), 0.),
                linear_color_stop(rgba(0x000000e0), 1.),
            )))
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
                    .gap(px(14.))
                    .child(ring_spinner(56.))
                    .child(
                        div()
                            .mt(px(10.))
                            .max_w(relative(0.7))
                            .truncate()
                            .text_size(px(30.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(Tokens::text_1())
                            .child(self.req.title.clone()),
                    )
                    .children(episode.map(|e| div().text_size(px(size::LG)).text_color(Tokens::text_2()).child(e)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .mt(px(4.))
                            .text_size(px(size::LG))
                            .text_color(Tokens::text_2())
                            .child(status)
                            .child(dots()),
                    )
                    .children(self.torrent.map(stat_chips))
                    .child(progress_line(progress, 280.))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("{}s", self.started.elapsed().as_secs()))),
            )
            .with_animation("loader-in", Animation::new(Duration::from_millis(350)).with_easing(ease_in_out), |el, d| el.opacity(d))
    }

    /// Compact buffering indicator over a playing video.
    fn buffering_badge(&self) -> impl IntoElement {
        let label = if self.snap.cache_pct > 0.0 { format!("Buffering {:.0}%", self.snap.cache_pct) } else { "Buffering".to_string() };
        div().absolute().top_0().left_0().size_full().flex().items_center().justify_center().child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.))
                .px(px(30.))
                .py(px(24.))
                .rounded(px(18.))
                .bg(rgba(0x0000008c))
                .child(ring_spinner(46.))
                .child(div().text_size(px(size::LG)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(label))
                .children(self.torrent.map(stat_chips))
                .with_animation("buffer-in", Animation::new(Duration::from_millis(250)).with_easing(ease_in_out), |el, d| el.opacity(d)),
        )
    }

    /// Fading centre indicator for play / pause / seek / volume.
    fn flash_view(&self) -> Option<impl IntoElement> {
        let (path, label, at) = self.flash.as_ref()?;
        let p = logic::ease_out(at.elapsed(), FLASH);
        let o = (1.0 - p).clamp(0.0, 1.0);
        let s = 78.0 + 18.0 * p;
        Some(
            div().absolute().top_0().left_0().size_full().flex().flex_col().items_center().justify_center().gap(px(10.)).opacity(o).child(
                div()
                    .size(px(s))
                    .rounded_full()
                    .bg(rgba(0x0000008c))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(path, Tokens::text_1(), s * 0.42)),
            )
            .when(!label.is_empty(), |d| {
                d.child(div().px(px(12.)).py(px(4.)).rounded(px(8.)).bg(rgba(0x0000008c)).text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(label.clone()))
            }),
        )
    }

    fn error_view(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let Phase::Error(msg) = &self.phase else { return None };
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
                .gap(px(12.))
                .bg(rgb(0x000000))
                .child(div().size(px(72.)).rounded_full().bg(rgba(0xf8717124)).flex().items_center().justify_center().child(icon("icons/triangle-alert.svg", Tokens::danger(), 34.)))
                .child(div().mt(px(6.)).text_size(px(size::XXL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("Couldn't play this"))
                .child(div().max_w(px(560.)).text_center().text_size(px(size::MD)).text_color(Tokens::text_3()).child(msg.clone()))
                .child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .mt(px(10.))
                        .child(button("retry", "Retry", ButtonKind::Primary).on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.retry(window, cx)))))
                        .child(button("err-back", "Back", ButtonKind::Secondary).on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.exit(window, cx))))),
                )
                .with_animation("error-in", Animation::new(Duration::from_millis(300)).with_easing(ease_in_out), |el, d| el.opacity(d)),
        )
    }

    fn top_bar(&self, o: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let episode = logic::episode_label(&self.req.title, self.req.season, self.req.episode);
        let windowed = !self.fullscreen;
        let maximized = self.follower.as_ref().is_some_and(|f| f.is_maximized());
        div()
            .id("top-bar")
            .absolute()
            .top(px(-(1.0 - o) * 14.0))
            .left_0()
            .w_full()
            .h(px(TOP_H + 30.))
            .opacity(o)
            .flex()
            .items_start()
            .pl(px(18.))
            .pt(px(14.))
            .gap(px(12.))
            .bg(linear_gradient(180., linear_color_stop(rgba(0x000000b3), 0.), linear_color_stop(rgba(0x00000000), 1.)))
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
            .child(round_ctl("back", "icons/arrow-left.svg", 24.).on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.exit(window, cx)))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(44.))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(div().truncate().text_size(px(19.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(self.req.title.clone()))
                    .children(episode.map(|e| div().text_size(px(size::MD)).text_color(Tokens::text_2()).child(e))),
            )
            .children(self.toast.as_ref().map(|(t, _)| {
                div()
                    .flex_none()
                    .mt(px(6.))
                    .max_w(px(420.))
                    .truncate()
                    .px(px(14.))
                    .py(px(7.))
                    .rounded_full()
                    .bg(rgba(0x00000099))
                    .border_1()
                    .border_color(rgba(0xffffff1f))
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_1())
                    .child(t.clone())
                    .with_animation("toast-in", Animation::new(Duration::from_millis(220)).with_easing(ease_in_out), |el, d| el.opacity(d))
            }))
            .when(windowed, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .mt(px(-14.))
                        .child(frame_btn("win-min", "icons/minus.svg", false).on_click(cx.listener(|this, _, _w, _cx| {
                            if let Some(f) = &this.follower {
                                f.minimize();
                            }
                        })))
                        .child(frame_btn("win-max", if maximized { "icons/copy.svg" } else { "icons/square.svg" }, false).on_click(cx.listener(|this, _, _w, _cx| {
                            if let Some(f) = &this.follower {
                                f.toggle_maximize();
                            }
                        })))
                        .child(frame_btn("win-close", "icons/x.svg", true).on_click(cx.listener(|this, _, _w, _cx| {
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
        let vol_icon = volume_icon(snap.muted, snap.volume);
        let time_text = if snap.dur > 0.0 {
            format!("{} / {}", logic::format_time(shown_pos), logic::format_time(snap.dur))
        } else {
            logic::format_time(shown_pos)
        };
        let left = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(ctl("play", if snap.paused { "icons/play.svg" } else { "icons/pause.svg" }, 30.).on_click(cx.listener(|this, _, _w, cx| this.toggle_pause(cx))))
            .child(jump_ctl("rew", "icons/rotate-ccw.svg").on_click(cx.listener(|this, _, _w, cx| this.seek_by(-10.0, cx))))
            .child(jump_ctl("fwd", "icons/rotate-cw.svg").on_click(cx.listener(|this, _, _w, cx| this.seek_by(10.0, cx))))
            .child(ctl("mute", vol_icon, 24.).on_click(cx.listener(|this, _, _w, cx| this.toggle_mute(cx))))
            .child(self.volume_slider(cx))
            .child(div().ml(px(14.)).text_size(px(size::MD)).text_color(Tokens::text_2()).child(time_text));
        let menu = self.menu;
        let right = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .when(backend::has_next(&self.req), |d| {
                d.child(
                    div()
                        .id("next")
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(40.))
                        .px(px(14.))
                        .mr(px(6.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(rgba(0xffffff14))
                        .hover(|s| s.bg(rgba(0xffffff29)))
                        .text_size(px(size::MD))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(Tokens::text_1())
                        .child(icon("icons/skip-forward.svg", Tokens::text_1(), 18.))
                        .child(if self.next_busy { "Finding…" } else { "Next episode" })
                        .on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.play_next(window, cx)))),
                )
            })
            .child(
                ctl("tracks", "icons/captions.svg", 24.)
                    .when(menu == Some(Menu::Tracks), |d| d.bg(rgba(0xffffff26)))
                    .on_click(cx.listener(|this, _, _w, cx| this.toggle_menu(Menu::Tracks, cx))),
            )
            .child(
                ctl("speed", "icons/gauge.svg", 24.)
                    .when(menu == Some(Menu::Speed), |d| d.bg(rgba(0xffffff26)))
                    .on_click(cx.listener(|this, _, _w, cx| this.toggle_menu(Menu::Speed, cx))),
            )
            .child(
                ctl("fs", if self.fullscreen { "icons/minimize.svg" } else { "icons/maximize.svg" }, 24.)
                    .on_click(cx.listener(|this, _, _w, cx| this.with_main(cx, |this, window, cx| this.toggle_fullscreen(window, cx)))),
            );
        div()
            .id("bottom-bar")
            .absolute()
            .bottom(px(-(1.0 - o) * 14.0))
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
            .bg(linear_gradient(0., linear_color_stop(rgba(0x000000d9), 0.), linear_color_stop(rgba(0x00000000), 1.)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(ready, |d| {
                d.child(div().flex().items_center().child(self.scrubber(cx)))
                .child(div().flex().items_center().justify_between().h(px(50.)).child(left).child(right))
            })
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
                    .bg(rgba(0xffffff2e))
                    // Buffered ranges.
                    .children(self.ranges.iter().filter(|_| dur > 0.0).map(|&(s, e)| {
                        div().absolute().top_0().h_full().left(relative(frac(s))).w(relative((frac(e) - frac(s)).max(0.0))).rounded(px(track_h / 2.)).bg(rgba(0xffffff4d))
                    }))
                    // Hover preview fill.
                    .children(hover.map(|f| div().absolute().top_0().left_0().h_full().w(relative(f as f32)).rounded(px(track_h / 2.)).bg(rgba(0xffffff26))))
                    .when(dur > 0.0, |d| {
                        d.children(self.skip.iter().map(|s| {
                            let color = match s.kind {
                                SkipType::Ed | SkipType::MixedEd => rgba(0xfbbf2466),
                                _ => rgba(0x2bd49a55),
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
                    .rounded(px(8.))
                    .bg(rgba(0x14161af0))
                    .border_1()
                    .border_color(rgba(0xffffff1a))
                    .text_center()
                    .text_size(px(size::MD))
                    .font_weight(FontWeight::SEMIBOLD)
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
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(rgba(0xffffff33))
                    .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(relative(frac))
                            .rounded(px(2.))
                            .bg(if self.snap.volume > 100.0 { Tokens::warning() } else { Tokens::text_1() }),
                    )
                    .child(div().absolute().top(px(-4.)).left(relative(frac)).ml(px(-6.)).size(px(12.)).rounded_full().bg(Tokens::text_1())),
            )
    }

    fn menu_view(&self, menu: Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let row = |id: String, flag_el: Option<AnyElement>, label: String, selected: bool| {
            div()
                .id(SharedString::from(id))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .h(px(MENU_ROW_H))
                .px(px(10.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(size::MD))
                .text_color(if selected { Tokens::text_1() } else { Tokens::text_2() })
                .when(selected, |d| d.bg(rgba(0xffffff0f)).font_weight(FontWeight::SEMIBOLD))
                .hover(|s| s.bg(rgba(0xffffff17)).text_color(Tokens::text_1()))
                .children(flag_el)
                .child(div().flex_1().min_w_0().truncate().child(label))
                .child(div().w(px(18.)).flex_none().when(selected, |d| d.child(icon("icons/check.svg", Tokens::accent(), 18.))))
        };
        let (w, h) = self.menu_size(menu);
        let head = |title: &'static str, path: &'static str| {
            div()
                .flex_none()
                .h(px(MENU_HEAD_H))
                .px(px(18.))
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(size::LG))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Tokens::text_1())
                .child(icon(path, Tokens::text_2(), 18.))
                .child(title)
        };
        let body = match menu {
            Menu::Tracks => {
                let mut audio = div().id("audio-list").flex_1().min_h_0().overflow_y_scroll().px(px(8.)).pb(px(8.)).flex().flex_col();
                let tracks: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Audio).collect();
                if tracks.is_empty() {
                    audio = audio.child(div().px(px(10.)).py(px(8.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child("No audio tracks"));
                }
                for t in tracks {
                    let id = t.id;
                    audio = audio.child(
                        row(format!("a{id}"), Some(flag(logic::flag_country(&t.lang, &t.title))), logic::track_label(t), t.selected)
                            .on_click(cx.listener(move |this, _, _w, cx| this.select_track(TrackKind::Audio, Some(id), cx))),
                    );
                }
                let subs: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Sub).collect();
                let off = !subs.iter().any(|t| t.selected);
                let mut list = div().id("sub-list").flex_1().min_h_0().overflow_y_scroll().px(px(8.)).pb(px(8.)).flex().flex_col();
                list = list.child(
                    row("s-off".into(), Some(flag_icon("icons/captions-off.svg")), "Off".into(), off).on_click(cx.listener(|this, _, _w, cx| this.select_track(TrackKind::Sub, None, cx))),
                );
                for t in subs {
                    let id = t.id;
                    list = list.child(
                        row(format!("s{id}"), Some(flag(logic::flag_country(&t.lang, &t.title))), logic::track_label(t), t.selected)
                            .on_click(cx.listener(move |this, _, _w, cx| this.select_track(TrackKind::Sub, Some(id), cx))),
                    );
                }
                let col = || div().flex_1().min_w_0().h_full().flex().flex_col();
                div()
                    .size_full()
                    .flex()
                    .child(col().child(head("Audio", "icons/audio-lines.svg")).child(audio))
                    .child(div().w(px(1.)).my(px(14.)).bg(rgba(0xffffff14)))
                    .child(col().child(head("Subtitles", "icons/captions.svg")).child(list))
            }
            Menu::Speed => {
                let mut list = div().id("speed-list").flex_1().min_h_0().px(px(8.)).pb(px(8.)).flex().flex_col();
                for s in logic::SPEEDS {
                    let selected = (self.snap.speed - s).abs() < 0.01;
                    let label = if s == 1.0 { "Normal".to_string() } else { format!("{s}x") };
                    list = list.child(row(format!("sp{s}"), None, label, selected).on_click(cx.listener(move |this, _, _w, cx| this.set_speed(s, cx))));
                }
                div().size_full().flex().flex_col().child(head("Playback speed", "icons/gauge.svg")).child(list)
            }
        };
        div()
            .id("menu")
            .absolute()
            .right(px(MENU_RIGHT))
            .bottom(px(BOTTOM_H - 18.))
            .w(px(w))
            .h(px(h))
            .rounded(px(14.))
            .overflow_hidden()
            .bg(rgba(0x101216f2))
            .border_1()
            .border_color(rgba(0xffffff17))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(body)
            .with_animation(
                SharedString::from(format!("menu-{}", self.menu_serial)),
                Animation::new(Duration::from_millis(200)).with_easing(ease_out_quint()),
                |el, d| el.opacity(d).mb(px((1.0 - d) * 10.0)),
            )
    }

    fn skip_button(&self, seg: SkipSegment, o: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let bottom = logic::skip_bottom(false) + (logic::skip_bottom(true) - logic::skip_bottom(false)) * o;
        div()
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
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(0xffffffa6))
            .bg(rgba(0x0e1014d9))
            .text_size(px(size::LG))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(Tokens::text_1())
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0xffffffeb)).text_color(Tokens::base()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(logic::skip_label(seg.kind))
            .child(icon("icons/skip-forward.svg", Tokens::text_1(), 18.))
            .on_click(cx.listener(|this, _, _w, cx| this.skip_now(cx)))
            .with_animation(
                SharedString::from(format!("skip-{}", seg.start as i64)),
                Animation::new(Duration::from_millis(260)).with_easing(ease_out_quint()),
                |el, d| el.opacity(d).mr(px((1.0 - d) * -24.0)),
            )
    }
}

fn volume_icon(muted: bool, volume: f64) -> &'static str {
    if muted || volume <= 0.0 {
        "icons/volume-x.svg"
    } else if volume < 50.0 {
        "icons/volume-1.svg"
    } else {
        "icons/volume-2.svg"
    }
}

/// Small flag image (flagcdn) for a country, or a neutral globe.
fn flag(country: Option<String>) -> AnyElement {
    match country {
        Some(cc) => img(logic::flag_url(&cc)).flex_none().w(px(22.)).h(px(16.)).rounded(px(3.)).object_fit(ObjectFit::Cover).into_any_element(),
        None => flag_icon("icons/globe.svg"),
    }
}

fn flag_icon(path: &'static str) -> AnyElement {
    div().flex_none().w(px(22.)).h(px(16.)).flex().items_center().justify_center().child(icon(path, Tokens::text_3(), 15.)).into_any_element()
}

/// White overlay control with a soft round hover.
fn ctl(id: impl Into<ElementId>, path: &'static str, size: f32) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size + 18.))
        .rounded_full()
        .cursor_pointer()
        .hover(|s| s.bg(rgba(0xffffff1f)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(icon(path, Tokens::text_1(), size))
}

/// Circular glass button (back arrow).
fn round_ctl(id: impl Into<ElementId>, path: &'static str, size: f32) -> Stateful<Div> {
    ctl(id, path, size).bg(rgba(0x00000059)).border_1().border_color(rgba(0xffffff1a))
}

/// Window frame button (minimize / maximize / close) for the overlay's own title area.
fn frame_btn(id: &'static str, path: &'static str, close: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(46.))
        .h(px(34.))
        .cursor_pointer()
        .hover(move |s| if close { s.bg(rgb(0xc42b1c)) } else { s.bg(rgba(0xffffff1f)) })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(icon(path, Tokens::text_1(), 15.))
}

/// Rewind / forward button with the "10" inside the circular arrow.
fn jump_ctl(id: &'static str, path: &'static str) -> Stateful<Div> {
    ctl(id, path, 28.).relative().child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .pt(px(1.))
            .text_size(px(9.))
            .font_weight(FontWeight::BOLD)
            .text_color(Tokens::text_1())
            .child("10"),
    )
}

/// Accent ring spinner.
fn ring_spinner(s: f32) -> impl IntoElement {
    div().size(px(s)).flex().items_center().justify_center().child(icon("icons/loader-circle.svg", Tokens::accent(), s).with_animation(
        "spin",
        Animation::new(Duration::from_millis(850)).repeat(),
        |el, delta| el.with_transformation(Transformation::rotate(percentage(delta))),
    ))
}

/// Three softly pulsing dots.
fn dots() -> impl IntoElement {
    div().flex().items_end().gap(px(4.)).ml(px(6.)).pb(px(3.)).children((0..3).map(|i| {
        div().size(px(5.)).rounded_full().bg(Tokens::text_2()).with_animation(
            SharedString::from(format!("dot-{i}")),
            Animation::new(Duration::from_millis(1200)).repeat(),
            move |el, delta| {
                let t = (delta - i as f32 * 0.18).rem_euclid(1.0);
                let a = (t * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                el.opacity(0.2 + 0.8 * a)
            },
        )
    }))
}

/// Peers / speed / progress chips.
fn stat_chips(s: TorrentStats) -> impl IntoElement {
    let chip = |path: &'static str, text: String| {
        div()
            .flex()
            .items_center()
            .gap(px(7.))
            .px(px(12.))
            .py(px(6.))
            .rounded_full()
            .bg(rgba(0xffffff12))
            .border_1()
            .border_color(rgba(0xffffff14))
            .text_size(px(size::SM))
            .text_color(Tokens::text_1())
            .child(icon(path, Tokens::accent(), 14.))
            .child(text)
    };
    div()
        .flex()
        .gap(px(8.))
        .mt(px(6.))
        .child(chip("icons/users.svg", format!("{} peers", s.peers)))
        .child(chip("icons/arrow-down.svg", logic::format_speed(s.speed)))
        .when(s.progress > 0.0, |d| d.child(chip("icons/download.svg", format!("{:.0}%", (s.progress * 100.0).min(100.0)))))
}

/// Thin progress line: determinate when `progress` is known, else a sliding shimmer.
fn progress_line(progress: Option<f32>, w: f32) -> impl IntoElement {
    let track = div().relative().mt(px(10.)).w(px(w)).h(px(3.)).rounded_full().overflow_hidden().bg(rgba(0xffffff1f));
    match progress {
        Some(p) => track.child(div().absolute().top_0().left_0().h_full().w(relative(p.clamp(0.0, 1.0))).rounded_full().bg(Tokens::accent())).into_any_element(),
        None => track
            .child(div().absolute().top_0().h_full().w(px(w * 0.3)).rounded_full().bg(Tokens::accent()).with_animation(
                "shimmer",
                Animation::new(Duration::from_millis(1400)).repeat().with_easing(ease_in_out),
                move |el, d| el.left(px(-w * 0.3 + d * w * 1.3)),
            ))
            .into_any_element(),
    }
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
        let fallback = self.overlay.is_none().then(|| self.render_overlay(window, cx));
        div()
            .id("player-root")
            .track_focus(&self.focus)
            .key_context("Player")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x000000))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
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

/// Root view of the transparent overlay window: renders the player's controls.
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
        let inner = self.player.upgrade().map(|p| p.update(cx, |p, cx| p.render_overlay(window, cx)));
        div()
            .id("overlay-root")
            .size_full()
            .track_focus(&self.focus)
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
