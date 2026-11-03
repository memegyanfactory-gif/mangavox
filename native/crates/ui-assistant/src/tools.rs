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

/// `offset` (default 0) and `limit` (default `default`, at most 200) of a paged reader.
pub fn page_args(args: &Map<String, Value>, default: usize) -> (usize, usize) {
    let offset = num_arg(args, "offset").map_or(0, |n| n.max(0.) as usize);
    let limit = num_arg(args, "limit").map_or(default, |n| (n.max(1.) as usize).min(200));
    (offset, limit)
}

/// One page of a list: the items, the total, and the offset of the next page when there is one.
pub fn paged(items: Vec<Value>, offset: usize, total: usize) -> Value {
    let end = offset + items.len();
    let mut v = json!({ "total": total, "shown": if items.is_empty() { "none".to_string() } else { format!("{}-{end}", offset + 1) }, "items": items });
    if end < total {
        v["next"] = json!(format!("{} more: call again with offset {end}", total - end));
    }
    v
}

// ── Progress of a long tool (shown on its step row while it runs) ──────────────────

pub type ProgressFn = Arc<dyn Fn(String) + Send + Sync>;

tokio::task_local! {
    static PROGRESS: ProgressFn;
}

/// The running tool's progress reporter. Take it at the start of a handler (it lives in the
/// handler's task) and move it into blocking work or threads.
pub fn progress_handle() -> Option<ProgressFn> {
    PROGRESS.try_with(|p| p.clone()).ok()
}

/// Says what the running tool is doing now ("Rendering 40%"); does nothing outside a tool call.
pub fn report_progress(text: impl Into<String>) {
    if let Some(p) = progress_handle() {
        p(text.into());
    }
}

/// Runs a tool future with `p` as its progress reporter.
pub async fn with_progress<F: std::future::Future>(p: ProgressFn, f: F) -> F::Output {
    PROGRESS.scope(p, f).await
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
        Route::Suggestions => "suggestions",
        Route::Thumbnails => "thumbnails",
        Route::YouTubeVideos => "youtube-videos",
        Route::Automation => "automation",
        Route::Creative => "creative",
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

pub const PAGES: &[&str] = &["library", "browse", "history", "downloads", "anime", "movies", "discover", "favorites", "media-history", "studio", "youtube", "suggestions", "thumbnails", "youtube-videos", "automation", "creative", "mobile", "settings"];
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
        t(
            "list_library",
            "List manga in the user's library, optionally filtered by a text query. Pages through big libraries with offset/limit.".into(),
            obj(json!({ "query": s("Optional filter"), "offset": { "type": "number", "description": "Skip this many (default 0)" }, "limit": { "type": "number", "description": "Max items (default 40)" } }), &[]),
            Risk::Read,
        ),
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
        t(
            "open_manga",
            "Open a manga's detail page: one in the library by title, or any loaded manga by its id (the mangaId from manga_chapters).".into(),
            obj(json!({ "title": s("Manga title (library)"), "id": s("Manga id (from manga_chapters / list_library)") }), &[]),
            Risk::Act,
        ),
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
        t(
            APP_GUIDE,
            format!("How MangaVox is organised and the right order of steps (find a manga → library → download → Studio steps → export), with the tools for each. Call it first when unsure how to do something. Topics: {}.", crate::guide::TOPICS.join(", ")),
            obj(json!({ "topic": s_enum("Optional topic (default: overview)", crate::guide::TOPICS) }), &[]),
            Risk::Read,
        ),
    ]
}

/// Explains the app (see [`crate::guide`]).
pub const APP_GUIDE: &str = "app_guide";
/// Searches every tool by what it does and makes the matches available (handled by the agent loop).
pub const FIND_TOOLS: &str = "find_tools";
/// One tool's full schema (handled by the agent loop).
pub const DESCRIBE_TOOL: &str = "describe_tool";

/// Core tools a small model always gets (with [`FIND_TOOLS`] / [`DESCRIBE_TOOL`] for the rest).
pub const LEAN_CORE: &[&str] = &[APP_GUIDE, "get_app_state", "navigate", "go_back", "open_settings", "app_suggestions", "suggest_replies"];

/// Words that carry no meaning for tool search.
const STOP: &[&str] = &["a", "an", "the", "to", "of", "for", "and", "or", "in", "on", "my", "me", "i", "it", "is", "be", "with", "from", "this", "that", "please", "can", "you", "how", "do", "what", "want", "some", "all", "into", "by", "at", "as", "tool", "tools", "app"];

