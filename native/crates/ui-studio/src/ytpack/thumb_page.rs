//! Studio › Thumbnails: pick a manga, then its Magnific thumbnail generator (`thumb_view`). The
//! page opens on the manga used last; Settings › Magnific holds the key and defaults.
use super::magnific;
use super::thumb_view::ThumbGen;
use super::*;
use mv_ui::{page_title, Route};

/// Bumped by Settings › Magnific when the key, defaults or credits change.
pub(crate) struct MagnificChanged(pub u64);
impl Global for MagnificChanged {}

pub(crate) fn notify_changed(cx: &mut App) {
    let n = cx.try_global::<MagnificChanged>().map(|g| g.0).unwrap_or(0);
    cx.set_global(MagnificChanged(n + 1));
}

pub(crate) struct ThumbnailsPage {
    library: Load<Vec<MangaGroup>>,
    selected: Option<String>,
    gens: HashMap<String, Entity<ThumbGen>>,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl ThumbnailsPage {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<MagnificChanged>() {
            cx.set_global(MagnificChanged(0));
        }
        let sub = cx.observe_global::<MagnificChanged>(|this, cx| {
            for g in this.gens.values() {
                g.update(cx, |g, cx| {
                    g.refresh_settings();
                    cx.notify();
                });
            }
        });
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(crate::data::load).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.library = match res {
                    Ok(groups) => {
                        let last = magnific::Prefs::load().manga;
                        this.selected = groups.iter().find(|g| g.id == last).or(groups.first()).map(|g| g.id.clone());
                        Load::Ready(groups)
                    }
                    Err(e) => Load::Failed(format!("{e:#}")),
                };
                cx.notify();
            });
        })
        .detach();
        ThumbnailsPage { library: Load::Loading, selected: None, gens: HashMap::new(), scroll: ScrollHandle::new(), _subs: vec![sub] }
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        let mut p = magnific::Prefs::load();
        p.manga = id.clone();
        p.save();
        self.selected = Some(id);
        cx.notify();
    }

    fn generator(&mut self, g: &MangaGroup, cx: &mut Context<Self>) -> Entity<ThumbGen> {
        if let Some(e) = self.gens.get(&g.id) {
            return e.clone();
        }
        let e = cx.new(|_| ThumbGen::new(g));
        self.gens.insert(g.id.clone(), e.clone());
        e
    }

    fn manga_strip(&self, groups: &[MangaGroup], cx: &mut Context<Self>) -> Div {
        div().flex().flex_wrap().gap(px(space::SM)).children(groups.iter().enumerate().map(|(i, g)| {
            let on = self.selected.as_deref() == Some(g.id.as_str());
            let id = g.id.clone();
            let made = super::thumbs::Gallery::load(&g.id).thumbs.len();
            div()
                .id(("thp-manga", i))
                .flex()
                .items_center()
                .gap(px(space::SM))
                .w(px(250.))
                .p(px(6.))
                .pr(px(space::MD))
                .rounded(px(radius::LG))
                .border_1()
                .border_color(if on { Tokens::accent() } else { Tokens::border() })
                .bg(if on { Tokens::accent_soft() } else { Tokens::surface() })
                .cursor_pointer()
                .hover(|s| s.bg(Tokens::hover()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select(id.clone(), cx)))
                .child(
                    div()
                        .flex_none()
                        .w(px(40.))
                        .h(px(56.))
                        .rounded(px(radius::MD))
                        .overflow_hidden()
                        .bg(Tokens::surface_2())
                        .when_some(g.cover.clone().filter(|c| c.is_file()), |d, c| d.child(img(c).size_full().object_fit(ObjectFit::Cover))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).line_clamp(2).child(g.title.clone()))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if made == 0 { "No thumbnails yet".to_string() } else { format!("{made} thumbnail{}", if made == 1 { "" } else { "s" }) })),
                )
        }))
    }
}

impl Render for ThumbnailsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = page_title("Thumbnails")
            .icon(ph::IMAGES)
            .subtitle("YouTube thumbnails made with Magnific. The AI reads the manga, studies what gets clicked, and writes the prompt; @Name puts a character's face in.")
            .action(btn("thp-settings", "Magnific settings", ButtonKind::Secondary).icon(ph::GEAR_SIX).on_click(|_, window, cx| mv_ui::navigate(Route::Settings { tab: Some("magnific".into()) }, window, cx)));
        let body: AnyElement = match &self.library {
            Load::Loading => skeleton_rows(&[None, Some(400.), None], 14.).into_any_element(),
            Load::Failed(e) => div().text_size(px(size::SM)).text_color(Tokens::danger()).child(format!("Could not load your manga. {e}")).into_any_element(),
            Load::Ready(groups) if groups.is_empty() => div()
                .flex()
                .flex_col()
                .gap(px(space::LG))
                .child(muted("Download a manga's chapters and let Studio read one; its characters become the face cards used here. Meanwhile, here is what wins clicks."))
                .child(super::thumb_view::rules_guide(None))
                .into_any_element(),
            Load::Ready(groups) => {
                let groups = groups.clone();
                let current = self.selected.clone().and_then(|id| groups.iter().find(|g| g.id == id).cloned()).or_else(|| groups.first().cloned());
                let strip = self.manga_strip(&groups, cx);
                let gen = current.map(|g| self.generator(&g, cx));
                div().flex().flex_col().gap(px(space::LG)).child(strip).children(gen.map(|g| AnyView::from(g).into_any_element())).into_any_element()
            }
        };
        div().size_full().flex().flex_col().child(
            div()
                .id("thp-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(div().flex().flex_col().gap(px(space::MD)).px(px(size::PAGE_X)).py(px(size::PAGE_Y)).child(header).child(body)),
        )
    }
}
