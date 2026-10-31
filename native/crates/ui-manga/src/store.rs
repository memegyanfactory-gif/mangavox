//! Database operations for the manga pages (all synchronous, take a `&mut Connection`).
use std::collections::{HashMap, HashSet};

use anyhow::Result;
use mv_core::repo::chapter::{self, AddChapterInput, Chapter};
use mv_core::repo::manga::{self, AddMangaInput, Manga, UpdateMangaDetails};
use mv_core::repo::history;
use mv_sources::{ChapterInfo, MangaDetails, MangaSearchResult};
use rusqlite::Connection;

use crate::logic::{self, LibStats};

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

/// Upsert the source's chapters (narrowed to the preferred language `lang`, falling back to all
/// languages when none match) and drop stale ones that are not downloaded (like `CHAPTER_SYNC`).
/// Returns the manga's chapters in `lang`, ascending.
pub fn apply_chapters(conn: &mut Connection, manga_id: &str, source: &str, remote: &[ChapterInfo], lang: &str) -> Result<Vec<Chapter>> {
    let remote = logic::keep_language(remote.to_vec(), lang, |c| c.language.as_str());
    let remote = remote.as_slice();
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
    chapters(conn, manga_id, lang)
}

/// A manga's chapters in the preferred language (all languages when none match), ascending, one
/// source per chapter number when several sources have it (see [`one_source_per_number`]).
pub fn chapters(conn: &Connection, manga_id: &str, lang: &str) -> Result<Vec<Chapter>> {
    let list = logic::keep_language(chapter::list(conn, manga_id)?, lang, |c| c.language.as_str());
    let primary = manga::get(conn, manga_id)?.map(|m| m.source_plugin).unwrap_or_default();
    Ok(one_source_per_number(list, &primary))
}

fn number_key(n: f64) -> i64 {
    (n * 1000.0).round() as i64
}

/// A title can have chapters from several sources (its own and others found for it). Where more
/// than one source has a chapter number, that number's chapters come from one of them: the one
/// it was downloaded from, else the one it was read on, else the source with the most chapters
/// of the title, else the title's own source. Chapters without a number are all kept.
pub fn one_source_per_number(list: Vec<Chapter>, primary: &str) -> Vec<Chapter> {
    let mut per_source: HashMap<String, usize> = HashMap::new();
    for c in &list {
        *per_source.entry(c.source_plugin.clone()).or_default() += 1;
    }
    if per_source.len() < 2 {
        return list;
    }
    let mut by_number: HashMap<i64, Vec<&Chapter>> = HashMap::new();
    for c in &list {
        if let Some(n) = c.number {
            by_number.entry(number_key(n)).or_default().push(c);
        }
    }
    let winner: HashMap<i64, String> = by_number
        .into_iter()
        .map(|(k, cs)| {
            let best = cs
                .iter()
                .max_by_key(|c| {
                    let all = |f: &dyn Fn(&Chapter) -> bool| cs.iter().any(|x| x.source_plugin == c.source_plugin && f(x));
                    (
                        all(&|x| x.download_status == "complete"),
                        all(&|x| x.is_read || x.read_progress > 0),
                        per_source.get(&c.source_plugin).copied().unwrap_or(0),
                        c.source_plugin == primary,
                    )
                })
                .map(|c| c.source_plugin.clone())
                .unwrap_or_default();
            (k, best)
        })
        .collect();
    list.into_iter().filter(|c| c.number.is_none_or(|n| winner.get(&number_key(n)) == Some(&c.source_plugin))).collect()
}

/// Chapter counts per manga for the library grid, using the same language rule as [`chapters`].
pub fn library_stats(conn: &Connection, lang: &str) -> Result<HashMap<String, LibStats>> {
    let mut stmt = conn.prepare("SELECT manga_id, language, is_read, download_status, number FROM chapter")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, i64>(2)? != 0,
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                r.get::<_, Option<f64>>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut per: HashMap<String, Vec<(String, bool, bool, Option<f64>)>> = HashMap::new();
    for (mid, l, read, dl, n) in rows {
        per.entry(mid).or_default().push((l, read, dl == "complete", n));
    }
    Ok(per
        .into_iter()
        .map(|(mid, cs)| {
            let cs = logic::keep_language(cs, lang, |c| c.0.as_str());
            // One entry per chapter number (several sources can have the same chapter).
            let mut numbered: HashMap<i64, (bool, bool)> = HashMap::new();
            let mut st = LibStats::default();
            for (_, read, dl, n) in cs {
                match n {
                    Some(n) => {
                        let e = numbered.entry(number_key(n)).or_default();
                        e.0 |= read;
                        e.1 |= dl;
                    }
                    None => {
                        st.total += 1;
                        st.unread += usize::from(!read);
                        st.downloaded += usize::from(dl);
                    }
                }
            }
            st.total += numbered.len();
            st.unread += numbered.values().filter(|e| !e.0).count();
            st.downloaded += numbered.values().filter(|e| e.1).count();
            (mid, st)
        })
        .collect())
}