/// What users say → words the tools use.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("recap", &["studio", "generate", "video"]),
    ("make", &["generate", "compose", "create"]),
    ("create", &["generate", "compose"]),
    ("subtitle", &["captions"]),
    ("caption", &["captions"]),
    ("narration", &["voice", "audio"]),
    ("narrator", &["voice", "voices"]),
    ("tts", &["voice", "audio"]),
    ("favourite", &["library", "favorites"]),
    ("favorite", &["library", "favorites"]),
    ("save", &["library", "export"]),
    ("queue", &["queue", "status"]),
    ("progress", &["status", "downloads"]),
    ("animation", &["motion", "compose"]),
    ("animate", &["motion", "compose"]),
    ("graphic", &["motion", "compose"]),
    ("intro", &["motion", "hook"]),
    ("trailer", &["motion", "hook"]),
    ("thumbnail", &["thumbnails", "thumbnail"]),
    ("watch", &["play", "media"]),
    ("stream", &["play", "media", "sources"]),
    ("movie", &["movies", "media"]),
    ("series", &["movies", "media"]),
    ("show", &["media", "movies", "anime"]),
    ("read", &["reader", "chapters"]),
    ("chapter", &["chapters"]),
    ("page", &["pages", "reader"]),
    ("panel", &["pages", "motion"]),
    ("render", &["video", "render"]),
    ("look", &["look", "effects", "preset"]),
    ("effect", &["effects", "look"]),
    ("watermark", &["look"]),
    ("music", &["music"]),
    ("sound", &["sfx", "music"]),
    ("pause", &["queue", "pause"]),
    ("cancel", &["queue", "cancel"]),
    ("retry", &["queue", "retry"]),
    ("delete", &["delete", "remove"]),
    ("remove", &["delete", "remove"]),
    ("search", &["search", "find"]),
    ("find", &["search"]),
    ("youtube", &["youtube"]),
    ("upload", &["youtube", "schedule"]),
    ("seo", &["youtube"]),
];

fn stem(w: &str) -> String {
    let w = w.trim();
    for suf in ["ing", "ed", "es", "s"] {
        if w.len() > suf.len() + 3 {
            if let Some(r) = w.strip_suffix(suf) {
                return r.to_string();
            }
        }
    }
    w.to_string()
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !STOP.contains(w)).map(stem).collect()
}

/// Tools ranked by how well they match `query` (what the user wants to do, or a tool name),
/// best first; only matches. A tool's name counts most, then its group, then its description.
pub fn rank_tools(query: &str, specs: &[AiToolSpec]) -> Vec<(String, f32)> {
    let q = query.trim().to_lowercase();
    let mut terms = words(&q);
    for (from, to) in SYNONYMS {
        // "animated", "animation" and "animate" all hit "animat".
        let key: String = stem(from).chars().take(6).collect();
        if terms.iter().any(|t| t.starts_with(&key)) {
            terms.extend(to.iter().map(|w| stem(w)));
        }
    }
    terms.sort();
    terms.dedup();
    let mut out: Vec<(String, f32)> = specs
        .iter()
        .filter(|t| !matches!(t.name.as_str(), FIND_TOOLS | DESCRIBE_TOOL | LOAD_TOOLS | RUN_TOOL))
        .filter_map(|t| {
            let name = t.name.to_lowercase();
            let name_words = words(&name.replace('_', " "));
            let desc_words = words(&t.description);
            let group = group_of(&t.name).key();
            let mut score = if name == q || name == q.replace(' ', "_") { 20.0 } else { 0.0 };
            for term in &terms {
                if name_words.iter().any(|w| w == term) {
                    score += 4.0;
                } else if name_words.iter().any(|w| w.starts_with(term.as_str()) || term.starts_with(w.as_str())) && term.len() >= 3 {
                    score += 2.0;
                }
                if group == term {
                    score += 1.5;
                }
                let hits = desc_words.iter().filter(|w| *w == term).count();
                score += (hits.min(3) as f32) * 0.75;
            }
            (score > 0.0).then_some((t.name.clone(), score))
        })
        .collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    out
}

/// "Did you mean …" for a tool name the model made up.
pub fn unknown_tool_error(name: &str) -> String {
    let near: Vec<String> = rank_tools(&name.replace('_', " "), &tool_specs()).into_iter().take(3).map(|(n, _)| n).collect();
    if near.is_empty() {
        format!("Unknown tool {name}. Use {FIND_TOOLS} to search the app's tools by what they do.")
    } else {
        format!("Unknown tool {name}. Did you mean {}? Or use {FIND_TOOLS} to search the app's tools.", near.join(", "))
    }
}

