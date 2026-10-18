//! The player page: black stage with the mpv popup, control bar below, optional side panel.
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_media::types::{SkipSegment, SkipType, StremioSubtitle};
use mv_ui::nav::{navigate, PlayRequest, Route};
use mv_ui::theme::{size, Tokens};
use mv_ui::{bg, button, chip, icon_button, ButtonKind};

use crate::backend::{self, Ident, Resolved};
use crate::logic;
use crate::mpv::{self, Event, Mpv, Track, TrackKind};

const TICK: Duration = Duration::from_millis(33);
const POLL_EVERY: u32 = 4; // ticks (~130 ms)
const TRACKS_EVERY: u32 = 8; // polls (~1 s)
const SAVE_EVERY: Duration = Duration::from_secs(10);
const PANEL_W: f32 = 300.0;

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
enum Panel {
    Audio,
    Subs,
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
}

pub struct PlayerView {
    req: PlayRequest,
    ident: Ident,
    focus: FocusHandle,
    /// Video area in device pixels, written by the stage canvas each layout.
    rect: Rc<Cell<(i32, i32, i32, i32)>>,
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
    panel: Option<Panel>,
    drag: Option<Drag>,
    /// Pending seek target while scrubbing.
    scrub: Option<f64>,
    ticks: u32,
    polls: u32,
    last_save: Instant,
    played_logged: bool,
    complete_logged: bool,
    toast: Option<(String, Instant)>,
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
            panel: None,
            drag: None,
            scrub: None,
            ticks: 0,
            polls: 0,
            last_save: Instant::now(),
            played_logged: false,
            complete_logged: false,
            toast: None,
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

    fn toast(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
    }

    // ── per-frame work ──

