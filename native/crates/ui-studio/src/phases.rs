//! The middle column: one header per phase (what it does, its main action and a reset) over a
//! virtual list of the chapter's pages, segments or settings.
use std::path::{Path, PathBuf};

use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::slider::Slider;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::{Delivery, PageRead, Prefs, STYLES};
use mv_ui::nav::PlayRequest;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, empty_view, filter_chip, fmt_bytes, glyphs, icon_button, icon_ph, icon_tag, inline_alert, kbd, menu_button, muted, path_chip, ph, ph_icon, progress_bar, scan_overlay, segmented,
    selected_badge, shimmer_text, size, skeleton_block, skeleton_line, spinner, tag, tip, AlertKind, Btn, ButtonKind, Interactive as _, MenuItem, Ph, Route, Tokens, Tone, Weight,
};

use crate::page::{plural, transcript_text, Focus, Load, NoticeAction, Phase, StudioPage};
use crate::run::RunKind;
use crate::service::{self, NarrationRequest};
use crate::setup::{self, VoiceReady};
use crate::widgets::{ago, card, delivery_preset, feel_icon, field_frame, fmt_secs, phase_icon, scene_tag, stat_pill, DELIVERIES};
use crate::{audio, run};

const SPEEDS: [f64; 4] = [0.9, 1.0, 1.1, 1.25];
const RESOLUTIONS: [&str; 2] = ["720p", "1080p"];
const FORMATS: [(&str, &str, Ph); 2] = [(mv_recap::FORMAT_LANDSCAPE, "16:9 Landscape", ph::MONITOR), (mv_recap::FORMAT_REELS, "9:16 Reels", ph::DEVICE_MOBILE)];
const PAGE_MAX_W: f32 = 760.;
/// Below this centre width the Read notes go under the page instead of beside it
/// (Material 3 window size classes: compact layouts stack panes).
const STACK_BELOW: f32 = 720.;

