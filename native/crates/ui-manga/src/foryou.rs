//! Browse -> "For you": suggestions from the whole taste profile with a "why" under each, "More
//! like this" / "Not interested" on every card, and the "Your taste" panel (what the profile
//! learned, with a reset). Also the suggestion card the manga page's "You may also like" uses.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui_kit::*;
use mv_core::repo::manga;
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, empty_view, error_view, icon_button, icon_tag, muted, ph, size, tag, ButtonKind, CoverSource, Route, Tokens, Tone};

use crate::backend;
use crate::ops;
use crate::suggest::{self, ForYou, Suggestion};
use crate::taste::{self, FeedbackKind, Kind, TitleRef};
use crate::tags::norm;
use crate::ui::{cover_src, manga_card, skeleton_cover, spawn_cover_downloads, width_probe, CardInfo, Grid, Load};

/// Height of the "why" line under a card (two lines of small text), so rows line up.
const WHY_H: f32 = 30.0;

/// What a suggestion card can do.
pub struct CardActions {
    pub open: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
    pub like: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
    pub dismiss: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

/// A cover card with its "why" line and More like this / Not interested.
pub fn suggestion_card(key: &str, s: &Suggestion, cover: Option<CoverSource>, busy: bool, liked: bool, grid: Grid, act: CardActions) -> Div {
    let CardActions { open, like, dismiss } = act;
    let info = CardInfo { title: s.item.title.clone().into(), cover, busy, ..Default::default() };
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(grid.card_w))
        .gap(px(4.))
        .child(manga_card(SharedString::from(format!("{key}-card")), info, grid).on_click(move |e, w, cx| open(e, w, cx)))
        .child(div().h(px(WHY_H)).text_size(px(size::XS)).line_height(px(15.)).text_color(Tokens::text_3()).line_clamp(2).child(s.why.clone()))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .ml(px(-4.))
                .child(
                    icon_button(SharedString::from(format!("{key}-like")), ph::THUMBS_UP, if liked { "Liked: click to undo" } else { "More like this" })
                        .small()
                        .selected(liked)
                        .on_click(move |e, w, cx| like(e, w, cx)),
                )
                .child(icon_button(SharedString::from(format!("{key}-no")), ph::THUMBS_DOWN, "Not interested").small().on_click(move |e, w, cx| dismiss(e, w, cx))),
        )
}

/// Rows of cards are this much taller than a plain cover grid row.
pub const CARD_EXTRA_H: f32 = WHY_H + 4. + 28. + 4.;

/// The title a card's feedback is about.
pub fn title_ref(s: &Suggestion) -> TitleRef {
    TitleRef { title: s.item.title.clone(), source: s.source.clone(), source_id: s.item.source_id.clone(), author: s.item.author.clone(), ..Default::default() }
}

/// Titles the user said "More like this" to (by title key).
pub fn liked_keys() -> HashSet<String> {
    taste::file().feedback.iter().filter(|f| f.kind == FeedbackKind::Like).map(|f| f.title.key()).collect()
}

/// Open a suggested title (straight away when it is already in the database). `done` runs on
/// the page with the outcome (to clear its busy marker).
pub fn open_suggestion<T: 'static>(s: &Suggestion, window: &mut Window, cx: &mut Context<T>, done: impl Fn(&mut T) + 'static) {
    let (source, item) = (s.source.clone(), s.item.clone());
    cx.spawn_in(window, async move |this, cx| {
        let (src, rid) = (source.clone(), item.source_id.clone());
        let title = item.title.clone();
        let known = mv_ui::bg::blocking(move || backend::with_db(|c| manga::find_by_source(c, &src, &rid))).await.ok().and_then(|r| r.ok()).flatten();
        let res = match known {
            Some(m) => Ok(m),
            None => mv_ui::bg::run(ops::prepare_from_source(source.clone(), item)).await.and_then(|r| r),
        };
        let _ = this.update_in(cx, |this, w, cx| {
            done(this);
            match res {
                Ok(m) => mv_ui::navigate(Route::MangaDetail { manga_id: m.id, source_id: Some(source) }, w, cx),
                Err(e) => toast::show(cx, Toast::error(format!("Could not open {title}: {e:#}"))),
            }
            cx.notify();
        });
    })
    .detach();
}

fn card_key(s: &Suggestion) -> String {
    format!("{}\u{1}{}", s.source, s.item.source_id)
}

// ---------------------------------------------------------------- the For you view

pub struct ForYouView {
    state: Load<ForYou>,
    covers: HashMap<String, PathBuf>,
    /// Card being opened.
    busy: Option<String>,
    liked: HashSet<String>,
    taste_open: bool,
    width: f32,
    scroll: ScrollHandle,
    generation: u64,
    /// [`taste::version`] the shown suggestions match (feedback given elsewhere reloads them).
    seen_taste: u64,
}

