//! Multi-step async operations that combine a source with the database.
use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use mv_core::repo::chapter::Chapter;
use mv_core::repo::manga::{self, Manga};
use mv_sources::MangaSearchResult;
use mv_ui::bg;

use crate::{backend, store};

/// Open a source result: details + chapter sync into the database, and return the entry.
/// Titles seen for the first time are marked "browse only" (not shown in the Library until the
/// user adds them). Nothing is written when the chapter list cannot be fetched.
pub async fn prepare_from_source(source_id: String, r: MangaSearchResult) -> Result<Manga> {
    let src = backend::source(&source_id).await?;
    let details = src.get_manga_details(&r.source_id).await.ok();
    let chapters = src.get_chapter_list(&r.source_id).await.map_err(|e| anyhow!("{e}"))?;
    let lang = backend::preferred_language();
    let m = bg::blocking(move || {
        backend::with_db(|c| {
            let existed = manga::find_by_source(c, &source_id, &r.source_id)?.is_some();
            let m = store::ensure_manga(c, &source_id, &r, details.as_ref())?;
            store::apply_chapters(c, &m.id, &source_id, &chapters, &lang)?;
            if let Some(d) = &details {
                store::apply_details(c, &m.id, d)?;
            }
            backend::store_extras(&m.id, &chapters);
            if !existed {
                backend::set_in_library(&m.id, false);
            }
            Ok(manga::get(c, &m.id)?.unwrap_or(m))
        })
    })
    .await??;
    Ok(m)
}

/// Re-read details and chapters from the manga's source.
pub async fn refresh_manga(manga_id: String) -> Result<(Manga, Vec<Chapter>)> {
    let id = manga_id.clone();
    let m = bg::blocking(move || backend::with_db(|c| manga::get(c, &id)?.ok_or_else(|| anyhow!("Manga not found")))).await??;
    let src = backend::source(&m.source_plugin).await?;
    let details = src.get_manga_details(&m.source_id).await.ok();
    let chapters = src.get_chapter_list(&m.source_id).await.map_err(|e| anyhow!("{e}"))?;
    let lang = backend::preferred_language();
    let res = bg::blocking(move || {
        backend::with_db(|c| {
            let list = store::apply_chapters(c, &m.id, &m.source_plugin, &chapters, &lang)?;
            if let Some(d) = &details {
                store::apply_details(c, &m.id, d)?;
            }
            backend::store_extras(&m.id, &chapters);
            Ok((manga::get(c, &m.id)?.unwrap_or(m), list))
        })
    })
    .await?;
    backend::bump_data();
    res
}

// ---------------------------------------------------------------- the same title in other sources

/// Another installed source that has the same title.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Link {
    pub source: String,
    pub id: String,
    pub title: String,
}

/// `mangaLinks.<manga id>` in the config: the title's other sources and when they were looked for.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Links {
    pub checked_at: i64,
    pub links: Vec<Link>,
}

const LINKS_KEY: &str = "mangaLinks";
/// Other sources are looked for again after this long (a refresh re-reads the known ones).
const RESCAN_SECS: i64 = 3 * 24 * 3600;

pub fn links(manga_id: &str) -> Option<Links> {
    let all = mv_core::config::Config::open_default().get(LINKS_KEY)?;
    serde_json::from_value(all.get(manga_id)?.clone()).ok()
}

fn save_links(manga_id: &str, l: &Links) -> Result<()> {
    let cfg = mv_core::config::Config::open_default();
    let mut all = cfg.get(LINKS_KEY).filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    all[manga_id] = serde_json::to_value(l)?;
    cfg.set(LINKS_KEY, all)
}

/// What [`sync_other_sources`] did.
#[derive(Clone, Debug, Default)]
pub struct MergeNote {
    /// Chapter numbers the title has now that it did not have before.
    pub added: usize,
    /// Names of the sources the listed chapters come from (other than the title's own).
    pub from: Vec<String>,
    /// The title's new name, when a source with more chapters calls it something better.
    pub renamed: Option<String>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn number_keys(cs: &[Chapter]) -> HashSet<i64> {
    cs.iter().filter_map(|c| c.number).map(|n| (n * 1000.0).round() as i64).collect()
}

/// The closest result to any of `names` (0.85+ title similarity, other names included).
fn best_match(list: Vec<MangaSearchResult>, names: &[String]) -> Option<MangaSearchResult> {
    let score = |r: &MangaSearchResult| {
        std::iter::once(&r.title)
            .chain(r.alt_titles.iter().flatten())
            .flat_map(|t| names.iter().map(move |n| mv_sources::util::title_similarity(t, n)))
            .fold(0.0, f64::max)
    };
    list.into_iter().map(|r| (score(&r), r)).filter(|(s, _)| *s >= 0.85).max_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, r)| r)
}

