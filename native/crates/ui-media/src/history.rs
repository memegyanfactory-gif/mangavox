//! Media history (resume points, grouped by day) and My List (the watchlist with a status per
//! title), from the shared SQLite database.
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::media::{self, MediaEvent, MediaTitle, PlaybackState, Shelf};
use mv_core::repo::watchlist::{self, WatchlistEntry};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, card_grid, confirm_dialog, cover_tile, empty_view, error_view, filter_chip, focus_ring_shadow, glyphs, icon_tag, navigate, overflow_menu, page, page_title, ph,
    section_header, segmented, skeleton_card, ButtonKind, MenuItem, Ph, Route, Tone,
};

use crate::common::{cover_of, db, fetch, fraction, history_changed, list_changed, list_epoch, restore_status, route_for_media, Load};
use crate::look::{self, time_left};

/// How long a removed history entry can still be brought back before it is deleted for good.
/// Longer than the 6 s Undo toast, so hovering the toast (which pauses it) still leaves time.
const UNDO_WINDOW: Duration = Duration::from_secs(10);

fn loading_grid() -> Div {
    card_grid().children((0..10).map(|_| skeleton_card()))
}

/// Browse buttons for the empty states: the next step is finding something to watch.
fn browse_actions(empty: mv_ui::EmptyState) -> mv_ui::EmptyState {
    empty
        .action(btn("browse-anime", "Browse anime", ButtonKind::Primary).icon(glyphs::ANIME).on_click(|_, w, cx| navigate(Route::AnimeHome, w, cx)))
        .secondary(btn("browse-movies", "Browse movies", ButtonKind::Ghost).icon(glyphs::MOVIES).on_click(|_, w, cx| navigate(Route::MoviesHome, w, cx)))
}

// ---------- history ----------

const KINDS: &[(Option<&str>, &str, Ph)] = &[(None, "All", ph::SQUARES_FOUR), (Some("anime"), "Anime", glyphs::ANIME), (Some("movie"), "Movies", glyphs::MOVIES), (Some("series"), "Series", ph::MONITOR_PLAY)];

/// Day groups, like Netflix Viewing activity and YouTube History.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum When {
    Today,
    Yesterday,
    ThisWeek,
    Earlier,
}

impl When {
    fn label(self) -> &'static str {
        match self {
            When::Today => "Today",
            When::Yesterday => "Yesterday",
            When::ThisWeek => "This week",
            When::Earlier => "Earlier",
        }
    }

    /// Group for something watched `days` calendar days ago.
    fn from_days(days: i64) -> When {
        match days {
            ..=0 => When::Today,
            1 => When::Yesterday,
            2..=6 => When::ThisWeek,
            _ => When::Earlier,
        }
    }

    /// Group for a timestamp in ms, by local calendar day.
    fn of(updated_ms: i64) -> When {
        let Some(t) = Local.timestamp_millis_opt(updated_ms).single() else { return When::Earlier };
        When::from_days((Local::now().date_naive() - t.date_naive()).num_days())
    }
}

/// A removed entry waiting out its Undo window.
struct Pending {
    item: PlaybackState,
    index: usize,
    cancelled: Rc<Cell<bool>>,
    committed: Rc<Cell<bool>>,
}

pub struct MediaHistory {
    kind: Option<&'static str>,
    items: Load<Vec<PlaybackState>>,
    /// Entries removed but still inside their Undo window, kept out of a reread (it would bring
    /// them back until the delete commits).
    hidden: HashSet<String>,
    /// `ui_player::watch_epoch()` when the list was read. The page is kept for Back, so what was
    /// played from here meanwhile shows its new progress on return.
    watch_seen: u64,
}

