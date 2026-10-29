//! Content (the channel's videos), Schedule (Studio's finished recaps, the upload queue in release
//! order) and the details editor both open: title, description, tags, release time, the SEO
//! checklist and the AI's titles, description and hashtags.
use std::collections::BTreeMap;

use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, empty_view, external_link, icon_button, icon_ph, inline_alert, list_row, ph, progress_bar, size, spinner, AlertKind, ButtonKind, Tokens};
use mv_youtube::{planner, seo, LocalEpisode, MetaContext, UploadItem, UploadState, VideoEdit};

use crate::chart::{bars, trend_chip};
use crate::hub::{self, fmt_num, fmt_when};
use crate::page::{card, card_title, hint, thumb, AiState, Focus, YouTubePage};

fn state_chip(s: UploadState) -> Div {
    let (fg, bg, icon) = match s {
        UploadState::Planned => (Tokens::text_2(), Tokens::hover(), ph::HOURGLASS),
        UploadState::Uploading => (Tokens::accent(), Tokens::accent_soft(), ph::UPLOAD_SIMPLE),
        UploadState::Scheduled => (Tokens::info(), Tokens::info_soft(), ph::CALENDAR_CHECK),
        UploadState::Published => (Tokens::success(), Tokens::success_soft(), ph::CHECK_CIRCLE),
        UploadState::Failed => (Tokens::danger(), Tokens::danger_soft(), ph::WARNING_CIRCLE),
    };
    div().flex().flex_none().items_center().gap(px(4.)).h(px(20.)).px(px(6.)).rounded(px(radius::XS)).bg(bg).text_size(px(size::XS)).font_weight(FontWeight::MEDIUM).text_color(fg).child(icon_ph(icon, fg, icon_px::XS)).child(s.label())
}

fn chapter_key(c: Option<f64>) -> i64 {
    c.map(|c| (c * 1000.) as i64).unwrap_or(i64::MAX)
}

impl YouTubePage {
    // ------------------------------------------------------------ content

    pub(crate) fn content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        if h.videos.is_empty() {
            return empty_view(ph::FILM_STRIP, if h.loading { "Loading videos…" } else { "No videos yet" }, "Videos on your channel show up here with their numbers. Upload Studio recaps from Schedule.").build().into_any_element();
        }
        let rows: Vec<_> = h.videos.iter().map(|v| (v.clone(), h.trend_of(&v.id), h.video_daily.get(&v.id).map(|d| d.iter().map(|s| s.views).collect::<Vec<u64>>()))).collect();
        let header = div()
            .flex()
            .items_center()
            .gap(px(space::MD))
            .px(px(space::SM))
            .pb(px(space::XS))
            .text_size(px(size::XS))
            .text_color(Tokens::text_3())
            .child(div().flex_1().child("Video"))
            .child(div().w(px(120.)).child("Visibility"))
            .child(div().w(px(90.)).child("Trend"))
            .child(div().w(px(64.)).text_right().child("Views"))
            .child(div().w(px(56.)).text_right().child("Likes"))
            .child(div().w(px(56.)).text_right().child("Comments"));
        card()
            .gap(px(2.))
            .child(header)
            .children(rows.into_iter().enumerate().map(|(i, (v, trend, daily))| {
                let id = v.id.clone();
                let vis = if v.is_scheduled() { format!("Scheduled · {}", v.publish_at.map(fmt_when).unwrap_or_default()) } else { capitalize(&v.privacy) };
                list_row(("yt-video", i), false)
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .p(px(space::SM))
                    .rounded(px(radius::SM))
                    .on_click(cx.listener(move |this, _, w, cx| this.open_editor(Focus::Video(id.clone()), w, cx)))
                    .child(thumb(&v.thumbnail, 96.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).line_clamp(2).child(v.title.clone()))
                            .child(hint(v.description.lines().next().unwrap_or("Add a description").chars().take(90).collect::<String>()))
                            .child(hint(hub::fmt_ago(v.published_at))),
                    )
                    .child(div().w(px(120.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(vis))
                    .child(div().w(px(90.)).child(match daily {
                        Some(d) if d.len() > 1 => crate::chart::sparkline(&d, Tokens::accent()).into_any_element(),
                        _ => trend_chip(trend).into_any_element(),
                    }))
                    .child(div().w(px(64.)).text_right().text_size(px(size::SM)).child(fmt_num(v.views)))
                    .child(div().w(px(56.)).text_right().text_size(px(size::SM)).child(fmt_num(v.likes)))
                    .child(div().w(px(56.)).text_right().text_size(px(size::SM)).child(fmt_num(v.comments)))
            }))
            .into_any_element()
    }

