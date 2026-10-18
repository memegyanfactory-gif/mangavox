//! Port of `media.repo.ts` (titles, events, playback state). The recommendation/profile
//! aggregation depends on `shared/media-intelligence` and is not ported yet.
use super::{new_id, now_ms};
use anyhow::Result;
use rusqlite::{params, params_from_iter, types::Value, Connection, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

fn parse_list(s: Option<String>) -> Vec<String> {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Mirrors `MediaTitle` (camelCase on the wire). Zero/empty values read back as `None`, like the TS mapper.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaTitle {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<i64>,
    pub mal_id: Option<i64>,
    pub cover_url: Option<String>,
    pub banner_url: Option<String>,
    pub year: Option<i64>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    pub description: Option<String>,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaEvent {
    pub id: Option<String>,
    pub media_id: Option<String>,
    pub kind: String,
    /// open | play | progress | complete | like | dislike | skip | search
    #[serde(rename = "type")]
    pub event_type: String,
    pub title: Option<MediaTitle>,
    pub query: Option<String>,
    pub provider_id: Option<String>,
    pub episode_number: Option<i64>,
    pub position_secs: Option<f64>,
    pub duration_secs: Option<f64>,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackState {
    pub media_id: String,
    pub kind: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub provider_id: Option<String>,
    pub episode_number: Option<i64>,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub completed: bool,
    pub updated_at: i64,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

fn map_title(r: &Row) -> rusqlite::Result<MediaTitle> {
    Ok(MediaTitle {
        id: r.get("id")?,
        kind: r.get("kind")?,
        title: r.get("title")?,
        imdb_id: non_empty(r.get("imdb_id")?),
        tmdb_id: r.get::<_, Option<i64>>("tmdb_id")?.filter(|v| *v != 0),
        mal_id: r.get::<_, Option<i64>>("mal_id")?.filter(|v| *v != 0),
        cover_url: non_empty(r.get("cover_url")?),
        banner_url: non_empty(r.get("banner_url")?),
        year: r.get::<_, Option<i64>>("year")?.filter(|v| *v != 0),
        genres: parse_list(r.get("genres_json")?),
        languages: parse_list(r.get("languages_json")?),
        description: non_empty(r.get("description")?),
        score: r.get::<_, Option<f64>>("score")?.filter(|v| *v != 0.0),
    })
}

pub fn get_title(conn: &Connection, id: &str) -> Result<Option<MediaTitle>> {
    use rusqlite::OptionalExtension;
    Ok(conn.query_row("SELECT * FROM media_title WHERE id = ?1", [id], map_title).optional()?)
}

pub fn upsert_title(conn: &Connection, t: &MediaTitle) -> Result<()> {
    conn.execute(
        "INSERT INTO media_title
           (id,kind,title,imdb_id,tmdb_id,mal_id,cover_url,banner_url,year,genres_json,languages_json,description,score,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
         ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,title=excluded.title,imdb_id=COALESCE(excluded.imdb_id,imdb_id),
           tmdb_id=COALESCE(excluded.tmdb_id,tmdb_id),mal_id=COALESCE(excluded.mal_id,mal_id),
           cover_url=COALESCE(excluded.cover_url,cover_url),banner_url=COALESCE(excluded.banner_url,banner_url),
           year=COALESCE(excluded.year,year),genres_json=excluded.genres_json,languages_json=excluded.languages_json,
           description=COALESCE(excluded.description,description),score=COALESCE(excluded.score,score),updated_at=excluded.updated_at",
        params![
            t.id,
            t.kind,
            t.title,
            t.imdb_id.clone().filter(|s| !s.is_empty()),
            t.tmdb_id.filter(|v| *v != 0),
            t.mal_id.filter(|v| *v != 0),
            t.cover_url.clone().filter(|s| !s.is_empty()),
            t.banner_url.clone().filter(|s| !s.is_empty()),
            t.year.filter(|v| *v != 0),
            serde_json::to_string(&t.genres)?,
            serde_json::to_string(&t.languages)?,
            t.description.clone().filter(|s| !s.is_empty()),
            t.score.filter(|v| *v != 0.0),
            now_ms()
        ],
    )?;
    Ok(())
}

/// Records an event (upserting its title) and updates resumable playback for play/progress/complete.
pub fn record_event(conn: &Connection, e: &MediaEvent) -> Result<String> {
    if let Some(t) = &e.title {
        upsert_title(conn, t)?;
    }
    let id = e.id.clone().unwrap_or_else(new_id);
    let media_id = e.media_id.clone().or_else(|| e.title.as_ref().map(|t| t.id.clone()));
    conn.execute(
        "INSERT INTO media_event
           (id,media_id,kind,event_type,query,provider_id,episode_number,position_secs,duration_secs,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            id,
            media_id,
            e.kind,
            e.event_type,
            e.query.clone().filter(|s| !s.is_empty()),
            e.provider_id.clone().filter(|s| !s.is_empty()),
            e.episode_number,
            e.position_secs,
            e.duration_secs,
            e.created_at.unwrap_or_else(now_ms)
        ],
    )?;
    if let Some(mid) = media_id {
        if ["play", "progress", "complete"].contains(&e.event_type.as_str()) {
            conn.execute(
                "INSERT INTO media_playback
                   (media_id,kind,provider_id,episode_number,position_secs,duration_secs,completed,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(media_id) DO UPDATE SET provider_id=excluded.provider_id,
                   episode_number=excluded.episode_number,position_secs=excluded.position_secs,
                   duration_secs=excluded.duration_secs,completed=excluded.completed,updated_at=excluded.updated_at",
                params![
                    mid,
                    e.kind,
                    e.provider_id.clone().filter(|s| !s.is_empty()),
                    e.episode_number,
                    e.position_secs.unwrap_or(0.0),
                    e.duration_secs.unwrap_or(0.0),
                    (e.event_type == "complete") as i64,
                    now_ms()
                ],
            )?;
        }
    }
    Ok(id)
}

pub fn list_history(conn: &Connection, kind: Option<&str>, provider_id: Option<&str>, limit: usize) -> Result<Vec<PlaybackState>> {
    let mut stmt = conn.prepare(
        "SELECT p.media_id,p.kind,t.title,t.cover_url,p.provider_id,p.episode_number,p.position_secs,
                p.duration_secs,p.completed,p.updated_at
         FROM media_playback p JOIN media_title t ON t.id = p.media_id
         WHERE (?1 IS NULL OR p.kind = ?1) AND (?2 IS NULL OR p.provider_id = ?2)
         ORDER BY p.updated_at DESC LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![kind, provider_id, limit as i64], |r| {
            Ok(PlaybackState {
                media_id: r.get(0)?,
                kind: r.get(1)?,
                title: r.get(2)?,
                cover_url: non_empty(r.get(3)?),
                provider_id: non_empty(r.get(4)?),
                episode_number: r.get(5)?,
                position_secs: r.get(6)?,
                duration_secs: r.get(7)?,
                completed: r.get::<_, i64>(8)? != 0,
                updated_at: r.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn list_titles_by_event(conn: &Connection, event_type: &str, kind: Option<&str>, limit: usize) -> Result<Vec<MediaTitle>> {
    let mut stmt = conn.prepare(
        "SELECT t.*, MAX(e.created_at) AS latest FROM media_event e
         JOIN media_title t ON t.id = e.media_id
         WHERE e.event_type = ?1 AND (?2 IS NULL OR e.kind = ?2)
         GROUP BY t.id ORDER BY latest DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![event_type, kind, limit as i64], map_title)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

const PLAY_EVENTS: &str = "('open','play','progress','complete')";

pub fn remove_history(conn: &Connection, media_id: &str) -> Result<()> {
    conn.execute("DELETE FROM media_playback WHERE media_id = ?1", [media_id])?;
    conn.execute(&format!("DELETE FROM media_event WHERE media_id = ?1 AND event_type IN {PLAY_EVENTS}"), [media_id])?;
    Ok(())
}

pub fn remove_events(conn: &Connection, media_id: &str, event_types: &[&str]) -> Result<()> {
    if event_types.is_empty() {
        return Ok(());
    }
    let ph = vec!["?"; event_types.len()].join(",");
    let mut args: Vec<Value> = vec![Value::Text(media_id.to_string())];
    args.extend(event_types.iter().map(|t| Value::Text((*t).to_string())));
    conn.execute(&format!("DELETE FROM media_event WHERE media_id = ? AND event_type IN ({ph})"), params_from_iter(args))?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shelf {
    History,
    Favorites,
    Watched,
}

pub fn clear_shelf(conn: &Connection, kind: Option<&str>, shelf: Shelf) -> Result<()> {
    match shelf {
        Shelf::History | Shelf::Watched => {
            conn.execute("DELETE FROM media_playback WHERE (?1 IS NULL OR kind = ?1)", [kind])?;
            conn.execute(&format!("DELETE FROM media_event WHERE (?1 IS NULL OR kind = ?1) AND event_type IN {PLAY_EVENTS}"), [kind])?;
        }
        Shelf::Favorites => {
            conn.execute("DELETE FROM media_event WHERE (?1 IS NULL OR kind = ?1) AND event_type = 'like'", [kind])?;
        }
    }
    Ok(())
}

/// Titles the user has finished (completed playback or a `complete` event).
pub fn watched_ids(conn: &Connection, kind: Option<&str>) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT media_id FROM media_playback WHERE completed = 1 AND (?1 IS NULL OR kind = ?1)")?;
    let mut ids: HashSet<String> = stmt.query_map([kind], |r| r.get(0))?.collect::<Result<_, _>>()?;
    ids.extend(list_titles_by_event(conn, "complete", kind, 500)?.into_iter().map(|t| t.id));
    Ok(ids)
}

pub fn reset_personalization(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM media_event", [])?;
    conn.execute("DELETE FROM media_playback", [])?;
    conn.execute("DELETE FROM media_import_batch", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::temp_db;

    fn title(id: &str) -> MediaTitle {
        MediaTitle { id: id.into(), kind: "anime".into(), title: format!("T {id}"), genres: vec!["action".into()], year: Some(2020), ..Default::default() }
    }

    fn ev(id: &str, ty: &str) -> MediaEvent {
        MediaEvent { media_id: Some(id.into()), kind: "anime".into(), event_type: ty.into(), position_secs: Some(10.0), duration_secs: Some(1400.0), episode_number: Some(1), ..Default::default() }
    }

    #[test]
    fn title_upsert_keeps_existing_optional_fields() {
        let (_d, conn) = temp_db();
        let mut t = title("a");
        t.cover_url = Some("http://c".into());
        upsert_title(&conn, &t).unwrap();
        t.cover_url = None;
        t.title = "Renamed".into();
        upsert_title(&conn, &t).unwrap();
        let g = get_title(&conn, "a").unwrap().unwrap();
        assert_eq!((g.title.as_str(), g.cover_url.as_deref(), g.year), ("Renamed", Some("http://c"), Some(2020)));
        assert_eq!(g.genres, vec!["action"]);
    }

    #[test]
    fn events_playback_and_shelves() {
        let (_d, conn) = temp_db();
        upsert_title(&conn, &title("a")).unwrap();
        upsert_title(&conn, &title("b")).unwrap();
        record_event(&conn, &ev("a", "open")).unwrap();
        assert!(list_history(&conn, None, None, 10).unwrap().is_empty());
        record_event(&conn, &ev("a", "progress")).unwrap();
        let mut done = ev("b", "complete");
        done.title = Some(MediaTitle { id: "b".into(), kind: "anime".into(), title: "B2".into(), ..Default::default() });
        record_event(&conn, &done).unwrap();
        record_event(&conn, &ev("a", "like")).unwrap();

        let h = list_history(&conn, Some("anime"), None, 10).unwrap();
        assert_eq!(h.len(), 2);
        assert!(h.iter().find(|p| p.media_id == "b").unwrap().completed);
        assert_eq!(list_history(&conn, Some("movie"), None, 10).unwrap().len(), 0);
        assert_eq!(list_titles_by_event(&conn, "like", None, 10).unwrap()[0].id, "a");
        assert_eq!(watched_ids(&conn, None).unwrap(), HashSet::from(["b".to_string()]));

        remove_events(&conn, "a", &["like"]).unwrap();
        assert!(list_titles_by_event(&conn, "like", None, 10).unwrap().is_empty());
        remove_history(&conn, "a").unwrap();
        assert_eq!(list_history(&conn, None, None, 10).unwrap().len(), 1);
        clear_shelf(&conn, Some("anime"), Shelf::History).unwrap();
        assert!(list_history(&conn, None, None, 10).unwrap().is_empty());
        record_event(&conn, &ev("a", "like")).unwrap();
        clear_shelf(&conn, None, Shelf::Favorites).unwrap();
        assert!(list_titles_by_event(&conn, "like", None, 10).unwrap().is_empty());
        reset_personalization(&conn).unwrap();
    }
}
