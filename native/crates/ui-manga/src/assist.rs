//! The manga tools for the assistant (registered in `ui_assistant::tools` at startup): search the
//! sources, read a title's chapters, add it to the Library, download chapters through the same
//! app-wide queue as the detail page (so the Downloads page and the progress toast show them),
//! and open the reader.
use std::time::Duration;

use futures::FutureExt as _;
use mv_core::repo::chapter::{self, Chapter};
use mv_core::repo::manga::{self, Manga};
use mv_sources::MangaSearchResult;
use mv_ui::Route;
use serde_json::{json, Map, Value};
use ui_assistant::chapters::{self, compress};
use ui_assistant::tools::{self as at, bool_arg, list_arg, str_arg, Phrases, Risk, ToolDef, ToolOutput, ToolResult};

use crate::downloads::{downloads, ChapterDl, Job};
use crate::{backend, logic, ops};

/// More chapters than this in one download asks the user first.
pub const BIG_DOWNLOAD: usize = 20;
/// Sources searched at once when the call names none.
const SEARCH_SOURCES: usize = 4;

pub fn register() {
    at::register(tools());
}

fn s(d: &str) -> Value {
    json!({ "type": "string", "description": d })
}

fn obj(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn chapters_schema() -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": "Exactly the chapters the user asked for: numbers (\"12\"), ranges (\"12-15\") or chapter ids. \"all\" only when the user said all." })
}

fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef::new(
            "manga_search",
            "Search the manga sources for a title. Returns source + id pairs to pass to manga_chapters. Without `source`, the first few enabled sources are searched.",
            obj(json!({ "query": s("Title or keywords"), "source": s("Optional source id (manga_sources)") }), &["query"]),
            Risk::Read,
        )
        .phrases(|a| Phrases::new("Search manga sources for", "Searching manga sources for", "Searched manga sources for", &format!("“{}”", str_arg(a, "query"))))
        .run(|a| async move { search(a).await }.boxed()),
        ToolDef::new("manga_sources", "The manga sources that can be searched (id, name, language).", obj(json!({}), &[]), Risk::Read)
            .phrases(|_| Phrases::new("List", "Listing", "Listed", "manga sources"))
            .run(|_| async move { sources().await }.boxed()),
        ToolDef::new(
            "manga_chapters",
            "A manga's chapters with their download state. Pass `manga` (a Library title or id), or `source` + `id` from manga_search (this loads the title into the app without adding it to the Library). `refresh` re-reads the chapter list from the source.",
            obj(json!({ "manga": s("Library title or manga id"), "source": s("Source id from manga_search"), "id": s("The title's id on that source"), "title": s("Title (with source + id)"), "refresh": { "type": "boolean" } }), &[]),
            Risk::Read,
        )
        .phrases(|a| Phrases::new("Read the chapters of", "Reading the chapters of", "Read the chapters of", &name_of(a)))
        .run(|a| async move { chapters_tool(a).await }.boxed()),
        ToolDef::new("manga_add_to_library", "Add a manga (its manga id from manga_chapters) to the Library.", obj(json!({ "manga_id": s("Manga id") }), &["manga_id"]), Risk::Act)
            .phrases(|_| Phrases::new("Add", "Adding", "Added", "the manga to your Library"))
            .run(|a| async move { add_to_library(a).await }.boxed()),
        ToolDef::new(
            "manga_download",
            "Download chapters of a manga for offline reading and Studio, through the app's download queue (progress shows on the Downloads page). `manga` is a manga id from manga_chapters or a Library title. Big downloads ask the user first.",
            obj(json!({ "manga": s("Manga id or Library title"), "chapters": chapters_schema() }), &["manga", "chapters"]),
            Risk::Act,
        )
        .risk_for(|a| if is_big(a) { Risk::Destructive } else { Risk::Act })
        .phrases(|a| Phrases::new("Download", "Queueing downloads of", "Queued downloads of", &format!("{} of {}", chapters_text(a), name_of(a))))
        .run(|a| async move { download(a).await }.boxed()),
        ToolDef::new("manga_downloads", "The download queue: what is downloading or waiting (with progress), failed chapters and whether it is paused.", obj(json!({}), &[]), Risk::Read)
            .phrases(|_| Phrases::new("Check", "Checking", "Checked", "the downloads"))
            .run(|_| async move { queue_status().await }.boxed()),
        ToolDef::new(
            "manga_downloads_control",
            "Pause or resume the download queue, retry the failed chapters, or cancel the downloads of one manga (`manga`) or all of them.",
            obj(json!({ "action": { "type": "string", "enum": ["pause", "resume", "retry_failed", "cancel_manga", "cancel_all"] }, "manga": s("Manga id or Library title (cancel_manga)") }), &["action"]),
            Risk::Act,
        )
        .phrases(|a| match str_arg(a, "action") {
            "pause" => Phrases::new("Pause", "Pausing", "Paused", "the downloads"),
            "resume" => Phrases::new("Resume", "Resuming", "Resumed", "the downloads"),
            "retry_failed" => Phrases::new("Retry", "Retrying", "Retried", "the failed downloads"),
            _ => Phrases::new("Cancel", "Cancelling", "Cancelled", "downloads"),
        })
        .run(|a| async move { control(a).await }.boxed()),
        ToolDef::new(
            "manga_delete_downloads",
            "Delete downloaded chapters of a manga from disk (pages and their Studio work). Always asks the user first.",
            obj(json!({ "manga": s("Manga id or Library title"), "chapters": chapters_schema() }), &["manga", "chapters"]),
            Risk::Destructive,
        )
        .phrases(|a| Phrases::new("Delete the downloads of", "Deleting the downloads of", "Deleted the downloads of", &format!("{} of {}", chapters_text(a), name_of(a))))
        .run(|a| async move { delete(a).await }.boxed()),
        ToolDef::new("open_reader", "Open the reader on a chapter of a manga in the app (Library title or manga id; the chapter number or id).", obj(json!({ "manga": s("Manga id or Library title"), "chapter": s("Chapter number or id") }), &["manga", "chapter"]), Risk::Act)
            .phrases(|a| Phrases::new("Open", "Opening", "Opened", &format!("Ch. {} of {}", str_arg(a, "chapter"), name_of(a))))
            .run(|a| async move { open_reader(a).await }.boxed()),
    ]
}