    fn tick(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ticks = self.ticks.wrapping_add(1);
        let Some(mpv) = &self.mpv else {
            if self.phase == Phase::Connecting && self.ticks % 8 == 0 {
                cx.notify(); // elapsed timer
            }
            return;
        };
        mpv::move_surface(mpv.surface(), self.rect.get());
        if self.ticks % POLL_EVERY != 0 {
            return;
        }
        self.polls = self.polls.wrapping_add(1);
        let mut changed = false;

        let events = mpv.poll_events();
        let (mut loaded, mut ended, mut failed) = (false, false, None);
        for ev in events {
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
        };
        if self.polls % TRACKS_EVERY == 0 || loaded {
            let t = mpv.tracks();
            if t != self.tracks {
                self.tracks = t;
                changed = true;
            }
        }
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
                let text = engine.stats(h).ok().map(|s| format!("{} peers · {}", s.peers, logic::format_speed(s.download_speed)));
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
        if changed {
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
            if self.snap.dur > 0.0 && self.snap.pos >= self.snap.dur - 0.5 && !self.snap.paused {
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

    fn toggle_panel(&mut self, p: Panel, cx: &mut Context<Self>) {
        self.panel = if self.panel == Some(p) { None } else { Some(p) };
        cx.notify();
    }

    fn toggle_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.toggle_fullscreen();
        cx.notify();
    }

    fn skip_now(&mut self, cx: &mut Context<Self>) {
        if let Some(seg) = logic::active_segment(&self.skip, self.snap.pos) {
            self.seek_to(seg.end, cx);
        }
    }

    fn exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.save("progress");
        if let Some(m) = &self.mpv {
            m.set_pause(true);
            m.set_visible(false);
        }
        window.defer(cx, |window, cx| navigate(Route::Back, window, cx));
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "space" | "k" => self.toggle_pause(cx),
            "left" => self.seek_by(-10.0, cx),
            "right" => self.seek_by(10.0, cx),
            "up" => self.set_volume(self.snap.volume + 5.0, cx),
            "down" => self.set_volume(self.snap.volume - 5.0, cx),
            "f" => self.toggle_fullscreen(window, cx),
            "m" => self.toggle_mute(cx),
            "escape" => {
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                    cx.notify();
                } else {
                    self.exit(window, cx);
                }
            }
            _ => {}
        }
    }

    // ── dragging (seek bar / volume) ──

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

    fn overlay(&self, cx: &mut Context<Self>) -> Option<Div> {
        let centered = || div().absolute().size_full().flex().flex_col().items_center().justify_center().gap(px(10.));
        match &self.phase {
            Phase::Connecting | Phase::Loading if self.mpv.is_none() => {
                let secs = self.started.elapsed().as_secs();
                let what = match backend_is_torrent(&self.req.source) {
                    true => "Connecting to peers…",
                    false => "Loading…",
                };
                let mut d = centered()
                    .child(div().text_size(px(size::LG)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(what))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("{}s", secs)));
                if let Some(s) = &self.torrent_status {
                    d = d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(s.clone()));
                }
                Some(d)
            }
            Phase::Error(msg) => Some(
                centered()
                    .bg(rgb(0x000000))
                    .child(div().text_size(px(size::LG)).font_weight(FontWeight::MEDIUM).text_color(Tokens::danger()).child("Couldn't play this"))
                    .child(div().max_w(px(520.)).text_center().text_size(px(size::SM)).text_color(Tokens::text_3()).child(msg.clone()))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(button("retry", "Retry", ButtonKind::Primary).on_click(cx.listener(|this, _, window, cx| {
                                this.mpv = None;
                                this.begin(window, cx);
                                cx.notify();
                            })))
                            .child(button("err-back", "Back", ButtonKind::Secondary).on_click(cx.listener(|this, _, window, cx| this.exit(window, cx)))),
                    ),
            ),
            _ => None,
        }
    }

    fn seek_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dur = self.snap.dur;
        let pos = self.scrub.unwrap_or(self.snap.pos);
        let frac = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let bounds = self.seek_bounds.clone();
        div()
            .id("seek")
            .flex_1()
            .h(px(18.))
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
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(Tokens::active())
                    .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
                    .when(dur > 0.0, |d| {
                        d.children(self.skip.iter().map(|s| {
                            let color = match s.kind {
                                SkipType::Ed | SkipType::MixedEd => Tokens::warning(),
                                _ => Tokens::accent_soft(),
                            };
                            div()
                                .absolute()
                                .top_0()
                                .h_full()
                                .left(relative((s.start / dur).clamp(0.0, 1.0) as f32))
                                .w(relative(((s.end - s.start) / dur).clamp(0.0, 1.0) as f32))
                                .bg(color)
                        }))
                    })
                    .child(div().absolute().top_0().left_0().h_full().w(relative(frac)).rounded(px(2.)).bg(Tokens::accent()))
                    .child(
                        div()
                            .absolute()
                            .top(px(-4.))
                            .left(relative(frac))
                            .ml(px(-6.))
                            .size(px(12.))
                            .rounded_full()
                            .bg(Tokens::text_1()),
                    ),
            )
    }

    fn volume_slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let v = if self.snap.muted { 0.0 } else { self.snap.volume };
        let frac = (v / 130.0).clamp(0.0, 1.0) as f32;
        let bounds = self.vol_bounds.clone();
        div()
            .id("vol")
            .w(px(84.))
            .h(px(18.))
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
                    .bg(Tokens::active())
                    .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(relative(frac))
                            .rounded(px(2.))
                            .bg(if self.snap.volume > 100.0 { Tokens::warning() } else { Tokens::text_2() }),
                    ),
            )
    }

    fn panel_view(&self, panel: Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let row = |id: String, label: String, selected: bool| {
            div()
                .id(SharedString::from(id))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(32.))
                .px(px(10.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(size::SM))
                .text_color(if selected { Tokens::text_1() } else { Tokens::text_2() })
                .when(selected, |d| d.bg(Tokens::accent_soft()))
                .hover(|s| s.bg(Tokens::hover()))
                .child(div().w(px(14.)).child(if selected { "✓" } else { "" }))
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let mut list = div().id("panel-list").flex_1().overflow_y_scroll().flex().flex_col().gap(px(2.)).p(px(8.));
        match panel {
            Panel::Audio => {
                let audio: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Audio).collect();
                if audio.is_empty() {
                    list = list.child(empty_note("No audio tracks"));
                }
                for t in audio {
                    let id = t.id;
                    list = list.child(row(format!("a{id}"), logic::track_label(t), t.selected).on_click(cx.listener(move |this, _, _w, cx| this.select_track(TrackKind::Audio, Some(id), cx))));
                }
            }
            Panel::Subs => {
                let subs: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Sub).collect();
                let none_selected = !subs.iter().any(|t| t.selected);
                list = list.child(row("s-off".into(), "Off".into(), none_selected).on_click(cx.listener(|this, _, _w, cx| this.select_track(TrackKind::Sub, None, cx))));
                for t in subs {
                    let id = t.id;
                    list = list.child(row(format!("s{id}"), logic::track_label(t), t.selected).on_click(cx.listener(move |this, _, _w, cx| this.select_track(TrackKind::Sub, Some(id), cx))));
                }
            }
            Panel::Speed => {
                for s in logic::SPEEDS {
                    let selected = (self.snap.speed - s).abs() < 0.01;
                    list = list.child(row(format!("sp{s}"), format!("{s}x"), selected).on_click(cx.listener(move |this, _, _w, cx| this.set_speed(s, cx))));
                }
            }
        }
        let title = match panel {
            Panel::Audio => "Audio",
            Panel::Subs => "Subtitles",
            Panel::Speed => "Speed",
        };
        div()
            .w(px(PANEL_W))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(Tokens::surface())
            .border_l_1()
            .border_color(Tokens::border())
            .child(div().px(px(16.)).py(px(12.)).text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title))
            .child(list)
    }
}

