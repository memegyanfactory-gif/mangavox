//! App-control tools the assistant can call (port of src/renderer/assistant/tools.ts).
//! Each tool has a JSON schema and a risk level that is checked against the access mode.
use std::sync::Mutex;

use futures::future::BoxFuture;
use futures::FutureExt;
use mv_ai::{AiAccessMode, AiToolSpec};
use mv_ui::Route;
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    Read,
    Act,
    Destructive,
}

impl Risk {
    pub fn as_str(&self) -> &'static str {
        match self {
            Risk::Read => "read",
            Risk::Act => "act",
            Risk::Destructive => "destructive",
        }
    }
}

/// Ask: everything except reads needs approval. Auto: only destructive. Full: nothing.
pub fn needs_approval(mode: AiAccessMode, risk: Risk) -> bool {
    match mode {
        AiAccessMode::Ask => risk != Risk::Read,
        AiAccessMode::Auto => risk == Risk::Destructive,
        AiAccessMode::Full => false,
    }
}

// ── Page context (the shell may call `set_page_context` on every navigation) ────

static PAGE: Mutex<Option<Route>> = Mutex::new(None);

pub fn set_page_context(route: &Route) {
    *PAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(route.clone());
}

pub fn page_context() -> Option<Route> {
    PAGE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Short stable page name used in prompts and tool results.
pub fn page_name(route: &Route) -> &'static str {
    match route {
        Route::Browse => "browse",
        Route::Library => "library",
        Route::MangaHistory => "history",
        Route::MangaDetail { .. } => "manga-detail",
        Route::Reader { .. } => "reader",
        Route::AnimeHome => "anime",
        Route::AnimeDetail { .. } => "anime-detail",
        Route::MoviesHome => "movies",
        Route::MovieDetail { .. } => "movie-detail",
        Route::Discover => "discover",
        Route::MediaHistory => "media-history",
        Route::Favorites => "favorites",
        Route::Player { .. } => "player",
        Route::Studio => "studio",
        Route::Settings { .. } => "settings",
        Route::Back => "back",
    }
}

pub fn section_name(route: &Route) -> &'static str {
    match route {
        Route::AnimeHome | Route::AnimeDetail { .. } | Route::Discover | Route::MediaHistory | Route::Favorites | Route::Player { .. } => "anime",
        Route::MoviesHome | Route::MovieDetail { .. } => "movies",
        _ => "manga",
    }
}

// ── Tool table ───────────────────────────────────────────────────────────────────

pub const PAGES: &[&str] = &["library", "browse", "history", "anime", "movies", "discover", "favorites", "media-history", "studio", "settings"];
pub const SETTINGS_TABS: &[&str] = &["providers", "models", "profile", "playback", "sources", "extensions", "addons", "streaming", "storage", "themes", "mode", "about"];

