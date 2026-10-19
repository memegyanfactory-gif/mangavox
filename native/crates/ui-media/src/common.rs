//! Shared helpers for the media pages: async loading, history/watchlist glue and the poster grid.
use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;
use mv_core::cache::DataCache;
use mv_core::repo::media::{self, MediaEvent, MediaTitle, PlaybackState};
use mv_core::repo::watchlist::{self, WatchlistEntry};
use mv_media::addons::AddonRegistry;
use mv_media::image_cache::ImageCache;
use mv_media::jikan::JikanClient;
use mv_media::types::AnimeSearchResult;
use mv_ui::theme::icon as isz;
use mv_ui::toast::{self, Toast};
use mv_ui::{bg, glyphs, icon, ph, CoverSource, Route, Tokens};

/// State of one async-loaded block.
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(v) => Some(v),
            _ => None,
        }
    }
}

impl<T> From<Result<T, String>> for Load<T> {
    fn from(r: Result<T, String>) -> Self {
        match r {
            Ok(v) => Load::Ready(v),
            Err(e) => Load::Failed(e),
        }
    }
}

/// Run `fut` on the background runtime, then hand the outcome to `apply` on the UI thread.
pub fn fetch<V: 'static, T: Send + 'static>(
    cx: &mut Context<V>,
    fut: impl Future<Output = Result<T, String>> + Send + 'static,
    apply: impl FnOnce(&mut V, Load<T>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let res = match bg::run(fut).await {
            Ok(r) => r,
            Err(e) => Err(e.to_string()),
        };
        let _ = this.update(cx, |v, cx| {
            apply(v, res.into());
            cx.notify();
        });
    })
    .detach();
}

/// Catalogs and details refresh once a day; stream links more often.
pub const DAY: Duration = Duration::from_secs(24 * 3600);
pub const EPISODES_TTL: Duration = Duration::from_secs(12 * 3600);
pub const STREAMS_TTL: Duration = Duration::from_secs(8 * 3600);

/// Run `fut` and store a successful result under `key`.
pub async fn cached<T: serde::Serialize + Send + 'static>(key: String, fut: impl Future<Output = Result<T, String>> + Send + 'static) -> Result<T, String> {
    let r = fut.await;
    if let Ok(v) = &r {
        DataCache::shared().put(&key, v);
    }
    r
}

/// Stale-while-revalidate `fetch`. Returns the cached value (however old) for the page to show at
/// once; when there is none or it is older than `ttl`, `fut` runs in the background and its result
/// is cached and handed to `apply`. A failed refresh keeps the cached value.
pub fn fetch_cached<V: 'static, T: serde::Serialize + serde::de::DeserializeOwned + Send + 'static>(
    cx: &mut Context<V>,
    key: String,
    ttl: Duration,
    fut: impl Future<Output = Result<T, String>> + Send + 'static,
    apply: impl FnOnce(&mut V, Load<T>) + 'static,
) -> Load<T> {
    let hit = DataCache::shared().get::<T>(&key);
    let had = hit.is_some();
    let log = std::env::var_os("MANGAVOX_CACHE_LOG").is_some();
    if log {
        eprintln!("[cache] {key}: {}", hit.as_ref().map_or("miss".into(), |(_, a)| format!("hit, {} min old", a.as_secs() / 60)));
    }
    if hit.as_ref().is_none_or(|(_, age)| *age >= ttl) {
        let started = std::time::Instant::now();
        fetch(cx, cached(key.clone(), fut), move |v, l| {
            if log {
                eprintln!("[cache] {key}: fetched in {} ms", started.elapsed().as_millis());
            }
            if !(had && matches!(l, Load::Failed(_))) {
                apply(v, l)
            }
        });
    }
    hit.map_or(Load::Loading, |(v, _)| Load::Ready(v))
}

