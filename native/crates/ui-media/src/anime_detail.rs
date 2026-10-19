//! Anime detail, Crunchyroll-style: banner with poster, rating, tags and actions, show facts,
//! then an episode grid with thumbnails. Plays via addons; the hero resumes where you left off.
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::cache::DataCache;
use mv_core::repo::media::PlaybackState;
use mv_media::episode_art;
use mv_media::types::{AnimeEpisode, AnimeSearchResult};
use mv_ui::theme::{icon as isz, radius, ramp};
use mv_ui::toast::{self, Toast};
use mv_ui::{bg, btn, empty_view, error_view, focus_ring_shadow, icon, menu_button, ph, size, skeleton, skeleton_line, ButtonKind, Interactive, MenuItem, Ph, RampExt, Tokens};

use crate::common::{
    all_watched, announce_completed, announce_list_toggle, cached, completion_due, fetch, fetch_cached, fraction, in_progress, in_watchlist, jikan, list_epoch, mark_completed,
    mark_watching, media_id, media_title, progress_of, progress_stamp, record_open, set_watchlist, Load, DAY, EPISODES_TTL,
};
use crate::look::{self, EDGE};
use crate::streams::{self, PlayBase, StreamState};

/// Jikan episode pages hold 100 episodes.
const EPISODE_PAGE: usize = 100;

pub struct AnimeDetail {
    mal_id: String,
    title: String,
    details: Load<AnimeSearchResult>,
    episodes: Load<Vec<AnimeEpisode>>,
    /// Jikan page (100 episodes) the list starts at; the range picker jumps between them.
    first_page: u32,
    /// Last Jikan page loaded ("Load more" appends the next one).
    page: u32,
    has_more: bool,
    loading_more: bool,
    /// Bumped on a range jump, so a slower page that arrives later is dropped.
    range_gen: u64,
    in_list: Option<bool>,
    /// `common::list_epoch()` when `in_list` was read: the page is kept for Back, so a title
    /// removed on My List meanwhile no longer shows "In My List".
    list_seen: Option<u64>,
    /// My List is being saved.
    saving: bool,
    streams: StreamState,
    /// The running source lookup; replacing or dropping it cancels the lookup.
    lookup: Option<Task<()>>,
    /// The first source lookup has been started.
    auto: bool,
    /// Episode whose links are shown.
    sel: Option<u32>,
    /// The links open under the clicked episode instead of in the hero.
    inline: bool,
    /// Addon / language chip selected in the sources panel.
    addon_filter: Option<String>,
    /// Episodes finished, drawn dimmed.
    watched: HashSet<u32>,
    /// The saved resume point for this show.
    resume: Option<PlaybackState>,
    /// The resume point has been read (the first source lookup waits for it).
    resume_loaded: bool,
    /// `common::progress_stamp()` when `watched` was loaded (player saves and History edits).
    watch_epoch: Option<(u64, u64)>,
    /// Stamp whose `watched` and `resume` are in, and the one last checked for "finished".
    progress_epoch: Option<(u64, u64)>,
    checked_epoch: Option<(u64, u64)>,
    /// The full synopsis is expanded in the hero.
    synopsis_open: bool,
    newest_first: bool,
    scroll: ScrollHandle,
}

/// Episode whose links were last open per title (and whether inline), so reopening the page
/// comes back to the same selection.
fn last_selection() -> &'static Mutex<HashMap<String, (u32, bool)>> {
    static M: OnceLock<Mutex<HashMap<String, (u32, bool)>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// "in 3 days" / "in 5 hours" until a unix timestamp.
fn airs_in(at: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let s = (at - now).max(0);
    match (s / 86400, s / 3600) {
        (d, _) if d >= 2 => format!("in {d} days"),
        (1, _) => "tomorrow".into(),
        (_, h) if h >= 1 => format!("in {h} hours"),
        _ => "soon".into(),
    }
}

/// What the hero's play button does next, from the saved resume point (NN/g visibility of
/// system status; Netflix and Crunchyroll switch to Resume / Continue).
#[derive(Debug, PartialEq)]
struct NextUp {
    label: String,
    episode: u32,
    /// (watched fraction, "12m left") when resuming mid-episode.
    progress: Option<(f32, String)>,
}

fn next_up(movie: bool, resume: Option<&PlaybackState>, episodes: &[u32], more_exist: bool) -> NextUp {
    let first = episodes.first().copied().unwrap_or(1);
    let Some(h) = resume else {
        return NextUp { label: if movie { "Watch now".into() } else { format!("Start watching E{first}") }, episode: first, progress: None };
    };
    let ep = h.episode_number.map_or(first, |n| n.max(1) as u32);
    if in_progress(h) {
        let left = look::time_left(h.position_secs, h.duration_secs).unwrap_or_default();
        return NextUp { label: if movie { "Resume".into() } else { format!("Resume E{ep}") }, episode: ep, progress: Some((fraction(h), left)) };
    }
    let finished = h.completed || (h.duration_secs > 0.0 && h.position_secs >= h.duration_secs * 0.95);
    match (movie, finished) {
        (true, true) => NextUp { label: "Watch again".into(), episode: 1, progress: None },
        (true, false) => NextUp { label: "Watch now".into(), episode: 1, progress: None },
        (false, false) => NextUp { label: format!("Watch E{ep}"), episode: ep, progress: None },
        (false, true) => match episodes.iter().copied().find(|n| *n > ep).or_else(|| more_exist.then_some(ep + 1)) {
            Some(n) => NextUp { label: format!("Continue E{n}"), episode: n, progress: None },
            None => NextUp { label: format!("Watch again E{first}"), episode: first, progress: None },
        },
    }
}