/// The chapter "Continue reading" opens for a title (see [`logic::continue_target`]).
pub fn continue_chapter(conn: &Connection, manga_id: &str, lang: &str) -> Result<Option<String>> {
    let cs = chapters(conn, manga_id, lang)?;
    Ok(logic::continue_target(&cs).map(|(i, _)| cs[i].id.clone()))
}

/// Titles with at least one downloaded chapter (library and browse-only alike), most recently
/// read first, each with its downloaded chapters ascending.
pub fn downloaded_titles(conn: &Connection) -> Result<Vec<(Manga, Vec<Chapter>)>> {
    let mut stmt = conn.prepare("SELECT DISTINCT manga_id FROM chapter WHERE download_status = 'complete'")?;
    let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for id in ids {
        let Some(m) = manga::get(conn, &id)? else { continue };
        let cs: Vec<Chapter> = chapter::list(conn, &id)?.into_iter().filter(|c| c.download_status == "complete").collect();
        out.push((m, cs));
    }
    out.sort_by(|a, b| b.0.last_read_at.cmp(&a.0.last_read_at).then_with(|| a.0.title.to_lowercase().cmp(&b.0.title.to_lowercase())));
    Ok(out)
}

/// SQLite `datetime('now')` text ("2026-10-31 12:00:00", UTC) as unix seconds.
pub fn sql_time(s: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S").ok().map(|d| d.and_utc().timestamp())
}

/// What the database says about every title, for the taste profile: chapters read (by the same
/// language and one-per-number rules as the library), favorite, library membership (`hidden`
/// = browse-only ids), categories and when it was last read.
pub fn taste_facts(conn: &Connection, lang: &str, hidden: &HashSet<String>) -> Result<Vec<crate::taste::TitleFacts>> {
    use mv_core::repo::category;
    let stats = library_stats(conn, lang)?;
    let names: HashMap<String, String> = category::list(conn)?.into_iter().map(|c| (c.id, c.name)).collect();
    let cats = category::manga_category_map(conn)?;
    Ok(manga::list(conn)?
        .into_iter()
        .map(|m| {
            let st = stats.get(&m.id).copied().unwrap_or_default();
            crate::taste::TitleFacts {
                title: crate::taste::TitleRef::from_manga(&m),
                in_library: !hidden.contains(&m.id),
                favorite: m.is_favorite,
                chapters: st.total,
                read: st.total - st.unread,
                last_read: m.last_read_at.as_deref().and_then(sql_time),
                added: sql_time(&m.added_at).unwrap_or(0),
                categories: cats.get(&m.id).into_iter().flatten().filter_map(|c| names.get(c).cloned()).collect(),
            }
        })
        .collect())
}

/// Mark chapters read or unread (unread also resets the saved page).
pub fn set_read(conn: &mut Connection, ids: &[String], read: bool) -> Result<()> {
    let tx = conn.transaction()?;
    for id in ids {
        if read {
            tx.execute("UPDATE chapter SET is_read = 1 WHERE id = ?1", [id])?;
        } else {
            tx.execute("UPDATE chapter SET is_read = 0, read_progress = 0 WHERE id = ?1", [id])?;
        }
    }
    tx.commit()?;
    crate::backend::bump_data();
    Ok(())
}

/// Put chapters back to an earlier `(id, is_read, read_progress)` state (Undo of a bulk mark).
pub fn restore_read(conn: &mut Connection, prev: &[(String, bool, i64)]) -> Result<()> {
    let tx = conn.transaction()?;
    for (id, read, progress) in prev {
        tx.execute("UPDATE chapter SET is_read = ?2, read_progress = ?3 WHERE id = ?1", rusqlite::params![id, read, progress])?;
    }
    tx.commit()?;
    crate::backend::bump_data();
    Ok(())
}

