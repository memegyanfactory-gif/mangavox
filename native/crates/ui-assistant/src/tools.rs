//! App-control tools the assistant can call (port of src/renderer/assistant/tools.ts).
//! Each tool has a JSON schema and a risk level that is checked against the access mode.
use std::sync::{Arc, Mutex, RwLock};

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

// ── Registered tools (other crates add theirs at startup) ────────────────────────────

pub type RunFn = Arc<dyn Fn(Map<String, Value>) -> BoxFuture<'static, ToolResult> + Send + Sync>;
pub type PhrasesFn = Arc<dyn Fn(&Map<String, Value>) -> Phrases + Send + Sync>;
pub type RiskFn = Arc<dyn Fn(&Map<String, Value>) -> Risk + Send + Sync>;

/// A tool another crate gives the assistant (Studio, manga, media, the video editor). The handler
/// runs on the tokio runtime; it reaches GPUI state through [`crate::on_ui`], so what it changes is
/// the same state the pages show.
#[derive(Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: String,
    pub parameters: Value,
    pub risk: Risk,
    /// Raises the risk for one call (a big download, an overwrite).
    pub risk_for: Option<RiskFn>,
    pub phrases: Option<PhrasesFn>,
    pub run: RunFn,
}

impl ToolDef {
    pub fn new(name: &'static str, description: impl Into<String>, parameters: Value, risk: Risk) -> Self {
        let run: RunFn = Arc::new(move |_| async move { Err(format!("{name} has no handler")) }.boxed());
        Self { name, description: description.into(), parameters, risk, risk_for: None, phrases: None, run }
    }

    pub fn run(mut self, f: impl Fn(Map<String, Value>) -> BoxFuture<'static, ToolResult> + Send + Sync + 'static) -> Self {
        self.run = Arc::new(f);
        self
    }

    pub fn phrases(mut self, f: impl Fn(&Map<String, Value>) -> Phrases + Send + Sync + 'static) -> Self {
        self.phrases = Some(Arc::new(f));
        self
    }

    pub fn risk_for(mut self, f: impl Fn(&Map<String, Value>) -> Risk + Send + Sync + 'static) -> Self {
        self.risk_for = Some(Arc::new(f));
        self
    }
}

static REGISTERED: RwLock<Vec<ToolDef>> = RwLock::new(Vec::new());

/// Adds tools to the assistant. A name that is already registered is replaced, so a crate can
/// register again without duplicates.
pub fn register(defs: Vec<ToolDef>) {
    let mut g = REGISTERED.write().unwrap_or_else(|e| e.into_inner());
    for d in defs {
        if builtin_tools().iter().any(|t| t.name == d.name) {
            eprintln!("[assistant] tool {} clashes with a built-in tool; skipped", d.name);
            continue;
        }
        g.retain(|t| t.name != d.name);
        g.push(d);
    }
}

pub fn registered(name: &str) -> Option<ToolDef> {
    REGISTERED.read().unwrap_or_else(|e| e.into_inner()).iter().find(|t| t.name == name).cloned()
}

fn registered_all() -> Vec<ToolDef> {
    REGISTERED.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Every arg the model may pass as a list ("chapters": [12, 13] or "12-14, 20").
pub fn list_arg(args: &Map<String, Value>, k: &str) -> Vec<String> {
    match args.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| match v {
            Value::String(s) => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter(|s| !s.is_empty())
        .collect(),
        Some(Value::String(s)) => s.split(',').map(|p| p.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::Number(n)) => vec![n.to_string()],
        _ => vec![],
    }
}

pub fn str_arg<'a>(args: &'a Map<String, Value>, k: &str) -> &'a str {
    arg(args, k)
}

