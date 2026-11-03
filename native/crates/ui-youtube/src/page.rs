//! The YouTube page, laid out like YouTube Studio (measured on studio.youtube.com): the 248 px
//! nav with the channel's big avatar and its sections on the left (Dashboard, Content, Analytics,
//! Community, Competitors, Automation, Schedule, Insights, Settings), the section with its 32 px
//! title in the middle, and the growth chat on the right, told about the channel and the video
//! being edited so it can fix titles, SEO and timing with you. Drawn with Studio's own colours
//! (`yt`), dark or light with the app.
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarAxis, ScrollbarMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{empty_view, glyphs, icon_ph, inline_alert, ph, size, spinner, AlertKind, Ph, Tokens};
use mv_youtube::MetaSuggestion;

use crate::hub::{self, Hub};
use crate::yt;

pub(crate) const SIDE_W: f32 = 384.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Dashboard,
    Content,
    Analytics,
    Community,
    Competitors,
    Automation,
    Schedule,
    Insights,
    Settings,
}

impl Tab {
    /// The nav, top to bottom (Settings sits at the bottom like Studio's).
    pub const NAV: [Tab; 8] = [Tab::Dashboard, Tab::Content, Tab::Analytics, Tab::Community, Tab::Competitors, Tab::Automation, Tab::Schedule, Tab::Insights];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Content => "Content",
            Tab::Analytics => "Analytics",
            Tab::Community => "Community",
            Tab::Competitors => "Competitors",
            Tab::Automation => "Automation",
            Tab::Schedule => "Schedule",
            Tab::Insights => "Insights",
            Tab::Settings => "Settings",
        }
    }

    /// The page title, Studio's wording.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Dashboard => "Channel dashboard",
            Tab::Content => "Channel content",
            Tab::Analytics => "Channel analytics",
            Tab::Community => "Community",
            Tab::Competitors => "Competitors",
            Tab::Automation => "Automation",
            Tab::Schedule => "Release schedule",
            Tab::Insights => "Insights",
            Tab::Settings => "Settings",
        }
    }

    pub fn icon(self) -> Ph {
        match self {
            Tab::Dashboard => ph::SQUARES_FOUR,
            Tab::Content => ph::PLAY_CIRCLE,
            Tab::Analytics => ph::CHART_BAR,
            Tab::Community => ph::USERS,
            Tab::Competitors => ph::BINOCULARS,
            Tab::Automation => ph::ROBOT,
            Tab::Schedule => ph::CALENDAR_DOTS,
            Tab::Insights => ph::LIGHTBULB,
            Tab::Settings => glyphs::SETTINGS,
        }
    }

    /// Works without a connected channel (public data or local work).
    pub fn offline(self) -> bool {
        matches!(self, Tab::Competitors | Tab::Automation | Tab::Settings)
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
    /// Content tab: filter text.
    pub(crate) filter: Entity<InputState>,
    /// Community tab.
    pub(crate) comments: crate::comments_view::CommentsState,
    /// Competitors tab.
    pub(crate) rivals: crate::competitors_view::RivalsState,
    /// Automation tab.
    pub(crate) auto: crate::automation_view::AutoViewState,
    _subs: Vec<Subscription>,
}

