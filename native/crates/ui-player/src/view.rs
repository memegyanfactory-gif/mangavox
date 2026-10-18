//! The player page: full-bleed video (mpv popup) with Netflix-style auto-hiding overlay controls.
//! The popup sits above GPUI, so every visible control area is cut out of its window region.
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
use crate::logic::{self, BOTTOM_H, MENU_HEAD_H, MENU_RIGHT, MENU_ROW_H, SKIP_H, SKIP_RIGHT, SKIP_W, TOP_H};
use crate::mpv::{self, Event, Mpv, Track, TrackKind};

const TICK: Duration = Duration::from_millis(33);
const POLL_EVERY: u32 = 4; // ticks (~130 ms)
const TRACKS_EVERY: u32 = 8; // polls (~1 s)
const SAVE_EVERY: Duration = Duration::from_secs(10);
const TRACKS_W: f32 = 520.0;
const SPEED_W: f32 = 220.0;

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

#[derive(Clone, Debug, Default, PartialEq)]
struct Snap {
    pos: f64,
    dur: f64,
    paused: bool,
    volume: f64,
    muted: bool,
    speed: f64,
    buffering: bool,
    /// End of the demuxer cache (seconds).
    buffered: f64,
    /// mpv's `cache-buffering-state` (0..100) while stalled.
    cache_pct: f64,
}