impl ForYouView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut v = Self { state: Load::Loading, covers: HashMap::new(), busy: None, liked: HashSet::new(), taste_open: false, width: 0., scroll: ScrollHandle::new(), generation: 0, seen_taste: 0 };
        v.load(false, cx);
        v
    }

    /// Show the kept suggestions when they are fresh, else build them (`force`: always).
    pub fn load(&mut self, force: bool, cx: &mut Context<Self>) {
        self.liked = liked_keys();
        self.seen_taste = taste::version();
        if !force {
            if let Some(fy) = suggest::for_you_cached() {
                self.set_ready(fy, cx);
                return;
            }
        }
        self.generation += 1;
        let generation = self.generation;
        self.state = Load::Loading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(suggest::for_you()).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match res {
                    Ok(fy) => this.set_ready(fy, cx),
                    Err(e) => this.state = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_ready(&mut self, fy: ForYou, cx: &mut Context<Self>) {
        let urls: Vec<(String, Option<String>)> = fy.items.iter().filter_map(|s| s.item.cover_url.clone().map(|u| (u, Some(s.source.clone())))).collect();
        self.state = Load::Ready(fy);
        cx.notify();
        cx.spawn(async move |this, cx| {
            if let Ok((have, missing)) = mv_ui::bg::blocking(move || backend::resolve_covers(urls)).await {
                let _ = this.update(cx, |this, cx| {
                    this.covers.extend(have);
                    spawn_cover_downloads(cx, missing, |t: &mut Self| &mut t.covers);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn toggle_like(&mut self, s: Suggestion, cx: &mut Context<Self>) {
        let key = norm(&s.item.title);
        let on = !self.liked.contains(&key);
        if on {
            self.liked.insert(key);
        } else {
            self.liked.remove(&key);
        }
        taste::set_verdict(title_ref(&s), on.then_some(FeedbackKind::Like));
        suggest::keep_cached();
        self.seen_taste = taste::version();
        cx.notify();
    }

    fn dismiss(&mut self, s: Suggestion, cx: &mut Context<Self>) {
        let Load::Ready(fy) = &mut self.state else { return };
        let Some(i) = fy.items.iter().position(|x| card_key(x) == card_key(&s)) else { return };
        let removed = fy.items.remove(i);
        taste::set_verdict(title_ref(&removed), Some(FeedbackKind::NotInterested));
        suggest::forget_cached(&removed.source, &removed.item.source_id);
        self.seen_taste = taste::version();
        let weak = cx.entity().downgrade();
        let title = removed.item.title.clone();
        toast::show(
            cx,
            Toast::info(format!("You won't see {title} again")).action("Undo", move |_, cx| {
                let back = removed.clone();
                taste::set_verdict(title_ref(&back), None);
                let _ = weak.update(cx, |this, cx| {
                    if let Load::Ready(fy) = &mut this.state {
                        fy.items.insert(i.min(fy.items.len()), back);
                        suggest::put_cached(fy.clone());
                    }
                    this.seen_taste = taste::version();
                    cx.notify();
                });
            }),
        );
        cx.notify();
    }

    fn reset_taste(&mut self, cx: &mut Context<Self>) {
        let old = taste::file();
        taste::reset();
        let weak = cx.entity().downgrade();
        toast::show(
            cx,
            Toast::info("Your taste was reset").action("Undo", move |_, cx| {
                let old = old.clone();
                taste::update(move |f| *f = old);
                let _ = weak.update(cx, |this, cx| this.load(true, cx));
            }),
        );
        self.load(true, cx);
    }

    fn open(&mut self, s: Suggestion, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(card_key(&s));
        cx.notify();
        open_suggestion(&s, window, cx, |this: &mut Self| this.busy = None);
    }

    /// "Your taste": what the profile learned, strongest first, and a reset.
    fn taste_panel(&self, fy: &ForYou, cx: &mut Context<Self>) -> Div {
        let p = &fy.profile;
        let row = |label: &'static str, items: Vec<AnyElement>| {
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(div().w(px(150.)).flex_none().pt(px(2.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(label))
                .child(div().flex_1().flex().flex_wrap().gap(px(6.)).children(items))
        };
        let liked_tags: Vec<AnyElement> = p
            .top(&[Kind::Genre, Kind::Demographic], true, 10)
            .into_iter()
            .map(|s| if s.finishes_most() { icon_tag(ph::CHECK, s.name.clone(), Tone::Success).into_any_element() } else { tag(s.name.clone(), Tone::Accent).into_any_element() })
            .collect();
        let avoided_tags: Vec<AnyElement> = p.top(&[Kind::Genre, Kind::Demographic], false, 6).into_iter().map(|s| tag(s.name.clone(), Tone::Danger).into_any_element()).collect();
        let people: Vec<AnyElement> = p
            .top(&[Kind::Author, Kind::Artist], true, 8)
            .into_iter()
            .map(|s| icon_tag(if s.kind == Kind::Artist { ph::PAINT_BRUSH } else { ph::PENCIL_SIMPLE }, s.name.clone(), Tone::Accent).into_any_element())
            .collect();
        let avoided_people: Vec<AnyElement> = p.top(&[Kind::Author, Kind::Artist], false, 4).into_iter().map(|s| tag(s.name.clone(), Tone::Danger).into_any_element()).collect();
        let none = || vec![muted("Nothing yet").into_any_element()];
        let or_none = |v: Vec<AnyElement>| if v.is_empty() { none() } else { v };
        let stats = format!(
            "Learned from {} titles: {} finished, {} dropped, {} liked. Recent reading counts most; a check means you finish most titles with that tag.",
            p.titles, p.finished, p.dropped, p.likes
        );
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .mx(px(size::PAGE_X))
            .mb(px(16.))
            .p(px(14.))
            .rounded(px(mv_ui::theme::radius::LG))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(row("Genres you like", or_none(liked_tags)))
            .child(row("Genres you avoid", or_none(avoided_tags)))
            .child(row("Creators you like", or_none(people)))
            .child(row("Creators you avoid", or_none(avoided_people)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pt(px(4.))
                    .child(div().flex_1().child(muted(stats)))
                    .child(
                        btn("taste-reset", "Reset taste", ButtonKind::DangerGhost)
                            .small()
                            .icon(ph::ARROW_COUNTER_CLOCKWISE)
                            .tooltip("Forget your likes and start learning again from what you read next")
                            .on_click(cx.listener(|this, _, _, cx| this.reset_taste(cx))),
                    ),
            )
    }
}

impl Render for ForYouView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.seen_taste != taste::version() && matches!(self.state, Load::Ready(_)) {
            self.load(false, cx);
        }
        let grid = Grid::new(self.width, window);
        let head = |sub: String, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(size::PAGE_X))
                .mb(px(12.))
                .child(div().flex_1().child(muted(sub)))
                .child(
                    btn("fy-taste", "Your taste", ButtonKind::Ghost)
                        .small()
                        .icon(ph::BRAIN)
                        .selected(self.taste_open)
                        .tooltip("What the suggestions learned about you")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.taste_open = !this.taste_open;
                            cx.notify();
                        })),
                )
                .child(icon_button("fy-refresh", ph::ARROW_CLOCKWISE, "Find new suggestions").small().on_click(cx.listener(|this, _, _, cx| this.load(true, cx))))
        };
        let body: AnyElement = match &self.state {
            Load::Loading => div()
                .flex()
                .flex_col()
                .child(head("Looking through your sources\u{2026}".into(), cx))
                .children((0..2).map(|_| grid.row().h(px(grid.row_h() + CARD_EXTRA_H)).children((0..grid.cols).map(|_| skeleton_cover(grid)))))
                .into_any_element(),
            Load::Failed(e) => error_view(e.clone()).title("Could not load suggestions").retry(cx.listener(|this, _, _, cx| this.load(true, cx))).into_any_element(),
            Load::Ready(fy) => {
                let p = &fy.profile;
                let sub = if p.is_empty() {
                    "Read, finish, favorite or rate a few titles and this fills with picks for you.".to_string()
                } else {
                    format!("Picked from your reading: {} titles, {} finished", p.titles, p.finished)
                };
                let mut col = div().flex().flex_col().pb(px(32.)).child(head(sub, cx));
                if self.taste_open {
                    col = col.child(self.taste_panel(fy, cx));
                }
                if fy.items.is_empty() {
                    col = col.child(empty_view(
                        ph::SPARKLE,
                        "Nothing to suggest yet",
                        "Suggestions come from what you read, finish and favorite, and from \u{201c}More like this\u{201d} on a manga page. Add a source with tags (like MangaDex) for more.",
                    ));
                } else {
                    let entity = cx.entity();
                    for chunk in fy.items.chunks(grid.cols.max(1)) {
                        col = col.child(div().flex().gap(px(16.)).px(px(size::PAGE_X)).mb(px(18.)).children(chunk.iter().map(|s| {
                            let key = card_key(s);
                            let busy = self.busy.as_deref() == Some(key.as_str());
                            let liked = self.liked.contains(&norm(&s.item.title));
                            let (a, b, c) = (s.clone(), s.clone(), s.clone());
                            let (e1, e2, e3) = (entity.clone(), entity.clone(), entity.clone());
                            let act = CardActions {
                                open: Box::new(move |_, w, cx| e1.update(cx, |t, cx| t.open(a.clone(), w, cx))),
                                like: Box::new(move |_, _, cx| e2.update(cx, |t, cx| t.toggle_like(b.clone(), cx))),
                                dismiss: Box::new(move |_, _, cx| e3.update(cx, |t, cx| t.dismiss(c.clone(), cx))),
                            };
                            let id = format!("fy-{}-{}", s.source, s.item.source_id);
                            suggestion_card(&id, s, cover_src(&self.covers, s.item.cover_url.as_deref()), busy, liked, grid, act)
                        })));
                    }
                }
                div().id("for-you").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(col).into_any_element()
            }
        };
        div().relative().size_full().child(width_probe(cx, |t: &mut Self| &mut t.width)).child(body)
    }
}