pub fn tool_specs() -> Vec<AiToolSpec> {
    app_tools().into_iter().map(|t| AiToolSpec { name: t.name.into(), description: t.description, parameters: t.parameters }).collect()
}

// ── Tool groups (a step carries only the tools for where the user is) ─────────────────

/// The tools fall into groups; a step gets the core ones plus the groups of the page the user is
/// on, and the model loads others with [`LOAD_TOOLS`] when the request needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Core,
    Manga,
    Media,
    Studio,
    Video,
    Youtube,
}

impl Group {
    pub const ALL: [Group; 6] = [Group::Core, Group::Manga, Group::Media, Group::Studio, Group::Video, Group::Youtube];

    pub fn key(&self) -> &'static str {
        match self {
            Group::Core => "core",
            Group::Manga => "manga",
            Group::Media => "media",
            Group::Studio => "studio",
            Group::Video => "video",
            Group::Youtube => "youtube",
        }
    }

    /// What the group's tools do, for the catalog in [`LOAD_TOOLS`].
    fn about(&self) -> &'static str {
        match self {
            Group::Core => "pages, settings, suggestions",
            Group::Manga => "the library, manga sources, chapters, downloads, the reader",
            Group::Media => "anime, movies and series: search, open, play, stream sources, downloads, watch history",
            Group::Studio => "recap videos: the Studio library, the generate queue and single steps, voices, look, captions, export, thumbnails, playing videos, custom motion-graphic videos (motion_compose)",
            Group::Video => "the Video editor: a chapter video's timeline, clips, layers, captions, music, look, render",
            Group::Youtube => "the YouTube channel: stats, videos, SEO, upload schedule",
        }
    }

    pub fn parse(s: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.key().eq_ignore_ascii_case(s.trim()))
    }
}

/// The group a tool belongs to (by name, so tools other crates add sort themselves).
pub fn group_of(name: &str) -> Group {
    match name {
        n if n.starts_with("video_") => Group::Video,
        n if n.starts_with("studio_") || n.starts_with("motion_") => Group::Studio,
        n if n.starts_with("youtube_") => Group::Youtube,
        n if n.starts_with("manga_") || matches!(n, "list_library" | "open_manga" | "open_reader") => Group::Manga,
        n if n.starts_with("media_") || matches!(n, "get_watch_history" | "search_anime" | "open_anime" | "search_movies" | "open_movie" | "play_title") => Group::Media,
        _ => Group::Core,
    }
}

/// The groups that come with a page (besides core).
pub fn page_groups(route: Option<&Route>) -> &'static [Group] {
    match route {
        Some(Route::Studio) => &[Group::Studio, Group::Video],
        Some(Route::YouTube | Route::Automation) => &[Group::Youtube, Group::Studio],
        Some(Route::Suggestions) => &[Group::Studio, Group::Manga],
        Some(Route::Thumbnails | Route::YouTubeVideos) => &[Group::Studio],
        Some(Route::Settings { .. } | Route::Welcome | Route::Mobile) => &[],
        Some(r) if section_name(r) != "manga" => &[Group::Media],
        _ => &[Group::Manga],
    }
}

/// Loads more tool groups (handled by the agent loop, not [`execute`]).
pub const LOAD_TOOLS: &str = "load_tools";
/// Runs any app tool by name: how an MCP client calls a tool loaded after it listed the tools.
pub const RUN_TOOL: &str = "run_tool";

/// The tool groups one chat turn can use. Starts from the page's groups and the ones the recent
/// conversation used; grows when the model calls [`LOAD_TOOLS`] or a tool of another group.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolScope {
    groups: std::collections::BTreeSet<Group>,
    /// Single tools added by [`FIND_TOOLS`] / [`DESCRIBE_TOOL`] or called directly in lean mode.
    extra: std::collections::BTreeSet<String>,
    /// Tools reach the model over MCP: it lists them once, so later groups go through [`RUN_TOOL`].
    mcp: bool,
    /// A small model: only [`LEAN_CORE`] plus the tools it finds, never whole groups.
    lean: bool,
}

/// How many recent messages decide which groups the conversation is using.
const RECENT: usize = 20;