impl StudioPage {
    pub(crate) fn center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(f) = &self.focus else {
            if self.focus_id.is_some() {
                return center_skeleton().into_any_element();
            }
            return self.welcome(cx);
        };
        let count = match self.phase {
            Phase::Read | Phase::Transcript => f.pages.len(),
            Phase::Story => f.story.as_ref().map(|s| s.segments.len()).unwrap_or(0),
            Phase::Audio => 1 + audio_pages(f).len(),
            Phase::Music | Phase::Video => 1,
        };
        let no_pages = f.pages.is_empty();
        let nothing_read = f.status.read_pages == 0;
        self.sync_list(count);
        // The header's Compose button and cue count read the focused chapter's music, so load
        // it before the header (the card below loads it too, but renders after).
        if self.phase == Phase::Music {
            self.music_sync();
        }
        let header = self.phase_header(window, cx);
        let empty: Option<AnyElement> = match self.phase {
            Phase::Story if count == 0 => Some(
                empty_view(glyphs::STORY, "No story yet", "Pick a feel above (or none to follow each scene) and press Write story. The AI turns what it read into a recap script.")
                    .when(nothing_read, |e| {
                        e.secondary(btn("story-go-read", "Read the chapter first", ButtonKind::Secondary).icon(glyphs::READ).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_phase(Phase::Read, cx))))
                    })
                    .into_any_element(),
            ),
            Phase::Read | Phase::Transcript if no_pages => {
                Some(empty_view(ph::IMAGE_BROKEN, "No pages", "This chapter's page folder is empty. Download it again from the manga page.").into_any_element())
            }
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .map(|d| match empty {
                Some(e) => d.child(div().flex().flex_1().items_center().justify_center().child(e)),
                None => d.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(list(self.list.clone(), cx.processor(|this: &mut Self, ix: usize, window, cx| this.row(ix, window, cx))).size_full().pr(px(space::SM)).pb(px(space::XXL)))
                        .child(crate::page::vbar("studio-center-bar", &self.list)),
                ),
            })
            .into_any_element()
    }

    /// No chapter open: what to do first, and the setup checklist when something is missing.
    fn welcome(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let issues = self.setup.read(cx).issues();
        let empty_lib = matches!(&self.library, Load::Ready(g) if g.is_empty());
        let lead = if empty_lib {
            empty_view(glyphs::DOWNLOADS, "Download a chapter to begin", "Studio turns downloaded chapters into narrated recap videos.")
                .action(btn("welcome-library", "Open Library", ButtonKind::Primary).icon(glyphs::LIBRARY).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx)))
        } else {
            empty_view(ph::BOOK_OPEN_TEXT, "Pick a chapter", "Choose a downloaded chapter on the left to see its pages. Tick several to read or narrate them together.")
        };
        div()
            .id("studio-welcome")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .items_center()
            .gap(px(space::LG))
            .p(px(space::XXL))
            .child(lead)
            .when(issues > 0 && !self.show_setup, |d| d.child(div().w_full().max_w(px(680.)).child(card().child(self.setup.clone()))))
            .into_any_element()
    }

    fn row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let el = match self.phase {
            Phase::Read => self.read_row(ix, cx),
            Phase::Transcript => self.transcript_row(ix, window, cx),
            Phase::Story => self.story_row(ix, window, cx),
            Phase::Audio if ix == 0 => self.voice_card(window, cx),
            Phase::Audio => self.audio_row(ix - 1, cx),
            Phase::Music => self.music_card(cx),
            Phase::Video => self.video_card(window, cx),
        };
        div().px(px(space::XL)).pt(px(14.)).child(el).into_any_element()
    }

    // ------------------------------------------------------------ headers

    /// Whether `phase` (or a later step built from it) has anything a reset would clear.
    fn has_work_from(&self, phase: Phase) -> bool {
        let Some(f) = &self.focus else { return false };
        let st = &f.status;
        // A transcript nobody edited is just the AI's text again: nothing to reset there.
        let edited = f.transcript.as_ref().is_some_and(|t| t.pages.iter().any(|p| p.edited));
        let work = [st.read_pages > 0, edited, st.story, st.audio_files > 0, st.music, st.video.is_some() || !st.reels.is_empty()];
        work[phase.index()..].iter().any(|w| *w)
    }

    fn phase_header(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        let f = self.focus.as_ref().expect("focus");
        let run = self.run.clone().filter(|r| r.running());
        let blocker = self.run_blocker();
        let phase = self.phase;
        let edited = f.transcript.as_ref().map(|t| t.pages.iter().filter(|p| p.edited).count()).unwrap_or(0);
        let (desc, stats): (&str, Vec<(Ph, String)>) = match phase {
            Phase::Read => (
                "The AI looks at every page: it transcribes the balloons, recognises characters and remembers their faces, and tags each scene.",
                vec![
                    (glyphs::READ, format!("{}/{} read", f.status.read_pages, f.status.total_pages)),
                    (ph::USERS, format!("{} characters", run::brain_snapshot(&f.item.manga_id, &f.item.manga_title).characters.len())),
                ],
            ),
            Phase::Transcript => (
                "What each page says. Edit any page, or ask the AI to fix it: it looks at the page again.",
                [(ph::FILE_TEXT, format!("{} pages", f.pages.len()))].into_iter().chain((edited > 0).then(|| (glyphs::EDIT, format!("{edited} edited")))).collect(),
            ),
            Phase::Story => (
                "The AI turns everything it read into a recap script. Pick a feel; with none it follows each scene's mood.",
                f.story.as_ref().map(|s| vec![(glyphs::STORY, format!("{} passages", s.segments.len()))]).unwrap_or_default(),
            ),
            Phase::Audio => ("Narrate the script with a local voice. Listen to each page and regenerate any that sound off.", vec![(ph::WAVEFORM, format!("{} clips", f.status.audio_files))]),
            Phase::Music => (
                "Your own score, composed for this chapter: it follows each scene, builds into fights, drops before reveals and ducks under the narrator.",
                self.music.score.as_ref().map(|s| vec![(glyphs::MUSIC, format!("{} cues", s.cues.len()))]).unwrap_or_default(),
            ),
            Phase::Video => (
                "Pages and panels follow the voice-over, in sync, as a recap video.",
                if f.status.video.is_some() { vec![(ph::CHECK_CIRCLE, "Rendered".to_string())] } else { vec![] },
            ),
        };
        let story_title = (phase == Phase::Story).then(|| f.story.as_ref().map(|s| s.title.clone()).filter(|t| !t.is_empty())).flatten();
        let breadcrumb = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .text_size(px(size::XS))
            .text_color(Tokens::text_3())
            .child(icon_ph(glyphs::MANGA, Tokens::text_3(), icon_px::XS))
            .child(div().truncate().child(f.item.manga_title.clone()))
            .child(icon_ph(ph::CARET_RIGHT, Tokens::text_3(), icon_px::XS))
            .child(div().flex_none().child(f.item.label()));
        // On a narrow centre (a 1280px window with both side columns open) the actions wrap
        // under the title instead of the title's pills running under the buttons.
        let title = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(260.))
            .gap(px(space::XS))
            .child(breadcrumb)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.))
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(32.))
                            .rounded(px(radius::MD))
                            .bg(Tokens::accent_soft())
                            .child(ph_icon(phase_icon(phase), Weight::Duotone, Tokens::accent(), icon_px::LG)),
                    )
                    .child(div().flex_none().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(phase.label()))
                    .when_some(story_title, |d, t| d.child(div().min_w_0().truncate().text_size(px(size::MD)).text_color(Tokens::text_2()).child(t)))
                    .children(stats.into_iter().map(|(i, t)| stat_pill(i, t))),
            );
        let actions = div().flex().flex_wrap().items_center().gap(px(space::SM));
        let has_read = f.status.read_pages > 0;
        let actions = match phase {
            Phase::Read => match run.as_ref().filter(|r| r.kind == RunKind::Read) {
                Some(_) => actions.child(stop_button(cx)),
                // Part-read (usually a stopped read): pick up where it stopped, or start over.
                None if self.partly_read() => actions
                    .child(self.model_button.clone())
                    .child(
                        btn("read-restart", "Start over", ButtonKind::Ghost)
                            .icon(glyphs::RETRY)
                            .tooltip("Read every page again from page 1")
                            .when_some(blocker, |b, r| b.disabled(r))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(true, cx))),
                    )
                    .child(
                        primary("read-continue", ph::PLAY, if self.selected.len() > 1 { self.target_label("Continue") } else { format!("Continue from page {}", f.status.read_pages + 1) })
                            .when_some(blocker, |b, r| b.disabled(r))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(false, cx))),
                    ),
                None => actions
                    .child(self.model_button.clone())
                    .when(has_read, |d| {
                        d.child(
                            btn("read-again", "Read again", ButtonKind::Ghost)
                                .icon(glyphs::RETRY)
                                .when_some(blocker, |b, r| b.disabled(r))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(true, cx))),
                        )
                    })
                    .child(primary("read-it", glyphs::READ, self.target_label("Read")).when_some(blocker, |b, r| b.disabled(r)).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(false, cx)))),
            },
            Phase::Transcript => actions.child(
                btn("to-story", "Continue to Story", ButtonKind::Secondary).trailing_icon(ph::ARROW_RIGHT).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_phase(Phase::Story, cx))),
            ),
            Phase::Story => match run.as_ref().filter(|r| r.kind == RunKind::Story) {
                Some(_) => actions.child(stop_button(cx)),
                None => {
                    let reason = blocker.or((!self.targets().iter().any(|c| self.statuses.get(&c.id).is_some_and(|s| s.read_pages > 0))).then_some("Read the chapter first (Read step)"));
                    actions.child(self.model_button.clone()).child(
                        primary("write-story", glyphs::STORY, self.target_label(if f.story.is_some() { "Rewrite story" } else { "Write story" }))
                            .when_some(reason, |b, r| b.disabled(r))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_story(cx))),
                    )
                }
            },
            Phase::Audio => {
                let job = self.jobs.iter().find(|j| j.chapter_id == f.item.id && !j.state.finished()).cloned();
                match job {
                    // A render of this chapter: Audio waits for it (its Cancel lives on Video).
                    Some(j) if is_render(&j) => actions.child(
                        primary("gen-audio", glyphs::NARRATE, self.target_label(if f.status.audio_files > 0 { "Regenerate all" } else { "Generate audio" }))
                            .disabled("Wait for the video render to finish (Video step)"),
                    ),
                    Some(j) => actions.child(btn("audio-cancel", "Cancel narration", ButtonKind::Ghost).icon(ph::STOP_CIRCLE).on_click(move |_, _, _| {
                        service::cancel(j.id);
                    })),
                    None => {
                        let no_story = self.targets().iter().any(|c| !self.statuses.get(&c.id).is_some_and(|s| s.story));
                        actions.child(
                            primary("gen-audio", glyphs::NARRATE, self.target_label(if f.status.audio_files > 0 { "Regenerate all" } else { "Generate audio" }))
                                .when(no_story, |b| b.disabled("Write the story first (Story step)"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.run_pipeline(&["tts"], true, cx))),
                        )
                    }
                }
            }
            Phase::Music => actions.child(self.compose_button(cx)),
            Phase::Video => actions.child(self.render_button(cx)),
        };
        // Reset this step and the steps built from it (they would describe work that is gone).
        let reset_reason = self.reset_blocker(&f.item.id).or((!self.has_work_from(phase)).then_some("Nothing to reset yet"));
        let reset = icon_button(
            ("reset-phase", phase.index()),
            ph::ARROW_COUNTER_CLOCKWISE,
            if phase == Phase::Video { "Reset Video".to_string() } else { format!("Reset {} and the steps built from it", phase.label()) },
        )
            .danger()
            .when_some(reset_reason, |b, r| b.disabled(r))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.confirm_reset_phase(phase, window, cx)));
        let mut head = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(space::MD))
            .px(px(space::XL))
            .pt(px(space::LG))
            .pb(px(14.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(div().flex().flex_wrap().items_end().gap(px(space::LG)).child(title).child(actions.child(div().w(px(1.)).h(px(20.)).bg(Tokens::border_strong())).child(reset)))
            .child(div().max_w(px(820.)).text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(desc));
        if phase == Phase::Story {
            head = head.child(self.style_chips(cx));
        }
        if let Some(r) = run.filter(|r| matches!((r.kind, phase), (RunKind::Read, Phase::Read) | (RunKind::Story, Phase::Story))) {
            let label = format!("{} · {}", r.status_line(), r.model);
            head = head.child(progress_line("head-run", r.progress(), label));
        }
        if let Some(j) = self.jobs.iter().find(|j| j.chapter_id == f.item.id && !j.state.finished()).filter(|_| matches!(phase, Phase::Audio | Phase::Video)) {
            head = head.child(progress_line("head-job", j.progress() as f32, j.status.clone()));
        }
        // What stops this step for these chapters, with the fix, under the (then disabled) button.
        head.children(self.step_issues(cx))
    }

    fn style_chips(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(div().flex().items_center().gap(px(6.)).mr(px(space::XS)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(icon_ph(ph::MASK_HAPPY, Tokens::text_3(), icon_px::SM)).child("Feel"))
            .children(STYLES.iter().enumerate().map(|(i, &s)| {
                let on = self.prefs.styles.iter().any(|x| x == s);
                filter_chip(("style", i), s, on).icon(feel_icon(s)).tooltip(tip(sentence(mv_recap::style_hint(s)))).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if on {
                        this.prefs.styles.retain(|x| x != s);
                    } else {
                        this.prefs.styles.push(s.to_string());
                    }
                    let styles = this.prefs.styles.clone();
                    Prefs::update(|p| p.styles = styles);
                    cx.notify();
                }))
            }))
            .when(self.prefs.styles.is_empty(), |d| d.child(muted("None picked: follows each scene").ml(px(space::XS))))
    }

    // ------------------------------------------------------------ Read

    fn read_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let f = self.focus.as_ref().expect("focus");
        let Some((path, ratio)) = f.pages.get(ix).cloned() else { return div().into_any_element() };
        let read = f.read.as_ref().and_then(|r| r.pages.iter().find(|p| p.index == ix)).cloned();
        let reading_now = self.run.as_ref().is_some_and(|r| r.running() && r.kind == RunKind::Read && r.chapter_id.as_deref() == Some(f.item.id.as_str()) && r.reading.contains(&ix));
        let faces = run::brain_snapshot(&f.item.manga_id, &f.item.manga_title);
        let blocker = self.run_blocker();
        let stacked = self.center_w < STACK_BELOW;
        let notes = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .flex_none()
            .p(px(space::LG))
            .rounded(px(radius::LG))
            .bg(Tokens::surface())
            .border_1()
            .border_color(if reading_now { Tokens::accent() } else { Tokens::border() })
            .map(|d| if stacked { d.w_full().max_w(px(PAGE_MAX_W)) } else { d.w(px(300.)) })
            .map(|d| match &read {
                None if reading_now => d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .text_size(px(size::SM))
                        .child(spinner(SharedString::from(format!("pg-read-{ix}")), Tokens::accent(), icon_px::SM))
                        .child(shimmer_text("Reading this page…")),
                ),
                None => d.child(div().flex().items_center().gap(px(space::SM)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(icon_ph(ph::CIRCLE_DASHED, Tokens::text_3(), icon_px::SM)).child("Not read yet")),
                Some(p) => d.child(page_notes(ix, p, &faces)),
            })
            .when(read.is_some() && !reading_now, |d| {
                d.child(
                    btn(("reread", ix), "Read this page again", ButtonKind::Ghost)
                        .small()
                        .icon(glyphs::READ)
                        .when_some(blocker, |b, r| b.disabled(r))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reread_page(ix, cx))),
                )
            });
        div().flex().when(stacked, |d| d.flex_col()).items_start().gap(px(space::LG)).child(page_image(ix, &path, ratio, PAGE_MAX_W, reading_now)).child(notes).into_any_element()
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
        let focused = editor.read(cx).focus_handle(cx).is_focused(window);
        let asking_here = self.ask == Some(ix);
        let busy = self.asking.as_ref().is_some_and(|(c, p)| *p == ix && self.focus_id.as_ref() == Some(c));
        let ask_empty = self.ask_input.read(cx).value().trim().is_empty();
        div()
            .flex()
            .items_start()
            .gap(px(space::LG))
            .child(page_image(ix, &path, ratio, 240., false))
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
                            .items_center()
                            .gap(px(space::SM))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format!("Page {}", ix + 1)))
                            .children(scene.filter(|s| !s.kind.is_empty()).map(|s| scene_tag(("tr-scene", ix), &s)))
                            // An edit is a normal, intended state: neutral, not a warning colour.
                            .when(edited, |d| d.child(icon_tag(glyphs::EDIT, "Edited", Tone::Neutral)))
                            .child(div().flex_1())
                            .when(edited, |d| {
                                d.child(
                                    btn(("revert", ix), "Revert to AI text", ButtonKind::Ghost)
                                        .small()
                                        .icon(ph::ARROW_U_UP_LEFT)
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reset_transcript_page(ix, cx))),
                                )
                            })
                            .child(btn(("ask", ix), "Ask AI", ButtonKind::Ghost).small().icon(ph::SPARKLE).selected(asking_here).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
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
                    .child(field_frame(("tr-edit", ix), focused).text_size(px(size::MD)).child(Textarea::new(&editor).appearance(false).bordered(false)))
                    .when(asking_here, |d| {
                        d.child(
                            div()
                                .id(("ask-box", ix))
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .p(px(space::SM))
                                .rounded(px(radius::MD))
                                .bg(Tokens::accent_soft())
                                .border_1()
                                .border_color(Tokens::accent_border())
                                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                                    if ev.keystroke.key == "escape" {
                                        this.ask = None;
                                        cx.stop_propagation();
                                        cx.notify();
                                    }
                                }))
                                .child(icon_ph(ph::SPARKLE, Tokens::accent(), icon_px::SM))
                                .child(div().flex_1().min_w_0().child(Input::new(&self.ask_input).small()))
                                .child(kbd("enter"))
                                .child(
                                    btn(("rewrite", ix), if busy { "Rewriting" } else { "Rewrite" }, ButtonKind::Primary)
                                        .small()
                                        .icon(ph::MAGIC_WAND)
                                        .loading(busy)
                                        .when(ask_empty && !busy, |b| b.disabled("Say what to change first"))
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.rewrite_page(ix, cx))),
                                )
                                .child(icon_button(("ask-close", ix), ph::X, "Close").small().kbd("escape").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.ask = None;
                                    cx.notify();
                                }))),
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
        let focused = editor.read(cx).focus_handle(cx).is_focused(window);
        div()
            .flex()
            .items_start()
            .gap(px(14.))
            .child(div().w(px(150.)).flex_none().rounded(px(radius::MD)).overflow_hidden().bg(Tokens::surface()).border_1().border_color(Tokens::border()).children(thumb.map(|p| img(p).w_full())))
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
                            .gap(px(space::SM))
                            .child(icon_ph(ph::LAYOUT, Tokens::text_3(), icon_px::XS))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(match seg.panel {
                                Some(n) => format!("Page {} · panel {}", seg.page + 1, n + 1),
                                None => format!("Page {}", seg.page + 1),
                            }))
                            .children(scene.filter(|s| !s.kind.is_empty()).map(|s| scene_tag(("st-scene", ix), &s)))
                            .child(div().flex_1())
                            .child(self.delivery_menu(ix, seg.delivery.clone(), cx)),
                    )
                    .child(field_frame(("st-edit", ix), focused).text_size(px(size::LG)).line_height(px(24.)).child(Textarea::new(&editor).appearance(false).bordered(false))),
            )
            .into_any_element()
    }

    /// How the narrator delivers one passage: follow the scene, or one of the presets.
    fn delivery_menu(&self, ix: usize, current: Option<Delivery>, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let pick = move |d: Option<Delivery>| {
            let weak = weak.clone();
            move |_: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| this.set_delivery(ix, d.clone(), cx));
            }
        };
        let mut items = vec![MenuItem::heading("Narrator delivery"), MenuItem::new("Follow the scene", pick(None)).icon(ph::MASK_HAPPY).checked(current.is_none()), MenuItem::separator()];
        for &(label, kind, intensity, icon) in DELIVERIES {
            let d = Delivery { kind: kind.into(), intensity };
            let on = current.as_ref() == Some(&d);
            items.push(MenuItem::new(format!("{label} · {intensity}"), pick(Some(d))).icon(icon).checked(on));
        }
        let (icon, label) = match current.as_ref() {
            None => (ph::MASK_HAPPY, "Auto".to_string()),
            Some(d) => match delivery_preset(d) {
                Some((label, _, i, icon)) => (*icon, format!("{label} · {i}")),
                None => (ph::MASK_HAPPY, format!("{} · {}", d.kind, d.intensity)),
            },
        };
        menu_button(("delivery", ix), icon, "How the narrator delivers this passage (applies the next time it is narrated)", items).label(label).small().border_1().border_color(Tokens::border()).into_any_element()
    }

    // ------------------------------------------------------------ Audio

    fn voice_card(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let env = self.setup.read(cx).env.clone();
        let engines = env.as_ref().map(|e| e.engines.clone()).unwrap_or_default();
        // The default comes from Settings → Audio; a chapter may have its own voice.
        let default = self.prefs.voice.clone().unwrap_or_default();
        let own = self.focus_id.as_deref().and_then(mv_studio::voices::chapter_voice);
        let voice = own.clone().unwrap_or_else(|| default.clone());
        let default_label = mv_studio::tts::find_voice(&default).map(|v| v.label).unwrap_or("none");
        let ticked = self.selected.len();
        // "Use default" works on the chapters a pick applies to, so it shows when one of them
        // has its own voice (the focused chapter may not be among the ticked ones).
        let targets_own = self.targets().iter().any(|c| mv_studio::voices::chapter_voice(&c.id).is_some());
        let (focus_label, focus_id, output_dir) = {
            let f = self.focus.as_ref().expect("focus");
            (f.item.label(), f.item.id.clone(), f.item.output_dir.clone())
        };
        // The slider shows the focused chapter's strength.
        if self.strength_for.as_deref() != Some(focus_id.as_str()) {
            let v = mv_studio::voices::emotion_strength(&output_dir);
            self.strength_value = v;
            self.strength_for = Some(focus_id.clone());
            self.strength.update(cx, |s, cx| s.set_value(v, window, cx));
        }
        let scope = match (ticked, own.is_some()) {
            (0, true) => format!("{focus_label} has its own voice. Default: {default_label}."),
            (0, false) => format!("Picking a voice changes {focus_label} only. Default: {default_label}."),
            (n, _) => format!("Picking a voice changes the {n} ticked chapter{}.", plural(n)),
        };
        let lang_ix = if self.lang == "hi" { 1 } else { 0 };
        let langs = segmented("voice-lang", [(Some(glyphs::LANGUAGE), "English"), (None, "Hindi")], lang_ix).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.lang = if *ix == 1 { "hi" } else { "en" };
            cx.notify();
        }));
        let speed_ix = SPEEDS.iter().position(|s| (*s - self.prefs.speed).abs() < 1e-6).unwrap_or(1);
        let speed_labels: Vec<String> = SPEEDS.iter().map(|s| format!("{s}×")).collect();
        let speeds = segmented("voice-speed", speed_labels.iter().map(|s| s.as_str()), speed_ix).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            let s = SPEEDS[(*ix).min(SPEEDS.len() - 1)];
            this.prefs.speed = s;
            Prefs::update(|p| p.speed = s);
            cx.notify();
        }));
        let playing = audio::now_playing();
        let tiles = div().flex().flex_wrap().gap(px(space::SM)).children(setup::voices(self.lang).into_iter().enumerate().map(|(i, v)| {
            let on = voice == v.id;
            let ready = setup::voice_ready(&engines, v);
            let checking = env.is_none();
            let usable = !checking && ready.usable();
            // Downloaded voice files without the engine that speaks them are not "not
            // downloaded" (the contradiction Settings → Audio and Studio used to show).
            let engine_missing = engines.voice_status(v.id).and_then(|s| s.missing) == Some(setup::Missing::Engine);
            let (status_icon, status, color) = match ready {
                _ if checking => (ph::CIRCLE_NOTCH, "Checking…".to_string(), Tokens::text_3()),
                VoiceReady::Ready(e) => (ph::CHECK_CIRCLE, format!("Ready · {e}"), Tokens::success()),
                VoiceReady::Fallback(e) => (ph::WARNING, format!("{e} stands in for now"), Tokens::warning()),
                VoiceReady::Missing if engine_missing => (ph::PACKAGE, "Engine not installed".to_string(), Tokens::text_3()),
                VoiceReady::Missing => (ph::DOWNLOAD_SIMPLE, "Not downloaded".to_string(), Tokens::text_3()),
            };
            let id = v.id;
            let previewing = self.previewing.as_deref() == Some(id);
            let sample_playing = playing.as_ref().is_some_and(|p| p.to_string_lossy().contains(&mv_studio::tts::safe_voice_name(id)));
            let expressive = mv_studio::tts::is_expressive(id);
            let tile_tip = match (usable, engine_missing) {
                (true, _) => format!("{} · {}", v.label, v.desc),
                (false, true) => format!("{}: its voice files are here; set up its engine in Settings → Audio first", v.label),
                (false, false) => format!("{}: download it in Settings → Audio first", v.label),
            };
            div()
                .id(("voice", i))
                .flex()
                .items_center()
                .gap(px(10.))
                .flex_1()
                .min_w(px(220.))
                .max_w(px(320.))
                .px(px(10.))
                .py(px(space::SM))
                .rounded(px(radius::LG))
                .border_1()
                .role(Role::RadioButton)
                .aria_selected(on)
                .tooltip(tip(tile_tip))
                .map(|d| if on { d.border_color(Tokens::accent()).bg(Tokens::accent_soft()) } else { d.border_color(Tokens::border()) })
                .when(!usable, |d| d.opacity(0.6))
                .when(usable && !on, |d| d.hover(|s| s.bg(Tokens::hover()).border_color(Tokens::border_strong())).pressable())
                .child(match (previewing, usable) {
                    (true, _) => div().flex().flex_none().items_center().justify_center().size(px(28.)).child(spinner(("voice-spin", i), Tokens::text_1(), icon_px::SM)).into_any_element(),
                    (false, true) => icon_button(("sample", i), if sample_playing { ph::STOP } else { ph::PLAY }, if sample_playing { "Stop the sample" } else { "Play a sample" })
                        .small()
                        .selected(sample_playing)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.sample(id, cx);
                        }))
                        .into_any_element(),
                    (false, false) => icon_button(("sample", i), ph::PLAY, "Play a sample")
                        .small()
                        .disabled(if engine_missing { "Set up this voice's engine first" } else { "Download this voice first" })
                        .into_any_element(),
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(v.label))
                                .when(expressive, |d| d.child(icon_tag(ph::MASK_HAPPY, "Expressive", Tone::Accent))),
                        )
                        .child(div().flex().items_center().gap(px(space::XS)).text_size(px(size::XS)).text_color(color).child(icon_ph(status_icon, color, icon_px::XS)).child(div().truncate().child(status)))
                        // What the voice is, so a pick needs no guessing (ElevenLabs voice library labels).
                        .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(v.desc)),
                )
                // Selected: a filled disc with a check (Fluent 2: filled marks the selection).
                .when(on, |d| d.child(selected_badge()))
                .when(!usable && !checking && !on, |d| {
                    let (label, tip) = if engine_missing { ("Set up", "Install this voice's engine in Settings → Audio") } else { ("Get", "Download this voice in Settings → Audio") };
                    d.child(btn(("get-voice", i), label, ButtonKind::Ghost).small().icon(glyphs::DOWNLOAD).tooltip(tip).on_click(|_, w, cx| {
                        mv_ui::navigate(Route::Settings { tab: Some("audio".into()) }, w, cx);
                    }))
                })
                .when(usable, |d| {
                    d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let chapters: Vec<crate::data::ChapterItem> = this.targets();
                        let default = this.prefs.voice.clone().unwrap_or_default();
                        let label = mv_studio::tts::find_voice(id).map(|v| v.label).unwrap_or(id);
                        if chapters.is_empty() {
                            this.prefs.voice = Some(id.to_string());
                            if let Err(e) = mv_studio::voices::set_default_voice(id) {
                                this.fail(format!("Could not save the voice: {e}"));
                            }
                        }
                        let mut saved = 0;
                        for ch in &chapters {
                            // Picking the default again clears the chapter's own choice.
                            match mv_studio::voices::set_chapter_voice(&ch.id, (id != default).then_some(id)) {
                                Ok(()) => saved += 1,
                                Err(e) => this.fail(format!("Could not save the voice: {e}")),
                            }
                        }
                        let what = match (saved, chapters.first()) {
                            (1, Some(c)) => c.label(),
                            (0, _) => "new chapters".to_string(),
                            (n, _) => format!("{n} chapters"),
                        };
                        toast::show(cx, Toast::success(format!("Narrator set to {label} for {what}")).icon(glyphs::VOICE));
                        cx.notify();
                    }))
                })
        }));
        let narrated = self.narrator_line(&focus_id, cx);
        let strength = self.strength_value;
        let expressive_voice = mv_studio::tts::is_expressive(&voice);
        let row_label = |icon: Ph, text: &'static str| div().flex().flex_none().items_center().gap(px(6.)).w(px(150.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(icon_ph(icon, Tokens::text_3(), icon_px::SM)).child(text);
        card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(icon_ph(ph::MICROPHONE_STAGE, Tokens::text_2(), icon_px::LG))
                    .child(div().flex_none().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Narrator"))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_3()).child(scope))
                    .when(targets_own, |d| {
                        d.child(btn("voice-use-default", "Use default", ButtonKind::Ghost).small().icon(ph::ARROW_U_UP_LEFT).tooltip(format!("Narrate with {default_label}")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            for ch in this.targets() {
                                let _ = mv_studio::voices::set_chapter_voice(&ch.id, None);
                            }
                            toast::show(cx, Toast::info("Back to the default narrator").icon(glyphs::VOICE));
                            cx.notify();
                        })))
                    })
                    .child(langs),
            )
            .children(narrated)
            .child(tiles)
            .child(div().flex().items_center().gap(px(space::MD)).pt(px(space::XS)).child(row_label(ph::GAUGE, "Speed")).child(speeds))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(row_label(ph::MASK_HAPPY, "Emotion strength"))
                    .child(div().w(px(200.)).flex_none().child(Slider::new(&self.strength)))
                    .child(div().w(px(40.)).flex_none().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format!("{strength:.1}×")))
                    .child(muted(match (strength, expressive_voice) {
                        (s, _) if s < 0.05 => "Flat: no scene emotion at all.".to_string(),
                        (_, true) => format!("How strongly the voice acts out each scene in {focus_label}. 1× follows the story as read."),
                        (_, false) => "Chatterbox voices act it out; Kokoro and Piper only change their pace.".to_string(),
                    })),
            )
            .into_any_element()
    }

    /// Who narrates the focused chapter: the running job's report, else the last narration's.
    fn narrator_line(&self, chapter_id: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let f = self.focus.as_ref()?;
        let live = self.jobs.iter().find(|j| j.chapter_id == chapter_id && !j.state.finished()).and_then(|j| j.narrator.clone());
        let (engine, voice, synth, stand_in, why) = match (live, &f.narrated) {
            (Some(n), _) => (n.engine_label().to_string(), n.voice.clone(), n.synth_voice.clone(), n.stand_in, None),
            (None, Some(n)) => (
                mv_studio::tts::engine_label(&n.engine).to_string(),
                n.voice.clone(),
                n.synth_voice.clone(),
                !n.synth_voice.is_empty() && n.synth_voice != n.voice,
                n.chatterbox_error.clone(),
            ),
            (None, None) => return None,
        };
        let name = |id: &str| mv_studio::tts::find_voice(id).map(|v| v.label.to_string()).unwrap_or_else(|| id.to_string());
        let el = if stand_in {
            let reason = why.map(|e| format!(" Chatterbox could not run: {e}")).unwrap_or_default();
            inline_alert(AlertKind::Warning, format!("Narrated by {engine}: {} stood in for {}.", name(&synth), name(&voice)), reason)
                .action(
                    "Open Setup",
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.show_setup = true;
                        cx.notify();
                    }),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(icon_tag(ph::CHECK_CIRCLE, format!("Narrated by {engine}"), Tone::Success))
                .child(muted(name(&synth)))
                .into_any_element()
        };
        Some(el)
    }

    fn audio_row(&mut self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let f = self.focus.as_ref().expect("focus");
        let Some((page, text)) = audio_pages(f).into_iter().nth(i) else { return div().into_any_element() };
        let wav = f.item.audio_dir().join(format!("page_{page:04}.wav"));
        let exists = f.clips.contains_key(&page) || std::fs::metadata(&wav).map(|m| m.len() > 100).unwrap_or(false);
        let playing = audio::now_playing().as_deref() == Some(wav.as_path());
        let busy_job = self.jobs.iter().find(|j| j.chapter_id == f.item.id && !j.state.finished());
        let regen_here = self.regenerating.as_ref() == Some(&(f.item.id.clone(), page));
        let secs = f.clips.get(&page).copied();
        let take = f.takes.get(&page).copied().unwrap_or(0);
        // The delivery the user directed for this page's passages (Story step), shown where it is heard.
        let delivery = f.story.as_ref().and_then(|s| s.segments.iter().filter(|s| s.page == page).find_map(|s| s.delivery.clone()));
        let thumb = f.pages.get(page).map(|p| p.0.clone());
        // From memory, not prefs.json / config.json: rows redraw every animation frame while a
        // job's spinner turns. The same default the voice card shows.
        let voice = mv_studio::voices::chapter_voice(&f.item.id).or_else(|| self.prefs.voice.clone()).unwrap_or_default();
        let expressive_ready = self
            .setup
            .read(cx)
            .env
            .as_ref()
            .zip(mv_studio::tts::find_voice(&voice))
            .is_some_and(|(e, v)| mv_studio::tts::is_expressive(v.id) && setup::voice_ready(&e.engines, v) == VoiceReady::Ready("Chatterbox"));
        let progress = playing.then(|| audio::elapsed(&wav).zip(secs).map(|(t, s)| if s > 0. { t / s } else { 0. })).flatten();
        let w2 = wav.clone();
        let busy_reason = busy_job.map(|j| if is_render(j) { "Wait for the video render to finish" } else { "Wait for the narration to finish" });
        let take_reason = busy_reason.or((!exists).then_some("Narrate this page first")).or((!expressive_ready).then_some(
            "Kokoro and Piper read a text the same way every time. A new take needs a Chatterbox voice.",
        ));
        div()
            .flex()
            .items_center()
            .gap(px(space::MD))
            .p(px(10.))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if playing { Tokens::accent() } else { Tokens::border() })
            .when(playing, |d| d.bg(Tokens::accent_soft()))
            .child(if exists {
                icon_button(("play", page), if playing { ph::STOP } else { ph::PLAY }, if playing { "Stop".to_string() } else { format!("Play page {}", page + 1) })
                    .selected(playing)
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        audio::toggle(&w2);
                        cx.notify();
                    }))
                    .into_any_element()
            } else {
                div().id(("no-clip", page)).flex().flex_none().items_center().justify_center().size(px(32.)).tooltip(tip("No audio yet")).child(icon_ph(ph::CIRCLE_DASHED, Tokens::text_3(), icon_px::MD)).into_any_element()
            })
            .child(div().w(px(44.)).h(px(60.)).flex_none().rounded(px(radius::SM)).overflow_hidden().bg(Tokens::surface()).children(thumb.map(|p| img(p).size_full().object_fit(ObjectFit::Cover))))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(space::XS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .text_size(px(size::XS))
                            .text_color(Tokens::text_3())
                            .child(div().font_weight(FontWeight::MEDIUM).text_color(Tokens::text_2()).child(format!("Page {}", page + 1)))
                            .when_some(secs, |d, s| d.child(fmt_secs(s)))
                            .when(!exists && !regen_here, |d| d.child("No audio yet"))
                            .when_some(delivery, |d, dl| {
                                let (icon, label) = match delivery_preset(&dl) {
                                    Some((label, _, i, icon)) => (*icon, format!("{label} · {i}")),
                                    None => (ph::MASK_HAPPY, format!("{} · {}", dl.kind, dl.intensity)),
                                };
                                d.child(icon_tag(icon, label, Tone::Accent))
                            })
                            .when(take > 0, |d| d.child(icon_tag(ph::SHUFFLE, format!("Take {}", take + 1), Tone::Neutral)))
                            .when(regen_here, |d| d.child(spinner(("regen-spin", page), Tokens::accent(), icon_px::XS)).child(shimmer_text("Voicing this page…"))),
                    )
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).line_clamp(2).child(text))
                    .when_some(progress, |d, p| d.child(progress_bar(("clip-progress", page), p, false))),
            )
            .child(
                icon_button(("regen", page), ph::ARROWS_CLOCKWISE, if exists { format!("Voice page {} again with its current text and delivery", page + 1) } else { format!("Narrate page {}", page + 1) })
                    .small()
                    .when_some(busy_reason, |b, r| b.disabled(r))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.regenerate_page(page, false, cx))),
            )
            .child(
                icon_button(("take", page), ph::SHUFFLE, "New take: a different performance of this page")
                    .small()
                    .when_some(take_reason, |b, r| b.disabled(r))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.regenerate_page(page, true, cx))),
            )
            .into_any_element()
    }

    /// Voices one page of the focused chapter again (the rest are reused). `new_take` moves the
    /// page to its next take, so Chatterbox performs it afresh. The old clip, and its take
    /// number, stay until the new clip is good (tts_bridge swaps and records it).
    pub(crate) fn regenerate_page(&mut self, page: usize, new_take: bool, cx: &mut Context<Self>) {
        let Some(f) = &self.focus else { return };
        let item = f.item.clone();
        self.regenerating = Some((item.id.clone(), page));
        self.run_pipeline_with(vec![item], &["tts"], false, vec![page], new_take, cx);
    }

    // ------------------------------------------------------------ Video

    fn render_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let f = self.focus.as_ref().expect("focus");
        let job = self.jobs.iter().find(|j| j.chapter_id == f.item.id && !j.state.finished()).cloned();
        // Only a render job is cancelled from here; narration still running waits (its Cancel
        // is on the Audio step, and "Cancel render" would have stopped the narration).
        let narrating = job.as_ref().is_some_and(|j| !is_render(j));
        if let Some(j) = job.filter(is_render) {
            let id = j.id;
            return btn("video-cancel", "Cancel render", ButtonKind::Secondary)
                .icon(ph::STOP_CIRCLE)
                .on_click(move |_, _, _| {
                    service::cancel(id);
                })
                .into_any_element();
        }
        let reason = narrating.then_some("Wait for the narration to finish (Audio step)").or(self.render_blocker(cx).map(|(t, _)| t));
        primary("render-video", glyphs::VIDEO, self.target_label(if f.status.video.is_some() { "Render again" } else { "Render video" }))
            .when_some(reason, |b, r| b.disabled(r))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.render_video(cx)))
            .into_any_element()
    }

    /// Why the video cannot render yet, and the step that fixes it.
    fn render_blocker(&self, cx: &App) -> Option<(&'static str, NoticeAction)> {
        let f = self.focus.as_ref()?;
        let env = self.setup.read(cx).env.as_ref();
        if f.status.audio_files == 0 {
            Some(("Generate the audio first (Audio step)", NoticeAction::GoTo(Phase::Audio)))
        } else if env.is_some_and(|e| !e.ffmpeg) {
            Some(("FFmpeg is needed to render video", NoticeAction::OpenSetup))
        } else {
            None
        }
    }

    fn video_card(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let f = self.focus.as_ref().expect("focus");
        // The caption studio (crate::video): preview, Watchfiwn caption templates and placement.
        let captions = crate::video::panel(
            crate::video::VideoContext {
                chapter: f.item.clone(),
                pages: f.pages.iter().map(|p| p.0.clone()).collect(),
                passages: f.story.as_ref().map(|s| s.segments.iter().map(|g| (g.page, g.text.clone())).collect()).unwrap_or_default(),
                reels: self.prefs.format == mv_recap::FORMAT_REELS,
            },
            window,
            cx,
        );
        let f = self.focus.as_ref().expect("focus");
        let video = f.status.video.clone();
        let meta = f.video.clone();
        let title = format!("{} · {}", f.item.manga_title, f.item.label());
        let res_ix = RESOLUTIONS.iter().position(|r| *r == self.prefs.resolution).unwrap_or(1);
        let res = segmented("video-res", RESOLUTIONS.map(|r| (Some(ph::HIGH_DEFINITION), r)), res_ix).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            let r = RESOLUTIONS[(*ix).min(1)].to_string();
            this.prefs.resolution = r.clone();
            Prefs::update(|p| p.resolution = r);
            cx.notify();
        }));
        let fmt_ix = FORMATS.iter().position(|(id, _, _)| *id == self.prefs.format).unwrap_or(0);
        let formats = segmented("video-format", FORMATS.map(|(_, l, i)| (Some(i), l)), fmt_ix).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            let id = FORMATS[(*ix).min(1)].0.to_string();
            this.prefs.format = id.clone();
            Prefs::update(|p| p.format = id);
            cx.notify();
        }));
        let reels = f.status.reels.clone();
        let blocker = self.render_blocker(cx);
        let bhippi = find_bhippi();
        let setting = |icon: Ph, label: &'static str, control: AnyElement| {
            div().flex().items_center().gap(px(space::MD)).child(div().flex().items_center().gap(px(6.)).w(px(110.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(icon_ph(icon, Tokens::text_3(), icon_px::SM)).child(label)).child(control)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .max_w(px(1180.))
            .child(
                card()
                    .child(setting(ph::FRAME_CORNERS, "Format", formats.into_any_element()))
                    .child(setting(ph::MONITOR, "Resolution", res.into_any_element()))
                    .when(self.prefs.format == mv_recap::FORMAT_REELS, |d| {
                        d.child(muted("Reels render upright (9:16) and cut the recap into parts of about a minute, each ending at a pause in the narration."))
                    }),
            )
            .child(captions)
            .when_some(blocker, |d, (text, action)| {
                d.child(inline_alert(AlertKind::Warning, text, "").action(
                    action_label(action),
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        match action {
                            NoticeAction::GoTo(p) => this.set_phase(p, cx),
                            _ => this.show_setup = true,
                        }
                        cx.notify();
                    }),
                ))
            })
            .map(|d| match video {
                Some(v) if !reels.is_empty() => {
                    let count = reels.len();
                    let thumb = f.pages.first().map(|p| p.0.clone());
                    let dir = reels[0].parent().map(Path::to_path_buf);
                    d.child(div().flex().flex_wrap().gap(px(space::MD)).children(reels.into_iter().enumerate().map(|(i, reel)| {
                        let name = format!("{title} · Reel {} of {count}", i + 1);
                        let secs = reel.file_name().and_then(|n| meta.reels.get(&n.to_string_lossy().to_string())).copied();
                        div()
                            .id(("reel", i))
                            .relative()
                            .w(px(150.))
                            .aspect_ratio(9. / 16.)
                            .rounded(px(radius::LG))
                            .overflow_hidden()
                            .bg(Tokens::canvas())
                            .border_1()
                            .border_color(Tokens::border())
                            .group("reel")
                            .role(Role::Button)
                            .aria_label(SharedString::from(name.clone()))
                            .tooltip(tip(format!("Play reel {} of {count}", i + 1)))
                            .hover(|s| s.border_color(Tokens::border_strong()))
                            .pressable()
                            .children(thumb.clone().map(|p| img(p).size_full().object_fit(ObjectFit::Cover).opacity(0.5)))
                            .child(div().absolute().inset_0().flex().items_center().justify_center().child(play_disc(44.)))
                            .child(
                                div()
                                    .absolute()
                                    .bottom(px(space::SM))
                                    .left(px(space::SM))
                                    .right(px(space::SM))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(size::XS))
                                    .text_color(Tokens::text_1())
                                    .child(div().px(px(6.)).rounded(px(radius::XS)).bg(Tokens::scrim()).child(format!("Reel {}/{count}", i + 1)))
                                    .children(secs.map(|s| div().px(px(6.)).rounded(px(radius::XS)).bg(Tokens::scrim()).child(fmt_secs(s)))),
                            )
                            .on_click(move |_, w, cx| play_video(&name, &reel, w, cx))
                    })))
                    .child(
                        div()
                            .flex()
                            .gap(px(space::SM))
                            .when_some(dir, |d, dir| d.child(btn("reels-folder", "Show reels in folder", ButtonKind::Secondary).icon(ph::FOLDER_OPEN).on_click(move |_, _, cx| cx.open_with_system(&dir))))
                            .when_some(bhippi, |d, exe| {
                                d.child(btn("video-bhippi", "Edit full video in Bhippi", ButtonKind::Ghost).trailing_icon(glyphs::EXTERNAL).on_click(move |_, _, _| {
                                    let _ = std::process::Command::new(&exe).arg(&v).spawn();
                                }))
                            }),
                    )
                }
                Some(v) => {
                    let (v1, v2, v3) = (v.clone(), v.clone(), v.clone());
                    let thumb = f.pages.first().map(|p| p.0.clone());
                    let facts: Vec<(Ph, String)> = [
                        meta.duration.map(|s| (ph::CLOCK, fmt_secs(s))),
                        // The file's own shape, not the Format setting (which may have changed since).
                        Some((ph::FRAME_CORNERS, if meta.upright { "9:16" } else { "16:9" }.to_string())),
                        (meta.bytes > 0).then(|| (glyphs::STORAGE, fmt_bytes(meta.bytes))),
                        meta.modified.map(|t| (ph::CALENDAR_CHECK, format!("Rendered {}", ago(t)))),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    d.child(
                        div()
                            .id("video-result")
                            .relative()
                            .w_full()
                            .aspect_ratio(16. / 9.)
                            .rounded(px(radius::XL))
                            .overflow_hidden()
                            .bg(Tokens::canvas())
                            .border_1()
                            .border_color(Tokens::border())
                            .group("video-result")
                            .role(Role::Button)
                            .aria_label("Play the recap")
                            .tooltip(tip("Play the recap"))
                            .hover(|s| s.border_color(Tokens::border_strong()))
                            .pressable()
                            .children(thumb.map(|p| img(p).size_full().object_fit(ObjectFit::Cover).opacity(0.5)))
                            .child(div().absolute().inset_0().flex().items_center().justify_center().child(play_disc(72.)))
                            .on_click(move |_, w, cx| play_video(&title, &v1, w, cx)),
                    )
                    .child(div().flex().flex_wrap().items_center().gap(px(space::SM)).children(facts.into_iter().map(|(i, t)| stat_pill(i, t))))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(space::SM))
                            .child(btn("video-folder", "Show in folder", ButtonKind::Secondary).icon(ph::FOLDER_OPEN).on_click(move |_, _, cx| {
                                if let Some(dir) = v2.parent() {
                                    cx.open_with_system(dir);
                                }
                            }))
                            .when_some(bhippi, |d, exe| {
                                d.child(btn("video-bhippi", "Edit in Bhippi", ButtonKind::Ghost).trailing_icon(glyphs::EXTERNAL).on_click(move |_, _, _| {
                                    let _ = std::process::Command::new(&exe).arg(&v3).spawn();
                                }))
                            })
                            .child(div().flex_1().min_w(px(160.)).child(path_chip(v.display().to_string()))),
                    )
                }
                None => d.child(div().py(px(space::XXL)).child(
                    empty_view(glyphs::VIDEO, "No video yet", "Render once the audio is ready. Each page or panel stays on screen exactly as long as its narration.").compact(),
                )),
            })
            .into_any_element()
    }

    // ------------------------------------------------------------ pipeline + samples

    /// Queues the Python pipeline for the target chapters with only `stages`.
    pub(crate) fn run_pipeline(&mut self, stages: &[&str], force_regen: bool, cx: &mut Context<Self>) {
        let targets = self.targets();
        self.run_pipeline_with(targets, stages, force_regen, vec![], false, cx);
    }

    /// Queues the pipeline for `targets`; `regen_pages` voices only those pages again (as their
    /// next take when `new_take`).
    pub(crate) fn run_pipeline_with(
        &mut self,
        targets: Vec<crate::data::ChapterItem>,
        stages: &[&str],
        force_regen: bool,
        regen_pages: Vec<usize>,
        new_take: bool,
        cx: &mut Context<Self>,
    ) {
        self.flush_edits(cx);
        let Some(env) = self.setup.read(cx).env.clone() else {
            // Nothing is queued, so the row a per-page Regenerate marked must not spin on.
            self.regenerating = None;
            toast::show(cx, Toast::info("Still checking your setup…"));
            return cx.notify();
        };
        if !env.runnable() {
            self.fail_with("Studio needs Python and the pipeline scripts.", NoticeAction::OpenSetup);
            self.regenerating = None;
            return cx.notify();
        }
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
                self.fail_with(format!("{label} isn't ready yet. Get it in Settings → Audio, or pick a ready voice."), NoticeAction::OpenAudioSettings);
                self.regenerating = None;
                return cx.notify();
            }
        }
        let missing: Vec<String> = targets.iter().filter(|c| !self.statuses.get(&c.id).is_some_and(|s| s.story)).map(|c| c.label()).collect();
        if stages.contains(&"tts") && !missing.is_empty() {
            self.fail_with(format!("Write the story first for: {}", missing.join(", ")), NoticeAction::GoTo(Phase::Story));
            self.regenerating = None;
            return cx.notify();
        }
        let mut queued: anyhow::Result<usize> = Ok(0);
        for (voice, chapters) in by_voice {
            let req = NarrationRequest {
                voice,
                speed: self.prefs.speed,
                video: stages.contains(&"video"),
                resolution: self.prefs.resolution.clone(),
                format: self.prefs.format.clone(),
                stages: Some(stages.iter().map(|s| s.to_string()).collect()),
                force_regen,
                regen_pages: regen_pages.clone(),
                new_take,
            };
            queued = match (queued, service::enqueue(&env, &req, &chapters)) {
                (Ok(a), Ok(b)) => Ok(a + b),
                (Err(e), _) | (_, Err(e)) => Err(e),
            };
        }
        match queued {
            Ok(0) => {
                self.regenerating = None;
                toast::show(cx, Toast::info("Already in progress"));
            }
            Ok(n) => {
                self.notice = None;
                self.jobs = service::snapshot();
                if n > 1 {
                    toast::show(cx, Toast::success(format!("Queued {n} chapter{}", plural(n))).icon(ph::QUEUE));
                }
            }
            Err(e) => {
                self.regenerating = None;
                self.fail(format!("{e:#}"));
            }
        }
        cx.notify();
    }

    fn sample(&mut self, voice: &'static str, cx: &mut Context<Self>) {
        let Some(env) = self.setup.read(cx).env.clone() else { return };
        let (Ok(py), Ok(layout)) = (env.python.clone(), env.scripts.clone()) else { return };
        let speed = self.prefs.speed;
        let cached = mv_studio::tts::preview_cache_path(&mv_studio::tts::default_samples_dir(), voice, mv_studio::tts::default_preview_engine(voice), speed);
        if audio::now_playing().is_some_and(|p| p == cached || p.to_string_lossy().contains(&mv_studio::tts::safe_voice_name(voice))) {
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
                    Ok(Err(e)) => this.fail(format!("The sample failed: {e}")),
                    Err(e) => this.fail(format!("The sample failed: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

// ---------------------------------------------------------------- pieces

/// A render job (sync and video), as opposed to narration.
pub(crate) fn is_render(j: &service::JobInfo) -> bool {
    j.stages.iter().any(|s| s == "video")
}

fn action_label(a: NoticeAction) -> &'static str {
    match a {
        NoticeAction::GoTo(Phase::Audio) => "Go to Audio",
        NoticeAction::GoTo(Phase::Story) => "Go to Story",
        NoticeAction::GoTo(_) => "Go there",
        _ => "Open Setup",
    }
}

/// "upbeat and bright; …" → "Upbeat and bright; …."
fn sentence(hint: &str) -> String {
    let mut c = hint.chars();
    match c.next() {
        Some(f) => format!("{}{}.", f.to_uppercase(), c.as_str()),
        None => String::new(),
    }
}

/// The phase's main action: a primary button with its icon.
pub(crate) fn primary(id: &'static str, icon: Ph, label: String) -> Btn {
    btn(id, label, ButtonKind::Primary).icon(icon)
}

fn stop_button(cx: &mut Context<StudioPage>) -> Btn {
    btn("run-stop", "Stop", ButtonKind::Secondary).icon(ph::STOP_CIRCLE).tooltip("Stop the AI run. Finished pages are kept.").on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
        run::cancel();
        cx.notify();
    }))
}