impl MediaHistory {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut s = Self { kind: None, items: Load::Loading, hidden: HashSet::new(), watch_seen: 0 };
        s.load(cx);
        s
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let kind = self.kind;
        self.watch_seen = ui_player::watch_epoch();
        fetch(cx, db(move |c| media::list_history(c, kind, None, 200)), move |v: &mut Self, l| {
            if v.kind != kind {
                return;
            }
            v.items = match l {
                Load::Ready(mut items) => {
                    items.retain(|h| !v.hidden.contains(&h.media_id));
                    Load::Ready(items)
                }
                // A failed reread keeps the list on screen.
                Load::Failed(_) if v.items.ready().is_some() => return,
                other => other,
            };
        });
    }

    fn set_kind(&mut self, kind: Option<&'static str>, cx: &mut Context<Self>) {
        if self.kind != kind {
            self.kind = kind;
            self.items = Load::Loading;
            self.load(cx);
            cx.notify();
        }
    }

    /// Hides the entry at once and deletes it after the Undo window; Undo puts it back.
    /// Material 3 snackbar (single lightweight Undo for reversible actions).
    fn remove(&mut self, media_id: String, cx: &mut Context<Self>) {
        let Load::Ready(v) = &mut self.items else { return };
        let Some(index) = v.iter().position(|h| h.media_id == media_id) else { return };
        let item = v.remove(index);
        self.hidden.insert(media_id.clone());
        cx.notify();
        let p = Pending { item, index, cancelled: Rc::new(Cell::new(false)), committed: Rc::new(Cell::new(false)) };
        let (cancelled, committed, id) = (p.cancelled.clone(), p.committed.clone(), media_id);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(UNDO_WINDOW).await;
            if !cancelled.get() {
                committed.set(true);
                let key = id.clone();
                // The homes and detail pages reread Continue Watching / Resume after this.
                let _ = db(move |c| {
                    media::remove_history(c, &key)?;
                    history_changed();
                    Ok(())
                })
                .await;
                let _ = this.update(cx, |v, _| v.hidden.remove(&id));
            }
        })
        .detach();
        let weak = cx.weak_entity();
        let title = p.item.title.clone();
        let pending = Rc::new(p);
        toast::show(
            cx,
            Toast::success(format!("Removed {title} from history")).icon(glyphs::DELETE).action("Undo", move |_, cx| {
                let p = pending.clone();
                if weak.update(cx, |v, cx| v.undo_remove(&p, cx)).is_err() && !p.committed.get() {
                    // The page is gone: just keep the entry.
                    p.cancelled.set(true);
                }
            }),
        );
    }

    fn undo_remove(&mut self, p: &Pending, cx: &mut Context<Self>) {
        if p.cancelled.get() {
            return;
        }
        p.cancelled.set(true);
        self.hidden.remove(&p.item.media_id);
        if p.committed.get() {
            // Too late to cancel: write the resume point back (per-episode marks are gone),
            // then reload the list.
            let h = p.item.clone();
            let kind = self.kind;
            fetch(
                cx,
                db(move |c| {
                    let e = MediaEvent {
                        media_id: Some(h.media_id.clone()),
                        kind: h.kind.clone(),
                        event_type: if h.completed { "complete".into() } else { "progress".into() },
                        provider_id: h.provider_id.clone(),
                        episode_number: h.episode_number,
                        position_secs: Some(h.position_secs),
                        duration_secs: Some(h.duration_secs),
                        ..Default::default()
                    };
                    media::record_event(c, &e)?;
                    history_changed();
                    media::list_history(c, kind, None, 200)
                }),
                move |v: &mut Self, l| {
                    if v.kind == kind {
                        v.items = l;
                    }
                },
            );
            return;
        }
        // Back into the list only when it shows that kind (the filter may have changed since).
        if let Load::Ready(v) = &mut self.items {
            if self.kind.is_none_or(|k| k == p.item.kind) {
                v.insert(p.index.min(v.len()), p.item.clone());
            }
        }
        cx.notify();
    }

    fn count(&self) -> usize {
        self.items.ready().map_or(0, Vec::len)
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        let kind = self.kind;
        let n = self.count();
        self.items = Load::Ready(Vec::new());
        cx.notify();
        let done = if n == 1 { "Cleared 1 title from history".to_string() } else { format!("Cleared {n} titles from history") };
        cx.spawn(async move |this, cx| {
            let r = db(move |c| {
                media::clear_shelf(c, kind, Shelf::History)?;
                history_changed();
                Ok(())
            })
            .await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(()) => toast::show(cx, Toast::success(done)),
                    Err(e) => {
                        toast::show(cx, Toast::error(format!("Could not clear history: {e}")));
                        v.items = Load::Loading;
                        v.load(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Clearing cannot be undone, so it asks first (NN/g: confirm only high-consequence actions).
    fn confirm_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.count();
        let scope = KINDS.iter().find(|k| k.0 == self.kind).filter(|k| k.0.is_some()).map(|k| format!(" {}", k.1.to_lowercase())).unwrap_or_default();
        let weak = cx.weak_entity();
        confirm_dialog(
            if n == 1 { format!("Clear 1 title from your{scope} history?") } else { format!("Clear {n} titles from your{scope} history?") },
            "Their resume points are removed. This cannot be undone.",
            "Clear history",
            true,
        )
        .cancel_label("Keep history")
        .on_confirm(move |_, cx| {
            let _ = weak.update(cx, |v, cx| v.clear(cx));
        })
        .open(window, cx);
    }

    fn card(&self, i: usize, h: &PlaybackState, cx: &mut Context<Self>) -> Option<Div> {
        let route = route_for_media(&h.media_id, &h.title)?;
        let left = time_left(h.position_secs, h.duration_secs).unwrap_or_else(|| "Started".into());
        let sub = match (h.completed, h.episode_number) {
            (true, Some(n)) => format!("E{n} · Watched"),
            (true, None) => "Watched".to_string(),
            (false, Some(n)) => format!("E{n} · {left}"),
            (false, None) => left,
        };
        let p = fraction(h);
        let (id, id2) = (h.media_id.clone(), h.media_id.clone());
        let card = cover_tile(("hist", i), h.title.clone())
            .sub(sub)
            .cover(cover_of(&h.cover_url))
            .when(!h.completed && p > 0.0, |c| c.progress(p))
            .on_click(move |_, w, cx| navigate(route.clone(), w, cx))
            // Keyboard path for the hover X: Delete on a focused card removes it.
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "delete" {
                    cx.stop_propagation();
                    this.remove(id2.clone(), cx);
                }
            }));
        let group: SharedString = format!("hist-card-{i}").into();
        Some(
            div().relative().group(group.clone()).child(card).child(
                div().absolute().top(px(6.)).right(px(6.)).child(
                    look::glass_button(("hist-rm", i), glyphs::CLOSE, "Remove from history (Delete)", 28.)
                        // Shown on hover, and whenever Tab reaches it.
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .focus_visible(|s| s.opacity(1.).shadow(focus_ring_shadow()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // The card under the button would open the title.
                            cx.stop_propagation();
                            this.remove(id.clone(), cx);
                        })),
                ),
            ),
        )
    }
}

