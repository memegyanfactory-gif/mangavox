//! The YouTube page, laid out like YouTube Studio: the channel and its sections on the left
//! (Dashboard, Content, Schedule, Analytics, Insights, Settings), the section in the middle and
//! the assistant chat on the right, told about the channel and the video being edited so it can
//! fix titles, SEO and timing with you.
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, empty_view, glyphs, icon_button, icon_ph, inline_alert, nav_item, ph, size, spinner, AlertKind, ButtonKind, Ph, Tokens};
use mv_youtube::MetaSuggestion;

use crate::hub::{self, Hub};

pub(crate) const NAV_W: f32 = 220.;
pub(crate) const SIDE_W: f32 = 384.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Dashboard,
    Content,
    Schedule,
    Analytics,
    Insights,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 6] = [Tab::Dashboard, Tab::Content, Tab::Schedule, Tab::Analytics, Tab::Insights, Tab::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Content => "Content",
            Tab::Schedule => "Schedule",
            Tab::Analytics => "Analytics",
            Tab::Insights => "Insights",
            Tab::Settings => "Settings",
        }
    }

    pub fn icon(self) -> Ph {
        match self {
            Tab::Dashboard => ph::SQUARES_FOUR,
            Tab::Content => ph::FILM_STRIP,
            Tab::Schedule => ph::CALENDAR_DOTS,
            Tab::Analytics => ph::CHART_LINE_UP,
            Tab::Insights => ph::LIGHTBULB,
            Tab::Settings => glyphs::SETTINGS,
        }
    }
}

/// What the editor is open on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Focus {
    /// A video on the channel.
    Video(String),
    /// An item in the upload queue.
    Item(String),
}

/// The AI's suggestion for the open editor.
#[derive(Default)]
pub(crate) struct AiState {
    pub busy: bool,
    pub result: Option<MetaSuggestion>,
    pub error: Option<String>,
}

pub struct YouTubePage {
    pub(crate) hub: Entity<Hub>,
    pub(crate) tab: Tab,
    pub(crate) focus: Option<Focus>,
    pub(crate) title: Entity<InputState>,
    pub(crate) desc: Entity<TextareaState>,
    pub(crate) tags: Entity<InputState>,
    pub(crate) when: Entity<InputState>,
    pub(crate) ask: Entity<InputState>,
    pub(crate) ai: AiState,
    pub(crate) client_id: Entity<InputState>,
    pub(crate) client_secret: Entity<InputState>,
    /// Analytics period in days.
    pub(crate) range: usize,
    pub(crate) dock: Option<AnyView>,
    pub(crate) side_open: bool,
    pub(crate) scroll: ScrollHandle,
    pub(crate) last_ctx: String,
    _subs: Vec<Subscription>,
}