fn backend_is_torrent(src: &str) -> bool {
    matches!(logic::classify_source(src), logic::SourceKind::Torrent(_))
}

fn empty_note(text: &'static str) -> Div {
    div().px(px(10.)).py(px(8.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(text)
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
        let rect = self.rect.clone();
        let surface = self.mpv.as_ref().map(|m| m.surface());
        let snap = self.snap.clone();
        let shown_pos = self.scrub.unwrap_or(snap.pos);
        let fullscreen = window.is_fullscreen();
        let skip_seg = logic::active_segment(&self.skip, snap.pos);

        let stage = div()
            .id("stage")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(rgb(0x000000))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.focus.focus(window, cx);
                    if ev.click_count == 2 {
                        this.toggle_fullscreen(window, cx);
                    } else if ev.click_count == 1 {
                        this.toggle_pause(cx);
                    }
                }),
            )
            .child(
                canvas(
                    move |bounds, _window, _cx| {
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
            .children(self.overlay(cx));

        let status = if let Phase::Error(_) = self.phase {
            None
        } else if snap.buffering && self.mpv.is_some() {
            Some(match &self.torrent_status {
                Some(s) => format!("Buffering… {s}"),
                None => "Buffering…".to_string(),
            })
        } else {
            self.toast.as_ref().map(|(t, _)| t.clone())
        };

        let vol_icon = if snap.muted || snap.volume <= 0.0 {
            "icons/volume-x.svg"
        } else if snap.volume < 50.0 {
            "icons/volume-1.svg"
        } else {
            "icons/volume-2.svg"
        };
        let title = match (self.req.season, self.req.episode) {
            (Some(s), Some(e)) => format!("{} · S{s} E{e}", self.req.title),
            (None, Some(e)) => format!("{} · Episode {e}", self.req.title),
            _ => self.req.title.clone(),
        };
        let panel = self.panel;

        let bar = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(16.))
            .pt(px(8.))
            .pb(px(10.))
            .bg(Tokens::surface())
            .border_t_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().w(px(52.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(logic::format_time(shown_pos)))
                    .child(self.seek_bar(cx))
                    .child(div().w(px(52.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(logic::format_time(snap.dur))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(icon_button("back", "icons/arrow-left.svg", true).on_click(cx.listener(|this, _, window, cx| this.exit(window, cx))))
                    .child(
                        icon_button("play", if snap.paused { "icons/play.svg" } else { "icons/pause.svg" }, self.mpv.is_some())
                            .on_click(cx.listener(|this, _, _w, cx| this.toggle_pause(cx))),
                    )
                    .child(icon_button("mute", vol_icon, self.mpv.is_some()).on_click(cx.listener(|this, _, _w, cx| this.toggle_mute(cx))))
                    .child(self.volume_slider(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(div().truncate().text_size(px(size::MD)).text_color(Tokens::text_1()).child(title))
                            .children(status.map(|s| div().flex_none().text_size(px(size::SM)).text_color(Tokens::text_3()).child(s))),
                    )
                    .children(skip_seg.map(|seg| {
                        button("skip", logic::skip_label(seg.kind), ButtonKind::Primary).on_click(cx.listener(|this, _, _w, cx| this.skip_now(cx)))
                    }))
                    .child(chip("audio-btn", "Audio", panel == Some(Panel::Audio)).on_click(cx.listener(|this, _, _w, cx| this.toggle_panel(Panel::Audio, cx))))
                    .child(chip("subs-btn", "Subtitles", panel == Some(Panel::Subs)).on_click(cx.listener(|this, _, _w, cx| this.toggle_panel(Panel::Subs, cx))))
                    .child(
                        chip("speed-btn", format!("{}x", snap.speed), panel == Some(Panel::Speed))
                            .on_click(cx.listener(|this, _, _w, cx| this.toggle_panel(Panel::Speed, cx))),
                    )
                    .child(
                        icon_button("fs", if fullscreen { "icons/minimize.svg" } else { "icons/maximize.svg" }, true)
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_fullscreen(window, cx))),
                    ),
            );

        div()
            .id("player-root")
            .track_focus(&self.focus)
            .key_context("Player")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x000000))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| this.drag_to(ev.position.x.as_f32(), cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _w, cx| this.drag_end(cx)))
            .child(div().flex_1().min_h_0().flex().child(stage).children(panel.map(|p| self.panel_view(p, cx))))
            .child(bar)
    }
}
