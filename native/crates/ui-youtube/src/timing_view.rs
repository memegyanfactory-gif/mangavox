//! When to upload: the live clock in the rail (your time, and the next golden time), and the
//! Schedule tab's golden-time card built from `mv_youtube::timing` (when other recap channels
//! upload and which hours beat their usual views), in the user's local time.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, icon_ph, spinner, tip, ButtonKind, Tokens};
use mv_ui::{ph, size};
use mv_youtube::timing::{next_at, Analysis, Window, DAYS};

use crate::hub;
use crate::page::{yt_card as card, yt_card_title as card_title, yt_hint as hint, YouTubePage};
use crate::yt;

const FULL_DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// "6 PM".
fn hhmm(hour: usize) -> String {
    crate::when::hour(hour)
}

/// "6–8 PM".
fn span(w: &Window) -> String {
    crate::when::hour_span(w.start_hour, w.hours)
}

/// "UTC+5:30".
fn zone_label(offset: i32) -> String {
    let sign = if offset < 0 { '-' } else { '+' };
    let m = offset.abs() / 60;
    if m % 60 == 0 {
        format!("UTC{sign}{}", m / 60)
    } else {
        format!("UTC{sign}{}:{:02}", m / 60, m % 60)
    }
}

/// "in 5 h 12 min".
fn until(secs: i64) -> String {
    let m = (secs.max(0) + 59) / 60;
    match (m / 60, m % 60) {
        (0, m) => format!("in {m} min"),
        (h, 0) => format!("in {h} h"),
        (h, m) => format!("in {h} h {m} min"),
    }
}

/// "Today 6 PM", "Tomorrow 3 AM", "Sat 6 PM" for a time in local seconds.
fn day_label(at: i64, now: i64, offset: i32) -> String {
    let day = |t: i64| (t + offset as i64).div_euclid(86_400);
    let (d, h) = mv_youtube::timing::local_day_hour(at, offset);
    let when = match day(at) - day(now) {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        _ => DAYS[d].to_string(),
    };
    format!("{when} {}", hhmm(h))
}

fn lift_text(x: f64) -> String {
    if x >= 1. {
        format!("{x:.1}× their usual views")
    } else {
        format!("{:.0}% below their usual views", (1. - x) * 100.)
    }
}

/// The best window and the next time it comes round (on a best day when there are some).
pub(crate) fn next_golden(a: &Analysis) -> Option<(i64, &Window)> {
    let g = a.golden.first()?;
    Some((next_at(hub::now(), hub::utc_offset(), g.start_hour, &a.best_days), g))
}

