//! The Music step: the chapter's own score (mv-score). Up top the few choices most people need - a
//! music style (auto from the story, or one genre with a preview of each), energy, how many sound
//! effects, level and ducking - then the score shown like a small DAW: cues on a timeline, the energy
//! curve, one lane per track with mute / solo / level, per-cue genre overrides and new variations,
//! the optional AI spotting pass and the orchestra download.
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_score::{orchestra, Instrument, Level, MusicSettings, Role, Score, SfxAmount, Strip, Style};
use mv_studio::voices::DownloadState;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, empty_view, filter_chip, fmt_bytes, glyphs, icon_button, icon_ph, icon_tag, icon_toggle, inline_alert, muted, ph, ph_icon, progress_bar, segmented, shimmer_text, size, spinner,
    switch, tag, tip, AlertKind, Btn, ButtonKind, Hue, Interactive as _, Ph, Tokens, Tone, Weight,
};

use crate::audio;
use crate::page::{chref, NoticeAction, Phase, StudioPage};
use crate::phases::primary;
use crate::widgets::{card, fmt_secs, stat_pill};

/// The focused chapter's music, loaded when the Music step first shows it.
#[derive(Default)]
pub(crate) struct MusicState {
    pub chapter: Option<String>,
    pub settings: MusicSettings,
    pub score: Option<Score>,
    pub fresh: bool,
    /// What is running ("Composing…"), if anything.
    pub busy: Option<String>,
    /// The chapters that run is scoring (ids): their files must not be reset meanwhile, and only
    /// their Music step shows the busy state.
    pub composing: Vec<String>,
    pub cue: Option<usize>,
    pub report: Option<String>,
    /// The genre whose preview is being generated.
    pub previewing: Option<Style>,
}

const DUCKS: [(&str, f32); 3] = [("Light", 5.0), ("Normal", 8.0), ("Strong", 12.0)];
/// The energy setting's steps (0 chill .. 1 hyped; 0.5 follows the story as read).
const ENERGY: [(&str, f32); 5] = [("Chill", 0.0), ("Calm", 0.25), ("As the story", 0.5), ("Intense", 0.75), ("Hyped", 1.0)];
const LANE_LABEL_W: f32 = 236.;
const LANE_H: f32 = 28.;

/// Hue and icon per cue style, so a cue never rests on colour alone (WCAG 1.4.1) and the
/// timeline follows the theme's category palette instead of raw hex.
fn style_look(s: Style) -> (Hue, Ph) {
    match s {
        Style::Tension => (Hue::Violet, ph::EYE),
        Style::Epic => (Hue::Orange, ph::CROWN_SIMPLE),
        Style::Phonk => (Hue::Rose, ph::FIRE),
        Style::Sad => (Hue::Cyan, ph::MASK_SAD),
        Style::Lofi => (Hue::Amber, ph::SMILEY),
        Style::Horror => (Hue::Slate, ph::GHOST),
        Style::Mystery => (Hue::Blue, ph::DETECTIVE),
    }
}

/// Whether the genre's preview is what is playing now.
fn preview_playing(st: Style) -> bool {
    audio::now_playing().and_then(|p| p.file_name().map(|n| n.to_string_lossy().starts_with(&format!("mangavox-genre-{}-", st.key())))).unwrap_or(false)
}