fn name_of(a: &Map<String, Value>) -> String {
    [str_arg(a, "title"), str_arg(a, "manga"), str_arg(a, "manga_id")].into_iter().find(|s| !s.is_empty() && !chapters::looks_like_id(s)).unwrap_or("the manga").to_string()
}

fn chapters_text(a: &Map<String, Value>) -> String {
    chapters::describe(&list_arg(a, "chapters"))
}

/// How many chapters a spec list names at most (ranges counted by width; "all" counts as big).
pub fn spec_size(raw: &[String]) -> usize {
    chapters::parse(raw)
        .iter()
        .map(|s| match s {
            chapters::Spec::All => usize::MAX / 4,
            chapters::Spec::Range(a, b) => (b - a).floor() as usize + 1,
            _ => 1,
        })
        .sum()
}

fn is_big(a: &Map<String, Value>) -> bool {
    spec_size(&list_arg(a, "chapters")) > BIG_DOWNLOAD
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Result<T, String> {
    mv_ui::bg::blocking(f).await.map_err(|e| e.to_string())?.map_err(|e| format!("{e:#}"))
}

/// A manga in the database: its id, else the Library title (exact, then the one containing it).
fn find_manga(c: &rusqlite::Connection, q: &str) -> anyhow::Result<Manga> {
    let q = q.trim();
    if q.is_empty() {
        anyhow::bail!("Say which manga");
    }
    if let Some(m) = manga::get(c, q)? {
        return Ok(m);
    }
    let all = manga::list(c)?;
    let low = q.to_lowercase();
    if let Some(m) = all.iter().find(|m| m.title.to_lowercase() == low) {
        return Ok(m.clone());
    }
    let hits: Vec<&Manga> = all.iter().filter(|m| m.title.to_lowercase().contains(&low)).collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => anyhow::bail!("\"{q}\" is not in the app yet. Find it with manga_search, then load it with manga_chapters."),
        many => anyhow::bail!("\"{q}\" matches several manga: {}. Which one?", many.iter().map(|m| m.title.as_str()).collect::<Vec<_>>().join(", ")),
    }
}

/// The chapters of the manga's own source, one per number (the first listed wins), in order.
pub fn one_per_number(list: Vec<Chapter>) -> Vec<Chapter> {
    let mut seen: Vec<f64> = vec![];
    let mut out = vec![];
    for c in list {
        if let Some(n) = c.number {
            // A downloaded duplicate replaces an earlier one that isn't.
            if let Some(i) = seen.iter().position(|x| (x - n).abs() < 1e-6) {
                let prev: &mut Chapter = &mut out[i];
                if prev.download_status != "complete" && c.download_status == "complete" {
                    *prev = c;
                }
                continue;
            }
            seen.push(n);
        } else {
            seen.push(f64::NAN);
        }
        out.push(c);
    }
    out.sort_by(|a, b| a.number.unwrap_or(f64::MAX).total_cmp(&b.number.unwrap_or(f64::MAX)));
    out
}