fn obj(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn s(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn s_enum(description: &str, values: &[&str]) -> Value {
    json!({ "type": "string", "description": description, "enum": values })
}

pub struct AppTool {
    pub name: &'static str,
    pub description: String,
    pub parameters: Value,
    pub risk: Risk,
}

pub fn app_tools() -> Vec<AppTool> {
    let t = |name, description: String, parameters, risk| AppTool { name, description, parameters, risk };
    vec![
        t("get_app_state", "Current page, media section and library size. Call this when you need to know where the user is.".into(), obj(json!({}), &[]), Risk::Read),
        t("navigate", format!("Open a page of the app. Pages: {}.", PAGES.join(", ")), obj(json!({ "page": s_enum("Page id", PAGES) }), &["page"]), Risk::Act),
        t("go_back", "Go back to the previous page.".into(), obj(json!({}), &[]), Risk::Act),
        t("set_category", "Switch the top-level media section.".into(), obj(json!({ "category": s_enum("Section", &["manga", "anime", "movies"]) }), &["category"]), Risk::Act),
        t(
            "open_settings",
            format!("Open Settings on a tab. Tabs: {} (\"providers\" = AI providers and keys, \"models\" = download local models).", SETTINGS_TABS.join(", ")),
            obj(json!({ "tab": s_enum("Settings tab", SETTINGS_TABS) }), &["tab"]),
            Risk::Act,
        ),
        t("list_themes", "List the available colour themes.".into(), obj(json!({}), &[]), Risk::Read),
        t("set_theme", "Change the app colour theme. Use list_themes for ids.".into(), obj(json!({ "id": s("Theme id") }), &["id"]), Risk::Act),
        t("list_library", "List manga in the user's library, optionally filtered by a text query.".into(), obj(json!({ "query": s("Optional filter") }), &[]), Risk::Read),
        t(
            "get_watch_history",
            "Recently watched anime, movies and series.".into(),
            obj(json!({ "kind": s_enum("Optional filter", &["anime", "movie", "series"]), "limit": { "type": "number", "description": "Max items (default 15)" } }), &[]),
            Risk::Read,
        ),
        t("search_anime", "Search anime by title. Returns ids you can pass to open_anime.".into(), obj(json!({ "query": s("Title or keywords") }), &["query"]), Risk::Read),
        t(
            "open_anime",
            "Open the detail page of an anime. Pass id from search_anime, or just a title.".into(),
            obj(json!({ "id": s("Anime id from search_anime"), "title": s("Title (used if no id)") }), &[]),
            Risk::Act,
        ),
        t("search_movies", "Search movies and TV series by title. Returns imdbIds you can pass to open_movie.".into(), obj(json!({ "query": s("Title or keywords") }), &["query"]), Risk::Read),
        t(
            "open_movie",
            "Open the detail page of a movie or series (from search_movies) so the user can press play.".into(),
            obj(json!({ "imdbId": s("IMDB id from search_movies"), "title": s("Title"), "kind": s_enum("Kind", &["movie", "series"]) }), &["imdbId", "title"]),
            Risk::Act,
        ),
        t("open_manga", "Open a manga that is already in the user's library, by title.".into(), obj(json!({ "title": s("Manga title") }), &["title"]), Risk::Act),
        t(
            "play_title",
            "Open the page of an anime, movie or series so it can be played (searches by title).".into(),
            obj(json!({ "title": s("Title to play"), "category": s_enum("Kind", &["anime", "movie", "series"]) }), &["title"]),
            Risk::Act,
        ),
    ]
}

pub fn tool_specs() -> Vec<AiToolSpec> {
    app_tools().into_iter().map(|t| AiToolSpec { name: t.name.into(), description: t.description, parameters: t.parameters }).collect()
}

pub fn risk_of(name: &str) -> Option<Risk> {
    app_tools().into_iter().find(|t| t.name == name).map(|t| t.risk)
}

fn arg<'a>(args: &'a Map<String, Value>, k: &str) -> &'a str {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

/// Short human label for the step row.
pub fn label(name: &str, args: &Map<String, Value>) -> String {
    let a = |k| arg(args, k);
    match name {
        "get_app_state" => "Checked what is on screen".into(),
        "navigate" => format!("Opened {}", a("page")),
        "go_back" => "Went back".into(),
        "set_category" => format!("Switched to {}", a("category")),
        "open_settings" => format!("Opened Settings → {}", a("tab")),
        "list_themes" => "Listed themes".into(),
        "set_theme" => format!("Changed theme to {}", a("id")),
        "list_library" => "Read your library".into(),
        "get_watch_history" => "Read your watch history".into(),
        "search_anime" => format!("Searched anime for “{}”", a("query")),
        "open_anime" => format!("Opened {}", if a("title").is_empty() { "anime" } else { a("title") }),
        "search_movies" => format!("Searched movies for “{}”", a("query")),
        "open_movie" => format!("Opened {}", a("title")),
        "open_manga" => format!("Opened {}", a("title")),
        "play_title" => format!("Opened {}", a("title")),
        other => format!("Ran {other}"),
    }
}

// ── Execution ─────────────────────────────────────────────────────────────────

/// What a tool produced. `Navigate` asks the UI thread to change page.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutput {
    Value(Value),
    Navigate(Route, Value),
}

