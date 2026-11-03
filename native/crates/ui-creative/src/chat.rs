//! Chat: the user talks to the director (the AI model picked under the chat box). Its answer is
//! a reply plus ops: writing the series, a chapter or a page happens right away; anything that
//! spends image or video credits becomes a card the user confirms, with the model to use.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::AiMessage;
use mv_create::prompts::{self, Op};
use mv_create::{CardState, ChatLine, Project, Proposal};
use mv_ui::theme::{radius, space};
use mv_ui::{btn, icon_ph, ph, size, ButtonKind, Tokens};

use crate::ai::{self, Brainbox};
use crate::jobs;
use crate::page::CreativePage;

const EXAMPLES: [&str; 3] = [
    "A manga like Dr. Stone, but the hero is my character: Arhan, 17, messy black hair, a scar over his left eyebrow. The twist: instead of science he rebuilds the world with forgotten music.",
    "Dark fantasy in the style of Berserk meets the humour of Spy x Family: a retired knight raising a dragon as his daughter.",
    "A sports manga like Haikyuu, but about competitive street dance in Mumbai.",
];

/// How much chat history the director sees.
const HISTORY: usize = 20;

impl CreativePage {
    pub(crate) fn send_from_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.composer.clone() else { return };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() || self.thinking.is_some() {
            return;
        }
        input.update(cx, |s, cx| s.set_value("", window, cx));
        self.send(text, cx);
    }

    fn library_titles(&self) -> Vec<String> {
        self.library.iter().map(|m| m.title.clone()).take(80).collect()
    }

    pub(crate) fn send(&mut self, text: String, cx: &mut Context<Self>) {
        let ai = match Brainbox::current(self.ai_cancel.clone(), cx) {
            Ok(a) => a,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        self.edit(|p| {
            if p.brief.is_empty() {
                p.brief = text.clone();
            }
            p.chat.push(ChatLine::new("user", text.clone()));
        });
        let Some(p) = self.p.clone() else { return };
        let mut messages = vec![AiMessage::system(prompts::director_system(&p, &self.library_titles()))];
        let start = p.chat.len().saturating_sub(HISTORY);
        for l in &p.chat[start..] {
            messages.push(match l.role.as_str() {
                "user" => AiMessage::user(l.text.clone()),
                "note" => AiMessage::assistant(format!("(app) {}", l.text)),
                _ => AiMessage::assistant(l.text.clone()),
            });
        }
        let status = Arc::new(Mutex::new("Thinking".to_string()));
        self.thinking = Some(status);
        self.error = None;
        self.tick(cx);
        let pid = p.id.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move { ai.json(messages).await }).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.project_id().as_deref() != Some(pid.as_str()) {
                    // Another manga is open now: this answer was for the one before.
                    this.thinking = None;
                    cx.notify();
                    return;
                }
                match res {
                    Ok(v) => {
                        let (reply, ops) = prompts::parse_director(&v);
                        if !reply.is_empty() {
                            this.edit(|p| p.chat.push(ChatLine::new("assistant", reply)));
                        }
                        this.run_ops(ops.into(), cx);
                    }
                    // Stop already said so.
                    Err(e) if e.to_string() == "stopped" => {}
                    Err(e) => {
                        this.thinking = None;
                        this.error = Some(format!("The AI could not answer: {e:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Runs the director's ops one after another (the writing ones call the AI again).
    fn run_ops(&mut self, mut ops: VecDeque<Op>, cx: &mut Context<Self>) {
        let Some(op) = ops.pop_front() else {
            self.thinking = None;
            return;
        };
        let status = self.thinking.clone().unwrap_or_else(|| Arc::new(Mutex::new(String::new())));
        self.thinking = Some(status.clone());
        let set = |s: &str| *status.lock().unwrap_or_else(|e| e.into_inner()) = s.to_string();
        let ai = match Brainbox::current(self.ai_cancel.clone(), cx) {
            Ok(a) => a,
            Err(e) => {
                self.thinking = None;
                self.error = Some(e);
                return;
            }
        };
        let Some(p) = self.p.clone() else { return };
        match op {
            Op::Propose(proposal) => {
                self.propose(proposal);
                self.run_ops(ops, cx);
            }
            Op::Settings { color, rtl, pages_per_chapter } => {
                self.edit(|p| {
                    if let Some(c) = color {
                        p.settings.color = c;
                    }
                    if let Some(r) = rtl {
                        p.settings.rtl = r;
                    }
                    if let Some(n) = pages_per_chapter {
                        p.settings.pages_per_chapter = n;
                    }
                });
                self.run_ops(ops, cx);
            }
            Op::CreateSeries { brief, inspirations } => {
                set("Writing the series bible, the cast and the outline");
                let found: Vec<_> = inspirations.iter().filter_map(|t| self.find_manga(t)).collect();
                let used: Vec<String> = found.iter().map(|m| m.title.clone()).collect();
                let brief2 = brief.clone();
                self.edit(|p| {
                    p.brief = brief2;
                    p.inspirations = inspirations.clone();
                });
                let p = self.p.clone().unwrap_or(p);
                // The manga's notes, its style pages and their import are file work: off the UI thread.
                let fut = async move {
                    let proj = p.clone();
                    let (notes, refs) = tokio::task::spawn_blocking(move || {
                        let notes: String = found.iter().map(|m| inspiration_notes(&m.id, &m.title)).collect::<Vec<_>>().join("\n\n");
                        let style: Vec<std::path::PathBuf> = found.iter().flat_map(|m| mv_create::style_pages(&m.pages_dirs, 2)).take(3).collect();
                        (notes, mv_create::import_refs(&proj, &style, "style").unwrap_or_default())
                    })
                    .await?;
                    let v = ai.ask(prompts::series(&p, &brief, &notes)).await?;
                    anyhow::Ok((v, refs))
                };
                self.ai_step(ops, cx, fut, move |this, (v, refs), _| {
                    this.apply_series(v)?;
                    if !refs.is_empty() {
                        this.edit(|p| p.style_refs = refs);
                    }
                    let p = this.p.clone().unwrap_or_default();
                    let first: Vec<String> = p.cast.iter().filter(|c| c.intro_chapter <= 1).map(|c| c.name.clone()).collect();
                    let mut msg = format!("Series written: “{}”, {} characters, {} chapters outlined.", p.title, p.cast.len(), p.chapters.len());
                    if !used.is_empty() {
                        msg.push_str(&format!(" Style references taken from your downloaded {}.", used.join(", ")));
                    }
                    this.note(msg);
                    this.propose(Proposal::Sheets { names: first, locations: vec![] });
                    Ok(())
                });
            }
            Op::ReviseSeries { instruction } => {
                set("Revising the series");
                let ask = prompts::revise(&p, &instruction);
                self.ai_step(ops, cx, async move { ai.ask(ask).await }, |this, v, _| {
                    this.apply_series(v)?;
                    let n = this.p.as_ref().map(|p| p.cast.len()).unwrap_or(0);
                    this.note(format!("Series updated ({n} characters)."));
                    Ok(())
                });
            }
            Op::WriteChapter { chapter, .. } | Op::EditPage { chapter, .. } if self.chapter_painting(chapter) => {
                // A new script would get the paintings of the old one.
                self.note(format!("Chapter {chapter} is being painted or animated right now; its script can change once that finishes (or press Stop)."));
                self.run_ops(ops, cx);
            }
            Op::WriteChapter { chapter, note } => {
                let st = status.clone();
                self.ai_step(ops, cx, ai::write_chapter(ai, p, chapter, note, st), move |this, (title, summary, pages), _| {
                    let n = pages.len();
                    this.edit(|p| {
                        let ch = p.chapter_mut(chapter);
                        if !title.is_empty() {
                            ch.title = title;
                        }
                        if !summary.is_empty() {
                            ch.summary = summary;
                        }
                        ch.pages = pages;
                    });
                    this.chapter = chapter;
                    this.note(format!("Chapter {chapter} scripted: {n} pages. Read it on the Manga tab."));
                    this.propose(Proposal::Pages { chapter, pages: vec![] });
                    Ok(())
                });
            }
            Op::EditPage { chapter, page, instruction } => {
                set("Rewriting the page");
                let Some(pg) = p.chapter(chapter).and_then(|c| c.pages.iter().find(|x| x.number == page)).cloned() else {
                    self.note(format!("There is no page {page} in chapter {chapter} yet."));
                    return self.run_ops(ops, cx);
                };
                self.ai_step(ops, cx, ai::edit_page(ai, p, chapter, pg, instruction), move |this, new, _| {
                    this.edit(|p| {
                        if let Some(x) = p.chapter_mut(chapter).pages.iter_mut().find(|x| x.number == page) {
                            *x = new;
                        }
                    });
                    this.note(format!("Page {page} of chapter {chapter} rewritten."));
                    this.propose(Proposal::Pages { chapter, pages: vec![page] });
                    Ok(())
                });
            }
        }
    }

    /// Chapter `n` has pages or clips queued or being made.
    pub(crate) fn chapter_painting(&self, n: u32) -> bool {
        let mine = |t: &jobs::Task| matches!(t, jobs::Task::Page { chapter, .. } | jobs::Task::Clip { chapter, .. } | jobs::Task::Stitch { chapter } if *chapter == n);
        self.queue.iter().any(mine) || self.running.as_ref().is_some_and(|r| r.label.starts_with(&format!("Chapter {n} ")))
    }

    /// Runs one AI future, applies its answer, then carries on with the rest of the ops.
    fn ai_step<T: Send + 'static>(&mut self, ops: VecDeque<Op>, cx: &mut Context<Self>, fut: impl std::future::Future<Output = anyhow::Result<T>> + Send + 'static, done: impl FnOnce(&mut Self, T, &mut Context<Self>) -> anyhow::Result<()> + 'static) {
        let pid = self.project_id();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(fut).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                // Written for a manga that is no longer open: drop it and the ops after it.
                if this.project_id() != pid {
                    this.thinking = None;
                    cx.notify();
                    return;
                }
                match res.and_then(|v| done(this, v, cx)) {
                    Ok(()) => this.run_ops(ops, cx),
                    Err(e) if e.to_string() == "stopped" => {}
                    Err(e) => {
                        this.thinking = None;
                        this.note(format!("That did not work: {e:#}. Ask again, or pick another AI model."));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_series(&mut self, v: serde_json::Value) -> anyhow::Result<()> {
        if let Some(p) = self.p.as_mut() {
            prompts::apply_series(p, v)?;
        }
        self.save();
        Ok(())
    }

    /// A card for the chat, unless an open card already offers the same.
    pub(crate) fn propose(&mut self, proposal: Proposal) {
        let Some(p) = self.p.as_ref() else { return };
        if jobs::plan(p, &proposal).is_empty() {
            return;
        }
        if p.chat.iter().any(|l| l.state == CardState::Open && l.proposal.as_ref() == Some(&proposal)) {
            return;
        }
        let text = match &proposal {
            Proposal::Sheets { names, .. } => format!("Paint model sheets and expression sheets for {}", if names.is_empty() { "chapter 1's cast".to_string() } else { names.join(", ") }),
            Proposal::Pages { chapter, pages } if pages.is_empty() => format!("Paint chapter {chapter}"),
            Proposal::Pages { chapter, pages } => format!("Paint chapter {chapter}, page{} {}", if pages.len() == 1 { "" } else { "s" }, pages.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")),
            Proposal::Video { chapter } => format!("Animate chapter {chapter} into a video"),
        };
        self.edit(|p| p.chat.push(ChatLine { proposal: Some(proposal), ..ChatLine::new("card", text) }));
    }

    /// The downloaded manga a title names ("dr stone" finds "Dr. STONE").
    fn find_manga(&self, title: &str) -> Option<crate::page::LibManga> {
        let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
        let t = norm(title);
        if t.len() < 3 {
            return None;
        }
        self.library.iter().find(|m| norm(&m.title) == t).or_else(|| self.library.iter().find(|m| norm(&m.title).contains(&t) || t.contains(&norm(&m.title)))).cloned()
    }

    // ------------------------------------------------------------------ view

    pub(crate) fn chat_tab(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.p.clone().unwrap_or_default();
        let lines = div().flex().flex_col().gap(px(space::MD)).w_full().max_w(px(860.)).mx_auto().children(p.chat.iter().enumerate().map(|(i, l)| self.line(&p, i, l, cx)));
        let empty = p.chat.is_empty();
        let thinking = self.thinking.as_ref().map(|s| s.lock().map(|s| s.clone()).unwrap_or_default());
        let composer = self.composer.clone();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .child(
                div()
                    .id("cr-chat")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.chat_scroll)
                    .child(if empty { self.chat_empty(cx).into_any_element() } else { lines.into_any_element() })
                    .when_some(thinking, |d, t| {
                        d.child(
                            div()
                                .w_full()
                                .max_w(px(860.))
                                .mx_auto()
                                .mt(px(space::MD))
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .child(mv_ui::spinner("cr-think", Tokens::accent(), 14.))
                                .child(mv_ui::shimmer_text(if t.is_empty() { "Thinking".to_string() } else { t }))
                                .child(btn("cr-stop-ai", "Stop", ButtonKind::Ghost).small().icon(ph::STOP).on_click(cx.listener(|this, _, _, cx| this.stop_ai(cx)))),
                        )
                    }),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(860.))
                    .mx_auto()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(space::SM))
                    .rounded(px(radius::XL))
                    .border_1()
                    .border_color(Tokens::border())
                    .bg(Tokens::surface())
                    .children(composer.map(|c| Textarea::new(&c).appearance(false).bordered(false)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .children(self.picker.clone())
                            .child(div().flex_1())
                            .child(btn("cr-send", "Send", ButtonKind::Primary).small().icon(ph::PAPER_PLANE_TILT).loading(self.thinking.is_some()).on_click(cx.listener(|this, _, window, cx| this.send_from_composer(window, cx)))),
                    ),
            )
    }

    fn chat_empty(&self, cx: &mut Context<Self>) -> Div {
        div()
            .w_full()
            .max_w(px(860.))
            .mx_auto()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .pt(px(24.))
            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("What manga do you want to make?"))
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Name a manga it should feel like, describe your own characters (or add their pictures on the Characters tab), and your twist. The AI writes the series and a cast that grows chapter by chapter; every character gets a model sheet and an expression sheet before they appear, so they stay on model on every page."))
            .children(EXAMPLES.iter().enumerate().map(|(i, e)| {
                let text = e.to_string();
                div()
                    .id(("cr-example", i))
                    .p(px(space::MD))
                    .rounded(px(radius::LG))
                    .border_1()
                    .border_color(Tokens::border())
                    .bg(Tokens::surface_2())
                    .cursor_pointer()
                    .hover(|s| s.bg(Tokens::hover()))
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_2())
                    .child(e.to_string())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(c) = &this.composer {
                            c.update(cx, |s, cx| {
                                s.set_value(text.clone(), window, cx);
                                s.focus(window, cx);
                            });
                        }
                    }))
            }))
    }

    fn line(&self, p: &Project, i: usize, l: &ChatLine, cx: &mut Context<Self>) -> AnyElement {
        match l.role.as_str() {
            "user" => div().flex().justify_end().child(div().max_w(px(640.)).px(px(space::MD)).py(px(space::SM)).rounded(px(radius::LG)).bg(Tokens::accent_soft()).text_size(px(size::SM)).text_color(Tokens::text_1()).whitespace_normal().child(l.text.clone())).into_any_element(),
            "note" => div().flex().items_start().gap(px(6.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(icon_ph(ph::CHECK, Tokens::text_3(), 12.)).child(div().flex_1().whitespace_normal().child(l.text.clone())).into_any_element(),
            "card" => self.card(p, i, l, cx).into_any_element(),
            _ => div().flex().items_start().gap(px(space::SM)).child(icon_ph(ph::PAINT_BRUSH, Tokens::accent(), 16.)).child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_1()).whitespace_normal().child(l.text.clone())).into_any_element(),
        }
    }

    /// A proposal: what it paints, with which model, roughly what it costs, and Paint.
    fn card(&self, p: &Project, i: usize, l: &ChatLine, cx: &mut Context<Self>) -> Div {
        let Some(proposal) = l.proposal.as_ref() else { return div() };
        let tasks = jobs::plan(p, proposal);
        let video = matches!(proposal, Proposal::Video { .. });
        let open = l.state == CardState::Open;
        let m = jobs::image_model(p, &self.keys);
        let (images, credits) = jobs::estimate(p, &self.keys, &tasks);
        let sheets = tasks.iter().filter(|t| matches!(t, jobs::Task::Sheet(_) | jobs::Task::Expressions(_))).count();
        let detail = if video {
            let vm = mv_create::video::video_model(&p.settings.video_model);
            let clips = tasks.iter().filter(|t| matches!(t, jobs::Task::Clip { .. })).count();
            format!("{clips} clips with {}, {} s each, stitched in reading order.", vm.name, vm.fit_seconds(p.settings.clip_seconds))
        } else {
            let mut s = format!("{images} image{} with {}", if images == 1 { "" } else { "s" }, m.name);
            if credits > 0. {
                s.push_str(&format!(" · about {} Magnific credits", credits.round()));
            } else if m.via != ui_studio::images::Via::Magnific {
                s.push_str(&format!(" · billed by {}", m.service_label()));
            }
            if sheets > 0 && matches!(proposal, Proposal::Pages { .. }) {
                s.push_str(&format!(" (including {sheets} missing character sheets, painted first)"));
            }
            s
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if open { Tokens::accent() } else { Tokens::border() })
            .bg(Tokens::surface_2())
            .child(div().flex().items_center().gap(px(space::SM)).child(icon_ph(if video { ph::FILM_STRIP } else { ph::PAINT_BRUSH }, Tokens::accent(), 16.)).child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(l.text.clone())))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(detail))
            .when(open && !video, |d| d.child(self.look_bar(cx)))
            .when(open && video, |d| d.child(self.video_bar(cx)))
            .child(
                div()
                    .flex()
                    .gap(px(space::SM))
                    .when(open, |d| {
                        d.child(btn(("cr-card-go", i), if video { "Animate" } else { "Paint" }, ButtonKind::Primary).small().icon(if video { ph::PLAY } else { ph::PAINT_BRUSH }).on_click(cx.listener(move |this, _, _, cx| this.start_card(i, cx))))
                            .child(btn(("cr-card-no", i), "Not now", ButtonKind::Ghost).small().on_click(cx.listener(move |this, _, _, cx| this.dismiss_card(i, cx))))
                    })
                    .when(l.state == CardState::Started, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Started — see the progress above.")))
                    .when(l.state == CardState::Dismissed, |d| d.child(btn(("cr-card-redo", i), "Paint after all", ButtonKind::Ghost).small().on_click(cx.listener(move |this, _, _, cx| this.start_card(i, cx))))),
            )
            .when(!open, |d| d.opacity(0.85))
    }
}

/// What Studio learned of a downloaded manga (its characters and story), for the inspiration.
fn inspiration_notes(id: &str, title: &str) -> String {
    let b = mv_recap::Brain::load(id, title);
    let mut s = format!("{title}:");
    for c in b.characters.iter().take(12) {
        s.push_str(&format!("\n- {}: {}", c.name, c.description.chars().take(160).collect::<String>()));
    }
    let notes: String = b.notes.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    if !notes.is_empty() {
        s.push_str(&format!("\nStory notes:\n{notes}"));
    }
    s
}
