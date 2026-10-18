use super::new_id;
use anyhow::Result;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Mirrors `Manga` in `src/shared/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manga {
    pub id: String,
    pub source_plugin: String,
    pub source_id: String,
    pub title: String,
    pub author: Option<String>,
    pub artist: Option<String>,
    pub description: Option<String>,
    pub cover_path: Option<String>,
    pub status: Option<String>,
    pub genres: Vec<String>,
    pub is_favorite: bool,
    pub notes: Option<String>,
    pub added_at: String,
    pub last_read_at: Option<String>,
    pub chapter_count: Option<i64>,
    pub unread_count: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddMangaInput {
    pub source_plugin: String,
    pub source_id: String,
    pub title: String,
    pub author: Option<String>,
    pub artist: Option<String>,
    pub description: Option<String>,
    pub cover_path: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
}

/// Fields left as `None` are untouched; `Some(None)` clears a nullable field.
#[derive(Debug, Clone, Default)]
pub struct UpdateMangaDetails {
    pub author: Option<Option<String>>,
    pub artist: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub cover_path: Option<Option<String>>,
    pub status: Option<Option<String>>,
    pub genres: Option<Vec<String>>,
}

fn row_to_manga(r: &Row, with_counts: bool) -> rusqlite::Result<Manga> {
    let genres: Option<String> = r.get("genres")?;
    Ok(Manga {
        id: r.get("id")?,
        source_plugin: r.get("source_plugin")?,
        source_id: r.get("source_id")?,
        title: r.get("title")?,
        author: r.get("author")?,
        artist: r.get("artist")?,
        description: r.get("description")?,
        cover_path: r.get("cover_path")?,
        status: r.get("status")?,
        genres: genres.and_then(|g| serde_json::from_str(&g).ok()).unwrap_or_default(),
        is_favorite: r.get::<_, Option<i64>>("is_favorite")?.unwrap_or(0) != 0,
        notes: r.get("notes")?,
        added_at: r.get("added_at")?,
        last_read_at: r.get("last_read_at")?,
        chapter_count: if with_counts { Some(r.get("chapter_count")?) } else { None },
        unread_count: if with_counts { Some(r.get("unread_count")?) } else { None },
    })
}

pub fn list(conn: &Connection) -> Result<Vec<Manga>> {
    let mut stmt = conn.prepare(
        "SELECT m.*,
                (SELECT COUNT(*) FROM chapter c WHERE c.manga_id = m.id) AS chapter_count,
                (SELECT COUNT(*) FROM chapter c WHERE c.manga_id = m.id AND c.is_read = 0) AS unread_count
         FROM manga m ORDER BY m.added_at DESC",
    )?;
    let rows = stmt.query_map([], |r| row_to_manga(r, true))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Manga>> {
    Ok(conn.query_row("SELECT * FROM manga WHERE id = ?1", [id], |r| row_to_manga(r, false)).optional()?)
}

pub fn search(conn: &Connection, query: &str) -> Result<Vec<Manga>> {
    let like = format!("%{query}%");
    let mut stmt = conn.prepare("SELECT * FROM manga WHERE title LIKE ?1 OR author LIKE ?1 ORDER BY title")?;
    let rows = stmt.query_map([like], |r| row_to_manga(r, false))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn find_by_source(conn: &Connection, source_plugin: &str, source_id: &str) -> Result<Option<Manga>> {
    Ok(conn
        .query_row(
            "SELECT * FROM manga WHERE source_plugin = ?1 AND source_id = ?2",
            [source_plugin, source_id],
            |r| row_to_manga(r, false),
        )
        .optional()?)
}

pub fn add(conn: &Connection, input: &AddMangaInput) -> Result<Manga> {
    let id = new_id();
    conn.execute(
        "INSERT INTO manga (id, source_plugin, source_id, title, author, artist, description, cover_path, status, genres)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            input.source_plugin,
            input.source_id,
            input.title,
            input.author,
            input.artist,
            input.description,
            input.cover_path,
            input.status,
            serde_json::to_string(&input.genres)?
        ],
    )?;
    Ok(get(conn, &id)?.expect("just inserted"))
}

pub fn remove(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM manga WHERE id = ?1", [id])?;
    Ok(())
}

pub fn remove_many(conn: &mut Connection, ids: &[String]) -> Result<()> {
    let tx = conn.transaction()?;
    for id in ids {
        tx.execute("DELETE FROM manga WHERE id = ?1", [id])?;
    }
    tx.commit()?;
    Ok(())
}

pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM manga", [])?;
    Ok(())
}

/// Returns the new favorite state (false when the manga does not exist).
pub fn toggle_favorite(conn: &Connection, id: &str) -> Result<bool> {
    let Some(m) = get(conn, id)? else { return Ok(false) };
    let new_val = !m.is_favorite;
    conn.execute("UPDATE manga SET is_favorite = ?1 WHERE id = ?2", params![new_val as i64, id])?;
    Ok(new_val)
}

pub fn update_notes(conn: &Connection, id: &str, notes: &str) -> Result<()> {
    conn.execute("UPDATE manga SET notes = ?1 WHERE id = ?2", [notes, id])?;
    Ok(())
}

pub fn update_last_read(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE manga SET last_read_at = datetime('now') WHERE id = ?1", [id])?;
    Ok(())
}

pub fn update_details(conn: &Connection, id: &str, u: &UpdateMangaDetails) -> Result<Option<Manga>> {
    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<Option<String>> = Vec::new();
    let mut push = |col: &'static str, v: &Option<Option<String>>| {
        if let Some(v) = v {
            sets.push(col);
            vals.push(v.clone());
        }
    };
    push("author = ?", &u.author);
    push("artist = ?", &u.artist);
    push("description = ?", &u.description);
    push("cover_path = ?", &u.cover_path);
    push("status = ?", &u.status);
    if let Some(g) = &u.genres {
        sets.push("genres = ?");
        vals.push(Some(serde_json::to_string(g)?));
    }
    if sets.is_empty() {
        return get(conn, id);
    }
    vals.push(Some(id.to_string()));
    conn.execute(&format!("UPDATE manga SET {} WHERE id = ?", sets.join(", ")), params_from_iter(vals.iter()))?;
    get(conn, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::temp_db;

    pub fn sample(src: &str) -> AddMangaInput {
        AddMangaInput {
            source_plugin: "mangadex".into(),
            source_id: src.into(),
            title: format!("Title {src}"),
            author: Some("Auth".into()),
            genres: vec!["action".into(), "drama".into()],
            ..Default::default()
        }
    }

    #[test]
    fn crud_roundtrip() {
        let (_d, mut conn) = temp_db();
        let m = add(&conn, &sample("a")).unwrap();
        assert_eq!(m.genres, vec!["action", "drama"]);
        assert!(!m.is_favorite);
        assert_eq!(find_by_source(&conn, "mangadex", "a").unwrap().unwrap().id, m.id);
        assert!(toggle_favorite(&conn, &m.id).unwrap());
        assert!(!toggle_favorite(&conn, "nope").unwrap());
        update_notes(&conn, &m.id, "hi").unwrap();
        update_last_read(&conn, &m.id).unwrap();
        let u = update_details(
            &conn,
            &m.id,
            &UpdateMangaDetails { author: Some(None), status: Some(Some("ongoing".into())), genres: Some(vec!["x".into()]), ..Default::default() },
        )
        .unwrap()
        .unwrap();
        assert_eq!(u.author, None);
        assert_eq!(u.status.as_deref(), Some("ongoing"));
        assert_eq!(u.genres, vec!["x"]);
        assert_eq!(u.notes.as_deref(), Some("hi"));
        assert!(u.last_read_at.is_some() && u.is_favorite);
        assert_eq!(search(&conn, "Title").unwrap().len(), 1);
        let l = list(&conn).unwrap();
        assert_eq!((l[0].chapter_count, l[0].unread_count), (Some(0), Some(0)));
        let b = add(&conn, &sample("b")).unwrap();
        remove_many(&mut conn, &[m.id.clone(), b.id.clone()]).unwrap();
        assert!(list(&conn).unwrap().is_empty());
        add(&conn, &sample("c")).unwrap();
        clear(&conn).unwrap();
        assert!(get(&conn, &m.id).unwrap().is_none());
    }

    #[test]
    fn unique_source_enforced() {
        let (_d, conn) = temp_db();
        add(&conn, &sample("a")).unwrap();
        assert!(add(&conn, &sample("a")).is_err());
    }
}
