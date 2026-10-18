//! Database operations for the manga pages (all synchronous, take a `&mut Connection`).
use std::collections::HashSet;

use anyhow::Result;
use mv_core::repo::chapter::{self, AddChapterInput, Chapter};
use mv_core::repo::manga::{self, AddMangaInput, Manga, UpdateMangaDetails};
use mv_core::repo::history;
use mv_sources::{ChapterInfo, MangaDetails, MangaSearchResult};
use rusqlite::Connection;

/// Find or create the library entry for a source result (details win over the listing row).
pub fn ensure_manga(conn: &Connection, source: &str, r: &MangaSearchResult, d: Option<&MangaDetails>) -> Result<Manga> {
    if let Some(m) = manga::find_by_source(conn, source, &r.source_id)? {
        return Ok(m);
    }
    manga::add(
        conn,
        &AddMangaInput {
            source_plugin: source.into(),
            source_id: r.source_id.clone(),
            title: r.title.clone(),
            author: d.and_then(|d| d.author.clone()).or_else(|| r.author.clone()),
            artist: d.and_then(|d| d.artist.clone()),
            description: d.and_then(|d| d.description.clone()),
            cover_path: d.and_then(|d| d.cover_url.clone()).or_else(|| r.cover_url.clone()),
            status: d.and_then(|d| d.status.clone()).or_else(|| r.status.clone()),
            genres: d.and_then(|d| d.genres.clone()).unwrap_or_default(),
        },
    )
}

/// Overwrite details that the source knows (never clears a field the source left empty).
pub fn apply_details(conn: &Connection, manga_id: &str, d: &MangaDetails) -> Result<()> {
    let some = |v: &Option<String>| v.clone().filter(|s| !s.is_empty()).map(Some);
    manga::update_details(
        conn,
        manga_id,
        &UpdateMangaDetails {
            author: some(&d.author),
            artist: some(&d.artist),
            description: some(&d.description),
            cover_path: some(&d.cover_url),
            status: some(&d.status),
            genres: d.genres.clone().filter(|g| !g.is_empty()),
        },
    )?;
    Ok(())
}

/// Upsert the source's chapters and drop stale ones that are not downloaded (like `CHAPTER_SYNC`).
pub fn apply_chapters(conn: &mut Connection, manga_id: &str, source: &str, remote: &[ChapterInfo]) -> Result<Vec<Chapter>> {
    let inputs: Vec<AddChapterInput> = remote
        .iter()
        .map(|c| AddChapterInput {
            manga_id: manga_id.into(),
            source_plugin: source.into(),
            source_id: c.source_id.clone(),
            number: c.number,
            title: c.title.clone(),
            language: Some(c.language.clone()).filter(|l| !l.is_empty()),
            page_count: c.page_count.map(i64::from),
        })
        .collect();
    chapter::add_batch(conn, &inputs)?;
    let keep: HashSet<&str> = remote.iter().map(|c| c.source_id.as_str()).collect();
    for local in chapter::list_by_source(conn, manga_id, source)? {
        if !keep.contains(local.source_id.as_str()) && local.download_status != "complete" {
            chapter::delete(conn, &local.id)?;
        }
    }
    chapter::list(conn, manga_id)
}

/// Persist reading progress: page, history entry, last-read, and "read" at the last page.
pub fn save_progress(conn: &Connection, manga_id: &str, chapter_id: &str, page: usize, total: usize) -> Result<()> {
    chapter::update_read_progress(conn, chapter_id, page as i64)?;
    history::add(conn, manga_id, chapter_id, page as i64)?;
    manga::update_last_read(conn, manga_id)?;
    if total > 0 && page + 1 >= total {
        chapter::mark_read(conn, chapter_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, Connection) {
        let d = tempfile::tempdir().unwrap();
        let c = mv_core::db::open_and_migrate(&d.path().join("t.db")).unwrap();
        (d, c)
    }

    fn info(id: &str, n: f64) -> ChapterInfo {
        ChapterInfo { source_id: id.into(), number: Some(n), language: "en".into(), ..Default::default() }
    }

    #[test]
    fn add_sync_and_progress() {
        let (_d, mut conn) = db();
        let r = MangaSearchResult { source_id: "x1".into(), title: "T".into(), cover_url: Some("http://c/1.jpg".into()), ..Default::default() };
        let d = MangaDetails { source_id: "x1".into(), title: "T".into(), author: Some("A".into()), genres: Some(vec!["g".into()]), ..Default::default() };
        let m = ensure_manga(&conn, "src", &r, Some(&d)).unwrap();
        assert_eq!((m.author.as_deref(), m.cover_path.as_deref()), (Some("A"), Some("http://c/1.jpg")));
        assert_eq!(ensure_manga(&conn, "src", &r, None).unwrap().id, m.id);

        let cs = apply_chapters(&mut conn, &m.id, "src", &[info("c1", 1.0), info("c2", 2.0)]).unwrap();
        assert_eq!(cs.len(), 2);
        let first = cs[0].id.clone();
        save_progress(&conn, &m.id, &first, 3, 4).unwrap();
        assert!(chapter::get(&conn, &first).unwrap().unwrap().is_read);
        assert_eq!(chapter::get(&conn, &first).unwrap().unwrap().read_progress, 3);
        assert_eq!(history::list(&conn, 10).unwrap().len(), 1);

        // c1 disappears upstream and is dropped; progress on c2 is kept across syncs
        save_progress(&conn, &m.id, &cs[1].id, 1, 10).unwrap();
        let cs = apply_chapters(&mut conn, &m.id, "src", &[info("c2", 2.0), info("c3", 3.0)]).unwrap();
        assert_eq!(cs.iter().map(|c| c.source_id.as_str()).collect::<Vec<_>>(), vec!["c2", "c3"]);
        assert_eq!(cs[0].read_progress, 1);

        apply_details(&conn, &m.id, &MangaDetails { description: Some("desc".into()), author: Some(String::new()), ..Default::default() }).unwrap();
        let m2 = manga::get(&conn, &m.id).unwrap().unwrap();
        assert_eq!((m2.description.as_deref(), m2.author.as_deref()), (Some("desc"), Some("A")));
    }
}
