//! Automation: the control centre that runs the channel. The nine stages across the top with how
//! many videos are at each, the full-auto switch (impossible to miss when it is on), Discover's
//! ranked manga, every video in production with its stage, progress and release time, and the log
//! of everything the automation did.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{ph, spinner, Ph};
use mv_youtube::automation::{Job, JobState, LogKind, Slot, Stage, DEFAULT_SLOTS};

use crate::automation::runner;
use crate::hub::fmt_num;
use crate::page::{yt_hint as hint, YouTubePage};
use crate::{when, yt};

pub(crate) struct AutoViewState {
    pub log_rows: usize,
}

impl AutoViewState {
    pub fn new(_window: &mut Window, _cx: &mut Context<YouTubePage>) -> Self {
        Self { log_rows: 40 }
    }
}

fn stage_icon(s: Stage) -> Ph {
    match s {
        Stage::Discover => ph::MAGNIFYING_GLASS,
        Stage::Download => ph::DOWNLOAD_SIMPLE,
        Stage::Read => ph::BOOK_OPEN,
        Stage::Write => ph::PENCIL_SIMPLE,
        Stage::Render => ph::FILM_SLATE,
        Stage::Approve => ph::SEAL_CHECK,
        Stage::Schedule => ph::CALENDAR_CHECK,
        Stage::Publish => ph::UPLOAD_SIMPLE,
        Stage::Collect => ph::CHART_LINE_UP,
        Stage::Done => ph::CHECK_CIRCLE,
    }
}

