//! The middle column: one header per phase (what it does + its main action) over a
//! virtual list of the chapter's pages, segments or settings.
use std::path::{Path, PathBuf};

use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::{PageRead, STYLES};
use mv_ui::nav::PlayRequest;
use mv_ui::{button, chip, empty_state, icon, size, ButtonKind, Route, Tokens};

use crate::page::{plural, transcript_text, Focus, Phase, StudioPage};
use crate::run::RunKind;
use crate::service::{self, NarrationRequest};
use crate::setup::{self, VoiceReady};
use crate::{audio, run};

const SPEEDS: [f64; 4] = [0.9, 1.0, 1.1, 1.25];
const RESOLUTIONS: [&str; 2] = ["720p", "1080p"];
const PAGE_MAX_W: f32 = 760.;

impl StudioPage {
    pub(crate) fn center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(f) = &self.focus else {
            let msg = if self.focus_id.is_some() { ("Opening chapter…", "") } else { ("Pick a chapter", "Choose a downloaded chapter on the left to see its pages. Tick several to read or narrate them in one go.") };
            return div().flex().flex_1().items_center().justify_center().child(empty_state(msg.0, msg.1)).into_any_element();
        };
        let count = match self.phase {
            Phase::Read | Phase::Transcript => f.pages.len(),
            Phase::Story => f.story.as_ref().map(|s| s.segments.len()).unwrap_or(0),
            Phase::Audio => 1 + audio_pages(f).len(),
            Phase::Video => 1,
        };
        self.sync_list(count);
        let header = self.phase_header(window, cx);
        let empty = count == 0;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .when(empty, |d| {
                let (t, b) = match self.phase {
                    Phase::Story => ("No story yet", "Pick the styles above and press Write story. The AI turns what it read into a recap script."),
                    _ => ("No pages", "This chapter's page folder is empty. Download it again from the manga page."),
                };
                d.child(div().flex().flex_1().items_center().justify_center().child(empty_state(t, b)))
            })
            .when(!empty, |d| {
                d.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(list(self.list.clone(), cx.processor(|this: &mut Self, ix: usize, window, cx| this.row(ix, window, cx))).size_full().pr(px(8.)).pb(px(24.)))
                        .child(crate::page::vbar("studio-center-bar", &self.list)),
                )
            })
            .into_any_element()
    }

    fn row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let el = match self.phase {
            Phase::Read => self.read_row(ix, cx),
            Phase::Transcript => self.transcript_row(ix, window, cx),
            Phase::Story => self.story_row(ix, window, cx),
            Phase::Audio if ix == 0 => self.voice_card(cx),
            Phase::Audio => self.audio_row(ix - 1, cx),
            Phase::Video => self.video_card(cx),
        };
        div().px(px(20.)).pt(px(14.)).child(el).into_any_element()
    }

    // ------------------------------------------------------------ headers

    fn phase_header(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        let f = self.focus.as_ref().expect("focus");
        let run = self.run.clone().filter(|r| r.running());
        let (desc, stats): (&str, String) = match self.phase {
            Phase::Read => (
                "The AI looks at every page: transcribes the balloons, recognises characters and remembers their faces, and tags each scene.",
                format!("{}/{} pages read · {} characters known", f.status.read_pages, f.status.total_pages, run::brain_snapshot(&f.item.manga_id, &f.item.manga_title).characters.len()),
            ),
            Phase::Transcript => ("What each page says. Edit any page, or ask the AI to fix it — it looks at the page again.", format!("{} pages", f.pages.len())),
            Phase::Story => (
                "The AI turns everything it read into a recap script. Pick a feel; by default it follows each scene's mood.",
                f.story.as_ref().map(|s| format!("{} · {} segments", if s.title.is_empty() { "Story" } else { s.title.as_str() }, s.segments.len())).unwrap_or_default(),
            ),
            Phase::Audio => ("Narrate the script with a local voice. Listen to each page and regenerate any that sound off.", format!("{} page clips", f.status.audio_files)),
            Phase::Video => ("Pages and panels follow the voice-over, in sync, as a recap video.", if f.status.video.is_some() { "Rendered".into() } else { String::new() }),
        };
        let title = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(3.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(self.phase.label()))
                    .child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("{} · {}", f.item.manga_title, f.item.label())))
                    .when(!stats.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("· {stats}")))),
            )
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(desc));
        let actions = div().flex().items_center().gap(px(8.)).flex_none();
        let actions = match self.phase {
            Phase::Read => match run.as_ref().filter(|r| r.kind == RunKind::Read) {
                Some(_) => actions.child(stop_button(cx)),
                None => actions
                    .child(self.model_button.clone())
                    .when(f.status.read_pages > 0, |d| {
                        d.child(button("read-again", "Read again", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(true, cx))))
                    })
                    .child(primary("read-it", "icons/sparkles.svg", self.target_label("Read it"), run.is_none()).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(false, cx)))),
            },
            Phase::Transcript => actions.child(button("to-story", "Continue to Story", ButtonKind::Secondary).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_phase(Phase::Story, cx)))),
            Phase::Story => match run.as_ref().filter(|r| r.kind == RunKind::Story) {
                Some(_) => actions.child(stop_button(cx)),
                None => actions.child(self.model_button.clone()).child(
                    primary("write-story", "icons/feather.svg", self.target_label(if f.story.is_some() { "Rewrite story" } else { "Write story" }), run.is_none())
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_story(cx))),
                ),
            },
            Phase::Audio => {
                let job = self.jobs.iter().find(|j| j.chapter_id == f.item.id && !j.state.finished()).cloned();
                match job {
                    Some(j) => actions
                        .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(j.status.clone()))
                        .child(button("audio-cancel", "Cancel", ButtonKind::Ghost).on_click(move |_, _, _| {
                            service::cancel(j.id);
                        })),
                    None => actions.child(
                        primary("gen-audio", "icons/audio-lines.svg", self.target_label(if f.status.audio_files > 0 { "Regenerate all" } else { "Generate audio" }), true)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.run_pipeline(&["tts"], true, cx))),
                    ),
                }
            }
            Phase::Video => actions,
        };
        let mut head = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(12.))
            .px(px(20.))
            .pt(px(18.))
            .pb(px(14.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(div().flex().items_center().gap(px(16.)).child(title).child(actions));
        if self.phase == Phase::Story {
            head = head.child(self.style_chips(cx));
        }
        if let Some(r) = run.filter(|r| matches!((r.kind, self.phase), (RunKind::Read, Phase::Read) | (RunKind::Story, Phase::Story))) {
            let label = format!("{} · {}", r.status_line(), r.model);
            head = head.child(progress_line(r.progress(), label));
        }
        if let Some(j) = self.jobs.iter().find(|j| j.chapter_id == f.item.id && !j.state.finished()).filter(|_| matches!(self.phase, Phase::Audio | Phase::Video)) {
            head = head.child(progress_line(j.progress() as f32, j.status.clone()));
        }
        head
    }

    fn style_chips(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(div().mr(px(4.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child("Feel"))
            .children(STYLES.iter().map(|&s| {
                let on = self.prefs.styles.iter().any(|x| x == s);
                chip(SharedString::from(format!("style-{s}")), s, on).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if on {
                        this.prefs.styles.retain(|x| x != s);
                    } else {
                        this.prefs.styles.push(s.to_string());
                    }
                    this.prefs.save();
                    cx.notify();
                }))
            }))
            .when(self.prefs.styles.is_empty(), |d| d.child(div().ml(px(4.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child("None picked: follows each scene")))
    }

    // ------------------------------------------------------------ Read

    fn read_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let f = self.focus.as_ref().expect("focus");
        let Some((path, ratio)) = f.pages.get(ix).cloned() else { return div().into_any_element() };
        let read = f.read.as_ref().and_then(|r| r.pages.iter().find(|p| p.index == ix)).cloned();
        let reading_now = self.run.as_ref().is_some_and(|r| r.running() && r.kind == RunKind::Read && r.chapter_id.as_deref() == Some(f.item.id.as_str()) && r.reading.contains(&ix));
        let faces = run::brain_snapshot(&f.item.manga_id, &f.item.manga_title);
        div()
            .flex()
            .items_start()
            .gap(px(16.))
            .child(page_image(ix, &path, ratio, PAGE_MAX_W, reading_now))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .w(px(300.))
                    .flex_none()
                    .p(px(14.))
                    .rounded(px(12.))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(if reading_now { Tokens::accent() } else { Tokens::border() })
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Page {}", ix + 1)))
                    .map(|d| match &read {
                        None if reading_now => d.child(div().text_size(px(size::SM)).text_color(Tokens::accent()).child("Reading this page…")),
                        None => d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child("Not read yet.")),
                        Some(p) => d.child(page_notes(p, &faces)),
                    })
                    .when(read.is_some() && !reading_now, |d| {
                        d.child(
                            button(SharedString::from(format!("reread-{ix}")), "Read this page again", ButtonKind::Ghost)
                                .h(px(26.))
                                .text_size(px(size::XS))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reread_page(ix, cx))),
                        )
                    }),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------ Transcript

    fn transcript_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (path, ratio, text, edited, scene) = {
            let f = self.focus.as_ref().expect("focus");
            let Some((path, ratio)) = f.pages.get(ix).cloned() else { return div().into_any_element() };
            let tp = f.transcript.as_ref().and_then(|t| t.pages.iter().find(|p| p.index == ix));
            let scene = f.read.as_ref().and_then(|r| r.pages.iter().find(|p| p.index == ix)).map(|p| p.scene.clone());
            (path, ratio, transcript_text(f, ix), tp.is_some_and(|p| p.edited), scene)
        };
        let editor = self.editor(Phase::Transcript, ix, &text, window, cx);
        let asking_here = self.ask == Some(ix);
        let busy = self.asking == Some(ix);
        div()
            .flex()
            .items_start()
            .gap(px(16.))
            .child(page_image(ix, &path, ratio, 240., false))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format!("Page {}", ix + 1)))
                            .children(scene.filter(|s| !s.kind.is_empty()).map(|s| scene_tag(&s)))
                            .when(edited, |d| d.child(tag("Edited", Tokens::warning())))
                            .child(div().flex_1())
                            .when(edited, |d| {
                                d.child(small_button(format!("reset-{ix}"), "icons/rotate-ccw.svg", "Use AI text").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reset_transcript_page(ix, cx))))
                            })
                            .child(small_button(format!("ask-{ix}"), "icons/sparkles.svg", "Ask AI").on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.ask = if this.ask == Some(ix) { None } else { Some(ix) };
                                if this.ask.is_some() {
                                    this.ask_input.update(cx, |s, cx| s.set_value("", window, cx));
                                    let fh = this.ask_input.read(cx).focus_handle(cx);
                                    fh.focus(window, cx);
                                }
                                this.update_assistant_context();
                                cx.notify();
                            }))),
                    )
                    .child(
                        div()
                            .rounded(px(10.))
                            .bg(Tokens::surface())
                            .border_1()
                            .border_color(Tokens::border())
                            .p(px(4.))
                            .text_size(px(size::MD))
                            .child(Textarea::new(&editor).appearance(false).bordered(false)),
                    )
                    .when(asking_here, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .p(px(8.))
                                .rounded(px(10.))
                                .bg(Tokens::accent_soft())
                                .child(icon("icons/sparkles.svg", Tokens::accent(), 14.))
                                .child(div().flex_1().child(Input::new(&self.ask_input).small()))
                                .child(
                                    button(SharedString::from(format!("rewrite-{ix}")), if busy { "Rewriting…" } else { "Rewrite" }, ButtonKind::Primary)
                                        .h(px(28.))
                                        .when(!busy, |b| b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.rewrite_page(ix, cx)))),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------ Story

    fn story_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (seg, thumb, scene) = {
            let f = self.focus.as_ref().expect("focus");
            let Some(seg) = f.story.as_ref().and_then(|s| s.segments.get(ix)).cloned() else { return div().into_any_element() };
            let panel = seg.panel.map(|n| mv_recap::panel_path(&f.chref, seg.page, n)).filter(|p| p.is_file());
            let thumb = panel.or_else(|| f.pages.get(seg.page).map(|p| p.0.clone()));
            let scene = f.read.as_ref().and_then(|r| r.pages.iter().find(|p| p.index == seg.page)).map(|p| p.scene.clone());
            (seg, thumb, scene)
        };
        let editor = self.editor(Phase::Story, ix, &seg.text, window, cx);
        div()
            .flex()
            .items_start()
            .gap(px(14.))
            .child(div().w(px(150.)).flex_none().rounded(px(8.)).overflow_hidden().bg(Tokens::surface()).children(thumb.map(|p| img(p).w_full())))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(match seg.panel {
                                Some(n) => format!("Page {} · panel {}", seg.page + 1, n + 1),
                                None => format!("Page {}", seg.page + 1),
                            }))
                            .children(scene.filter(|s| !s.kind.is_empty()).map(|s| scene_tag(&s))),
                    )
                    .child(div().text_size(px(size::LG)).line_height(px(24.)).child(Textarea::new(&editor).appearance(false).bordered(false))),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------ Audio

    fn voice_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let env = self.setup.read(cx).env.clone();
        let engines = env.as_ref().map(|e| e.engines.clone()).unwrap_or_default();
        // The default comes from Settings → Audio; a chapter may have its own voice.
        let default = self.prefs.voice.clone().unwrap_or_default();
        let own = self.focus_id.as_deref().and_then(mv_studio::voices::chapter_voice);
        let voice = own.clone().unwrap_or_else(|| default.clone());
        let default_label = mv_studio::tts::find_voice(&default).map(|v| v.label).unwrap_or("none");
        let langs = div().flex().gap(px(6.)).children([("en", "English"), ("hi", "Hindi")].map(|(code, label)| {
            chip(SharedString::from(format!("lang-{code}")), label, self.lang == code).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.lang = code;
                cx.notify();
            }))
        }));
        let speeds = div().flex().gap(px(6.)).children(SPEEDS.map(|s| {
            chip(SharedString::from(format!("speed-{s}")), format!("{s}x"), self.prefs.speed == s).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.prefs.speed = s;
                this.prefs.save();
                cx.notify();
            }))
        }));
        let playing = audio::now_playing();
        let tiles = div().flex().flex_wrap().gap(px(8.)).children(setup::voices(self.lang).into_iter().map(|v| {
            let on = voice == v.id;
            let ready = setup::voice_ready(&engines, v);
            let (status, color) = match ready {
                _ if env.is_none() => ("Checking…".to_string(), Tokens::text_3()),
                VoiceReady::Ready(e) => (e.to_string(), Tokens::accent()),
                VoiceReady::Fallback(e) => (format!("Uses {e} for now"), Tokens::warning()),
                VoiceReady::Missing => ("Not installed".to_string(), Tokens::text_3()),
            };
            let id = v.id;
            let previewing = self.previewing.as_deref() == Some(id);
            let sample_playing = playing.as_ref().is_some_and(|p| p.to_string_lossy().contains(&mv_studio::tts::safe_voice_name(id)));
            div()
                .id(SharedString::from(format!("voice-{id}")))
                .flex()
                .items_center()
                .gap(px(10.))
                .w(px(232.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(10.))
                .border_1()
                .cursor_pointer()
                .map(|d| if on { d.border_color(Tokens::accent()).bg(Tokens::accent_soft()) } else { d.border_color(Tokens::border()).hover(|s| s.bg(Tokens::hover())) })
                .child(
                    div()
                        .id(SharedString::from(format!("sample-{id}")))
                        .size(px(30.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(Tokens::surface_2())
                        .hover(|s| s.bg(Tokens::active()))
                        .child(icon(
                            if previewing { "icons/loader-circle.svg" } else if sample_playing { "icons/square.svg" } else { "icons/play.svg" },
                            Tokens::text_1(),
                            13.,
                        ))
                        .when(ready.usable(), |d| {
                            d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.sample(id, cx);
                            }))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(v.label))
                        .child(div().truncate().text_size(px(size::XS)).text_color(color).child(status)),
                )
                .when(on, |d| d.child(icon("icons/star.svg", Tokens::accent(), 13.)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let chapters: Vec<String> = this.targets().into_iter().map(|c| c.id).collect();
                    let default = this.prefs.voice.clone().unwrap_or_default();
                    if chapters.is_empty() {
                        this.prefs.voice = Some(id.to_string());
                        if let Err(e) = mv_studio::voices::set_default_voice(id) {
                            this.say(true, format!("Could not save the voice: {e}"));
                        }
                    }
                    for ch in chapters {
                        // Picking the default again clears the chapter's own choice.
                        if let Err(e) = mv_studio::voices::set_chapter_voice(&ch, (id != default).then_some(id)) {
                            this.say(true, format!("Could not save the voice: {e}"));
                        }
                    }
                    cx.notify();
                }))
        }));
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .rounded(px(12.))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).child("Narrator"))
                    .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if own.is_some() {
                        format!("Own voice for this chapter. Default: {default_label}.")
                    } else {
                        format!("Default: {default_label}. A voice picked here is for this chapter only.")
                    }))
                    .when(own.is_some(), |d| {
                        d.child(chip("voice-use-default", "Use default", false).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            for ch in this.targets() {
                                let _ = mv_studio::voices::set_chapter_voice(&ch.id, None);
                            }
                            cx.notify();
                        })))
                    })
                    .child(langs),
            )
            .child(tiles)
            .child(div().flex().items_center().gap(px(10.)).child(div().w(px(48.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child("Speed")).child(speeds))
            .into_any_element()
    }

    fn audio_row(&mut self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let f = self.focus.as_ref().expect("focus");
        let Some((page, text)) = audio_pages(f).into_iter().nth(i) else { return div().into_any_element() };
        let wav = f.item.audio_dir().join(format!("page_{page:04}.wav"));
        let exists = std::fs::metadata(&wav).map(|m| m.len() > 100).unwrap_or(false);
        let playing = audio::now_playing().as_deref() == Some(wav.as_path());
        let busy = self.jobs.iter().any(|j| j.chapter_id == f.item.id && !j.state.finished());
        let thumb = f.pages.get(page).map(|p| p.0.clone());
        let w2 = wav.clone();
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .p(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(if playing { Tokens::accent() } else { Tokens::border() })
            .child(
                div()
                    .id(SharedString::from(format!("play-{page}")))
                    .size(px(34.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .map(|d| if exists { d.bg(Tokens::accent()).cursor_pointer().hover(|s| s.opacity(0.9)) } else { d.bg(Tokens::surface_2()) })
                    .child(icon(if playing { "icons/square.svg" } else { "icons/play.svg" }, if exists { Tokens::base() } else { Tokens::text_3() }, 14.))
                    .when(exists, |d| {
                        d.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            audio::toggle(&w2);
                            cx.notify();
                        }))
                    }),
            )
            .child(div().w(px(44.)).h(px(60.)).flex_none().rounded(px(6.)).overflow_hidden().bg(Tokens::surface()).children(thumb.map(|p| img(p).size_full().object_fit(ObjectFit::Cover))))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Page {}{}", page + 1, if exists { "" } else { " · no audio yet" })))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).line_clamp(2).child(text)),
            )
            .when(!busy, |d| {
                d.child(small_button(format!("regen-{page}"), "icons/refresh-cw.svg", "Regenerate").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let _ = std::fs::remove_file(&wav);
                    this.run_pipeline(&["tts"], false, cx);
                })))
            })
            .into_any_element()
    }

    // ------------------------------------------------------------ Video

    fn video_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let f = self.focus.as_ref().expect("focus");
        let env = self.setup.read(cx).env.clone();
        let ffmpeg = env.as_ref().is_some_and(|e| e.ffmpeg);
        let job = self.jobs.iter().find(|j| j.chapter_id == f.item.id && !j.state.finished()).cloned();
        let has_audio = f.status.audio_files > 0;
        let video = f.status.video.clone();
        let title = format!("{} · {}", f.item.manga_title, f.item.label());
        let res = div().flex().gap(px(6.)).children(RESOLUTIONS.map(|r| {
            chip(SharedString::from(format!("res-{r}")), r, self.prefs.resolution == r).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.prefs.resolution = r.to_string();
                this.prefs.save();
                cx.notify();
            }))
        }));
        let blocker = if !has_audio {
            Some("Generate the audio first (Audio step).")
        } else if env.is_some() && !ffmpeg {
            Some("FFmpeg is needed to render video. See Setup at the top.")
        } else {
            None
        };
        let render_btn = match &job {
            Some(j) => button("video-cancel", "Cancel render", ButtonKind::Secondary).on_click({
                let id = j.id;
                move |_, _, _| {
                    service::cancel(id);
                }
            }),
            None => primary("render-video", "icons/clapperboard.svg", if video.is_some() { "Render again".to_string() } else { "Render video".to_string() }, blocker.is_none())
                .when(blocker.is_none(), |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.run_pipeline(&["sync_map", "video"], false, cx)))),
        };
        let bhippi = find_bhippi();
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .max_w(px(900.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .p(px(16.))
                    .rounded(px(12.))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(Tokens::border())
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Resolution"))
                    .child(res)
                    .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_3()).children(blocker))
                    .child(render_btn),
            )
            .map(|d| match video {
                Some(v) => {
                    let (v1, v2, v3) = (v.clone(), v.clone(), v.clone());
                    let thumb = f.pages.first().map(|p| p.0.clone());
                    d.child(
                        div()
                            .id("video-result")
                            .relative()
                            .w_full()
                            .aspect_ratio(16. / 9.)
                            .rounded(px(14.))
                            .overflow_hidden()
                            .bg(rgb(0x000000))
                            .cursor_pointer()
                            .children(thumb.map(|p| img(p).size_full().object_fit(ObjectFit::Cover).opacity(0.45)))
                            .child(
                                div().absolute().inset_0().flex().items_center().justify_center().child(
                                    div().size(px(72.)).rounded_full().bg(Tokens::accent()).flex().items_center().justify_center().child(icon("icons/play.svg", Tokens::base(), 30.)),
                                ),
                            )
                            .on_click(move |_, w, cx| play_video(&title, &v1, w, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(button("video-folder", "Show in folder", ButtonKind::Secondary).on_click(move |_, _, cx| {
                                if let Some(dir) = v2.parent() {
                                    cx.open_with_system(dir);
                                }
                            }))
                            .when_some(bhippi, |d, exe| {
                                d.child(button("video-bhippi", "Edit in Bhippi", ButtonKind::Secondary).on_click(move |_, _, _| {
                                    let _ = std::process::Command::new(&exe).arg(&v3).spawn();
                                }))
                            }),
                    )
                }
                None => d.child(div().py(px(40.)).child(empty_state("No video yet", "Render once the audio is ready. Each page or panel stays on screen exactly as long as its narration."))),
            })
            .into_any_element()
    }

    // ------------------------------------------------------------ pipeline + samples

    /// Queues the Python pipeline for the target chapters with only `stages`.
    pub(crate) fn run_pipeline(&mut self, stages: &[&str], force_regen: bool, cx: &mut Context<Self>) {
        self.flush_edits(cx);
        let Some(env) = self.setup.read(cx).env.clone() else {
            self.say(true, "Still checking your setup…");
            return cx.notify();
        };
        if !env.runnable() {
            self.show_setup = true;
            self.say(true, "Studio needs Python and the pipeline scripts. See Setup.");
            return cx.notify();
        }
        let targets = self.targets();
        // Each chapter is narrated with its own voice, else the default (Settings → Audio).
        let mut by_voice: Vec<(String, Vec<crate::data::ChapterItem>)> = vec![];
        for c in &targets {
            let v = mv_studio::voices::voice_for_chapter(&c.id);
            match by_voice.iter_mut().find(|(g, _)| *g == v) {
                Some((_, list)) => list.push(c.clone()),
                None => by_voice.push((v, vec![c.clone()])),
            }
        }
        if stages.contains(&"tts") {
            let missing_voice = by_voice.iter().map(|(v, _)| v).find(|v| !mv_studio::tts::find_voice(v).is_some_and(|v| setup::voice_ready(&env.engines, v).usable()));
            if let Some(v) = missing_voice {
                let label = mv_studio::tts::find_voice(v).map(|v| v.label).unwrap_or(v.as_str());
                self.show_setup = true;
                self.say(true, format!("{label} isn't installed yet. Download it in Settings → Audio, or pick a ready voice."));
                return cx.notify();
            }
        }
        let missing: Vec<String> = targets.iter().filter(|c| !self.statuses.get(&c.id).is_some_and(|s| s.story)).map(|c| c.label()).collect();
        if stages.contains(&"tts") && !missing.is_empty() {
            self.say(true, format!("Write the story first for: {}", missing.join(", ")));
            return cx.notify();
        }
        let mut queued: anyhow::Result<usize> = Ok(0);
        for (voice, chapters) in by_voice {
            let req = NarrationRequest {
                voice,
                speed: self.prefs.speed,
                video: stages.contains(&"video"),
                resolution: self.prefs.resolution.clone(),
                stages: Some(stages.iter().map(|s| s.to_string()).collect()),
                force_regen,
            };
            queued = match (queued, service::enqueue(&env, &req, &chapters)) {
                (Ok(a), Ok(b)) => Ok(a + b),
                (Err(e), _) | (_, Err(e)) => Err(e),
            };
        }
        match queued {
            Ok(0) => self.say(false, "Already in progress."),
            Ok(n) => {
                self.notice = None;
                self.jobs = service::snapshot();
                if n > 1 {
                    self.say(false, format!("Queued {n} chapter{}.", plural(n)));
                }
            }
            Err(e) => self.say(true, format!("{e:#}")),
        }
        cx.notify();
    }

    fn sample(&mut self, voice: &'static str, cx: &mut Context<Self>) {
        let Some(env) = self.setup.read(cx).env.clone() else { return };
        let (Ok(py), Ok(layout)) = (env.python.clone(), env.scripts.clone()) else { return };
        let speed = self.prefs.speed;
        let cached = mv_studio::tts::preview_cache_path(&mv_studio::tts::default_samples_dir(), voice, mv_studio::tts::default_preview_engine(voice), speed);
        if audio::now_playing().as_deref() == Some(cached.as_path()) {
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
                    Ok(Err(e)) => this.say(true, format!("Sample failed: {e}")),
                    Err(e) => this.say(true, format!("Sample failed: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

// ---------------------------------------------------------------- pieces

/// Pages that have narration in the story, with their joined text.
fn audio_pages(f: &Focus) -> Vec<(usize, String)> {
    let Some(story) = &f.story else { return vec![] };
    let mut out: Vec<(usize, String)> = vec![];
    for s in &story.segments {
        let t = s.text.trim();
        if t.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((p, text)) if *p == s.page => {
                text.push(' ');
                text.push_str(t);
            }
            _ => out.push((s.page, t.to_string())),
        }
    }
    out
}

fn page_image(ix: usize, path: &Path, ratio: Option<f32>, max_w: f32, glow: bool) -> Div {
    let im = img(path.to_path_buf());
    div()
        .relative()
        .flex_1()
        .max_w(px(max_w))
        .min_w(px(120.))
        .rounded(px(8.))
        .overflow_hidden()
        .bg(Tokens::surface())
        .border_1()
        .border_color(if glow { Tokens::accent() } else { Tokens::border() })
        .map(|d| match ratio {
            Some(r) => d.child(div().w_full().aspect_ratio(r).child(im.size_full())),
            None => d.child(im.w_full()),
        })
        .child(
            div()
                .absolute()
                .top(px(8.))
                .left(px(8.))
                .px(px(7.))
                .py(px(2.))
                .rounded(px(6.))
                .bg(Tokens::scrim())
                .text_size(px(size::XS))
                .text_color(Tokens::text_1())
                .child(format!("{}", ix + 1)),
        )
}

fn page_notes(p: &PageRead, brain: &mv_recap::Brain) -> Div {
    let mut names: Vec<String> = vec![];
    for c in p.panels.iter().flat_map(|pn| pn.characters.iter()) {
        if !names.contains(&c.name) {
            names.push(c.name.clone());
        }
    }
    let lines: Vec<String> = p
        .panels
        .iter()
        .flat_map(|pn| pn.lines.iter())
        .filter(|l| !l.text.trim().is_empty())
        .map(|l| match &l.speaker {
            Some(s) if !s.is_empty() => format!("{s}: {}", l.text),
            _ => l.text.clone(),
        })
        .collect();
    let more = lines.len().saturating_sub(5);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(div().flex().flex_wrap().gap(px(5.)).when(!p.scene.kind.is_empty(), |d| d.child(scene_tag(&p.scene))).children(p.scene.tags.iter().take(4).map(|t| tag(t.clone(), Tokens::text_3()))))
        .when(!p.summary.is_empty(), |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(p.summary.clone())))
        .when(!names.is_empty(), |d| {
            d.child(div().flex().flex_wrap().gap(px(6.)).children(names.into_iter().map(|n| {
                let face = brain.find(&n).and_then(|c| c.faces.first().cloned());
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .pl(px(2.))
                    .pr(px(8.))
                    .py(px(2.))
                    .rounded_full()
                    .bg(Tokens::surface_2())
                    .child(face_avatar(face, 20.))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(n))
            })))
        })
        .when(!lines.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .pl(px(8.))
                    .border_l_2()
                    .border_color(Tokens::border())
                    .children(lines.into_iter().take(5).map(|l| div().text_size(px(size::XS)).text_color(Tokens::text_2()).line_clamp(2).child(l)))
                    .when(more > 0, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("+{more} more line{}", plural(more))))),
            )
        })
}

