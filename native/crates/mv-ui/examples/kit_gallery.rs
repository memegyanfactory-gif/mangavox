//! Visual check for the component kit: every control in its states, live.
//! `cargo run -p mv-ui --example kit_gallery`
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use mv_ui::toast::{self, Toast};
use mv_ui::{
    badge, banner, btn, checkbox, confirm_dialog, confirm_inline, count_badge, cover_tile, disclosure, empty_view, error_view, external_link, favorite_star, filter_chip, icon_button,
    icon_tag, icon_toggle, inline_alert, interactive, kbd, menu_button, meter, nav_item, overflow_menu, page_title, path_chip, ph, progress_ring, section_header, section_label, segmented,
    selected_badge, size, skeleton_card, skeleton_row, skeleton_text, status_tag, stepper, switch, tag, tip, AlertKind, ButtonKind, CheckState, CoverSource, MenuItem,
    StatusKind, Tokens, Tone,
};

struct Kit {
    fav: bool,
    pin: bool,
    seg: usize,
    view: usize,
    chips: [bool; 3],
    check: [bool; 3],
    on: bool,
    nav: usize,
    confirming: bool,
    step: usize,
}

fn section(title: &'static str) -> Div {
    div().mt(px(8.)).text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title)
}

fn row() -> Div {
    div().flex().flex_wrap().items_center().gap(px(12.))
}