impl Render for MediaHistory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.watch_seen != ui_player::watch_epoch() && self.items.ready().is_some() {
            self.load(cx);
        }
        let n = self.count();
        let body = match &self.items {
            Load::Loading => loading_grid().into_any_element(),
            Load::Failed(e) => error_view(e.clone()).title("History could not be loaded").retry(cx.listener(|this, _, _, cx| {
                this.items = Load::Loading;
                this.load(cx);
                cx.notify();
            })).into_any_element(),
            Load::Ready(v) if v.is_empty() => match self.kind {
                None => browse_actions(empty_view(glyphs::HISTORY, "Nothing watched yet", "Titles you play show up here, so you can pick up where you left off.")).into_any_element(),
                Some(_) => empty_view(glyphs::HISTORY, "Nothing here yet", "Nothing of this kind has been played. Pick All to see everything.").into_any_element(),
            },
            Load::Ready(v) => {
                let mut groups: Vec<(When, Vec<(usize, PlaybackState)>)> = vec![];
                for (i, h) in v.iter().enumerate() {
                    let w = When::of(h.updated_at);
                    match groups.iter_mut().find(|(g, _)| *g == w) {
                        Some((_, items)) => items.push((i, h.clone())),
                        None => groups.push((w, vec![(i, h.clone())])),
                    }
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .children(groups.into_iter().map(|(w, items)| {
                        let count = items.len();
                        div()
                            .flex()
                            .flex_col()
                            .child(section_header(w.label(), None).count(count))
                            .child(card_grid().children(items.into_iter().filter_map(|(i, h)| self.card(i, &h, cx))))
                    }))
                    .into_any_element()
            }
        };
        let selected = KINDS.iter().position(|k| k.0 == self.kind).unwrap_or(0);
        let clear = btn("clear-history", "Clear history", ButtonKind::Secondary).icon(glyphs::DELETE);
        let clear = if n == 0 { clear.disabled("Nothing to clear") } else { clear.on_click(cx.listener(|this, _, w, cx| this.confirm_clear(w, cx))) };
        page("media-history")
            .child(page_title("History").icon(glyphs::HISTORY).subtitle(match &self.items {
                Load::Ready(_) if n == 1 => "1 title".to_string(),
                Load::Ready(_) => format!("{n} titles"),
                _ => "What you played, newest first".to_string(),
            }).action(clear))
            .child(
                div().mb(px(20.)).child(segmented("history-kind", KINDS.iter().map(|(_, label, glyph)| (Some(*glyph), *label)), selected).on_change(cx.listener(|this, ix: &usize, _, cx| {
                    let kind = KINDS.get(*ix).and_then(|k| k.0);
                    this.set_kind(kind, cx);
                }))),
            )
            .child(body)
    }
}