impl YouTubePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hub = hub::hub(cx);
        hub.update(cx, |h, cx| {
            h.reload_local(cx);
            if !h.loaded_once {
                h.refresh(cx);
            }
        });
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title viewers see in search"));
        let desc = cx.new(|cx| TextareaState::new(window, cx).auto_grow(6, 18));
        let tags = cx.new(|cx| InputState::new(window, cx).placeholder("manga recap, solo leveling, sung jinwoo"));
        let when = cx.new(|cx| InputState::new(window, cx).placeholder("YYYY-MM-DD HH:MM (empty = right away)"));
        let ask = cx.new(|cx| InputState::new(window, cx).placeholder("Optional: \"more dramatic\", \"mention the big fight\"…"));
        let account = hub.read(cx).account.credentials.clone();
        let client_id = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("1234567890-abc.apps.googleusercontent.com");
            s.set_value(account.client_id.clone(), window, cx);
            s
        });
        let client_secret = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("GOCSPX-…").masked(true);
            s.set_value(account.client_secret.clone(), window, cx);
            s
        });
        let mut subs = vec![cx.observe(&hub, |this, _, cx| {
            this.update_context(cx);
            cx.notify();
        })];
        for s in [&title, &tags] {
            subs.push(cx.subscribe_in(s, window, |_, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        subs.push(cx.subscribe_in(&desc, window, |_, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        }));
        subs.push(cx.subscribe_in(&ask, window, |this: &mut Self, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.suggest(cx);
            }
        }));
        let start = if hub.read(cx).account.connected() { Tab::Dashboard } else { Tab::Settings };
        Self {
            hub,
            tab: start,
            focus: None,
            title,
            desc,
            tags,
            when,
            ask,
            ai: AiState::default(),
            client_id,
            client_secret,
            range: 28,
            dock: None,
            side_open: true,
            scroll: ScrollHandle::new(),
            last_ctx: String::new(),
            _subs: subs,
        }
    }

    pub(crate) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.focus = None;
        self.ai = AiState::default();
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.update_context(cx);
        cx.notify();
    }

    /// Tells the assistant chat what the user is looking at (channel numbers, the open video).
    pub(crate) fn update_context(&mut self, cx: &mut Context<Self>) {
        let h = self.hub.read(cx);
        let mut s = String::from("The user is on MangaVox's YouTube page (a YouTube Studio for their manga recap channel). You are their YouTube growth coach: give concrete titles, descriptions, hashtags, tags, thumbnails ideas and release timing. You can read and change the channel with the youtube_* tools.");
        if let Some(c) = &h.channel {
            s.push_str(&format!(" Channel: \"{}\" ({}), {} subscribers, {} views, {} videos.", c.title, c.handle, c.subscribers, c.views, c.video_count));
        } else if !h.account.connected() {
            s.push_str(" No channel is connected yet; they connect it in the Settings section of this page.");
        }
        let views28: u64 = h.channel_daily.iter().rev().take(28).map(|d| d.views).sum();
        if views28 > 0 {
            s.push_str(&format!(" Views in the last 28 days: {views28}."));
        }
        s.push_str(&format!(" Section open: {}.", self.tab.label()));
        match &self.focus {
            Some(Focus::Video(id)) => {
                if let Some(v) = h.video(id) {
                    s.push_str(&format!(" They are editing the video \"{}\" (id {}, {} views, {} likes, trend {:?}). Current description: {}", v.title, v.id, v.views, v.likes, h.trend_of(id), v.description.chars().take(600).collect::<String>()));
                }
            }
            Some(Focus::Item(id)) => {
                if let Some(i) = h.queue.iter().find(|i| &i.id == id) {
                    s.push_str(&format!(" They are preparing the upload \"{}\" ({} of {}).", i.title, i.episode_label(), i.manga_title.clone().unwrap_or_default()));
                }
            }
            None => {}
        }
        if s != self.last_ctx {
            self.last_ctx = s.clone();
            ui_assistant::set_studio_context(Some(s));
        }
    }

    // ------------------------------------------------------------ layout

    fn nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let h = self.hub.read(cx);
        let header = match &h.channel {
            Some(c) => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(space::SM))
                .py(px(space::LG))
                .child(avatar(&c.thumbnail, 72.))
                .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).text_center().child(c.title.clone()))
                .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(c.handle.clone())),
            None => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(space::SM))
                .py(px(space::LG))
                .child(div().size(px(72.)).rounded_full().bg(Tokens::well()).flex().items_center().justify_center().child(icon_ph(ph::YOUTUBE_LOGO, Tokens::danger(), icon_px::HERO)))
                .child(div().text_size(px(size::MD)).text_color(Tokens::text_2()).child(if h.account.connected() { "Loading channel…" } else { "No channel connected" })),
        };
        let insights = h.suggestions().iter().filter(|s| s.severity >= mv_youtube::Severity::Warning).count();
        let tab = self.tab;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(NAV_W))
            .h_full()
            .px(px(space::SM))
            .pb(px(space::SM))
            .map(column)
            .child(header)
            .child(div().flex().flex_col().gap(px(2.)).children(Tab::ALL.into_iter().map(|t| {
                nav_item(("yt-tab", t as usize), t.icon(), t.label(), t == tab, false)
                    .badge(if t == Tab::Insights { insights } else { 0 })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(t, cx)))
            })))
            .child(div().flex_1())
            .child(self.status_line(cx))
    }

    fn status_line(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let h = self.hub.read(cx);
        let text = if h.loading {
            "Refreshing…".to_string()
        } else if let Some(t) = h.refreshed_at {
            format!("Updated {}", hub::fmt_ago(t))
        } else {
            "Not refreshed yet".to_string()
        };
        let busy = h.loading;
        div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::SM))
            .text_size(px(size::XS))
            .text_color(Tokens::text_3())
            .when(busy, |d| d.child(spinner("yt-ref-spin", Tokens::text_3(), icon_px::SM)))
            .child(div().flex_1().child(text))
            .when(h.account.connected(), |d| {
                d.child(icon_button("yt-refresh", ph::ARROWS_CLOCKWISE, "Refresh channel").small().on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.refresh(cx)))))
            })
    }

    fn side(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.dock.is_none() {
            self.dock = ui_assistant::dock(window, cx);
        }
        let body = match self.dock.clone() {
            Some(d) => div().flex_1().min_h_0().child(d).into_any_element(),
            None => div().p(px(space::LG)).child(empty_view(glyphs::ASSISTANT, "Chat unavailable", "The assistant could not start.").compact()).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDE_W))
            .h_full()
            .map(column)
            .child(
                div()
                    .flex_none()
                    .h(px(52.))
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .px(px(space::MD))
                    .border_b_1()
                    .border_color(Tokens::border())
                    .child(icon_ph(glyphs::ASSISTANT, Tokens::accent(), icon_px::MD))
                    .child(div().flex_1().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).child("Growth chat"))
                    .child(icon_button("yt-side-close", ph::SIDEBAR_SIMPLE, "Hide chat").small().on_click(cx.listener(|this, _, _, cx| {
                        this.side_open = false;
                        cx.notify();
                    }))),
            )
            .child(body)
            .into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let h = self.hub.read(cx);
        let title: SharedString = match (&self.focus, self.tab) {
            (Some(Focus::Video(_)), _) => "Video details".into(),
            (Some(Focus::Item(_)), _) => "Upload details".into(),
            (None, t) => format!("Channel {}", t.label().to_lowercase()).into(),
        };
        let uploading = h.uploading;
        let planned = h.queue.iter().filter(|i| i.state == mv_youtube::UploadState::Planned).count();
        div()
            .flex_none()
            .h(px(52.))
            .flex()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::LG))
            .border_b_1()
            .border_color(Tokens::border())
            .when(self.focus.is_some(), |d| {
                d.child(icon_button("yt-back", glyphs::BACK, "Back").small().on_click(cx.listener(|this, _, _, cx| {
                    this.focus = None;
                    this.ai = AiState::default();
                    this.update_context(cx);
                    cx.notify();
                })))
            })
            .child(div().flex_1().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title))
            .when(uploading, |d| d.child(div().flex().items_center().gap(px(space::XS)).text_size(px(size::SM)).text_color(Tokens::accent()).child(spinner("yt-up-spin", Tokens::accent(), icon_px::SM)).child("Uploading")))
            .when(h.connected() && planned > 0 && !uploading, |d| {
                d.child(btn("yt-start", format!("Upload {planned} planned"), ButtonKind::Primary).small().icon(ph::UPLOAD_SIMPLE).on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.start_uploads(cx)))))
            })
            .when(uploading, |d| d.child(btn("yt-stop", "Pause after this one", ButtonKind::Secondary).small().icon(ph::PAUSE).on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.stop_uploads(cx))))))
            .when(!self.side_open, |d| {
                d.child(icon_button("yt-side-open", glyphs::ASSISTANT, "Show growth chat").small().on_click(cx.listener(|this, _, _, cx| {
                    this.side_open = true;
                    cx.notify();
                })))
            })
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        if !h.account.connected() && self.tab != Tab::Settings {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p(px(space::XL))
                .child(
                    empty_view(ph::YOUTUBE_LOGO, "Connect your YouTube channel", "Link your channel once to upload Studio recaps, schedule them in chapter order, and see how every video is doing.")
                        .action(btn("yt-go-connect", "Connect channel", ButtonKind::Primary).icon(ph::PLUGS_CONNECTED).on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Settings, cx)))),
                )
                .into_any_element();
        }
        let error = h.error.clone();
        let signed_out = h.signed_out;
        let content = match (&self.focus, self.tab) {
            (Some(f), _) => {
                let f = f.clone();
                self.editor(&f, window, cx)
            }
            (None, Tab::Dashboard) => self.dashboard(cx),
            (None, Tab::Content) => self.content(cx),
            (None, Tab::Schedule) => self.schedule(window, cx),
            (None, Tab::Analytics) => self.analytics(cx),
            (None, Tab::Insights) => self.insights(cx),
            (None, Tab::Settings) => self.settings(window, cx),
        };
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("yt-body")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::LG))
                            .p(px(space::XL))
                            .children(error.map(|e| {
                                inline_alert(AlertKind::Error, if signed_out { "Signed out" } else { "Something went wrong" }, e)
                                    .action(if signed_out { "Connect again" } else { "Retry" }, cx.listener(move |this, _, _, cx| {
                                        this.hub.update(cx, |h, cx| if signed_out { h.connect(cx) } else { h.refresh(cx) });
                                    }))
                                    .build()
                            }))
                            .child(content),
                    ),
            )
            .child(div().absolute().inset_0().child(Scrollbar::new(&self.scroll).id("yt-bar").axis(ScrollbarAxis::Vertical).mode(ScrollbarMode::Always).viewport_from_layout()))
            .into_any_element()
    }

    /// Opens the editor on a video or queue item, filling the fields from it.
    pub(crate) fn open_editor(&mut self, f: Focus, window: &mut Window, cx: &mut Context<Self>) {
        let h = self.hub.read(cx);
        let (title, desc, tags, when) = match &f {
            Focus::Video(id) => match h.video(id) {
                Some(v) => (v.title.clone(), v.description.clone(), v.tags.join(", "), v.publish_at.map(hub::to_local_input).unwrap_or_default()),
                None => return,
            },
            Focus::Item(id) => match h.queue.iter().find(|i| &i.id == id) {
                Some(i) => (i.title.clone(), i.description.clone(), i.tags.join(", "), i.publish_at.map(hub::to_local_input).unwrap_or_default()),
                None => return,
            },
        };
        self.title.update(cx, |s, cx| s.set_value(title, window, cx));
        self.desc.update(cx, |s, cx| s.set_value(desc, window, cx));
        self.tags.update(cx, |s, cx| s.set_value(tags, window, cx));
        self.when.update(cx, |s, cx| s.set_value(when, window, cx));
        self.ask.update(cx, |s, cx| s.set_value("", window, cx));
        self.focus = Some(f);
        self.ai = AiState::default();
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.update_context(cx);
        cx.notify();
    }

    pub(crate) fn toast_err(cx: &mut App, msg: impl Into<SharedString>) {
        toast::show(cx, Toast::error(msg));
    }
}