impl ToolScope {
    pub fn new(page: Option<&Route>, history: &[mv_ai::AiMessage], mcp: bool) -> Self {
        let mut groups: std::collections::BTreeSet<Group> = std::iter::once(Group::Core).chain(page_groups(page).iter().copied()).collect();
        for m in history.iter().rev().take(RECENT) {
            groups.extend(m.tool_calls.iter().flatten().map(|c| group_of(&c.name)));
            groups.extend(m.tool_name.as_deref().map(group_of));
        }
        ToolScope { groups, extra: Default::default(), mcp, lean: false }
    }

    /// For small models (local ones, the text protocol): the core set, the tools the recent
    /// conversation used, and [`FIND_TOOLS`] for everything else.
    pub fn lean(history: &[mv_ai::AiMessage], mcp: bool) -> Self {
        let mut extra: std::collections::BTreeSet<String> = LEAN_CORE.iter().map(|s| s.to_string()).collect();
        let known: Vec<String> = app_tools().into_iter().map(|t| t.name.to_string()).collect();
        for m in history.iter().rev().take(RECENT) {
            extra.extend(m.tool_calls.iter().flatten().map(|c| c.name.clone()).filter(|n| known.contains(n)));
            extra.extend(m.tool_name.clone().filter(|n| known.contains(n)));
        }
        ToolScope { groups: Default::default(), extra, mcp, lean: true }
    }

    pub fn is_lean(&self) -> bool {
        self.lean
    }

    pub fn groups(&self) -> Vec<Group> {
        self.groups.iter().copied().collect()
    }

    /// Adds the group of a tool the model called directly (just the tool in lean mode).
    pub fn add_for(&mut self, name: &str) {
        if self.lean {
            self.extra.insert(name.to_string());
        } else {
            self.groups.insert(group_of(name));
        }
    }

    fn has(&self, name: &str) -> bool {
        self.groups.contains(&group_of(name)) || self.extra.contains(name)
    }

    /// Runs [`FIND_TOOLS`]: the best matches for `query`, made available from the next round.
    pub fn find(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let q = arg(args, "query");
        if q.is_empty() {
            return Err("Say what you want to do, e.g. {\"query\": \"download chapters\"}".into());
        }
        let limit = num_arg(args, "limit").map_or(6, |n| (n.max(1.) as usize).min(12));
        let all = tool_specs();
        let hits: Vec<AiToolSpec> = rank_tools(q, &all).into_iter().take(limit).filter_map(|(n, _)| all.iter().find(|t| t.name == n).cloned()).collect();
        if hits.is_empty() {
            return Ok(json!({ "tools": [], "note": format!("Nothing matched \"{q}\". Try other words, or {APP_GUIDE} for how the app is organised.") }));
        }
        for h in &hits {
            self.extra.insert(h.name.clone());
        }
        let mut v = json!({ "tools": hits.iter().map(signature).collect::<Vec<_>>(), "note": format!("These tools are available now. {DESCRIBE_TOOL} gives one tool's full schema.") });
        if self.mcp {
            v["howToCall"] = json!(format!("If your tool list does not show them, call them with {RUN_TOOL} {{\"name\": ..., \"args\": {{...}}}}."));
        }
        Ok(v)
    }

    /// Runs [`DESCRIBE_TOOL`]: one tool's description and JSON schema, and makes it available.
    pub fn describe(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let name = crate::textcall::clean_name(arg(args, "name"));
        let Some(t) = tool_specs().into_iter().find(|t| t.name == name) else {
            return Err(unknown_tool_error(&name));
        };
        self.extra.insert(t.name.clone());
        let mut v = json!({ "name": t.name, "description": t.description, "parameters": t.parameters, "risk": risk_of(&t.name).map(|r| r.as_str()) });
        if self.mcp {
            v["howToCall"] = json!(format!("Call it directly, or with {RUN_TOOL} if your tool list does not show it."));
        }
        Ok(v)
    }

