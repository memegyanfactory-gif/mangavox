//! The Generate sheet: the questions the whole pipeline needs before it starts (narrator voice,
//! Landscape or Reels, subtitles, music), prefilled from the last answers and remembered for the
//! next time. Confirming applies the answers to the chapters and hands them to [`pipeline`].
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::{Prefs, Step};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::Interactive as _;
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, glyphs, icon_button, icon_ph, icon_tag, inline_alert, muted, ph, ph_icon, segmented, selected_badge, size, spinner, switch, AlertKind, ButtonKind, Ph, Route, Tokens, Tone, Weight};
use serde::{Deserialize, Serialize};

use crate::data::ChapterItem;
use crate::page::{plural, StudioPage};
use crate::pipeline;
use crate::setup;
use crate::{audio, widgets::phase_icon};

/// The answers Generate asks for, remembered in `studio/generate.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Answers {
    /// Narrator for the chapters (`None`: the default voice from Settings → Audio).
    pub voice: Option<String>,
    /// `mv_recap::FORMAT_LANDSCAPE` or `FORMAT_REELS`.
    pub format: String,
    /// Burn subtitles into the video.
    pub captions: bool,
    /// Caption template id (`video::styles`); `None` keeps the Studio default.
    pub subtitle_style: Option<String>,
    /// Where subtitles sit (`restyle::PLACES` index; 0 = the template's own place).
    pub subtitle_place: usize,
    /// Subtitle size multiplier.
    pub subtitle_size: f32,
    /// Compose a score for every chapter.
    pub music: bool,
}

impl Default for Answers {
    fn default() -> Self {
        Self { voice: None, format: mv_recap::FORMAT_LANDSCAPE.into(), captions: true, subtitle_style: None, subtitle_place: 0, subtitle_size: 1.0, music: true }
    }
}

impl Answers {
    fn path() -> std::path::PathBuf {
        mv_recap::studio_root().join("generate.json")
    }

    /// The saved answers; the format follows the Video step's setting when nothing was saved.
    pub fn load(prefs: &Prefs) -> Answers {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| Answers { format: prefs.format.clone(), ..Default::default() })
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(p, b);
        }
    }
}

/// Steps Generate can make again on purpose ("Redo from"), as the menus name them. Every step
/// after the one picked is made again too, since it is built from it.
pub(crate) const REDO: [(Step, &str, Ph); 5] = [
    (Step::Read, "Read the pages", glyphs::READ),
    (Step::Story, "Write the story", glyphs::STORY),
    (Step::Audio, "Narrate (audio)", ph::MICROPHONE_STAGE),
    (Step::Music, "Music (new take)", glyphs::MUSIC),
    (Step::Video, "Video", glyphs::VIDEO),
];