pub fn bool_arg(args: &Map<String, Value>, k: &str) -> Option<bool> {
    match args.get(k) {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Some(true),
            "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn num_arg(args: &Map<String, Value>, k: &str) -> Option<f64> {
    match args.get(k) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
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
        Route::MangaDownloads => "downloads",
        Route::MangaDetail { .. } => "manga-detail",
        Route::Reader { .. } => "reader",
        Route::AnimeHome => "anime",
        Route::AnimeDetail { .. } => "anime-detail",
        Route::MoviesHome => "movies",
        Route::MovieDetail { .. } => "movie-detail",
        Route::Mobile => "mobile",
        Route::Discover => "discover",
        Route::MediaHistory => "media-history",
        Route::Favorites => "favorites",
        Route::Player { .. } => "player",
        Route::Studio => "studio",
        Route::YouTube => "youtube",
        Route::Settings { .. } => "settings",
        Route::Welcome => "welcome",
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

pub const PAGES: &[&str] = &["library", "browse", "history", "downloads", "anime", "movies", "discover", "favorites", "media-history", "studio", "youtube", "mobile", "settings"];
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

/// Built-in tools plus every registered one.
pub fn app_tools() -> Vec<AppTool> {
    let mut all = builtin_tools();
    all.extend(registered_all().into_iter().map(|d| AppTool { name: d.name, description: d.description, parameters: d.parameters, risk: d.risk }));
    // Provider-style tools (Studio's Video editor, `ext`).
    all.extend(crate::ext::tools());
    all
}

fn builtin_tools() -> Vec<AppTool> {
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
        t(
            "youtube_channel",
            "The connected YouTube channel: subscribers, views in the last 28 days, the newest videos with views and trend (rising/steady/dipping), the upload queue with release times, and growth suggestions.".into(),
            obj(json!({}), &[]),
            Risk::Read,
        ),
        t(
            "youtube_video",
            "One YouTube video's title, description, tags, numbers, views per day since release and its SEO checklist. Pass the id from youtube_channel.".into(),
            obj(json!({ "id": s("Video id") }), &["id"]),
            Risk::Read,
        ),
        t(
            "youtube_update_video",
            "Change a live YouTube video's title, description and/or tags (only the fields given). Use to apply better SEO the user agreed to.".into(),
            obj(json!({ "id": s("Video id"), "title": s("New title (max 100 chars)"), "description": s("New description"), "tags": s("Comma-separated tags") }), &["id"]),
            Risk::Act,
        ),
        t(
            "youtube_schedule",
            "Set when a video goes public: a queued upload (queue item id) or a private video on YouTube (video id). `at` is local time \"YYYY-MM-DD HH:MM\". Keep each series in chapter order.".into(),
            obj(json!({ "id": s("Queue item id or video id"), "at": s("Local time YYYY-MM-DD HH:MM") }), &["id", "at"]),
            Risk::Act,
        ),
        t(
            "suggest_replies",
            "Show 1-4 clickable buttons under your reply with the next steps the user is likely to want (\"Generate recaps for Ch. 12-14\", \"Export the full recap\"). Each prompt is sent as the user's message when clicked, so word it as their request and name the exact titles and chapters. Call it last, at most once per reply.".into(),
            obj(
                json!({ "suggestions": { "type": "array", "description": "Buttons", "items": obj(json!({ "label": s("Short button text"), "prompt": s("The message sent when clicked") }), &["label", "prompt"]) } }),
                &["suggestions"],
            ),
            Risk::Read,
        ),
        t("app_suggestions", "What the app suggests right now: failed Studio chapters to retry, new downloads without recaps, recaps ready to export, YouTube titles to write. Use when the user asks what to do next.".into(), obj(json!({}), &[]), Risk::Read),
    ]
}

pub fn tool_specs() -> Vec<AiToolSpec> {
    app_tools().into_iter().map(|t| AiToolSpec { name: t.name.into(), description: t.description, parameters: t.parameters }).collect()
}

pub fn risk_of(name: &str) -> Option<Risk> {
    app_tools().into_iter().find(|t| t.name == name).map(|t| t.risk)
}

/// The risk of one call: a registered tool may raise it for these args (a big download).
pub fn risk_for(name: &str, args: &Map<String, Value>) -> Option<Risk> {
    if let Some(d) = registered(name) {
        return Some(d.risk_for.as_ref().map(|f| f(args)).unwrap_or(d.risk));
    }
    risk_of(name)
}

fn arg<'a>(args: &'a Map<String, Value>, k: &str) -> &'a str {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

/// Short human label for the step row, in the past tense ("Opened Library").
pub fn label(name: &str, args: &Map<String, Value>) -> String {
    phrases(name, args).done
}

/// One tool call worded for each moment: before it runs (`ask`, imperative: "Open Library"),
/// while it runs (`doing`: "Opening Library") and after (`done`: "Opened Library"), so an
/// approval never claims the action already happened.
pub struct Phrases {
    pub ask: String,
    pub doing: String,
    pub done: String,
}

impl Phrases {
    /// `Phrases::new("Generate", "Generating", "Generated", "Ch. 12")`.
    pub fn new(ask: &str, doing: &str, done: &str, object: &str) -> Self {
        let join = |v: &str| if object.is_empty() { v.to_string() } else { format!("{v} {object}") };
        Phrases { ask: join(ask), doing: join(doing), done: join(done) }
    }
}

