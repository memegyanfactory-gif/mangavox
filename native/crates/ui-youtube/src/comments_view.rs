//! Community: the comments on the channel's videos, laid out like Studio's (author, time, text,
//! likes, then the replies indented under it), filtered by video or by what still needs an answer,
//! and answered right here (`comments.insert`). What viewers say is tallied for the suggestions.
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{inline_alert, ph, spinner, AlertKind};
use mv_youtube::comments::{feedback, Comment, Thread};

use crate::hub::{self, fmt_num};
use crate::page::{yt_hint as hint, Tab, YouTubePage};
use crate::yt;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Show {
    All,
    Unanswered,
    Questions,
}

pub(crate) struct CommentsState {
    pub show: Show,
    /// Only this video's threads.
    pub video: Option<String>,
    /// The thread the reply box is open on.
    pub replying_to: Option<String>,
    pub reply: Entity<InputState>,
    /// Threads whose replies are folded away.
    pub folded: Vec<String>,
    /// The reply went out: empty the box on the next frame (it needs the window).
    pub clear: bool,
}

impl CommentsState {
    pub fn new(window: &mut Window, cx: &mut Context<YouTubePage>) -> Self {
        let reply = cx.new(|cx| InputState::new(window, cx).placeholder("Add a reply…"));
        cx.subscribe_in(&reply, window, |this: &mut YouTubePage, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::PressEnter { .. } => this.send_reply(cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        })
        .detach();
        Self { show: Show::All, video: None, replying_to: None, reply, folded: vec![], clear: false }
    }
}

impl YouTubePage {
    fn send_reply(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.comments.replying_to.clone() else { return };
        let text = self.comments.reply.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let page = cx.entity().downgrade();
        let sent = thread.clone();
        self.hub.update(cx, |h, cx| {
            h.reply(thread, text, cx, move |ok, cx| {
                if ok {
                    let _ = page.update(cx, |p, cx| {
                        // The user may have opened another thread's box meanwhile: leave that one.
                        if p.comments.replying_to.as_deref() == Some(sent.as_str()) {
                            p.comments.replying_to = None;
                            p.comments.clear = true;
                        }
                        cx.notify();
                    });
                }
            })
        });
    }

