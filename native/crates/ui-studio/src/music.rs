//! The Music step: the chapter's own score (mv-score) shown like a small DAW - cues on a timeline, the
//! energy curve, one lane per track with mute / solo / level - with per-cue mood overrides, new
//! variations, the optional AI spotting pass and the orchestra download.
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_score::{orchestra, Instrument, Level, MusicSettings, Role, Score, Strip, Style};
use mv_studio::voices::DownloadState;
use mv_ui::{button, chip, empty_state, icon, progress_bar, size, ButtonKind, Tokens};

use crate::audio;
use crate::page::{chref, StudioPage};
use crate::phases::primary;

/// The focused chapter's music, loaded when the Music step first shows it.
#[derive(Default)]
pub(crate) struct MusicState {
    pub chapter: Option<String>,
    pub settings: MusicSettings,
    pub score: Option<Score>,
    pub fresh: bool,
    /// What is running ("Composing…"), if anything.
    pub busy: Option<String>,
    pub cue: Option<usize>,
    pub report: Option<String>,
}

const DUCKS: [(&str, f32); 3] = [("Light", 5.0), ("Normal", 8.0), ("Strong", 12.0)];
const LANE_LABEL_W: f32 = 196.;

fn style_color(s: Style) -> Rgba {
    rgb(match s {
        Style::Action => 0xE5484D,
        Style::Epic => 0xF76B15,
        Style::Tense => 0x8E4EC6,
        Style::Mystery => 0x3E63DD,
        Style::Emotional => 0x0090FF,
        Style::Romance => 0xE93D82,
        Style::Comedy => 0xD6A100,
        Style::Calm => 0x30A46C,
        Style::Flashback => 0xA18072,
        Style::Reveal => 0x12A594,
    })
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0) as u64;
    format!("{}:{:02}", t / 60, t % 60)
}

fn instrument_name(i: Instrument, orchestra: bool) -> String {
    match i {
        Instrument::Kit => "Kit".into(),
        Instrument::Synth(v) => format!("{v:?}"),
        Instrument::Gm { program, fallback } => {
            if orchestra {
                format!("GM {program}")
            } else {
                format!("{fallback:?}")
            }
        }
    }
}

impl StudioPage {
    /// Loads the focused chapter's music settings and score when the focus changed.
    fn music_sync(&mut self) {
        let Some(f) = &self.focus else { return };
        if self.music.chapter.as_deref() == Some(f.item.id.as_str()) {
            return;
        }
        let r = chref(&f.item);
        let settings = mv_score::load_settings(&r);
        self.music = MusicState {
            chapter: Some(f.item.id.clone()),
            fresh: mv_score::is_fresh(&r, &settings),
            score: mv_score::load_score(&r),
            settings,
            ..Default::default()
        };
    }