impl YouTubePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_tab(None, window, cx)
    }

    /// Opens on `tab` (the Studio nav's Automation entry), else Dashboard (Settings when no
    /// channel is connected).
    pub fn with_tab(tab: Option<Tab>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        let when = cx.new(|cx| InputState::new(window, cx).placeholder("2026-11-03 6:30 PM (empty = right away)"));
        let ask = cx.new(|cx| InputState::new(window, cx).placeholder("Optional: \"more dramatic\", \"mention the big fight\"…"));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
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
        for s in [&title, &tags, &filter] {
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
        let comments = crate::comments_view::CommentsState::new(window, cx);
        let rivals = crate::competitors_view::RivalsState::new(window, cx);
        let auto = crate::automation_view::AutoViewState::new(window, cx);
        let (runner, tracker) = (crate::automation::runner(cx), crate::rivals::tracker(cx));
        subs.push(cx.observe(&runner, |_, _, cx| cx.notify()));
        subs.push(cx.observe(&tracker, |_, _, cx| cx.notify()));
        // The clock ticks over to the next minute.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(15)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();
        let start = tab.unwrap_or(if hub.read(cx).account.connected() { Tab::Dashboard } else { Tab::Settings });
        let mut page = Self {
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
            filter,
            comments,
            rivals,
            auto,
            _subs: subs,
        };
        page.on_open(start, cx);
        page
    }

    /// What a section loads when it opens.
    fn on_open(&mut self, tab: Tab, cx: &mut Context<Self>) {
        match tab {
            // The golden-time research refreshes itself when it is a week old.
            Tab::Schedule => self.hub.update(cx, |h, cx| {
                if h.timing_stale() && !h.timing_busy && h.timing_error.is_none() {
                    h.research_timing(cx);
                }
            }),
            Tab::Community => self.hub.update(cx, |h, cx| h.load_comments(false, cx)),
            _ => {}
        }
    }

    pub(crate) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.on_open(tab, cx);
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
        let mut s = String::from("The user is on MangaVox's YouTube page (a YouTube Studio for their manga recap channel). You are their YouTube growth coach: give concrete titles, descriptions, hashtags, tags, thumbnails ideas and release timing. You can read and change the channel with the youtube_* tools, read what competitors do with the competitors_* tools, and see the automation with automation_status. Times you mention are local, 12-hour (\"6:30 PM\").");
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
        let auto_on = crate::automation::runner(cx).read(cx).state.full_auto;
        let waiting = crate::automation::runner(cx).read(cx).waiting();
        let h = self.hub.read(cx);
        let (avatar, name) = match &h.channel {
            Some(c) => (c.thumbnail.clone(), c.title.clone()),
            None => (String::new(), if h.account.connected() { "Loading…".to_string() } else { "Not connected".to_string() }),
        };
        let insights = h.suggestions().iter().filter(|s| s.severity >= mv_youtube::Severity::Warning).count();
        let unanswered = h.comments_unanswered();
        let tab = self.tab;
        let row = |t: Tab, badge: usize, cx: &mut Context<Self>| {
            let on = t == tab;
            div()
                .id(("yt-nav", t as usize))
                .flex()
                .items_center()
                .gap(px(24.))
                .h(px(40.))
                .pl(px(12.))
                .pr(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(on, |d| d.bg(yt::tone(0.1)))
                .when(!on, |d| d.hover(|s| s.bg(yt::tone(0.05))))
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(t, cx)))
                .child(yt::icon_of(t.icon(), 24., yt::TEXT))
                .child(yt::line(14., if on { FontWeight::MEDIUM } else { FontWeight::NORMAL }, yt::TEXT, t.label()).flex_1())
                .when(t == Tab::Automation && auto_on, |d| d.child(yt::badge("AUTO", 0xffffff, yt::c(yt::RED).into())))
                .when(badge > 0, |d| d.child(yt::text(12., 16., FontWeight::MEDIUM, yt::TEXT_2).child(badge.to_string())))
        };
        let items: Vec<_> = Tab::NAV
            .into_iter()
            .map(|t| {
                let badge = match t {
                    Tab::Insights => insights,
                    Tab::Community => unanswered,
                    Tab::Automation => waiting,
                    _ => 0,
                };
                row(t, badge, cx)
            })
            .collect();
        let settings = row(Tab::Settings, 0, cx);
        div()
            .id("yt-nav")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(yt::NAV_W))
            .h_full()
            .pr(px(8.))
            .pl(px(12.))
            .bg(yt::c(yt::BG))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .pt(px(24.))
                    .pb(px(16.))
                    .child(div().mb(px(8.)).child(if avatar.is_empty() {
                        div().size(px(112.)).rounded_full().bg(yt::tone(0.1)).flex().items_center().justify_center().child(yt::icon_of(ph::YOUTUBE_LOGO, 48., yt::RED)).into_any_element()
                    } else {
                        yt::avatar(&avatar, 112.).into_any_element()
                    }))
                    .child(yt::text(15., 22., FontWeight::MEDIUM, yt::TEXT).child("Your channel"))
                    .child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, name).max_w(px(200.))),
            )
            .child(div().flex().flex_col().gap(px(2.)).children(items))
            .child(div().flex_1().min_h(px(16.)))
            .child(self.clock(cx))
            .child(settings)
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
        let q = h.yt.quota();
        let left = q.left(hub::now());
        let quota_tip = format!(
            "YouTube API quota today (estimate): {} of {} units used, {} calls, about {} uploads left. Resets at midnight Pacific ({}).",
            yt::num(q.used as u64),
            yt::num(mv_youtube::quota::DAILY as u64),
            q.calls,
            q.uploads_left(hub::now()),
            crate::when::when(mv_youtube::quota::next_reset(hub::now()))
        );
        let busy = h.loading;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .py(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(busy, |d| d.child(spinner("yt-ref-spin", yt::c(yt::TEXT_2), icon_px::SM)))
                    .child(yt::line(12., FontWeight::NORMAL, yt::TEXT_2, text).flex_1())
                    .when(h.account.connected(), |d| {
                        d.child(
                            div()
                                .id("yt-refresh")
                                .flex_none()
                                .cursor_pointer()
                                .tooltip(mv_ui::tip("Refresh channel"))
                                .on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.refresh_now(cx))))
                                .child(yt::icon_of(ph::ARROWS_CLOCKWISE, 16., yt::TEXT_2)),
                        )
                    }),
            )
            .child(
                div()
                    .id("yt-quota")
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .tooltip(mv_ui::tip(quota_tip))
                    .child(div().flex().justify_between().child(yt::caption("API quota today")).child(yt::caption(format!("{}%", (left as f32 / mv_youtube::quota::DAILY as f32 * 100.).round()))))
                    .child(div().h(px(4.)).w_full().rounded(px(2.)).bg(yt::tone(0.1)).child(div().h_full().rounded(px(2.)).w(relative(left as f32 / mv_youtube::quota::DAILY as f32)).bg(yt::c(if left < mv_youtube::quota::UPLOAD { yt::ERROR } else { yt::LINK })))),
            )
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
                    .child(mv_ui::icon_button("yt-side-close", ph::SIDEBAR_SIMPLE, "Hide chat").small().on_click(cx.listener(|this, _, _, cx| {
                        this.side_open = false;
                        cx.notify();
                    }))),
            )
            .child(body)
            .into_any_element()
    }

    /// The 32 px page title row with the section's actions on the right.
    fn title_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let h = self.hub.read(cx);
        let title: SharedString = match (&self.focus, self.tab) {
            (Some(Focus::Video(_)), _) => "Video details".into(),
            (Some(Focus::Item(_)), _) => "Upload details".into(),
            (None, t) => t.title().into(),
        };
        let uploading = h.uploading;
        let planned = h.queue.iter().filter(|i| i.state == mv_youtube::UploadState::Planned).count();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .min_w_0()
            .px(px(32.))
            .pt(px(23.))
            .pb(px(16.))
            .when(self.focus.is_some(), |d| {
                d.child(yt::icon_btn("yt-back", glyphs::BACK, "Back").on_click(cx.listener(|this, _, _, cx| {
                    this.focus = None;
                    this.ai = AiState::default();
                    this.update_context(cx);
                    cx.notify();
                })))
            })
            .child(yt::page_title(title).flex_1())
            .when(uploading, |d| d.child(div().flex().flex_none().items_center().gap(px(6.)).child(spinner("yt-up-spin", yt::c(yt::LINK), icon_px::SM)).child(yt::text(14., 20., FontWeight::MEDIUM, yt::LINK).child("Uploading"))))
            .when(h.connected() && planned > 0 && !uploading, |d| {
                d.child(yt::pill_icon("yt-start", ph::UPLOAD_SIMPLE, format!("Upload {planned} planned")).on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.start_uploads(cx)))))
            })
            .when(uploading, |d| d.child(yt::pill_icon("yt-stop", ph::PAUSE, "Pause after this one").on_click(cx.listener(|this, _, _, cx| this.hub.update(cx, |h, cx| h.stop_uploads(cx))))))
            .when(!self.side_open, |d| {
                d.child(yt::icon_btn("yt-side-open", glyphs::ASSISTANT, "Show growth chat").on_click(cx.listener(|this, _, _, cx| {
                    this.side_open = true;
                    cx.notify();
                })))
            })
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let h = self.hub.read(cx);
        if !h.account.connected() && !self.tab.offline() {
            return div()
                .flex()
                .flex_1()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(16.))
                .p(px(32.))
                .child(yt::icon_of(ph::YOUTUBE_LOGO, 64., yt::RED))
                .child(yt::text(20., 28., FontWeight::BOLD, yt::TEXT).child("Connect your YouTube channel"))
                .child(yt::para(14., yt::TEXT_2, 3, "Link your channel once to upload Studio recaps, schedule them in chapter order, answer comments and see how every video is doing.").max_w(px(460.)).text_center())
                .child(yt::pill_primary("yt-go-connect", "Connect channel").on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Settings, cx))))
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
            (None, Tab::Analytics) => self.analytics(cx),
            (None, Tab::Community) => self.community(window, cx),
            (None, Tab::Competitors) => self.competitors(window, cx),
            (None, Tab::Automation) => self.automation(window, cx),
            (None, Tab::Schedule) => self.schedule(window, cx),
            (None, Tab::Insights) => self.insights(cx),
            (None, Tab::Settings) => self.settings(window, cx),
        };
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div().id("yt-body").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(px(24.))
                        .px(px(32.))
                        .pb(px(40.))
                        .children(error.map(|e| {
                            inline_alert(AlertKind::Error, if signed_out { "Signed out" } else { "Something went wrong" }, e)
                                .action(if signed_out { "Connect again" } else { "Retry" }, cx.listener(move |this, _, _, cx| {
                                    this.hub.update(cx, |h, cx| if signed_out { h.connect(cx) } else { h.refresh_now(cx) });
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
                Some(v) => (v.title.clone(), v.description.clone(), v.tags.join(", "), v.publish_at.map(crate::when::input).unwrap_or_default()),
                None => return,
            },
            Focus::Item(id) => match h.queue.iter().find(|i| &i.id == id) {
                Some(i) => (i.title.clone(), i.description.clone(), i.tags.join(", "), i.publish_at.map(crate::when::input).unwrap_or_default()),
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
        let side = (self.side_open && vw > 1280.).then(|| self.side(window, cx));
        let nav = self.nav(cx);
        let title = self.title_row(cx);
        let body = self.body(window, cx);
        div()
            .flex()
            .size_full()
            .bg(yt::c(yt::BG))
            .font_family(yt::font(cx))
            .text_color(yt::c(yt::TEXT))
            .child(nav)
            .child(div().flex().flex_col().flex_1().min_w_0().h_full().child(title).child(body))
            .children(side.map(|s| div().flex_none().h_full().p(px(6.)).child(s)))
    }
}

/// A Studio-style column: rounded panel on the sidebar fill (the chat panel, the Suggestions page).
pub(crate) fn column(d: Div) -> Div {
    d.relative().rounded(px(radius::LG)).overflow_hidden().bg(Tokens::sidebar()).border_1().border_color(Tokens::border())
}

/// A content card (Suggestions page, app theme).
pub(crate) fn card() -> Div {
    div().flex().flex_col().gap(px(space::MD)).p(px(space::LG)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

/// Card heading with an icon (Suggestions page, app theme).
pub(crate) fn card_title(icon: Ph, text: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap(px(space::SM)).text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(icon_ph(icon, Tokens::text_2(), icon_px::MD)).child(text.into())
}

/// A Studio card (YouTube page sections).
pub(crate) fn yt_card() -> Div {
    yt::card()
}

/// A Studio card title with an icon, cut to fit.
pub(crate) fn yt_card_title(icon: Ph, text: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap(px(10.)).min_w_0().child(yt::icon_of(icon, 20., yt::TEXT_2)).child(yt::card_title(text))
}

/// A grey line of help text (wraps).
pub(crate) fn yt_hint(text: impl Into<SharedString>) -> Div {
    yt::text(13., 19., FontWeight::NORMAL, yt::TEXT_2).min_w_0().whitespace_normal().child(text.into())
}

/// A 16:9 video thumbnail from a URL.
pub(crate) fn thumb(url: &str, w: f32) -> Div {
    yt::thumb(url, w, None)
}