/// Placeholder for a chapter that is opening: title, description and three pages.
fn center_skeleton() -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap(px(space::MD))
        .px(px(space::XL))
        .pt(px(space::LG))
        .child(skeleton_line(px(140.)).h(px(10.)))
        .child(div().flex().items_center().gap(px(10.)).child(skeleton_block(px(32.), px(32.))).child(skeleton_line(px(180.)).h(px(20.))))
        .child(skeleton_line(relative(0.55)))
        .child(div().h(px(1.)).bg(Tokens::border()))
        .child(div().flex().gap(px(space::LG)).pt(px(space::SM)).children((0..3).map(|_| skeleton_block(px(200.), px(290.)))))
}

/// The accent play disc over video thumbnails; grows a little on hover of its card.
fn play_disc(d: f32) -> Div {
    div().flex().items_center().justify_center().size(px(d)).rounded_full().bg(Tokens::accent()).shadow_lg().child(ph_icon(ph::PLAY, Weight::Fill, Tokens::on_accent(), (d * 0.42).round()))
}

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
        .w_full()
        .max_w(px(max_w))
        .min_w(px(120.))
        .rounded(px(radius::MD))
        .overflow_hidden()
        .bg(Tokens::surface())
        .border_1()
        .border_color(if glow { Tokens::accent() } else { Tokens::border() })
        .map(|d| match ratio {
            Some(r) => d.child(div().w_full().aspect_ratio(r).child(im.size_full())),
            None => d.child(im.w_full()),
        })
        .when(glow, |d| d.child(scan_overlay(SharedString::from(format!("scan-{ix}")))))
        .child(
            div()
                .absolute()
                .top(px(space::SM))
                .left(px(space::SM))
                .px(px(7.))
                .py(px(2.))
                .rounded(px(radius::SM))
                .bg(Tokens::scrim())
                .text_size(px(size::XS))
                .font_weight(FontWeight::MEDIUM)
                .text_color(Tokens::text_1())
                .child(format!("{}", ix + 1)),
        )
}