    fn music_edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut MusicSettings)) {
        let Some(focus) = &self.focus else { return };
        let r = chref(&focus.item);
        f(&mut self.music.settings);
        if let Err(e) = mv_score::save_settings(&r, &self.music.settings) {
            self.say(true, format!("Could not save the music settings: {e:#}"));
        }
        self.music.fresh = mv_score::is_fresh(&r, &self.music.settings);
        cx.notify();
    }

    /// Composes and renders the score for the target chapters (AI spotting first when it is on and
    /// there is no cue sheet yet). `then_render` queues the video afterwards.
    pub(crate) fn compose_music(&mut self, then_render: bool, cx: &mut Context<Self>) {
        if self.music.busy.is_some() {
            return;
        }
        let chapters: Vec<_> = self.targets().iter().filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.audio_files > 0)).map(chref).collect();
        if chapters.is_empty() {
            self.say(true, "Generate the narration audio first (Audio step).");
            return cx.notify();
        }
        let reg = ui_assistant::registry(cx);
        let model = self.model(cx).map(|m| m.0);
        self.music.busy = Some(if then_render { "Scoring before the render…".into() } else { "Composing…".into() });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let mut notes: Vec<String> = vec![];
                for ch in chapters {
                    let mut settings = mv_score::load_settings(&ch);
                    if !settings.enabled {
                        continue;
                    }
                    if settings.use_ai && settings.directions.is_empty() {
                        match &model {
                            Some(m) => match mv_score::ai::spot(reg.clone(), m.clone(), ch.clone(), Default::default()).await {
                                Ok(d) => {
                                    settings.directions = d;
                                    let _ = mv_score::save_settings(&ch, &settings);
                                }
                                Err(e) => notes.push(format!("{}: AI spotting failed ({e:#}), used the rules.", ch.label)),
                            },
                            None => notes.push(format!("{}: no AI model picked, used the rules.", ch.label)),
                        }
                    }
                    let c2 = ch.clone();
                    let s2 = settings.clone();
                    match tokio::task::spawn_blocking(move || mv_score::build(&c2, &s2, true)).await {
                        Ok(Ok((score, rep))) => notes.push(format!(
                            "{}: {} cues, {} long{}",
                            ch.label,
                            score.cues.len(),
                            fmt_time(score.duration),
                            rep.music_lufs.zip(rep.narration_lufs).map(|(m, n)| format!(", music {:.0} LU under the voice", n - m)).unwrap_or_default()
                        )),
                        Ok(Err(e)) => notes.push(format!("{}: {e:#}", ch.label)),
                        Err(e) => notes.push(format!("{}: {e}", ch.label)),
                    }
                }
                notes
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.music.busy = None;
                this.music.chapter = None; // reload
                match res {
                    Ok(notes) => this.music.report = Some(notes.join("\n")),
                    Err(e) => this.say(true, format!("Music failed: {e:#}")),
                }
                this.refresh_focus(true, cx);
                if then_render {
                    this.run_pipeline(&["sync_map", "video"], false, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The Video step's render button: brings the score up to date first when music is on.
    pub(crate) fn render_video(&mut self, cx: &mut Context<Self>) {
        let stale = self.targets().iter().any(|c| {
            let r = chref(c);
            let s = mv_score::load_settings(&r);
            s.enabled && !mv_score::is_fresh(&r, &s)
        });
        if stale {
            self.compose_music(true, cx);
        } else {
            self.run_pipeline(&["sync_map", "video"], false, cx);
        }
    }

    fn play_from(&mut self, t: f64, cx: &mut Context<Self>) {
        let Some(f) = &self.focus else { return };
        let src = mv_score::preview_wav(&chref(&f.item));
        let out = std::env::temp_dir().join("mangavox-score-from.wav");
        let res = (|| -> anyhow::Result<PathBuf> {
            let mut r = hound::WavReader::open(&src)?;
            let spec = r.spec();
            r.seek((t * spec.sample_rate as f64) as u32)?;
            let mut w = hound::WavWriter::create(&out, spec)?;
            for s in r.samples::<i16>() {
                w.write_sample(s?)?;
            }
            w.finalize()?;
            Ok(out.clone())
        })();
        match res {
            Ok(p) => audio::play(&p),
            Err(e) => self.say(true, format!("Could not play the preview: {e:#}")),
        }
        cx.notify();
    }

    fn download_orchestra(&mut self, cx: &mut Context<Self>) {
        mv_ui::bg::runtime().spawn(async {
            let _ = orchestra::run_download().await;
        });
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let running = matches!(orchestra::download_state(), Some(DownloadState::Running(_)));
            if this
                .update(cx, |this, cx| {
                    if !running {
                        this.music.chapter = None;
                    }
                    cx.notify();
                })
                .is_err()
                || !running
            {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------ view

    pub(crate) fn music_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.music_sync();
        let f = self.focus.as_ref().expect("focus");
        let has_audio = f.status.audio_files > 0;
        let m = &self.music;
        let s = m.settings.clone();
        let busy = m.busy.clone();

        let label = |t: &'static str| div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(t);
        let row = || div().flex().flex_wrap().items_center().gap(px(6.));

        let controls = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .rounded(px(12.))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(
                row()
                    .child(chip("music-on", if s.enabled { "Music on" } else { "Music off" }, s.enabled).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.music_edit(cx, |s| s.enabled = !s.enabled))))
                    .child(chip("music-sfx", "Sound effects", s.sfx).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.music_edit(cx, |s| s.sfx = !s.sfx))))
                    .child(chip("music-ai", "AI spotting", s.use_ai).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.music_edit(cx, |s| s.use_ai = !s.use_ai))))
                    .child(div().flex_1())
                    .map(|d| match &busy {
                        Some(b) => d.child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(b.clone())),
                        None => d
                            .when(s.use_ai && !s.directions.is_empty(), |d| {
                                d.child(button("music-respot", "Ask the AI again", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.music_edit(cx, |s| s.directions.clear());
                                    this.compose_music(false, cx);
                                })))
                            })
                            .when(self.music.score.is_some(), |d| {
                                let playing = audio::now_playing().is_some();
                                d.child(button("music-preview", if playing { "Stop" } else { "Play with narration" }, ButtonKind::Secondary).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if audio::now_playing().is_some() {
                                        audio::stop();
                                    } else if let Some(f) = &this.focus {
                                        audio::play(&mv_score::preview_wav(&chref(&f.item)));
                                    }
                                    cx.notify();
                                })))
                            })
                            .child(
                                primary(
                                    "music-compose",
                                    "icons/music.svg",
                                    self.target_label(if self.music.score.is_none() { "Compose score" } else if self.music.fresh { "Recompose" } else { "Update score" }),
                                    has_audio && s.enabled,
                                )
                                .when(has_audio && s.enabled, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.compose_music(false, cx)))),
                            ),
                    }),
            )
            .child(
                row()
                    .child(label("Level"))
                    .children(Level::ALL.map(|l| chip(SharedString::from(format!("lvl-{}", l.label())), l.label(), s.level == l).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.music_edit(cx, |s| s.level = l)))))
                    .child(div().w(px(12.)))
                    .child(label("Duck under voice"))
                    .children(DUCKS.map(|(n, db)| chip(SharedString::from(format!("duck-{n}")), n, (s.duck_db - db).abs() < 0.1).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.music_edit(cx, |s| s.duck_db = db))))),
            )
            .child(self.orchestra_row(cx))
            .when(!has_audio, |d| d.child(label("Generate the narration first (Audio step): the score is timed to it.")))
            .when(has_audio && self.music.score.is_some() && !self.music.fresh && busy.is_none() && s.enabled, |d| {
                d.child(div().text_size(px(size::XS)).text_color(Tokens::accent()).child("Settings or narration changed since the last score. Update it, or the video render will do it for you."))
            })
            .when_some(self.music.report.clone(), |d, r| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(r)));

        let body = match self.music.score.clone() {
            Some(score) => self.timeline(&score, cx),
            None => div().py(px(30.)).child(empty_state("No score yet", "Compose one: the AI follows each scene's mood and intensity, builds into fights, drops before reveals and stays quiet under the narration. Everything is generated on this PC, so the music is yours.")).into_any_element(),
        };
        div().flex().flex_col().gap(px(16.)).max_w(px(1100.)).child(controls).child(body).into_any_element()
    }

    fn orchestra_row(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let label = div().text_size(px(size::XS)).text_color(Tokens::text_3());
        let row = div().flex().items_center().gap(px(10.));
        if orchestra::installed() {
            return row.child(icon("icons/check.svg", Tokens::accent(), 13.)).child(label.child("Orchestra installed: strings, brass, piano, choir and more (GeneralUser GS, royalty-free).")).into_any_element();
        }
        match orchestra::download_state() {
            Some(DownloadState::Running(p)) => row
                .child(label.child(format!("Downloading the orchestra… {:.0} / {:.0} MB", p.done as f64 / 1e6, p.total as f64 / 1e6)))
                .child(div().w(px(160.)).child(progress_bar("orch-progress", p.fraction(), true)))
                .child(button("orch-cancel", "Cancel", ButtonKind::Ghost).on_click(|_, _, _| orchestra::cancel_download()))
                .into_any_element(),
            other => row
                .child(label.child(match other {
                    Some(DownloadState::Failed(e)) => format!("Orchestra download failed: {e}"),
                    _ => format!("Using the built-in synth. Add the orchestra ({:.0} MB, royalty-free) for real strings, brass and piano.", orchestra::ORCHESTRA.size() as f64 / 1e6),
                }))
                .child(button("orch-download", "Download orchestra", ButtonKind::Secondary).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.download_orchestra(cx))))
                .into_any_element(),
        }
    }

    fn timeline(&mut self, score: &Score, cx: &mut Context<Self>) -> AnyElement {
        let dur = score.duration.max(1.0);
        let x = |t: f64| relative((t / dur).clamp(0.0, 1.0) as f32);
        let w = |a: f64, b: f64| relative(((b.min(dur) - a.max(0.0)) / dur).max(0.002) as f32);
        let selected = self.music.cue;
        let s = self.music.settings.clone();
        let lane = || div().relative().flex_1().h(px(24.)).rounded(px(5.)).bg(Tokens::surface_2()).overflow_hidden();
        let head = |content: AnyElement| div().w(px(LANE_LABEL_W)).flex_none().flex().items_center().gap(px(4.)).child(content);
        let text = |t: String| div().text_size(px(size::XS)).text_color(Tokens::text_2()).truncate().child(t);

        // Ruler.
        let step = if dur > 600.0 { 120.0 } else if dur > 240.0 { 60.0 } else { 30.0 };
        let ticks = (0..=((dur / step) as usize)).map(|k| {
            let t = k as f64 * step;
            div().absolute().left(x(t)).top_0().text_size(px(size::XS)).text_color(Tokens::text_3()).child(fmt_time(t))
        });
        let ruler = div().flex().gap(px(8.)).child(head(div().into_any_element())).child(div().relative().flex_1().h(px(16.)).children(ticks));

        // Cues.
        let cues = score.cues.iter().map(|c| {
            let i = c.index;
            let on = selected == Some(i);
            div()
                .id(SharedString::from(format!("cue-{i}")))
                .absolute()
                .left(x(c.start))
                .w(w(c.start, c.end))
                .top_0()
                .bottom_0()
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(4.))
                .bg(style_color(c.style))
                .opacity(if on { 1.0 } else { 0.78 })
                .border_r_1()
                .border_color(Tokens::base())
                .when(on, |d| d.border_2().border_color(Tokens::text_1()))
                .cursor_pointer()
                .text_size(px(size::XS))
                .text_color(rgb(0xffffff))
                .when(c.hit, |d| d.child("▼"))
                .child(div().truncate().child(format!("{} · {}", c.style.label(), c.bpm.round())))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.music.cue = if this.music.cue == Some(i) { None } else { Some(i) };
                    cx.notify();
                }))
        });
        let cue_lane = div().flex().gap(px(8.)).child(head(text("Cues".into()).into_any_element())).child(lane().h(px(30.)).children(cues));

        // Energy curve (0-10), bucketed.
        let buckets = 160usize;
        let e = &score.energy.0;
        let bars = (0..buckets).map(|b| {
            let lo = b * e.len() / buckets;
            let hi = ((b + 1) * e.len() / buckets).max(lo + 1).min(e.len().max(1));
            let v = if e.is_empty() { 0.0 } else { e[lo.min(e.len() - 1)..hi].iter().copied().fold(0f32, f32::max) };
            div().flex_1().h(relative((v / 10.0).clamp(0.04, 1.0))).bg(Tokens::accent()).opacity(0.55)
        });
        let energy = div().flex().gap(px(8.)).child(head(text("Energy".into()).into_any_element())).child(div().flex().items_end().flex_1().h(px(36.)).children(bars));

        // Tracks.
        let orchestra = score.orchestra;
        let tracks = Role::ALL.map(|role| {
            let strip = s.strip(role);
            let audible = s.track_gain(role) > 0.0;
            let t = score.track(role);
            let clips: Vec<AnyElement> = t
                .map(|t| {
                    t.clips
                        .iter()
                        .flat_map(|c| {
                            if role == Role::Sfx {
                                c.notes
                                    .iter()
                                    .map(|n| div().absolute().left(x(n.t)).w(px(3.)).top(px(4.)).bottom(px(4.)).rounded(px(1.)).bg(Tokens::accent()).into_any_element())
                                    .collect::<Vec<_>>()
                            } else if c.notes.is_empty() {
                                vec![]
                            } else {
                                let style = score.cues.get(c.cue).map(|q| q.style).unwrap_or(Style::Calm);
                                let end = c.notes.iter().map(|n| n.t + n.dur).fold(c.start, f64::max).min(c.end);
                                vec![div()
                                    .absolute()
                                    .left(x(c.start))
                                    .w(w(c.start, end))
                                    .top(px(3.))
                                    .bottom(px(3.))
                                    .rounded(px(3.))
                                    .bg(style_color(style))
                                    .opacity(0.5)
                                    .px(px(4.))
                                    .text_size(px(size::XS))
                                    .text_color(rgb(0xffffff))
                                    .truncate()
                                    .child(instrument_name(c.instrument, orchestra))
                                    .into_any_element()]
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let key = role.key();
            let toggle = move |id: String, label: &'static str, on: bool| {
                div()
                    .id(SharedString::from(id))
                    .w(px(20.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(size::XS))
                    .when(on, |d| d.bg(Tokens::accent()).text_color(Tokens::base()))
                    .when(!on, |d| d.bg(Tokens::surface_2()).text_color(Tokens::text_2()))
                    .child(label)
            };
            let header = div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(div().flex_1().min_w_0().text_size(px(size::XS)).text_color(if audible { Tokens::text_1() } else { Tokens::text_3() }).truncate().child(role.label()))
                .child(toggle(format!("mute-{key}"), "M", strip.mute).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.music_edit(cx, |s| {
                        let st = s.mixer.entry(role).or_insert_with(Strip::default);
                        st.mute = !st.mute;
                    })
                })))
                .child(toggle(format!("solo-{key}"), "S", strip.solo).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.music_edit(cx, |s| {
                        let st = s.mixer.entry(role).or_insert_with(Strip::default);
                        st.solo = !st.solo;
                    })
                })))
                .child(toggle(format!("down-{key}"), "−", false).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.music_edit(cx, |s| {
                        let st = s.mixer.entry(role).or_insert_with(Strip::default);
                        st.gain_db = (st.gain_db - 3.0).max(-24.0);
                    })
                })))
                .child(div().w(px(36.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{:+.0} dB", strip.gain_db)))
                .child(toggle(format!("up-{key}"), "+", false).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.music_edit(cx, |s| {
                        let st = s.mixer.entry(role).or_insert_with(Strip::default);
                        st.gain_db = (st.gain_db + 3.0).min(9.0);
                    })
                })));
            div().flex().gap(px(8.)).child(head(header.w_full().into_any_element())).child(lane().when(!audible, |d| d.opacity(0.4)).children(clips))
        });

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(16.))
            .rounded(px(12.))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(ruler)
            .child(cue_lane)
            .child(energy)
            .children(tracks);
        if let Some(c) = selected.and_then(|i| score.cues.get(i)).cloned() {
            card = card.child(self.cue_panel(&c, cx));
        }
        card.into_any_element()
    }

    fn cue_panel(&mut self, c: &mv_score::Cue, cx: &mut Context<Self>) -> AnyElement {
        let key = c.pages.0;
        let forced = self.music.settings.styles.get(&key).copied();
        let variation = self.music.settings.variations.get(&key).copied().unwrap_or(0);
        let start = c.start;
        let mut tags = vec![];
        if c.hit {
            tags.push("hit on entry");
        }
        if c.drop_before {
            tags.push("drop before");
        }
        if c.riser_out {
            tags.push("riser out");
        }
        div()
            .mt(px(8.))
            .p(px(12.))
            .rounded(px(10.))
            .bg(Tokens::surface_2())
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().size(px(10.)).rounded_full().bg(style_color(c.style)))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(format!("Cue {} · {}", c.index + 1, c.label())))
                    .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!(
                        "{}–{} · pages {}–{} · energy {:.1}{}",
                        fmt_time(c.start),
                        fmt_time(c.end),
                        c.pages.0 + 1,
                        c.pages.1 + 1,
                        c.energy,
                        if tags.is_empty() { String::new() } else { format!(" · {}", tags.join(", ")) }
                    )))
                    .child(button("cue-play", "Play from here", ButtonKind::Secondary).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.play_from(start, cx))))
                    .child(button("cue-vary", "New variation", ButtonKind::Ghost).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.music_edit(cx, |s| {
                            s.variations.insert(key, variation + 1);
                        })
                    }))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(div().mr(px(4.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child("Mood"))
                    .child(chip("cue-auto", "Auto", forced.is_none()).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.music_edit(cx, |s| {
                            s.styles.remove(&key);
                        })
                    })))
                    .children(Style::ALL.map(|st| {
                        chip(SharedString::from(format!("cue-style-{}", st.label())), st.label(), forced == Some(st)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.music_edit(cx, |s| {
                                s.styles.insert(key, st);
                            })
                        }))
                    })),
            )
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Changes apply when you press Update score."))
            .into_any_element()
    }
}