fn chapter_list(c: &rusqlite::Connection, m: &Manga) -> anyhow::Result<Vec<Chapter>> {
    Ok(one_per_number(chapter::list_by_source(c, &m.id, &m.source_plugin)?))
}

/// The chapters `raw` names; an error names the ones that do not exist.
fn pick(m: &Manga, list: &[Chapter], raw: &[String]) -> Result<Vec<Chapter>, String> {
    if raw.is_empty() {
        return Err("Say which chapters".into());
    }
    let items: Vec<(&str, Option<f64>)> = list.iter().map(|c| (c.id.as_str(), c.number)).collect();
    chapters::pick(&items, raw).map(|ix| ix.into_iter().map(|i| list[i].clone()).collect()).map_err(|missing| {
        format!("{} of {} {} not in its chapter list ({}). Try manga_chapters with refresh.", missing.join(", "), m.title, if missing.len() == 1 { "is" } else { "are" }, compress(&list.iter().filter_map(|c| c.number).collect::<Vec<_>>()))
    })
}

// ---------------------------------------------------------------- tools

async fn sources() -> ToolResult {
    let list = mv_ui::bg::run(async { backend::installed_sources(backend::registry().await.list()) }).await.map_err(|e| e.to_string())?;
    Ok(ToolOutput::Value(Value::Array(list.into_iter().map(|s| json!({ "id": s.id, "name": s.name, "lang": s.lang, "adult": s.nsfw })).collect())))
}

fn result_json(source: &str, r: &MangaSearchResult) -> Value {
    json!({ "source": source, "id": r.source_id, "title": r.title, "author": r.author, "status": r.status, "adult": r.adult })
}

async fn search(a: Map<String, Value>) -> ToolResult {
    let q = str_arg(&a, "query").to_string();
    if q.is_empty() {
        return Err("query is required".into());
    }
    let wanted = str_arg(&a, "source").to_string();
    let reg = backend::registry().await;
    let ids: Vec<String> = if wanted.is_empty() {
        backend::installed_sources(reg.list()).into_iter().filter(|s| !s.nsfw).map(|s| s.id).take(SEARCH_SOURCES).collect()
    } else {
        vec![wanted]
    };
    if ids.is_empty() {
        return Err("No manga source is installed. Add one in Settings → Sources.".into());
    }
    let searches = ids.iter().map(|id| {
        let (id, q) = (id.clone(), q.clone());
        async move {
            let r = async {
                let src = backend::source(&id).await?;
                tokio::time::timeout(Duration::from_secs(20), src.search_manga(&q, 1)).await.map_err(|_| anyhow::anyhow!("timed out"))?.map_err(|e| anyhow::anyhow!("{e}"))
            }
            .await;
            (id, r)
        }
    });
    let mut rows = vec![];
    let mut failed = vec![];
    for (id, r) in futures::future::join_all(searches).await {
        match r {
            Ok(list) => rows.extend(list.iter().take(5).map(|x| result_json(&id, x))),
            Err(e) => failed.push(format!("{id}: {e}")),
        }
    }
    Ok(ToolOutput::Value(json!({ "results": rows, "failedSources": failed })))
}

