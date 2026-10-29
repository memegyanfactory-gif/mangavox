//! The Generate queue and the Issues list, as the side panel shows them (Activity and Issues
//! tabs), and what an issue's fix buttons do.
use std::collections::HashMap;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::Step;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::Interactive as _;
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, confirm_dialog, disclosure, empty_view, icon_button, icon_ph, inline_alert, ph, ph_icon, size, status_tag, tip, AlertKind, ButtonKind, StatusKind, Tokens, Weight};

use crate::data::ChapterItem;
use crate::issues::{Fix, Issue, Named};
use crate::page::{plural, Load, Phase, Side, StudioPage, MONO};
use crate::phases::progress_line;
use crate::pipeline::{self, ItemState, QueueItem, StepState};
use crate::run::RunKind;
use crate::widgets::{card, phase_icon};

impl StudioPage {
    /// The library's chapters with these ids.
    pub(crate) fn chapter_items(&self, ids: &[String]) -> Vec<ChapterItem> {
        let Load::Ready(groups) = &self.library else { return vec![] };
        groups.iter().flat_map(|g| g.chapters.iter()).filter(|c| ids.contains(&c.id)).cloned().collect()
    }

    /// Runs an issue's fix.
    pub(crate) fn apply_fix(&mut self, fix: Fix, window: &mut Window, cx: &mut Context<Self>) {
        match fix {
            Fix::Make { chapters, until, force } => {
                let items = self.chapter_items(&chapters);
                if items.is_empty() {
                    toast::show(cx, Toast::warning("That chapter is no longer downloaded"));
                    return;
                }
                self.sync_pipeline(cx);
                let format = self.prefs.format.clone();
                let (n, w, already) = pipeline::enqueue(&items, until, force, &format, None, true);
                for c in &items {
                    if !self.fixing.contains(&c.id) {
                        self.fixing.push(c.id.clone());
                    }
                }
                let what = match items.as_slice() {
                    [one] => one.label(),
                    many => format!("{} chapters", many.len()),
                };
                let msg = if n + w > 0 { format!("{what}: making {} now", until.label().to_lowercase()) } else { format!("{what} is already in the queue ({already})") };
                toast::show(cx, Toast::success(msg).icon(phase_icon(Phase::of(until))));
                self.side_open = true;
                self.side = Side::Activity;
            }
            Fix::Retry(id) => {
                self.sync_pipeline(cx);
                pipeline::resume(id);
                if let Some(c) = self.queue.get(id).map(|it| it.chapter.id.clone()).filter(|c| !self.fixing.contains(c)) {
                    self.fixing.push(c);
                }
                self.side_open = true;
                self.side = Side::Activity;
            }
            Fix::Open { chapter, step } => self.go_to(&chapter, Some(step), cx),
            Fix::OpenSetup => {
                self.show_setup = true;
                self.sheet = None;
            }
            Fix::AudioSettings => StudioPage::open_audio_settings(window, cx),
            Fix::PickModel { chapter } => {
                match chapter {
                    Some(c) => self.go_to(&c, Some(Step::Read), cx),
                    None => {
                        self.view = crate::page::View::Editor;
                        self.set_phase(Phase::Read, cx);
                    }
                }
                toast::show(cx, Toast::info("Pick a model with the model button next to Read, then retry").icon(mv_ui::glyphs::MODEL));
            }
            Fix::ResumeQueue => {
                self.sync_pipeline(cx);
                pipeline::resume_all();
            }
        }
        // Show the fix at once (tiles and issues read this copy of the queue; the poll would
        // only catch up a moment later).
        self.queue = pipeline::snapshot();
        cx.notify();
    }