/// Shared artwork cache (`~/.mangavox/cache/images`).
pub fn images() -> &'static Arc<ImageCache> {
    static C: OnceLock<Arc<ImageCache>> = OnceLock::new();
    C.get_or_init(|| Arc::new(ImageCache::new(mv_core::paths::root().join("cache").join("images"), 6)))
}

/// Cached file for a remote image when this session found one on disk; otherwise `None` (show the
/// URL) and the file is downloaded in the background for next time.
pub fn local_image(url: &str) -> Option<PathBuf> {
    let c = images();
    let (file, first) = c.resolve(url);
    match &file {
        Some(p) if first => {
            let p = p.clone();
            bg::runtime().spawn_blocking(move || ImageCache::touch(&p));
        }
        None if c.claim(url) => {
            let (c, u) = (c.clone(), url.to_string());
            bg::runtime().spawn(async move { c.download(u).await });
        }
        _ => {}
    }
    file
}

pub fn image_source(url: &str) -> ImageSource {
    local_image(url).map_or_else(|| url.to_string().into(), Into::into)
}

/// Blocking SQLite work against the user's database, off the UI thread.
pub fn db<T: Send + 'static>(f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<T> + Send + 'static) -> impl Future<Output = Result<T, String>> + Send {
    let fut = bg::blocking(move || {
        let conn = mv_core::db::open_default()?;
        f(&conn)
    });
    async move { fut.await.map_err(|e| e.to_string())?.map_err(|e| e.to_string()) }
}

pub fn jikan() -> Result<&'static JikanClient, String> {
    JikanClient::shared().map_err(|e| e.to_string())
}

/// Process-wide installed-addon registry (`~/.mangavox/addons`).
pub fn registry() -> &'static Arc<AddonRegistry> {
    static REG: OnceLock<Arc<AddonRegistry>> = OnceLock::new();
    REG.get_or_init(|| Arc::new(AddonRegistry::open(mv_core::paths::root().join("addons"))))
}

pub fn input(window: &mut Window, cx: &mut App, placeholder: &'static str) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

/// Search field with the leading magnifier (Material 3 search bar anatomy) and the clear X.
pub fn search_field(state: &Entity<InputState>) -> Input {
    Input::new(state).cleanable(true).prefix(icon(glyphs::SEARCH.bold(), Tokens::text_3(), isz::SM))
}

/// Debounce for search-as-you-type.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

// ---------- cards ----------

pub fn cover_of(url: &Option<String>) -> Option<CoverSource> {
    let u = url.as_ref().filter(|u| !u.is_empty())?;
    Some(local_image(u).map_or_else(|| CoverSource::Url(u.clone().into()), CoverSource::File))
}

/// Jikan results (numeric id) open AnimeDetail; Cinemeta/IMDb results open MovieDetail.
pub fn result_route(r: &AnimeSearchResult) -> Route {
    let numeric = !r.id.is_empty() && r.id.bytes().all(|b| b.is_ascii_digit());
    if numeric || !r.id.starts_with("tt") {
        Route::AnimeDetail { mal_id: r.id.clone(), title: r.title.clone() }
    } else {
        let kind = if r.anime_type.as_deref() == Some("Movie") { "movie" } else { "series" };
        Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_else(|| r.id.clone()), kind: kind.into(), title: r.title.clone() }
    }
}