async fn chapters_tool(a: Map<String, Value>) -> ToolResult {
    let (source, id) = (str_arg(&a, "source").to_string(), str_arg(&a, "id").to_string());
    let m: Manga = if !source.is_empty() && !id.is_empty() {
        // Already known to the app: use it, otherwise load it from the source (not added to the Library).
        let (s2, i2) = (source.clone(), id.clone());
        match blocking(move || backend::with_db(|c| Ok(manga::find_by_source(c, &s2, &i2)?))).await? {
            Some(m) if bool_arg(&a, "refresh") != Some(true) => m,
            _ => {
                let title = str_arg(&a, "title").to_string();
                let r = MangaSearchResult { source_id: id.clone(), title: if title.is_empty() { id.clone() } else { title }, alt_titles: None, cover_url: None, author: None, status: None, adult: false };
                mv_ui::bg::run(ops::prepare_from_source(source.clone(), r)).await.map_err(|e| e.to_string())?.map_err(|e| format!("Could not load it from {source}: {e:#}"))?
            }
        }
    } else {
        let q = str_arg(&a, "manga").to_string();
        let m = blocking(move || backend::with_db(|c| find_manga(c, &q))).await?;
        if bool_arg(&a, "refresh") == Some(true) {
            let id = m.id.clone();
            mv_ui::bg::run(ops::refresh_manga(id)).await.map_err(|e| e.to_string())?.map_err(|e| format!("Could not refresh: {e:#}"))?;
        }
        m
    };
    let mm = m.clone();
    let list = blocking(move || backend::with_db(|c| chapter_list(c, &mm))).await?;
    let in_library = !backend::browse_only().contains(&m.id);
    let rows: Vec<Value> = list
        .iter()
        .map(|c| json!({ "chapter": logic::chapter_short(c), "number": c.number, "id": c.id, "title": c.title, "downloaded": c.download_status == "complete", "read": c.is_read }))
        .collect();
    let downloaded: Vec<f64> = list.iter().filter(|c| c.download_status == "complete").filter_map(|c| c.number).collect();
    Ok(ToolOutput::Value(json!({
        "mangaId": m.id,
        "title": m.title,
        "source": m.source_plugin,
        "inLibrary": in_library,
        "count": list.len(),
        "downloaded": compress(&downloaded),
        "chapters": at::trim_result(Value::Array(rows), 5000),
    })))
}

async fn add_to_library(a: Map<String, Value>) -> ToolResult {
    let q = str_arg(&a, "manga_id").to_string();
    let m = blocking(move || backend::with_db(|c| find_manga(c, &q))).await?;
    backend::set_in_library(&m.id, true);
    backend::bump_data();
    Ok(ToolOutput::Value(json!({ "ok": true, "title": m.title })))
}

async fn download(a: Map<String, Value>) -> ToolResult {
    let q = str_arg(&a, "manga").to_string();
    let raw = list_arg(&a, "chapters");
    let (m, list) = blocking(move || {
        backend::with_db(|c| {
            let m = find_manga(c, &q)?;
            let list = chapter_list(c, &m)?;
            Ok((m, list))
        })
    })
    .await?;
    let picked = pick(&m, &list, &raw)?;
    let (have, todo): (Vec<Chapter>, Vec<Chapter>) = picked.into_iter().partition(|c| c.download_status == "complete");
    let jobs: Vec<Job> = todo.iter().map(|c| Job { manga_id: m.id.clone(), manga_title: m.title.clone(), chapter_id: c.id.clone(), label: logic::chapter_short(c) }).collect();
    let n = jobs.len();
    if n > 0 {
        ui_assistant::on_ui(move |_, cx| {
            let q = downloads(cx);
            q.update(cx, |d, cx| d.enqueue(jobs, cx));
        })
        .await?;
    }
    Ok(ToolOutput::Value(json!({
        "mangaId": m.id,
        "title": m.title,
        "queued": todo.iter().map(logic::chapter_short).collect::<Vec<_>>(),
        "alreadyDownloaded": have.iter().map(logic::chapter_short).collect::<Vec<_>>(),
        "note": if n > 0 { "Downloading in the background; check with manga_downloads. Studio sees each chapter once it is downloaded." } else { "Nothing to download." },
    })))
}

async fn queue_status() -> ToolResult {
    let v = ui_assistant::on_ui(|_, cx| {
        let q = downloads(cx);
        let d = q.read(cx);
        let rows: Vec<Value> = d
            .pending()
            .into_iter()
            .map(|(j, st)| match st {
                ChapterDl::Active(p) => json!({ "manga": j.manga_title, "chapter": j.label, "state": "downloading", "progress": (p * 100.).round() }),
                ChapterDl::Queued => json!({ "manga": j.manga_title, "chapter": j.label, "state": "queued" }),
            })
            .collect();
        let failed: Vec<Value> = d.failed_jobs().iter().map(|j| json!({ "manga": j.manga_title, "chapter": j.label, "error": d.error(&j.chapter_id) })).collect();
        json!({ "paused": d.is_paused(), "busy": d.is_busy(), "chapters": rows, "failed": failed })
    })
    .await?;
    Ok(ToolOutput::Value(v))
}

async fn control(a: Map<String, Value>) -> ToolResult {
    let action = str_arg(&a, "action").to_string();
    let manga_id = if action == "cancel_manga" {
        let q = str_arg(&a, "manga").to_string();
        Some(blocking(move || backend::with_db(|c| find_manga(c, &q))).await?.id)
    } else {
        None
    };
    let act = action.clone();
    ui_assistant::on_ui(move |_, cx| {
        let q = downloads(cx);
        q.update(cx, |d, cx| match act.as_str() {
            "pause" => d.set_paused(true, cx),
            "resume" => d.set_paused(false, cx),
            "retry_failed" => d.retry_failed(cx),
            "cancel_all" => d.cancel_all(cx),
            "cancel_manga" => d.cancel_manga(manga_id.as_deref().unwrap_or(""), cx),
            _ => {}
        });
    })
    .await?;
    if !["pause", "resume", "retry_failed", "cancel_manga", "cancel_all"].contains(&action.as_str()) {
        return Err(format!("Unknown action \"{action}\""));
    }
    queue_status().await
}