/// "Read, Story, Audio, Music and Video": what a redo from `from` makes again.
pub(crate) fn redo_summary(from: Step) -> String {
    let names: Vec<&str> = REDO.iter().filter(|(s, _, _)| s.index() >= from.index()).map(|(s, _, _)| s.label()).collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => last.to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod redo_tests {
    use super::redo_summary;
    use mv_recap::Step;

    #[test]
    fn a_redo_names_every_step_it_makes_again() {
        assert_eq!(redo_summary(Step::Audio), "Audio, Music and Video");
        assert_eq!(redo_summary(Step::Video), "Video");
    }
}

/// The open Generate sheet.
pub(crate) struct Sheet {
    pub chapters: Vec<ChapterItem>,
    pub answers: Answers,
    /// Make this step and every later one again, even where done.
    pub redo: Option<Step>,
    pub lang: &'static str,
    /// Every voice is listed (else the ready ones and the picked one).
    pub all_voices: bool,
}

const FORMATS: [(&str, &str, &str, Ph); 2] = [
    (mv_recap::FORMAT_LANDSCAPE, "Landscape", "16:9 · one video for YouTube", ph::MONITOR),
    (mv_recap::FORMAT_REELS, "Reels", "9:16 · cut into ~1 minute parts", ph::DEVICE_MOBILE),
];

impl StudioPage {
    /// Opens the questions for `chapters` (the dashboard's Generate button).
    /// `redo`: make that step and the later ones again, even where they are done.
    pub(crate) fn open_generate(&mut self, chapters: Vec<ChapterItem>, redo: Option<Step>, cx: &mut Context<Self>) {
        if chapters.is_empty() {
            toast::show(cx, Toast::info("Pick at least one chapter first"));
            return;
        }
        let mut answers = Answers::load(&self.prefs);
        // Chapters that already share a narrator keep it; otherwise the last answer, else the default.
        let mut own: Vec<String> = chapters.iter().map(|c| mv_studio::voices::voice_for_chapter(&c.id)).collect();
        own.dedup();
        let shared = (own.len() == 1 && chapters.iter().any(|c| mv_studio::voices::chapter_voice(&c.id).is_some())).then(|| own[0].clone());
        let voice = shared.or(answers.voice.clone()).unwrap_or_else(|| self.prefs.voice.clone().unwrap_or_default());
        // Subtitles follow the caption setting the Video step saved (the Studio default).
        let subs = crate::video::default_setting();
        answers.captions = subs.enabled;
        answers.subtitle_place = crate::restyle::place_index(subs.pos);
        answers.subtitle_size = subs.size;
        answers.subtitle_style = Some(subs.style);
        let lang = crate::page::lang_of(&voice);
        self.sheet = Some(Sheet { chapters, answers: Answers { voice: Some(voice), ..answers }, redo, lang, all_voices: false });
        cx.notify();
    }

    /// Applies the answers to the chapters and queues them.
    fn confirm_generate(&mut self, cx: &mut Context<Self>) {
        let Some(sheet) = self.sheet.take() else { return };
        let a = sheet.answers.clone();
        a.save();
        if self.prefs.format != a.format {
            self.prefs.format = a.format.clone();
            let f = a.format.clone();
            Prefs::update(|p| p.format = f);
        }
        self.gen_answers = Some(a.clone());
        let (n, widened, already) = self.generate_chapters(&sheet.chapters, &a, sheet.redo, true, cx);
        // A redo is a one-off: the next Generate only fills gaps again.
        self.dash_redo = None;
        let msg = match (n + widened, already) {
            (0, k) => format!("{k} chapter{} already in the queue", plural(k)),
            (m, 0) => format!("Generating {m} chapter{}", plural(m)),
            (m, k) => format!("Generating {m} chapter{} · {k} already queued", plural(m)),
        };
        toast::show(cx, Toast::success(msg).icon(ph::ROCKET_LAUNCH));
        self.side_open = true;
        self.side = crate::page::Side::Activity;
        cx.notify();
    }

    /// Applies `a` (voice, music) to `chapters` and adds them to the queue. Returns the counts
    /// [`pipeline::enqueue`] reports.
    /// `redo`: make that step and every later one again for all of them.
    pub(crate) fn generate_chapters(&mut self, chapters: &[ChapterItem], a: &Answers, redo: Option<Step>, start: bool, cx: &mut Context<Self>) -> (usize, usize, usize) {
        let default = self.prefs.voice.clone().unwrap_or_default();
        // A made video whose subtitles or music change is made again (otherwise it is "done").
        let subs_before: Vec<crate::video::SubtitleSetting> = chapters.iter().map(crate::video::subtitle_setting_for).collect();
        let mut remake: Vec<bool> = vec![false; chapters.len()];
        for (i, c) in chapters.iter().enumerate() {
            if let Some(v) = &a.voice {
                if let Err(e) = mv_studio::voices::set_chapter_voice(&c.id, (*v != default).then_some(v.as_str())) {
                    self.fail(format!("Could not save the voice for {}: {e}", c.label()));
                }
            }
            let r = crate::page::chref(c);
            let mut m = mv_score::load_settings(&r);
            if m.enabled != a.music {
                m.enabled = a.music;
                let _ = mv_score::save_settings(&r, &m);
                remake[i] = true;
            }
        }
        apply_subtitles(chapters, a);
        self.sync_pipeline(cx);
        if let Some(r) = redo {
            return pipeline::enqueue(chapters, Step::Video, Some(r), &a.format, Some(a.captions), start);
        }
        let (mut redo, mut rest) = (vec![], vec![]);
        for (i, c) in chapters.iter().enumerate() {
            let made = self.statuses.get(&c.id).is_some_and(|s| s.video.is_some());
            if made && (remake[i] || crate::video::subtitle_setting_for(c) != subs_before[i]) {
                redo.push(c.clone());
            } else {
                rest.push(c.clone());
            }
        }
        let mut n = pipeline::enqueue(&rest, Step::Video, None, &a.format, Some(a.captions), start);
        if !redo.is_empty() {
            let m = pipeline::enqueue(&redo, Step::Video, Some(Step::Video), &a.format, Some(a.captions), start);
            n = (n.0 + m.0, n.1 + m.1, n.2 + m.2);
        }
        n
    }

    /// Hands the pipeline what it needs from the page (registry, model, setup, speed).
    pub(crate) fn sync_pipeline(&self, cx: &mut Context<Self>) {
        pipeline::set_context(pipeline::Ctx {
            reg: ui_assistant::registry(cx),
            model: self.model(cx),
            env: self.setup.read(cx).env.clone(),
            speed: self.prefs.speed,
            resolution: self.prefs.resolution.clone(),
            styles: self.prefs.styles.clone(),
        });
    }

    fn preview_voice(&mut self, voice: &'static str, cx: &mut Context<Self>) {
        let Some(env) = self.setup.read(cx).env.clone() else { return };
        let (Ok(py), Ok(layout)) = (env.python.clone(), env.scripts.clone()) else { return };
        let speed = self.prefs.speed;
        if audio::now_playing().is_some_and(|p| p.to_string_lossy().contains(&mv_studio::tts::safe_voice_name(voice))) {
            audio::stop();
            return cx.notify();
        }
        self.previewing = Some(voice.to_string());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let samples = mv_studio::tts::default_samples_dir();
                mv_studio::tts::voice_preview(&py, &layout, &samples, &std::env::temp_dir(), voice, speed, None).await
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.previewing = None;
                match res {
                    Ok(Ok(p)) => audio::play(&p.path),
                    Ok(Err(e)) => toast::show(cx, Toast::error(format!("The sample failed: {e}"))),
                    Err(e) => toast::show(cx, Toast::error(format!("The sample failed: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Subtitles question: on/off and the caption template (the catalogue `video.rs` loads from
    /// `python/caption_styles.json`: favourites and the current pick first). Size, place and
    /// colours stay on the Video step's caption studio.
    fn subtitle_slot(&self, on: bool, style: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let current = style.map(str::to_string).unwrap_or_else(|| crate::video::catalog().default.clone());
        let favs = crate::video::favorites();
        let all = crate::video::styles();
        // Favourites, the current one, the default, then the catalogue order: 10 in all.
        let mut ids: Vec<String> = vec![];
        for id in favs.iter().cloned().chain([current.clone(), crate::video::catalog().default.clone()]).chain(all.iter().map(|s| s.id.clone())) {
            if !ids.contains(&id) && all.iter().any(|s| s.id == id) {
                ids.push(id);
            }
            if ids.len() >= 10 {
                break;
            }
        }
        let chips = div().flex().flex_wrap().gap(px(6.)).children(ids.iter().enumerate().map(|(i, id)| {
            let st = crate::video::find_style(id);
            let picked = *id == current;
            let id = id.clone();
            mv_ui::filter_chip(("gen-sub-style", i), st.label.clone(), picked).when(favs.contains(&id), |c| c.icon(ph::STAR)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if let Some(s) = this.sheet.as_mut() {
                    s.answers.subtitle_style = Some(id.clone());
                    s.answers.captions = true;
                }
                cx.notify();
            }))
        }));
        let summary = crate::video::find_style(&current).summary();
        let (place, sz) = self.sheet.as_ref().map(|s| (s.answers.subtitle_place, s.answers.subtitle_size)).unwrap_or((0, 1.0));
        let place_seg = segmented("gen-sub-place", crate::restyle::PLACES.iter().map(|p| (None, p.0)), place.min(3)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            if let Some(s) = this.sheet.as_mut() {
                s.answers.subtitle_place = *ix;
            }
            cx.notify();
        }));
        let size_seg = segmented("gen-sub-size", crate::restyle::SIZES.iter().map(|s| (None, s.0)), crate::restyle::size_index(sz)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            if let Some(s) = this.sheet.as_mut() {
                s.answers.subtitle_size = crate::restyle::SIZES[*ix].1;
            }
            cx.notify();
        }));
        let tune = div().flex().flex_wrap().items_center().gap(px(space::SM)).child(muted("Place")).child(place_seg).child(muted("Size")).child(size_seg);
        question(
            ph::SUBTITLES,
            "Subtitles",
            "Words appear on screen in time with the narration. Drag them anywhere or change colours on the Video step.",
            div()
                .flex()
                .flex_col()
                .gap(px(space::SM))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(switch("gen-captions", on).on_click(cx.listener(|this, on: &bool, _, cx| {
                            if let Some(s) = this.sheet.as_mut() {
                                s.answers.captions = *on;
                            }
                            cx.notify();
                        })))
                        .child(muted(if on { format!("Burned into the video · {summary}") } else { "No subtitles".to_string() })),
                )
                .when(on, |d| d.child(chips).child(tune))
                .into_any_element(),
        )
    }

    /// The sheet over the Studio page (backdrop + centred card).
    pub(crate) fn generate_sheet(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sheet = self.sheet.as_ref()?;
        let chapters = sheet.chapters.clone();
        let sheet_chapters = chapters.clone();
        let a = sheet.answers.clone();
        let redo = sheet.redo;
        let lang = sheet.lang;
        let env = self.setup.read(cx).env.clone();
        let engines = env.as_ref().map(|e| e.engines.clone()).unwrap_or_default();
        let checking = env.is_none();
        let n = chapters.len();
        let manga: Vec<String> = {
            let mut m: Vec<String> = chapters.iter().map(|c| c.manga_title.clone()).collect();
            m.dedup();
            m
        };
        let numbers = chapters.iter().map(|c| pipeline::short_label(c.number, c.title.as_deref())).collect::<Vec<_>>();
        let numbers = if numbers.len() > 12 { format!("{} … {}", numbers[..10].join(", "), numbers[numbers.len() - 1]) } else { numbers.join(", ") };

        // What will run: per step, how many chapters still need it (the rest are skipped).
        let need = |s: Step| {
            chapters
                .iter()
                .filter(|c| {
                    let st = self.statuses.get(&c.id).cloned().unwrap_or_default();
                    let wanted = a.voice.clone().unwrap_or_default();
                    let changed = pipeline::voice_changed(st.audio_files, pipeline::narrated_voice(&c.output_dir).as_deref(), &wanted);
                    let f = pipeline::Facts { narration_complete: st.audio_files > 0, voice_changed: changed, music_on: a.music, music_fresh: st.music, status: st };
                    // A new narrator re-makes the audio, and the music and video built on it.
                    // A redo makes its step and every later one again.
                    let redone = redo.is_some_and(|r| s.index() >= r.index());
                    pipeline::plan(s, &f, &a.format, redone || (changed && s.index() > Step::Audio.index())).is_ok()
                })
                .count()
        };
        let steps: Vec<(Step, usize)> = [Step::Read, Step::Story, Step::Audio, Step::Music, Step::Video].into_iter().map(|s| (s, need(s))).collect();
        let needs_ai = steps.iter().any(|(s, k)| matches!(s, Step::Read | Step::Story) && *k > 0);
        let model = self.model(cx);

        // Q1: narrator.
        let voice = a.voice.clone().unwrap_or_default();
        let playing = audio::now_playing();
        // Ready voices first; the rest stay one click away so the other questions stay in view.
        let mut voices = setup::voices(lang);
        voices.sort_by_key(|v| !setup::voice_ready(&engines, v).usable());
        let total_voices = voices.len();
        let ready_count = voices.iter().filter(|v| setup::voice_ready(&engines, v).usable()).count();
        let all_voices = sheet.all_voices;
        let limit = if all_voices { usize::MAX } else { ready_count.clamp(4, 8) };
        let voices: Vec<_> = voices.into_iter().enumerate().filter(|(i, v)| *i < limit || v.id == voice).map(|(_, v)| v).collect();
        let hidden = total_voices - voices.len();
        let voice_ok = !checking && mv_studio::tts::find_voice(&voice).is_some_and(|v| setup::voice_ready(&engines, v).usable());
        let voice_rows = div().flex().flex_wrap().gap(px(space::SM)).children(voices.into_iter().enumerate().map(|(i, v)| {
            let on = v.id == voice;
            let ready = setup::voice_ready(&engines, v);
            let usable = !checking && ready.usable();
            let id = v.id;
            let previewing = self.previewing.as_deref() == Some(id);
            let sample_playing = playing.as_ref().is_some_and(|p| p.to_string_lossy().contains(&mv_studio::tts::safe_voice_name(id)));
            let status = match ready {
                _ if checking => "Checking…".to_string(),
                setup::VoiceReady::Ready(e) => e.to_string(),
                setup::VoiceReady::Fallback(e) => format!("{e} stands in"),
                setup::VoiceReady::Missing => "Not downloaded".to_string(),
            };
            div()
                .id(("gen-voice", i))
                .flex()
                .items_center()
                .gap(px(space::SM))
                .w(px(232.))
                .px(px(space::SM))
                .py(px(6.))
                .rounded(px(radius::MD))
                .border_1()
                .role(Role::RadioButton)
                .aria_selected(on)
                .map(|d| if on { d.border_color(Tokens::accent()).bg(Tokens::accent_soft()) } else { d.border_color(Tokens::border()) })
                .when(!usable, |d| d.opacity(0.55))
                .when(usable && !on, |d| d.hover(|s| s.bg(Tokens::hover()).border_color(Tokens::border_strong())).pressable())
                .child(match (previewing, usable) {
                    (true, _) => div().flex().flex_none().items_center().justify_center().size(px(28.)).child(spinner(("gen-voice-spin", i), Tokens::text_1(), icon_px::SM)).into_any_element(),
                    (false, true) => icon_button(("gen-sample", i), if sample_playing { ph::STOP } else { ph::PLAY }, if sample_playing { "Stop the sample" } else { "Preview this voice" })
                        .small()
                        .selected(sample_playing)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.preview_voice(id, cx);
                        }))
                        .into_any_element(),
                    (false, false) => icon_button(("gen-sample", i), ph::PLAY, "Preview this voice").small().disabled("Download this voice in Settings → Audio first").into_any_element(),
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(v.label))
                        .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(status)),
                )
                .when(on, |d| d.child(selected_badge()))
                .when(usable, |d| {
                    d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(s) = this.sheet.as_mut() {
                            s.answers.voice = Some(id.to_string());
                        }
                        cx.notify();
                    }))
                })
        }));
        let more_voices = (hidden > 0 || all_voices).then(|| {
            btn("gen-more-voices", if all_voices { "Show fewer voices".to_string() } else { format!("Show all {total_voices} voices") }, ButtonKind::Ghost)
                .small()
                .icon(if all_voices { ph::CARET_UP } else { ph::CARET_DOWN })
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(s) = this.sheet.as_mut() {
                        s.all_voices = !s.all_voices;
                    }
                    cx.notify();
                }))
        });
        let langs = segmented("gen-lang", [(Some(glyphs::LANGUAGE), "English"), (None, "Hindi")], if lang == "hi" { 1 } else { 0 }).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            if let Some(s) = this.sheet.as_mut() {
                s.lang = if *ix == 1 { "hi" } else { "en" };
            }
            cx.notify();
        }));

        // Q2: format, as two big choices.
        let formats = div().flex().flex_wrap().gap(px(space::SM)).children(FORMATS.iter().enumerate().map(|(i, &(id, label, desc, icon))| {
            let on = a.format == id;
            div()
                .id(("gen-format", i))
                .flex()
                .items_center()
                .gap(px(space::MD))
                .flex_1()
                .min_w(px(200.))
                .p(px(space::MD))
                .rounded(px(radius::LG))
                .border_1()
                .role(Role::RadioButton)
                .aria_selected(on)
                .map(|d| if on { d.border_color(Tokens::accent()).bg(Tokens::accent_soft()) } else { d.border_color(Tokens::border()).hover(|s| s.bg(Tokens::hover()).border_color(Tokens::border_strong())) })
                .pressable()
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(36.))
                        .rounded(px(radius::MD))
                        .bg(if on { Tokens::accent_a(0.18) } else { Tokens::surface_2() })
                        .child(ph_icon(icon, if on { Weight::Fill } else { Weight::Regular }, if on { Tokens::accent() } else { Tokens::text_2() }, icon_px::LG)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(label))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(desc)),
                )
                .when(on, |d| d.child(selected_badge()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(s) = this.sheet.as_mut() {
                        s.answers.format = id.to_string();
                    }
                    cx.notify();
                }))
        }));

        // Redo: make a step (and the ones after it) again, even where done.
        let redo_items: Vec<(Option<Step>, &str)> = std::iter::once((None, "Nothing")).chain(REDO.iter().map(|(s, _, _)| (Some(*s), s.label()))).collect();
        let redo_pick = redo_items.iter().position(|(s, _)| *s == redo).unwrap_or(0);
        let redo_steps: Vec<Option<Step>> = redo_items.iter().map(|(s, _)| *s).collect();
        let redo_seg = segmented("gen-redo", redo_items.iter().map(|(_, l)| (None, *l)), redo_pick).small().on_change(cx.listener(move |this, ix: &usize, _, cx| {
            let r = redo_steps[*ix];
            if let Some(s) = this.sheet.as_mut() {
                s.redo = r;
            }
            this.dash_redo = r;
            cx.notify();
        }));
        let redo_q = div().flex().flex_col().gap(px(space::SM)).child(div().flex().child(redo_seg)).child(muted(match redo {
            None => "Only the steps a chapter is missing are made.".to_string(),
            Some(r) => format!("{} are made again for every chapter, even where done.", redo_summary(r)),
        }));

        // Q4: music.
        let music = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(switch("gen-music", a.music).on_click(cx.listener(|this, on: &bool, _, cx| {
                if let Some(s) = this.sheet.as_mut() {
                    s.answers.music = *on;
                }
                cx.notify();
            })))
            .child(muted(if a.music { "A score is composed for each chapter and mixed under the voice" } else { "Narration only" }));

        // What would stop it, said next to the button (never a silent disabled button).
        let blocker: Option<(String, Option<(&'static str, crate::issues::Fix)>)> = if checking {
            Some(("Checking your setup…".into(), None))
        } else if env.as_ref().is_some_and(|e| !e.runnable()) {
            Some(("Studio needs Python and the pipeline scripts.".into(), Some(("Open Setup", crate::issues::Fix::OpenSetup))))
        } else if !voice_ok {
            Some(("Pick a narrator that is ready, or download this one.".into(), Some(("Open Audio settings", crate::issues::Fix::AudioSettings))))
        } else {
            None
        };
        let warn_model = needs_ai && model.is_none();
        let plan_row = div().flex().flex_wrap().items_center().gap(px(6.)).children(steps.iter().map(|&(s, k)| {
            let (tone, text) = if s == Step::Music && !a.music {
                (Tone::Neutral, "Music off".to_string())
            } else if k == 0 {
                (Tone::Success, format!("{} done", s.label()))
            } else {
                (Tone::Accent, format!("{} · {k}", s.label()))
            };
            icon_tag(phase_icon(crate::page::Phase::of(s)), text, tone)
        }));
        let skipped: usize = steps.iter().map(|(_, k)| n - k).sum();
        let revoice = {
            let wanted = a.voice.clone().unwrap_or_default();
            chapters
                .iter()
                .filter(|c| pipeline::voice_changed(self.statuses.get(&c.id).map(|s| s.audio_files).unwrap_or(0), pipeline::narrated_voice(&c.output_dir).as_deref(), &wanted))
                .count()
        };
        let card = div()
            .id("gen-sheet")
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(760.))
            .max_h(relative(0.92))
            .rounded(px(radius::XL))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border_strong())
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // Header.
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::MD))
                    .p(px(space::LG))
                    .border_b_1()
                    .border_color(Tokens::border())
                    .child(div().flex().flex_none().items_center().justify_center().size(px(40.)).rounded(px(radius::LG)).bg(Tokens::accent_soft()).child(ph_icon(ph::ROCKET_LAUNCH, Weight::Duotone, Tokens::accent(), icon_px::XL)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(match redo {
                                Some(r) => format!("Redo {} from {}", crate::dashboard::chapters_phrase(&sheet_chapters), r.label()),
                                None => format!("Generate {}", crate::dashboard::chapters_phrase(&sheet_chapters)),
                            }))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("{} · {numbers}", manga.join(", "))))
                            .child(div().pt(px(space::SM)).child(plan_row))
                            .child(muted(if skipped > 0 {
                                format!("Each chapter goes Read → Transcript → Story → Audio → Music → Video. Steps already made are skipped ({skipped} here).")
                            } else {
                                "Each chapter goes Read → Transcript → Story → Audio → Music → Video, one after another.".to_string()
                            }))
                            .when(revoice > 0, |d| {
                                d.child(div().text_size(px(size::SM)).text_color(Tokens::warning()).child(format!(
                                    "{revoice} chapter{} will be narrated again with the new voice; its music and video are made again too.",
                                    plural(revoice)
                                )))
                            }),
                    )
                    .child(icon_button("gen-close", ph::X, "Close").kbd("escape").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.sheet = None;
                        cx.notify();
                    }))),
            )
            // Questions.
            .child(
                div()
                    .id("gen-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap(px(space::LG))
                    .p(px(space::LG))
                    .child(question(ph::MICROPHONE_STAGE, "Narrator voice", "Who reads the recap. Press play to hear a voice first.", div().flex().flex_col().gap(px(space::SM)).child(div().flex().child(langs)).child(voice_rows).children(more_voices).into_any_element()))
                    .child(question(ph::ARROWS_CLOCKWISE, "Make again", "Redo a step even where it is done; every step after it is made again too.", redo_q.into_any_element()))
                    .child(question(ph::FRAME_CORNERS, "Format", "Landscape makes one video; Reels makes upright parts for Shorts, Reels and TikTok.", formats.into_any_element()))
                    .child(self.subtitle_slot(a.captions, a.subtitle_style.as_deref(), cx))
                    .child(question(glyphs::MUSIC, "Music", "An original score that follows each scene.", music.into_any_element()))
                    .when(needs_ai, |d| {
                        d.child(question(
                            glyphs::MODEL,
                            "AI model",
                            "Reads the pages and writes the story.",
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(space::SM))
                                .child(div().flex().child(self.model_button.clone()))
                                .when(warn_model, |d| d.child(inline_alert(AlertKind::Warning, "Pick a model to read and write the story", "Without one, those chapters stop at Read with an issue to fix.")))
                                .into_any_element(),
                        ))
                    }),
            )
            // Footer.
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .px(px(space::LG))
                    .py(px(space::MD))
                    .border_t_1()
                    .border_color(Tokens::border())
                    .bg(Tokens::surface_2())
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(200.))
                            .items_center()
                            .gap(px(space::SM))
                            .text_size(px(size::SM))
                            .map(|d| match &blocker {
                                Some((why, _)) => d.text_color(Tokens::warning()).child(icon_ph(ph::WARNING, Tokens::warning(), icon_px::SM)).child(div().min_w_0().child(why.clone())),
                                None => d.text_color(Tokens::text_3()).child(icon_ph(ph::CLOCK_COUNTER_CLOCKWISE, Tokens::text_3(), icon_px::SM)).child(div().min_w_0().child("Your answers are remembered for next time")),
                            }),
                    )
                    .when_some(blocker.as_ref().and_then(|b| b.1.clone()), |d, (label, fix)| {
                        d.child(btn("gen-fix", label, ButtonKind::Secondary).small().on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.sheet = None;
                            this.apply_fix(fix.clone(), window, cx);
                        })))
                    })
                    .child(btn("gen-cancel", "Not now", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.sheet = None;
                        cx.notify();
                    })))
                    .child(
                        btn("gen-go", if redo.is_some() { format!("Redo {}", crate::dashboard::chapters_phrase(&sheet_chapters)) } else { format!("Generate {}", crate::dashboard::chapters_phrase(&sheet_chapters)) }, ButtonKind::Primary)
                            .icon(ph::ROCKET_LAUNCH)
                            .when_some(blocker.as_ref().map(|b| b.0.clone()), |b, why| b.disabled(why))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.confirm_generate(cx))),
                    ),
            );
        Some(
            div()
                .id("gen-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(space::XL))
                .bg(Tokens::canvas().alpha(0.78))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.sheet = None;
                        cx.notify();
                    }),
                )
                .child(card)
                .into_any_element(),
        )
    }

    pub(crate) fn open_audio_settings(window: &mut Window, cx: &mut App) {
        mv_ui::navigate(Route::Settings { tab: Some("audio".into()) }, window, cx);
    }
}

