//! The right-hand AI panel: the current run and the narration queue (Activity), the
//! manga's character brain (Characters), and the assistant chat (Chat).
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, disclosure, empty_view, glyphs, icon_button, icon_ph, icon_tag, icon_toggle, inline_alert, kbd, muted, ph, size, status_tag, AlertKind, ButtonKind, StatusKind, Tokens, Tone,
};

use crate::page::{plural, vbar, Side, StudioPage, MONO, SIDE_W};
use crate::phases::{face_avatar, progress_line};
use crate::run::{self, RunKind, RunState};
use crate::service::{self, JobView};
use crate::widgets::{card, column, tab_bar};

impl StudioPage {
    pub(crate) fn side_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.side == Side::Chat && self.dock.is_none() {
            self.dock = ui_assistant::dock(window, cx);
        }
        // A live dot on Activity while something runs and another tab is open.
        let live = self.run.as_ref().is_some_and(|r| r.running()) || self.jobs.iter().any(|j| !j.state.finished());
        let tabs = tab_bar(
            "studio-side-tabs",
            &[(Side::Activity, "Activity", ph::PULSE, live), (Side::Brain, "Characters", ph::USERS, false), (Side::Chat, "Chat", ph::CHAT_CIRCLE_DOTS, false)],
            self.side,
            cx.listener(|this, s: &Side, _, cx| {
                this.side = *s;
                cx.notify();
            }),
        );
        let body: AnyElement = match self.side {
            Side::Activity => {
                let body = self.activity(cx).size_full().track_scroll(&self.side_scroll);
                div().relative().flex_1().min_h_0().child(body).child(vbar("side-bar", &self.side_scroll)).into_any_element()
            }
            Side::Brain => {
                let body = self.brain_view(window.last_input_was_keyboard(), cx).size_full().track_scroll(&self.side_scroll);
                div().relative().flex_1().min_h_0().child(body).child(vbar("side-bar", &self.side_scroll)).into_any_element()
            }
            Side::Chat => match self.dock.clone() {
                Some(d) => div().flex_1().min_h_0().child(d).into_any_element(),
                None => div()
                    .p(px(space::LG))
                    .child(empty_view(glyphs::ASSISTANT, "Chat unavailable", "The assistant could not start.").compact().retry(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.dock = ui_assistant::dock(window, cx);
                        if this.dock.is_none() {
                            toast::show(cx, Toast::error("The assistant still could not start"));
                        }
                        cx.notify();
                    })))
                    .into_any_element(),
            },
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDE_W))
            .h_full()
            .map(column)
            .child(div().flex_none().h(px(52.)).flex().items_center().px(px(space::MD)).border_b_1().border_color(Tokens::border()).child(div().flex_1().child(tabs)))
            .child(body)
            .into_any_element()
    }

    // ------------------------------------------------------------ Activity

    fn activity(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let run = self.run.clone();
        let jobs = self.jobs.clone();
        let any_finished = jobs.iter().any(|j| j.state.finished());
        let can_read = self.focus.is_some() && self.run_blocker().is_none();
        div()
            .id("studio-activity")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(space::MD))
            .gap(px(space::MD))
            .when(run.is_none() && jobs.is_empty(), |d| {
                d.child(
                    div().pt(px(space::XL)).child(
                        empty_view(ph::PULSE, "Nothing running", "Read a chapter and the AI's progress, the characters it meets and every narration or render job show up here.")
                            .compact()
                            .when(can_read, |e| {
                                e.action(btn("act-empty-read", "Read this chapter", ButtonKind::Secondary).icon(glyphs::READ).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(false, cx))))
                            }),
                    ),
                )
            })
            .when_some(run, |d, r| d.child(self.run_card(&r, cx)))
            .when(!jobs.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .pt(px(space::XS))
                        .child(icon_ph(ph::QUEUE, Tokens::text_3(), icon_px::SM))
                        .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child("Audio and video"))
                        .when(any_finished, |d| {
                            d.child(btn("jobs-clear", "Clear finished", ButtonKind::Ghost).small().icon(ph::BROOM).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                service::clear_finished();
                                this.jobs = service::snapshot();
                                cx.notify();
                            })))
                        }),
                )
                .children(jobs.into_iter().map(|j| {
                    let kind = match &j.state {
                        JobView::Queued => StatusKind::Queued,
                        JobView::Running => StatusKind::Running,
                        JobView::Done => StatusKind::Done,
                        JobView::Failed(_) => StatusKind::Failed,
                        JobView::Cancelled => StatusKind::Cancelled,
                    };
                    let video = j.stages.iter().any(|s| s == "video");
                    let what = match (video, j.regen_pages.as_slice()) {
                        (true, _) => "Video".to_string(),
                        (false, [p]) => format!("Page {}", p + 1),
                        (false, []) => "Audio".to_string(),
                        (false, ps) => format!("{} pages", ps.len()),
                    };
                    let id = j.id;
                    let narrator = j.narrator.clone();
                    card()
                        .gap(px(space::SM))
                        .p(px(space::MD))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .child(icon_ph(if video { glyphs::VIDEO } else { glyphs::NARRATE }, Tokens::text_2(), icon_px::MD))
                                .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format!("{what} · {}", j.chapter_label)))
                                .child(status_tag(kind))
                                .when(!j.state.finished(), |d| {
                                    d.child(icon_button(SharedString::from(format!("job-x-{id}")), ph::X, "Cancel this job").small().on_click(move |_, _, _| {
                                        service::cancel(id);
                                    }))
                                }),
                        )
                        .when_some(narrator, |d, n| {
                            let tone = if n.stand_in { Tone::Warning } else { Tone::Neutral };
                            let text = if n.stand_in { format!("{} stands in", n.engine_label()) } else { n.engine_label().to_string() };
                            d.child(div().flex().child(icon_tag(glyphs::VOICE, text, tone)))
                        })
                        .when(!j.state.finished(), |d| d.child(progress_line(&format!("job-{}", j.id), j.progress() as f32, j.status.clone())))
                        .when_some(if let JobView::Failed(e) = &j.state { Some(e.clone()) } else { None }, |d, e| d.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::danger()).child(e)))
                }))
            })
    }

    fn run_card(&self, r: &run::RunInfo, cx: &mut Context<Self>) -> Div {
        let (kind, label) = match &r.state {
            RunState::Running => (StatusKind::Running, "Working"),
            RunState::Done => (StatusKind::Done, "Done"),
            RunState::Cancelled => (StatusKind::Cancelled, "Stopped"),
            RunState::Failed(_) => (StatusKind::Failed, "Failed"),
        };
        let what = match r.kind {
            RunKind::Read => "Reading",
            RunKind::Story => "Writing the story",
        };
        let lines: Vec<String> = r.logs.iter().rev().take(80).rev().cloned().collect();
        let n_lines = lines.len();
        card()
            .gap(px(10.))
            .p(px(space::MD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(icon_ph(if r.kind == RunKind::Read { glyphs::READ } else { glyphs::STORY }, Tokens::text_2(), icon_px::MD))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(what))
                    .child(status_tag(kind).label(label))
                    .when(r.running(), |d| {
                        d.child(icon_button("act-stop", ph::STOP_CIRCLE, "Stop the run (finished pages are kept)").small().on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                            run::cancel();
                            cx.notify();
                        })))
                    }),
            )
            .child(div().flex().items_center().gap(px(6.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(icon_ph(glyphs::MODEL, Tokens::text_3(), icon_px::XS)).child(r.model.clone()))
            .when(r.running(), |d| d.child(progress_line("act-run", r.progress(), r.status_line())))
            .when_some(if let RunState::Failed(e) = &r.state { Some(e.clone()) } else { None }, |d, e| d.child(inline_alert(AlertKind::Error, "The run stopped with an error", e)))
            .when(r.state == RunState::Cancelled && r.kind == RunKind::Read, |d| {
                let at = match (&r.chapter, r.total) {
                    (Some(c), t) if t > 0 => format!("Stopped in {c} after {} of {t} pages. Finished pages are saved.", r.done.min(t)),
                    _ => "Stopped. Finished pages are saved.".to_string(),
                };
                let blocker = self.run_blocker();
                d.child(muted(at)).child(
                    div()
                        .flex()
                        .gap(px(space::SM))
                        .child(
                            btn("act-continue", "Continue", ButtonKind::Primary)
                                .small()
                                .icon(ph::PLAY)
                                .when_some(blocker, |b, r| b.disabled(r))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(false, cx))),
                        )
                        .child(
                            btn("act-restart", "Start over", ButtonKind::Secondary)
                                .small()
                                .icon(glyphs::RETRY)
                                .tooltip("Read every page again from page 1")
                                .when_some(blocker, |b, r| b.disabled(r))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(true, cx))),
                        ),
                )
            })
            .when(!r.new_characters.is_empty(), |d| {
                let n = r.new_characters.len();
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(div().flex().items_center().gap(px(6.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(icon_ph(ph::USER_PLUS, Tokens::text_3(), icon_px::XS)).child(format!("Met {n} new character{}", plural(n))))
                        .child(div().flex().flex_wrap().gap(px(space::XS)).children(r.new_characters.iter().map(|c| mv_ui::tag(c.clone(), Tone::Accent)))),
                )
            })
            // The raw log is detail: one click away (NN/g progressive disclosure).
            .when(n_lines > 0, |d| {
                d.child(
                    disclosure("act-log", format!("Show log ({n_lines} line{})", plural(n_lines))).content(
                        div()
                            .id("act-log-body")
                            .flex()
                            .flex_col()
                            .max_h(px(260.))
                            .overflow_y_scroll()
                            .p(px(space::SM))
                            .rounded(px(radius::MD))
                            .bg(Tokens::well())
                            .font_family(MONO)
                            .text_size(px(size::XS))
                            .text_color(Tokens::text_2())
                            .children(lines.into_iter().map(|l| div().child(l))),
                    ),
                )
            })
    }

    // ------------------------------------------------------------ Characters

    /// `keyboard`: the last input was the keyboard, so the per-card actions stay visible (hidden
    /// elements are no Tab stops, and a keyboard user cannot hover).
    fn brain_view(&mut self, keyboard: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let Some(f) = &self.focus else {
            return div()
                .id("brain")
                .p(px(space::LG))
                .child(empty_view(glyphs::MANGA, "No manga open", "Pick a chapter to see the characters the AI knows for that manga.").compact());
        };
        let (mid, title) = (f.item.manga_id.clone(), f.item.manga_title.clone());
        let brain = run::brain_snapshot(&mid, &title);
        let renaming = self.renaming.clone();
        let mut chars = brain.characters.clone();
        chars.sort_by(|a, b| b.appearances.cmp(&a.appearances).then(a.name.cmp(&b.name)));
        let blocker = self.run_blocker();
        div()
            .id("brain")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(space::MD))
            .gap(px(space::SM))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .pb(px(space::XS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.clone()))
                            .child(mv_ui::tag(format!("{} character{}", chars.len(), plural(chars.len())), Tone::Neutral)),
                    )
                    .child(div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_3()).child("Faces the AI captured while reading. Rename one to teach it; giving two the same name merges them.")),
            )
            .when(chars.is_empty(), |d| {
                d.child(
                    div().pt(px(space::LG)).child(
                        empty_view(ph::USERS, "No characters yet", "Read a chapter and the AI starts remembering who is who.")
                            .compact()
                            .action(btn("brain-read", "Read this chapter", ButtonKind::Secondary).icon(glyphs::READ).when_some(blocker, |b, r| b.disabled(r)).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_read(false, cx)))),
                    ),
                )
            })
            .children(chars.into_iter().map(|c| {
                let editing = renaming.as_ref().filter(|(id, _)| *id == c.id).map(|(_, s)| s.clone());
                let (id1, id2) = (c.id.clone(), c.id.clone());
                let (m1, t1) = (mid.clone(), title.clone());
                let (m2, t2) = (mid.clone(), title.clone());
                let removed = c.clone();
                let locked = c.user_locked;
                let group: SharedString = format!("char-{}", c.id).into();
                card()
                    .gap(px(space::SM))
                    .p(px(space::MD))
                    .group(group.clone())
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(10.))
                            .child(face_avatar(c.faces.first().cloned(), 44.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(3.))
                                    .map(|d| match &editing {
                                        Some(st) => d.child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(space::XS))
                                                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                                                    if ev.keystroke.key == "escape" {
                                                        this.renaming = None;
                                                        this.rename_sub = None;
                                                        cx.stop_propagation();
                                                        cx.notify();
                                                    }
                                                }))
                                                .child(Input::new(st).small())
                                                .child(div().flex().items_center().gap(px(space::XS)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(kbd("enter")).child("save").child(kbd("escape")).child("cancel")),
                                        ),
                                        None => d.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.))
                                                .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(c.name.clone()))
                                                .when(locked, |d| d.child(icon_ph(ph::LOCK_SIMPLE, Tokens::text_3(), icon_px::XS))),
                                        ),
                                    })
                                    .when(!c.description.is_empty(), |d| d.child(div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_2()).line_clamp(3).child(c.description.clone())))
                                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!(
                                        "Seen {} time{}{}",
                                        c.appearances,
                                        plural(c.appearances as usize),
                                        if c.aliases.is_empty() { String::new() } else { format!(" · aka {}", c.aliases.join(", ")) }
                                    ))),
                            )
                            // Secondary actions show on hover, so a long list stays scannable;
                            // they stay while renaming and whenever the keyboard is in use (Tab
                            // must reach Rename and Forget: WCAG 2.1.1).
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .gap(px(2.))
                                    .when(editing.is_none() && !keyboard, |d| d.invisible().group_hover(group.clone(), |s| s.visible()))
                                    .child(icon_button(SharedString::from(format!("rn-{id1}")), glyphs::EDIT, "Rename").small().on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                        this.start_rename(&id1, window, cx);
                                    })))
                                    .child(
                                        icon_toggle(SharedString::from(format!("lk-{id2}")), ph::LOCK_SIMPLE, locked, Tokens::accent(), "Let the AI rename it again", "Keep this name (the AI won't rename it)")
                                            .size(icon_px::SM)
                                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                                run::edit_brain(&m1, &t1, |b| {
                                                    if let Some(ch) = b.characters.iter_mut().find(|x| x.id == id2) {
                                                        ch.user_locked = !ch.user_locked;
                                                    }
                                                });
                                                cx.notify();
                                            })),
                                    )
                                    .child(icon_button(SharedString::from(format!("rm-{}", c.id)), glyphs::DELETE, "Forget character").small().danger().on_click(cx.listener(
                                        move |_, _: &ClickEvent, _, cx| {
                                            forget_character(&m2, &t2, removed.clone(), cx);
                                            cx.notify();
                                        },
                                    ))),
                            ),
                    )
                    .when(c.faces.len() > 1, |d| d.child(div().flex().gap(px(space::XS)).children(c.faces.iter().skip(1).take(5).map(|p| face_avatar(Some(p.clone()), 28.)))))
            }))
    }

    fn start_rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = &self.focus else { return };
        let brain = run::brain_snapshot(&f.item.manga_id, &f.item.manga_title);
        let Some(c) = brain.characters.iter().find(|c| c.id == id) else { return };
        let state = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("Character name");
            s.set_value(c.name.clone(), window, cx);
            s
        });
        let id = id.to_string();
        let sub = cx.subscribe_in(&state, window, move |this: &mut Self, st, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                let name = st.read(cx).value().trim().to_string();
                this.finish_rename(&id, name, cx);
            }
        });
        let fh = state.read(cx).focus_handle(cx);
        fh.focus(window, cx);
        self.renaming = Some((c.id.clone(), state));
        self.rename_sub = Some(sub);
        cx.notify();
    }

    fn finish_rename(&mut self, id: &str, name: String, cx: &mut Context<Self>) {
        if self.renaming.as_ref().map(|r| r.0.as_str()) != Some(id) {
            return;
        }
        self.renaming = None;
        self.rename_sub = None;
        let Some(f) = &self.focus else { return };
        if !name.is_empty() {
            let id = id.to_string();
            let mut merged = None;
            run::edit_brain(&f.item.manga_id, &f.item.manga_title, |b| {
                // Renaming onto an existing character merges the two.
                let other = b.characters.iter().find(|c| c.id != id && c.name.eq_ignore_ascii_case(&name)).map(|c| c.id.clone());
                match other {
                    Some(keep) => {
                        b.merge(&keep, &id);
                        merged = Some(name.clone());
                    }
                    None => {
                        b.rename(&id, &name);
                        if let Some(c) = b.characters.iter_mut().find(|c| c.id == id) {
                            c.user_locked = true;
                        }
                    }
                }
            });
            if let Some(n) = merged {
                toast::show(cx, Toast::success(format!("Merged into {n}")).icon(ph::USERS));
            }
        }
        cx.notify();
    }
}

/// Removes a character from the manga's brain, with Undo: the faces stay on disk, so putting
/// the record back restores it exactly (NN/g: offer undo for destructive actions).
fn forget_character(manga_id: &str, title: &str, c: mv_recap::Character, cx: &mut App) {
    let id = c.id.clone();
    run::edit_brain(manga_id, title, |b| b.remove(&id));
    let (m, t, name) = (manga_id.to_string(), title.to_string(), c.name.clone());
    toast::show(
        cx,
        Toast::success(format!("Forgot {name}")).icon(glyphs::DELETE).action("Undo", move |_, _| {
            let c = c.clone();
            run::edit_brain(&m, &t, move |b| {
                if !b.characters.iter().any(|x| x.id == c.id) {
                    b.characters.push(c);
                }
            });
        }),
    );
}