// ---------- My List ----------

/// (stored status, label, glyph, tone). Status tags carry a glyph and a word, never colour alone.
const STATUSES: &[(&str, &str, Ph, Tone)] = &[
    ("watching", "Watching", ph::PLAY_CIRCLE, Tone::Accent),
    ("planned", "Planned", ph::CLOCK, Tone::Neutral),
    ("completed", "Completed", ph::CHECK_CIRCLE, Tone::Success),
    ("on_hold", "On hold", ph::PAUSE_CIRCLE, Tone::Warning),
    ("dropped", "Dropped", ph::X_CIRCLE, Tone::Danger),
];

fn status_meta(status: &str) -> (&'static str, Ph, Tone) {
    STATUSES.iter().find(|s| s.0 == status).map(|s| (s.1, s.2, s.3)).unwrap_or(("Planned", ph::CLOCK, Tone::Neutral))
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "anime" => "Anime",
        "movie" => "Movie",
        "series" => "Series",
        _ => "Title",
    }
}

type Fav = (WatchlistEntry, Option<MediaTitle>);

/// My List: everything saved with Add to My List (the watchlist), filterable by status.
pub struct Favorites {
    status: Option<&'static str>,
    items: Load<Vec<Fav>>,
    /// `common::list_epoch()` when the list was read. The page is kept for Back, so a title
    /// removed or moved to Completed on its detail page meanwhile shows that on return.
    list_seen: u64,
}