pub struct PlayerView {
    req: PlayRequest,
    ident: Ident,
    focus: FocusHandle,
    /// Video area in device pixels, written by the stage canvas each layout.
    rect: Rc<Cell<(i32, i32, i32, i32)>>,
    /// Same area in window pixels.
    stage: Rc<Cell<Bounds<Pixels>>>,
    seek_bounds: Rc<Cell<Bounds<Pixels>>>,
    vol_bounds: Rc<Cell<Bounds<Pixels>>>,
    mpv: Option<Mpv>,
    phase: Phase,
    started: Instant,
    info_hash: Option<String>,
    torrent_status: Option<String>,
    snap: Snap,
    tracks: Vec<Track>,
    skip: Vec<SkipSegment>,
    skip_requested: bool,
    subs_requested: bool,
    menu: Option<Menu>,
    drag: Option<Drag>,
    /// Pending seek target while scrubbing.
    scrub: Option<f64>,
    /// Mouse x over the seek bar (hover tooltip).
    hover_x: Option<f32>,
    ticks: u32,
    polls: u32,
    last_save: Instant,
    played_logged: bool,
    complete_logged: bool,
    toast: Option<(String, Instant)>,
    // Overlay visibility.
    last_input: Instant,
    controls: bool,
    cursor_hidden: bool,
    last_cursor: Option<(i32, i32)>,
    mouse_in_bar: bool,
    /// Region holes currently applied to the popup.
    holes: Vec<[i32; 4]>,
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
        let mut this = Self {
            req,
            ident,
            focus,
            rect: Rc::new(Cell::new((0, 0, 0, 0))),
            stage: Rc::new(Cell::new(Bounds::default())),
            seek_bounds: Rc::new(Cell::new(Bounds::default())),
            vol_bounds: Rc::new(Cell::new(Bounds::default())),
            mpv: None,
            phase: Phase::Connecting,
            started: Instant::now(),
            info_hash: None,
            torrent_status: None,
            snap: Snap { volume: 100.0, speed: 1.0, ..Default::default() },
            tracks: vec![],
            skip: vec![],
            skip_requested: false,
            subs_requested: false,
            menu: None,
            drag: None,
            scrub: None,
            hover_x: None,
            ticks: 0,
            polls: 0,
            last_save: Instant::now(),
            played_logged: false,
            complete_logged: false,
            toast: None,
            last_input: Instant::now(),
            controls: true,
            cursor_hidden: false,
            last_cursor: None,
            mouse_in_bar: false,
            holes: vec![],
            next_busy: false,
            leaving: false,
        };
        this.begin(window, cx);
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if this.update_in(cx, |this, window, cx| this.tick(window, cx)).is_err() {
                break;
            }
        })
        .detach();
        this
    }

    /// Resolves the source (torrent metadata etc.) then starts mpv.
    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.phase = Phase::Connecting;
        self.started = Instant::now();
        self.holes.clear();
        let req = self.req.clone();
        let ident = self.ident.clone();
        cx.spawn_in(window, async move |this, cx| {
            let resume = bg::blocking(move || backend::load_resume(&ident)).await.ok().flatten();
            let resolved = bg::run(async move { backend::resolve(&req).await }).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| this.on_resolved(resolved, resume, window, cx));
        })
        .detach();
    }

    fn on_resolved(&mut self, resolved: anyhow::Result<Resolved>, resume: Option<f64>, window: &mut Window, cx: &mut Context<Self>) {
        let r = match resolved {
            Ok(r) => r,
            Err(e) => {
                self.phase = Phase::Error(format!("{e:#}"));
                cx.notify();
                return;
            }
        };
        self.info_hash = r.info_hash.clone();
        match Mpv::create(window, self.rect.get()) {
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
                self.mpv = Some(m);
            }
            Err(e) => self.phase = Phase::Error(format!("{e:#}")),
        }
        if matches!(self.phase, Phase::Error(_)) {
            self.mpv = None;
        }
        cx.notify();
    }

    /// Short message in the top bar (also reveals the controls).
    fn toast(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
        self.last_input = Instant::now();
        self.controls = true;
    }

    // ── per-frame work ──

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ticks = self.ticks.wrapping_add(1);
        let Some(mpv) = &self.mpv else {
            if self.phase == Phase::Connecting && self.ticks % 8 == 0 {
                cx.notify(); // elapsed timer
            }
            return;
        };
        mpv::move_surface(mpv.surface(), self.rect.get());
        self.update_holes(window.scale_factor());
        if self.ticks % POLL_EVERY != 0 {
            return;
        }
        let Some(mpv) = &self.mpv else { return };
        mpv.pass_mouse_through();
        self.polls = self.polls.wrapping_add(1);
        let mut changed = false;

        let (mut loaded, mut ended, mut failed) = (false, false, None);
        for ev in mpv.poll_events() {
            match ev {
                Event::FileLoaded => loaded = true,
                Event::Ended => ended = true,
                Event::Failed(m) => failed = Some(m),
            }
        }

        let paused_for_cache = mpv.get_bool("paused-for-cache").unwrap_or(false);
        let snap = Snap {
            pos: mpv.get_f64("time-pos").unwrap_or(self.snap.pos),
            dur: mpv.get_f64("duration").unwrap_or(self.snap.dur),
            paused: mpv.get_bool("pause").unwrap_or(false),
            volume: mpv.get_f64("volume").unwrap_or(self.snap.volume),
            muted: mpv.get_bool("mute").unwrap_or(false),
            speed: mpv.get_f64("speed").unwrap_or(1.0),
            buffering: paused_for_cache || (self.phase == Phase::Loading && !loaded),
            buffered: mpv.get_f64("demuxer-cache-time").unwrap_or(0.0),
            cache_pct: if paused_for_cache { mpv.get_f64("cache-buffering-state").unwrap_or(0.0) } else { 0.0 },
        };
        if self.polls % TRACKS_EVERY == 0 || loaded {
            let t = mpv.tracks();
            if t != self.tracks {
                self.tracks = t;
                changed = true;
            }
        }
        let cursor = mpv.cursor_over();
        if snap != self.snap {
            self.snap = snap;
            changed = true;
        }

        if let Some(msg) = failed {
            self.phase = Phase::Error(msg);
            if let Some(m) = &self.mpv {
                m.set_visible(false);
            }
            changed = true;
        } else if loaded || (self.phase == Phase::Loading && self.snap.dur > 0.0) {
            self.phase = Phase::Playing;
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

        // Torrent status while connecting to peers / buffering.
        if self.polls % 8 == 0 {
            if let (Some(h), Some(engine)) = (&self.info_hash, backend::engine_if_started()) {
                let text = engine.stats(h).ok().map(|s| logic::torrent_line(s.peers, s.download_speed, s.progress));
                if text != self.torrent_status {
                    self.torrent_status = text;
                    changed = true;
                }
            }
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

        // Cursor polling catches movement even when mouse input doesn't reach GPUI.
        if cursor.is_some() && cursor != self.last_cursor && self.last_cursor.is_some() {
            self.activity(cx);
        }
        self.last_cursor = cursor;
        let visible = logic::controls_visible(self.last_input.elapsed(), self.pinned());
        if visible != self.controls {
            self.controls = visible;
            self.hover_x = None;
            self.mouse_in_bar &= visible;
            changed = true;
        } else if !visible && !self.cursor_hidden {
            // One poll after hiding, so GPUI's own cursor update has settled.
            self.cursor_hidden = true;
            if let Some(m) = &self.mpv {
                m.hide_cursor();
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Keeps the popup's region in sync with what GPUI draws over the video.
    fn update_holes(&mut self, scale: f32) {
        let Some(m) = &self.mpv else { return };
        let (_, _, w, h) = self.rect.get();
        let holes: Vec<[i32; 4]> = logic::holes(w as f32 / scale, h as f32 / scale, &self.overlay_state())
            .into_iter()
            .map(|r| r.map(|v| (v * scale).round() as i32))
            .collect();
        if holes != self.holes {
            m.set_holes(&holes);
            self.holes = holes;
        }
    }

    fn overlay_state(&self) -> logic::Overlay {
        logic::Overlay {
            controls: self.controls,
            menu: self.menu.filter(|_| self.controls).map(|m| self.menu_size(m)),
            skip: self.skip_segment().is_some(),
            center: self.busy(),
        }
    }

    /// Controls stay up while paused, loading, scrubbing, in a menu or hovering a bar.
    fn pinned(&self) -> bool {
        self.phase != Phase::Playing || self.snap.paused || self.menu.is_some() || self.drag.is_some() || self.mouse_in_bar
    }

    /// Loading / buffering spinner over the video.
    fn busy(&self) -> bool {
        self.mpv.is_some() && !matches!(self.phase, Phase::Error(_)) && (self.phase == Phase::Loading || self.snap.buffering)
    }

    fn skip_segment(&self) -> Option<SkipSegment> {
        (self.phase == Phase::Playing).then(|| logic::active_segment(&self.skip, self.snap.pos)).flatten()
    }

    fn activity(&mut self, cx: &mut Context<Self>) {
        self.last_input = Instant::now();
        self.cursor_hidden = false;
        if !self.controls {
            self.controls = true;
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
            cx.notify();
        }
    }

    fn seek_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            let t = logic::seek_target(self.snap.pos, delta, self.snap.dur);
            self.snap.pos = t;
            m.seek_absolute(t);
            cx.notify();
        }
    }

    fn seek_to(&mut self, t: f64, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            self.snap.pos = t;
            m.seek_absolute(t);
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

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = &self.mpv {
            self.snap.muted = !self.snap.muted;
            let _ = m.set("mute", if self.snap.muted { "yes" } else { "no" });
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
        self.menu = if self.menu == Some(menu) { None } else { Some(menu) };
        cx.notify();
    }

    fn toggle_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.toggle_fullscreen();
        cx.notify();
    }

    fn skip_now(&mut self, cx: &mut Context<Self>) {
        if let Some(seg) = self.skip_segment() {
            self.seek_to(seg.end, cx);
        }
    }

    /// Saves progress and hides the video before the page goes away.
    fn leave(&mut self) {
        self.save("progress");
        if let Some(m) = &self.mpv {
            m.set_pause(true);
            m.set_visible(false);
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
            "up" => self.set_volume(self.snap.volume + 5.0, cx),
            "down" => self.set_volume(self.snap.volume - 5.0, cx),
            "f" => self.toggle_fullscreen(window, cx),
            "m" => self.toggle_mute(cx),
            "n" => self.play_next(window, cx),
            "s" => self.skip_now(cx),
            "escape" => {
                if self.menu.is_some() {
                    self.menu = None;
                    cx.notify();
                } else if window.is_fullscreen() {
                    window.toggle_fullscreen();
                    cx.notify();
                } else {
                    self.exit(window, cx);
                }
            }
            _ => {}
        }
    }

    // ── mouse ──

    fn on_mouse_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        self.activity(cx);
        let st = self.stage.get();
        let y = (pos.y - st.origin.y).as_f32();
        self.mouse_in_bar = self.controls && (y < TOP_H || y > st.size.height.as_f32() - BOTTOM_H);
        let hover = (self.controls && self.seek_bounds.get().contains(&pos)).then_some(pos.x.as_f32());
        if hover != self.hover_x {
            self.hover_x = hover;
            cx.notify();
        }
        self.drag_to(pos.x.as_f32(), cx);
    }

    fn drag_to(&mut self, x: f32, cx: &mut Context<Self>) {
        match self.drag {
            Some(Drag::Seek) => {
                let b = self.seek_bounds.get();
                let f = logic::fraction_in(x, b.origin.x.as_f32(), b.size.width.as_f32());
                self.scrub = Some(f * self.snap.dur);
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
        if self.drag == Some(Drag::Seek) {
            if let Some(t) = self.scrub.take() {
                self.seek_to(t, cx);
            }
        }
        self.drag = None;
    }

    // ── rendering ──

    fn menu_size(&self, menu: Menu) -> (f32, f32) {
        match menu {
            Menu::Tracks => {
                let count = |k| self.tracks.iter().filter(|t| t.kind == k).count();
                (TRACKS_W, logic::menu_height(count(TrackKind::Audio).max(count(TrackKind::Sub) + 1)))
            }
            Menu::Speed => (SPEED_W, logic::menu_height(logic::SPEEDS.len())),
        }
    }

    /// Spinner / status in the middle, or the error card.
    fn center(&self, cx: &mut Context<Self>) -> Option<Div> {
        let full = || div().absolute().top_0().left_0().size_full().flex().flex_col().items_center().justify_center();
        let line = |text: String, color: Rgba, size: f32| div().text_size(px(size)).text_color(color).text_center().child(text);
        if let Phase::Error(msg) = &self.phase {
            return Some(
                full()
                    .gap(px(12.))
                    .bg(rgb(0x000000))
                    .child(icon("icons/triangle-alert.svg", Tokens::danger(), 40.))
                    .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Couldn't play this"))
                    .child(div().max_w(px(520.)).text_center().text_size(px(size::SM)).text_color(Tokens::text_3()).child(msg.clone()))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .mt(px(6.))
                            .child(button("retry", "Retry", ButtonKind::Primary).on_click(cx.listener(|this, _, window, cx| {
                                this.mpv = None;
                                this.begin(window, cx);
                                cx.notify();
                            })))
                            .child(button("err-back", "Back", ButtonKind::Secondary).on_click(cx.listener(|this, _, window, cx| this.exit(window, cx)))),
                    ),
            );
        }
        let connecting = self.mpv.is_none();
        if !connecting && !self.busy() {
            return None;
        }
        let torrent = matches!(logic::classify_source(&self.req.source), logic::SourceKind::Torrent(_));
        let title = if connecting {
            if torrent { "Connecting to peers…" } else { "Loading…" }.to_string()
        } else if self.snap.cache_pct > 0.0 {
            format!("Buffering {:.0}%", self.snap.cache_pct)
        } else if self.phase == Phase::Loading {
            "Loading…".to_string()
        } else {
            "Buffering…".to_string()
        };
        let mut sub = self.torrent_status.clone().unwrap_or_default();
        if connecting {
            let secs = format!("{}s", self.started.elapsed().as_secs());
            sub = if sub.is_empty() { secs } else { format!("{sub} · {secs}") };
        }
        Some(
            full().child(
                div()
                    .w(px(logic::CENTER_W))
                    .h(px(logic::CENTER_H))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .child(spinner())
                    .child(line(title, Tokens::text_1(), size::LG))
                    .when(!sub.is_empty(), |d| d.child(line(sub, Tokens::text_3(), size::SM))),
            ),
        )
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let episode = logic::episode_label(&self.req.title, self.req.season, self.req.episode);
        div()
            .id("top-bar")
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(px(TOP_H))
            .px(px(20.))
            .flex()
            .items_center()
            .gap(px(14.))
            .bg(linear_gradient(180., linear_color_stop(rgba(0x000000e6), 0.), linear_color_stop(rgba(0x00000000), 1.)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(ctl("back", "icons/arrow-left.svg", 26.).on_click(cx.listener(|this, _, window, cx| this.exit(window, cx))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(div().truncate().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(self.req.title.clone()))
                    .children(episode.map(|e| div().flex_none().text_size(px(size::MD)).text_color(Tokens::text_2()).child(e))),
            )
            .children(self.toast.as_ref().map(|(t, _)| {
                div().flex_none().max_w(px(420.)).truncate().px(px(12.)).py(px(6.)).rounded(px(8.)).bg(rgba(0xffffff14)).text_size(px(size::SM)).text_color(Tokens::text_1()).child(t.clone())
            }))
    }

    fn bottom_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = &self.snap;
        let ready = self.mpv.is_some();
        let shown_pos = self.scrub.unwrap_or(snap.pos);
        let vol_icon = if snap.muted || snap.volume <= 0.0 {
            "icons/volume-x.svg"
        } else if snap.volume < 50.0 {
            "icons/volume-1.svg"
        } else {
            "icons/volume-2.svg"
        };
        let time = |t: String, color: Rgba| div().flex_none().min_w(px(52.)).text_size(px(size::SM)).text_color(color).child(t);
        let remaining = if snap.dur > 0.0 { format!("-{}", logic::format_time(snap.dur - shown_pos)) } else { "--:--".into() };
        let left = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(ctl("play", if snap.paused { "icons/play.svg" } else { "icons/pause.svg" }, 30.).on_click(cx.listener(|this, _, _w, cx| this.toggle_pause(cx))))
            .child(jump_ctl("rew", "icons/rotate-ccw.svg").on_click(cx.listener(|this, _, _w, cx| this.seek_by(-10.0, cx))))
            .child(jump_ctl("fwd", "icons/rotate-cw.svg").on_click(cx.listener(|this, _, _w, cx| this.seek_by(10.0, cx))))
            .child(ctl("mute", vol_icon, 24.).on_click(cx.listener(|this, _, _w, cx| this.toggle_mute(cx))))
            .child(self.volume_slider(cx));
        let menu = self.menu;
        let right = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .when(backend::has_next(&self.req), |d| {
                d.child(
                    div()
                        .id("next")
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(40.))
                        .px(px(12.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .hover(|s| s.bg(Tokens::hover()))
                        .text_size(px(size::MD))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(Tokens::text_1())
                        .child(icon("icons/skip-forward.svg", Tokens::text_1(), 22.))
                        .child(if self.next_busy { "Finding…" } else { "Next episode" })
                        .on_click(cx.listener(|this, _, window, cx| this.play_next(window, cx))),
                )
            })
            .child(
                ctl("tracks", "icons/messages-square.svg", 24.)
                    .when(menu == Some(Menu::Tracks), |d| d.bg(Tokens::active()))
                    .on_click(cx.listener(|this, _, _w, cx| this.toggle_menu(Menu::Tracks, cx))),
            )
            .child(
                ctl("speed", "icons/gauge.svg", 24.)
                    .when(menu == Some(Menu::Speed), |d| d.bg(Tokens::active()))
                    .on_click(cx.listener(|this, _, _w, cx| this.toggle_menu(Menu::Speed, cx))),
            )
            .child(
                ctl("fs", if window.is_fullscreen() { "icons/minimize.svg" } else { "icons/maximize.svg" }, 24.)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_fullscreen(window, cx))),
            );
        div()
            .id("bottom-bar")
            .absolute()
            .bottom_0()
            .left_0()
            .w_full()
            .h(px(BOTTOM_H))
            .px(px(20.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .justify_end()
            .gap(px(6.))
            .bg(linear_gradient(0., linear_color_stop(rgba(0x000000e6), 0.), linear_color_stop(rgba(0x00000000), 1.)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(ready, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(time(logic::format_time(shown_pos), Tokens::text_1()))
                        .child(self.scrubber(cx))
                        .child(time(remaining, Tokens::text_2())),
                )
            })
            .child(div().flex().items_center().justify_between().h(px(48.)).when(ready, |d| d.child(left).child(right)))
    }

    fn scrubber(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dur = self.snap.dur;
        let pos = self.scrub.unwrap_or(self.snap.pos);
        let frac = |t: f64| if dur > 0.0 { (t / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let active = self.hover_x.is_some() || self.drag == Some(Drag::Seek);
        let (track_h, thumb) = if active { (8., 16.) } else { (4., 12.) };
        let b = self.seek_bounds.get();
        let hover = self.hover_x.filter(|_| dur > 0.0).map(|x| logic::fraction_in(x, b.origin.x.as_f32(), b.size.width.as_f32()));
        let bounds = self.seek_bounds.clone();
        div()
            .id("seek")
            .relative()
            .flex_1()
            .h(px(24.))
            .flex()
            .items_center()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                    if this.snap.dur > 0.0 {
                        this.drag = Some(Drag::Seek);
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
                    .bg(rgba(0xffffff33))
                    .child(div().absolute().top_0().left_0().h_full().w(relative(frac(self.snap.buffered.max(pos)))).rounded(px(track_h / 2.)).bg(rgba(0xffffff5c)))
                    .when(dur > 0.0, |d| {
                        d.children(self.skip.iter().map(|s| {
                            let color = match s.kind {
                                SkipType::Ed | SkipType::MixedEd => Tokens::warning(),
                                _ => Tokens::accent_soft(),
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
                            .bg(Tokens::accent()),
                    ),
            )
            .children(hover.map(|f| {
                div()
                    .absolute()
                    .bottom(px(28.))
                    .left(relative(f as f32))
                    .ml(px(-32.))
                    .w(px(64.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .bg(Tokens::surface_2())
                    .text_center()
                    .text_size(px(size::SM))
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
            .w(px(96.))
            .h(px(24.))
            .flex()
            .items_center()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
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
        let row = |id: String, label: String, selected: bool| {
            div()
                .id(SharedString::from(id))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(MENU_ROW_H))
                .px(px(10.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(size::MD))
                .text_color(if selected { Tokens::text_1() } else { Tokens::text_3() })
                .when(selected, |d| d.font_weight(FontWeight::MEDIUM))
                .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                .child(div().w(px(16.)).flex_none().when(selected, |d| d.child(icon("icons/check.svg", Tokens::text_1(), 16.))))
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let (w, h) = self.menu_size(menu);
        let body = match menu {
            Menu::Tracks => {
                let mut audio = div().id("audio-list").flex_1().min_h_0().overflow_y_scroll().px(px(8.)).pb(px(8.)).flex().flex_col();
                let tracks: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Audio).collect();
                if tracks.is_empty() {
                    audio = audio.child(div().px(px(10.)).py(px(8.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child("No audio tracks"));
                }
                for t in tracks {
                    let id = t.id;
                    audio = audio.child(row(format!("a{id}"), logic::track_label(t), t.selected).on_click(cx.listener(move |this, _, _w, cx| this.select_track(TrackKind::Audio, Some(id), cx))));
                }
                let subs: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Sub).collect();
                let off = !subs.iter().any(|t| t.selected);
                let mut list = div().id("sub-list").flex_1().min_h_0().overflow_y_scroll().px(px(8.)).pb(px(8.)).flex().flex_col();
                list = list.child(row("s-off".into(), "Off".into(), off).on_click(cx.listener(|this, _, _w, cx| this.select_track(TrackKind::Sub, None, cx))));
                for t in subs {
                    let id = t.id;
                    list = list.child(row(format!("s{id}"), logic::track_label(t), t.selected).on_click(cx.listener(move |this, _, _w, cx| this.select_track(TrackKind::Sub, Some(id), cx))));
                }
                let head = |title: &'static str| {
                    div().flex_none().h(px(MENU_HEAD_H)).px(px(18.)).flex().items_center().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title)
                };
                let col = || div().flex_1().min_w_0().h_full().flex().flex_col();
                div()
                    .size_full()
                    .flex()
                    .child(col().child(head("Audio")).child(audio))
                    .child(div().w(px(1.)).my(px(12.)).bg(Tokens::border()))
                    .child(col().child(head("Subtitles")).child(list))
            }
            Menu::Speed => {
                let mut list = div().id("speed-list").flex_1().min_h_0().px(px(8.)).pb(px(8.)).flex().flex_col();
                for s in logic::SPEEDS {
                    let selected = (self.snap.speed - s).abs() < 0.01;
                    let label = if s == 1.0 { "1x (Normal)".to_string() } else { format!("{s}x") };
                    list = list.child(row(format!("sp{s}"), label, selected).on_click(cx.listener(move |this, _, _w, cx| this.set_speed(s, cx))));
                }
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(div().flex_none().h(px(MENU_HEAD_H)).px(px(18.)).flex().items_center().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Playback speed"))
                    .child(list)
            }
        };
        div()
            .id("menu")
            .absolute()
            .right(px(MENU_RIGHT))
            .bottom(px(BOTTOM_H))
            .w(px(w))
            .h(px(h))
            .rounded(px(10.))
            .overflow_hidden()
            .bg(rgba(0x14181cf5))
            .border_1()
            .border_color(Tokens::border())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(body)
    }

    fn skip_button(&self, seg: SkipSegment, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("skip")
            .absolute()
            .right(px(SKIP_RIGHT))
            .bottom(px(logic::skip_bottom(self.controls)))
            .w(px(SKIP_W))
            .h(px(SKIP_H))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .rounded(px(6.))
            .border_2()
            .border_color(rgba(0xffffffb3))
            .bg(rgba(0x14181ce6))
            .text_size(px(size::LG))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(Tokens::text_1())
            .cursor_pointer()
            .hover(|s| s.bg(Tokens::text_1()).text_color(Tokens::base()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(logic::skip_label(seg.kind))
            .child(icon("icons/skip-forward.svg", Tokens::text_1(), 18.))
            .on_click(cx.listener(|this, _, _w, cx| this.skip_now(cx)))
    }
}

/// Large white overlay control.
fn ctl(id: impl Into<ElementId>, path: &'static str, size: f32) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size + 18.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(Tokens::hover()))
        .child(icon(path, Tokens::text_1(), size))
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

fn spinner() -> impl IntoElement {
    icon("icons/loader-circle.svg", Tokens::text_1(), 52.).with_animation(
        "spin",
        Animation::new(Duration::from_millis(900)).repeat(),
        |s, delta| s.with_transformation(Transformation::rotate(percentage(delta))),
    )
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

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let (rect, stage) = (self.rect.clone(), self.stage.clone());
        let surface = self.mpv.as_ref().map(|m| m.surface());
        let controls = self.controls && !matches!(self.phase, Phase::Error(_));
        let menu = self.menu.filter(|_| controls);
        let skip = self.skip_segment();

        div()
            .id("player-root")
            .track_focus(&self.focus)
            .key_context("Player")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x000000))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| this.on_mouse_move(ev.position, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _w, cx| this.drag_end(cx)))
            // Mouse back button: same as the back arrow (the shell's handler would skip exit()).
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.exit(window, cx);
                }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _w, _cx| {
                if !*hovered {
                    this.mouse_in_bar = false;
                    this.hover_x = None;
                }
            }))
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        stage.set(bounds);
                        let r = (
                            (bounds.origin.x.as_f32() * scale) as i32,
                            (bounds.origin.y.as_f32() * scale) as i32,
                            (bounds.size.width.as_f32() * scale) as i32,
                            (bounds.size.height.as_f32() * scale) as i32,
                        );
                        if rect.get() != r {
                            rect.set(r);
                            if let Some(h) = surface {
                                mpv::move_surface(h, r);
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            // Click target for the whole video: pause, double-click fullscreen, close menus.
            .child(div().id("stage").absolute().top_0().left_0().size_full().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.focus.focus(window, cx);
                    if this.menu.take().is_some() {
                        cx.notify();
                    } else if ev.click_count == 2 {
                        this.toggle_pause(cx); // undo the first click's pause
                        this.toggle_fullscreen(window, cx);
                    } else if ev.click_count == 1 {
                        this.toggle_pause(cx);
                    }
                }),
            ))
            .children(self.center(cx))
            .when(controls, |d| d.child(self.top_bar(cx)).child(self.bottom_bar(window, cx)))
            .children(menu.map(|m| self.menu_view(m, cx)))
            .children(skip.map(|s| self.skip_button(s, cx)))
    }
}