pub(crate) fn face_avatar(face: Option<PathBuf>, s: f32) -> Div {
    div()
        .size(px(s))
        .flex_none()
        .rounded_full()
        .overflow_hidden()
        .bg(Tokens::active())
        .flex()
        .items_center()
        .justify_center()
        .map(|d| match face {
            Some(p) => d.child(img(p).size_full().rounded_full().object_fit(ObjectFit::Cover)),
            None => d.child(icon("icons/user-round.svg", Tokens::text_3(), s * 0.6)),
        })
}

fn scene_tag(s: &mv_recap::Scene) -> Div {
    let color = match s.kind.as_str() {
        "fight" | "action" | "horror" => Tokens::danger(),
        "emotional" | "romance" | "flashback" => rgb(0xa78bfa),
        "comedy" => Tokens::warning(),
        "reveal" => rgb(0x60a5fa),
        _ => Tokens::accent(),
    };
    let label = if s.mood.is_empty() { s.kind.clone() } else { format!("{} · {}", s.kind, s.mood) };
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .px(px(7.))
        .py(px(1.))
        .rounded(px(6.))
        .bg(Tokens::surface_2())
        .text_size(px(size::XS))
        .text_color(color)
        .child(label)
        .when(s.intensity > 0, |d| {
            d.child(div().flex().gap(px(1.)).children((0..5).map(|i| div().w(px(3.)).h(px(8.)).rounded(px(1.)).bg(if (i as u8) * 2 < s.intensity { color } else { Tokens::active() }))))
        })
}