/// Whether an episode after `last` exists when only the show's total (if known) can tell.
fn more_after(last: Option<i64>, total: Option<u32>) -> bool {
    let last = last.map_or(0, |n| u32::try_from(n).unwrap_or(0));
    total.is_none_or(|t| last < t)
}

fn retry_sources(v: &mut AnimeDetail, window: &mut Window, cx: &mut Context<AnimeDetail>) {
    let ep = v.sel.unwrap_or(1);
    v.lookup(ep, false, window, cx);
}

fn close_inline(v: &mut AnimeDetail) {
    v.inline = false;
    v.sel = None;
    v.streams = StreamState::Idle;
    v.lookup = None;
    last_selection().lock().unwrap_or_else(|e| e.into_inner()).remove(&v.mal_id);
}

impl AnimeDetail {
    pub fn new(mal_id: String, title: String, cx: &mut Context<Self>) -> Self {
        let details = Self::fetch_details(&mal_id, &title, cx);
        let mut page = Self {
            mal_id,
            title,
            details,
            episodes: Load::Loading,
            first_page: 1,
            page: 1,
            has_more: false,
            loading_more: false,
            range_gen: 0,
            in_list: None,
            list_seen: None,
            saving: false,
            streams: StreamState::Idle,
            lookup: None,
            auto: false,
            sel: None,
            inline: false,
            addon_filter: None,
            watched: HashSet::new(),
            resume: None,
            resume_loaded: false,
            watch_epoch: None,
            progress_epoch: None,
            checked_epoch: None,
            synopsis_open: false,
            newest_first: false,
            scroll: ScrollHandle::new(),
        };
        page.load_page(1, true, cx);
        page
    }

    fn fetch_details(mal_id: &str, title: &str, cx: &mut Context<Self>) -> Load<AnimeSearchResult> {
        let (id, t) = (mal_id.to_string(), title.to_string());
        let key = format!("jikan:anime:{mal_id}");
        let warm = DataCache::shared().age(&key).is_some();
        let details = fetch_cached(
            cx,
            key,
            DAY,
            async move {
                let d = jikan()?.anime_details(&id).await.map_err(|e| e.to_string())?;
                if !warm {
                    let _ = record_open(media_title("anime", &id, &d, &t)).await;
                }
                Ok(d)
            },
            |v: &mut Self, l| v.details = l,
        );
        if let Load::Ready(d) = &details {
            fetch(cx, record_open(media_title("anime", mal_id, d, title)), |_: &mut Self, _| {});
        }
        details
    }

    fn reload_details(&mut self, cx: &mut Context<Self>) {
        self.details = Load::Loading;
        cx.notify();
        let (id, t) = (self.mal_id.clone(), self.title.clone());
        self.details = Self::fetch_details(&id, &t, cx);
    }