    /// Every issue right now (see `issues::collect`), minus the dismissed ones.
    pub(crate) fn collect_issues(&self, cx: &mut Context<Self>) -> Vec<Issue> {
        let mut names: HashMap<String, Named> = HashMap::new();
        if let Load::Ready(groups) = &self.library {
            let many = groups.len() > 1;
            for c in groups.iter().flat_map(|g| g.chapters.iter()) {
                let base = match c.number {
                    Some(_) => pipeline::short_label(c.number, None).replacen("Ch.", "Chapter", 1),
                    None => c.label(),
                };
                let label = if many { format!("{base} ({})", c.manga_title) } else { base };
                names.insert(c.id.clone(), Named { label });
            }
        }
        let setup = self.setup.read(cx).env.as_ref().map(|e| {
            let voice = self.prefs.voice.clone().unwrap_or_default();
            let not_ready = mv_studio::tts::find_voice(&voice).filter(|v| !crate::setup::voice_ready(&e.engines, v).usable()).map(|v| v.label.to_string());
            (e.runnable(), e.ffmpeg, not_ready)
        });
        let editor = self.view == crate::page::View::Editor && self.focus.is_some();
        let targets: Vec<String> = if editor { self.targets().into_iter().map(|c| c.id).collect() } else { vec![] };
        let inputs = crate::issues::Inputs {
            queue: &self.queue,
            jobs: &self.jobs,
            run: self.run.as_ref(),
            statuses: &self.statuses,
            names: &names,
            setup,
            targets: &targets,
            step: editor.then(|| self.phase.step()),
            fixing: &self.fixing,
        };
        let mut out = crate::issues::collect(&inputs);
        out.retain(|i| !self.dismissed.contains(&i.key));
        out
    }