pub(crate) fn tag(text: impl Into<SharedString>, color: Rgba) -> Div {
    div().px(px(7.)).py(px(1.)).rounded(px(6.)).bg(Tokens::surface_2()).text_size(px(size::XS)).text_color(color).child(text.into())
}

pub(crate) fn primary(id: &str, ic: &'static str, label: String, enabled: bool) -> Stateful<Div> {
    button(SharedString::from(id.to_string()), label, ButtonKind::Primary)
        .h(px(36.))
        .px(px(16.))
        .rounded(px(10.))
        .child(icon(ic, Tokens::base(), 15.))
        .flex_row_reverse()
        .when(!enabled, |b| b.opacity(0.45).cursor_default())
}

fn small_button(id: String, ic: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(26.))
        .px(px(8.))
        .rounded(px(7.))
        .cursor_pointer()
        .text_size(px(size::XS))
        .text_color(Tokens::text_2())
        .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
        .child(icon(ic, Tokens::text_2(), 12.))
        .child(label)
}

fn stop_button(cx: &mut Context<StudioPage>) -> Stateful<Div> {
    button("run-stop", "Stop", ButtonKind::Secondary).child(icon("icons/circle-stop.svg", Tokens::text_1(), 14.)).flex_row_reverse().on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
        run::cancel();
        cx.notify();
    }))
}

