//! Shared helpers for the media pages: async loading, cards, rails, history/watchlist glue.
use std::future::Future;
use std::sync::{Arc, OnceLock};

use gpui_kit::component::input::InputState;
use gpui_kit::*;
use mv_core::repo::media::{self, MediaEvent, MediaTitle};
use mv_core::repo::watchlist;
use mv_media::addons::AddonRegistry;
use mv_media::jikan::JikanClient;
use mv_media::types::AnimeSearchResult;
use mv_ui::{
    bg, card_grid, cover_card, empty_state, error_state, navigate, size, skeleton_card, CoverSource, Route, Tokens,
};

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

// ---------- cards ----------

pub fn cover_of(url: &Option<String>) -> Option<CoverSource> {
    url.as_ref().filter(|u| !u.is_empty()).map(|u| CoverSource::Url(u.clone().into()))
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

pub fn on_open(card: Stateful<Div>, route: Route) -> Stateful<Div> {
    card.on_click(move |_, w, cx| navigate(route.clone(), w, cx))
}

pub fn result_card(id: (&'static str, usize), r: &AnimeSearchResult) -> Stateful<Div> {
    let sub = [r.year.map(|y| y.to_string()), r.anime_type.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let badge = r.score.filter(|s| *s > 0.0).map(|s| SharedString::from(format!("{s:.1}")));
    on_open(cover_card(id, r.title.clone().into(), sub.into(), cover_of(&r.cover_url), badge), result_route(r))
}

/// Wrapping grid with loading / error / empty states.
pub fn grid(id: &'static str, load: &Load<Vec<AnimeSearchResult>>) -> Div {
    match load {
        Load::Loading => card_grid().children((0..12).map(|_| skeleton_card())),
        Load::Failed(e) => error_state(e.clone()),
        Load::Ready(v) if v.is_empty() => empty_state("No results", "Try a different search."),
        Load::Ready(v) => card_grid().children(v.iter().enumerate().map(|(i, r)| result_card((id, i), r))),
    }
}

pub fn muted(text: impl Into<SharedString>) -> Div {
    div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(text.into())
}

pub fn fmt_size(bytes: u64) -> String {
    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
    }
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

pub fn set_watchlist(title: MediaTitle, add: bool) -> impl Future<Output = Result<bool, String>> + Send {
    db(move |c| {
        if add {
            media::upsert_title(c, &title)?;
            watchlist::set_status(c, &title.id, &title.kind, "planned")?;
        } else {
            watchlist::remove_status(c, &title.id)?;
        }
        Ok(add)
    })
}