/// One question: icon, title, why it is asked, and its control.
pub(crate) fn question(icon: Ph, title: &'static str, why: &'static str, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .gap(px(space::MD))
        .child(div().flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(radius::MD)).bg(Tokens::surface_2()).child(icon_ph(icon, Tokens::text_2(), icon_px::MD)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(space::SM))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title))
                        .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(why)),
                )
                .child(control),
        )
        .into_any_element()
}

/// Saves the subtitle answer: the Studio default, and the chapters that have their own caption
/// style keep it but follow the on/off and template picked here.
fn apply_subtitles(chapters: &[ChapterItem], a: &Answers) {
    let mut d = crate::video::default_setting();
    let style = a.subtitle_style.clone();
    let set = |s: &mut crate::video::SubtitleSetting| {
        s.enabled = a.captions;
        if let Some(st) = &style {
            s.style = st.clone();
        }
        crate::restyle::set_place(s, a.subtitle_place);
        crate::restyle::set_size(s, a.subtitle_size);
    };
    set(&mut d);
    crate::video::save_default(&d);
    for c in chapters {
        if let Some(mut own) = crate::video::chapter_setting(&c.output_dir) {
            set(&mut own);
            crate::video::save_chapter_setting(&c.output_dir, Some(&own));
        }
    }
}