pub type ToolResult = Result<ToolOutput, String>;

/// Runs tools; a trait so the agent loop can be tested with a fake.
pub trait ToolRunner: Send + Sync + 'static {
    fn run(&self, name: String, args: Map<String, Value>) -> BoxFuture<'static, ToolResult>;
}

pub struct AppTools;

impl ToolRunner for AppTools {
    fn run(&self, name: String, args: Map<String, Value>) -> BoxFuture<'static, ToolResult> {
        async move { execute(&name, &args).await }.boxed()
    }
}

/// Keep tool results small enough for small local context windows.
pub fn trim_result(value: Value, max: usize) -> Value {
    let text = value.to_string();
    if text.len() <= max {
        return value;
    }
    match value {
        Value::Array(items) => {
            let n = items.len();
            json!({ "items": items.into_iter().take(15).collect::<Vec<_>>(), "note": format!("{n} items, first 15 shown") })
        }
        _ => json!({ "truncated": text.chars().take(max).collect::<String>() }),
    }
}

fn nav(route: Route, v: Value) -> ToolResult {
    Ok(ToolOutput::Navigate(route, v))
}

pub fn route_for_page(page: &str) -> Option<Route> {
    Some(match page {
        "library" => Route::Library,
        "browse" => Route::Browse,
        "history" => Route::MangaHistory,
        "anime" => Route::AnimeHome,
        "movies" => Route::MoviesHome,
        "discover" => Route::Discover,
        "favorites" => Route::Favorites,
        "media-history" => Route::MediaHistory,
        "studio" => Route::Studio,
        "settings" => Route::Settings { tab: None },
        _ => return None,
    })
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())
}

fn compact(a: &mv_media::types::AnimeSearchResult) -> Value {
    json!({
        "id": a.id, "imdbId": a.imdb_id, "title": a.title, "year": a.year, "type": a.anime_type,
        "episodes": a.episode_count, "score": a.score,
        "genres": a.genres.as_ref().map(|g| g.iter().take(4).collect::<Vec<_>>()),
    })
}