    // ------------------------------------------------------------ schedule

    pub(crate) fn schedule(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let episodes = h.episodes.clone();
        let queue = h.queue.clone();
        let next: Vec<String> = planner::next_up(&episodes, &queue).into_iter().map(|e| e.chapter_id.clone()).collect();

        // Ready to upload: finished recaps by manga, chapters in order, the next one marked.
        let mut by_manga: BTreeMap<String, Vec<LocalEpisode>> = BTreeMap::new();
        for e in episodes.iter() {
            by_manga.entry(e.manga_title.clone()).or_default().push(e.clone());
        }
        let ready = card()
            .child(card_title(ph::FILM_SLATE, "Ready to upload"))
            .child(hint("Finished Studio recaps. Upload the series in order: the next chapter for each manga is marked."))
            .when(by_manga.is_empty(), |d| d.child(hint("No finished recaps yet. Make one in Studio, then come back.")))
            .children(by_manga.into_iter().enumerate().map(|(gi, (title, mut eps))| {
                planner::series_order(&mut eps);
                let pending: Vec<LocalEpisode> = eps.iter().filter(|e| !queue.iter().any(|q| q.chapter_id.as_deref() == Some(e.chapter_id.as_str()) && q.state != UploadState::Failed)).cloned().collect();
                let all = pending.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::SM))
                    .pt(px(space::SM))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(div().flex_1().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).child(title))
                            .when(all.len() > 1, |d| {
                                d.child(btn(("yt-queue-all", gi), format!("Queue all {} in order", all.len()), ButtonKind::Secondary).small().icon(ph::QUEUE).on_click(cx.listener(move |this, _, _, cx| {
                                    let n = this.hub.update(cx, |h, cx| all.iter().filter(|e| h.queue_episode(e, cx).is_some()).count());
                                    toast::show(cx, Toast::success(format!("Queued {n} episodes in chapter order")));
                                })))
                            }),
                    )
                    .child(div().flex().flex_wrap().gap(px(space::SM)).children(eps.into_iter().enumerate().map(|(i, e)| {
                        let queued = !pending.iter().any(|p| p.chapter_id == e.chapter_id);
                        let is_next = next.contains(&e.chapter_id);
                        let missing = e.chapter.map(|c| planner::missing_before(&e.manga_id, c, &episodes, &queue)).unwrap_or_default();
                        let ep = e.clone();
                        div()
                            .id(("yt-ep", gi * 1000 + i))
                            .flex()
                            .items_center()
                            .gap(px(space::XS))
                            .h(px(30.))
                            .px(px(space::SM))
                            .rounded(px(radius::MD))
                            .border_1()
                            .text_size(px(size::SM))
                            .map(|d| {
                                if queued {
                                    d.border_color(Tokens::border()).text_color(Tokens::text_3()).child(icon_ph(ph::CHECK, Tokens::success(), icon_px::XS))
                                } else if is_next {
                                    d.border_color(Tokens::accent_border()).bg(Tokens::accent_soft()).text_color(Tokens::text_1()).child(icon_ph(ph::ARROW_RIGHT, Tokens::accent(), icon_px::XS))
                                } else {
                                    d.border_color(Tokens::border()).text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()))
                                }
                            })
                            .child(e.label.clone())
                            .when(!queued, |d| {
                                d.cursor_pointer().tooltip(mv_ui::tip(if missing.is_empty() { "Add to the queue".to_string() } else { format!("Chapters {} come first; they will be queued ahead of it", missing.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")) })).on_click(cx.listener(move |this, _, w, cx| this.queue_with_gaps(&ep, w, cx)))
                            })
                    })))
            }))
            .child(div().pt(px(space::SM)).child(btn("yt-pick", "Upload another video file…", ButtonKind::Ghost).small().icon(ph::FILE_VIDEO).on_click(cx.listener(|this, _, w, cx| this.pick_file(w, cx)))));

        // The queue in release order, grouped by day like a calendar.
        let mut items = queue.clone();
        items.retain(|i| i.state != UploadState::Published || i.publish_at.is_some_and(|t| t > hub::now() - 3 * 86_400));
        items.sort_by_key(|i| (i.publish_at.unwrap_or(0), chapter_key(i.chapter)));
        let mut days: BTreeMap<String, Vec<UploadItem>> = BTreeMap::new();
        for i in items {
            let key = i.publish_at.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d · %A").to_string()).unwrap_or_else(|| "0 · As soon as it uploads".into());
            days.entry(key).or_default().push(i);
        }
        let uploading = h.uploading;
        let queue_card = card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().flex_1().child(card_title(ph::CALENDAR_DOTS, "Release calendar")))
                    .child(btn("yt-autoplan", "Plan in chapter order", ButtonKind::Secondary).small().icon(ph::SPARKLE).tooltip("Gives every queued episode a release slot so each series goes out in order").on_click(cx.listener(|this, _, _, cx| {
                        let n = this.hub.update(cx, |h, cx| h.auto_plan(cx));
                        toast::show(cx, Toast::success(if n == 0 { "The plan is already in chapter order".to_string() } else { format!("Re-planned {n} releases") }));
                    }))),
            )
            .child(hint("Uploaded videos stay private until their time, then YouTube publishes them, even when MangaVox is closed."))
            .when(days.is_empty(), |d| d.child(hint("The queue is empty. Pick the next chapter above.")))
            .children(days.into_iter().map(|(day, items)| {
                let label = day.split_once(" · ").map(|(d, w)| if d == "0" { w.to_string() } else { format!("{w}, {d}") }).unwrap_or(day.clone());
                div().flex().flex_col().gap(px(space::XS)).pt(px(space::SM)).child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(label.to_uppercase())).children(items.into_iter().map(|i| self.queue_row(i, uploading, cx)))
            }));

        div().flex().flex_col().gap(px(space::LG)).child(queue_card).child(ready).into_any_element()
    }

    fn queue_row(&self, i: UploadItem, uploading: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let (id, id2, id3) = (i.id.clone(), i.id.clone(), i.id.clone());
        let editable = matches!(i.state, UploadState::Planned | UploadState::Failed | UploadState::Scheduled);
        div()
            .id(SharedString::from(format!("q-{}", i.id)))
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .p(px(space::SM))
            .rounded(px(radius::MD))
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().w(px(48.)).text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(i.publish_at.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_else(|| "Now".into())))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).child(i.title.clone()))
                            .child(hint(format!("{}{}", i.manga_title.clone().map(|m| format!("{m} · ")).unwrap_or_default(), i.episode_label()))),
                    )
                    .when(i.rescheduled_from.is_some(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child("Moved up")))
                    .child(state_chip(i.state))
                    .when(editable, |d| d.child(icon_button(SharedString::from(format!("q-edit-{}", i.id)), ph::PENCIL_SIMPLE, "Edit details").small().on_click(cx.listener(move |this, _, w, cx| this.open_editor(Focus::Item(id.clone()), w, cx)))))
                    .when(i.state == UploadState::Failed, |d| {
                        d.child(icon_button(SharedString::from(format!("q-retry-{}", i.id)), ph::ARROW_CLOCKWISE, "Try again").small().on_click(cx.listener(move |this, _, _, cx| {
                            this.hub.update(cx, |h, cx| h.update_item(&id3, cx, |i| {
                                i.state = UploadState::Planned;
                                i.error = None;
                            }))
                        })))
                    })
                    .when(matches!(i.state, UploadState::Planned | UploadState::Failed), |d| d.child(icon_button(SharedString::from(format!("q-del-{}", i.id)), ph::TRASH, "Remove from queue").small().on_click(cx.listener(move |this, _, _, cx| this.hub.update(cx, |h, cx| h.remove_item(&id2, cx)))))),
            )
            .when(i.state == UploadState::Uploading, |d| d.child(progress_bar(SharedString::from(format!("q-p-{}", i.id)), i.progress, uploading)))
            .children(i.error.clone().map(|e| div().text_size(px(size::XS)).text_color(Tokens::danger()).child(e)))
    }

    /// Queues an episode; earlier chapters of the series that are finished but not queued go in
    /// ahead of it, so viewers never meet Chapter 5 before Chapter 2.
    fn queue_with_gaps(&mut self, ep: &LocalEpisode, window: &mut Window, cx: &mut Context<Self>) {
        let (episodes, queue) = {
            let h = self.hub.read(cx);
            (h.episodes.clone(), h.queue.clone())
        };
        let missing = ep.chapter.map(|c| planner::missing_before(&ep.manga_id, c, &episodes, &queue)).unwrap_or_default();
        let mut before: Vec<LocalEpisode> = episodes.iter().filter(|e| e.manga_id == ep.manga_id && e.chapter.is_some_and(|c| missing.iter().any(|m| (m - c).abs() < 1e-6))).cloned().collect();
        planner::series_order(&mut before);
        let id = self.hub.update(cx, |h, cx| {
            for e in &before {
                h.queue_episode(e, cx);
            }
            h.queue_episode(ep, cx)
        });
        if !before.is_empty() {
            toast::show(cx, Toast::info(format!("Also queued {} earlier chapter{} so the series stays in order", before.len(), if before.len() == 1 { "" } else { "s" })));
        }
        if let Some(id) = id {
            self.open_editor(Focus::Item(id), window, cx);
            self.suggest(cx);
        }
    }

    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Upload this video".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(file) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                let id = this.hub.update(cx, |h, cx| h.queue_file(file, cx));
                this.open_editor(Focus::Item(id), window, cx);
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ editor

    pub(crate) fn editor(&mut self, f: &Focus, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let title = self.title.read(cx).value().to_string();
        let desc = self.desc.read(cx).value().to_string();
        let tags = split_tags(&self.tags.read(cx).value());
        let (keyword, header, extra): (String, AnyElement, Option<AnyElement>) = match f {
            Focus::Video(id) => {
                let Some(v) = h.video(id).cloned() else { return hint("This video is no longer on the channel.").into_any_element() };
                let keyword = h.item_for_video(id).and_then(|i| i.manga_title.clone()).unwrap_or_default();
                let daily: Vec<u64> = h.video_daily.get(id).map(|d| d.iter().map(|s| s.views).collect()).unwrap_or_default();
                let trend = h.trend_of(id);
                let header = div()
                    .flex()
                    .gap(px(space::LG))
                    .child(thumb(&v.thumbnail, 240.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::SM))
                            .child(div().flex().items_center().gap(px(space::SM)).child(trend_chip(trend)).child(hint(format!("{} · published {}", capitalize(&v.privacy), hub::fmt_ago(v.published_at)))))
                            .child(div().flex().gap(px(space::LG)).child(stat(ph::EYE, fmt_num(v.views), "views")).child(stat(ph::THUMBS_UP, fmt_num(v.likes), "likes")).child(stat(ph::CHAT_CIRCLE, fmt_num(v.comments), "comments")))
                            .child(div().flex().gap(px(space::MD)).child(external_link("Watch", v.url())).child(external_link("Open in YouTube Studio", v.studio_url()))),
                    )
                    .into_any_element();
                let chart = (!daily.is_empty()).then(|| {
                    let labels: Vec<String> = (0..daily.len()).map(|i| format!("Day {}", i + 1)).collect();
                    card().child(card_title(ph::CHART_BAR, "Views per day since release")).child(bars("ed-daily", &daily.iter().map(|v| *v as f64).collect::<Vec<_>>(), &labels, "views", Tokens::accent(), 90.)).into_any_element()
                });
                (keyword, header, chart)
            }
            Focus::Item(id) => {
                let Some(i) = h.queue.iter().find(|i| &i.id == id).cloned() else { return hint("This upload is no longer in the queue.").into_any_element() };
                let missing = match (&i.manga_id, i.chapter) {
                    (Some(m), Some(c)) => planner::missing_before(m, c, &h.episodes, &h.queue),
                    _ => vec![],
                };
                let header = div()
                    .flex()
                    .flex_col()
                    .gap(px(space::SM))
                    .child(div().flex().items_center().gap(px(space::SM)).child(state_chip(i.state)).child(hint(format!("{} · {}", i.manga_title.clone().unwrap_or_else(|| "Video file".into()), i.episode_label()))))
                    .child(hint(i.file.display().to_string()))
                    .when(!missing.is_empty(), |d| {
                        d.child(inline_alert(AlertKind::Warning, "Out of order", format!("Chapters {} are not queued yet. Viewers would miss part of the story.", missing.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", "))).build())
                    })
                    .into_any_element();
                (i.manga_title.clone().unwrap_or_default(), header, None)
            }
        };
        let (score, checks) = seo::score(&title, &desc, &tags, &keyword);
        let score_color = if score >= 80 { Tokens::success() } else if score >= 50 { Tokens::warning() } else { Tokens::danger() };
        let is_video = matches!(f, Focus::Video(_));

        let fields = card()
            .child(card_title(ph::PENCIL_SIMPLE, "Details"))
            .child(field("Title", format!("{}/100", title.chars().count()), Input::new(&self.title).into_any_element()))
            .child(field("Description", format!("{} hashtags", seo::hashtags_in(&desc).len()), div().rounded(px(radius::MD)).border_1().border_color(Tokens::border()).bg(Tokens::surface()).p(px(space::XS)).child(Textarea::new(&self.desc).appearance(false).bordered(false)).into_any_element()))
            .child(field("Tags", format!("{} tags · comma separated", tags.len()), Input::new(&self.tags).into_any_element()))
            .child(field("Release", "Local time. Empty = publish when uploaded".into(), self.when_row(cx)))
            .child(
                div()
                    .flex()
                    .gap(px(space::SM))
                    .child(btn("ed-save", if is_video { "Save to YouTube" } else { "Save" }, ButtonKind::Primary).icon(ph::CHECK).on_click(cx.listener(|this, _, _, cx| this.save_editor(cx))))
                    .child(btn("ed-ai", "Suggest with AI", ButtonKind::Secondary).icon(ph::SPARKLE).loading(self.ai.busy).on_click(cx.listener(|this, _, _, cx| this.suggest(cx)))),
            );

        let seo_card = card()
            .child(div().flex().items_center().gap(px(space::SM)).child(card_title(ph::MAGNIFYING_GLASS, "SEO check")).child(div().flex_1()).child(div().text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(score_color).child(format!("{score}"))))
            .children(checks.into_iter().map(|c| {
                div()
                    .flex()
                    .gap(px(space::SM))
                    .child(icon_ph(if c.ok { ph::CHECK_CIRCLE } else { ph::WARNING_CIRCLE }, if c.ok { Tokens::success() } else { Tokens::warning() }, icon_px::SM))
                    .child(div().flex().flex_col().flex_1().child(div().text_size(px(size::SM)).child(c.label)).when(!c.ok, |d| d.child(hint(c.hint.clone()))))
            }));

        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(header)
            .children(extra)
            .child(div().flex().flex_wrap().gap(px(space::LG)).child(div().flex_1().min_w(px(380.)).flex().flex_col().gap(px(space::LG)).child(fields).child(self.ai_card(cx))).child(div().w(px(300.)).child(seo_card)))
            .into_any_element()
    }

    fn when_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        let taken: Vec<i64> = h.queue.iter().filter_map(|i| i.publish_at).collect();
        let slots = planner::next_slots(hub::now(), &taken, &h.settings, 3, hub::utc_offset());
        div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .child(Input::new(&self.when))
            .child(div().flex().flex_wrap().gap(px(space::XS)).children(slots.into_iter().enumerate().map(|(i, t)| {
                mv_ui::chip(("ed-slot", i), format!("Free slot · {}", fmt_when(t)), false).on_click(cx.listener(move |this, _, w, cx| this.when.update(cx, |s, cx| s.set_value(hub::to_local_input(t), w, cx))))
            })))
            .into_any_element()
    }

    fn ai_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let ai = &self.ai;
        let base = card()
            .child(card_title(ph::SPARKLE, "AI suggestions"))
            .child(div().flex().gap(px(space::SM)).child(div().flex_1().child(Input::new(&self.ask))).child(icon_button("ed-ai-go", ph::PAPER_PLANE_TILT, "Ask").on_click(cx.listener(|this, _, _, cx| this.suggest(cx)))));
        if ai.busy {
            return base.child(div().flex().items_center().gap(px(space::SM)).child(spinner("ai-spin", Tokens::accent(), icon_px::SM)).child(hint("Writing titles, a description and hashtags…"))).into_any_element();
        }
        if let Some(e) = &ai.error {
            return base.child(inline_alert(AlertKind::Error, "The AI could not answer", e.clone()).build()).into_any_element();
        }
        let Some(r) = ai.result.clone() else {
            return base.child(hint("Uses the model picked in the chat panel. It reads the recap's story and your best titles, then writes five catchy titles, an SEO description and hashtags.")).into_any_element();
        };
        let (desc, tags) = (r.description.clone(), r.tags.join(", "));
        base.child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child("TITLES · click one to use it"))
            .children(r.titles.iter().enumerate().map(|(i, t)| {
                let t2 = t.clone();
                div()
                    .id(("ai-title", i))
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .p(px(space::SM))
                    .rounded(px(radius::MD))
                    .border_1()
                    .border_color(Tokens::border())
                    .cursor_pointer()
                    .hover(|s| s.bg(Tokens::hover()).border_color(Tokens::accent_border()))
                    .on_click(cx.listener(move |this, _, w, cx| this.title.update(cx, |s, cx| s.set_value(t2.clone(), w, cx))))
                    .child(div().flex_1().text_size(px(size::MD)).child(t.clone()))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{}", t.chars().count())))
            }))
            .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child("DESCRIPTION"))
            .child(div().p(px(space::SM)).rounded(px(radius::MD)).bg(Tokens::well()).text_size(px(size::SM)).text_color(Tokens::text_2()).whitespace_normal().child(r.description.clone()))
            .child(div().flex().flex_wrap().gap(px(space::XS)).children(r.hashtags.iter().map(|t| div().px(px(space::SM)).h(px(22.)).flex().items_center().rounded(px(radius::PILL)).bg(Tokens::info_soft()).text_size(px(size::XS)).text_color(Tokens::info()).child(t.clone()))))
            .when(!r.notes.is_empty(), |d| d.child(hint(r.notes.clone())))
            .child(
                div()
                    .flex()
                    .gap(px(space::SM))
                    .child(btn("ai-use-all", "Use all", ButtonKind::Primary).small().icon(ph::CHECKS).on_click(cx.listener({
                        let (desc, tags) = (desc.clone(), tags.clone());
                        let title = r.titles.first().cloned().unwrap_or_default();
                        move |this, _, w, cx| {
                            if !title.is_empty() {
                                this.title.update(cx, |s, cx| s.set_value(title.clone(), w, cx));
                            }
                            this.desc.update(cx, |s, cx| s.set_value(desc.clone(), w, cx));
                            this.tags.update(cx, |s, cx| s.set_value(tags.clone(), w, cx));
                        }
                    })))
                    .child(btn("ai-use-desc", "Use description", ButtonKind::Ghost).small().on_click(cx.listener(move |this, _, w, cx| this.desc.update(cx, |s, cx| s.set_value(desc.clone(), w, cx)))))
                    .child(btn("ai-use-tags", "Use tags", ButtonKind::Ghost).small().on_click(cx.listener(move |this, _, w, cx| this.tags.update(cx, |s, cx| s.set_value(tags.clone(), w, cx))))),
            )
            .into_any_element()
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.focus.clone() else { return };
        let title = seo::clean_title(&self.title.read(cx).value());
        let description = seo::clean_description(&self.desc.read(cx).value());
        let tags = seo::clean_tags(&split_tags(&self.tags.read(cx).value()));
        let when_text = self.when.read(cx).value().to_string();
        let at = if when_text.trim().is_empty() {
            None
        } else {
            match hub::parse_local(&when_text) {
                Some(t) if t > hub::now() + 15 * 60 => Some(t),
                Some(_) => return Self::toast_err(cx, "Pick a release time at least 15 minutes from now"),
                None => return Self::toast_err(cx, "Write the release time as YYYY-MM-DD HH:MM"),
            }
        };
        if title.is_empty() {
            return Self::toast_err(cx, "Give the video a title");
        }
        match f {
            Focus::Video(id) => {
                let v = self.hub.read(cx).video(&id).cloned();
                let Some(v) = v else { return };
                // Only a private video can get a release time; public ones keep theirs.
                let publish_at = (v.privacy == "private" && at != v.publish_at).then_some(at);
                self.hub.update(cx, |h, cx| h.edit_video(id, VideoEdit { title: Some(title), description: Some(description), tags: Some(tags), publish_at, privacy: None }, cx));
            }
            Focus::Item(id) => {
                let old = self.hub.read(cx).queue.iter().find(|i| i.id == id).map(|i| i.publish_at);
                self.hub.update(cx, |h, cx| {
                    h.update_item(&id, cx, |i| {
                        i.title = title;
                        i.description = description;
                        i.tags = tags;
                    });
                    if old != Some(at) {
                        h.reschedule(&id, at, cx);
                    }
                });
                toast::show(cx, Toast::success("Saved"));
            }
        }
    }

    /// Asks the assistant's model for titles, a description, hashtags and tags for the open video.
    pub(crate) fn suggest(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.focus.clone() else { return };
        if self.ai.busy {
            return;
        }
        let (provider, model) = ui_assistant::assistant_model(cx);
        let provider: Option<mv_ai::AiProviderId> = serde_json::from_value(serde_json::Value::String(provider)).ok();
        let Some(provider) = provider.filter(|_| !model.is_empty()) else {
            self.ai = AiState { error: Some("Pick a model in the chat panel first (the button under the message box).".into()), ..Default::default() };
            cx.notify();
            return;
        };
        let ctx = self.meta_context(&f, cx);
        let registry = ui_assistant::registry(cx);
        self.ai = AiState { busy: true, ..Default::default() };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move { mv_youtube::ai::suggest(registry, provider, model, &ctx).await }).await;
            let _ = this.update(cx, |this, cx| {
                this.ai = match r {
                    Ok(Ok(m)) => AiState { result: Some(m), ..Default::default() },
                    Ok(Err(e)) => AiState { error: Some(format!("{e:#}")), ..Default::default() },
                    Err(e) => AiState { error: Some(format!("{e:#}")), ..Default::default() },
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn meta_context(&self, f: &Focus, cx: &mut Context<Self>) -> MetaContext {
        let h = self.hub.read(cx);
        let mut top: Vec<_> = h.videos.iter().filter(|v| v.privacy == "public").collect();
        top.sort_by_key(|v| std::cmp::Reverse(v.views));
        let mut ctx = MetaContext {
            channel_title: h.channel.as_ref().map(|c| c.title.clone()).unwrap_or_default(),
            top_titles: top.iter().take(8).map(|v| v.title.clone()).collect(),
            instruction: self.ask.read(cx).value().to_string(),
            language: h.settings.language.clone(),
            ..Default::default()
        };
        let item = match f {
            Focus::Video(id) => {
                ctx.current = h.video(id).cloned();
                h.item_for_video(id).cloned()
            }
            Focus::Item(id) => h.queue.iter().find(|i| &i.id == id).cloned(),
        };
        if let Some(i) = item {
            ctx.manga_title = i.manga_title.clone().unwrap_or_default();
            ctx.episode_label = h.episodes.iter().find(|e| Some(&e.chapter_id) == i.chapter_id.as_ref()).map(|e| e.label.clone()).unwrap_or_else(|| i.episode_label());
            ctx.summary = story_summary(&i.file);
            // The chapter before it, when it is on YouTube: "Previous episode: <link>".
            if let (Some(m), Some(c)) = (&i.manga_id, i.chapter) {
                ctx.previous = h
                    .queue
                    .iter()
                    .filter(|q| q.manga_id.as_ref() == Some(m) && q.chapter.is_some_and(|qc| qc < c) && q.video_id.is_some())
                    .max_by_key(|q| chapter_key(q.chapter))
                    .map(|q| (q.title.clone(), format!("https://youtu.be/{}", q.video_id.clone().unwrap_or_default())));
            }
        } else if let Some(v) = &ctx.current {
            ctx.summary = v.description.clone();
        }
        ctx
    }
}

/// The recap's story text (`<chapter output>/studio/story.json`), trimmed for the prompt.
fn story_summary(video: &std::path::Path) -> String {
    let Some(dir) = video.parent() else { return String::new() };
    let Ok(raw) = std::fs::read(dir.join("studio").join("story.json")) else { return String::new() };
    let Ok(story) = serde_json::from_slice::<mv_recap::Story>(&raw) else { return String::new() };
    let mut s = story.title.clone();
    for seg in story.segments {
        if s.len() > 3500 {
            break;
        }
        s.push('\n');
        s.push_str(seg.text.trim());
    }
    s
}

fn split_tags(s: &str) -> Vec<String> {
    s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn stat(icon: mv_ui::Ph, value: String, label: &str) -> Div {
    div().flex().items_center().gap(px(space::XS)).child(icon_ph(icon, Tokens::text_3(), icon_px::SM)).child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).child(value)).child(hint(label.to_string()))
}

fn field(label: &str, aside: String, control: AnyElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(space::XS))
        .child(div().flex().items_center().child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_2()).child(label.to_string())).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(aside)))
        .child(control)
}
