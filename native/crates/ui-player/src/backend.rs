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

/// The shared torrent engine, started on first use (or by [`warm_up`] at app start).
pub async fn engine() -> Result<Arc<Engine>> {
    ENGINE
        .get_or_try_init(|| async {
            let t = std::time::Instant::now();
            let dir: PathBuf = mv_core::paths::root().join("cache").join("torrents");
            std::fs::create_dir_all(&dir)?;
            let engine = Engine::new(EngineConfig::new(dir)).await;
            match &engine {
                Ok(_) => mv_core::logging::global().info("torrent", &format!("torrent engine started in {} ms", t.elapsed().as_millis())),
                Err(e) => mv_core::logging::global().warn("torrent", &format!("torrent engine failed to start: {e:#}")),
            }
            Ok::<_, anyhow::Error>(Arc::new(engine?))
        })
        .await
        .cloned()
}

/// Starts the torrent engine in the background (DHT bootstrap, trackers, cache restore), so the
/// first Play of the session does not pay for it. Cheap to call more than once.
pub fn warm_up() {
    if ENGINE.get().is_some() {
        return;
    }
    mv_ui::bg::runtime().spawn(async {
        let _ = engine().await;
    });
}

/// Sources to fall back on for the next player opening `primary` (set by the page it is opened
/// from, which knows the ranked list).
static ALTERNATES: std::sync::Mutex<Option<(String, Vec<PlayRequest>)>> = std::sync::Mutex::new(None);

/// Offers the player other sources for `primary`, best first: `ranked` is the page's whole ranked
/// list `(request, seeders)`; the playing source, dead torrents and repeats are dropped.
pub fn offer_alternates(primary: &PlayRequest, ranked: Vec<(PlayRequest, Option<u64>)>) {
    let alts = logic::pick_alternates(primary, ranked, MAX_ALTERNATES);
    *ALTERNATES.lock().unwrap_or_else(|e| e.into_inner()) = Some((logic::source_key(&primary.source), alts));
}

/// At most this many automatic source switches per Play.
const MAX_ALTERNATES: usize = 4;

