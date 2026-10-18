use super::new_id;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Mirrors `Chapter` in `src/shared/types.ts` (status columns are plain strings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub id: String,
    pub manga_id: String,
    pub source_plugin: String,
    pub source_id: String,
    pub number: Option<f64>,
    pub title: Option<String>,
    pub language: String,
    pub page_count: Option<i64>,
    pub download_status: String,
    pub ocr_status: String,
    pub tts_status: String,
    pub video_status: String,
    pub read_progress: i64,
    pub is_read: bool,
    pub pages_dir: Option<String>,
    pub added_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddChapterInput {
    pub manga_id: String,
    pub source_plugin: String,
    pub source_id: String,
    pub number: Option<f64>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub page_count: Option<i64>,
}

fn row_to_chapter(r: &Row) -> rusqlite::Result<Chapter> {
    Ok(Chapter {
        id: r.get("id")?,
        manga_id: r.get("manga_id")?,
        source_plugin: r.get::<_, Option<String>>("source_plugin")?.unwrap_or_default(),
        source_id: r.get("source_id")?,
        number: r.get("number")?,
        title: r.get("title")?,
        language: r.get("language")?,
        page_count: r.get("page_count")?,
        download_status: r.get("download_status")?,
        ocr_status: r.get("ocr_status")?,
        tts_status: r.get("tts_status")?,
        video_status: r.get("video_status")?,
        read_progress: r.get("read_progress")?,
        is_read: r.get::<_, i64>("is_read")? != 0,
        pages_dir: r.get("pages_dir")?,
        added_at: r.get("added_at")?,
    })
}

pub fn list(conn: &Connection, manga_id: &str) -> Result<Vec<Chapter>> {
    let mut stmt = conn.prepare("SELECT * FROM chapter WHERE manga_id = ?1 ORDER BY number ASC")?;
    let rows = stmt.query_map([manga_id], row_to_chapter)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn list_by_source(conn: &Connection, manga_id: &str, source_plugin: &str) -> Result<Vec<Chapter>> {
    let mut stmt =
        conn.prepare("SELECT * FROM chapter WHERE manga_id = ?1 AND source_plugin = ?2 ORDER BY number ASC")?;
    let rows = stmt.query_map([manga_id, source_plugin], row_to_chapter)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Chapter>> {
    Ok(conn.query_row("SELECT * FROM chapter WHERE id = ?1", [id], row_to_chapter).optional()?)
}

pub fn find_by_source(conn: &Connection, manga_id: &str, source_id: &str) -> Result<Option<Chapter>> {
    Ok(conn
        .query_row("SELECT * FROM chapter WHERE manga_id = ?1 AND source_id = ?2", [manga_id, source_id], row_to_chapter)
        .optional()?)
}

/// INSERT OR IGNORE; returns the existing chapter when (manga_id, source_id) already exists.
pub fn add(conn: &Connection, i: &AddChapterInput) -> Result<Chapter> {
    let id = new_id();
    conn.execute(
        "INSERT OR IGNORE INTO chapter (id, manga_id, source_plugin, source_id, number, title, language, page_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, i.manga_id, i.source_plugin, i.source_id, i.number, i.title, i.language.as_deref().unwrap_or("en"), i.page_count],
    )?;
    match get(conn, &id)? {
        Some(c) => Ok(c),
        None => Ok(find_by_source(conn, &i.manga_id, &i.source_id)?.expect("chapter exists")),
    }
}

/// Upsert on (manga_id, source_id); keeps progress/status of existing rows.
pub fn add_batch(conn: &mut Connection, chapters: &[AddChapterInput]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO chapter (id, manga_id, source_plugin, source_id, number, title, language, page_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(manga_id, source_id) DO UPDATE SET
               source_plugin = excluded.source_plugin,
               number = excluded.number,
               title = excluded.title,
               page_count = excluded.page_count",
        )?;
        for c in chapters {
            stmt.execute(params![new_id(), c.manga_id, c.source_plugin, c.source_id, c.number, c.title, c.language.as_deref().unwrap_or("en"), c.page_count])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM chapter WHERE id = ?1", [id])?;
    Ok(())
}

pub fn update_download_status(conn: &Connection, id: &str, status: &str, pages_dir: Option<&str>) -> Result<()> {
    match pages_dir {
        Some(dir) => conn.execute("UPDATE chapter SET download_status = ?1, pages_dir = ?2 WHERE id = ?3", params![status, dir, id])?,
        None => conn.execute("UPDATE chapter SET download_status = ?1 WHERE id = ?2", params![status, id])?,
    };
    Ok(())
}

pub fn update_ocr_status(conn: &Connection, id: &str, status: &str) -> Result<()> {
    conn.execute("UPDATE chapter SET ocr_status = ?1 WHERE id = ?2", [status, id])?;
    Ok(())
}

pub fn update_tts_status(conn: &Connection, id: &str, status: &str) -> Result<()> {
    conn.execute("UPDATE chapter SET tts_status = ?1 WHERE id = ?2", [status, id])?;
    Ok(())
}

pub fn update_video_status(conn: &Connection, id: &str, status: &str) -> Result<()> {
    conn.execute("UPDATE chapter SET video_status = ?1 WHERE id = ?2", [status, id])?;
    Ok(())
}

pub fn update_read_progress(conn: &Connection, id: &str, page: i64) -> Result<()> {
    conn.execute("UPDATE chapter SET read_progress = ?1 WHERE id = ?2", params![page, id])?;
    Ok(())
}

pub fn mark_read(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE chapter SET is_read = 1 WHERE id = ?1", [id])?;
    Ok(())
}

/// Chapters left queued/downloading by a crash or quit become downloadable again.
pub fn reset_stale_downloads(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("UPDATE chapter SET download_status = 'none' WHERE download_status IN ('queued', 'downloading')", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::temp_db;
    use crate::repo::{manga, seed_manga};

    fn input(m: &str, src: &str, n: f64) -> AddChapterInput {
        AddChapterInput { manga_id: m.into(), source_plugin: "mangadex".into(), source_id: src.into(), number: Some(n), title: Some(format!("Ch {n}")), ..Default::default() }
    }

    #[test]
    fn crud_roundtrip() {
        let (_d, mut conn) = temp_db();
        let m = seed_manga(&conn, "a");
        let c = add(&conn, &input(&m.id, "c2", 2.0)).unwrap();
        assert_eq!((c.language.as_str(), c.download_status.as_str(), c.is_read), ("en", "none", false));
        let again = add(&conn, &input(&m.id, "c2", 99.0)).unwrap();
        assert_eq!(again.id, c.id);
        assert_eq!(again.number, Some(2.0));
        add_batch(&mut conn, &[input(&m.id, "c1", 1.0), input(&m.id, "c2", 2.5)]).unwrap();
        let all = list(&conn, &m.id).unwrap();
        assert_eq!(all.iter().map(|c| c.number.unwrap()).collect::<Vec<_>>(), vec![1.0, 2.5]);
        assert_eq!(all[1].id, c.id);
        assert_eq!(list_by_source(&conn, &m.id, "mangadex").unwrap().len(), 2);
        assert_eq!(list_by_source(&conn, &m.id, "other").unwrap().len(), 0);

        update_download_status(&conn, &c.id, "complete", Some("/p")).unwrap();
        update_ocr_status(&conn, &c.id, "running").unwrap();
        update_tts_status(&conn, &c.id, "queued").unwrap();
        update_video_status(&conn, &c.id, "failed").unwrap();
        update_read_progress(&conn, &c.id, 7).unwrap();
        mark_read(&conn, &c.id).unwrap();
        let g = get(&conn, &c.id).unwrap().unwrap();
        assert_eq!((g.download_status.as_str(), g.pages_dir.as_deref(), g.ocr_status.as_str()), ("complete", Some("/p"), "running"));
        assert_eq!((g.tts_status.as_str(), g.video_status.as_str(), g.read_progress, g.is_read), ("queued", "failed", 7, true));

        update_download_status(&conn, &c.id, "downloading", None).unwrap();
        assert_eq!(reset_stale_downloads(&conn).unwrap(), 1);
        assert_eq!(manga::list(&conn).unwrap()[0].unread_count, Some(1));
        delete(&conn, &c.id).unwrap();
        assert!(find_by_source(&conn, &m.id, "c2").unwrap().is_none());
        // cascade on manga delete
        manga::remove(&conn, &m.id).unwrap();
        assert!(list(&conn, &m.id).unwrap().is_empty());
    }
}
