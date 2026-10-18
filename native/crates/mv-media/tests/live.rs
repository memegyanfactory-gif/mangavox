//! Live network tests. They are ignored by default because they hit the real public services.
//! Run with: cargo test -p mv-media -- --ignored --test-threads=1

use mv_media::cinemeta::{CinemetaCatalog, CinemetaClient};
use mv_media::jikan::{JikanClient, ScheduleDay};
use mv_media::skip_times::SkipTimesClient;
use mv_media::stremio::StremioClient;

const CINEMETA_ADDON: &str = "https://v3-cinemeta.strem.io/manifest.json";
const AOT_MAL_ID: &str = "16498";

#[tokio::test]
#[ignore]
async fn jikan_top_search_details_episodes() {
    let jikan = JikanClient::new().unwrap();
    let top = jikan.top_anime(1, None).await.unwrap();
    assert!(!top.is_empty());
    let found = jikan
        .search_anime("attack on titan", 1, Some("tv"))
        .await
        .unwrap();
    assert!(found.iter().any(|a| a.id == AOT_MAL_ID));
    let details = jikan.anime_details(AOT_MAL_ID).await.unwrap();
    assert_eq!(details.id, AOT_MAL_ID);
    let episodes = jikan.anime_episodes(AOT_MAL_ID, 1).await.unwrap();
    assert!(!episodes.is_empty());
}

#[tokio::test]
#[ignore]
async fn jikan_schedule_genres_relations_and_imdb() {
    let jikan = JikanClient::new().unwrap();
    assert!(!jikan.genres().await.unwrap().is_empty());
    let _ = jikan.schedule_anime(ScheduleDay::Monday, 1).await.unwrap();
    let relations = jikan.anime_relations(AOT_MAL_ID).await.unwrap();
    assert!(!relations.is_empty());
    assert!(jikan.imdb_id(AOT_MAL_ID).await.is_some());
}

#[tokio::test]
#[ignore]
async fn cinemeta_catalog_search_and_meta() {
    let cinemeta = CinemetaClient::new().unwrap();
    assert!(!cinemeta
        .movies_catalog(CinemetaCatalog::Top, 1, None)
        .await
        .unwrap()
        .is_empty());
    assert!(!cinemeta
        .search_movies("inception")
        .await
        .unwrap()
        .is_empty());
    assert!(!cinemeta
        .search_series("breaking bad")
        .await
        .unwrap()
        .is_empty());
    let movie = cinemeta.movie_details("tt1375666").await.unwrap();
    assert_eq!(movie.map(|m| m.title).as_deref(), Some("Inception"));
    assert!(!cinemeta
        .series_episodes("tt0903747")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[ignore]
async fn stremio_manifest_catalog_meta_subtitles() {
    let stremio = StremioClient::new().unwrap();
    let manifest = stremio.fetch_manifest(CINEMETA_ADDON).await.unwrap();
    assert!(manifest.resources.iter().any(|r| r == "catalog"));
    let catalog = stremio
        .fetch_catalog(&manifest.endpoint, "top", "movie", 1)
        .await
        .unwrap();
    assert!(!catalog.is_empty());
    let meta = stremio
        .fetch_meta(&manifest.endpoint, "tt0903747", "series")
        .await
        .unwrap();
    assert!(!meta.episodes.is_empty());
    let subs = stremio
        .fetch_subtitles("https://opensubtitles-v3.strem.io", "tt1375666", "movie")
        .await
        .unwrap();
    assert!(!subs.is_empty());
}

#[tokio::test]
#[ignore]
async fn aniskip_returns_segments() {
    let skip = SkipTimesClient::new().unwrap();
    let result = skip.get(16498, 1.0, 1440.0).await.unwrap();
    assert!(result.found);
}