    /// Fetch one page of episodes, then their stills. `replace` starts the list at that page (first
    /// load, or a jump with the range picker); otherwise the page is appended ("Load more").
    /// Cached pages show at once and are refetched only when stale.
    fn load_page(&mut self, page: u32, replace: bool, cx: &mut Context<Self>) {
        let id = self.mal_id.clone();
        let key = format!("jikan:episodes:{id}:{page}");
        let generation = self.range_gen;
        let merge = |cur: &mut Vec<AnimeEpisode>, more: Vec<AnimeEpisode>| {
            // A stale cached page and its refetch both arrive: merge by number, never duplicate.
            cur.retain(|e| !more.iter().any(|m| m.number == e.number));
            cur.extend(more);
            cur.sort_by_key(|e| e.number);
        };
        let shown = match DataCache::shared().get::<Vec<AnimeEpisode>>(&key) {
            Some((eps, age)) => {
                self.has_more = eps.len() >= EPISODE_PAGE;
                self.loading_more = false;
                match &mut self.episodes {
                    Load::Ready(cur) if !replace => merge(cur, eps),
                    _ => self.episodes = Load::Ready(eps),
                }
                self.fill_art(cx);
                cx.notify();
                if age < EPISODES_TTL {
                    return;
                }
                true
            }
            None => false,
        };
        cx.spawn(async move |this, cx| {
            let res = bg::run(cached(key, async move { jikan()?.anime_episodes(&id, page).await.map_err(|e| e.to_string()) })).await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |v, cx| {
                if v.range_gen != generation {
                    return;
                }
                v.loading_more = false;
                match (res, replace) {
                    // A failed refresh keeps the cached page that is already shown.
                    (Err(_), _) if shown => {}
                    (Err(e), false) => {
                        // Keep what is shown; offer the button again.
                        v.page -= 1;
                        v.has_more = true;
                        toast::show(cx, Toast::error(format!("More episodes could not be loaded: {e}")));
                    }
                    (Err(e), true) => v.episodes = Load::Failed(e),
                    (Ok(more), _) => {
                        v.has_more = more.len() >= EPISODE_PAGE;
                        match &mut v.episodes {
                            Load::Ready(cur) if !replace || shown => merge(cur, more),
                            _ => v.episodes = Load::Ready(more),
                        }
                        v.fill_art(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Range picker: show episodes from Jikan page `page` (100 per page) instead of scrolling
    /// through "Load more" (Crunchyroll's episode ranges for long shows).
    fn jump_to(&mut self, page: u32, cx: &mut Context<Self>) {
        if page == self.first_page && self.episodes.ready().is_some() {
            return;
        }
        self.range_gen += 1;
        // Links open under an episode move up to the hero, since that episode leaves the grid.
        self.inline = false;
        self.first_page = page;
        self.page = page;
        self.has_more = false;
        self.loading_more = false;
        self.newest_first = false;
        self.episodes = Load::Loading;
        cx.notify();
        self.load_page(page, true, cx);
    }

    /// Episodes the show has so far: the total when Jikan knows it, else the latest aired one.
    fn episode_total(&self) -> Option<u32> {
        let d = self.details.ready()?;
        d.episode_count.filter(|n| *n > 0).or_else(|| d.next_episode.map(|n| n.saturating_sub(1)).filter(|n| *n > 0))
    }

    /// Jikan pages to offer in the range picker: every page up to the total, or up to the furthest
    /// page known to exist.
    fn range_pages(&self) -> u32 {
        let known = self.page + u32::from(self.has_more);
        self.episode_total().map_or(known, |t| t.div_ceil(EPISODE_PAGE as u32)).max(known)
    }

    /// Give episodes without a still their own (Kitsu / IMDB), instead of the series banner on every card.
    fn fill_art(&mut self, cx: &mut Context<Self>) {
        let art_key = format!("episode-art:{}", self.mal_id);
        let Load::Ready(eps) = &mut self.episodes else { return };
        let known: HashMap<u32, String> = DataCache::shared().get(&art_key).map(|(m, _)| m).unwrap_or_default();
        for e in eps.iter_mut().filter(|e| e.thumbnail.is_none()) {
            e.thumbnail = known.get(&e.number).cloned();
        }
        let mut missing: Vec<AnimeEpisode> = eps.iter().filter(|e| e.thumbnail.is_none()).cloned().collect();
        if missing.is_empty() {
            return;
        }
        let finished = self.details.ready().filter(|d| d.status.as_deref().is_some_and(|s| s.to_lowercase().contains("finished")));
        let expected = finished.and_then(|d| d.episode_count);
        let id = self.mal_id.clone();
        cx.spawn(async move |this, cx| {
            let Ok(filled) = bg::run(async move {
                episode_art::fill_thumbnails(&id, &mut missing, expected).await;
                missing
            })
            .await
            else {
                return;
            };
            let art: HashMap<u32, String> = filled.into_iter().filter_map(|e| Some((e.number, e.thumbnail?))).collect();
            if !art.is_empty() {
                let mut all = known;
                all.extend(art.clone());
                let _ = bg::blocking(move || DataCache::shared().put(&art_key, &all)).await;
            }
            let _ = this.update(cx, |v, cx| {
                if let Load::Ready(cur) = &mut v.episodes {
                    for e in cur.iter_mut().filter(|e| e.thumbnail.is_none()) {
                        e.thumbnail = art.get(&e.number).cloned();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn is_movie(&self) -> bool {
        self.details.ready().is_some_and(|d| d.anime_type.as_deref() == Some("Movie"))
    }

    fn display_title(&self) -> String {
        self.details.ready().map(|d| d.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| self.title.clone())
    }

    fn episode_numbers(&self) -> Vec<u32> {
        self.episodes.ready().map(|v| v.iter().map(|e| e.number).collect()).unwrap_or_default()
    }

    fn next(&self) -> NextUp {
        // A later range is on screen: the first episodes are not loaded, so the hero works from
        // the resume point and the show's total (it said "Continue E221" after the last of 220).
        if self.first_page > 1 {
            let more = more_after(self.resume.as_ref().and_then(|h| h.episode_number), self.episode_total());
            return next_up(self.is_movie(), self.resume.as_ref(), &[], more);
        }
        let total = self.details.ready().and_then(|d| d.episode_count);
        let eps = self.episode_numbers();
        let more = self.has_more || total.is_some_and(|t| eps.last().is_none_or(|l| *l < t));
        next_up(self.is_movie(), self.resume.as_ref(), &eps, more)
    }

    /// Play the best known link at once, otherwise look it up and play.
    fn play(&mut self, episode: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.in_list == Some(true) {
            fetch(cx, mark_watching(media_id("anime", &self.mal_id)), |_: &mut Self, _| {});
        }
        if let Some(req) = streams::best(&self.streams, Some(1), Some(episode)) {
            streams::play_now(req, window, cx);
            return;
        }
        self.lookup(episode, true, window, cx);
    }

    /// [`Self::play`] from the hero (`from_list` false) or an episode card. When the links still have
    /// to be found, the progress and any failure show next to what was clicked: under the card's
    /// row, or in the hero. Before, a card far down the list reported into the hero, off screen.
    fn play_at(&mut self, episode: u32, from_list: bool, window: &mut Window, cx: &mut Context<Self>) {
        if streams::best(&self.streams, Some(1), Some(episode)).is_none() {
            self.inline = from_list;
        }
        self.play(episode, window, cx);
    }

    /// Whether the show is in My List, reread whenever My List changed since (the page is kept
    /// for Back). A toggle being saved has the newer answer, so a read that lands then is dropped.
    fn refresh_in_list(&mut self, cx: &mut Context<Self>) {
        let epoch = list_epoch();
        if self.list_seen == Some(epoch) {
            return;
        }
        self.list_seen = Some(epoch);
        fetch(cx, in_watchlist(media_id("anime", &self.mal_id)), move |v: &mut Self, l| {
            if let (Load::Ready(on), false, true) = (l, v.saving, v.list_seen == Some(epoch)) {
                v.in_list = Some(on);
            }
        });
    }

    /// Reload which episodes are finished, and the resume point, when the player saved progress
    /// or History changed since.
    fn refresh_watched(&mut self, cx: &mut Context<Self>) {
        let epoch = progress_stamp();
        if self.watch_epoch == Some(epoch) {
            return;
        }
        self.watch_epoch = Some(epoch);
        fetch(cx, progress_of("anime", media_id("anime", &self.mal_id)), move |v: &mut Self, l| {
            if let Load::Ready((watched, resume)) = l {
                v.watched = watched;
                v.resume = resume;
                v.progress_epoch = Some(epoch);
            }
            v.resume_loaded = true;
        });
    }

    /// A listed show the user has finished moves to Completed in My List: a movie watched to the
    /// end, or a series that has finished airing with every episode watched. Checked once per
    /// player save, after the saved progress is in; adding an already finished show is not a
    /// finish (see `completion_due`).
    fn check_completed(&mut self, cx: &mut Context<Self>) {
        if !completion_due(self.in_list, self.progress_epoch, &mut self.checked_epoch) {
            return;
        }
        let Some(d) = self.details.ready() else { return };
        self.checked_epoch = self.progress_epoch;
        let done = if d.anime_type.as_deref() == Some("Movie") {
            self.resume.as_ref().is_some_and(|h| h.completed)
        } else {
            let finished = d.status.as_deref().is_some_and(|s| s.to_lowercase().contains("finished"));
            finished && d.episode_count.is_some_and(|n| all_watched(&self.watched, n))
        };
        if done {
            let id = media_id("anime", &self.mal_id);
            cx.spawn(async move |_, cx| {
                if let Ok(Some(before)) = mark_completed(id).await {
                    cx.update(|cx| announce_completed(cx, before));
                }
            })
            .detach();
        }
    }

    /// Find links for an episode and list them on the page.
    fn lookup(&mut self, episode: u32, autoplay: bool, window: &mut Window, cx: &mut Context<Self>) {
        let movie = self.is_movie();
        self.sel = Some(episode);
        self.addon_filter = None;
        last_selection().lock().unwrap_or_else(|e| e.into_inner()).insert(self.mal_id.clone(), (episode, self.inline));
        let name = self.display_title();
        let label = if movie { name.clone() } else { format!("{name}, episode {episode}") };
        self.streams = StreamState::Searching(label.clone());
        cx.notify();
        let base = PlayBase {
            title: if movie { name } else { format!("{name} - Episode {episode}") },
            imdb_id: None,
            mal_id: Some(self.mal_id.clone()),
            season: Some(1),
            episode: Some(episode),
        };
        self.lookup = Some(streams::find(cx, window, label, base, movie, autoplay, |v: &mut Self, s| v.streams = s));
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading_more {
            return;
        }
        self.page += 1;
        self.has_more = false;
        self.loading_more = true;
        cx.notify();
        self.load_page(self.page, false, cx);
    }

    /// Adds or removes the show; the toast offers Undo (Material 3 snackbar), which puts a removed
    /// show back with the status it had.
    fn toggle_list(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.in_list.filter(|_| !self.saving) else { return };
        let fallback = AnimeSearchResult { title: self.title.clone(), ..Default::default() };
        let t = media_title("anime", &self.mal_id, self.details.ready().unwrap_or(&fallback), &self.title);
        self.saving = true;
        cx.notify();
        let undo_title = t.clone();
        cx.spawn(async move |this, cx| {
            let r = set_watchlist(t, !cur).await;
            let _ = this.update(cx, |v, cx| {
                v.saving = false;
                match r {
                    Ok(before) => {
                        let now = !cur;
                        v.in_list = Some(now);
                        announce_list_toggle(cx, now, before, undo_title, |v: &mut Self, on| v.in_list = Some(on));
                    }
                    Err(e) => {
                        v.in_list = Some(cur);
                        toast::show(cx, Toast::error(format!("My List could not be updated: {e}")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn hero(&self, width: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let height = 480.;
        let movie = self.is_movie();
        let Load::Ready(d) = &self.details else {
            let copy = match &self.details {
                Load::Failed(e) => look::hero_copy(70.)
                    .child(div().ramp(ramp::TITLE_LARGE).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).line_clamp(2).child(SharedString::from(self.title.clone())))
                    .child(error_view(e.clone()).title("This title could not be loaded").compact().retry(cx.listener(|this, _, _, cx| this.reload_details(cx))).items_start()),
                _ => look::hero_copy(70.).child(look::hero_skeleton()),
            };
            return look::billboard("anime-detail-hero", height, None, None).child(copy);
        };
        let tags = [d.anime_type.clone(), d.status.clone(), d.episode_count.filter(|_| !movie).map(|n| format!("{n} episodes")), d.duration_minutes.map(|m| format!("{m} min"))];
        let quality = streams::quality_tags(&self.streams);
        let next = self.next();
        let panel = if self.inline { None } else { self.panel(cx) };
        let full = d.description.clone().unwrap_or_default();
        let long = !full.is_empty() && (full.len() > look::blurb(&full).len() + 40 || full.len() > 260);
        let open = self.synopsis_open && long;
        // The hero copy is anchored to the bottom of a 480px banner: a 200px synopsis pushed a
        // two-line title out of the top and clipped it. Open, the text scrolls in 120px and the
        // title keeps one line.
        let synopsis = if open {
            div()
                .id("synopsis")
                .max_h(px(120.))
                .overflow_y_scroll()
                .text_size(px(size::MD))
                .line_height(px(22.))
                .text_color(Tokens::text_1())
                .child(SharedString::from(full.clone()))
                .into_any_element()
        } else {
            div()
                .text_size(px(size::LG))
                .line_height(px(22.))
                .text_color(Tokens::text_1())
                .line_clamp(3)
                .child(SharedString::from(if full.is_empty() { "No synopsis available.".to_string() } else { look::blurb(&full) }))
                .into_any_element()
        };
        let ep = next.episode;
        look::billboard("anime-detail-hero", height, d.banner_url.clone().or_else(|| d.cover_url.clone()), None)
            .child(
                look::hero_copy(56.)
                    .w(px(640.))
                    .when(!self.inline && !matches!(self.streams, StreamState::Idle), |d| d.max_w(look::beside_panel(width)))
                    .child(div().ramp(ramp::TITLE_LARGE).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).line_clamp(if open { 1 } else { 2 }).child(SharedString::from(d.title.clone())))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .children(look::score(d.score, isz::MD, None).map(|s| div().mr(px(4.)).child(s)))
                            .children(tags.into_iter().flatten().map(look::meta_tag))
                            .children(quality.into_iter().map(look::quality_tag)),
                    )
                    .child(div().text_size(px(size::MD)).text_color(Tokens::text_2()).child(SharedString::from(d.genres.clone().unwrap_or_default().join(", "))))
                    .child(synopsis)
                    .when(long, |c| {
                        c.child(
                            div().child(
                                btn("synopsis-more", if open { "Show less" } else { "More details" }, ButtonKind::Ghost)
                                    .small()
                                    .trailing_icon(if open { ph::CARET_UP } else { ph::CARET_DOWN })
                                    .ml(px(-10.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.synopsis_open = !this.synopsis_open;
                                        cx.notify();
                                    })),
                            ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .mt(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(10.))
                                    .child(look::hero_play("play-main", next.label.clone()).on_click(cx.listener(move |this, _, w, cx| this.play_at(ep, false, w, cx))))
                                    .child(look::list_button("toggle-list", self.in_list == Some(true), self.saving).on_click(cx.listener(|this, _, _, cx| this.toggle_list(cx)))),
                            )
                            .when_some(next.progress.clone(), |c, (p, left)| c.child(look::resume_note("resume-bar", p, left))),
                    ),
            )
            .map(|hero| match panel {
                Some(p) => hero.child(div().absolute().right(px(EDGE)).top(px(24.)).bottom(px(24.)).w(px(look::SIDE_PANEL)).child(p)),
                None => hero.child(
                    div()
                        .absolute()
                        .right(px(EDGE))
                        .bottom(px(56.))
                        .w(px(220.))
                        .h(px(330.))
                        .rounded(px(radius::SM))
                        .overflow_hidden()
                        .border_1()
                        .border_color(Tokens::border_strong())
                        .shadow_lg()
                        .bg(Tokens::surface_2())
                        .when_some(d.cover_url.clone(), |x, u| x.child(look::pic(u))),
                ),
            })
    }

    /// Links for the selected episode (`None` when nothing is selected).
    fn panel(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let heading = match (self.is_movie(), self.sel) {
            (false, Some(e)) => {
                let name = self.episodes.ready().and_then(|v| v.iter().find(|x| x.number == e)).and_then(|x| x.title.clone()).filter(|t| !t.is_empty());
                format!("Episode {e}{}", name.map(|n| format!(": {n}")).unwrap_or_default())
            }
            _ => "Sources".to_string(),
        };
        let close: Option<fn(&mut Self)> = if self.inline { Some(close_inline) } else { None };
        streams::sources_panel(&self.streams, heading, self.addon_filter.as_deref(), cx, |v: &mut Self, f| v.addon_filter = f, close, retry_sources)
    }

    /// Studio / next episode facts under the hero.
    fn facts(&self) -> Option<Div> {
        let d = self.details.ready()?;
        let mut items: Vec<(Ph, &str, String)> = vec![];
        if let Some(s) = d.studios.as_ref().filter(|s| !s.is_empty()) {
            items.push((ph::FILM_SLATE, "Studio", s.join(", ")));
        }
        if let (Some(n), Some(at)) = (d.next_episode, d.next_episode_at) {
            items.push((ph::CALENDAR_DOTS, "Next episode", format!("Episode {n} airs {}", airs_in(at))));
        }
        if let Some(y) = d.year {
            items.push((ph::CALENDAR_BLANK, "Year", y.to_string()));
        }
        if let Some(c) = d.cast.as_ref().filter(|c| !c.is_empty()) {
            items.push((ph::MICROPHONE_STAGE, "Voices", c.iter().take(4).cloned().collect::<Vec<_>>().join(", ")));
        }
        if items.is_empty() {
            return None;
        }
        Some(div().flex().flex_wrap().gap_x(px(40.)).gap_y(px(12.)).px(px(EDGE)).mb(px(32.)).children(items.into_iter().map(|(g, k, v)| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().flex().items_center().gap(px(6.)).text_size(px(size::XS)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_3()).child(icon(g.bold(), Tokens::text_3(), isz::XS)).child(k.to_uppercase()))
                .child(div().text_size(px(size::MD)).text_color(Tokens::text_1()).child(SharedString::from(v)))
        })))
    }

    /// One episode card. Click plays it (the hover shows a play glyph); the corner button opens its
    /// sources under the row. Finished episodes are dimmed with a "Watched" mark, the one in
    /// progress shows how far you got.
    fn episode_card(&self, ep: &AnimeEpisode, w: f32, h: f32, fallback_img: Option<String>, cx: &mut Context<Self>) -> Stateful<Div> {
        let n = ep.number;
        let group: SharedString = format!("aep-{}-{n}", self.mal_id).into();
        let title = ep.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| format!("Episode {n}"));
        let watched = self.watched.contains(&n);
        let selected = self.inline && self.sel == Some(n);
        let progress = self.resume.as_ref().filter(|r| r.episode_number == Some(n as i64) && in_progress(r)).map(fraction);
        let sources = look::glass_toggle(("ep-sources", n as usize), ph::LIST, selected, if selected { "Hide sources" } else { "Choose source" }, 28.)
            .when(!selected, |b| b.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)))
            // Visible when reached with Tab, not only on hover.
            .focus_visible(|s| s.opacity(1.).shadow(focus_ring_shadow()))
            .on_click(cx.listener(move |this, _, w, cx| {
                // The card under the button would start playback.
                cx.stop_propagation();
                if this.inline && this.sel == Some(n) {
                    close_inline(this);
                    cx.notify();
                    return;
                }
                this.inline = true;
                this.lookup(n, false, w, cx);
            }));
        div()
            .id(("ep", n as usize))
            .group(group.clone())
            .w(px(w))
            .flex()
            .flex_col()
            .gap(px(8.))
            .rounded(px(radius::SM))
            .cursor_pointer()
            .role(Role::Button)
            .aria_label(SharedString::from(format!("Play episode {n}: {title}")))
            .focus_ring()
            .child(
                div()
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .rounded(px(radius::SM))
                    .overflow_hidden()
                    .bg(Tokens::surface_2())
                    .border_2()
                    .border_color(if selected { Tokens::accent() } else { Tokens::base().alpha(0.) })
                    .when_some(ep.thumbnail.clone().or(fallback_img), |d, u| d.child(look::pic(u)))
                    .when(watched, |d| d.child(div().absolute().inset_0().bg(Tokens::scrim())))
                    .when(watched, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(px(6.))
                                .top(px(6.))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(radius::XS))
                                .bg(Tokens::glass())
                                .text_size(px(size::XS))
                                .text_color(Tokens::text_1())
                                .child(icon(ph::CHECK.bold(), Tokens::text_1(), isz::XS))
                                .child("Watched"),
                        )
                    })
                    .child(
                        div().absolute().inset_0().flex().items_center().justify_center().bg(Tokens::overlay()).opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)).child(
                            div().size(px(48.)).rounded_full().bg(Tokens::accent()).flex().items_center().justify_center().shadow_lg().child(icon(ph::PLAY.fill(), Tokens::on_accent(), isz::LG)),
                        ),
                    )
                    .when(watched, |d| d.child(div().absolute().left_0().bottom_0().w_full().h(px(3.)).bg(Tokens::accent())))
                    .when_some(progress, |d, p| d.child(div().absolute().left_0().right_0().bottom_0().h(px(3.)).bg(Tokens::text_1().alpha(0.25)).child(div().h_full().w(relative(p.max(0.02))).bg(Tokens::accent()))))
                    .child(div().absolute().right(px(6.)).bottom(px(6.)).px(px(5.)).rounded(px(radius::XS)).bg(Tokens::glass()).text_size(px(size::XS)).text_color(Tokens::text_1()).child(SharedString::from(format!("E{n}"))))
                    .child(div().absolute().top(px(6.)).right(px(6.)).child(sources)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(SharedString::from(format!("Episode {n}"))))
                    .child(
                        div()
                            .text_size(px(size::MD))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if watched { Tokens::text_3() } else { Tokens::text_1() })
                            .line_clamp(2)
                            .group_hover(group, |s| s.text_color(Tokens::accent()))
                            .child(SharedString::from(title)),
                    ),
            )
            .on_click(cx.listener(move |this, _, w, cx| this.play_at(n, true, w, cx)))
    }

    fn episodes(&self, width: f32, cx: &mut Context<Self>) -> Div {
        let per = look::per_row(width, 260., 16.);
        let w = ((width - EDGE * 2.0 - 16.0 * (per as f32 - 1.0)) / per as f32).clamp(220., 340.);
        let h = w * 9. / 16.;
        let fallback_img = self.details.ready().and_then(|d| d.banner_url.clone().or_else(|| d.cover_url.clone()));
        let count = self.episodes.ready().map(|v| v.len()).unwrap_or(0);
        let body = match &self.episodes {
            Load::Loading => div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(24.)).children((0..8).map(|_| div().flex().flex_col().gap(px(8.)).child(skeleton(px(w), px(h), radius::SM)).child(skeleton_line(px(w * 0.3))).child(skeleton_line(px(w * 0.7))))),
            Load::Failed(e) => div().child(error_view(e.clone()).title("Episodes could not be loaded").compact().retry(cx.listener(|this, _, _, cx| {
                this.episodes = Load::Loading;
                this.load_page(this.first_page, true, cx);
                cx.notify();
            }))),
            Load::Ready(v) if v.is_empty() && self.first_page > 1 => div().child(empty_view(ph::PLAYLIST, "No episodes in this range yet", "Pick an earlier range above.").compact()),
            Load::Ready(v) if v.is_empty() => div().child(empty_view(ph::PLAYLIST, "No episode list yet", "Use the play button above to look for sources anyway.").compact()),
            Load::Ready(v) => {
                let mut order: Vec<&AnimeEpisode> = v.iter().collect();
                if self.newest_first {
                    order.reverse();
                }
                // The links open right under the row holding the selected episode.
                let open_after = self.sel.filter(|_| self.inline).and_then(|n| order.iter().position(|e| e.number == n)).map(|i| ((i / per + 1) * per).min(order.len()) - 1);
                let mut grid = div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(24.));
                for (i, ep) in order.iter().enumerate() {
                    grid = grid.child(self.episode_card(ep, w, h, fallback_img.clone(), cx));
                    if open_after == Some(i) {
                        if let Some(p) = self.panel(cx) {
                            grid = grid.child(div().w(px((width - EDGE * 2.0).max(320.))).h(px(440.)).child(p));
                        }
                    }
                }
                grid
            }
        };
        let weak = cx.weak_entity();
        let sort_item = |label: &'static str, newest: bool| {
            let weak = weak.clone();
            MenuItem::new(label, move |_, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.newest_first = newest;
                    cx.notify();
                });
            })
            .checked(self.newest_first == newest)
        };
        // Newest first needs the whole list, or it would start at the last loaded episode.
        let sort = menu_button("ep-sort", if self.newest_first { ph::SORT_DESCENDING } else { ph::SORT_ASCENDING }, "Sort episodes", vec![
            MenuItem::heading("Sort episodes"),
            sort_item("Oldest first", false),
            sort_item("Newest first", true).disabled(self.has_more || self.first_page > 1),
        ]);
        // Long shows get Crunchyroll-style ranges of 100, so episode 900 is one click away.
        let total = self.episode_total();
        let pages = self.range_pages();
        let page_size = EPISODE_PAGE as u32;
        let span = |p: u32| {
            let start = (p - 1) * page_size + 1;
            let end = total.map_or(p * page_size, |t| t.min(p * page_size)).max(start);
            format!("{start}–{end}")
        };
        let range = (pages > 1).then(|| {
            let shown_start = (self.first_page - 1) * page_size + 1;
            let shown_end = self.episodes.ready().and_then(|v| v.last()).map_or(self.first_page * page_size, |e| e.number);
            let items = (1..=pages)
                .map(|p| {
                    let weak = weak.clone();
                    MenuItem::new(format!("Episodes {}", span(p)), move |_, cx| {
                        let _ = weak.update(cx, |v, cx| v.jump_to(p, cx));
                    })
                    .checked(p == self.first_page)
                })
                .collect();
            menu_button("ep-range", ph::LIST_NUMBERS, "Jump to episodes", items).label(format!("Episodes {shown_start}–{}", shown_end.max(shown_start)))
        });
        let count_label = match total {
            Some(t) => Some(t.max(count as u32).to_string()),
            None => (count > 0).then(|| format!("{count}{}", if self.has_more { "+" } else { "" })),
        };
        div()
            .px(px(EDGE))
            .mb(px(40.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mb(px(16.))
                    .pb(px(10.))
                    .border_b_1()
                    .border_color(Tokens::border())
                    .child(look::media_section("Episodes", ph::PLAYLIST, count_label))
                    .child(div().flex_1())
                    .children(range)
                    .when(count > 1, |d| d.child(sort)),
            )
            .child(body)
            .when(self.has_more || self.loading_more, |d| {
                d.child(
                    div()
                        .mt(px(20.))
                        .flex()
                        .justify_center()
                        .child(btn("more-eps", "Load more episodes", ButtonKind::Secondary).icon(ph::CARET_DOWN).loading(self.loading_more).on_click(cx.listener(|this, _, _, cx| this.load_more(cx)))),
                )
            })
    }
}

impl Render for AnimeDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = look::page_width(&self.scroll, window);
        let movie = self.is_movie();
        self.refresh_in_list(cx);
        self.refresh_watched(cx);
        self.check_completed(cx);
        // Links load by themselves once we know the first episode (or that it is a movie);
        // a title opened before comes back to the episode that was selected then.
        if !self.auto && self.resume_loaded && self.details.ready().is_some() {
            let saved = last_selection().lock().unwrap_or_else(|e| e.into_inner()).get(&self.mal_id).copied();
            let start = if movie {
                Some((1, false))
            } else {
                self.episodes.ready().map(|eps| match saved {
                    Some((n, inline)) => (n, inline && eps.iter().any(|e| e.number == n)),
                    None => (self.next().episode, false),
                })
            };
            if let Some((ep, inline)) = start {
                self.auto = true;
                self.inline = inline;
                self.lookup(ep, false, window, cx);
            }
        }
        div()
            .relative()
            .size_full()
            .bg(Tokens::base())
            .child(mv_ui::scroll_page("anime-detail", &self.scroll).child(self.hero(width, cx)).children(self.facts()).when(!movie, |d| d.child(self.episodes(width, cx))))
    }
}

#[cfg(test)]
mod tests {
    use super::{more_after, next_up, NextUp};
    use mv_core::repo::media::PlaybackState;

    #[test]
    fn no_continue_past_the_last_episode() {
        assert!(!more_after(Some(220), Some(220)));
        assert!(more_after(Some(219), Some(220)));
        assert!(more_after(Some(5), None));
        assert!(more_after(None, Some(220)));
    }

    fn at(ep: i64, pos: f64, dur: f64, completed: bool) -> PlaybackState {
        PlaybackState {
            media_id: "anime:mal:1".into(),
            kind: "anime".into(),
            title: "Show".into(),
            cover_url: None,
            provider_id: None,
            episode_number: Some(ep),
            position_secs: pos,
            duration_secs: dur,
            completed,
            updated_at: 0,
        }
    }

    #[test]
    fn hero_starts_resumes_or_continues() {
        let eps = [1, 2, 3];
        assert_eq!(next_up(false, None, &eps, false), NextUp { label: "Start watching E1".into(), episode: 1, progress: None });
        let r = next_up(false, Some(&at(2, 600., 1440., false)), &eps, false);
        assert_eq!((r.label.as_str(), r.episode), ("Resume E2", 2));
        assert!(r.progress.is_some());
        assert_eq!(next_up(false, Some(&at(2, 1440., 1440., true)), &eps, false).label, "Continue E3");
        assert_eq!(next_up(false, Some(&at(3, 1440., 1440., true)), &eps, false).label, "Watch again E1");
        assert_eq!(next_up(false, Some(&at(3, 1440., 1440., true)), &eps, true).label, "Continue E4");
        assert_eq!(next_up(true, Some(&at(1, 5000., 5000., true)), &[], false).label, "Watch again");
    }

    #[test]
    fn hero_ignores_a_later_range_on_screen() {
        // Range 101-200 is shown: the hero still starts at E1 or continues from the resume point.
        assert_eq!(next_up(false, None, &[], true).label, "Start watching E1");
        assert_eq!(next_up(false, Some(&at(50, 1440., 1440., true)), &[], true).label, "Continue E51");
    }
}