/// Icon per mixer lane.
fn role_icon(r: Role) -> Ph {
    match r {
        Role::Pads => ph::WAVEFORM,
        Role::Keys => ph::MUSIC_NOTE,
        Role::Lead => ph::MUSIC_NOTES,
        Role::Bass => ph::SPEAKER_HIGH,
        Role::Drums => ph::PULSE,
        Role::Fx => ph::STAR_FOUR,
        Role::Sfx => ph::LIGHTNING,
    }
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
    pub(crate) fn music_sync(&mut self) {
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
            busy: self.music.busy.take(),
            composing: std::mem::take(&mut self.music.composing),
            previewing: self.music.previewing.take(),
            ..Default::default()
        };
    }

    fn music_edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut MusicSettings)) {
        let Some(focus) = &self.focus else { return };
        let r = chref(&focus.item);
        f(&mut self.music.settings);
        if let Err(e) = mv_score::save_settings(&r, &self.music.settings) {
            self.fail(format!("Could not save the music settings: {e:#}"));
        }
        self.music.fresh = mv_score::is_fresh(&r, &self.music.settings);
        cx.notify();
    }

    /// Why the score cannot be composed now, if it cannot.
    fn compose_blocker(&self) -> Option<&'static str> {
        let f = self.focus.as_ref()?;
        if self.music.busy.is_some() && !self.composing_focus() {
            Some("Another chapter's score is being composed. Wait for it to finish.")
        } else if f.status.audio_files == 0 {
            Some("Generate the narration first (Audio step): the score is timed to it")
        } else if !self.music.settings.enabled {
            Some("Music is off for this chapter. Turn it on below.")
        } else {
            None
        }
    }

    /// The running compose scores the focused chapter.
    fn composing_focus(&self) -> bool {
        self.focus.as_ref().is_some_and(|f| self.music.composing.contains(&f.item.id))
    }

    /// The Music step's main action, in the phase header like every other step.
    pub(crate) fn compose_button(&self, cx: &mut Context<Self>) -> Btn {
        let label = if self.music.score.is_none() {
            "Compose score"
        } else if self.music.fresh {
            "Recompose"
        } else {
            "Update score"
        };
        primary("music-compose", glyphs::MUSIC, self.target_label(label))
            .loading(self.music.busy.is_some() && self.composing_focus())
            .when_some(self.compose_blocker(), |b, r| b.disabled(r))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.compose_music(false, cx)))
    }

    /// Composes and renders the score for the target chapters (AI spotting first when it is on and
    /// there is no cue sheet yet). `then_render` queues the video afterwards.
    pub(crate) fn compose_music(&mut self, then_render: bool, cx: &mut Context<Self>) {
        if self.music.busy.is_some() {
            return;
        }
        let chapters: Vec<_> = self.targets().iter().filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.audio_files > 0)).map(chref).collect();
        if chapters.is_empty() {
            self.fail_with("Generate the narration audio first: the score is timed to it.", NoticeAction::GoTo(Phase::Audio));
            return cx.notify();
        }
        let reg = ui_assistant::registry(cx);
        let model = self.model(cx).map(|m| m.0);
        self.music.composing = chapters.iter().map(|c| c.chapter_id.clone()).collect();
        self.music.busy = Some(if then_render { "Scoring before the render…".into() } else { "Composing…".into() });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let mut notes: Vec<String> = vec![];
                let mut cues = 0usize;
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
                        Ok(Ok((score, rep))) => {
                            cues += score.cues.len();
                            notes.push(format!(
                                "{}: {} cues, {} long{}",
                                ch.label,
                                score.cues.len(),
                                fmt_secs(score.duration as f32),
                                rep.music_lufs.zip(rep.narration_lufs).map(|(m, n)| format!(", music {:.0} LU under the voice", n - m)).unwrap_or_default()
                            ))
                        }
                        Ok(Err(e)) => notes.push(format!("{}: {e:#}", ch.label)),
                        Err(e) => notes.push(format!("{}: {e}", ch.label)),
                    }
                }
                (notes, cues)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.music.busy = None;
                this.music.composing.clear();
                this.music.chapter = None; // reload
                match res {
                    Ok((notes, cues)) => {
                        if cues > 0 && !then_render {
                            toast::show(cx, Toast::success(format!("Score ready · {cues} cue{}", crate::page::plural(cues))).icon(glyphs::MUSIC));
                        }
                        this.music.report = Some(notes.join("\n"));
                    }
                    Err(e) => this.fail(format!("Music failed: {e:#}")),
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
            Err(e) => self.fail(format!("Could not play the preview: {e:#}")),
        }
        cx.notify();
    }

    /// Plays a short generated demo of a genre (made on first use, then cached); stops it when it is
    /// already playing.
    fn preview_style(&mut self, st: Style, cx: &mut Context<Self>) {
        if preview_playing(st) {
            audio::stop();
            return cx.notify();
        }
        if self.music.previewing.is_some() {
            return;
        }
        self.music.previewing = Some(st);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move { tokio::task::spawn_blocking(move || mv_score::demo::preview_wav(st)).await }).await;
            let _ = this.update(cx, |this, cx| {
                this.music.previewing = None;
                match res {
                    Ok(Ok(Ok(p))) => audio::play(&p),
                    Ok(Ok(Err(e))) => this.fail(format!("Could not make the {} preview: {e:#}", st.label())),
                    Ok(Err(e)) => this.fail(format!("Could not make the {} preview: {e}", st.label())),
                    Err(e) => this.fail(format!("Could not make the {} preview: {e:#}", st.label())),
                }
                cx.notify();
            });
        })
        .detach();
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
                        if orchestra::installed() {
                            toast::show(cx, Toast::success("Orchestra installed: recompose to hear it").icon(glyphs::MUSIC));
                        }
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
        let has_read = f.status.read_pages > 0;
        let s = self.music.settings.clone();
        // Busy only for the chapters being scored (a batch compose keeps running when the
        // focus moves on).
        let busy = self.music.busy.clone().filter(|_| self.composing_focus());
        let level_ix = Level::ALL.iter().position(|l| *l == s.level).unwrap_or(1);
        let duck_ix = DUCKS.iter().position(|(_, db)| (s.duck_db - db).abs() < 0.1).unwrap_or(1);
        let energy_ix = ENERGY.iter().enumerate().min_by(|a, b| (a.1 .1 - s.energy).abs().total_cmp(&(b.1 .1 - s.energy).abs())).map(|(i, _)| i).unwrap_or(2);
        let sfx_ix = SfxAmount::ALL.iter().position(|a| *a == s.sfx).unwrap_or(1);
        let row = |icon: Ph, label: &'static str, hint: &'static str, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(space::MD))
                .py(px(space::SM))
                .border_t_1()
                .border_color(Tokens::border())
                .child(icon_ph(icon, Tokens::text_3(), icon_px::MD))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(label))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(hint)),
                )
                .child(div().flex_none().child(control))
        };
        let playing = audio::now_playing().is_some();
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::MD))
            .child(
                switch("music-on", s.enabled).label("Music for this chapter").on_click(cx.listener(|this, on: &bool, _, cx| {
                    let on = *on;
                    this.music_edit(cx, |s| s.enabled = on)
                })),
            )
            .child(div().flex_1())
            .map(|d| match &busy {
                Some(b) => d.child(div().flex().items_center().gap(px(space::SM)).text_size(px(size::SM)).child(spinner("music-busy", Tokens::accent(), icon_px::SM)).child(shimmer_text(b.clone()))),
                None => d
                    .when(s.use_ai && !s.directions.is_empty(), |d| {
                        d.child(btn("music-respot", "Ask the AI again", ButtonKind::Ghost).small().icon(ph::SPARKLE).tooltip("Forget the AI's cue sheet and spot the chapter again").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.music_edit(cx, |s| s.directions.clear());
                            this.compose_music(false, cx);
                        })))
                    })
                    .when(self.music.score.is_some(), |d| {
                        d.child(btn("music-preview", if playing { "Stop" } else { "Play with narration" }, ButtonKind::Secondary).small().icon(if playing { ph::STOP } else { ph::PLAY }).selected(playing).on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                if audio::now_playing().is_some() {
                                    audio::stop();
                                } else if let Some(f) = &this.focus {
                                    audio::play(&mv_score::preview_wav(&chref(&f.item)));
                                }
                                cx.notify();
                            },
                        )))
                    }),
            });
        let controls = card()
            .gap(px(space::SM))
            .child(head)
            .when(s.enabled, |d| {
                d.child(self.style_picker(&s, cx))
                    .child(row(
                        ph::SPEEDOMETER,
                        "Energy",
                        "Calmer or more intense than the story reads: tempo, drums and layers follow",
                        segmented("music-energy", ENERGY.map(|(n, _)| n), energy_ix)
                            .small()
                            .on_change(cx.listener(|this, ix: &usize, _, cx| {
                                let v = ENERGY[(*ix).min(ENERGY.len() - 1)].1;
                                this.music_edit(cx, |s| s.energy = v)
                            }))
                            .into_any_element(),
                    ))
                    .child(row(
                        ph::LIGHTNING,
                        "Sound effects",
                        "Whooshes on scene changes, impacts and a heartbeat on reveals, hits in fights, the manga's own SFX",
                        segmented("music-sfx", SfxAmount::ALL.map(|a| a.label()), sfx_ix)
                            .small()
                            .on_change(cx.listener(|this, ix: &usize, _, cx| {
                                let a = SfxAmount::ALL[(*ix).min(SfxAmount::ALL.len() - 1)];
                                this.music_edit(cx, |s| s.sfx = a)
                            }))
                            .into_any_element(),
                    ))
                .child(row(
                    ph::GAUGE,
                    "Level",
                    "How present the music is overall",
                    segmented("music-level", Level::ALL.map(|l| l.label()), level_ix)
                        .small()
                        .on_change(cx.listener(|this, ix: &usize, _, cx| {
                            let l = Level::ALL[(*ix).min(Level::ALL.len() - 1)];
                            this.music_edit(cx, |s| s.level = l)
                        }))
                        .into_any_element(),
                ))
                .child(row(
                    ph::MICROPHONE,
                    "Duck under the voice",
                    "How far the music dips while the narrator speaks",
                    segmented("music-duck", DUCKS.map(|(n, _)| n), duck_ix)
                        .small()
                        .on_change(cx.listener(|this, ix: &usize, _, cx| {
                            let db = DUCKS[(*ix).min(DUCKS.len() - 1)].1;
                            this.music_edit(cx, |s| s.duck_db = db)
                        }))
                        .into_any_element(),
                ))
                .child(row(
                    ph::SPARKLE,
                    "AI spotting",
                    "The AI model marks where each cue starts and what it feels like (else rules decide)",
                    switch("music-ai", s.use_ai)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            let on = *on;
                            this.music_edit(cx, |s| s.use_ai = on)
                        }))
                        .into_any_element(),
                ))
                .child(self.orchestra_row(cx))
            });

        let stale = has_audio && self.music.score.is_some() && !self.music.fresh && busy.is_none() && s.enabled;
        let body = match self.music.score.clone() {
            Some(score) => self.timeline(&score, cx),
            None => div()
                .py(px(space::XL))
                .child(empty_view(
                    glyphs::MUSIC,
                    "No score yet",
                    "Compose one the way recap channels score a chapter: phonk for fights, epic orchestra for climaxes, dark beds for tension, sad piano, lo-fi for comedy. It builds into fights, drops before reveals, adds whooshes and impacts, and stays under the narration. It is generated on this PC, so the music is yours.",
                ))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .max_w(px(1100.))
            .when(!has_audio, |d| {
                d.child(inline_alert(AlertKind::Warning, "Narrate the chapter first", "The score is timed to the narration, so it can only be composed once the Audio step is done.").action(
                    "Go to Audio",
                    cx.listener(|this, _: &ClickEvent, _, cx| this.set_phase(Phase::Audio, cx)),
                ))
            })
            .when(has_audio && !has_read && s.enabled && s.genre.is_none(), |d| {
                d.child(inline_alert(AlertKind::Info, "Read the chapter for music that follows the story", "Without the Read step every page counts as quiet narration, so Auto scores it all as Mystery. Read it, or pick a style below.").action(
                    "Go to Read",
                    cx.listener(|this, _: &ClickEvent, _, cx| this.set_phase(Phase::Read, cx)),
                ))
            })
            .when(stale, |d| {
                d.child(inline_alert(AlertKind::Info, "The score is out of date", "The settings or the narration changed since it was composed. Update it, or the video render does it for you.").action(
                    "Update score",
                    cx.listener(|this, _: &ClickEvent, _, cx| this.compose_music(false, cx)),
                ))
            })
            .child(controls)
            .when_some(self.music.report.clone().filter(|r| !r.is_empty()), |d, r| {
                d.child(div().flex().items_start().gap(px(space::SM)).child(icon_ph(ph::INFO, Tokens::text_3(), icon_px::SM)).child(div().flex_1().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_3()).child(r)))
            })
            .child(body)
            .into_any_element()
    }

    /// "Music style": Auto (a genre per scene from the story) or one genre for the whole chapter, each
    /// with a play button for a short generated demo.
    fn style_picker(&mut self, s: &MusicSettings, cx: &mut Context<Self>) -> AnyElement {
        let previewing = self.music.previewing;
        let tile = |id: SharedString, icon: Ph, hue: Hue, title: &'static str, blurb: &'static str, on: bool| {
            div()
                .id(id)
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(space::SM))
                .p(px(space::SM))
                .rounded(px(radius::MD))
                .border_1()
                .border_color(if on { Tokens::accent() } else { Tokens::border() })
                .bg(if on { Tokens::accent_soft() } else { Tokens::surface_2() })
                .role(gpui_kit::Role::Button)
                .aria_selected(on)
                .pressable()
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(30.))
                        .rounded(px(radius::MD))
                        .bg(Tokens::hue_soft(hue))
                        .child(ph_icon(icon, if on { Weight::Fill } else { Weight::Regular }, Tokens::hue(hue), icon_px::SM)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.))
                        .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(title))
                        .child(div().text_size(px(size::XS)).line_height(px(15.)).text_color(Tokens::text_3()).child(blurb)),
                )
        };
        let auto = div().flex().items_center().w(px(268.)).child(
            tile("music-style-auto".into(), ph::MAGIC_WAND, Hue::Green, "Auto from the story", "A genre per scene: fights, climaxes, sad and calm stretches each get their own", s.genre.is_none())
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.music_edit(cx, |s| s.genre = None))),
        );
        let genres = Style::ALL.map(|st| {
            let (hue, icon) = style_look(st);
            let playing = preview_playing(st);
            let making = previewing == Some(st);
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .w(px(268.))
                .child(tile(SharedString::from(format!("music-style-{}", st.key())), icon, hue, st.label(), st.blurb(), s.genre == Some(st)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.music_edit(cx, |s| s.genre = Some(st)))))
                .child(if making {
                    div().flex_none().size(px(28.)).flex().items_center().justify_center().child(spinner(SharedString::from(format!("music-prev-busy-{}", st.key())), Tokens::accent(), icon_px::SM)).into_any_element()
                } else {
                    icon_button(SharedString::from(format!("music-prev-{}", st.key())), if playing { ph::STOP } else { ph::PLAY }, if playing { format!("Stop the {} preview", st.label()) } else { format!("Hear a short {} demo", st.label()) })
                        .small()
                        .selected(playing)
                        .when(previewing.is_some(), |b| b.disabled("Another preview is being made"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.preview_style(st, cx)))
                        .into_any_element()
                })
        });
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .py(px(space::SM))
            .border_t_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(icon_ph(ph::MUSIC_NOTES, Tokens::text_3(), icon_px::MD))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child("Music style"))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(match s.genre {
                                None => "Auto picks a genre per scene, like recap channels do. Pick one to use it for the whole chapter; press play to hear it.".to_string(),
                                Some(g) => format!("{} for the whole chapter (single cues can still be changed on the timeline).", g.label()),
                            })),
                    ),
            )
            .child(div().flex().flex_wrap().gap(px(space::SM)).child(auto).children(genres))
            .into_any_element()
    }

    fn orchestra_row(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let base = div().flex().items_center().gap(px(space::MD)).py(px(space::SM)).border_t_1().border_color(Tokens::border());
        let tile = |done: bool| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(32.))
                .rounded(px(radius::MD))
                .bg(Tokens::hue_soft(Hue::Violet))
                .child(ph_icon(ph::MUSIC_NOTES, if done { Weight::Fill } else { Weight::Regular }, Tokens::hue(Hue::Violet), icon_px::MD))
        };
        let text = |title: &'static str, hint: String| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(title))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(hint))
        };
        if orchestra::installed() {
            return base
                .child(tile(true))
                .child(text("Orchestra", "Strings, brass, piano, choir and more (GeneralUser GS, royalty-free).".into()))
                .child(icon_tag(ph::CHECK_CIRCLE, "Installed", Tone::Success))
                .into_any_element();
        }
        match orchestra::download_state() {
            Some(DownloadState::Running(p)) => base
                .child(tile(false))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(6.))
                        .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format!("Downloading the orchestra · {} of {}", fmt_bytes(p.done), fmt_bytes(p.total))))
                        .child(progress_bar("orch-progress", p.fraction(), true)),
                )
                .child(btn("orch-cancel", "Cancel", ButtonKind::Ghost).small().icon(ph::X).on_click(|_, _, _| orchestra::cancel_download()))
                .into_any_element(),
            other => {
                let failed = match other {
                    Some(DownloadState::Failed(e)) => Some(e),
                    _ => None,
                };
                let hint = match &failed {
                    Some(e) => format!("The download failed: {e}"),
                    None => format!("The built-in synth plays now. Add the orchestra ({}, royalty-free) for real strings, brass and piano.", fmt_bytes(orchestra::ORCHESTRA.size())),
                };
                base.child(tile(false))
                    .child(text("Orchestra", hint))
                    .child(
                        btn("orch-download", if failed.is_some() { "Try again" } else { "Download" }, ButtonKind::Secondary)
                            .small()
                            .icon(if failed.is_some() { glyphs::RETRY } else { glyphs::DOWNLOAD })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.download_orchestra(cx))),
                    )
                    .into_any_element()
            }
        }
    }

    fn timeline(&mut self, score: &Score, cx: &mut Context<Self>) -> AnyElement {
        let dur = score.duration.max(1.0);
        let x = |t: f64| relative((t / dur).clamp(0.0, 1.0) as f32);
        let w = |a: f64, b: f64| relative(((b.min(dur) - a.max(0.0)) / dur).max(0.002) as f32);
        let selected = self.music.cue;
        let s = self.music.settings.clone();
        let lane = || div().relative().flex_1().h(px(LANE_H)).rounded(px(radius::SM)).bg(Tokens::well()).overflow_hidden();
        let head = |content: AnyElement| div().w(px(LANE_LABEL_W)).flex_none().flex().items_center().gap(px(space::XS)).child(content);
        let label = |icon: Ph, t: String| div().flex().items_center().gap(px(6.)).min_w_0().child(icon_ph(icon, Tokens::text_3(), icon_px::SM)).child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).truncate().child(t));

        // Ruler.
        let step = if dur > 600.0 {
            120.0
        } else if dur > 240.0 {
            60.0
        } else {
            30.0
        };
        let ticks = (0..=((dur / step) as usize)).map(|k| {
            let t = k as f64 * step;
            div().absolute().left(x(t)).top_0().pl(px(3.)).border_l_1().border_color(Tokens::border_strong()).text_size(px(size::XS)).text_color(Tokens::text_3()).child(fmt_secs(t as f32))
        });
        let ruler = div().flex().gap(px(space::SM)).child(head(div().into_any_element())).child(div().relative().flex_1().h(px(16.)).children(ticks));

        // Cues: soft fill in the style's hue, its icon and name; the selected one is solid-edged.
        let cues = score.cues.iter().map(|c| {
            let i = c.index;
            let on = selected == Some(i);
            let (hue, icon) = style_look(c.style);
            let color = Tokens::hue(hue);
            div()
                .id(SharedString::from(format!("cue-{i}")))
                .absolute()
                .left(x(c.start))
                .w(w(c.start, c.end))
                .top(px(2.))
                .bottom(px(2.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(space::XS))
                .rounded(px(radius::XS))
                .bg(if on { color.alpha(0.34) } else { Tokens::hue_soft(hue) })
                .border_l_2()
                .border_color(color)
                .when(on, |d| d.border_1().border_l_2().border_color(color))
                .text_size(px(size::XS))
                .font_weight(FontWeight::MEDIUM)
                .text_color(Tokens::text_1())
                .role(gpui_kit::Role::Button)
                .aria_selected(on)
                .tooltip(tip(format!("{} · {}–{}{}", c.label(), fmt_secs(c.start as f32), fmt_secs(c.end as f32), if c.hit { " · hit on entry" } else { "" })))
                .pressable()
                .child(icon_ph(icon, color, icon_px::XS))
                .when(c.hit, |d| d.child(icon_ph(ph::ARROW_DOWN, color, icon_px::XS)))
                .child(div().truncate().child(format!("{} · {}", c.style.label(), c.bpm.round())))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.music.cue = if this.music.cue == Some(i) { None } else { Some(i) };
                    cx.notify();
                }))
        });
        let cue_lane = div().flex().gap(px(space::SM)).child(head(label(glyphs::MUSIC, "Cues".into()).into_any_element())).child(lane().h(px(34.)).children(cues));

        // Energy curve (0-10), bucketed.
        let buckets = 160usize;
        let e = &score.energy.0;
        let bars = (0..buckets).map(|b| {
            let lo = b * e.len() / buckets;
            let hi = ((b + 1) * e.len() / buckets).max(lo + 1).min(e.len().max(1));
            let v = if e.is_empty() { 0.0 } else { e[lo.min(e.len() - 1)..hi].iter().copied().fold(0f32, f32::max) };
            div().flex_1().h(relative((v / 10.0).clamp(0.04, 1.0))).rounded_t(px(1.)).bg(Tokens::accent()).opacity(0.55)
        });
        let energy = div().flex().gap(px(space::SM)).child(head(label(ph::CHART_LINE_UP, "Energy".into()).into_any_element())).child(div().flex().items_end().gap(px(1.)).flex_1().h(px(36.)).children(bars));

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
                                c.notes.iter().map(|n| div().absolute().left(x(n.t)).w(px(3.)).top(px(5.)).bottom(px(5.)).rounded(px(1.)).bg(Tokens::accent()).into_any_element()).collect::<Vec<_>>()
                            } else if c.notes.is_empty() {
                                vec![]
                            } else {
                                let style = score.cues.get(c.cue).map(|q| q.style).unwrap_or(Style::Mystery);
                                let hue = style_look(style).0;
                                let end = c.notes.iter().map(|n| n.t + n.dur).fold(c.start, f64::max).min(c.end);
                                vec![div()
                                    .absolute()
                                    .left(x(c.start))
                                    .w(w(c.start, end))
                                    .top(px(4.))
                                    .bottom(px(4.))
                                    .rounded(px(radius::XS))
                                    .bg(Tokens::hue_soft(hue))
                                    .border_l_2()
                                    .border_color(Tokens::hue(hue))
                                    .px(px(4.))
                                    .text_size(px(size::XS))
                                    .text_color(Tokens::text_2())
                                    .truncate()
                                    .child(instrument_name(c.instrument, orchestra))
                                    .into_any_element()]
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let key = role.key();
            let name = role.label();
            let mixer = move |this: &mut StudioPage, cx: &mut Context<StudioPage>, f: fn(&mut Strip)| {
                this.music_edit(cx, |s| f(s.mixer.entry(role).or_insert_with(Strip::default)));
            };
            let header = div()
                .flex()
                .items_center()
                .gap(px(2.))
                .w_full()
                .child(div().id(SharedString::from(format!("lane-{key}"))).flex_1().min_w_0().tooltip(tip(name)).child(label(role_icon(role), name.to_string()).when(!audible, |d| d.opacity(0.6))))
                .child(
                    icon_toggle(SharedString::from(format!("mute-{key}")), ph::SPEAKER_SLASH, strip.mute, Tokens::warning(), format!("Unmute {name}"), format!("Mute {name}"))
                        .size(icon_px::SM)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| mixer(this, cx, |st| st.mute = !st.mute))),
                )
                .child(
                    icon_toggle(SharedString::from(format!("solo-{key}")), ph::HEADPHONES, strip.solo, Tokens::accent(), format!("Stop soloing {name}"), format!("Solo {name}"))
                        .size(icon_px::SM)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| mixer(this, cx, |st| st.solo = !st.solo))),
                )
                .child(
                    icon_button(SharedString::from(format!("down-{key}")), ph::MINUS, format!("{name} quieter (−3 dB)"))
                        .small()
                        .when(strip.gain_db <= -24.0, |b| b.disabled("At the lowest level"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| mixer(this, cx, |st| st.gain_db = (st.gain_db - 3.0).max(-24.0)))),
                )
                .child(div().w(px(40.)).flex_none().text_center().text_size(px(size::XS)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_2()).child(format!("{:+.0} dB", strip.gain_db)))
                .child(
                    icon_button(SharedString::from(format!("up-{key}")), ph::PLUS, format!("{name} louder (+3 dB)"))
                        .small()
                        .when(strip.gain_db >= 9.0, |b| b.disabled("At the highest level"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| mixer(this, cx, |st| st.gain_db = (st.gain_db + 3.0).min(9.0)))),
                );
            div().flex().items_center().gap(px(space::SM)).child(head(header.into_any_element())).child(lane().when(!audible, |d| d.opacity(0.4)).children(clips))
        });

        let summary = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .pb(px(space::XS))
            .child(div().flex_1().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Timeline"))
            .child(stat_pill(ph::CLOCK, fmt_secs(score.duration as f32)))
            .child(stat_pill(glyphs::MUSIC, format!("{} cues", score.cues.len())))
            .child(stat_pill(if orchestra { ph::MUSIC_NOTES } else { ph::WAVEFORM }, if orchestra { "Orchestra" } else { "Built-in synth" }));
        let mut card = card()
            .gap(px(6.))
            .child(summary)
            .child(ruler)
            .child(cue_lane)
            .child(energy)
            .child(div().h(px(1.)).my(px(space::XS)).bg(Tokens::border()))
            .children(tracks);
        if let Some(c) = selected.and_then(|i| score.cues.get(i)).cloned() {
            card = card.child(self.cue_panel(&c, cx));
        } else {
            card = card.child(muted("Select a cue to change its genre or try a new variation.").mt(px(space::XS)));
        }
        card.into_any_element()
    }

    fn cue_panel(&mut self, c: &mv_score::Cue, cx: &mut Context<Self>) -> AnyElement {
        let key = c.pages.0;
        let forced = self.music.settings.styles.get(&key).copied();
        let variation = self.music.settings.variations.get(&key).copied().unwrap_or(0);
        let start = c.start;
        let (hue, icon) = style_look(c.style);
        let mut tags = vec![];
        if c.hit {
            tags.push("Hit on entry");
        }
        if c.drop_before {
            tags.push("Drop before");
        }
        if c.riser_out {
            tags.push("Riser out");
        }
        if c.record_stop {
            tags.push("Record stop in");
        }
        div()
            .mt(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(Tokens::border())
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().flex().flex_none().items_center().justify_center().size(px(28.)).rounded(px(radius::MD)).bg(Tokens::hue_soft(hue)).child(ph_icon(icon, Weight::Fill, Tokens::hue(hue), icon_px::SM)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format!("Cue {} · {}", c.index + 1, c.label())))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!(
                                "{}–{} · pages {}–{} · energy {:.1}{}",
                                fmt_secs(c.start as f32),
                                fmt_secs(c.end as f32),
                                c.pages.0 + 1,
                                c.pages.1 + 1,
                                c.energy,
                                if variation > 0 { format!(" · variation {}", variation + 1) } else { String::new() }
                            ))),
                    )
                    .children(tags.into_iter().map(|t| tag(t, Tone::Neutral)))
                    .child(btn("cue-play", "Play from here", ButtonKind::Secondary).small().icon(ph::PLAY).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.play_from(start, cx))))
                    .child(btn("cue-vary", "New variation", ButtonKind::Ghost).small().icon(ph::SHUFFLE).tooltip("Same genre, different notes").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.music_edit(cx, |s| {
                            s.variations.insert(key, variation + 1);
                        })
                    })))
                    .child(icon_button("cue-close", ph::X, "Close").small().kbd("escape").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.music.cue = None;
                        cx.notify();
                    }))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(div().flex().items_center().gap(px(6.)).mr(px(space::XS)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(icon_ph(ph::MASK_HAPPY, Tokens::text_3(), icon_px::SM)).child("Genre"))
                    .child(filter_chip("cue-auto", "Auto", forced.is_none()).icon(ph::MAGIC_WAND).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.music_edit(cx, |s| {
                            s.styles.remove(&key);
                        })
                    })))
                    .children(Style::ALL.map(|st| {
                        filter_chip(SharedString::from(format!("cue-style-{}", st.label())), st.label(), forced == Some(st)).icon(style_look(st).1).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.music_edit(cx, |s| {
                                s.styles.insert(key, st);
                            })
                        }))
                    })),
            )
            .child(div().flex().items_center().gap(px(6.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(icon_ph(ph::INFO, Tokens::text_3(), icon_px::XS)).child("Changes apply when you press Update score."))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{style_look, Ph, Style};

    #[test]
    fn every_cue_style_has_an_icon() {
        let icons: Vec<Ph> = Style::ALL.iter().map(|s| style_look(*s).1).collect();
        for (i, a) in icons.iter().enumerate() {
            assert!(!icons[i + 1..].contains(a), "style icons are unique");
        }
    }
}