fn page_notes(ix: usize, p: &PageRead, brain: &mv_recap::Brain) -> Div {
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
        .child(div().flex().flex_wrap().gap(px(5.)).when(!p.scene.kind.is_empty(), |d| d.child(scene_tag(("rd-scene", ix), &p.scene))).children(p.scene.tags.iter().take(4).map(|t| tag(t.clone(), Tone::Neutral))))
        .when(!p.summary.is_empty(), |d| d.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_1()).child(p.summary.clone())))
        .when(!names.is_empty(), |d| {
            d.child(div().flex().flex_wrap().gap(px(6.)).children(names.into_iter().map(|n| {
                let face = brain.find(&n).and_then(|c| c.faces.first().cloned());
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .pl(px(2.))
                    .pr(px(space::SM))
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
                    .pl(px(space::SM))
                    .border_l_2()
                    .border_color(Tokens::border_strong())
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
            None => d.child(ph_icon(ph::USER, Weight::Duotone, Tokens::text_3(), (s * 0.6).round())),
        })
}

/// Spinner, status and percent over an animated bar. `id` keeps each bar's animation apart.
pub(crate) fn progress_line(id: &str, p: f32, label: String) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(spinner(SharedString::from(format!("{id}-spin")), Tokens::accent(), icon_px::SM))
                .child(div().flex_1().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(label))
                .child(div().text_size(px(size::XS)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_2()).child(format!("{}%", (p * 100.).round() as i32))),
        )
        .child(progress_bar(SharedString::from(format!("{id}-bar")), p, true))
}

pub(crate) fn play_video(title: &str, path: &Path, window: &mut Window, cx: &mut App) {
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

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::sentence;

    #[test]
    fn hints_become_sentences() {
        assert_eq!(sentence("upbeat and bright"), "Upbeat and bright.");
        assert_eq!(sentence(""), "");
    }
}