impl Render for Kit {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mixed = match self.check.iter().filter(|c| **c).count() {
            0 => CheckState::Unchecked,
            3 => CheckState::Checked,
            _ => CheckState::Mixed,
        };
        let page = div()
            .id("kit")
            .size_full()
            .bg(Tokens::base())
            .overflow_y_scroll()
            .p(px(28.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                page_title("Component kit")
                    .icon(ph::SQUARES_FOUR)
                    .subtitle("Every mv-ui control in its states")
                    .action(btn("pt-new", "New", ButtonKind::Primary).icon(ph::PLUS))
                    .action(overflow_menu(
                        "pt-more",
                        vec![
                            MenuItem::heading("Sort by"),
                            MenuItem::new("Title", |_, _| {}).checked(true).icon(ph::TEXT_AA),
                            MenuItem::new("Last read", |_, _| {}).icon(ph::CLOCK),
                            MenuItem::separator(),
                            MenuItem::new("Refresh", |_, cx| toast::show(cx, Toast::info("Refreshing library"))).icon(ph::ARROWS_CLOCKWISE).kbd("ctrl-r"),
                            MenuItem::new("Delete all", |_, cx| toast::show(cx, Toast::error("Could not delete: files are in use"))).icon(ph::TRASH).danger(),
                        ],
                    )),
            )
            .child(section("Buttons"))
            .child(
                row()
                    .child(btn("b1", "Primary", ButtonKind::Primary).icon(ph::PLAY).on_click(|_, _, cx| toast::show(cx, Toast::success("Playback started"))))
                    .child(btn("b2", "Secondary", ButtonKind::Secondary).icon(ph::DOWNLOAD_SIMPLE))
                    .child(btn("b3", "Ghost", ButtonKind::Ghost))
                    .child(btn("b4", "Delete", ButtonKind::Danger).icon(ph::TRASH))
                    .child(btn("b5", "Remove", ButtonKind::DangerGhost))
                    .child(btn("b6", "Saving", ButtonKind::Primary).loading(true))
                    .child(btn("b7", "Export", ButtonKind::Secondary).disabled("Render the video first"))
                    .child(btn("b8", "Docs", ButtonKind::Ghost).trailing_icon(ph::ARROW_UP_RIGHT))
                    .child(btn("b9", "Small", ButtonKind::Secondary).small())
                    .child(btn("b10", "Large", ButtonKind::Primary).large()),
            )
            .child(
                row()
                    .child(icon_button("i1", ph::SIDEBAR_SIMPLE, "Toggle sidebar").kbd("ctrl-b"))
                    .child(icon_button("i2", ph::BELL, "Notifications").selected(self.pin).on_click(cx.listener(|this, _, _, cx| {
                        this.pin = !this.pin;
                        cx.notify();
                    })))
                    .child(icon_button("i3", ph::TRASH, "Delete").danger())
                    .child(icon_button("i4", ph::GEAR_SIX, "Settings").disabled("Not available offline"))
                    .child(icon_button("i5", "icons/close.svg", true))
                    .child(icon_button("i6", ph::MAGNIFYING_GLASS, "Search").small())
                    .child(icon_button("i7", ph::PLAY, "Play").large())
                    .child(
                        favorite_star("fav", self.fav).on_click(cx.listener(|this, _, _, cx| {
                            this.fav = !this.fav;
                            cx.notify();
                        })),
                    )
                    .child(icon_toggle("bm", ph::BOOKMARK_SIMPLE, self.pin, Tokens::accent(), "Remove bookmark", "Bookmark"))
                    .child(selected_badge())
                    .child(menu_button("sort", ph::SORT_ASCENDING, "Sort", vec![MenuItem::new("Ascending", |_, _| {}).checked(true), MenuItem::new("Descending", |_, _| {})]))
                    .child(menu_button("lang", ph::TRANSLATE, "Language", vec![MenuItem::new("English", |_, _| {}).checked(true), MenuItem::new("Japanese", |_, _| {})]).label("English")),
            )
            .child(section("Chips, segmented, checkbox, switch"))
            .child(
                row()
                    .children(["Ongoing", "Completed", "Hiatus"].into_iter().enumerate().map(|(i, l)| {
                        filter_chip(("chip", i), l, self.chips[i]).count([128, 54, 7][i]).on_click(cx.listener(move |this, _, _, cx| {
                            this.chips[i] = !this.chips[i];
                            cx.notify();
                        }))
                    }))
                    .child(filter_chip("chip-icon", "Downloaded", false).icon(ph::DOWNLOAD_SIMPLE))
                    .child(segmented("seg", [(None, "Paged"), (None, "Webtoon"), (None, "Double")], self.seg).on_change(cx.listener(|this, ix: &usize, _, cx| {
                        this.seg = *ix;
                        cx.notify();
                    })))
                    .child(segmented("view", [(Some(ph::SQUARES_FOUR), "Grid"), (Some(ph::LIST), "List")], self.view).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                        this.view = *ix;
                        cx.notify();
                    }))),
            )
            .child(
                row()
                    .child(checkbox("all", mixed).child("All chapters").on_click(cx.listener(move |this, _, _, cx| {
                        let to = mixed != CheckState::Checked;
                        this.check = [to; 3];
                        cx.notify();
                    })))
                    .children((0..3).map(|i| {
                        checkbox(("ch", i), self.check[i]).child(format!("Chapter {}", i + 1)).on_click(cx.listener(move |this, _, _, cx| {
                            this.check[i] = !this.check[i];
                            cx.notify();
                        }))
                    }))
                    .child(switch("sw", self.on).label("Autoplay").on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.on = *on;
                        cx.notify();
                    }))),
            )
            .child(section("Navigation"))
            .child(
                div()
                    .flex()
                    .gap(px(24.))
                    .child(div().w(px(220.)).p(px(10.)).rounded(px(10.)).bg(Tokens::sidebar()).flex().flex_col().gap(px(2.)).children(
                        [(ph::COMPASS, "Browse"), (ph::BOOKS, "Library"), (ph::CLOCK_COUNTER_CLOCKWISE, "History"), (ph::DOWNLOAD_SIMPLE, "Downloads")].into_iter().enumerate().map(|(i, (ic, l))| {
                            nav_item(("nav", i), ic, l, self.nav == i, false).badge(if i == 3 { 2 } else { 0 }).on_click(cx.listener(move |this, _, _, cx| {
                                this.nav = i;
                                cx.notify();
                            }))
                        }),
                    ))
                    .child(div().w(px(60.)).p(px(10.)).rounded(px(10.)).bg(Tokens::sidebar()).flex().flex_col().items_center().gap(px(2.)).children(
                        [(ph::COMPASS, "Browse"), (ph::BOOKS, "Library"), (ph::CLOCK_COUNTER_CLOCKWISE, "History")].into_iter().enumerate().map(|(i, (ic, l))| nav_item(("navc", i), ic, l, self.nav == i, true).kbd(format!("ctrl-{}", i + 1))),
                    )),
            )
            .child(section_header("Continue reading", Some(ph::BOOK_OPEN)).count(12).see_all(|_, _, cx| toast::show(cx, Toast::info("See all"))))
            .child(
                row()
                    .items_start()
                    .child(cover_tile("c1", "Frieren: Beyond Journey's End").sub("Chapter 128").progress(0.62).favorite(true).badge("2 new"))
                    .child(cover_tile("c2", "A very long manga title that goes on and keeps going past two lines").sub("Chapter 3").cover(CoverSource::Url("https://invalid.example/cover.jpg".into())))
                    .child(skeleton_card()),
            )
            .child(section("Status"))
            .child(
                row()
                    .child(status_tag(StatusKind::Queued))
                    .child(status_tag(StatusKind::Running).label("Rendering 3/8"))
                    .child(status_tag(StatusKind::Done))
                    .child(status_tag(StatusKind::Failed))
                    .child(status_tag(StatusKind::Cancelled))
                    .child(status_tag(StatusKind::Info).label("Beta"))
                    .child(tag("1080p", Tone::Neutral))
                    .child(icon_tag(ph::HARD_DRIVE, "Local", Tone::Accent))
                    .child(tag("Warning", Tone::Warning))
                    .child(badge("NEW", Tone::Accent))
                    .child(badge("Default", Tone::Neutral))
                    .children(count_badge(3))
                    .children(count_badge(240))
                    .child(kbd("ctrl-k"))
                    .child(kbd("ctrl-shift-p"))
                    .child(meter(3., 5))
                    .child(progress_ring(0.0))
                    .child(progress_ring(0.35))
                    .child(progress_ring(1.0)),
            )
            .child(
                stepper(
                    "steps",
                    [(ph::SCAN, "Read"), (ph::NOTE_PENCIL, "Story"), (ph::MICROPHONE, "Voice"), (ph::MUSIC_NOTES, "Music"), (ph::FILM_STRIP, "Video")],
                    self.step,
                    &[true, true, false, false, false],
                    Some(2),
                    None,
                )
                .on_select(cx.listener(|this, ix: &usize, _, cx| {
                    this.step = *ix;
                    cx.notify();
                })),
            )
            .child(stepper("steps-fail", [(ph::SCAN, "Read"), (ph::NOTE_PENCIL, "Story"), (ph::MICROPHONE, "Voice")], 2, &[true, true, false], None, Some(2)).compact().w(px(260.)))
            .child(section("Alerts and confirmations"))
            .child(inline_alert(AlertKind::Warning, "Voice engine not installed", "Studio needs Kokoro to narrate. It is a 320 MB download.").action("Install", |_, _, _| {}).dismissible(|_, _, _| {}))
            .child(inline_alert(AlertKind::Info, "Library updates run every 6 hours", ""))
            .child(banner(AlertKind::Error, "Could not reach MangaDex. Check your connection.").action("Retry", |_, _, _| {}))
            .child(
                row()
                    .child(if self.confirming {
                        confirm_inline("Delete this chat?", "Delete chat", true)
                            .on_cancel(cx.listener(|this, _, _, cx| {
                                this.confirming = false;
                                cx.notify();
                            }))
                            .on_confirm(cx.listener(|this, _, _, cx| {
                                this.confirming = false;
                                toast::show(cx, Toast::success("Chat deleted").action("Undo", |_, cx| toast::show(cx, Toast::info("Chat restored"))));
                                cx.notify();
                            }))
                            .into_any_element()
                    } else {
                        btn("del", "Delete chat", ButtonKind::DangerGhost)
                            .icon(ph::TRASH)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirming = true;
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .child(btn("dlg", "Reset MangaVox…", ButtonKind::Secondary).on_click(|_, window, cx| {
                        confirm_dialog("Erase all MangaVox data?", "Your library, history, downloads and voices are deleted. This cannot be undone.", "Erase everything", true)
                            .cancel_label("Keep my data")
                            .on_confirm(|_, cx| toast::show(cx, Toast::warning("Everything was erased")))
                            .open(window, cx)
                    }))
                    .child(btn("t-err", "Error toast", ButtonKind::Ghost).on_click(|_, _, cx| toast::show(cx, Toast::error("Download failed: disk full").action("Retry", |_, _| {}))))
                    .child(path_chip("C:\\Users\\aayus\\.mangavox\\library\\one-piece\\chapter-1100\\page-001.webp").w(px(320.)))
                    .child(external_link("Kokoro on Hugging Face", "https://huggingface.co/hexgrad/Kokoro-82M")),
            )
            .child(disclosure("logs", "Show log").content(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child("[12:01] Started\n[12:02] Read 24 pages")).trailing(section_label("24 lines")))
            .child(section("Empty, error and loading"))
            .child(
                div()
                    .flex()
                    .gap(px(16.))
                    .child(div().flex_1().rounded(px(12.)).border_1().border_color(Tokens::border()).child(empty_view(ph::BOOKS, "Your library is empty", "Add manga from Browse and they show up here.").action(btn("go", "Browse sources", ButtonKind::Primary).icon(ph::COMPASS)).secondary(btn("imp", "Import", ButtonKind::Ghost))))
                    .child(div().flex_1().rounded(px(12.)).border_1().border_color(Tokens::border()).child(error_view("MangaDex did not answer in time.").retry(|_, _, cx| toast::show(cx, Toast::info("Retrying")))))
                    .child(div().flex_1().flex().flex_col().gap(px(10.)).p(px(12.)).rounded(px(12.)).border_1().border_color(Tokens::border()).child(skeleton_row()).child(skeleton_row()).child(skeleton_text(3))),
            )
            .child(interactive("custom").p(px(12.)).rounded(px(8.)).text_color(Tokens::text_2()).tooltip(tip("A custom div with interactive()")).child("interactive(): hover, pressed, Tab focus ring"))
            .child(div().h(px(80.)));
        // The page scrolls; the toast layer sits over it, as in the app shell.
        div().relative().size_full().child(page).child(mv_ui::toast::host(cx))
    }
}

fn main() {
    gpui_kit::application().with_assets(mv_ui::Assets).run(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        mv_ui::theme::apply_component_theme(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(900.)), cx);
        gpui_kit::open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), titlebar: Some(TitlebarOptions { title: Some("MangaVox kit gallery".into()), ..Default::default() }), ..Default::default() },
            cx,
            |_, cx| cx.new(|_| Kit { fav: true, pin: false, seg: 0, view: 0, chips: [true, false, false], check: [true, false, false], on: true, nav: 1, confirming: false, step: 2 }),
        )
        .expect("open window");
    });
}