pub async fn execute(name: &str, args: &Map<String, Value>) -> ToolResult {
    let a = |k| arg(args, k);
    match name {
        "get_app_state" => {
            let route = page_context();
            let library = blocking(|| mv_core::library::open_readonly().and_then(|c| mv_core::library::list_manga(&c)).map(|m| m.len()).ok()).await?;
            Ok(ToolOutput::Value(json!({
                "page": route.as_ref().map(page_name),
                "category": route.as_ref().map(section_name),
                "librarySize": library,
            })))
        }
        "navigate" => match route_for_page(a("page")) {
            Some(r) => nav(r, json!({ "ok": true, "page": a("page") })),
            None => Err(format!("Unknown page \"{}\"", a("page"))),
        },
        "go_back" => nav(Route::Back, json!({ "ok": true })),
        "set_category" => match a("category") {
            "manga" => nav(Route::Library, json!({ "ok": true })),
            "anime" => nav(Route::AnimeHome, json!({ "ok": true })),
            "movies" => nav(Route::MoviesHome, json!({ "ok": true })),
            other => Err(format!("Unknown section \"{other}\"")),
        },
        "open_settings" => {
            let tab = a("tab");
            if !SETTINGS_TABS.contains(&tab) {
                return Err(format!("Unknown settings tab \"{tab}\""));
            }
            nav(Route::Settings { tab: Some(tab.to_string()) }, json!({ "ok": true }))
        }
        "list_themes" => Ok(ToolOutput::Value(json!({ "note": "Theme switching is not available in this build yet.", "themes": [] }))),
        "set_theme" => Err("Theme switching is not available in this build yet.".into()),
        "list_library" => {
            let q = a("query").to_lowercase();
            let rows = blocking(move || {
                mv_core::library::open_readonly().and_then(|c| mv_core::library::list_manga(&c)).map_err(|e| e.to_string())
            })
            .await??;
            let items: Vec<Value> = rows
                .into_iter()
                .filter(|m| q.is_empty() || m.title.to_lowercase().contains(&q))
                .take(40)
                .map(|m| json!({ "id": m.id, "title": m.title, "status": m.status, "author": m.author, "unread": m.unread }))
                .collect();
            Ok(ToolOutput::Value(Value::Array(items)))
        }
        "get_watch_history" => {
            let kind = Some(a("kind").to_string()).filter(|k| !k.is_empty());
            let limit = args.get("limit").and_then(Value::as_f64).map(|n| n as usize).unwrap_or(15).clamp(1, 50);
            let rows = blocking(move || {
                mv_core::library::open_readonly().and_then(|c| mv_core::repo::media::list_history(&c, kind.as_deref(), None, limit)).map_err(|e| e.to_string())
            })
            .await??;
            let items: Vec<Value> = rows
                .into_iter()
                .map(|r| json!({ "title": r.title, "kind": r.kind, "episode": r.episode_number, "completed": r.completed, "updatedAt": r.updated_at }))
                .collect();
            Ok(ToolOutput::Value(Value::Array(items)))
        }
        "search_anime" => {
            let q = a("query");
            if q.is_empty() {
                return Err("query is required".into());
            }
            let c = mv_media::jikan::JikanClient::shared().map_err(|e| e.to_string())?;
            let rows = c.search_anime(q, 1, None).await.map_err(|e| e.to_string())?;
            Ok(ToolOutput::Value(Value::Array(rows.iter().take(8).map(compact).collect())))
        }
        "open_anime" => {
            let query = if a("title").is_empty() { a("id") } else { a("title") };
            if query.is_empty() {
                return Err("Give a title or an id".into());
            }
            let id = a("id");
            // A numeric id needs no search.
            if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !a("title").is_empty() {
                return nav(Route::AnimeDetail { mal_id: id.to_string(), title: a("title").to_string() }, json!({ "ok": true, "opened": a("title") }));
            }
            let c = mv_media::jikan::JikanClient::shared().map_err(|e| e.to_string())?;
            let rows = c.search_anime(query, 1, None).await.map_err(|e| e.to_string())?;
            let hit = rows.iter().find(|r| !id.is_empty() && r.id == id).or_else(|| rows.first()).ok_or_else(|| format!("No anime found for \"{query}\""))?;
            nav(Route::AnimeDetail { mal_id: hit.id.clone(), title: hit.title.clone() }, json!({ "ok": true, "opened": hit.title }))
        }
        "search_movies" => {
            let q = a("query");
            if q.is_empty() {
                return Err("query is required".into());
            }
            let c = mv_media::cinemeta::CinemetaClient::new().map_err(|e| e.to_string())?;
            let rows = c.search_movies(q).await.map_err(|e| e.to_string())?;
            Ok(ToolOutput::Value(Value::Array(rows.iter().take(8).map(compact).collect())))
        }
        "open_movie" => {
            let imdb = a("imdbId");
            if imdb.is_empty() {
                return Err("imdbId is required".into());
            }
            let kind = if a("kind") == "series" { "series" } else { "movie" };
            nav(Route::MovieDetail { imdb_id: imdb.to_string(), kind: kind.into(), title: a("title").to_string() }, json!({ "ok": true, "opened": a("title") }))
        }
        "open_manga" => {
            let q = a("title").to_lowercase();
            if q.is_empty() {
                return Err("title is required".into());
            }
            let rows = blocking(|| mv_core::library::open_readonly().and_then(|c| mv_core::library::list_manga(&c)).map_err(|e| e.to_string())).await??;
            let hit = rows
                .iter()
                .find(|m| m.title.to_lowercase() == q)
                .or_else(|| rows.iter().find(|m| m.title.to_lowercase().contains(&q)))
                .ok_or_else(|| format!("\"{}\" is not in the library. Add it from Browse first.", a("title")))?;
            nav(Route::MangaDetail { manga_id: hit.id.clone(), source_id: None }, json!({ "ok": true, "opened": hit.title }))
        }
        "play_title" => {
            let title = a("title");
            if title.is_empty() {
                return Err("title is required".into());
            }
            let cat = a("category");
            if cat != "movie" && cat != "series" {
                if let Ok(c) = mv_media::jikan::JikanClient::shared() {
                    if let Ok(rows) = c.search_anime(title, 1, None).await {
                        if let Some(hit) = rows.first() {
                            return nav(
                                Route::AnimeDetail { mal_id: hit.id.clone(), title: hit.title.clone() },
                                json!({ "ok": true, "opened": hit.title, "note": "Opened the title page; the user presses play there." }),
                            );
                        }
                    }
                }
                if cat == "anime" {
                    return Err(format!("No anime matched \"{title}\""));
                }
            }
            let c = mv_media::cinemeta::CinemetaClient::new().map_err(|e| e.to_string())?;
            let rows = c.search_movies(title).await.map_err(|e| e.to_string())?;
            let hit = rows.first().ok_or_else(|| format!("No title matched \"{title}\""))?;
            let imdb = hit.imdb_id.clone().unwrap_or_else(|| hit.id.clone());
            let kind = if cat == "series" || matches!(hit.anime_type.as_deref(), Some("Series") | Some("TV")) { "series" } else { "movie" };
            nav(
                Route::MovieDetail { imdb_id: imdb, kind: kind.into(), title: hit.title.clone() },
                json!({ "ok": true, "opened": hit.title, "note": "Opened the title page; the user presses play there." }),
            )
        }
        other => Err(format!("Unknown tool {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_valid_json_schema_objects() {
        let specs = tool_specs();
        assert!(specs.len() >= 14);
        let mut names = std::collections::HashSet::new();
        for s in &specs {
            assert!(names.insert(s.name.clone()), "duplicate {}", s.name);
            assert_eq!(s.parameters["type"], "object");
            assert!(s.parameters["properties"].is_object());
            for r in s.parameters["required"].as_array().unwrap() {
                assert!(s.parameters["properties"].get(r.as_str().unwrap()).is_some());
            }
        }
    }

    #[test]
    fn approval_policy() {
        use AiAccessMode::*;
        assert!(!needs_approval(Ask, Risk::Read));
        assert!(needs_approval(Ask, Risk::Act));
        assert!(needs_approval(Ask, Risk::Destructive));
        assert!(!needs_approval(Auto, Risk::Act));
        assert!(needs_approval(Auto, Risk::Destructive));
        assert!(!needs_approval(Full, Risk::Destructive));
        assert_eq!(risk_of("search_anime"), Some(Risk::Read));
        assert_eq!(risk_of("navigate"), Some(Risk::Act));
        assert_eq!(risk_of("nope"), None);
    }

    #[tokio::test]
    async fn navigation_tools() {
        let mut a = Map::new();
        a.insert("page".into(), json!("anime"));
        assert_eq!(execute("navigate", &a).await.unwrap(), ToolOutput::Navigate(Route::AnimeHome, json!({ "ok": true, "page": "anime" })));
        a.insert("page".into(), json!("nowhere"));
        assert!(execute("navigate", &a).await.is_err());
        let mut b = Map::new();
        b.insert("tab".into(), json!("models"));
        match execute("open_settings", &b).await.unwrap() {
            ToolOutput::Navigate(Route::Settings { tab }, _) => assert_eq!(tab.as_deref(), Some("models")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(execute("go_back", &Map::new()).await.unwrap(), ToolOutput::Navigate(Route::Back, _)));
        assert!(execute("bogus", &Map::new()).await.is_err());
    }

    #[test]
    fn trims_big_results() {
        let big = Value::Array((0..500).map(|i| json!({ "n": i, "pad": "x".repeat(50) })).collect());
        let t = trim_result(big, 6000);
        assert_eq!(t["items"].as_array().unwrap().len(), 15);
        let small = json!([1, 2]);
        assert_eq!(trim_result(small.clone(), 6000), small);
    }

    #[test]
    fn labels() {
        let mut a = Map::new();
        a.insert("query".into(), json!("naruto"));
        assert_eq!(label("search_anime", &a), "Searched anime for “naruto”");
    }
}