/// Persist reading progress: page, history entry, last-read, and "read" at the last page.
pub fn save_progress(conn: &Connection, manga_id: &str, chapter_id: &str, page: usize, total: usize) -> Result<()> {
    chapter::update_read_progress(conn, chapter_id, page as i64)?;
    history::add(conn, manga_id, chapter_id, page as i64)?;
    manga::update_last_read(conn, manga_id)?;
    if total > 0 && page + 1 >= total {
        chapter::mark_read(conn, chapter_id)?;
    }
    crate::backend::bump_data();
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

        let cs = apply_chapters(&mut conn, &m.id, "src", &[info("c1", 1.0), info("c2", 2.0)], "en").unwrap();
        assert_eq!(cs.len(), 2);
        let first = cs[0].id.clone();
        save_progress(&conn, &m.id, &first, 3, 4).unwrap();
        assert!(chapter::get(&conn, &first).unwrap().unwrap().is_read);
        assert_eq!(chapter::get(&conn, &first).unwrap().unwrap().read_progress, 3);
        assert_eq!(history::list(&conn, 10).unwrap().len(), 1);

        // c1 disappears upstream and is dropped; progress on c2 is kept across syncs
        save_progress(&conn, &m.id, &cs[1].id, 1, 10).unwrap();
        let cs = apply_chapters(&mut conn, &m.id, "src", &[info("c2", 2.0), info("c3", 3.0)], "en").unwrap();
        assert_eq!(cs.iter().map(|c| c.source_id.as_str()).collect::<Vec<_>>(), vec!["c2", "c3"]);
        assert_eq!(cs[0].read_progress, 1);

        apply_details(&conn, &m.id, &MangaDetails { description: Some("desc".into()), author: Some(String::new()), ..Default::default() }).unwrap();
        let m2 = manga::get(&conn, &m.id).unwrap().unwrap();
        assert_eq!((m2.description.as_deref(), m2.author.as_deref()), (Some("desc"), Some("A")));

        // stats + continue + mark unread
        let st = library_stats(&conn, "en").unwrap();
        assert_eq!(st[&m.id], LibStats { total: 2, unread: 2, downloaded: 0 });
        assert_eq!(continue_chapter(&conn, &m.id, "en").unwrap().as_deref(), Some(cs[0].id.as_str()));
        set_read(&mut conn, &[cs[0].id.clone()], true).unwrap();
        assert_eq!(library_stats(&conn, "en").unwrap()[&m.id].unread, 1);
        set_read(&mut conn, &[cs[0].id.clone()], false).unwrap();
        let c0 = chapter::get(&conn, &cs[0].id).unwrap().unwrap();
        assert!(!c0.is_read && c0.read_progress == 0);
        // Undo puts back both the read flag and the saved page.
        restore_read(&mut conn, &[(cs[0].id.clone(), true, 7)]).unwrap();
        let c0 = chapter::get(&conn, &cs[0].id).unwrap().unwrap();
        assert!(c0.is_read && c0.read_progress == 7);
    }

    #[test]
    fn chapters_from_several_sources_merge_one_per_number() {
        let (_d, mut conn) = db();
        let r = MangaSearchResult { source_id: "m1".into(), title: "Sword".into(), ..Default::default() };
        let m = ensure_manga(&conn, "mangadex", &r, None).unwrap();
        // The title's own source has chapters 1-3 in Polish; another source has 1-5 in English.
        let pl: Vec<ChapterInfo> = (1..=3).map(|i| ChapterInfo { language: "pl".into(), ..info(&format!("md{i}"), i as f64) }).collect();
        apply_chapters(&mut conn, &m.id, "mangadex", &pl, "en").unwrap();
        let en: Vec<ChapterInfo> = (1..=5).map(|i| info(&format!("ot{i}"), i as f64)).collect();
        let cs = apply_chapters(&mut conn, &m.id, "other", &en, "en").unwrap();
        assert_eq!(cs.len(), 5);
        assert!(cs.iter().all(|c| c.source_plugin == "other"));

        // Same language on both: one chapter per number, from the source with the most chapters,
        // except where a chapter was already read on the other one.
        let md_en: Vec<ChapterInfo> = (1..=3).map(|i| info(&format!("mde{i}"), i as f64)).collect();
        let cs = apply_chapters(&mut conn, &m.id, "mangadex", &md_en, "en").unwrap();
        assert_eq!(cs.iter().map(|c| c.number.unwrap()).collect::<Vec<_>>(), vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!(cs.iter().all(|c| c.source_plugin == "other"));
        let md2 = chapter::list(&conn, &m.id).unwrap().into_iter().find(|c| c.source_id == "mde2").unwrap();
        save_progress(&conn, &m.id, &md2.id, 1, 10).unwrap();
        let cs = chapters(&conn, &m.id, "en").unwrap();
        assert_eq!(cs.iter().map(|c| c.source_id.as_str()).collect::<Vec<_>>(), vec!["ot1", "mde2", "ot3", "ot4", "ot5"]);
        assert_eq!(library_stats(&conn, "en").unwrap()[&m.id], LibStats { total: 5, unread: 5, downloaded: 0 });
    }

    #[test]
    fn taste_facts_read_counts_library_and_categories() {
        let (_d, mut conn) = db();
        let r = MangaSearchResult { source_id: "t1".into(), title: "Taste".into(), ..Default::default() };
        let d = MangaDetails { genres: Some(vec!["Horror".into()]), author: Some("Ito".into()), ..Default::default() };
        let m = ensure_manga(&conn, "src", &r, Some(&d)).unwrap();
        let cs = apply_chapters(&mut conn, &m.id, "src", &[info("c1", 1.0), info("c2", 2.0), info("c3", 3.0)], "en").unwrap();
        save_progress(&conn, &m.id, &cs[0].id, 4, 5).unwrap();
        let cat = mv_core::repo::category::create(&conn, "Dropped").unwrap();
        mv_core::repo::category::set_manga_categories(&mut conn, &m.id, &[cat.id]).unwrap();
        let other = ensure_manga(&conn, "src", &MangaSearchResult { source_id: "t2".into(), title: "Other".into(), ..Default::default() }, None).unwrap();
        let hidden: HashSet<String> = [other.id.clone()].into_iter().collect();
        let facts = taste_facts(&conn, "en", &hidden).unwrap();
        let f = facts.iter().find(|f| f.title.title == "Taste").unwrap();
        assert_eq!((f.chapters, f.read, f.in_library), (3, 1, true));
        assert_eq!(f.title.genres, ["Horror"]);
        assert_eq!(f.categories, ["Dropped"]);
        assert!(f.last_read.is_some() && f.added > 1_600_000_000);
        assert!(!facts.iter().find(|f| f.title.title == "Other").unwrap().in_library);
        assert_eq!(sql_time("2026-01-02 03:04:05"), Some(1_767_323_045));
        assert_eq!(sql_time("nope"), None);
    }

    #[test]
    fn chapter_language_filter() {
        let (_d, mut conn) = db();
        let r = MangaSearchResult { source_id: "x2".into(), title: "L".into(), ..Default::default() };
        let m = ensure_manga(&conn, "src", &r, None).unwrap();
        let es = ChapterInfo { language: "es".into(), ..info("e1", 1.0) };
        let cs = apply_chapters(&mut conn, &m.id, "src", &[info("c1", 1.0), es.clone(), info("c2", 2.0)], "en").unwrap();
        assert_eq!(cs.iter().map(|c| c.source_id.as_str()).collect::<Vec<_>>(), vec!["c1", "c2"]);
        // the Spanish chapter is never stored, and a Spanish-only title still lists its chapters
        assert_eq!(chapter::list(&conn, &m.id).unwrap().len(), 2);
        let r2 = MangaSearchResult { source_id: "x3".into(), title: "S".into(), ..Default::default() };
        let m2 = ensure_manga(&conn, "src", &r2, None).unwrap();
        assert_eq!(apply_chapters(&mut conn, &m2.id, "src", &[es], "en").unwrap().len(), 1);
        // rows left over in other languages are hidden from the list
        chapter::add(&conn, &AddChapterInput { manga_id: m.id.clone(), source_plugin: "src".into(), source_id: "old-fr".into(), number: Some(1.0), language: Some("fr".into()), ..Default::default() }).unwrap();
        assert_eq!(chapters(&conn, &m.id, "en").unwrap().len(), 2);
        assert_eq!(library_stats(&conn, "en").unwrap()[&m.id].total, 2);
    }
}
