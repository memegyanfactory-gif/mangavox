//! Movie, series and anime tools for the assistant (registered in `ui_assistant::tools` at
//! startup): list a title's stream sources, play one in the app's player (the same path as the
//! detail pages' Play button), and download one in the background into the streaming cache.
use std::sync::Mutex;

use futures::FutureExt as _;
use mv_media::types::AnimeTorrentStream;
use mv_ui::nav::PlayRequest;
use serde_json::{json, Map, Value};
use ui_assistant::tools::{self as at, num_arg, str_arg, Phrases, Risk, ToolDef, ToolOutput, ToolResult};
use ui_player::logic::SourceKind;

use crate::remote;

/// Sources listed per title.
const LIST: usize = 8;

/// Downloads the assistant started: (title, info hash).
static STARTED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

pub fn register() {
    at::register(tools());
}

fn title_schema() -> Value {
    json!({
        "kind": { "type": "string", "enum": ["movie", "series", "anime"] },
        "imdbId": { "type": "string", "description": "IMDb id (tt…) from search_movies, for movies and series" },
        "malId": { "type": "string", "description": "MyAnimeList id from search_anime, for anime" },
        "title": { "type": "string" },
        "season": { "type": "number" },
        "episode": { "type": "number" },
        "pick": { "type": "number", "description": "Index from media_sources (default: the best one)" },
    })
}

fn tools() -> Vec<ToolDef> {
    let schema = |required: &[&str]| json!({ "type": "object", "properties": title_schema(), "required": required });
    vec![
        ToolDef::new(
            "media_sources",
            "Stream sources for a movie, a series episode or an anime episode (quality, size, seeders), best first. Use the index as `pick` in media_play or media_download.",
            schema(&["kind", "title"]),
            Risk::Read,
        )
        .phrases(|a| Phrases::new("Find sources for", "Finding sources for", "Found sources for", &label(a)))
        .run(|a| async move { list(a).await }.boxed()),
        ToolDef::new("media_play", "Play a movie, series episode or anime episode in the app's player (the best source, or `pick`).", schema(&["kind", "title"]), Risk::Act)
            .phrases(|a| Phrases::new("Play", "Starting", "Started", &label(a)))
            .run(|a| async move { play(a).await }.boxed()),
        ToolDef::new(
            "media_download",
            "Download a movie or episode in the background (into MangaVox's streaming cache, so it plays instantly later). Usually several GB, so the user is asked first. Progress: media_downloads.",
            schema(&["kind", "title"]),
            Risk::Destructive,
        )
        .phrases(|a| Phrases::new("Download", "Starting the download of", "Started downloading", &label(a)))
        .run(|a| async move { download(a).await }.boxed()),
        ToolDef::new("media_downloads", "Progress of the movie and episode downloads the assistant started.", json!({ "type": "object", "properties": {}, "required": [] }), Risk::Read)
            .phrases(|_| Phrases::new("Check", "Checking", "Checked", "video downloads"))
            .run(|_| async move { progress().await }.boxed()),
    ]
}