type Found = (Link, String, Vec<mv_sources::ChapterInfo>);

/// What a look through the other sources found.
#[derive(Default)]
struct Scan {
    found: Vec<Found>,
    /// Every name the title was searched by.
    names: Vec<String>,
    /// The name the title's own source gives it now (it may have switched to an English one).
    own_title: Option<String>,
}

/// Searches the other installed sources for the title (by its name and the other names its own
/// source knows) and reads their chapter lists. 18+ sources are only searched for 18+ titles.
async fn find_elsewhere(m: &Manga) -> Scan {
    let reg = backend::registry().await;
    let own = reg.get(&m.source_plugin);
    let adult = own.as_ref().is_some_and(|s| s.info().nsfw);
    let mut names = vec![m.title.clone()];
    let mut own_title = None;
    if let Some(src) = &own {
        if let Some(Ok(list)) = backend::within(20, src.search_manga(&m.title, 1)).await {
            if let Some(r) = list.into_iter().find(|r| r.source_id == m.source_id) {
                names.push(r.title.clone());
                names.extend(r.alt_titles.unwrap_or_default());
                own_title = Some(r.title);
            }
        }
    }
    let mut seen = HashSet::new();
    names.retain(|n| mv_sources::util::is_latin(n) && seen.insert(mv_sources::util::title_key(n)));
    names.truncate(4);
    let others: Vec<_> = backend::installed_sources(reg.list())
        .into_iter()
        .filter(|s| s.id != m.source_plugin && (adult || !s.nsfw))
        .filter_map(|s| reg.get(&s.id).map(|src| (s, src)))
        .collect();
    let searches = others.into_iter().map(|(info, src)| {
        let names = names.clone();
        async move {
            for n in names.iter().take(3) {
                let Some(Ok(list)) = backend::within(20, src.search_manga(n, 1)).await else { continue };
                let Some(hit) = best_match(list, &names) else { continue };
                let chapters = backend::within(40, src.get_chapter_list(&hit.source_id)).await?.ok().filter(|c| !c.is_empty())?;
                return Some((Link { source: info.id.clone(), id: hit.source_id, title: hit.title }, info.name.clone(), chapters));
            }
            None
        }
    });
    let found = futures::future::join_all(searches).await.into_iter().flatten().collect();
    Scan { found, names, own_title }
}

/// The title's best name after a scan: the name its own source gives it now, when that changed
/// (MangaDex shows the English name of a manhwa); else the name used by the source that gives
/// the most chapters, when it gives more than the title's own source. A name the source only
/// echoed back from the search is not a new name.
fn better_name(current: &str, scan: &Scan, count: &HashMap<&str, usize>, own_source: &str) -> Option<String> {
    let key = mv_sources::util::title_key;
    let usable = |t: &str| mv_sources::util::is_latin(t) && !t.trim().is_empty() && key(t) != key(current);
    if let Some(t) = scan.own_title.as_deref().filter(|t| usable(t)) {
        return Some(t.to_string());
    }
    let own = count.get(own_source).copied().unwrap_or(0);
    let searched: HashSet<String> = scan.names.iter().map(|n| key(n)).collect();
    scan.found
        .iter()
        .map(|(l, _, _)| (count.get(l.source.as_str()).copied().unwrap_or(0), l))
        .filter(|(n, l)| *n > own && usable(&l.title) && !searched.contains(&key(&l.title)))
        .max_by_key(|(n, _)| *n)
        .map(|(_, l)| l.title.clone())
}