    /// The specs to send: the scope's tools plus [`LOAD_TOOLS`] (and [`RUN_TOOL`] over MCP).
    pub fn specs(&self) -> Vec<AiToolSpec> {
        let mut out: Vec<AiToolSpec> = tool_specs().into_iter().filter(|t| self.has(&t.name)).collect();
        out.push(AiToolSpec {
            name: FIND_TOOLS.into(),
            description: "Searches all of the app's tools by what you want to do (\"download chapters\", \"export reels\", \"animated intro\") and makes the best matches available. Use it whenever the tool you need is not in your list.".into(),
            parameters: obj(json!({ "query": s("What you want to do, in a few words"), "limit": { "type": "number", "description": "Max tools (default 6)" } }), &["query"]),
        });
        out.push(AiToolSpec {
            name: DESCRIBE_TOOL.into(),
            description: "One tool's full description and argument schema (use before calling a tool you are unsure about).".into(),
            parameters: obj(json!({ "name": s("Tool name") }), &["name"]),
        });
        let others: Vec<String> = Group::ALL.iter().filter(|g| !self.groups.contains(g)).map(|g| format!("{} ({})", g.key(), g.about())).collect();
        if !others.is_empty() && !self.lean {
            out.push(AiToolSpec {
                name: LOAD_TOOLS.into(),
                description: format!("Loads more of the app's tools when the request needs them. Not loaded yet: {}.", others.join("; ")),
                parameters: obj(json!({ "groups": { "type": "array", "items": s_enum("Group", &Group::ALL.map(|g| g.key())), "description": "Groups to load" } }), &["groups"]),
            });
        }
        if self.mcp {
            out.push(AiToolSpec {
                name: RUN_TOOL.into(),
                description: "Runs an app tool by name with its arguments: for tools load_tools returned that your tool list does not show.".into(),
                parameters: obj(json!({ "name": s("Tool name"), "args": { "type": "object", "description": "The tool's arguments" } }), &["name"]),
            });
        }
        out
    }

    /// Runs [`LOAD_TOOLS`]: adds the groups and says what they hold. Over MCP the result carries
    /// the tools' signatures, since the client's list stays as it was.
    pub fn load(&mut self, args: &Map<String, Value>) -> Result<Value, String> {
        let wanted = list_arg(args, "groups");
        let groups: Vec<Group> = wanted.iter().filter_map(|g| Group::parse(g)).collect();
        if groups.is_empty() {
            return Err(format!("Name one or more groups: {}", Group::ALL.map(|g| g.key()).join(", ")));
        }
        self.groups.extend(groups.iter().copied());
        let specs: Vec<AiToolSpec> = tool_specs().into_iter().filter(|t| groups.contains(&group_of(&t.name))).collect();
        Ok(if self.mcp {
            json!({ "loaded": groups.iter().map(Group::key).collect::<Vec<_>>(), "tools": specs.iter().map(signature).collect::<Vec<_>>(), "howToCall": format!("Call these with {RUN_TOOL} {{\"name\": ..., \"args\": {{...}}}}.") })
        } else {
            json!({ "loaded": groups.iter().map(Group::key).collect::<Vec<_>>(), "tools": specs.iter().map(|t| t.name.clone()).collect::<Vec<_>>(), "note": "These tools are available now." })
        })
    }
}

/// One line per tool: `name(arg: type, optional?: type): what it does`.
pub fn signature(t: &AiToolSpec) -> String {
    let props = t.parameters.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = t.parameters.get("required").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let args: Vec<String> = props
        .map(|p| {
            p.iter()
                .map(|(k, v)| {
                    let ty = v.get("enum").and_then(Value::as_array).map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("|")).unwrap_or_else(|| match v.get("items").and_then(|i| i.get("properties")).and_then(Value::as_object) {
                        // Lists of objects show their keys: suggestions: [{label, prompt}].
                        Some(item) => format!("[{{{}}}]", item.keys().cloned().collect::<Vec<_>>().join(", ")),
                        None => v.get("type").and_then(Value::as_str).unwrap_or("any").to_string(),
                    });
                    format!("{k}{}: {ty}", if required.contains(&k.as_str()) { "" } else { "?" })
                })
                .collect()
        })
        .unwrap_or_default();
    format!("{}({}): {}", t.name, args.join(", "), t.description)
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
        APP_GUIDE => p("Read", "Reading", "Read", "the app guide"),
        FIND_TOOLS => p("Look for", "Looking for", "Looked for", &format!("tools to {}", a("query"))),
        DESCRIBE_TOOL => p("Read", "Reading", "Read", &format!("how {} works", a("name"))),
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