    pub(crate) fn community(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if std::mem::take(&mut self.comments.clear) {
            self.comments.reply.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let h = self.hub.read(cx);
        let can_reply = h.account.can_write_comments();
        let st = &self.comments;
        let threads: Vec<Thread> = h
            .threads
            .iter()
            .filter(|t| st.video.as_ref().is_none_or(|v| &t.video_id == v))
            .filter(|t| match st.show {
                Show::All => true,
                Show::Unanswered => !t.top.mine && !t.answered(),
                Show::Questions => t.top.text.contains('?') && !t.answered(),
            })
            .cloned()
            .collect();
        let busy = h.comments_busy;
        let error = h.comments_error.clone();
        let more = h.threads_next.is_some();
        let fb = feedback(&h.threads);
        // Videos with comments, most discussed first, for the filter chips.
        let mut by_video: Vec<(String, String, usize)> = vec![];
        for t in &h.threads {
            match by_video.iter_mut().find(|v| v.0 == t.video_id) {
                Some(v) => v.2 += 1,
                None => by_video.push((t.video_id.clone(), h.video(&t.video_id).map(|v| v.title.clone()).unwrap_or_else(|| "Video".into()), 1)),
            }
        }
        by_video.sort_by_key(|v| std::cmp::Reverse(v.2));
        by_video.truncate(6);

        let chips = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .py(px(12.))
            .border_b_1()
            .border_color(yt::tone(0.1))
            .children([(Show::All, "All comments"), (Show::Unanswered, "Unanswered"), (Show::Questions, "Questions")].into_iter().enumerate().map(|(i, (s, label))| {
                let on = st.show == s;
                yt::chip(("cm-show", i), label, on).on_click(cx.listener(move |this, _, _, cx| {
                    this.comments.show = s;
                    cx.notify();
                }))
            }))
            .child(div().w(px(1.)).h(px(24.)).bg(yt::tone(0.2)))
            .child(yt::chip("cm-all-videos", "All videos", st.video.is_none()).on_click(cx.listener(|this, _, _, cx| {
                this.comments.video = None;
                cx.notify();
            })))
            .children(by_video.into_iter().enumerate().map(|(i, (id, title, n))| {
                let on = st.video.as_deref() == Some(id.as_str());
                let short: String = if title.chars().count() > 28 { format!("{}…", title.chars().take(27).collect::<String>()) } else { title.clone() };
                yt::chip(("cm-video", i), format!("{short} ({n})"), on).tooltip(mv_ui::tip(title)).on_click(cx.listener(move |this, _, _, cx| {
                    this.comments.video = Some(id.clone());
                    cx.notify();
                }))
            }))
            .child(div().flex_1())
            .when(busy, |d| d.child(spinner("cm-spin", yt::c(yt::TEXT_2), 16.)))
            .child(yt::icon_btn("cm-refresh", ph::ARROWS_CLOCKWISE, "Read comments again (1 quota unit)").on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.load_comments(true, cx)))));

        let list = if threads.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.))
                .py(px(64.))
                .child(yt::icon_of(ph::CHAT_CIRCLE_DOTS, 64., yt::TEXT_3))
                .child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT_2).child(if busy { "Reading comments…" } else { "No comments found. Try another filter." }))
                .into_any_element()
        } else {
            div().flex().flex_col().children(threads.into_iter().map(|t| self.thread_row(t, can_reply, window, cx))).into_any_element()
        };

        let summary = yt::card()
            .w(px(300.))
            .flex_none()
            .child(yt::card_title("What viewers say"))
            .child(yt::caption(format!("From {} comments read", fb.comments)))
            .child(yt::metric_row("Need an answer", fmt_num(fb.unanswered)))
            .child(yt::metric_row("Questions", fmt_num(fb.questions)))
            .child(yt::metric_row("Positive", fmt_num(fb.positive)))
            .child(yt::metric_row("Critical", fmt_num(fb.negative)))
            .child(yt::divider())
            .child(yt::heading("Asked to recap next", None))
            .when(fb.requests.is_empty(), |d| d.child(yt::caption("No requests yet")))
            .children({
                let mut r: Vec<_> = fb.requests.into_iter().collect();
                r.sort_by_key(|x| std::cmp::Reverse(x.1));
                r.into_iter().take(6).map(|(title, n)| yt::metric_row(capitalize_words(&title), format!("{n}×")))
            })
            .child(hint("Requests raise those manga in Automation › Discover."));

        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .when(!can_reply, |d| {
                d.child(
                    inline_alert(AlertKind::Warning, "Connect again to reply", "Reading works now. Replying needs Google's \"manage your YouTube comments\" permission, which this sign-in does not have yet. Connect your channel again in Settings and allow it.")
                        .action("Open Settings", cx.listener(|this, _, _, cx| this.set_tab(Tab::Settings, cx)))
                        .build(),
                )
            })
            .children(error.map(|e| inline_alert(AlertKind::Error, "Could not read comments", e).build()))
            .child(chips)
            .child(div().flex().items_start().gap(px(yt::GAP)).child(div().flex().flex_col().flex_1().min_w_0().child(list).when(more, |d| d.child(div().pt(px(16.)).child(yt::pill("cm-more", "Load older comments").on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.more_comments(cx)))))))).child(summary))
            .into_any_element()
    }

    fn thread_row(&self, t: Thread, can_reply: bool, _window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let h = self.hub.read(cx);
        let video = h.video(&t.video_id).cloned();
        let open = self.comments.replying_to.as_deref() == Some(t.id.as_str());
        let sending = h.replying.contains(&t.id);
        let folded = self.comments.folded.contains(&t.id);
        let missing = (t.reply_count as usize).saturating_sub(t.replies.len());
        let (id1, id2, id3, id4) = (t.id.clone(), t.id.clone(), t.id.clone(), t.id.clone());
        div()
            .id(SharedString::from(format!("cm-{}", t.id)))
            .flex()
            .gap(px(16.))
            .py(px(16.))
            .min_w_0()
            .border_b_1()
            .border_color(yt::tone(0.1))
            .child(yt::avatar(&t.top.author_avatar, 40.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(comment_head(&t.top))
                    .child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT).min_w_0().whitespace_normal().child(t.top.text.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .pt(px(4.))
                            .child(yt::stat(ph::THUMBS_UP, fmt_num(t.top.likes)))
                            .when(can_reply && t.can_reply, |d| {
                                d.child(div().id(SharedString::from(format!("cm-r-{}", t.id))).cursor_pointer().on_click(cx.listener(move |this, _, w, cx| {
                                    // One box: text typed for another thread does not follow it here.
                                    if this.comments.replying_to.as_deref() != Some(id1.as_str()) {
                                        this.comments.reply.update(cx, |s, cx| s.set_value("", w, cx));
                                    }
                                    this.comments.replying_to = Some(id1.clone());
                                    this.comments.reply.update(cx, |s, cx| s.focus(w, cx));
                                    cx.notify();
                                })).child(yt::text(12., 18., FontWeight::MEDIUM, yt::TEXT).child("Reply")))
                            })
                            .when(t.answered(), |d| d.child(yt::status("Answered", yt::GREEN)))
                            .when(!t.replies.is_empty(), |d| {
                                d.child(div().id(SharedString::from(format!("cm-f-{}", t.id))).flex().items_center().gap(px(4.)).cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                    let f = &mut this.comments.folded;
                                    if let Some(i) = f.iter().position(|x| x == &id2) {
                                        f.remove(i);
                                    } else {
                                        f.push(id2.clone());
                                    }
                                    cx.notify();
                                })).child(yt::icon_of(if folded { ph::CARET_DOWN } else { ph::CARET_UP }, 14., yt::LINK)).child(yt::text(12., 18., FontWeight::MEDIUM, yt::LINK).child(format!("{} repl{}", t.reply_count, if t.reply_count == 1 { "y" } else { "ies" }))))
                            }),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .pt(px(8.))
                                .child(div().border_b_1().border_color(yt::tone(0.3)).child(Input::new(&self.comments.reply).appearance(false)))
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(px(8.))
                                        .child(yt::pill("cm-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| {
                                            this.comments.replying_to = None;
                                            cx.notify();
                                        })))
                                        .child(yt::pill_primary("cm-send", if sending { "Sending…" } else { "Reply" }).on_click(cx.listener(|this, _, _, cx| this.send_reply(cx)))),
                                ),
                        )
                    })
                    .when(!folded, |d| {
                        d.children(t.replies.iter().map(|r| {
                            div()
                                .flex()
                                .gap(px(12.))
                                .pt(px(12.))
                                .min_w_0()
                                .child(yt::avatar(&r.author_avatar, 24.))
                                .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(comment_head(r)).child(yt::text(14., 20., FontWeight::NORMAL, yt::TEXT).min_w_0().whitespace_normal().child(r.text.clone())).child(div().pt(px(2.)).child(yt::stat(ph::THUMBS_UP, fmt_num(r.likes)))))
                        }))
                        .when(missing > 0, |d| {
                            d.child(div().id(SharedString::from(format!("cm-m-{}", id3))).pt(px(8.)).cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.hub.update(cx, |h, cx| h.load_replies(id4.clone(), cx)))).child(yt::text(12., 18., FontWeight::MEDIUM, yt::LINK).child(format!("Show {missing} more repl{}", if missing == 1 { "y" } else { "ies" }))))
                        })
                    }),
            )
            .children(video.map(|v| {
                let url = v.url();
                div()
                    .id(SharedString::from(format!("cm-v-{}", t.id)))
                    .flex()
                    .flex_none()
                    .gap(px(8.))
                    .w(px(220.))
                    .cursor_pointer()
                    .tooltip(mv_ui::tip("Watch on YouTube"))
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .child(yt::thumb(&v.thumbnail, 88., None))
                    .child(yt::para(12., yt::TEXT_2, 3, v.title.clone()).flex_1())
            }))
    }
}

/// "@author · 2 days ago", the channel's own name in a grey pill like Studio's creator tag.
fn comment_head(c: &Comment) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .min_w_0()
        .child(if c.mine {
            div().px(px(6.)).rounded(px(10.)).bg(yt::tone(0.2)).child(yt::line(12., FontWeight::MEDIUM, yt::TEXT, c.author.clone())).into_any_element()
        } else {
            yt::line(13., FontWeight::MEDIUM, yt::TEXT, c.author.clone()).into_any_element()
        })
        .child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).flex_none().whitespace_nowrap().child(format!("· {}", hub::fmt_ago(c.published_at))))
        .when(c.updated_at > c.published_at + 60, |d| d.child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).flex_none().child("(edited)")))
}

fn capitalize_words(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