    /// Compares the queue before and after a change: toasts finished and failed chapters, and
    /// says whether files changed (a step finished), so the library is read again.
    pub(crate) fn queue_moved(&self, now: &pipeline::Queue, cx: &mut Context<Self>) -> bool {
        let mut moved = false;
        for it in &now.items {
            let Some(prev) = self.queue.get(it.id) else { continue };
            if it.steps.iter().zip(prev.steps.iter()).any(|(a, b)| a != b && matches!(a, StepState::Done | StepState::Failed)) {
                moved = true;
            }
            if it.state == prev.state {
                continue;
            }
            let weak = cx.entity().downgrade();
            let chapter = it.chapter.id.clone();
            match it.state {
                ItemState::Done => {
                    moved = true;
                    toast::show(
                        cx,
                        Toast::success(format!("{}: recap ready", it.chapter.label())).icon(mv_ui::glyphs::VIDEO).action("Open", move |_, cx| {
                            let _ = weak.update(cx, |this, cx| this.go_to(&chapter, Some(Step::Video), cx));
                        }),
                    );
                }
                ItemState::Failed => {
                    moved = true;
                    let step = it.current_step().unwrap_or(Step::Read);
                    toast::show(
                        cx,
                        Toast::error(format!("{}: {} failed", it.chapter.label(), step.label().to_lowercase())).action("See the fix", move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.side_open = true;
                                this.side = Side::Issues;
                                cx.notify();
                            });
                        }),
                    );
                }
                _ => {}
            }
        }
        moved
    }

    /// The running step's own progress (0..=1) and status line, from its AI run or job.
    pub(crate) fn item_progress(&self, it: &QueueItem) -> Option<(f32, String)> {
        let step = it.current_step()?;
        match step {
            Step::Read | Step::Story => {
                let r = self.run.as_ref().filter(|r| r.running() && r.chapter_id.as_deref() == Some(it.chapter.id.as_str()))?;
                let want = if step == Step::Read { RunKind::Read } else { RunKind::Story };
                (r.kind == want).then(|| (r.progress(), r.status_line()))
            }
            Step::Audio | Step::Video => {
                let j = self.jobs.iter().find(|j| Some(j.id) == it.job)?;
                Some((j.progress() as f32, j.status.clone()))
            }
            Step::Music => Some((0., "Composing the score…".into())),
            Step::Transcript => Some((0., "Saving the transcript…".into())),
        }
    }

    /// "42%" or the step's status, for one-line summaries.
    pub(crate) fn item_detail(&self, it: &QueueItem) -> Option<String> {
        let (p, line) = self.item_progress(it)?;
        Some(if p > 0. { format!("{:.0}%", p * 100.) } else { line })
    }

    // ------------------------------------------------------------ queue card

    pub(crate) fn queue_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let q = &self.queue;
        if q.items.is_empty() {
            return None;
        }
        let active = q.active();
        let done = q.items.iter().filter(|i| i.state == ItemState::Done).count();
        let failed = q.items.iter().filter(|i| i.state == ItemState::Failed).count();
        let (kind, label) = match (active, q.paused) {
            (0, _) if failed > 0 => (StatusKind::Failed, format!("{failed} failed")),
            (0, _) => (StatusKind::Done, "Done".to_string()),
            (_, true) => (StatusKind::Queued, "Paused".to_string()),
            _ => (StatusKind::Running, format!("{done}/{}", q.items.len())),
        };
        let pause = if q.paused {
            icon_button("q-resume-all", ph::PLAY, "Resume Generate").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.sync_pipeline(cx);
                pipeline::resume_all();
                cx.notify();
            }))
        } else {
            icon_button("q-pause-all", ph::PAUSE, "Pause Generate (the running step stops; finished work is kept)").small().when(active == 0, |b| b.disabled("Nothing is waiting")).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                pipeline::pause_all();
                cx.notify();
            }))
        };
        Some(
            card()
                .gap(px(space::SM))
                .p(px(space::MD))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(ph_icon(ph::ROCKET_LAUNCH, Weight::Duotone, Tokens::accent(), icon_px::MD))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Generate"))
                        .child(status_tag(kind).label(label))
                        .child(pause)
                        .when(active > 0, |d| {
                            d.child(icon_button("q-cancel-all", ph::STOP_CIRCLE, "Cancel every chapter still to make").small().danger().on_click(cx.listener(|_, _: &ClickEvent, window, cx| {
                                confirm_dialog("Cancel Generate?", "Chapters still waiting leave the queue. Work already made stays.", "Cancel Generate", true)
                                    .cancel_label("Keep going")
                                    .on_confirm(|_, _| {
                                        pipeline::cancel_all();
                                    })
                                    .open(window, cx);
                            })))
                        })
                        .when(done > 0, |d| {
                            d.child(icon_button("q-clear", ph::BROOM, "Clear finished chapters").small().on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                                pipeline::clear_finished();
                                cx.notify();
                            })))
                        }),
                )
                .when(q.paused && q.interrupted && active > 0, |d| {
                    d.child(inline_alert(AlertKind::Info, "Paused when MangaVox closed", format!("{active} chapter{} still to make.", plural(active))).action(
                        "Resume",
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.sync_pipeline(cx);
                            pipeline::resume_all();
                            cx.notify();
                        }),
                    ))
                })
                .children(q.items.iter().map(|it| self.queue_row(it, cx)))
                .into_any_element(),
        )
    }

    fn queue_row(&self, it: &QueueItem, cx: &mut Context<Self>) -> AnyElement {
        let id = it.id;
        let (kind, label) = match it.state {
            ItemState::Queued => (StatusKind::Queued, "Waiting".to_string()),
            ItemState::Running => (StatusKind::Running, it.current_step().map(|s| s.label().to_string()).unwrap_or_else(|| "Starting".into())),
            ItemState::Paused => (StatusKind::Cancelled, "Paused".to_string()),
            ItemState::Done => (StatusKind::Done, "Done".to_string()),
            ItemState::Failed => (StatusKind::Failed, format!("{} failed", it.current_step().map(|s| s.label()).unwrap_or("Step"))),
        };
        let chapter = it.chapter.id.clone();
        let open_step = it.current_step();
        let controls = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .map(|d| match it.state {
                ItemState::Queued | ItemState::Running => d.child(icon_button(("q-pause", id as usize), ph::PAUSE, "Pause this chapter").small().on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    pipeline::pause(id);
                    cx.notify();
                }))),
                ItemState::Paused => d.child(icon_button(("q-resume", id as usize), ph::PLAY, "Resume this chapter").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.sync_pipeline(cx);
                    pipeline::resume(id);
                    cx.notify();
                }))),
                ItemState::Failed => d.child(icon_button(("q-retry", id as usize), mv_ui::glyphs::RETRY, "Retry from the step that failed").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.sync_pipeline(cx);
                    pipeline::resume(id);
                    cx.notify();
                }))),
                ItemState::Done => d,
            })
            .child(
                icon_button(("q-x", id as usize), ph::X, if it.state.active() { "Take this chapter out of the queue" } else { "Remove from the list" }).small().on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    pipeline::cancel(id);
                    cx.notify();
                })),
            );
        let progress = (it.state == ItemState::Running).then(|| self.item_progress(it)).flatten();
        div()
            .id(("q-row", id as usize))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(space::SM))
            .rounded(px(radius::MD))
            .bg(Tokens::well())
            .border_1()
            .border_color(if it.state == ItemState::Failed { Tokens::danger_border() } else { Tokens::border() })
            .tooltip(tip("Open this chapter at its current step"))
            .hover(|s| s.border_color(Tokens::border_strong()))
            .pressable()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.go_to(&chapter, open_step, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(it.chapter.label()))
                            .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(it.chapter.manga_title.clone())),
                    )
                    .child(status_tag(kind).label(label))
                    .child(controls),
            )
            .child(step_trail(it))
            .when(it.until < Step::Video.index() && it.state.active(), |d| {
                d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Making up to {} (a fix asked for it)", it.until_step().label())))
            })
            .when_some(progress, |d, (p, line)| d.child(progress_line(&format!("q-prog-{id}"), p, line)))
            .when_some(it.note.clone(), |d, n| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(n)))
            .into_any_element()
    }

    // ------------------------------------------------------------ issues

    /// One issue: the sentence, its fixes, and the raw error behind "Details".
    pub(crate) fn issue_card(&self, ix: usize, issue: &Issue, cx: &mut Context<Self>) -> AnyElement {
        if let Some(id) = issue.fixing {
            return self.fixing_card(ix, issue, id, cx);
        }
        let (color, soft, border, icon) = issue.kind.colors();
        let key = issue.key.clone();
        let nav = issue.chapter.clone().map(|c| (c, issue.step));
        let queue_item = issue.queue_item;
        let where_ = issue.step.map(|s| s.label());
        div()
            .id(("issue", ix))
            .flex()
            .gap(px(space::SM))
            .p(px(space::SM))
            .rounded(px(radius::MD))
            .bg(soft)
            .border_1()
            .border_color(border)
            .when(nav.is_some(), |d| d.hover(|s| s.border_color(color)).pressable().tooltip(tip("Open this chapter at that step")))
            .when_some(nav, |d, (c, s)| d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.go_to(&c, s, cx))))
            .child(div().pt(px(1.)).child(ph_icon(icon, Weight::Fill, color, icon_px::MD)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .child(div().text_size(px(size::SM)).line_height(px(18.)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(issue.title.clone()))
                    .when_some(where_, |d, w| d.child(div().flex().items_center().gap(px(4.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(icon_ph(phase_icon(Phase::of(issue.step.unwrap_or(Step::Read))), Tokens::text_3(), icon_px::XS)).child(format!("{w} step"))))
                    .when(!issue.actions.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().gap(px(6.)).children(issue.actions.iter().enumerate().map(|(i, a)| {
                            let fix = a.fix.clone();
                            btn(("issue-act", ix * 10 + i), a.label.clone(), if i == 0 { ButtonKind::Primary } else { ButtonKind::Secondary }).small().on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.apply_fix(fix.clone(), window, cx);
                            }))
                        })))
                    })
                    .when_some(issue.detail.clone(), |d, raw| {
                        d.child(
                            disclosure(("issue-detail", ix), "Details").content(
                                div()
                                    .id(("issue-raw", ix))
                                    .max_h(px(160.))
                                    .overflow_y_scroll()
                                    .p(px(space::SM))
                                    .rounded(px(radius::SM))
                                    .bg(Tokens::well())
                                    .font_family(MONO)
                                    .text_size(px(size::XS))
                                    .text_color(Tokens::text_2())
                                    .child(raw),
                            ),
                        )
                    }),
            )
            .child(icon_button(("issue-x", ix), ph::X, "Dismiss").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                if let Some(id) = queue_item {
                    pipeline::dismiss(id);
                }
                this.dismissed.insert(key.clone());
                cx.notify();
            })))
            .into_any_element()
    }

    /// An issue Generate is fixing now: the chapter, which step it is on, and its progress.
    fn fixing_card(&self, ix: usize, issue: &Issue, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let item = self.queue.get(id);
        let step = item.and_then(|it| it.current_step()).or(issue.step).unwrap_or(Step::Read);
        let (p, line) = match item.map(|it| (it.state, self.item_progress(it))) {
            Some((ItemState::Running, Some((p, line)))) => (p, format!("{} · {line}", step.label())),
            Some((ItemState::Running, None)) => (0., format!("{} · starting", step.label())),
            Some((ItemState::Paused, _)) => (0., "Paused · resume Generate to carry on".to_string()),
            _ => (0., "Waiting in the queue".to_string()),
        };
        let nav = issue.chapter.clone();
        div()
            .id(("issue", ix))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(space::SM))
            .rounded(px(radius::MD))
            .bg(Tokens::accent_soft())
            .border_1()
            .border_color(Tokens::accent_border())
            .when(nav.is_some(), |d| d.hover(|s| s.border_color(Tokens::accent())).pressable().tooltip(tip("Open this chapter")))
            .when_some(nav, |d, c| d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.go_to(&c, Some(step), cx))))
            .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(issue.title.clone()))
            .child(crate::phases::progress_line(&format!("fixing-{ix}"), p, line))
            .into_any_element()
    }

    /// The Issues tab.
    pub(crate) fn issues_view(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let issues = self.issues.clone();
        div()
            .id("studio-issues")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(space::MD))
            .gap(px(space::SM))
            .map(|d| {
                if issues.is_empty() {
                    d.child(div().pt(px(space::XL)).child(empty_view(ph::CHECK_CIRCLE, "No issues", "When a step fails or a chapter is missing something, it shows up here with the fix.").compact()))
                } else {
                    let n = issues.iter().filter(|i| i.fixing.is_none()).count();
                    let fixing = issues.len() - n;
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(match (n, fixing) {
                                (0, f) => format!("{f} being fixed"),
                                (n, 0) => format!("{n} issue{}", plural(n)),
                                (n, f) => format!("{n} issue{} · {f} being fixed", plural(n)),
                            }))
                            .child(btn("issues-clear", "Dismiss all", ButtonKind::Ghost).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                for i in this.issues.clone() {
                                    this.dismissed.insert(i.key);
                                }
                                cx.notify();
                            }))),
                    )
                    .children(issues.iter().enumerate().map(|(i, is)| self.issue_card(i, is, cx)))
                }
            })
    }

    /// A failure as Activity shows it: the sentence, the fixes, and the raw error behind Details.
    pub(crate) fn failure_block(&self, id: &str, step: Step, raw: &str, chapter: Option<&str>, cx: &mut Context<Self>) -> Div {
        let (why, mut actions) = crate::issues::explain(step, raw, chapter.unwrap_or_default(), None);
        if chapter.is_none() {
            actions.retain(|a| !matches!(a.fix, Fix::Make { .. }));
        }
        let mut sentence = why;
        if let Some(c) = sentence.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        let raw = raw.to_string();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .child(div().pt(px(2.)).child(icon_ph(ph::WARNING_CIRCLE, Tokens::danger(), icon_px::SM)))
                    .child(div().flex_1().min_w_0().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_1()).child(format!("{sentence}."))),
            )
            .when(!actions.is_empty(), |d| {
                d.child(div().flex().flex_wrap().gap(px(6.)).children(actions.into_iter().enumerate().map(|(i, a)| {
                    let fix = a.fix.clone();
                    btn(SharedString::from(format!("{id}-fix-{i}")), a.label, if i == 0 { ButtonKind::Primary } else { ButtonKind::Secondary })
                        .small()
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.apply_fix(fix.clone(), window, cx)))
                })))
            })
            .child(
                disclosure(SharedString::from(format!("{id}-raw")), "Details")
                    .content(div().p(px(space::SM)).rounded(px(radius::SM)).bg(Tokens::well()).font_family(MONO).text_size(px(size::XS)).text_color(Tokens::text_2()).child(raw)),
            )
    }

    /// The issues of the open step (its missing prerequisites, and failures of the open chapter
    /// at this step), shown in the step's header.
    pub(crate) fn step_issues(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let step = self.phase.step();
        let focus = self.focus_id.clone();
        let mine: Vec<(usize, Issue)> = self
            .issues
            .iter()
            .enumerate()
            .filter(|(_, i)| i.key.starts_with("gap-") || (i.chapter.is_some() && i.chapter == focus && i.step == Some(step)))
            .map(|(ix, i)| (ix, i.clone()))
            .collect();
        if mine.is_empty() {
            return None;
        }
        Some(div().flex().flex_col().gap(px(6.)).max_w(px(900.)).children(mine.iter().map(|(ix, i)| self.issue_card(200 + ix, i, cx))).into_any_element())
    }

    /// The first issues, inline at the top of Activity, with a link to the rest.
    pub(crate) fn issues_inline(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.issues.is_empty() {
            return None;
        }
        let n = self.issues.len();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(space::SM))
                .children(self.issues.iter().take(2).enumerate().map(|(i, is)| self.issue_card(100 + i, is, cx)))
                .when(n > 2, |d| {
                    d.child(btn("issues-more", format!("See all {n} issues"), ButtonKind::Ghost).small().trailing_icon(ph::ARROW_RIGHT).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.side = Side::Issues;
                        cx.notify();
                    })))
                })
                .into_any_element(),
        )
    }
}