impl Render for YouTubePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vw = f32::from(window.viewport_size().width);
        // Narrow windows: the chat takes its turn instead of squeezing the page.
        let side = (self.side_open && vw > 1100.).then(|| self.side(window, cx));
        let nav = self.nav(cx);
        let toolbar = self.toolbar(cx);
        let body = self.body(window, cx);
        div()
            .flex()
            .size_full()
            .gap(px(6.))
            .p(px(6.))
            .bg(Tokens::canvas())
            .child(nav)
            .child(div().flex().flex_col().flex_1().min_w_0().h_full().map(column).child(toolbar).child(body))
            .children(side)
    }
}

/// A Studio-style column: rounded panel on the sidebar fill.
pub(crate) fn column(d: Div) -> Div {
    d.relative().rounded(px(radius::LG)).overflow_hidden().bg(Tokens::sidebar()).border_1().border_color(Tokens::border())
}

/// A content card.
pub(crate) fn card() -> Div {
    div().flex().flex_col().gap(px(space::MD)).p(px(space::LG)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

/// Card heading with an icon.
pub(crate) fn card_title(icon: Ph, text: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap(px(space::SM)).text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(icon_ph(icon, Tokens::text_2(), icon_px::MD)).child(text.into())
}

/// A round channel avatar from a URL (or a placeholder).
pub(crate) fn avatar(url: &str, d: f32) -> Div {
    let base = div().flex_none().size(px(d)).rounded_full().overflow_hidden().bg(Tokens::well());
    if url.is_empty() {
        base
    } else {
        base.child(img(SharedString::from(url.to_string())).size_full().object_fit(ObjectFit::Cover))
    }
}

/// A 16:9 video thumbnail from a URL.
pub(crate) fn thumb(url: &str, w: f32) -> Div {
    let base = div().flex_none().w(px(w)).h(px(w * 9. / 16.)).rounded(px(radius::SM)).overflow_hidden().bg(Tokens::well());
    if url.is_empty() {
        base.flex().items_center().justify_center().child(icon_ph(ph::FILM_STRIP, Tokens::text_3(), icon_px::LG))
    } else {
        base.child(img(SharedString::from(url.to_string())).size_full().object_fit(ObjectFit::Cover))
    }
}

/// Small label under a heading.
pub(crate) fn hint(text: impl Into<SharedString>) -> Div {
    div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(text.into())
}