pub(crate) fn progress_line(p: f32, label: String) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().flex().items_center().gap(px(8.)).child(icon("icons/loader-circle.svg", Tokens::accent(), 12.)).child(div().flex_1().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(label)).child(
            div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{}%", (p * 100.).round() as i32)),
        ))
        .child(div().w_full().h(px(3.)).rounded_full().bg(Tokens::active()).child(div().h_full().rounded_full().bg(Tokens::accent()).w(relative(p.clamp(0.0, 1.0)))))
}

fn play_video(title: &str, path: &Path, window: &mut Window, cx: &mut App) {
    let request = PlayRequest { title: title.to_string(), source: path.display().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
    mv_ui::navigate(Route::Player { request }, window, cx);
}

/// Bhippi Video Editor, when installed (per-user install or a local build).
fn find_bhippi() -> Option<PathBuf> {
    static FOUND: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            let mut roots = vec![];
            if let Some(l) = dirs_local() {
                roots.push(l.join("Programs"));
                roots.push(l);
            }
            roots.push(PathBuf::from(r"C:\Program Files"));
            for root in roots {
                let Ok(rd) = std::fs::read_dir(&root) else { continue };
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if !name.contains("bhippi") || !e.path().is_dir() {
                        continue;
                    }
                    let Ok(inner) = std::fs::read_dir(e.path()) else { continue };
                    for f in inner.flatten() {
                        let n = f.file_name().to_string_lossy().to_lowercase();
                        if n.ends_with(".exe") && n.contains("bhippi") && !n.contains("unins") {
                            return Some(f.path());
                        }
                    }
                }
            }
            None
        })
        .clone()
}

fn dirs_local() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}
