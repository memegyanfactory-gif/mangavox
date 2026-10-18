use super::new_id;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Mirrors `ReadingHistory` in `src/shared/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadingHistory {
    pub id: String,
    pub manga_id: String,
    pub chapter_id: String,
    pub source_plugin: String,
    pub source_chapter_id: String,
    pub manga_title: String,
    pub chapter_number: Option<f64>,
    pub chapter_title: Option<String>,
    pub cover_url: Option<String>,
    pub page: i64,
    pub read_at: String,
}

/// One (latest) entry per manga, newest first.
pub fn list(conn: &Connection, limit: usize) -> Result<Vec<ReadingHistory>> {
    let mut stmt = conn.prepare(
        "SELECT h.id, h.manga_id, h.chapter_id, h.page, h.read_at,
                m.title, m.cover_path, c.number, c.title, c.source_plugin, c.source_id
         FROM reading_history h
         JOIN manga m ON m.id = h.manga_id
         JOIN chapter c ON c.id = h.chapter_id
         ORDER BY h.read_at DESC
         LIMIT 500",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ReadingHistory {
                id: r.get(0)?,
                manga_id: r.get(1)?,
                chapter_id: r.get(2)?,
                page: r.get(3)?,
                read_at: r.get(4)?,
                manga_title: r.get(5)?,
                cover_url: r.get(6)?,
                chapter_number: r.get(7)?,
                chapter_title: r.get(8)?,
                source_plugin: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                source_chapter_id: r.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = HashSet::new();
    Ok(rows.into_iter().filter(|r| seen.insert(r.manga_id.clone())).take(limit).collect())
}

/// History is one resumable entry per manga.
pub fn add(conn: &Connection, manga_id: &str, chapter_id: &str, page: i64) -> Result<()> {
    conn.execute("DELETE FROM reading_history WHERE manga_id = ?1", [manga_id])?;
    conn.execute(
        "INSERT INTO reading_history (id, manga_id, chapter_id, page) VALUES (?1, ?2, ?3, ?4)",
        params![new_id(), manga_id, chapter_id, page],
    )?;
    Ok(())
}

pub fn touch(conn: &Connection, manga_id: &str) -> Result<()> {
    let existing: Option<(String, i64)> = conn
        .query_row(
            "SELECT chapter_id, page FROM reading_history WHERE manga_id = ?1 ORDER BY read_at DESC LIMIT 1",
            [manga_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((chapter_id, page)) = existing {
        return add(conn, manga_id, &chapter_id, page);
    }
    let chapter: Option<(String, i64)> = conn
        .query_row(
            "SELECT id, read_progress FROM chapter WHERE manga_id = ?1
             ORDER BY CASE WHEN read_progress > 0 THEN 0 ELSE 1 END, read_progress DESC, number ASC LIMIT 1",
            [manga_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((id, progress)) = chapter {
        add(conn, manga_id, &id, progress)?;
    }
    Ok(())
}

pub fn remove(conn: &Connection, manga_id: &str) -> Result<()> {
    conn.execute("DELETE FROM reading_history WHERE manga_id = ?1", [manga_id])?;
    Ok(())
}

pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM reading_history", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::temp_db;
    use crate::repo::chapter::{self, AddChapterInput};
    use crate::repo::seed_manga;

    fn ch(conn: &Connection, m: &str, src: &str, n: f64) -> String {
        chapter::add(conn, &AddChapterInput { manga_id: m.into(), source_plugin: "mangadex".into(), source_id: src.into(), number: Some(n), ..Default::default() }).unwrap().id
    }

    #[test]
    fn one_entry_per_manga_and_touch() {
        let (_d, conn) = temp_db();
        let m1 = seed_manga(&conn, "a");
        let m2 = seed_manga(&conn, "b");
        let (c1, c2, c3) = (ch(&conn, &m1.id, "1", 1.0), ch(&conn, &m1.id, "2", 2.0), ch(&conn, &m2.id, "1", 1.0));
        add(&conn, &m1.id, &c1, 3).unwrap();
        add(&conn, &m1.id, &c2, 9).unwrap();
        add(&conn, &m2.id, &c3, 0).unwrap();
        let l = list(&conn, 50).unwrap();
        assert_eq!(l.len(), 2);
        let e = l.iter().find(|h| h.manga_id == m1.id).unwrap();
        assert_eq!((e.chapter_id.as_str(), e.page, e.manga_title.as_str(), e.source_plugin.as_str()), (c2.as_str(), 9, "Title a", "mangadex"));
        assert_eq!(list(&conn, 1).unwrap().len(), 1);

        remove(&conn, &m2.id).unwrap();
        touch(&conn, &m2.id).unwrap(); // falls back to a chapter
        assert_eq!(list(&conn, 50).unwrap().len(), 2);
        clear(&conn).unwrap();
        assert!(list(&conn, 50).unwrap().is_empty());
        let m3 = seed_manga(&conn, "c");
        touch(&conn, &m3.id).unwrap(); // no chapters: no-op
        assert!(list(&conn, 50).unwrap().is_empty());
    }
}
