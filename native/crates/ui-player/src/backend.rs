//! Non-UI IO for the player: torrent engine, resume/progress persistence, skip times, subtitles.
//! Everything here is `Send` and meant to run through `mv_ui::bg`.
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, Result};
use mv_core::repo::media::{self, MediaEvent, MediaTitle};
use mv_media::addons::AddonRegistry;
use mv_media::stream_utils::{has_playable_link, stream_resource_id};
use mv_media::stremio::StremioClient;
use mv_media::types::{SkipSegment, StremioSubtitle};
use mv_torrent::{AddOptions, Engine, EngineConfig};
use mv_ui::nav::PlayRequest;
use tokio::sync::OnceCell;

use crate::logic::{self, SourceKind};

/// Public Stremio subtitles addon.
const SUBTITLES_ENDPOINT: &str = "https://opensubtitles-v3.strem.io";

static ENGINE: OnceCell<Arc<Engine>> = OnceCell::const_new();

/// The shared torrent engine, started lazily on first use.
pub async fn engine() -> Result<Arc<Engine>> {
    ENGINE
        .get_or_try_init(|| async {
            let dir: PathBuf = mv_core::paths::root().join("cache").join("torrents");
            std::fs::create_dir_all(&dir)?;
            Ok::<_, anyhow::Error>(Arc::new(Engine::new(EngineConfig::new(dir)).await?))
        })
        .await
        .cloned()
}

/// The engine if it has already started (non-blocking, for stats polling).
pub fn engine_if_started() -> Option<Arc<Engine>> {
    ENGINE.get().cloned()
}

/// What mpv should load.
pub struct Resolved {
    pub target: String,
    /// Set for torrents so the UI can show stats and release the torrent on exit.
    pub info_hash: Option<String>,
}

/// Turns a `PlayRequest.source` into something mpv can open. Torrents wait for metadata.
pub async fn resolve(req: &PlayRequest) -> Result<Resolved> {
    match logic::classify_source(&req.source) {
        SourceKind::Url(u) => Ok(Resolved { target: u, info_hash: None }),
        SourceKind::File(p) => {
            if !std::path::Path::new(&p).exists() {
                return Err(anyhow!("File not found: {p}"));
            }
            Ok(Resolved { target: p, info_hash: None })
        }
        SourceKind::Torrent(src) => {
            let engine = engine().await?;
            let added = engine
                .add(src, AddOptions { file_idx: req.file_idx, ..Default::default() })
                .await?;
            Ok(Resolved { target: added.stream_url, info_hash: Some(added.info_hash) })
        }
    }
}

/// Release a streamed torrent (files stay in the cache for quick resume).
pub fn release_torrent(info_hash: String) {
    if let Some(engine) = engine_if_started() {
        mv_ui::bg::runtime().spawn(async move {
            let _ = engine.remove(&info_hash, false).await;
        });
    }
}

/// Identity of the title being played, matching the Electron app's media ids.
#[derive(Clone, Debug)]
pub struct Ident {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub imdb_id: Option<String>,
    pub mal_id: Option<i64>,
    pub episode: Option<i64>,
}

pub fn ident(req: &PlayRequest) -> Ident {
    let mal = req.mal_id.as_deref().and_then(|m| m.parse::<i64>().ok());
    let imdb = req.imdb_id.clone().filter(|s| !s.is_empty());
    let (kind, id) = if req.mal_id.is_some() {
        ("anime", format!("anime:mal:{}", req.mal_id.clone().unwrap_or_default()))
    } else if req.season.is_some() || req.episode.is_some() {
        ("series", format!("series:imdb:{}", imdb.clone().unwrap_or_else(|| req.title.clone())))
    } else {
        ("movie", format!("movie:imdb:{}", imdb.clone().unwrap_or_else(|| req.title.clone())))
    };
    Ident {
        id,
        kind,
        title: req.title.clone(),
        imdb_id: if kind == "anime" { None } else { imdb },
        mal_id: if kind == "anime" { mal } else { None },
        episode: req.episode.map(|e| e as i64),
    }
}