/// Adds the chapters other installed sources have for this title, so a title whose own source
/// has few (9 Polish chapters) lists every chapter any source has (one per number, see
/// [`store::one_source_per_number`]). Other sources are looked for when `rescan` is set or the
/// last look is 3+ days old; otherwise the known ones are re-read. When the source that gives
/// the most chapters names the title differently (an English name for a romanized one), the
/// title takes that name.
pub async fn sync_other_sources(manga_id: String, rescan: bool) -> Result<MergeNote> {
    let id = manga_id.clone();
    let m = bg::blocking(move || backend::with_db(|c| manga::get(c, &id)?.ok_or_else(|| anyhow!("Manga not found")))).await??;
    let known = links(&m.id).unwrap_or_default();
    let scan: Scan = if rescan || now() - known.checked_at > RESCAN_SECS {
        find_elsewhere(&m).await
    } else {
        let reg = backend::registry().await;
        let reads = known.links.iter().cloned().filter_map(|l| reg.get(&l.source).map(|s| (l, s))).map(|(l, src)| async move {
            let chapters = backend::within(40, src.get_chapter_list(&l.id)).await?.ok().filter(|c| !c.is_empty())?;
            Some((l, src.info().name, chapters))
        });
        Scan { found: futures::future::join_all(reads).await.into_iter().flatten().collect(), ..Default::default() }
    };
    let lang = backend::preferred_language();
    let gone: Vec<String> = known.links.iter().filter(|l| !scan.found.iter().any(|f| f.0.source == l.source)).map(|l| l.source.clone()).collect();
    let note = bg::blocking(move || {
        backend::with_db(|c| {
            let found = &scan.found;
            let before = number_keys(&store::chapters(c, &m.id, &lang)?);
            for (link, _, chapters) in found {
                store::apply_chapters(c, &m.id, &link.source, chapters, &lang)?;
            }
            // A source that no longer has the title keeps only downloaded chapters.
            for source in &gone {
                store::apply_chapters(c, &m.id, source, &[], &lang)?;
            }
            let after = store::chapters(c, &m.id, &lang)?;
            let added = number_keys(&after).difference(&before).count();
            let from: Vec<String> = found.iter().filter(|(l, _, _)| after.iter().any(|ch| ch.source_plugin == l.source)).map(|(_, name, _)| name.clone()).collect();
            let mut count: HashMap<&str, usize> = HashMap::new();
            for ch in &after {
                *count.entry(ch.source_plugin.as_str()).or_default() += 1;
            }
            let renamed = better_name(&m.title, &scan, &count, &m.source_plugin);
            if let Some(t) = &renamed {
                c.execute("UPDATE manga SET title = ?1 WHERE id = ?2", rusqlite::params![t, m.id])?;
            }
            save_links(&m.id, &Links { checked_at: now(), links: found.iter().map(|f| f.0.clone()).collect() })?;
            Ok(MergeNote { added, from, renamed })
        })
    })
    .await??;
    backend::bump_data();
    Ok(note)
}

/// The title has never been looked for in the other sources, or not for 3+ days.
pub fn needs_scan(manga_id: &str) -> bool {
    links(manga_id).is_none_or(|l| now() - l.checked_at > RESCAN_SECS)
}

#[cfg(test)]
mod tests {
    use super::{better_name, Link, Scan};
    use std::collections::HashMap;

    fn link(source: &str, title: &str) -> (Link, String, Vec<mv_sources::ChapterInfo>) {
        (Link { source: source.into(), id: "x".into(), title: title.into() }, source.into(), vec![])
    }

    #[test]
    fn picks_the_better_name() {
        let names = vec!["Geom Meokneun Swordmaster".to_string(), "The Sword-Eating Swordmaster".to_string()];
        let count: HashMap<&str, usize> = [("mangadex", 0), ("comick", 52)].into_iter().collect();
        // The title's own source now gives the English name.
        let s = Scan { found: vec![link("comick", "Geom Meokneun Swordmaster")], names: names.clone(), own_title: Some("The Sword-Eating Swordmaster".into()) };
        assert_eq!(better_name("Geom Meokneun Swordmaster", &s, &count, "mangadex").as_deref(), Some("The Sword-Eating Swordmaster"));
        // A source that only echoes a searched name back does not rename it.
        let s = Scan { found: vec![link("comick", "Geom Meokneun Swordmaster")], names: names.clone(), own_title: None };
        assert_eq!(better_name("Geom Meokneun Swordmaster", &s, &count, "mangadex"), None);
        // A source with more chapters and a name of its own does.
        let s = Scan { found: vec![link("comick", "Sword Devouring Swordmaster")], names, own_title: Some("Geom Meokneun Swordmaster".into()) };
        assert_eq!(better_name("Geom Meokneun Swordmaster", &s, &count, "mangadex").as_deref(), Some("Sword Devouring Swordmaster"));
    }
}