impl YouTubePage {
    pub(crate) fn automation(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let r = runner(cx);
        let rr = r.read(cx);
        let st = rr.state.clone();
        let discovering = rr.discovering;
        let step = rr.discover_step.lock().map(|s| s.clone()).unwrap_or_default();
        let join = rr.join_progress.lock().map(|s| s.clone()).unwrap_or_default();
        let connected = self.hub.read(cx).connected();

        // Full auto: the big switch.
        let on = st.full_auto;
        let switch = div()
            .id("au-full")
            .flex()
            .flex_none()
            .items_center()
            .w(px(56.))
            .h(px(32.))
            .p(px(3.))
            .rounded(px(16.))
            .cursor_pointer()
            .bg(if on { yt::c(yt::RED).into() } else { yt::tone(0.2) })
            .when(on, |d| d.justify_end())
            .on_click(cx.listener(move |_, _, _, cx| runner(cx).update(cx, |r, cx| r.set_full_auto(!on, cx))))
            .child(div().size(px(26.)).rounded_full().bg(rgb(0xffffff)));
        let banner = div()
            .flex()
            .items_center()
            .gap(px(20.))
            .p(px(20.))
            .rounded(px(16.))
            .border_1()
            .border_color(if on { yt::c(yt::RED).into() } else { yt::tone(0.2) })
            .bg(if on { hsla(0., 1., 0.5, 0.08) } else { yt::tone(0.03) })
            .child(yt::icon_of(ph::ROBOT, 40., if on { yt::RED } else { yt::TEXT_2 }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(div().flex().items_center().gap(px(10.)).child(yt::text(20., 28., FontWeight::BOLD, yt::TEXT).child(if on { "Full auto is ON" } else { "Full auto is off" })).when(on, |d| d.child(yt::badge("LIVE", 0xffffff, yt::c(yt::RED).into()))))
                    .child(yt::para(
                        14.,
                        yt::TEXT_2,
                        3,
                        if on {
                            "MangaVox picks what viewers want, downloads every chapter, reads them, writes the whole story, renders it, schedules it at the best time and publishes it on YouTube, all without asking. Everything it does is in the log below."
                        } else {
                            "Each step runs on its own, but finished renders wait for your approval before they are scheduled. Turn on full auto to let it run the channel from discovery to publishing."
                        },
                    )),
            )
            .child(switch);

        // The nine stages, with the number of videos at each.
        let counts: Vec<usize> = Stage::PIPELINE.iter().map(|s| st.jobs.iter().filter(|j| j.active() && j.stage == *s).count()).collect();
        let pipeline = div().flex().flex_wrap().gap(px(8.)).children(Stage::PIPELINE.iter().zip(counts).enumerate().map(|(i, (s, n))| {
            div()
                .id(("au-stage", i))
                .flex()
                .flex_col()
                .gap(px(6.))
                .flex_1()
                .min_w(px(104.))
                .p(px(12.))
                .rounded(px(12.))
                .border_1()
                .border_color(if n > 0 { yt::c(yt::LINK).into() } else { yt::tone(0.15) })
                .bg(if n > 0 { hsla(0.57, 1., 0.62, 0.08) } else { yt::tone(0.0) })
                .tooltip(mv_ui::tip(s.detail()))
                .child(div().flex().items_center().justify_between().child(yt::icon_of(stage_icon(*s), 20., if n > 0 { yt::LINK } else { yt::TEXT_2 })).child(yt::text(11., 14., FontWeight::MEDIUM, yt::TEXT_3).child(format!("{}", i + 1))))
                .child(yt::line(13., FontWeight::MEDIUM, yt::TEXT, s.label()))
                .child(yt::line(12., FontWeight::NORMAL, if n > 0 { yt::LINK } else { yt::TEXT_2 }, if n == 0 { "—".to_string() } else { format!("{n} video{}", if n == 1 { "" } else { "s" }) }))
        }));

        // Options.
        let opt_row = |label: &'static str, sub: String, control: AnyElement| {
            // Wraps the control under the label when both don't fit, so a wide chip row never squeezes the label away.
            div().flex().flex_wrap().items_center().gap(px(16.)).min_w_0().child(div().flex().flex_col().flex_1().min_w(px(180.)).child(yt::line(14., FontWeight::MEDIUM, yt::TEXT, label)).child(yt::para(12., yt::TEXT_2, 2, sub))).child(div().flex_none().child(control))
        };
        let chips = |id: &'static str, items: Vec<(usize, String)>, cur: usize, set: fn(&mut mv_youtube::automation::Automation, usize), cx: &mut Context<Self>| {
            div()
                .flex()
                .gap(px(6.))
                .children(items.into_iter().enumerate().map(move |(i, (v, label))| yt::chip((id, i), label, cur == v).on_click(cx.listener(move |_, _, _, cx| runner(cx).update(cx, |r, cx| r.set_options(cx, |s| set(s, v)))))))
                .into_any_element()
        };
        let slots_text = st.slots.iter().map(|s| when::clock_of(s.hour as u32, s.minute as u32)).collect::<Vec<_>>().join(", ");
        let options = yt::card()
            .child(yt::card_title("How it runs"))
            .child(opt_row("Chapters per video", "Long recaps do best; every chapter is downloaded and read before writing".into(), chips("au-ch", vec![(20, "20".into()), (30, "30".into()), (50, "50".into()), (0, "All".into())], st.max_chapters, |s, v| s.max_chapters = v, cx)))
            .child(opt_row("Release rhythm", "Days between two automatic releases".into(), chips("au-cad", vec![(1, "Daily".into()), (2, "Every 2 days".into()), (3, "Every 3 days".into()), (7, "Weekly".into())], st.cadence_days as usize, |s, v| s.cadence_days = v as u32, cx)))
            .child(opt_row("Videos at a time", "How many recaps are made in parallel".into(), chips("au-par", vec![(1, "1".into()), (2, "2".into()), (3, "3".into())], st.parallel, |s, v| s.parallel = v, cx)))
            .child(opt_row(
                "Approve renders",
                if on { "Skipped while full auto is on".into() } else { "Finished videos wait for you before they are scheduled".to_string() },
                chips("au-appr", vec![(1, "Ask me".into()), (0, "Don't ask".into())], st.approve_renders as usize, |s, v| s.approve_renders = v == 1, cx),
            ))
            .child(opt_row(
                "Your release times",
                format!("{slots_text} your time ({}); combined with recap channels' golden hours, Sunday/Saturday strength and, as data comes in, your own videos' results", when::zone()),
                chips("au-slots", vec![(0, "5:30 / 6:30 / 6:45 PM".into()), (1, "7 / 8 PM".into()), (2, "Noon / 3 PM".into())], slot_preset(&st.slots), |s, v| {
                    s.slots = match v {
                        1 => vec![Slot::new(19, 0), Slot::new(20, 0)],
                        2 => vec![Slot::new(12, 0), Slot::new(15, 0)],
                        _ => DEFAULT_SLOTS.to_vec(),
                    }
                }, cx),
            ))
            .when(!connected, |d| d.child(div().p(px(12.)).rounded(px(8.)).bg(hsla(0.11, 1., 0.5, 0.1)).child(yt::para(13., yt::AMBER, 2, "Connect your YouTube channel (Settings) so finished recaps can be uploaded and scheduled. Everything before that runs already."))));