impl YouTubePage {
    /// The rail's clock: the time here (12-hour), and when the next golden time is.
    pub(crate) fn clock(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let h = self.hub.read(cx);
        let off = hub::utc_offset();
        let now = hub::now();
        let local = crate::when::clock(now);
        let (day, _) = mv_youtube::timing::local_day_hour(now, off);
        let a = h.timing_analysis();
        let next = next_golden(&a).map(|(t, _)| (t, day_label(t, now, off)));
        let busy = h.timing_busy;
        div()
            .id("yt-clock")
            .flex()
            .flex_col()
            .gap(px(2.))
            .min_w_0()
            .mb(px(8.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(12.))
            .bg(yt::tone(0.05))
            .cursor_pointer()
            .hover(|s| s.bg(yt::tone(0.1)))
            .tooltip(tip("Your local time and the next golden upload time (Schedule)"))
            .on_click(cx.listener(|this, _, _, cx| this.set_tab(crate::page::Tab::Schedule, cx)))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(8.))
                    .min_w_0()
                    .child(yt::text(20., 26., FontWeight::BOLD, yt::TEXT).flex_none().whitespace_nowrap().child(local))
                    .child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, format!("{} · {}", FULL_DAYS[day], crate::when::zone()))),
            )
            .child(match next {
                Some((t, label)) => div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .child(yt::icon_of(ph::CLOCK_COUNTDOWN, 14., yt::LINK))
                    .child(yt::line(12., FontWeight::MEDIUM, yt::LINK, format!("Golden time {label} · {}", until(t - now)))),
                None => yt::line(12., FontWeight::NORMAL, yt::TEXT_2, if busy { "Researching upload times…" } else { "Open Schedule for the golden time" }),
            })
    }

    /// Schedule › Golden upload time.
    pub(crate) fn golden_card(&mut self, cx: &mut Context<Self>) -> Div {
        let h = self.hub.read(cx);
        let busy = h.timing_busy;
        let step = h.timing_step.lock().map(|s| s.clone()).unwrap_or_default();
        let error = h.timing_error.clone();
        let researched = h.timing.fetched_at;
        let release_hour = h.settings.publish_hour as usize;
        let a = h.timing_analysis();
        let off = hub::utc_offset();
        let now = hub::now();

        let research_btn = btn("yt-gold-research", if researched == 0 { "Research upload times" } else { "Research again" }, ButtonKind::Secondary)
            .small()
            .icon(ph::MAGNIFYING_GLASS)
            .loading(busy)
            .tooltip("Reads the exact publish time of a few hundred recent recap uploads and the busiest recap channels' schedules (about a minute; no API quota)")
            .on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.research_timing(cx))));
        let head = div().flex().items_center().gap(px(space::SM)).child(div().flex_1().child(card_title(ph::CLOCK_COUNTDOWN, "Golden upload time"))).child(research_btn);
        let mut c = card().child(head);
        if busy {
            c = c.child(div().flex().items_center().gap(px(space::SM)).child(spinner("yt-gold-spin", Tokens::accent(), icon_px::SM)).child(hint(if step.is_empty() { "Researching…".to_string() } else { step })));
        }
        if let Some(e) = error {
            c = c.child(div().text_size(px(size::SM)).text_color(Tokens::danger()).child(e));
        }
        if a.uploads == 0 {
            return c.child(hint("Find out when other manhwa and manga recap channels upload and which hours get them the most views, in your time zone. Press Research upload times."));
        }

        // Hero: the next golden time, and making it the release time.
        let hero = match next_golden(&a) {
            Some((t, g)) => {
                let hour = g.start_hour;
                let using = release_hour == hour;
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::LG))
                    .p(px(space::MD))
                    .rounded(px(radius::LG))
                    .bg(Tokens::accent_soft())
                    .border_1()
                    .border_color(Tokens::accent_border())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(if a.by_habit { "Most-used upload time" } else { "Next golden time" }))
                            .child(div().text_size(px(26.)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(day_label(t, now, off)))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::accent()).child(until(t - now))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(220.))
                            .gap(px(4.))
                            .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Upload {} your time", span(g))))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).whitespace_normal().child(if a.by_habit {
                                format!("{:.0}% of recap uploads go out in this window.", g.share * 100.)
                            } else {
                                format!("Recaps uploaded in this window got {} ({} uploads compared).", lift_text(g.cell.lift()), g.cell.scored)
                            }))
                            .when(!a.best_days.is_empty(), |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("Best days: {}", a.best_days.iter().map(|d| format!("{} ({:.1}×)", DAYS[*d], a.by_day[*d].lift())).collect::<Vec<_>>().join(", "))))),
                    )
                    .child(if using {
                        div().flex().items_center().gap(px(4.)).text_size(px(size::SM)).text_color(Tokens::success()).child(icon_ph(ph::CHECK_CIRCLE, Tokens::success(), icon_px::SM)).child("Your releases go out then").into_any_element()
                    } else {
                        btn("yt-gold-use", format!("Release at {}", hhmm(hour)), ButtonKind::Primary)
                            .icon(ph::CALENDAR_CHECK)
                            .tooltip(format!("Planned releases go public at {} your time (now {}). YouTube publishes them for you; you don't need to be awake.", hhmm(hour), hhmm(release_hour)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.hub.update(cx, |h, cx| {
                                    let mut s = h.settings.clone();
                                    s.publish_hour = hour as u8;
                                    h.save_settings(s, cx);
                                });
                                toast::show(cx, Toast::success(format!("Releases now go public at {} your time", hhmm(hour))).icon(ph::CALENDAR_CHECK));
                            }))
                            .into_any_element()
                    })
            }
            None => div().child(hint("Not enough uploads were read to pick a time; research again.")),
        };

        // The ranked windows.
        let windows = div().flex().flex_wrap().gap(px(space::SM)).children(a.golden.iter().enumerate().map(|(i, w)| {
            let hour = w.start_hour;
            div()
                .id(("yt-gold-win", i))
                .flex()
                .flex_col()
                .gap(px(2.))
                .w(px(210.))
                .p(px(space::SM))
                .rounded(px(radius::LG))
                .border_1()
                .border_color(if i == 0 { Tokens::accent_border() } else { Tokens::border() })
                .cursor_pointer()
                .hover(|s| s.bg(Tokens::hover()))
                .tooltip(tip(format!("Use {} as your release time", hhmm(hour))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.hub.update(cx, |h, cx| {
                        let mut s = h.settings.clone();
                        s.publish_hour = hour as u8;
                        h.save_settings(s, cx);
                    });
                    toast::show(cx, Toast::success(format!("Releases now go public at {} your time", hhmm(hour))).icon(ph::CALENDAR_CHECK));
                }))
                .child(div().flex().items_center().gap(px(6.)).child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(if i == 0 { Tokens::accent() } else { Tokens::text_3() }).child(["BEST", "2ND", "3RD"][i.min(2)])).child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(span(w))))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(if a.by_habit { format!("{} uploads", w.cell.uploads) } else { format!("{:.2}× views · {} compared", w.cell.lift(), w.cell.scored) }))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{:.0}% of channels' uploads", w.share * 100.)))
        }));

        let crowd = a.busiest.as_ref().map(|b| {
            let crowded = a.golden.first().is_some_and(|g| g.start_hour == b.start_hour);
            hint(format!(
                "Most channels upload {} ({:.0}% of uploads){}.",
                span(b),
                b.share * 100.,
                if crowded { ": the best window is also the busiest, so expect competition" } else if a.by_habit { "" } else { "; the golden window above is less crowded" }
            ))
        });

        c.child(hero).child(windows).children(crowd).child(self.heatmap(&a)).child(hint(format!(
            "From {} recent uploads by {} recap channels, read {}. Times are in your time zone ({}). Golden = uploads there beat their own channel's usual views, so a big channel does not win just by being big.",
            a.uploads,
            a.channels,
            hub::fmt_ago(researched),
            zone_label(off)
        )))
    }

    /// Days × hours: grey = nobody uploads, green = uploads there beat their channel's usual
    /// views, red = below it; stronger colour, stronger effect.
    fn heatmap(&self, a: &Analysis) -> Stateful<Div> {
        let cell_w = 22.;
        let head = div().flex().gap(px(2.)).pl(px(36.)).children((0..24).map(|hh| div().w(px(cell_w)).text_size(px(9.)).text_color(Tokens::text_3()).child(if hh % 3 == 0 { crate::when::hour(hh).replace(" AM", "a").replace(" PM", "p") } else { String::new() })));
        let rows = (0..7).map(|d| {
            div().flex().items_center().gap(px(2.)).child(div().w(px(34.)).text_size(px(size::XS)).text_color(if a.best_days.contains(&d) { Tokens::accent() } else { Tokens::text_3() }).child(DAYS[d])).children((0..24).map(move |hh| {
                let c = a.heat[d][hh];
                let bg = if c.uploads == 0 {
                    Tokens::surface_2()
                } else if c.scored == 0 {
                    Tokens::surface_3()
                } else {
                    let l = c.lift().log2().clamp(-1.5, 1.5);
                    let mut col = if l >= 0. { Tokens::success() } else { Tokens::danger() };
                    col.a = (0.18 + l.abs() / 1.5 * 0.72) as f32;
                    col
                };
                div()
                    .id(SharedString::from(format!("yt-heat-{d}-{hh}")))
                    .w(px(cell_w))
                    .h(px(16.))
                    .rounded(px(3.))
                    .bg(bg)
                    .tooltip(tip(if c.uploads == 0 {
                        format!("{} {} · no uploads", DAYS[d], hhmm(hh))
                    } else if c.scored == 0 {
                        format!("{} {} · {} upload{}", DAYS[d], hhmm(hh), c.uploads, if c.uploads == 1 { "" } else { "s" })
                    } else {
                        format!("{} {} · {} upload{} · {:.1}× their usual views", DAYS[d], hhmm(hh), c.uploads, if c.uploads == 1 { "" } else { "s" }, c.lift())
                    }))
            }))
        });
        div().id("yt-heat").flex().flex_col().gap(px(2.)).overflow_x_scroll().child(head).children(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::{span, until, zone_label, Window};

    #[test]
    fn labels() {
        assert_eq!(zone_label(19_800), "UTC+5:30");
        assert_eq!(zone_label(-14_400), "UTC-4");
        assert_eq!(until(3600 * 5 + 12 * 60), "in 5 h 12 min");
        assert_eq!(until(30), "in 1 min");
        let w = Window { start_hour: 23, hours: 2, cell: Default::default(), share: 0. };
        assert_eq!(span(&w), "11 PM–1 AM");
    }
}