/// Keep tool results small enough for small local context windows. A list keeps as many items as
/// fit; an object keeps its fields and cuts its biggest list; anything else is cut as text. Every
/// cut says so, and how to get the rest, so nothing is lost silently.
pub fn trim_result(value: Value, max: usize) -> Value {
    if value.to_string().len() <= max {
        return value;
    }
    const NOTE: &str = "Narrow the request, or pass offset/limit where the tool takes them, to see the rest.";
    match value {
        Value::Array(items) => {
            let n = items.len();
            let kept = fit_items(items, max.saturating_sub(200));
            let k = kept.len();
            json!({ "items": kept, "note": format!("Result too long: items 1-{k} of {n} shown. {NOTE}") })
        }
        Value::Object(mut map) => {
            let biggest = map.iter().filter(|(_, v)| v.is_array()).max_by_key(|(_, v)| v.to_string().len()).map(|(k, v)| (k.clone(), v.to_string().len()));
            // Cut the biggest list when the other fields leave room for part of it.
            if let Some((key, size)) = biggest {
                let rest = Value::Object(map.clone()).to_string().len() - size;
                if rest + 300 < max {
                    if let Some(Value::Array(items)) = map.remove(&key) {
                        let n = items.len();
                        let kept = fit_items(items, max - rest - 300);
                        map.insert("truncated".into(), json!(format!("Result too long: {key} 1-{} of {n} shown. {NOTE}", kept.len())));
                        map.insert(key, Value::Array(kept));
                        return Value::Object(map);
                    }
                }
            }
            cut_text(&Value::Object(map).to_string(), max)
        }
        other => cut_text(&other.to_string(), max),
    }
}

/// The leading items whose JSON fits in `budget` characters.
fn fit_items(items: Vec<Value>, budget: usize) -> Vec<Value> {
    let mut used = 2;
    items
        .into_iter()
        .take_while(|v| {
            used += v.to_string().len() + 1;
            used <= budget
        })
        .collect()
}

