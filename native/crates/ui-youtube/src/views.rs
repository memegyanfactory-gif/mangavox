//! Dashboard, Analytics, Insights and Settings.
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{btn, external_link, icon_ph, inline_alert, ph, segmented, setting_row, size, switch, AlertKind, ButtonKind, Tokens};
use mv_youtube::{Action, Severity, Suggestion, Trend, UploadState};

use crate::chart::{self, bars, kpi, trend_chip};
use crate::hub::{self, fmt_num, fmt_when};
use crate::page::{card, card_title, hint, thumb, Focus, Tab, YouTubePage};

impl YouTubePage {
    // ------------------------------------------------------------ dashboard

    pub(crate) fn dashboard(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let days = 28;
        let views: Vec<f64> = h.channel_daily.iter().map(|d| d.views as f64).collect();
        let minutes: Vec<f64> = h.channel_daily.iter().map(|d| d.watch_minutes).collect();
        let subs: Vec<f64> = h.channel_daily.iter().map(|d| d.subs_gained as f64).collect();
        let last = |v: &[f64]| v.iter().rev().take(days).sum::<f64>();
        let subscribers = h.channel.as_ref().map(|c| fmt_num(c.subscribers)).unwrap_or_else(|| "—".into());
        let tiles = div()
            .flex()
            .flex_wrap()
            .gap(px(space::MD))
            .child(kpi(ph::USERS, "Subscribers", subscribers, None))
            .child(kpi(ph::EYE, "Views · 28 days", fmt_num(last(&views) as u64), chart::change(&views, days)))
            .child(kpi(ph::CLOCK, "Watch time · 28 days", format!("{:.1} h", last(&minutes) / 60.), chart::change(&minutes, days)))
            .child(kpi(ph::USER_PLUS, "Subscribers gained", format!("{:+}", last(&subs) as i64), chart::change(&subs, days)));

        let recent: Vec<_> = h.channel_daily.iter().rev().take(days).rev().collect();
        let views_chart = card()
            .child(card_title(ph::CHART_BAR, "Views, last 28 days"))
            .child(bars("dash-views", &recent.iter().map(|d| d.views as f64).collect::<Vec<_>>(), &recent.iter().map(|d| d.date.clone()).collect::<Vec<_>>(), "views", Tokens::accent(), 140.));

        // The newest public video, like YouTube Studio's "Latest video performance".
        let latest = h.videos.iter().find(|v| v.privacy != "private").cloned();
        let latest_card = latest.map(|v| {
            let trend = h.trend_of(&v.id);
            let daily: Vec<u64> = h.video_daily.get(&v.id).map(|d| d.iter().map(|s| s.views).collect()).unwrap_or_default();
            let id = v.id.clone();
            card()
                .child(card_title(ph::PLAY_CIRCLE, "Latest video performance"))
                .child(div().flex().gap(px(space::MD)).child(thumb(&v.thumbnail, 160.)).child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(space::XS))
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).line_clamp(2).child(v.title.clone()))
                        .child(hint(format!("Published {}", hub::fmt_ago(v.published_at))))
                        .child(div().flex().items_center().gap(px(space::SM)).child(trend_chip(trend)).child(hint(format!("{} views · {} likes · {} comments", fmt_num(v.views), fmt_num(v.likes), fmt_num(v.comments))))),
                ))
                .when(!daily.is_empty(), |d| {
                    let labels: Vec<String> = (0..daily.len()).map(|i| format!("Day {}", i + 1)).collect();
                    d.child(bars("dash-latest", &daily.iter().map(|v| *v as f64).collect::<Vec<_>>(), &labels, "views", Tokens::info(), 60.))
                })
                .child(btn("dash-edit-latest", "Improve title & description", ButtonKind::Secondary).small().icon(ph::SPARKLE).on_click(cx.listener(move |this, _, w, cx| {
                    this.tab = Tab::Content;
                    this.open_editor(Focus::Video(id.clone()), w, cx);
                })))
        });

        let upcoming: Vec<_> = h.upcoming().into_iter().take(5).cloned().collect();
        let schedule_card = card()
            .child(card_title(ph::CALENDAR_DOTS, "Up next"))
            .when(upcoming.is_empty(), |d| d.child(hint("Nothing scheduled. Queue finished recaps in Schedule and they go out in chapter order.")))
            .children(upcoming.into_iter().map(|i| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(icon_ph(if i.state == UploadState::Scheduled { ph::CALENDAR_CHECK } else { ph::HOURGLASS }, Tokens::text_3(), icon_px::SM))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).child(i.title.clone()))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(i.publish_at.map(fmt_when).unwrap_or_default()))
            }))
            .child(btn("dash-schedule", "Open schedule", ButtonKind::Ghost).small().on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Schedule, cx))));

        let tips: Vec<Suggestion> = h.suggestions().into_iter().take(3).collect();
        let tips_card = card()
            .child(card_title(ph::LIGHTBULB, "What to do next"))
            .when(tips.is_empty(), |d| d.child(hint("No alerts. Your releases are on track.")))
            .children(tips.into_iter().enumerate().map(|(i, s)| self.suggestion_row(("dash-tip", i), s, cx)));

        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(tiles)
            .child(div().flex().flex_wrap().gap(px(space::LG)).child(div().flex_1().min_w(px(360.)).flex().flex_col().gap(px(space::LG)).child(views_chart).children(latest_card)).child(div().w(px(340.)).flex().flex_col().gap(px(space::LG)).child(tips_card).child(schedule_card)))
            .into_any_element()
    }

    // ------------------------------------------------------------ analytics

    pub(crate) fn analytics(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let days = self.range;
        let rows: Vec<_> = h.channel_daily.iter().rev().take(days).rev().cloned().collect();
        let labels: Vec<String> = rows.iter().map(|d| d.date.clone()).collect();
        let series = |f: fn(&mv_youtube::DayStat) -> f64| rows.iter().map(f).collect::<Vec<f64>>();
        let all_views: Vec<f64> = h.channel_daily.iter().map(|d| d.views as f64).collect();
        let total = |v: &[f64]| v.iter().sum::<f64>();
        let views = series(|d| d.views as f64);
        let hours = series(|d| d.watch_minutes / 60.);
        let subs = series(|d| d.subs_gained as f64);
        let avg = series(|d| d.avg_view_secs);
        let avg_view = if avg.is_empty() { 0. } else { avg.iter().sum::<f64>() / avg.iter().filter(|v| **v > 0.).count().max(1) as f64 };
        let picker = segmented("yt-range", ["7 days", "28 days", "90 days"], match days {
            7 => 0,
            28 => 1,
            _ => 2,
        })
        .small()
        .on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.range = [7, 28, 90][*ix];
            cx.notify();
        }));

        // Videos ranked by views, with how each is trending.
        let mut ranked: Vec<_> = h.videos.iter().filter(|v| v.privacy != "private").cloned().collect();
        ranked.sort_by_key(|v| std::cmp::Reverse(v.views));
        let top = card().child(card_title(ph::TROPHY, "Top videos")).children(ranked.into_iter().take(8).enumerate().map(|(i, v)| {
            let trend = h.trend_of(&v.id);
            let id = v.id.clone();
            div()
                .id(("yt-top", i))
                .flex()
                .items_center()
                .gap(px(space::MD))
                .p(px(space::XS))
                .rounded(px(radius::SM))
                .cursor_pointer()
                .hover(|s| s.bg(Tokens::hover()))
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.tab = Tab::Content;
                    this.open_editor(Focus::Video(id.clone()), w, cx);
                }))
                .child(div().w(px(18.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("{}", i + 1)))
                .child(thumb(&v.thumbnail, 72.))
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).child(v.title.clone()))
                .child(trend_chip(trend))
                .child(div().w(px(64.)).text_right().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).child(fmt_num(v.views)))
        }));

        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(div().flex().items_center().child(div().flex_1().child(hint("Numbers come from YouTube Analytics and lag by about two days."))).child(picker))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(space::MD))
                    .child(kpi(ph::EYE, "Views", fmt_num(total(&views) as u64), chart::change(&all_views, days)))
                    .child(kpi(ph::CLOCK, "Watch time", format!("{:.1} h", total(&hours)), None))
                    .child(kpi(ph::TIMER, "Average view", format!("{}:{:02}", avg_view as u64 / 60, avg_view as u64 % 60), None))
                    .child(kpi(ph::USER_PLUS, "Subscribers", format!("{:+}", total(&subs) as i64), None)),
            )
            .child(card().child(card_title(ph::EYE, "Views")).child(bars("an-views", &views, &labels, "views", Tokens::accent(), 160.)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(space::LG))
                    .child(card().flex_1().min_w(px(300.)).child(card_title(ph::CLOCK, "Watch time (hours)")).child(bars("an-hours", &hours, &labels, "hours", Tokens::info(), 110.)))
                    .child(card().flex_1().min_w(px(300.)).child(card_title(ph::USER_PLUS, "Subscribers gained")).child(bars("an-subs", &subs, &labels, "subscribers", Tokens::success(), 110.))),
            )
            .child(top)
            .into_any_element()
    }

    // ------------------------------------------------------------ insights

    pub(crate) fn suggestion_row(&self, id: impl Into<ElementId>, s: Suggestion, cx: &mut Context<Self>) -> Stateful<Div> {
        let (icon, color, bg) = match s.severity {
            Severity::Urgent => (ph::TREND_DOWN, Tokens::danger(), Tokens::danger_soft()),
            Severity::Warning => (ph::WARNING, Tokens::warning(), Tokens::warning_soft()),
            Severity::Info => (ph::LIGHTBULB, Tokens::info(), Tokens::info_soft()),
        };
        let action = s.action.clone().map(|a| {
            let label = match &a {
                Action::PublishSooner { at, .. } => format!("Move to {}", fmt_when(*at)),
                Action::RefreshMetadata { .. } => "Improve with AI".to_string(),
                Action::QueueNext { .. } => "Queue it".to_string(),
            };
            btn(ElementId::Name(format!("sg-act-{}", s.title).into()), label, ButtonKind::Secondary).small().on_click(cx.listener(move |this, _, w, cx| this.run_action(a.clone(), w, cx)))
        });
        div()
            .id(id)
            .flex()
            .gap(px(space::MD))
            .p(px(space::MD))
            .rounded(px(radius::MD))
            .bg(bg)
            .child(div().pt(px(2.)).child(icon_ph(icon, color, icon_px::MD)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(space::XS))
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(s.title.clone()))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(s.body.clone()))
                    .children(action.map(|b| div().pt(px(space::XS)).child(b))),
            )
    }

    pub(crate) fn run_action(&mut self, a: Action, window: &mut Window, cx: &mut Context<Self>) {
        match a {
            Action::PublishSooner { item_id, at } => self.hub.update(cx, |h, cx| h.reschedule(&item_id, Some(at), cx)),
            Action::RefreshMetadata { video_id } => {
                self.tab = Tab::Content;
                self.open_editor(Focus::Video(video_id), window, cx);
                self.suggest(cx);
            }
            Action::QueueNext { chapter_id } => {
                let ep = self.hub.read(cx).episodes.iter().find(|e| e.chapter_id == chapter_id).cloned();
                if let Some(ep) = ep {
                    let id = self.hub.update(cx, |h, cx| h.queue_episode(&ep, cx));
                    if let Some(id) = id {
                        self.tab = Tab::Schedule;
                        self.open_editor(Focus::Item(id), window, cx);
                        self.suggest(cx);
                    }
                }
            }
        }
    }

    pub(crate) fn insights(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let sugg = h.suggestions();
        let moves = h.moves.clone();
        let auto = h.settings.auto_reschedule;
        let threshold = h.settings.dip_threshold;
        // Every recent video's first days, so the dips the automation reacts to are visible.
        let watched: Vec<_> = h.videos.iter().filter(|v| h.video_daily.contains_key(&v.id)).cloned().collect();
        let rows = watched.into_iter().map(|v| {
            let daily: Vec<u64> = h.video_daily.get(&v.id).map(|d| d.iter().map(|s| s.views).collect()).unwrap_or_default();
            let trend = h.trend_of(&v.id);
            let series = h.item_for_video(&v.id).map(|i| format!("{} · {}", i.manga_title.clone().unwrap_or_default(), i.episode_label()));
            let color = if matches!(trend, Trend::Dipping { .. }) { Tokens::danger() } else { Tokens::accent() };
            div()
                .flex()
                .items_center()
                .gap(px(space::MD))
                .py(px(space::XS))
                .child(thumb(&v.thumbnail, 64.))
                .child(div().flex().flex_col().flex_1().min_w_0().child(div().truncate().text_size(px(size::SM)).child(v.title.clone())).child(hint(series.unwrap_or_else(|| format!("Published {}", hub::fmt_ago(v.published_at))))))
                .child(chart::sparkline(&daily, color))
                .child(trend_chip(trend))
        }).collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(
                card()
                    .child(card_title(ph::ROBOT, "Smart upload"))
                    .child(hint(format!(
                        "Watches each new video for its first five days. When one loses {:.0}% of its best day's views, the next chapter of the same series is pulled forward to the next free slot, so viewers stay in the story. Chapters always go out in order.",
                        threshold * 100.
                    )))
                    .child(switch("yt-auto", auto).label("Move the next episode forward automatically").on_click(cx.listener(|this, on: &bool, _, cx| {
                        let mut s = this.hub.read(cx).settings.clone();
                        s.auto_reschedule = *on;
                        this.hub.update(cx, |h, cx| h.save_settings(s, cx));
                    }))),
            )
            .child(card().child(card_title(ph::LIGHTBULB, "Suggestions")).when(sugg.is_empty(), |d| d.child(hint("Nothing needs attention. New videos are checked every few hours."))).children(sugg.into_iter().enumerate().map(|(i, s)| self.suggestion_row(("ins", i), s, cx))))
            .child(card().child(card_title(ph::PULSE, "New videos, first days")).children(rows))
            .when(!moves.is_empty(), |d| d.child(card().child(card_title(ph::CLOCK_COUNTER_CLOCKWISE, "What the automation changed")).children(moves.into_iter().map(hint))))
            .into_any_element()
    }

    // ------------------------------------------------------------ settings

    pub(crate) fn settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let connected = h.account.connected();
        let connecting = h.connecting;
        let channel = h.channel.clone();
        let s = h.settings.clone();
        let creds_set = h.account.credentials.is_set();

        let connect = card()
            .child(card_title(ph::YOUTUBE_LOGO, if connected { "Connected channel" } else { "Connect your channel" }))
            .when(!connected, |d| {
                d.child(hint("YouTube needs an OAuth client of your own (free). Once:"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::XS))
                            .text_size(px(size::SM))
                            .text_color(Tokens::text_2())
                            .child("1. In Google Cloud Console create a project and enable “YouTube Data API v3” and “YouTube Analytics API”.")
                            .child("2. Set up the OAuth consent screen (External, add yourself as a test user).")
                            .child("3. Credentials → Create credentials → OAuth client ID → Desktop app. Paste its ID and secret below."),
                    )
                    .child(div().flex().gap(px(space::MD)).child(external_link("Open Google Cloud credentials", "https://console.cloud.google.com/apis/credentials")).child(external_link("Enable YouTube Data API", "https://console.cloud.google.com/apis/library/youtube.googleapis.com")))
                    .child(setting_row("Client ID", "Ends in .apps.googleusercontent.com", div().w(px(360.)).child(Input::new(&self.client_id))))
                    .child(setting_row("Client secret", "Stays on this PC, in your MangaVox data folder", div().w(px(360.)).child(Input::new(&self.client_secret))))
                    .child(
                        div().flex().gap(px(space::SM)).child(
                            btn("yt-connect", if connecting { "Waiting for the browser…" } else { "Connect YouTube" }, ButtonKind::Primary)
                                .icon(ph::SIGN_IN)
                                .loading(connecting)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let id = this.client_id.read(cx).value().to_string();
                                    let secret = this.client_secret.read(cx).value().to_string();
                                    if id.trim().is_empty() || secret.trim().is_empty() {
                                        YouTubePage::toast_err(cx, "Paste the client ID and secret first");
                                        return;
                                    }
                                    let yt = this.hub.read(cx).yt.clone();
                                    let hub = this.hub.clone();
                                    // Save the client first, then sign in with it.
                                    cx.spawn(async move |_, cx| {
                                        let _ = mv_ui::bg::run(async move { yt.set_credentials(mv_youtube::Credentials { client_id: id.trim().into(), client_secret: secret.trim().into() }).await }).await;
                                        hub.update(cx, |h, cx| {
                                            h.account = h.yt.store().account();
                                            h.connect(cx);
                                        });
                                    })
                                    .detach();
                                })),
                        ),
                    )
                    .when(creds_set && !connecting, |d| d.child(hint("A browser tab opens for Google sign-in. Pick the channel's account and allow access.")))
            })
            .when_some(channel.filter(|_| connected), |d, c| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::MD))
                        .child(crate::page::avatar(&c.thumbnail, 48.))
                        .child(div().flex().flex_col().flex_1().child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).child(c.title.clone())).child(hint(format!("{} · {} subscribers · {} videos", c.handle, fmt_num(c.subscribers), fmt_num(c.video_count)))))
                        .child(external_link("Open in YouTube Studio", format!("https://studio.youtube.com/channel/{}", c.id)))
                        .child(btn("yt-disconnect", "Disconnect", ButtonKind::Ghost).small().icon(ph::SIGN_OUT).on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.disconnect(cx))))),
                )
            });

        let hub = self.hub.clone();
        let save = |f: fn(&mut mv_youtube::Settings, usize), _: &mut Context<Self>| {
            let hub = hub.clone();
            move |ix: &usize, _: &mut Window, cx: &mut App| {
                let mut s = hub.read(cx).settings.clone();
                f(&mut s, *ix);
                hub.update(cx, |h, cx| h.save_settings(s, cx));
            }
        };
        const CADENCE: [u32; 4] = [12, 24, 48, 72];
        const HOURS: [u8; 6] = [9, 12, 15, 18, 20, 22];
        const DIPS: [f32; 3] = [0.25, 0.4, 0.6];
        let plan = card()
            .child(card_title(ph::CALENDAR_DOTS, "Release plan"))
            .child(setting_row(
                "How often",
                "Time between two releases of the queue",
                segmented("yt-cadence", ["Twice a day", "Daily", "Every 2 days", "Every 3 days"], CADENCE.iter().position(|c| *c == s.cadence_hours).unwrap_or(1)).small().on_change(save(|s, i| s.cadence_hours = CADENCE[i], cx)),
            ))
            .child(setting_row(
                "Release time",
                "Local time videos go public",
                segmented("yt-hour", ["9:00", "12:00", "15:00", "18:00", "20:00", "22:00"], HOURS.iter().position(|h| *h == s.publish_hour).unwrap_or(3)).small().on_change(save(|s, i| s.publish_hour = HOURS[i], cx)),
            ))
            .child(setting_row(
                "Dip sensitivity",
                "Drop from a video's best day that counts as a dip",
                segmented("yt-dip", ["25%", "40%", "60%"], DIPS.iter().position(|d| (*d - s.dip_threshold).abs() < 0.01).unwrap_or(1)).small().on_change(save(|s, i| s.dip_threshold = DIPS[i], cx)),
            ))
            .child(setting_row(
                "Visibility",
                "New uploads stay private until you pick Unlisted or Public",
                segmented("yt-privacy", ["Private", "Unlisted", "Public"], match s.privacy.as_str() { "unlisted" => 1, "public" => 2, _ => 0 }).small().on_change(save(|s, i| s.privacy = ["private", "unlisted", "public"][i.min(2)].into(), cx)),
            ))
            .child(setting_row(
                "Category",
                "YouTube category for new uploads",
                segmented("yt-cat", ["Film & Animation", "Entertainment"], if s.category_id == "24" { 1 } else { 0 }).small().on_change(save(|s, i| s.category_id = if i == 1 { "24".into() } else { "1".into() }, cx)),
            ))
            .child(setting_row(
                "Series playlists",
                "Add each episode to a “<manga> Recap” playlist so viewers watch in order",
                switch("yt-pl", s.series_playlists).on_click(cx.listener(|this, on: &bool, _, cx| {
                    let mut s = this.hub.read(cx).settings.clone();
                    s.series_playlists = *on;
                    this.hub.update(cx, |h, cx| h.save_settings(s, cx));
                })),
            ))
            .child(setting_row(
                "Made for kids",
                "Required by YouTube; recaps are usually not",
                switch("yt-kids", s.made_for_kids).on_click(cx.listener(|this, on: &bool, _, cx| {
                    let mut s = this.hub.read(cx).settings.clone();
                    s.made_for_kids = *on;
                    this.hub.update(cx, |h, cx| h.save_settings(s, cx));
                })),
            ));

        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(connect)
            .child(plan)
            .child(inline_alert(AlertKind::Info, "Upload limits", "YouTube's free API quota allows about six uploads a day. The queue stops when it runs out and keeps its order for the next day.").build())
            .into_any_element()
    }
}