/// "TV · 24 eps · 2023" under a poster.
pub fn subline(r: &AnimeSearchResult) -> String {
    [r.anime_type.clone(), r.episode_count.filter(|n| *n > 1).map(|n| format!("{n} eps")), r.year.map(|y| y.to_string())].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

// ---------- history / watchlist glue ----------

/// Media id used by the history and watchlist tables (same scheme as the Electron app).
pub fn media_id(kind: &str, id: &str) -> String {
    if kind == "anime" {
        format!("anime:mal:{id}")
    } else {
        format!("{kind}:imdb:{id}")
    }
}

pub fn route_for_media(media_id: &str, title: &str) -> Option<Route> {
    let mut p = media_id.splitn(3, ':');
    match (p.next(), p.next(), p.next()) {
        (Some("anime"), Some("mal"), Some(id)) => Some(Route::AnimeDetail { mal_id: id.into(), title: title.into() }),
        (Some(k @ ("movie" | "series")), Some("imdb"), Some(id)) => {
            Some(Route::MovieDetail { imdb_id: id.into(), kind: k.into(), title: title.into() })
        }
        _ => None,
    }
}

pub fn media_title(kind: &str, id: &str, r: &AnimeSearchResult, fallback_title: &str) -> MediaTitle {
    MediaTitle {
        id: media_id(kind, id),
        kind: kind.into(),
        title: if r.title.is_empty() { fallback_title.into() } else { r.title.clone() },
        imdb_id: (kind != "anime").then(|| id.to_string()),
        mal_id: if kind == "anime" { id.parse().ok() } else { None },
        cover_url: r.cover_url.clone(),
        banner_url: r.banner_url.clone(),
        year: r.year.map(i64::from),
        genres: r.genres.clone().unwrap_or_default(),
        description: r.description.clone(),
        score: r.score,
        ..Default::default()
    }
}

/// Record an `open` event (keeps the title row fresh so history/continue-watching can join it).
pub fn record_open(title: MediaTitle) -> impl Future<Output = Result<(), String>> + Send {
    db(move |c| {
        let e = MediaEvent { media_id: Some(title.id.clone()), kind: title.kind.clone(), event_type: "open".into(), title: Some(title), ..Default::default() };
        media::record_event(c, &e).map(|_| ())
    })
}

pub fn in_watchlist(media_id: String) -> impl Future<Output = Result<bool, String>> + Send {
    db(move |c| Ok(watchlist::list(c, None)?.iter().any(|e| e.media_id == media_id)))
}

static LIST_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Bumped after every My List write from these pages. The Movies home is kept alive between
/// visits, so it compares this on render and reloads its "My List" row when it moved.
pub fn list_epoch() -> u64 {
    LIST_EPOCH.load(Ordering::Relaxed)
}

/// Records that My List changed (call after the write).
pub fn list_changed() {
    LIST_EPOCH.fetch_add(1, Ordering::Relaxed);
}

static HISTORY_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Records that History removed, cleared or wrote back entries (call after the write).
pub fn history_changed() {
    HISTORY_EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// Changes whenever saved progress may have: the player saved, or History removed, cleared or
/// restored entries. Pages that stay alive (the homes, detail pages kept for Back) reread their
/// progress when it moved; before, a cleared History still showed in Continue Watching.
pub fn progress_stamp() -> (u64, u64) {
    (ui_player::watch_epoch(), HISTORY_EPOCH.load(Ordering::Relaxed))
}

/// Adds a title to My List (as Planned) or removes it. Returns the entry as it was before, so the
/// Undo of a removal can put back the status it had (Watching, Completed...) instead of starting
/// over at Planned. Adding a title that is already listed keeps its status.
pub fn set_watchlist(title: MediaTitle, add: bool) -> impl Future<Output = Result<Option<WatchlistEntry>, String>> + Send {
    db(move |c| {
        let before = watchlist::list(c, None)?.into_iter().find(|e| e.media_id == title.id);
        if add {
            media::upsert_title(c, &title)?;
            if before.is_none() {
                watchlist::set_status(c, &title.id, &title.kind, "planned")?;
            }
        } else {
            watchlist::remove_status(c, &title.id)?;
        }
        list_changed();
        Ok(before)
    })
}

/// The toast after the My List button on a detail page, with Undo (Material 3 snackbar). Undoing
/// a removal writes the entry back as it was; undoing an add removes the title again. Works after
/// leaving the page too; while the page is open, `apply` gets the restored in-list state.
pub fn announce_list_toggle<V: 'static>(cx: &mut Context<V>, now: bool, before: Option<WatchlistEntry>, title: MediaTitle, apply: fn(&mut V, bool)) {
    let weak = cx.weak_entity();
    let restore = before.filter(|_| !now);
    toast::show(
        cx,
        Toast::success(if now { "Added to My List" } else { "Removed from My List" }).icon(glyphs::SAVED).action("Undo", move |_, cx| {
            let (weak, restore, title) = (weak.clone(), restore.clone(), title.clone());
            cx.spawn(async move |cx| {
                let r = match restore {
                    Some(e) => restore_status(e).await.map(|_| true),
                    None => set_watchlist(title, !now).await.map(|_| !now),
                };
                match r {
                    Ok(state) => {
                        let _ = weak.update(cx, |v, cx| {
                            apply(v, state);
                            cx.notify();
                        });
                    }
                    Err(e) => cx.update(|cx| toast::show(cx, Toast::error(format!("My List could not be updated: {e}")))),
                }
            })
            .detach();
        }),
    );
}

/// Moves a listed title to `to` when its status is one of `from`; anything the user picked by
/// hand (On hold, Dropped) is left alone. Returns the entry as it was before, when it changed.
fn promote(c: &rusqlite::Connection, media_id: &str, from: &[&str], to: &str) -> anyhow::Result<Option<WatchlistEntry>> {
    let Some(e) = watchlist::list(c, None)?.into_iter().find(|e| e.media_id == media_id) else { return Ok(None) };
    if !from.contains(&e.status.as_str()) {
        return Ok(None);
    }
    watchlist::set_status(c, &e.media_id, &e.kind, to)?;
    list_changed();
    Ok(Some(e))
}

/// A listed title the user starts playing moves from Planned to Watching, so My List's status
/// filters fill themselves in.
pub fn mark_watching(media_id: String) -> impl Future<Output = Result<(), String>> + Send {
    db(move |c| promote(c, &media_id, &["planned"], "watching").map(|_| ()))
}

/// A listed title the user has finished moves to Completed. Done once per title: after that the
/// user's own choice wins (a rewatch set back to Watching is not flipped again). Returns the entry
/// as it was, for Undo.
pub fn mark_completed(media_id: String) -> impl Future<Output = Result<Option<WatchlistEntry>, String>> + Send {
    db(move |c| {
        let key = format!("media:auto-completed:{media_id}");
        if DataCache::shared().age(&key).is_some() {
            // Touch the marker so the cache's 30-day pruning does not forget it.
            DataCache::shared().put(&key, &true);
            return Ok(None);
        }
        let before = promote(c, &media_id, &["planned", "watching"], "completed")?;
        if before.is_some() {
            DataCache::shared().put(&key, &true);
        }
        Ok(before)
    })
}

/// Puts a My List status back (Undo of an automatic change).
pub fn restore_status(e: WatchlistEntry) -> impl Future<Output = Result<(), String>> + Send {
    db(move |c| {
        watchlist::set_status(c, &e.media_id, &e.kind, &e.status)?;
        list_changed();
        Ok(())
    })
}

/// After an automatic move to Completed: say so, with Undo (the change was not the user's own).
pub fn announce_completed(cx: &mut App, before: WatchlistEntry) {
    toast::show(
        cx,
        Toast::success("Finished: moved to Completed in My List").icon(ph::CHECK_CIRCLE).action("Undo", move |_, cx| {
            cx.background_spawn(restore_status(before.clone())).detach();
        }),
    );
}

/// Every episode `1..=total` is in `watched`.
pub fn all_watched(watched: &HashSet<u32>, total: u32) -> bool {
    total > 0 && (1..=total).all(|n| watched.contains(&n))
}

/// Whether a listed title is finished enough to move to Completed, given the page's state.
/// Only a title already listed when its progress was read qualifies: adding a show the user
/// finished long ago is a choice to keep it (Planned), not a finish, so a title found unlisted
/// marks that progress as checked. `None` while the list state is still loading.
pub fn completion_due<E: PartialEq + Copy>(in_list: Option<bool>, progress: Option<E>, checked: &mut Option<E>) -> bool {
    match in_list {
        None => false,
        Some(false) => {
            *checked = progress;
            false
        }
        Some(true) => progress.is_some() && *checked != progress,
    }
}

/// What the player saved for one title, read together so the page sees one consistent state:
/// the finished episode numbers and the resume point.
pub fn progress_of(kind: &'static str, media_id: String) -> impl Future<Output = Result<(HashSet<u32>, Option<PlaybackState>), String>> + Send {
    db(move |c| {
        let watched = media::watched_episodes(c, &media_id)?.into_iter().filter_map(|n| u32::try_from(n).ok()).collect();
        let resume = media::list_history(c, Some(kind), None, 1000)?.into_iter().find(|h| h.media_id == media_id);
        Ok((watched, resume))
    })
}

/// Watched far enough to count as unfinished (more than 30 s in and not completed).
pub fn in_progress(h: &PlaybackState) -> bool {
    !h.completed && h.position_secs > 30.0 && h.duration_secs > 0.0 && h.position_secs < h.duration_secs * 0.95
}

/// Watched fraction for a progress bar.
pub fn fraction(h: &PlaybackState) -> f32 {
    if h.duration_secs > 0.0 {
        (h.position_secs / h.duration_secs).clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's glob exports its own `test` attribute.
    use super::{all_watched, completion_due, fraction, in_progress, media_id, route_for_media, PlaybackState, Route};

    #[test]
    fn adding_a_finished_title_is_not_a_finish() {
        let mut checked = None;
        // Unlisted when the progress came in: that progress counts as checked.
        assert!(!completion_due(Some(false), Some(1), &mut checked));
        assert_eq!(checked, Some(1));
        // Added afterwards: nothing is due until the player saves again.
        assert!(!completion_due(Some(true), Some(1), &mut checked));
        assert!(completion_due(Some(true), Some(2), &mut checked));
        // Still loading whether it is listed: wait.
        let mut fresh = None;
        assert!(!completion_due(None, Some(1), &mut fresh));
        assert_eq!(fresh, None);
        assert!(completion_due(Some(true), Some(1), &mut fresh));
    }

    #[test]
    fn all_watched_needs_every_episode() {
        let seen: std::collections::HashSet<u32> = [1, 2, 3].into_iter().collect();
        assert!(all_watched(&seen, 3));
        assert!(!all_watched(&seen, 4));
        assert!(!all_watched(&seen, 0));
    }

    fn state(pos: f64, dur: f64, completed: bool) -> PlaybackState {
        PlaybackState {
            media_id: String::new(),
            kind: "anime".into(),
            title: String::new(),
            cover_url: None,
            provider_id: None,
            episode_number: Some(1),
            position_secs: pos,
            duration_secs: dur,
            completed,
            updated_at: 0,
        }
    }

    #[test]
    fn media_ids_round_trip_to_routes() {
        assert_eq!(media_id("anime", "52991"), "anime:mal:52991");
        assert!(matches!(route_for_media("anime:mal:52991", "Frieren"), Some(Route::AnimeDetail { .. })));
        assert!(matches!(route_for_media(&media_id("series", "tt0903747"), "BB"), Some(Route::MovieDetail { .. })));
        assert!(route_for_media("manga:x", "").is_none());
    }

    #[test]
    fn progress_needs_a_real_start() {
        assert!(in_progress(&state(600., 1440., false)));
        assert!(!in_progress(&state(10., 1440., false)));
        assert!(!in_progress(&state(1430., 1440., false)));
        assert!(!in_progress(&state(600., 1440., true)));
        assert_eq!(fraction(&state(720., 1440., false)), 0.5);
    }
}