/// Display name for a page id ("media-history" → "Watch history").
pub fn page_title(id: &str) -> String {
    match id {
        "library" => "Library",
        "browse" => "Browse",
        "history" => "Reading history",
        "downloads" => "Downloads",
        "anime" => "Anime",
        "movies" => "Movies",
        "discover" => "Discover",
        "favorites" => "Favorites",
        "media-history" => "Watch history",
        "studio" => "Studio",
        "mobile" => "Mobile",
        "youtube" => "YouTube",
        "settings" => "Settings",
        "" => "the page",
        other => return capitalize(other),
    }
    .to_string()
}

fn settings_tab_title(id: &str) -> String {
    match id {
        "providers" => "AI providers".into(),
        "models" => "Model library".into(),
        "addons" => "Add-ons".into(),
        other => capitalize(other),
    }
}

fn capitalize(s: &str) -> String {
    let mut cs = s.chars();
    cs.next().map(|f| f.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default()
}

pub fn phrases(name: &str, args: &Map<String, Value>) -> Phrases {
    let a = |k| arg(args, k);
    let p = |verb: &str, doing: &str, done: &str, obj: &str| {
        let join = |v: &str| if obj.is_empty() { v.to_string() } else { format!("{v} {obj}") };
        Phrases { ask: join(verb), doing: join(doing), done: join(done) }
    };
    let titled = |k: &str, fallback: &str| {
        let v = arg(args, k);
        if v.is_empty() { fallback.to_string() } else { v.to_string() }
    };
    match name {
        "get_app_state" => p("Check", "Checking", "Checked", "what is on screen"),
        "navigate" => p("Open", "Opening", "Opened", &page_title(a("page"))),
        "go_back" => p("Go back", "Going back", "Went back", ""),
        "set_category" => p("Switch to", "Switching to", "Switched to", &capitalize(a("category"))),
        "open_settings" => p("Open", "Opening", "Opened", &format!("Settings → {}", settings_tab_title(a("tab")))),
        "list_themes" => p("List themes", "Listing themes", "Listed themes", ""),
        "set_theme" => p("Change the theme to", "Changing the theme to", "Changed the theme to", a("id")),
        "list_library" => p("Read", "Reading", "Read", "your library"),
        "get_watch_history" => p("Read", "Reading", "Read", "your watch history"),
        "search_anime" => p("Search anime for", "Searching anime for", "Searched anime for", &format!("“{}”", a("query"))),
        "open_anime" => p("Open", "Opening", "Opened", &titled("title", "the anime")),
        "search_movies" => p("Search movies for", "Searching movies for", "Searched movies for", &format!("“{}”", a("query"))),
        "open_movie" => p("Open", "Opening", "Opened", &titled("title", "the movie")),
        "open_manga" => p("Open", "Opening", "Opened", &titled("title", "the manga")),
        "play_title" => p("Open", "Opening", "Opened", &titled("title", "the title")),
        "youtube_channel" => p("Read", "Reading", "Read", "your YouTube channel"),
        "youtube_video" => p("Read", "Reading", "Read", "the video's details"),
        "youtube_update_video" => p("Update", "Updating", "Updated", &titled("title", "the video on YouTube")),
        "youtube_schedule" => p("Schedule the video for", "Scheduling the video for", "Scheduled the video for", a("at")),
        "suggest_replies" => p("Suggest", "Suggesting", "Suggested", "next steps"),
        "app_suggestions" => p("Check", "Checking", "Checked", "what to do next"),
        other => match registered(other).and_then(|d| d.phrases) {
            Some(f) => f(args),
            None => crate::ext::phrases(other, args).unwrap_or_else(|| p("Run", "Running", "Ran", &other.replace('_', " "))),
        },
    }
}

// ── Execution ─────────────────────────────────────────────────────────────────

/// What a tool produced. `Navigate` asks the UI thread to change page.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutput {
    Value(Value),
    Navigate(Route, Value),
    /// Buttons to show under the reply (see `suggest_replies`).
    Suggest(Vec<crate::suggest::Suggestion>, Value),
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
        "downloads" => Route::MangaDownloads,
        "anime" => Route::AnimeHome,
        "movies" => Route::MoviesHome,
        "discover" => Route::Discover,
        "favorites" => Route::Favorites,
        "media-history" => Route::MediaHistory,
        "studio" => Route::Studio,
        "mobile" => Route::Mobile,
        "youtube" => Route::YouTube,
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
        name if name.starts_with("youtube_") => crate::youtube::execute(name, args).await.map(ToolOutput::Value),
        "suggest_replies" => {
            let list = crate::suggest::parse_replies(args.get("suggestions").unwrap_or(&Value::Null));
            if list.is_empty() {
                return Err("Give 1-4 suggestions, each with a label and a prompt".into());
            }
            let n = list.len();
            Ok(ToolOutput::Suggest(list, json!({ "ok": true, "shown": n })))
        }
        "app_suggestions" => {
            let list = blocking(crate::suggest::collect).await?;
            Ok(ToolOutput::Value(Value::Array(list.iter().map(|s| json!({ "title": s.title, "detail": s.detail, "prompt": s.prompt })).collect())))
        }
        other => match registered(other) {
            Some(d) => (d.run)(args.clone()).await,
            // Provider-style tools (Studio's Video editor registers through `ext`).
            None => match crate::ext::run(other, args) {
                Some(f) => f.await,
                None => Err(format!("Unknown tool {other}")),
            },
        },
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

    #[tokio::test]
    async fn registered_tools_join_the_table_and_run() {
        register(vec![
            ToolDef::new("test_reg_echo", "Echo", obj(json!({ "n": { "type": "number" } }), &[]), Risk::Act)
                .risk_for(|a| if num_arg(a, "n").unwrap_or(0.) > 10. { Risk::Destructive } else { Risk::Act })
                .phrases(|_| Phrases::new("Echo", "Echoing", "Echoed", "it"))
                .run(|a| async move { Ok(ToolOutput::Value(Value::Object(a))) }.boxed()),
            // A built-in name never gets replaced.
            ToolDef::new("navigate", "Hijack", obj(json!({}), &[]), Risk::Read),
        ]);
        assert!(tool_specs().iter().any(|s| s.name == "test_reg_echo"));
        assert_eq!(tool_specs().iter().filter(|s| s.name == "navigate").count(), 1);
        let mut a = Map::new();
        a.insert("n".into(), json!(3));
        assert_eq!(risk_for("test_reg_echo", &a), Some(Risk::Act));
        assert_eq!(phrases("test_reg_echo", &a).ask, "Echo it");
        assert_eq!(execute("test_reg_echo", &a).await.unwrap(), ToolOutput::Value(json!({ "n": 3 })));
        a.insert("n".into(), json!(30));
        assert_eq!(risk_for("test_reg_echo", &a), Some(Risk::Destructive));
        assert_eq!(risk_for("navigate", &a), Some(Risk::Act));
        assert_eq!(phrases("some_tool", &a).ask, "Run some tool");
    }

    #[tokio::test]
    async fn suggest_replies_becomes_buttons() {
        let mut a = Map::new();
        a.insert("suggestions".into(), json!([{ "label": "Export", "prompt": "Export the full recap of Solo" }]));
        match execute("suggest_replies", &a).await.unwrap() {
            ToolOutput::Suggest(list, _) => assert_eq!(list[0].prompt, "Export the full recap of Solo"),
            other => panic!("{other:?}"),
        }
        assert!(execute("suggest_replies", &Map::new()).await.is_err());
    }

    #[test]
    fn list_and_scalar_args() {
        let mut a = Map::new();
        a.insert("chapters".into(), json!([12, "13-14", " "]));
        a.insert("csv".into(), json!("1, 2"));
        a.insert("flag".into(), json!("yes"));
        assert_eq!(list_arg(&a, "chapters"), ["12", "13-14"]);
        assert_eq!(list_arg(&a, "csv"), ["1", "2"]);
        assert_eq!(bool_arg(&a, "flag"), Some(true));
        assert_eq!(bool_arg(&a, "none"), None);
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

    #[test]
    fn phrases_use_the_right_tense() {
        let mut a = Map::new();
        a.insert("page".into(), json!("media-history"));
        let p = phrases("navigate", &a);
        assert_eq!(p.ask, "Open Watch history");
        assert_eq!(p.doing, "Opening Watch history");
        assert_eq!(p.done, "Opened Watch history");
        assert_eq!(phrases("go_back", &Map::new()).ask, "Go back");
        let mut t = Map::new();
        t.insert("tab".into(), json!("providers"));
        assert_eq!(phrases("open_settings", &t).ask, "Open Settings → AI providers");
        assert_eq!(phrases("open_anime", &Map::new()).ask, "Open the anime");
    }
}