        // Discover.
        let picks = st.picks.clone();
        let discover = yt::card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().flex().flex_col().flex_1().min_w_0().child(yt::card_title("Discover")).child(yt::caption(if st.picks_at == 0 { "What manga recaps viewers love right now, ranked".to_string() } else { format!("Read {}", when::when(st.picks_at)) })))
                    .when(discovering, |d| d.child(spinner("au-disc-spin", yt::c(yt::LINK), 16.)))
                    .child(yt::pill_icon("au-discover", ph::MAGNIFYING_GLASS, if discovering { "Reading YouTube…" } else { "Find what's hot now" }).on_click(cx.listener(|_, _, _, cx| runner(cx).update(cx, |r, cx| r.discover(cx))))),
            )
            .when(discovering, |d| d.child(yt::line(13., FontWeight::NORMAL, yt::TEXT_2, step)))
            .when(picks.is_empty() && !discovering, |d| d.child(hint("Searches recap videos on YouTube (this week, this month, by views), works out which manga each covers, and ranks them by demand, growth and how few channels cover them, plus what your own viewers ask for in comments. Public pages only, no API quota.")))
            .children(picks.into_iter().take(10).enumerate().map(|(i, p)| {
                let pick = p.clone();
                let example = p.example.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .py(px(8.))
                    .min_w_0()
                    .border_t_1()
                    .border_color(yt::tone(0.08))
                    .child(yt::text(14., 20., FontWeight::BOLD, yt::TEXT_2).w(px(22.)).flex_none().child(format!("{}", i + 1)))
                    .child(div().w(px(44.)).h(px(62.)).flex_none().rounded(px(6.)).overflow_hidden().bg(yt::tone(0.1)).when(!p.cover.is_empty(), |d| d.child(img(SharedString::from(p.cover.clone())).size_full().object_fit(ObjectFit::Cover))))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(3.))
                            .child(div().flex().items_center().gap(px(8.)).min_w_0().child(yt::line(15., FontWeight::MEDIUM, yt::TEXT, p.title.clone())).when(p.asked > 0, |d| d.child(yt::badge(format!("{} asked", p.asked), 0x0f0f0f, yt::c(yt::AMBER).into()))))
                            .child(yt::para(12., yt::TEXT_2, 2, p.reason.clone()))
                            .children(example.map(|(t, url)| div().id(("au-ex", i)).cursor_pointer().on_click(move |_, _, cx| cx.open_url(&url)).child(yt::line(12., FontWeight::NORMAL, yt::LINK, format!("Hot recap: {t}"))))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(6.))
                            .flex_none()
                            .w(px(120.))
                            .child(div().flex().items_center().gap(px(6.)).child(div().w(px(60.)).h(px(6.)).rounded(px(3.)).bg(yt::tone(0.1)).child(div().h_full().rounded(px(3.)).w(relative(p.score as f32 / 100.)).bg(yt::c(yt::GREEN)))).child(yt::text(13., 18., FontWeight::BOLD, yt::TEXT).child(format!("{}", p.score))))
                            .child(yt::pill(("au-make", i), "Make this").on_click(cx.listener(move |_, _, _, cx| {
                                let p = pick.clone();
                                runner(cx).update(cx, |r, cx| r.start_job(p, false, cx));
                            }))),
                    )
            }));

        // Jobs.
        let mut jobs = st.jobs.clone();
        jobs.sort_by_key(|j| (!j.active(), std::cmp::Reverse(j.updated_at)));
        let jobs_card = yt::card()
            .child(div().flex().items_center().child(yt::card_title("In production").flex_1()).child(yt::caption(format!("{} active · {} done", jobs.iter().filter(|j| j.active()).count(), jobs.iter().filter(|j| j.state == JobState::Done).count()))))
            .when(jobs.is_empty(), |d| d.child(hint("Pick a manga in Discover (or turn on full auto) and it shows up here, stage by stage.")))
            .children(jobs.into_iter().take(20).map(|j| self.job_row(j, &join, cx)));

        // Log.
        let rows = self.auto.log_rows;
        let log: Vec<_> = st.log.iter().rev().take(rows).cloned().collect();
        let total = st.log.len();
        let log_card = yt::card()
            .child(div().flex().items_center().child(yt::card_title("What it did").flex_1()).child(yt::caption(format!("{total} entries"))))
            .when(log.is_empty(), |d| d.child(hint("Every step the automation takes is written here.")))
            .children(log.into_iter().map(|e| {
                let (icon, color) = match e.kind {
                    LogKind::Info => (ph::INFO, yt::TEXT_2),
                    LogKind::Success => (ph::CHECK_CIRCLE, yt::GREEN),
                    LogKind::Warning => (ph::WARNING, yt::AMBER),
                    LogKind::Error => (ph::WARNING_CIRCLE, yt::ERROR),
                };
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .py(px(4.))
                    .min_w_0()
                    .child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).w(px(130.)).flex_none().whitespace_nowrap().child(when::when(e.at)))
                    .child(div().pt(px(1.)).flex_none().child(yt::icon_of(icon, 16., color)))
                    .child(yt::text(13., 18., FontWeight::NORMAL, yt::TEXT).flex_1().min_w_0().whitespace_normal().child(e.text.clone()))
                    .when(e.auto, |d| d.child(yt::badge("AUTO", 0xffffff, yt::c(yt::RED).into())))
            }))
            .when(total > rows, |d| {
                d.child(yt::pill("au-log-more", "Show older").on_click(cx.listener(|this, _, _, cx| {
                    this.auto.log_rows += 60;
                    cx.notify();
                })))
            });

        div()
            .flex()
            .flex_col()
            .gap(px(yt::GAP))
            .child(banner)
            .child(pipeline)
            .child(div().flex().flex_wrap().items_start().gap(px(yt::GAP)).child(div().flex().flex_col().gap(px(yt::GAP)).flex_1().min_w(px(460.)).child(jobs_card).child(discover)).child(div().flex().flex_col().gap(px(yt::GAP)).w(px(420.)).min_w(px(340.)).child(options).child(log_card)))
            .into_any_element()
    }

    fn job_row(&self, j: Job, join: &(f32, String), cx: &mut Context<Self>) -> Div {
        let id = j.id.clone();
        let progress = if j.stage == Stage::Render && j.step.starts_with("Joining") { 4. / 9. + join.0 / 9. } else { j.progress() };
        let (state_label, color) = match j.state {
            JobState::Running => ("Running", yt::LINK),
            JobState::Waiting => ("Needs you", yt::AMBER),
            JobState::Paused => ("Paused", yt::TEXT_2),
            JobState::Failed => ("Stopped", yt::ERROR),
            JobState::Done => ("Done", yt::GREEN),
        };
        let stage_dots = div().flex().items_center().gap(px(4.)).children(Stage::PIPELINE.iter().map(|s| {
            let done = *s < j.stage || j.stage == Stage::Done;
            let now = *s == j.stage && j.stage != Stage::Done;
            div()
                .flex_1()
                .h(px(4.))
                .rounded(px(2.))
                .bg(if done { yt::c(yt::GREEN).into() } else if now { yt::c(if j.state == JobState::Failed { yt::ERROR } else { yt::LINK }).into() } else { yt::tone(0.12) })
        }));
        let (a, b, c, d2, e) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
        let file = j.file.clone();
        div()
            .flex()
            .gap(px(16.))
            .p(px(16.))
            .min_w_0()
            .rounded(px(12.))
            .border_1()
            .border_color(if j.state == JobState::Waiting { yt::c(yt::AMBER).into() } else { yt::tone(0.12) })
            .child(match j.thumbnail.as_ref().filter(|t| t.is_file()) {
                Some(t) => yt::thumb(&t.to_string_lossy(), 160., Some(j.duration_secs).filter(|s| *s > 0.).map(|s| yt::duration(s as u64))).into_any_element(),
                None => div().w(px(64.)).h(px(90.)).flex_none().rounded(px(8.)).overflow_hidden().bg(yt::tone(0.1)).when(!j.pick.cover.is_empty(), |d| d.child(img(SharedString::from(j.pick.cover.clone())).size_full().object_fit(ObjectFit::Cover))).into_any_element(),
            })
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
                            .gap(px(10.))
                            .min_w_0()
                            .child(yt::line(16., FontWeight::MEDIUM, yt::TEXT, j.pick.title.clone()).flex_1())
                            .when(j.auto, |d| d.child(yt::badge("AUTO", 0xffffff, yt::c(yt::RED).into())))
                            .child(yt::status(state_label, color)),
                    )
                    .child(stage_dots)
                    .child(div().flex().items_center().gap(px(8.)).min_w_0().child(yt::icon_of(stage_icon(j.stage), 16., yt::TEXT_2)).child(yt::line(13., FontWeight::MEDIUM, yt::TEXT, j.stage.label()).flex_none()).child(yt::line(13., FontWeight::NORMAL, yt::TEXT_2, format!("· {}", j.step))).child(div().flex_1()).child(yt::text(12., 18., FontWeight::NORMAL, yt::TEXT_2).flex_none().child(format!("{:.0}%", progress * 100.))))
                    .when(!j.chapters.is_empty(), |d| d.child(yt::caption(format!("{} chapters · started {}", j.chapters.len(), when::when(j.created_at)))))
                    .children(j.publish_at.map(|t| yt::para(13., yt::TEXT, 2, format!("Release: {} · {}", when::when(t), j.why_time))))
                    .children(j.results.as_ref().map(|r| div().flex().gap(px(16.)).child(yt::stat(ph::CHART_BAR, fmt_num(r.views))).child(yt::stat(ph::THUMBS_UP, fmt_num(r.likes))).child(yt::stat(ph::CHAT_CIRCLE, fmt_num(r.comments))).child(yt::stat(ph::TIMER, format!("{:.0}% watched", r.retention * 100.)))))
                    .children(j.error.clone().map(|e| yt::para(13., yt::ERROR, 3, e)))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(8.))
                            .when(j.state == JobState::Waiting, |d| {
                                d.child(yt::pill_primary(SharedString::from(format!("au-ok-{a}")), "Approve and schedule").on_click(cx.listener(move |_, _, _, cx| runner(cx).update(cx, |r, cx| r.approve(&a, cx)))))
                            })
                            .children(file.filter(|f| f.is_file()).map(|f| yt::pill_icon(SharedString::from(format!("au-watch-{b}")), ph::PLAY, "Watch the render").on_click(move |_, _, cx| cx.open_url(&format!("file:///{}", f.to_string_lossy().replace('\\', "/"))))))
                            .when(j.state == JobState::Running, |d| d.child(yt::pill_icon(SharedString::from(format!("au-pause-{c}")), ph::PAUSE, "Pause").on_click(cx.listener(move |_, _, _, cx| runner(cx).update(cx, |r, cx| r.pause(&c, cx))))))
                            .when(matches!(j.state, JobState::Paused | JobState::Failed), |d| d.child(yt::pill_icon(SharedString::from(format!("au-resume-{d2}")), ph::PLAY, if j.state == JobState::Failed { "Retry" } else { "Resume" }).on_click(cx.listener(move |_, _, _, cx| runner(cx).update(cx, |r, cx| r.resume(&d2, cx))))))
                            .when(!j.active() || j.state == JobState::Paused, |d| d.child(yt::pill_icon(SharedString::from(format!("au-rm-{e}")), ph::TRASH, "Remove").on_click(cx.listener(move |_, _, _, cx| runner(cx).update(cx, |r, cx| r.remove(&e, cx)))))),
                    ),
            )
    }
}

fn slot_preset(slots: &[Slot]) -> usize {
    match slots.first().map(|s| s.hour) {
        Some(19) => 1,
        Some(12) => 2,
        _ => 0,
    }
}