async fn delete(a: Map<String, Value>) -> ToolResult {
    let q = str_arg(&a, "manga").to_string();
    let raw = list_arg(&a, "chapters");
    let (m, list) = blocking(move || {
        backend::with_db(|c| {
            let m = find_manga(c, &q)?;
            let list = chapter_list(c, &m)?;
            Ok((m, list))
        })
    })
    .await?;
    let picked: Vec<Chapter> = pick(&m, &list, &raw)?.into_iter().filter(|c| c.download_status == "complete").collect();
    if picked.is_empty() {
        return Err("None of these chapters are downloaded".into());
    }
    let labels: Vec<String> = picked.iter().map(logic::chapter_short).collect();
    let pairs: Vec<(String, String)> = picked.iter().map(|c| (m.id.clone(), c.id.clone())).collect();
    ui_assistant::on_ui(move |_, cx| {
        let q = downloads(cx);
        q.update(cx, |d, cx| d.delete(pairs, false, cx));
    })
    .await?;
    Ok(ToolOutput::Value(json!({ "title": m.title, "deleted": labels })))
}

async fn open_reader(a: Map<String, Value>) -> ToolResult {
    let q = str_arg(&a, "manga").to_string();
    let want = str_arg(&a, "chapter").to_string();
    let (m, list) = blocking(move || {
        backend::with_db(|c| {
            let m = find_manga(c, &q)?;
            let list = chapter_list(c, &m)?;
            Ok((m, list))
        })
    })
    .await?;
    let ch = pick(&m, &list, &[want])?.into_iter().next().ok_or("No such chapter")?;
    Ok(ToolOutput::Navigate(Route::Reader { manga_id: m.id.clone(), chapter_id: ch.id.clone() }, json!({ "ok": true, "opened": format!("{} {}", m.title, logic::chapter_short(&ch)) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, n: Option<f64>, done: bool) -> Chapter {
        Chapter {
            id: id.into(),
            manga_id: "m".into(),
            source_plugin: "s".into(),
            source_id: id.into(),
            number: n,
            title: None,
            language: "en".into(),
            page_count: None,
            download_status: if done { "complete".into() } else { "none".into() },
            ocr_status: String::new(),
            tts_status: String::new(),
            video_status: String::new(),
            read_progress: 0,
            is_read: false,
            pages_dir: None,
            added_at: String::new(),
        }
    }

    #[test]
    fn duplicates_keep_one_chapter_per_number() {
        let list = one_per_number(vec![c("a", Some(2.), false), c("b", Some(1.), false), c("c", Some(2.), true), c("d", None, false)]);
        let ids: Vec<&str> = list.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["b", "c", "d"]);
    }

    #[test]
    fn big_downloads_are_counted_by_range() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(spec_size(&s(&["1-10", "12"])), 11);
        assert!(spec_size(&s(&["1-30"])) > BIG_DOWNLOAD);
        assert!(spec_size(&s(&["all"])) > BIG_DOWNLOAD);
        let mut a = Map::new();
        a.insert("chapters".into(), json!(["1-5"]));
        assert!(!is_big(&a));
        a.insert("chapters".into(), json!(["1-21"]));
        assert!(is_big(&a));
    }

    #[test]
    fn picking_names_missing_chapters() {
        let m = Manga {
            id: "m".into(),
            source_plugin: "s".into(),
            source_id: "m".into(),
            title: "Solo".into(),
            author: None,
            artist: None,
            description: None,
            cover_path: None,
            status: None,
            genres: vec![],
            is_favorite: false,
            notes: None,
            added_at: String::new(),
            last_read_at: None,
            chapter_count: None,
            unread_count: None,
        };
        let list = vec![c("a", Some(1.), false), c("b", Some(2.), false)];
        assert_eq!(pick(&m, &list, &["2".into()]).unwrap()[0].id, "b");
        let e = pick(&m, &list, &["2".into(), "3".into()]).unwrap_err();
        assert!(e.starts_with("3 of Solo is not in its chapter list (1-2)"), "{e}");
    }
}