/// The alternates offered for `req` (once).
pub fn take_alternates(req: &PlayRequest) -> Vec<PlayRequest> {
    let mut g = ALTERNATES.lock().unwrap_or_else(|e| e.into_inner());
    match g.take() {
        Some((key, alts)) if key == logic::source_key(&req.source) => alts,
        _ => vec![],
    }
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

/// Start warming a torrent source in the background so Play opens it instantly.
/// URLs and files need no warm-up; errors are ignored (it is only a head start).
pub fn prefetch(req: &PlayRequest) {
    let SourceKind::Torrent(src) = logic::classify_source(&req.source) else { return };
    let file_idx = req.file_idx;
    mv_ui::bg::runtime().spawn(async move {
        if let Ok(engine) = engine().await {
            let _ = engine.prefetch(src, AddOptions { file_idx, ..Default::default() });
        }
    });
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
    /// A catalog title (IMDb or MAL id). A local file such as a Studio recap video has neither,
    /// so it is kept out of watch history and Continue Watching.
    pub tracked: bool,
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
    let tracked = imdb.is_some() || req.mal_id.as_deref().is_some_and(|m| !m.is_empty());
    Ident {
        id,
        kind,
        title: req.title.clone(),
        imdb_id: if kind == "anime" { None } else { imdb },
        mal_id: if kind == "anime" { mal } else { None },
        episode: req.episode.map(|e| e as i64),
        tracked,
    }
}

/// Saved resume position for this title/episode, if worth resuming.
pub fn load_resume(ident: &Ident) -> Option<f64> {
    if !ident.tracked {
        return None;
    }
    let conn = mv_core::db::open_default().ok()?;
    let rows = media::list_history(&conn, Some(ident.kind), None, 1000).ok()?;
    let row = rows.into_iter().find(|r| r.media_id == ident.id)?;
    if row.episode_number != ident.episode {
        return None;
    }
    logic::resume_position(row.position_secs, row.duration_secs, row.completed)
}

/// Whether playback of `ident` goes into watch history, Continue Watching and resume points:
/// catalog titles only, and never in private mode.
pub fn records_watch(ident: &Ident, private: bool) -> bool {
    ident.tracked && !private
}

/// `kind` is `play`, `progress` or `complete`.
pub fn save_event(ident: &Ident, kind: &str, pos: f64, duration: f64) -> Result<()> {
    if !records_watch(ident, mv_core::privacy::active()) {
        return Ok(());
    }
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
    crate::bump_watch_epoch();
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

/// Settings → Playback (auto-play, countdown, skip intro, pre-download, subtitle language).
pub fn load_prefs() -> logic::PlaybackPrefs {
    let all = mv_core::config::Config::open_default().get_all();
    let mut prefs = logic::PlaybackPrefs::from_config(all.get("playback"), all.get("playback.skipIntro"), all.get("playback.subtitleLanguage"));
    prefs.memory = logic::PlayerMemory::from_json(all.get(MEMORY_KEY));
    prefs
}

const MEMORY_KEY: &str = "player.memory";

/// Remembers the viewer's audio / subtitle language, volume and speed for the next video.
pub fn save_memory(memory: &logic::PlayerMemory) {
    let _ = mv_core::config::Config::open_default().set(MEMORY_KEY, memory.to_json());
}

/// Info hash of a torrent source before it is added, so the loader can show peer stats while
/// metadata is still arriving. Not used to release the torrent.
pub fn source_hash(req: &PlayRequest) -> Option<String> {
    match logic::classify_source(&req.source) {
        SourceKind::Torrent(src) => src.info_hash().ok(),
        _ => None,
    }
}

/// Backdrop for the loading screen: the metahub background for IMDb titles, the AniList / MAL
/// banner (or cover) for anime. Apple HIG Loading: give people something meaningful to look at.
pub async fn artwork(req: PlayRequest) -> Option<String> {
    if let Some(id) = req.imdb_id.as_deref().filter(|s| s.starts_with("tt")) {
        return Some(format!("https://images.metahub.space/background/medium/{id}/img"));
    }
    let mal = req.mal_id.filter(|m| !m.is_empty())?;
    let a = mv_media::jikan::JikanClient::shared().ok()?.anime_details(&mal).await.ok()?;
    a.banner_url.or(a.cover_url)
}

/// What the Up Next card shows about the following episode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NextInfo {
    pub title: Option<String>,
    pub still: Option<String>,
}

/// Name and still of the episode after `req` (anime through Jikan + Kitsu stills, series
/// through Cinemeta). `None` when nothing is known; the card then falls back to the artwork.
pub async fn next_info(req: PlayRequest) -> Option<NextInfo> {
    let next = req.episode? + 1;
    if let Some(mal) = req.mal_id.clone().filter(|m| !m.is_empty()) {
        let page = (next - 1) / 100 + 1;
        let eps = mv_media::jikan::JikanClient::shared().ok()?.anime_episodes(&mal, page).await.unwrap_or_default();
        let mut ep = eps.into_iter().find(|e| e.number == next).unwrap_or(mv_media::types::AnimeEpisode { number: next, ..Default::default() });
        mv_media::episode_art::fill_thumbnails(&mal, std::slice::from_mut(&mut ep), None).await;
        return Some(NextInfo { title: ep.title, still: ep.thumbnail });
    }
    let imdb = req.imdb_id.filter(|s| s.starts_with("tt"))?;
    let season = req.season.unwrap_or(1);
    let eps = mv_media::cinemeta::CinemetaClient::new().ok()?.series_episodes(&imdb).await.ok()?;
    let ep = eps.into_iter().find(|e| logic::episode_from_id(&e.id) == Some((season, next)))?;
    Some(NextInfo { title: ep.title, still: ep.thumbnail })
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

#[cfg(test)]
mod tests {
    use super::{ident, records_watch};
    use mv_ui::nav::PlayRequest;

    #[test]
    fn private_playback_is_not_recorded() {
        let req = PlayRequest { title: "Frieren".into(), source: "magnet:?xt=1".into(), file_idx: None, imdb_id: None, mal_id: Some("52991".into()), season: None, episode: Some(3) };
        let id = ident(&req);
        assert!(records_watch(&id, false));
        assert!(!records_watch(&id, true), "no history, resume point or Continue Watching while private");
    }

    #[test]
    fn only_catalog_titles_go_into_watch_history() {
        let req = |imdb: Option<&str>, mal: Option<&str>| PlayRequest { title: "Ch. 1".into(), source: "E:/video_final.mp4".into(), file_idx: None, imdb_id: imdb.map(Into::into), mal_id: mal.map(Into::into), season: None, episode: None };
        assert!(!ident(&req(None, None)).tracked, "a Studio recap video is not a movie");
        assert!(!ident(&req(Some(""), Some(""))).tracked);
        assert!(ident(&req(Some("tt0111161"), None)).tracked);
        assert!(ident(&req(None, Some("21"))).tracked);
    }
}