/// Saved resume position for this title/episode, if worth resuming.
pub fn load_resume(ident: &Ident) -> Option<f64> {
    let conn = mv_core::db::open_default().ok()?;
    let rows = media::list_history(&conn, Some(ident.kind), None, 1000).ok()?;
    let row = rows.into_iter().find(|r| r.media_id == ident.id)?;
    if row.episode_number != ident.episode {
        return None;
    }
    logic::resume_position(row.position_secs, row.duration_secs, row.completed)
}

/// `kind` is `play`, `progress` or `complete`.
pub fn save_event(ident: &Ident, kind: &str, pos: f64, duration: f64) -> Result<()> {
    let conn = mv_core::db::open_default()?;
    // Never overwrite catalog metadata (genres, cover) with our sparse title.
    let title = match media::get_title(&conn, &ident.id)? {
        Some(_) => None,
        None => Some(MediaTitle {
            id: ident.id.clone(),
            kind: ident.kind.to_string(),
            title: ident.title.clone(),
            imdb_id: ident.imdb_id.clone(),
            mal_id: ident.mal_id,
            ..Default::default()
        }),
    };
    media::record_event(
        &conn,
        &MediaEvent {
            media_id: Some(ident.id.clone()),
            kind: ident.kind.to_string(),
            event_type: kind.to_string(),
            title,
            episode_number: ident.episode,
            position_secs: Some(pos),
            duration_secs: Some(duration),
            ..Default::default()
        },
    )?;
    Ok(())
}

/// AniSkip segments (empty when unknown).
pub async fn skip_segments(mal_id: u64, episode: u32, duration: f64) -> Result<Vec<SkipSegment>> {
    let client = mv_media::skip_times::SkipTimesClient::new()?;
    Ok(client.get(mal_id, episode as f64, duration).await?.segments)
}

/// External subtitles for an IMDb id (+ season/episode), capped to a sensible number.
pub async fn subtitles(imdb: String, season: Option<u32>, episode: Option<u32>) -> Result<Vec<StremioSubtitle>> {
    let (id, kind) = logic::stremio_subtitle_id(&imdb, season, episode);
    let client = StremioClient::new()?;
    let mut subs = client.fetch_subtitles(SUBTITLES_ENDPOINT, &id, kind).await?;
    subs.truncate(24);
    Ok(subs)
}

/// Whether `next_episode` can look anything up for this request.
pub fn has_next(req: &PlayRequest) -> bool {
    req.episode.is_some() && (req.imdb_id.as_deref().is_some_and(|s| !s.is_empty()) || req.mal_id.is_some())
}

/// The following episode's request, using the first playable stream from installed addons.
pub async fn next_episode(req: PlayRequest) -> Result<PlayRequest> {
    static REG: OnceLock<AddonRegistry> = OnceLock::new();
    let ep = req.episode.ok_or_else(|| anyhow!("No episode number"))?;
    let imdb = match req.imdb_id.clone().filter(|s| !s.is_empty()) {
        Some(i) => i,
        None => {
            let mal = req.mal_id.clone().ok_or_else(|| anyhow!("No title id"))?;
            mv_media::jikan::JikanClient::shared()?.imdb_id(&mal).await.ok_or_else(|| anyhow!("This title has no IMDb mapping"))?
        }
    };
    let rid = stream_resource_id(&imdb, false, req.season.unwrap_or(1), ep + 1).ok_or_else(|| anyhow!("Invalid title id"))?;
    let reg = REG.get_or_init(|| AddonRegistry::open(mv_core::paths::root().join("addons")));
    let streams = reg.find_streams(&rid, "series").await;
    let s = streams.iter().find(|s| has_playable_link(s)).ok_or_else(|| anyhow!("No sources found for episode {}", ep + 1))?;
    Ok(PlayRequest {
        title: logic::next_title(&req.title, ep),
        source: logic::playable_source(&s.info_hash),
        file_idx: s.file_idx.map(|i| i as usize),
        imdb_id: Some(imdb),
        mal_id: req.mal_id,
        season: req.season,
        episode: Some(ep + 1),
    })
}