/// A reread list in the order already on screen: a status change bumps `updated_at` and would
/// move that card to the front under the pointer. Titles not shown yet lead, newest first.
fn keep_order(shown: &[Fav], mut fresh: Vec<Fav>) -> Vec<Fav> {
    fresh.sort_by_key(|(e, _)| shown.iter().position(|(s, _)| s.media_id == e.media_id).map_or((0, 0), |p| (1, p)));
    fresh
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
        let mut s = Self { status: None, items: Load::Loading, list_seen: 0 };
        s.load(cx);
        s
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.list_seen = list_epoch();
        fetch(cx, load_favs(), |v: &mut Self, l| {
            v.items = match (l, &v.items) {
                (Load::Ready(fresh), Load::Ready(shown)) => Load::Ready(keep_order(shown, fresh)),
                // A failed reread keeps the list on screen.
                (Load::Failed(_), Load::Ready(_)) => return,
                (l, _) => l,
            };
        });
    }

    /// Removes at once; Undo writes the same status back.
    fn remove(&mut self, media_id: String, cx: &mut Context<Self>) {
        let Load::Ready(v) = &mut self.items else { return };
        let Some(index) = v.iter().position(|(e, _)| e.media_id == media_id) else { return };
        let fav = v.remove(index);
        cx.notify();
        let id = media_id.clone();
        fetch(
            cx,
            db(move |c| {
                watchlist::remove_status(c, &id)?;
                list_changed();
                Ok(())
            }),
            |_: &mut Self, _| {},
        );
        let title = fav.1.as_ref().map(|t| t.title.clone()).unwrap_or_else(|| "Title".into());
        let weak = cx.weak_entity();
        let fav = Rc::new(fav);
        toast::show(
            cx,
            Toast::success(format!("Removed {title} from My List")).icon(glyphs::SAVED).action("Undo", move |_, cx| {
                let fav = fav.clone();
                if weak.update(cx, |v, cx| v.restore(index, &fav, cx)).is_err() {
                    // The page is gone: write the status back anyway.
                    cx.background_spawn(restore_status(fav.0.clone())).detach();
                }
            }),
        );
    }

    fn restore(&mut self, index: usize, fav: &Fav, cx: &mut Context<Self>) {
        fetch(cx, restore_status(fav.0.clone()), |_: &mut Self, _| {});
        if let Load::Ready(v) = &mut self.items {
            if !v.iter().any(|(x, _)| x.media_id == fav.0.media_id) {
                v.insert(index.min(v.len()), fav.clone());
            }
        }
        cx.notify();
    }

    fn set_status(&mut self, media_id: String, status: &'static str, cx: &mut Context<Self>) {
        let Load::Ready(v) = &mut self.items else { return };
        let Some((e, _)) = v.iter_mut().find(|(e, _)| e.media_id == media_id) else { return };
        if e.status == status {
            return;
        }
        e.status = status.to_string();
        let kind = e.kind.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = db(move |c| {
                watchlist::set_status(c, &media_id, &kind, status)?;
                list_changed();
                Ok(())
            })
            .await;
            // A failed write says so and rereads the list (before, the page went to Loading and
            // stayed there).
            if let Err(e) = r {
                let _ = this.update(cx, |v, cx| {
                    toast::show(cx, Toast::error(format!("The status could not be saved: {e}")));
                    v.items = Load::Loading;
                    v.load(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn card(&self, i: usize, e: &WatchlistEntry, t: &Option<MediaTitle>, cx: &mut Context<Self>) -> Option<Div> {
        let title = t.as_ref().map(|t| t.title.clone()).unwrap_or_else(|| e.media_id.clone());
        let route = route_for_media(&e.media_id, &title)?;
        let sub = [Some(kind_label(&e.kind).to_string()), t.as_ref().and_then(|t| t.year).map(|y| y.to_string())].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let (label, glyph, tone) = status_meta(&e.status);
        let weak = cx.weak_entity();
        let mut items = vec![MenuItem::heading("Status")];
        for &(value, name, g, _) in STATUSES {
            let (weak, id) = (weak.clone(), e.media_id.clone());
            items.push(
                MenuItem::new(name, move |_, cx| {
                    let _ = weak.update(cx, |v, cx| v.set_status(id.clone(), value, cx));
                })
                .icon(g)
                .checked(e.status == value),
            );
        }
        let (weak2, id) = (weak, e.media_id.clone());
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("Remove from My List", move |_, cx| {
                let _ = weak2.update(cx, |v, cx| v.remove(id.clone(), cx));
            })
            .icon(glyphs::DELETE)
            .danger(),
        );
        let card = cover_tile(("fav", i), title).sub(sub).cover(cover_of(&t.as_ref().and_then(|t| t.cover_url.clone()))).on_click(move |_, w, cx| navigate(route.clone(), w, cx));
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .w(px(150.))
                .child(card)
                .child(div().flex().items_center().justify_between().child(icon_tag(glyph, label, tone)).child(overflow_menu(("fav-menu", i), items).small())),
        )
    }
}

impl Render for Favorites {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.list_seen != list_epoch() && self.items.ready().is_some() {
            self.load(cx);
        }
        let status = self.status;
        let all = self.items.ready().map_or(0, Vec::len);
        // Only statuses that have titles get a chip, so no filter ever leads to an empty page.
        let counts: Vec<(&'static str, &'static str, Ph, usize)> = STATUSES
            .iter()
            .filter_map(|&(value, label, glyph, _)| {
                let n = self.items.ready().map_or(0, |v| v.iter().filter(|(e, _)| e.status == value).count());
                (n > 0).then_some((value, label, glyph, n))
            })
            .collect();
        let chips = (counts.len() > 1).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .mb(px(20.))
                .child(filter_chip("status-all", "All", status.is_none()).count(all).on_click(cx.listener(|this, _, _, cx| {
                    this.status = None;
                    cx.notify();
                })))
                .children(counts.iter().enumerate().map(|(i, &(value, label, glyph, n))| {
                    filter_chip(("status", i), label, status == Some(value)).icon(glyph).count(n).on_click(cx.listener(move |this, _, _, cx| {
                        this.status = if this.status == Some(value) { None } else { Some(value) };
                        cx.notify();
                    }))
                }))
        });
        let body = match &self.items {
            Load::Loading => loading_grid().into_any_element(),
            Load::Failed(e) => error_view(e.clone()).title("My List could not be loaded").retry(cx.listener(|this, _, _, cx| {
                this.items = Load::Loading;
                this.load(cx);
                cx.notify();
            })).into_any_element(),
            Load::Ready(v) if v.is_empty() => browse_actions(empty_view(glyphs::SAVED, "Your list is empty", "Choose Add to My List on any anime, movie or series to keep it here.")).into_any_element(),
            Load::Ready(v) => {
                let shown: Vec<(usize, &Fav)> = v.iter().enumerate().filter(|(_, (e, _))| status.is_none_or(|s| e.status == s)).collect();
                if shown.is_empty() {
                    empty_view(ph::FUNNEL_X, "Nothing with this status", "Pick All to see every saved title.")
                        .action(btn("status-show-all", "Show all", ButtonKind::Secondary).on_click(cx.listener(|this, _, _, cx| {
                            this.status = None;
                            cx.notify();
                        })))
                        .into_any_element()
                } else {
                    card_grid().children(shown.into_iter().filter_map(|(i, (e, t))| self.card(i, e, t, cx))).into_any_element()
                }
            }
        };
        page("favorites")
            .child(page_title("My List").icon(glyphs::SAVED).subtitle(match &self.items {
                Load::Ready(_) if all == 1 => "1 saved title".to_string(),
                Load::Ready(_) => format!("{all} saved titles"),
                _ => "Anime, movies and series you saved".to_string(),
            }))
            .children(chips)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{keep_order, status_meta, When, WatchlistEntry};

    #[test]
    fn reread_list_keeps_the_cards_in_place() {
        let fav = |id: &str, at: i64| (WatchlistEntry { media_id: id.into(), kind: "movie".into(), status: "planned".into(), updated_at: at }, None);
        let shown = vec![fav("a", 3), fav("b", 2), fav("c", 1)];
        // "c" changed status (newest now) and "d" was added elsewhere: "d" leads, the rest stay put.
        let fresh = vec![fav("d", 9), fav("c", 8), fav("a", 3), fav("b", 2)];
        let ids: Vec<String> = keep_order(&shown, fresh).into_iter().map(|(e, _)| e.media_id).collect();
        assert_eq!(ids, ["d", "a", "b", "c"]);
    }

    #[test]
    fn history_groups_by_calendar_day() {
        assert_eq!(When::from_days(0), When::Today);
        assert_eq!(When::from_days(-1), When::Today);
        assert_eq!(When::from_days(1), When::Yesterday);
        assert_eq!(When::from_days(6), When::ThisWeek);
        assert_eq!(When::from_days(30), When::Earlier);
    }

    #[test]
    fn unknown_status_reads_as_planned() {
        assert_eq!(status_meta("completed").0, "Completed");
        assert_eq!(status_meta("weird").0, "Planned");
    }
}