fn cut_text(text: &str, max: usize) -> Value {
    let keep: String = text.chars().take(max.saturating_sub(120)).collect();
    json!({ "truncated": keep, "note": format!("Cut at {} of {} characters. Ask for less (a narrower query, or offset/limit where the tool takes them).", keep.len(), text.len()) })
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
        "suggestions" => Route::Suggestions,
        "thumbnails" => Route::Thumbnails,
        "youtube-videos" => Route::YouTubeVideos,
        "automation" => Route::Automation,
        "creative" => Route::Creative,
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
            let hits: Vec<_> = rows.into_iter().filter(|m| q.is_empty() || m.title.to_lowercase().contains(&q)).collect();
            let (offset, limit) = page_args(args, 40);
            let items: Vec<Value> = hits
                .iter()
                .skip(offset)
                .take(limit)
                .map(|m| json!({ "id": m.id, "title": m.title, "status": m.status, "author": m.author, "unread": m.unread }))
                .collect();
            Ok(ToolOutput::Value(paged(items, offset, hits.len())))
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
            let id = a("id").to_string();
            if q.is_empty() && id.is_empty() {
                return Err("Give the manga's title or its id".into());
            }
            if !id.is_empty() {
                let found = blocking({
                    let id = id.clone();
                    move || mv_core::library::open_readonly().ok().and_then(|c| mv_core::repo::manga::get(&c, &id).ok().flatten()).map(|m| m.title)
                })
                .await?;
                return match found {
                    Some(title) => nav(Route::MangaDetail { manga_id: id, source_id: None }, json!({ "ok": true, "opened": title })),
                    None => Err(format!("No manga with id \"{id}\". Load it with manga_chapters (source + id) first.")),
                };
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
        APP_GUIDE => Ok(ToolOutput::Value(crate::guide::guide(a("topic")))),
        "app_suggestions" => {
            let list = blocking(crate::suggest::collect).await?;
            Ok(ToolOutput::Value(Value::Array(list.iter().map(|s| json!({ "title": s.title, "detail": s.detail, "prompt": s.prompt })).collect())))
        }
        other => match registered(other) {
            Some(d) => (d.run)(args.clone()).await,
            // Provider-style tools (Studio's Video editor registers through `ext`).
            None => match crate::ext::run(other, args) {
                Some(f) => f.await,
                None => Err(unknown_tool_error(other)),
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
    fn trims_big_results_and_says_so() {
        let big = Value::Array((0..500).map(|i| json!({ "n": i, "pad": "x".repeat(50) })).collect());
        let t = trim_result(big, 6000);
        let kept = t["items"].as_array().unwrap().len();
        assert!(kept > 50 && kept < 100, "as many items as fit: {kept}");
        assert!(t.to_string().len() <= 6000);
        assert!(t["note"].as_str().unwrap().contains(&format!("1-{kept} of 500")));
        let small = json!([1, 2]);
        assert_eq!(trim_result(small.clone(), 6000), small);
        // An object keeps its fields and cuts its biggest list.
        let obj = json!({ "manga": "Solo", "clipCount": 300, "clips": (0..300).map(|i| format!("{i} | p1 | 0+3 | Auto | {}", "w".repeat(60))).collect::<Vec<_>>(), "layers": [] });
        let t = trim_result(obj, 6000);
        assert_eq!((t["manga"].as_str(), t["clipCount"].as_u64(), t["layers"].as_array().map(Vec::len)), (Some("Solo"), Some(300), Some(0)));
        let n = t["clips"].as_array().unwrap().len();
        assert!(n > 20 && t.to_string().len() <= 6000, "{n}");
        assert!(t["truncated"].as_str().unwrap().contains(&format!("clips 1-{n} of 300")));
        // Anything else is cut as text, with its full length.
        let t = trim_result(json!({ "text": "z".repeat(9000) }), 6000);
        assert!(t["truncated"].as_str().unwrap().len() < 6000 && t["note"].as_str().unwrap().contains("of 9011 characters"));
    }

    #[test]
    fn pages_say_where_the_next_one_starts() {
        let mut a = Map::new();
        assert_eq!(page_args(&a, 40), (0, 40));
        a.insert("offset".into(), json!(40));
        a.insert("limit".into(), json!("10"));
        assert_eq!(page_args(&a, 40), (40, 10));
        let p = paged((0..10).map(|i| json!(i)).collect(), 40, 55);
        assert_eq!((p["total"].as_u64(), p["shown"].as_str()), (Some(55), Some("41-50")));
        assert_eq!(p["next"], "5 more: call again with offset 50");
        assert!(paged(vec![json!(1)], 54, 55).get("next").is_none());
    }

    #[test]
    fn tools_come_in_groups_by_page() {
        assert_eq!(group_of("video_timeline"), Group::Video);
        assert_eq!(group_of("studio_youtube_package"), Group::Studio);
        assert_eq!(group_of("manga_download"), Group::Manga);
        assert_eq!(group_of("open_reader"), Group::Manga);
        assert_eq!(group_of("play_title"), Group::Media);
        assert_eq!(group_of("youtube_channel"), Group::Youtube);
        assert_eq!(group_of("navigate"), Group::Core);
        assert_eq!(page_groups(Some(&Route::Studio)), &[Group::Studio, Group::Video]);
        assert_eq!(page_groups(Some(&Route::AnimeHome)), &[Group::Media]);
        assert_eq!(page_groups(Some(&Route::MoviesHome)), &[Group::Media]);
        assert_eq!(page_groups(Some(&Route::Library)), &[Group::Manga]);
        assert_eq!(page_groups(None), &[Group::Manga]);
        assert!(page_groups(Some(&Route::Settings { tab: None })).is_empty());
    }

    #[test]
    fn a_scope_sends_its_groups_and_can_load_more() {
        let names = |s: &ToolScope| s.specs().into_iter().map(|t| t.name).collect::<Vec<_>>();
        let mut scope = ToolScope::new(Some(&Route::AnimeHome), &[], false);
        let n = names(&scope);
        assert!(n.contains(&"search_anime".into()) && n.contains(&"navigate".into()) && n.contains(&LOAD_TOOLS.into()));
        assert!(!n.contains(&"list_library".into()) && !n.contains(&RUN_TOOL.into()));
        let load = scope.specs().into_iter().find(|t| t.name == LOAD_TOOLS).unwrap();
        assert!(load.description.contains("manga (") && !load.description.contains("media ("));
        let mut a = Map::new();
        a.insert("groups".into(), json!(["manga", "bogus"]));
        let r = scope.load(&a).unwrap();
        assert_eq!(r["loaded"], json!(["manga"]));
        assert!(names(&scope).contains(&"list_library".into()));
        assert!(scope.load(&Map::new()).is_err());
        // The conversation's recent tools keep their groups.
        let mut m = mv_ai::AiMessage::new(mv_ai::AiRole::Tool, "{}");
        m.tool_name = Some("youtube_channel".into());
        assert!(ToolScope::new(Some(&Route::Library), &[m], false).groups().contains(&Group::Youtube));
        // Over MCP the list has run_tool, and loading returns signatures to call through it.
        let mut mcp = ToolScope::new(Some(&Route::Library), &[], true);
        assert!(names(&mcp).contains(&RUN_TOOL.into()));
        a.insert("groups".into(), json!("media"));
        let r = mcp.load(&a).unwrap();
        assert!(r["tools"].as_array().unwrap().iter().any(|t| t.as_str().unwrap().starts_with("search_anime(query: string)")));
        assert!(r["howToCall"].as_str().unwrap().contains(RUN_TOOL));
        // Everything loaded: no load_tools left to offer.
        let mut all = Map::new();
        all.insert("groups".into(), json!(Group::ALL.map(|g| g.key())));
        mcp.load(&all).unwrap();
        assert!(!names(&mcp).contains(&LOAD_TOOLS.into()));
    }

    fn spec(name: &str, description: &str) -> AiToolSpec {
        AiToolSpec { name: name.into(), description: description.into(), parameters: obj(json!({}), &[]) }
    }

    #[test]
    fn find_tools_ranks_by_name_then_description() {
        let specs = vec![
            spec("manga_download", "Download chapters of a manga through the app's queue."),
            spec("manga_downloads", "The download queue: progress and failed chapters."),
            spec("studio_export", "Export the full recap of a manga as one video, parts or reels."),
            spec("motion_compose", "Render a custom motion graphic video from a JSON spec."),
            spec("navigate", "Open a page of the app."),
            spec(FIND_TOOLS, "Searches tools."),
        ];
        let names = |q: &str| rank_tools(q, &specs).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(names("download chapters")[0], "manga_download");
        assert_eq!(names("export reels")[0], "studio_export");
        // Synonyms: what users say.
        assert_eq!(names("animated intro")[0], "motion_compose");
        assert_eq!(names("make a recap")[0], "studio_export");
        // An exact name wins outright; the meta tools never show.
        assert_eq!(names("navigate")[0], "navigate");
        assert!(!names("search tools").contains(&FIND_TOOLS.to_string()));
        assert!(names("zzzz qqqq").is_empty());
    }

    #[test]
    fn a_lean_scope_starts_small_and_grows_by_single_tools() {
        let mut scope = ToolScope::lean(&[], false);
        let names = |s: &ToolScope| s.specs().into_iter().map(|t| t.name).collect::<Vec<_>>();
        let n = names(&scope);
        for core in LEAN_CORE {
            assert!(n.contains(&core.to_string()), "{core}");
        }
        assert!(n.contains(&FIND_TOOLS.into()) && n.contains(&DESCRIBE_TOOL.into()));
        assert!(!n.contains(&LOAD_TOOLS.into()) && !n.contains(&"list_library".into()) && !n.contains(&"set_theme".into()));
        let mut a = Map::new();
        a.insert("query".into(), json!("search movies"));
        let r = scope.find(&a).unwrap();
        assert!(r["tools"][0].as_str().unwrap().starts_with("search_movies("), "{r}");
        assert!(names(&scope).contains(&"search_movies".into()));
        assert!(!names(&scope).contains(&"search_anime".into()) || r["tools"].to_string().contains("search_anime"), "only what was found");
        // Calling a tool directly adds just that tool.
        scope.add_for("list_library");
        assert!(names(&scope).contains(&"list_library".into()) && !names(&scope).contains(&"open_manga".into()));
        // describe_tool gives the schema; a made-up name gets suggestions.
        let mut d = Map::new();
        d.insert("name".into(), json!("mangavox.open_settings"));
        let r = scope.describe(&d).unwrap();
        assert_eq!(r["parameters"]["required"], json!(["tab"]));
        d.insert("name".into(), json!("open_setting"));
        assert!(scope.describe(&d).unwrap_err().contains("open_settings"));
        assert!(scope.find(&Map::new()).is_err());
        // The conversation's recent tools stay available.
        let mut m = mv_ai::AiMessage::new(mv_ai::AiRole::Tool, "{}");
        m.tool_name = Some("get_watch_history".into());
        assert!(names(&ToolScope::lean(&[m], false)).contains(&"get_watch_history".into()));
        // Over MCP the result says how to call tools the client does not list.
        let mut mcp = ToolScope::lean(&[], true);
        assert!(mcp.find(&a).unwrap()["howToCall"].as_str().unwrap().contains(RUN_TOOL));
    }

    #[tokio::test]
    async fn app_guide_explains_the_flow() {
        let mut a = Map::new();
        a.insert("topic".into(), json!("studio"));
        let ToolOutput::Value(v) = execute(APP_GUIDE, &a).await.unwrap() else { panic!() };
        assert!(v["steps"].to_string().contains("studio_run_steps"));
        assert_eq!(risk_of(APP_GUIDE), Some(Risk::Read));
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