/// Six small step marks: made, skipped, running, failed or still to make.
fn step_trail(it: &QueueItem) -> Div {
    let until = it.until.min(5);
    div().flex().items_center().gap(px(4.)).children(Step::ALL.iter().enumerate().map(|(i, s)| {
        let st = it.steps[i];
        let (color, weight) = match st {
            _ if i > until => (Tokens::text_disabled(), Weight::Regular),
            StepState::Done => (Tokens::success(), Weight::Fill),
            StepState::Skipped => (Tokens::text_3(), Weight::Fill),
            StepState::Running => (Tokens::accent(), Weight::Fill),
            StepState::Failed => (Tokens::danger(), Weight::Fill),
            StepState::Pending => (Tokens::text_3(), Weight::Regular),
        };
        let word = match st {
            _ if i > until => "not asked for",
            StepState::Done => "made",
            StepState::Skipped => "already made, skipped",
            StepState::Running => "working",
            StepState::Failed => "failed",
            StepState::Pending => "to do",
        };
        div()
            .id(("q-step", it.id as usize * 8 + i))
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded(px(radius::SM))
            .when(st == StepState::Running, |d| d.bg(Tokens::accent_soft()))
            .when(st == StepState::Failed, |d| d.bg(Tokens::danger_soft()))
            .tooltip(tip(format!("{}: {word}", s.label())))
            .child(ph_icon(if st == StepState::Failed { ph::X_CIRCLE } else { phase_icon(Phase::of(*s)) }, weight, color, icon_px::SM))
    }))
}