fn label(a: &Map<String, Value>) -> String {
    let t = str_arg(a, "title");
    let t = if t.is_empty() { "the title" } else { t };
    match (num_arg(a, "season"), num_arg(a, "episode")) {
        (Some(s), Some(e)) => format!("{t} S{:02}E{:02}", s as u32, e as u32),
        (None, Some(e)) => format!("{t} episode {}", e as u32),
        _ => t.to_string(),
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

fn stream_json(i: usize, s: &AnimeTorrentStream) -> Value {
    json!({ "pick": i, "name": s.title.lines().next().unwrap_or(""), "quality": s.quality, "size": s.size.map(gb), "seeders": s.seeders, "from": s.source })
}

async fn sources_for(a: &Map<String, Value>) -> Result<Vec<(PlayRequest, AnimeTorrentStream)>, String> {
    let title = str_arg(a, "title").to_string();
    let season = num_arg(a, "season").map(|n| n as u32);
    let episode = num_arg(a, "episode").map(|n| n as u32);
    match str_arg(a, "kind") {
        "anime" => {
            let mal = str_arg(a, "malId").to_string();
            if mal.is_empty() || !mal.chars().all(|c| c.is_ascii_digit()) {
                return Err("malId (from search_anime) is required for anime".into());
            }
            remote::anime_sources(title, mal, episode).await
        }
        kind => {
            let imdb = str_arg(a, "imdbId").to_string();
            if !imdb.starts_with("tt") {
                return Err("imdbId (tt…, from search_movies) is required".into());
            }
            remote::sources(title, imdb, kind != "series", season, episode).await
        }
    }
}

fn chosen(a: &Map<String, Value>, list: Vec<(PlayRequest, AnimeTorrentStream)>) -> Result<(PlayRequest, AnimeTorrentStream), String> {
    let n = list.len();
    let i = num_arg(a, "pick").map(|p| p as usize).unwrap_or(0);
    list.into_iter().nth(i).ok_or_else(|| if n == 0 { "No source found".to_string() } else { format!("pick must be below {n}") })
}

async fn list(a: Map<String, Value>) -> ToolResult {
    let list = sources_for(&a).await?;
    Ok(ToolOutput::Value(json!({ "title": label(&a), "sources": list.iter().take(LIST).enumerate().map(|(i, (_, s))| stream_json(i, s)).collect::<Vec<_>>() })))
}

async fn play(a: Map<String, Value>) -> ToolResult {
    let (req, s) = chosen(&a, sources_for(&a).await?)?;
    let title = req.title.clone();
    ui_assistant::on_ui(move |w, cx| remote::play_now(req, w, cx)).await?;
    Ok(ToolOutput::Value(json!({ "ok": true, "playing": title, "source": stream_json(0, &s) })))
}

async fn download(a: Map<String, Value>) -> ToolResult {
    let (req, s) = chosen(&a, sources_for(&a).await?)?;
    let SourceKind::Torrent(src) = ui_player::logic::classify_source(&req.source) else {
        return Err("This source streams from a link and can't be downloaded; play it instead.".into());
    };
    let engine = ui_player::backend::engine().await.map_err(|e| format!("The download engine did not start: {e:#}"))?;
    let added = engine.add(src, mv_torrent::AddOptions { file_idx: req.file_idx, ..Default::default() }).await.map_err(|e| format!("Could not start the download: {e:#}"))?;
    let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
    started.retain(|(_, h)| *h != added.info_hash);
    started.push((req.title.clone(), added.info_hash.clone()));
    Ok(ToolOutput::Value(json!({ "ok": true, "title": req.title, "size": s.size.map(gb), "note": "Downloading in the background; it plays from the download when opened." })))
}

async fn progress() -> ToolResult {
    let started = STARTED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(engine) = ui_player::backend::engine_if_started() else {
        return Ok(ToolOutput::Value(json!({ "downloads": [] })));
    };
    let rows: Vec<Value> = started
        .iter()
        .map(|(title, hash)| match engine.stats(hash) {
            Ok(st) => json!({ "title": title, "progress": (st.progress * 100.).round(), "done": st.finished, "downloaded": gb(st.downloaded), "total": gb(st.total), "peers": st.peers, "speedMBs": (st.download_speed / 1e6 * 10.).round() / 10., "error": st.error }),
            Err(_) => json!({ "title": title, "state": "stopped" }),
        })
        .collect();
    Ok(ToolOutput::Value(json!({ "downloads": rows })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_name_the_episode() {
        let mut a = Map::new();
        a.insert("title".into(), json!("Dark"));
        assert_eq!(label(&a), "Dark");
        a.insert("season".into(), json!(2));
        a.insert("episode".into(), json!(5));
        assert_eq!(label(&a), "Dark S02E05");
        a.remove("season");
        assert_eq!(label(&a), "Dark episode 5");
        assert_eq!(gb(4_250_000_000), "4.2 GB");
    }

    #[tokio::test]
    async fn ids_are_checked_before_any_lookup() {
        let mut a = Map::new();
        a.insert("kind".into(), json!("anime"));
        a.insert("title".into(), json!("x"));
        assert!(sources_for(&a).await.unwrap_err().contains("malId"));
        a.insert("kind".into(), json!("movie"));
        a.insert("imdbId".into(), json!("12"));
        assert!(sources_for(&a).await.unwrap_err().contains("imdbId"));
    }
}
