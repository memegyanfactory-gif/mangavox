//! Media history (resume points) and favorites (watchlist), from the shared SQLite database.
use gpui_kit::*;
use mv_core::repo::media::{self, MediaTitle, PlaybackState, Shelf};
use mv_core::repo::watchlist::{self, WatchlistEntry};
use mv_ui::{button, card_grid, chip, cover_card, empty_state, error_state, page, page_header, skeleton_card, ButtonKind, Tokens};

use crate::common::{cover_of, db, fetch, on_open, route_for_media, Load};

fn card_with_remove(card: Stateful<Div>, id: (&'static str, usize), on_remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Div {
    div().flex().flex_col().gap(px(2.)).child(card).child(
        div()
            .id(id)
            .px(px(4.))
            .text_size(px(11.))
            .text_color(Tokens::text_3())
            .cursor_pointer()
            .hover(|s| s.text_color(Tokens::text_1()))
            .child("Remove")
            .on_click(on_remove),
    )
}

fn loading_grid() -> Div {
    card_grid().children((0..10).map(|_| skeleton_card()))
}

// ---------- history ----------

const KINDS: &[(&str, Option<&str>)] = &[("All", None), ("Anime", Some("anime")), ("Movies", Some("movie")), ("Series", Some("series"))];

pub struct MediaHistory {
    kind: Option<&'static str>,
    items: Load<Vec<PlaybackState>>,
}

impl MediaHistory {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut s = Self { kind: None, items: Load::Loading };
        s.load(cx);
        s
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let kind = self.kind;
        fetch(cx, db(move |c| media::list_history(c, kind, None, 200)), move |v: &mut Self, l| {
            if v.kind == kind {
                v.items = l;
            }
        });
    }

    fn remove(&mut self, media_id: String, cx: &mut Context<Self>) {
        if let Load::Ready(v) = &mut self.items {
            v.retain(|h| h.media_id != media_id);
        }
        cx.notify();
        fetch(cx, db(move |c| media::remove_history(c, &media_id)), move |_: &mut Self, _| {});
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        let kind = self.kind;
        self.items = Load::Ready(Vec::new());
        cx.notify();
        fetch(cx, db(move |c| media::clear_shelf(c, kind, Shelf::History)), move |_: &mut Self, _| {});
    }
}

impl Render for MediaHistory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.items {
            Load::Loading => loading_grid(),
            Load::Failed(e) => error_state(e.clone()),
            Load::Ready(v) if v.is_empty() => empty_state("Nothing watched yet", "Titles you play show up here so you can pick up where you left off."),
            Load::Ready(v) => card_grid().children(v.iter().enumerate().filter_map(|(i, h)| {
                let route = route_for_media(&h.media_id, &h.title)?;
                let pct = if h.duration_secs > 0.0 { (h.position_secs / h.duration_secs * 100.0).round() as i64 } else { 0 };
                let sub = match (h.completed, h.episode_number) {
                    (true, Some(n)) => format!("Episode {n}  ·  watched"),
                    (true, None) => "Watched".to_string(),
                    (false, Some(n)) => format!("Episode {n}  ·  {pct}%"),
                    (false, None) => format!("{pct}%"),
                };
                let card = on_open(cover_card(("hist", i), h.title.clone().into(), sub.into(), cover_of(&h.cover_url), None), route);
                let id = h.media_id.clone();
                let remove = cx.listener(move |this, _: &ClickEvent, _, cx| this.remove(id.clone(), cx));
                Some(card_with_remove(card, ("hist-rm", i), remove))
            })),
        };
        page("media-history")
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .child(page_header("History", None))
                    .child(button("clear-history", "Clear", ButtonKind::Ghost).on_click(cx.listener(|this, _, _, cx| this.clear(cx)))),
            )
            .child(div().flex().gap(px(8.)).mb(px(20.)).children(KINDS.iter().map(|(label, kind)| {
                let kind: Option<&'static str> = *kind;
                chip(*label, *label, self.kind == kind).on_click(cx.listener(move |this, _, _, cx| {
                    this.kind = kind;
                    this.items = Load::Loading;
                    this.load(cx);
                    cx.notify();
                }))
            })))
            .child(body)
    }
}

// ---------- favorites ----------

const STATUSES: &[(&str, Option<&str>)] = &[
    ("All", None),
    ("Watching", Some("watching")),
    ("Planned", Some("planned")),
    ("Completed", Some("completed")),
    ("On hold", Some("on_hold")),
    ("Dropped", Some("dropped")),
];

type Fav = (WatchlistEntry, Option<MediaTitle>);

pub struct Favorites {
    status: Option<&'static str>,
    items: Load<Vec<Fav>>,
}

fn load_favs() -> impl std::future::Future<Output = Result<Vec<Fav>, String>> + Send {
    db(|c| {
        let mut out: Vec<Fav> = watchlist::list(c, None)?
            .into_iter()
            .filter(|e| e.kind != "manga")
            .map(|e| {
                let t = media::get_title(c, &e.media_id).ok().flatten();
                (e, t)
            })
            .collect();
        out.sort_by_key(|(e, _)| std::cmp::Reverse(e.updated_at));
        Ok(out)
    })
}

impl Favorites {
    pub fn new(cx: &mut Context<Self>) -> Self {
        fetch(cx, load_favs(), |v: &mut Self, l| v.items = l);
        Self { status: None, items: Load::Loading }
    }

    fn remove(&mut self, media_id: String, cx: &mut Context<Self>) {
        if let Load::Ready(v) = &mut self.items {
            v.retain(|(e, _)| e.media_id != media_id);
        }
        cx.notify();
        fetch(cx, db(move |c| watchlist::remove_status(c, &media_id)), move |_: &mut Self, _| {});
    }
}

impl Render for Favorites {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status;
        let body = match &self.items {
            Load::Loading => loading_grid(),
            Load::Failed(e) => error_state(e.clone()),
            Load::Ready(v) => {
                let shown: Vec<_> = v.iter().filter(|(e, _)| status.is_none_or(|s| e.status == s)).collect();
                if shown.is_empty() {
                    empty_state("Your list is empty", "Use Add to list on any anime, movie or series.")
                } else {
                    card_grid().children(shown.into_iter().enumerate().filter_map(|(i, (e, t))| {
                        let title = t.as_ref().map(|t| t.title.clone()).unwrap_or_else(|| e.media_id.clone());
                        let route = route_for_media(&e.media_id, &title)?;
                        let sub = e.status.replace('_', " ");
                        let card = on_open(cover_card(("fav", i), title.into(), sub.into(), cover_of(&t.as_ref().and_then(|t| t.cover_url.clone())), None), route);
                        let id = e.media_id.clone();
                        let remove = cx.listener(move |this, _: &ClickEvent, _, cx| this.remove(id.clone(), cx));
                        Some(card_with_remove(card, ("fav-rm", i), remove))
                    }))
                }
            }
        };
        page("favorites")
            .child(page_header("Favorites", Some("Your watchlist".into())))
            .child(div().flex().flex_wrap().gap(px(8.)).mb(px(20.)).children(STATUSES.iter().map(|(label, s)| {
                let s: Option<&'static str> = *s;
                chip(*label, *label, status == s).on_click(cx.listener(move |this, _, _, cx| {
                    this.status = s;
                    cx.notify();
                }))
            })))
            .child(body)
    }
}
