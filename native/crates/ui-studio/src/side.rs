//! The right-hand AI panel: the current run and the narration queue (Activity), the
//! manga's character brain (Brain), and the assistant chat (Chat).
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{empty_state, icon, icon_button, size, Tokens};

use crate::page::{plural, vbar, Side, StudioPage, MONO, SIDE_W};
use crate::phases::{face_avatar, progress_line, tag};
use crate::run::{self, RunKind, RunState};
use crate::service::{self, JobView};

impl StudioPage {
    pub(crate) fn side_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.side == Side::Chat && self.dock.is_none() {
            self.dock = ui_assistant::dock(window, cx);
        }
        let tabs = div().flex().items_center().gap(px(2.)).p(px(3.)).rounded(px(9.)).bg(rgba(0x00000038)).border_1().border_color(Tokens::border()).children(
            [(Side::Activity, "Activity", "icons/activity.svg"), (Side::Brain, "Characters", "icons/users.svg"), (Side::Chat, "Chat", "icons/message-square.svg")].map(|(s, label, ic)| {
                let on = self.side == s;
                let fg = if on { Tokens::text_1() } else { Tokens::text_3() };
                div()
                    .id(label)
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .h(px(26.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(size::XS))
                    .text_color(fg)
                    .when(on, |d| d.bg(Tokens::active()))
                    .hover(|st| st.text_color(Tokens::text_1()))
                    .child(icon(ic, fg, 13.))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.side = s;
                        cx.notify();
                    }))
            }),
        );
        let body: AnyElement = match self.side {
            Side::Activity => {
                let body = self.activity(cx).size_full().track_scroll(&self.side_scroll);
                div().relative().flex_1().min_h_0().child(body).child(vbar("side-bar", &self.side_scroll)).into_any_element()
            }
            Side::Brain => {
                let body = self.brain_view(window, cx).size_full().track_scroll(&self.side_scroll);
                div().relative().flex_1().min_h_0().child(body).child(vbar("side-bar", &self.side_scroll)).into_any_element()
            }
            Side::Chat => match self.dock.clone() {
                Some(d) => div().flex_1().min_h_0().child(d).into_any_element(),
                None => div().p(px(16.)).child(empty_state("Chat unavailable", "The assistant could not start.")).into_any_element(),
            },
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDE_W))
            .h_full()
            .map(crate::page::panel)
            .child(div().flex_none().h(px(52.)).flex().items_center().px(px(12.)).border_b_1().border_color(Tokens::border()).child(div().flex_1().child(tabs)))
            .child(body)
            .into_any_element()
    }

    // ------------------------------------------------------------ Activity

    fn activity(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let run = self.run.clone();
        let jobs = self.jobs.clone();
        let any_finished = jobs.iter().any(|j| j.state.finished());
        div()
            .id("studio-activity")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .gap(px(12.))
            .when(run.is_none() && jobs.is_empty(), |d| {
                d.child(
                    div()
                        .pt(px(30.))
                        .child(empty_state("Nothing running", "Press Read it to let the AI read the selected chapters. Its progress and what it learns show up here.")),
                )
            })
            .when_some(run, |d, r| {
                let (status, color) = match &r.state {
                    RunState::Running => ("Working", Tokens::accent()),
                    RunState::Done => ("Done", Tokens::accent()),
                    RunState::Cancelled => ("Stopped", Tokens::text_3()),
                    RunState::Failed(_) => ("Failed", Tokens::danger()),
                };
                let what = match r.kind {
                    RunKind::Read => "Reading",
                    RunKind::Story => "Writing the story",
                };
                let lines: Vec<String> = r.logs.iter().rev().take(80).rev().cloned().collect();
                d.child(
                    card()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(icon(if r.kind == RunKind::Read { "icons/scan-eye.svg" } else { "icons/feather.svg" }, Tokens::text_2(), 14.))
                                .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).child(what))
                                .child(tag(status, color))
                                .when(r.running(), |d| {
                                    d.child(icon_button("act-stop", "icons/circle-stop.svg", true).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                                        run::cancel();
                                        cx.notify();
                                    })))
                                }),
                        )
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Model: {}", r.model)))
                        .when(r.running(), |d| {
                            let label = match (&r.chapter, r.total) {
                                (Some(c), t) if t > 0 => format!("{c} · page {}/{t}", r.done.min(t)),
                                (Some(c), _) => c.clone(),
                                _ => "Starting…".into(),
                            };
                            d.child(progress_line(r.progress(), label))
                        })
                        .when(!r.new_characters.is_empty(), |d| {
                            let n = r.new_characters.len();
                            d.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Met {n} new character{}", plural(n))))
                                    .child(div().flex().flex_wrap().gap(px(5.)).children(r.new_characters.iter().map(|c| tag(c.clone(), Tokens::text_1())))),
                            )
                        })
                        .child(
                            div()
                                .id("act-log")
                                .flex()
                                .flex_col()
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .p(px(8.))
                                .rounded(px(8.))
                                .bg(Tokens::base())
                                .font_family(MONO)
                                .text_size(px(size::XS))
                                .text_color(Tokens::text_2())
                                .children(lines.into_iter().map(|l| div().child(l))),
                        ),
                )
            })
            .when(!jobs.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .pt(px(4.))
                        .child(div().flex_1().text_size(px(size::XS)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_3()).child("AUDIO & VIDEO"))
                        .when(any_finished, |d| {
                            d.child(
                                div()
                                    .id("jobs-clear")
                                    .text_size(px(size::XS))
                                    .text_color(Tokens::text_3())
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(Tokens::text_1()))
                                    .child("Clear finished")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        service::clear_finished();
                                        this.jobs = service::snapshot();
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .children(jobs.into_iter().map(|j| {
                    let (status, color) = match &j.state {
                        JobView::Queued => ("Queued", Tokens::text_3()),
                        JobView::Running => ("Running", Tokens::accent()),
                        JobView::Done => ("Done", Tokens::accent()),
                        JobView::Failed(_) => ("Failed", Tokens::danger()),
                        JobView::Cancelled => ("Cancelled", Tokens::text_3()),
                    };
                    let kind = if j.stages.iter().any(|s| s == "video") { "Video" } else { "Audio" };
                    let id = j.id;
                    card()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(icon(if kind == "Video" { "icons/clapperboard.svg" } else { "icons/audio-lines.svg" }, Tokens::text_2(), 14.))
                                .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).child(format!("{kind} · {}", j.chapter_label)))
                                .child(tag(status, color))
                                .when(!j.state.finished(), |d| {
                                    d.child(icon_button(SharedString::from(format!("job-x-{id}")), "icons/x.svg", true).on_click(move |_, _, _| {
                                        service::cancel(id);
                                    }))
                                }),
                        )
                        .when(!j.state.finished(), |d| d.child(progress_line(j.progress() as f32, j.status.clone())))
                        .when_some(if let JobView::Failed(e) = &j.state { Some(e.clone()) } else { None }, |d, e| {
                            d.child(div().text_size(px(size::XS)).text_color(Tokens::danger()).child(e))
                        })
                }))
            })
    }

    // ------------------------------------------------------------ Brain

    fn brain_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let Some(f) = &self.focus else {
            return div().id("brain").p(px(16.)).child(empty_state("No manga open", "Pick a chapter to see the characters the AI knows for that manga."));
        };
        let (mid, title) = (f.item.manga_id.clone(), f.item.manga_title.clone());
        let brain = run::brain_snapshot(&mid, &title);
        let renaming = self.renaming.clone();
        let mut chars = brain.characters.clone();
        chars.sort_by(|a, b| b.appearances.cmp(&a.appearances).then(a.name.cmp(&b.name)));
        let _ = window;
        div()
            .id("brain")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .pb(px(4.))
                    .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).child(format!("{title} · {} character{}", chars.len(), plural(chars.len()))))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Faces the AI captured while reading. Rename to teach it; giving two the same name merges them.")),
            )
            .when(chars.is_empty(), |d| d.child(div().pt(px(20.)).child(empty_state("No characters yet", "Read a chapter and the AI will start remembering who is who."))))
            .children(chars.into_iter().map(|c| {
                let editing = renaming.as_ref().filter(|(id, _)| *id == c.id).map(|(_, s)| s.clone());
                let (id1, id2, id3) = (c.id.clone(), c.id.clone(), c.id.clone());
                let (m1, t1, m2, t2) = (mid.clone(), title.clone(), mid.clone(), title.clone());
                let locked = c.user_locked;
                card()
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
                                        Some(st) => d.child(Input::new(st).small()),
                                        None => d.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.))
                                                .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(c.name.clone()))
                                                .when(locked, |d| d.child(icon("icons/lock.svg", Tokens::text_3(), 11.))),
                                        ),
                                    })
                                    .when(!c.description.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).line_clamp(3).child(c.description.clone())))
                                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!(
                                        "Seen {} time{}{}",
                                        c.appearances,
                                        plural(c.appearances as usize),
                                        if c.aliases.is_empty() { String::new() } else { format!(" · aka {}", c.aliases.join(", ")) }
                                    ))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(2.))
                                    .child(icon_button(SharedString::from(format!("rn-{id1}")), "icons/pencil.svg", true).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                        this.start_rename(&id1, window, cx);
                                    })))
                                    .child(icon_button(SharedString::from(format!("lk-{id2}")), if locked { "icons/lock.svg" } else { "icons/lock-open.svg" }, true).on_click(cx.listener(
                                        move |_, _: &ClickEvent, _, cx| {
                                            run::edit_brain(&m1, &t1, |b| {
                                                if let Some(ch) = b.characters.iter_mut().find(|x| x.id == id2) {
                                                    ch.user_locked = !ch.user_locked;
                                                }
                                            });
                                            cx.notify();
                                        },
                                    )))
                                    .child(icon_button(SharedString::from(format!("rm-{id3}")), "icons/trash.svg", true).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                        run::edit_brain(&m2, &t2, |b| b.remove(&id3));
                                        cx.notify();
                                    }))),
                            ),
                    )
                    .when(c.faces.len() > 1, |d| d.child(div().flex().gap(px(4.)).children(c.faces.iter().skip(1).take(5).map(|p| face_avatar(Some(p.clone()), 28.)))))
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
            run::edit_brain(&f.item.manga_id, &f.item.manga_title, |b| {
                // Renaming onto an existing character merges the two.
                let other = b.characters.iter().find(|c| c.id != id && c.name.eq_ignore_ascii_case(&name)).map(|c| c.id.clone());
                match other {
                    Some(keep) => b.merge(&keep, &id),
                    None => {
                        b.rename(&id, &name);
                        if let Some(c) = b.characters.iter_mut().find(|c| c.id == id) {
                            c.user_locked = true;
                        }
                    }
                }
            });
        }
        cx.notify();
    }
}

fn card() -> Div {
    div().flex().flex_col().gap(px(8.)).p(px(12.)).rounded(px(10.)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}
