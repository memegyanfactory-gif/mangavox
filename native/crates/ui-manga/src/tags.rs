//! Pure helpers for tag browsing and "You may also like": matching genre names to a source's
//! tag list, grouping tags for the picker, choosing the tags a recommendation query uses and
//! ranking / de-duplicating the results.
use std::collections::HashMap;

use mv_sources::{MangaSearchResult, Tag};

/// How many recommendations the detail page shows at most.
pub const MAX_RECS: usize = 8;
/// Fewer than this and the next (broader) query is tried.
pub const MIN_RECS: usize = 5;

/// Comparison key for tag / genre / title names: lowercase letters and digits only, so
/// "Sci-Fi", "sci fi" and "SciFi" match.
pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The tag a genre chip refers to (by name, or by id for sources whose ids are slugs).
pub fn find_tag<'a>(tags: &'a [Tag], name: &str) -> Option<&'a Tag> {
    let key = norm(name);
    if key.is_empty() {
        return None;
    }
    tags.iter().find(|t| norm(&t.name) == key).or_else(|| tags.iter().find(|t| norm(&t.id) == key))
}

/// Tags grouped for the picker, groups in first-appearance order (sources already sort them).
/// Ungrouped tags land in "Tags".
pub fn group_tags(tags: &[Tag]) -> Vec<(String, Vec<Tag>)> {
    let mut out: Vec<(String, Vec<Tag>)> = Vec::new();
    for t in tags {
        let g = t.group.clone().unwrap_or_else(|| "Tags".into());
        match out.iter_mut().find(|(name, _)| *name == g) {
            Some((_, v)) => v.push(t.clone()),
            None => out.push((g, vec![t.clone()])),
        }
    }
    out
}

/// Formats / languages / publishing facts that say little about what a story is like.
const GENERIC: [&str; 18] = [
    "longstrip", "webcomic", "fullcolor", "oneshot", "adaptation", "officialcolored", "fancolored", "anthology", "4koma", "doujinshi",
    "awardwinning", "usercreated", "manga", "manhwa", "manhua", "english", "japanese", "translated",
];

/// Genres so common that alone they say little ("Action" covers half the catalogue).
const BROAD: [&str; 7] = ["action", "adventure", "comedy", "drama", "fantasy", "romance", "sliceoflife"];

/// 0 = specific genre (Horror, Mystery, ...), 1 = theme, 2 = broad genre, 3 = demographic,
/// 4 = anything else (formats, content warnings: never used).
fn rank(t: &Tag) -> u8 {
    match t.group.as_deref() {
        None | Some("Genres") if BROAD.contains(&norm(&t.name).as_str()) => 2,
        None | Some("Genres") => 0,
        Some("Themes") => 1,
        Some("Demographics") => 3,
        _ => 4,
    }
}

/// Up to `max` of the title's genres that exist in `tags`, most telling first: specific genres,
/// themes, broad genres, then demographics (formats like "Long Strip" are skipped). Keeps the
/// title's order within a rank.
pub fn pick_rec_tags(genres: &[String], tags: &[Tag], max: usize) -> Vec<Tag> {
    let mut picked: Vec<Tag> = Vec::new();
    for g in genres {
        if GENERIC.contains(&norm(g).as_str()) {
            continue;
        }
        if let Some(t) = find_tag(tags, g) {
            if !picked.iter().any(|p| p.id == t.id) && !GENERIC.contains(&norm(&t.name).as_str()) && rank(t) < 4 {
                picked.push(t.clone());
            }
        }
    }
    picked.sort_by_key(rank);
    picked.truncate(max);
    picked
}

/// Tag-id sets to query, most specific first. Sources that combine tags get the narrowing
/// AND queries (3 tags, 2, 1); others one query per tag (results are ranked by overlap).
pub fn query_plan(picked: &[Tag], multi: bool) -> Vec<Vec<String>> {
    let ids: Vec<String> = picked.iter().map(|t| t.id.clone()).collect();
    if multi {
        (1..=ids.len()).rev().map(|n| ids[..n].to_vec()).collect()
    } else {
        ids.into_iter().map(|i| vec![i]).collect()
    }
}

/// Merge result lists into at most `max` unique titles, excluding the title itself (by
/// `exclude_id` when the lists come from its own source, and always by title). Titles found by
/// more queries rank first, then by where they first appeared (earlier lists are more specific).
pub fn merge_recs(lists: &[Vec<MangaSearchResult>], exclude_id: Option<&str>, exclude_title: &str, max: usize) -> Vec<MangaSearchResult> {
    let own_title = norm(exclude_title);
    // key -> (hits, first position, item)
    let mut seen: HashMap<String, (usize, usize, MangaSearchResult)> = HashMap::new();
    let mut by_title: HashMap<String, String> = HashMap::new();
    let mut pos = 0usize;
    for list in lists {
        let mut in_this: Vec<String> = Vec::new();
        for it in list {
            pos += 1;
            let t = norm(&it.title);
            if it.source_id.is_empty() || Some(it.source_id.as_str()) == exclude_id || (!own_title.is_empty() && t == own_title) {
                continue;
            }
            // Same title under another id (duplicate uploads) counts as the same entry.
            let key = match by_title.get(&t) {
                Some(k) if !t.is_empty() => k.clone(),
                _ => it.source_id.clone(),
            };
            if !t.is_empty() {
                by_title.entry(t).or_insert_with(|| key.clone());
            }
            if in_this.contains(&key) {
                continue;
            }
            in_this.push(key.clone());
            seen.entry(key).and_modify(|e| e.0 += 1).or_insert((1, pos, it.clone()));
        }
    }
    let mut v: Vec<(usize, usize, MangaSearchResult)> = seen.into_values().collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    v.into_iter().take(max).map(|(_, _, it)| it).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, name: &str, group: Option<&str>) -> Tag {
        Tag { id: id.into(), name: name.into(), group: group.map(Into::into) }
    }

    fn r(id: &str, title: &str) -> MangaSearchResult {
        MangaSearchResult { source_id: id.into(), title: title.into(), ..Default::default() }
    }

    fn ids(v: &[MangaSearchResult]) -> Vec<&str> {
        v.iter().map(|x| x.source_id.as_str()).collect()
    }

    fn sample() -> Vec<Tag> {
        vec![
            t("u-action", "Action", Some("Genres")),
            t("u-romance", "Romance", Some("Genres")),
            t("u-scifi", "Sci-Fi", Some("Genres")),
            t("u-isekai", "Isekai", Some("Themes")),
            t("u-strip", "Long Strip", Some("Formats")),
            t("u-color", "Full Color", Some("Formats")),
            t("demographic:seinen", "Seinen", Some("Demographics")),
            t("u-gore", "Gore", Some("Content")),
            t("u-horror", "Horror", Some("Genres")),
            t("u-monsters", "Monsters", Some("Themes")),
        ]
    }

    #[test]
    fn find_tag_matches_loosely_by_name_or_id() {
        let tags = sample();
        assert_eq!(find_tag(&tags, "sci fi").unwrap().id, "u-scifi");
        assert_eq!(find_tag(&tags, "ACTION").unwrap().id, "u-action");
        assert_eq!(find_tag(&tags, "u-isekai").unwrap().name, "Isekai");
        assert!(find_tag(&tags, "Mystery").is_none());
        assert!(find_tag(&tags, " - ").is_none());
    }

    #[test]
    fn group_tags_keeps_source_order() {
        let mut tags = sample();
        tags.push(t("x", "Loose", None));
        let g = group_tags(&tags);
        let names: Vec<&str> = g.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Genres", "Themes", "Formats", "Demographics", "Content", "Tags"]);
        assert_eq!(g[0].1.len(), 4);
        assert_eq!(g[2].1[0].name, "Long Strip");
    }

    #[test]
    fn pick_rec_tags_prefers_genres_and_skips_formats() {
        let genres: Vec<String> = ["Long Strip", "Seinen", "Isekai", "Full Color", "Action", "Gore", "Unknown", "Action", "Romance"].iter().map(|s| s.to_string()).collect();
        let p = pick_rec_tags(&genres, &sample(), 3);
        let names: Vec<&str> = p.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Isekai", "Action", "Romance"], "a theme beats broad genres");
        let p = pick_rec_tags(&genres, &sample(), 10);
        let names: Vec<&str> = p.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Isekai", "Action", "Romance", "Seinen"], "content / format tags never drive recommendations");
        // Berserk-like: the specific genre and the theme come before Action.
        let berserk: Vec<String> = ["Action", "Horror", "Monsters", "Seinen"].iter().map(|s| s.to_string()).collect();
        let names: Vec<String> = pick_rec_tags(&berserk, &sample(), 3).into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["Horror", "Monsters", "Action"]);
        assert!(pick_rec_tags(&["Long Strip".to_string()], &sample(), 3).is_empty());
        assert!(pick_rec_tags(&genres, &[], 3).is_empty());
    }

    #[test]
    fn query_plan_narrows_or_splits() {
        let p = pick_rec_tags(&["Action".into(), "Romance".into(), "Isekai".into()], &sample(), 3);
        assert_eq!(query_plan(&p, true), vec![vec!["u-isekai", "u-action", "u-romance"], vec!["u-isekai", "u-action"], vec!["u-isekai"]]);
        assert_eq!(query_plan(&p, false), vec![vec!["u-isekai"], vec!["u-action"], vec!["u-romance"]]);
        assert!(query_plan(&[], true).is_empty());
    }

    #[test]
    fn merge_recs_ranks_overlap_dedupes_and_excludes_self() {
        let a = vec![r("self", "Berserk"), r("1", "Vagabond"), r("2", "Claymore"), r("3", "Gantz")];
        let b = vec![r("2", "Claymore"), r("4", "Hellsing"), r("other-berserk", "BERSERK"), r("5", "Vagabond")];
        let c = vec![r("4", "Hellsing"), r("2", "Claymore"), r("2", "Claymore"), r("", "No id")];
        let m = merge_recs(&[a, b, c], Some("self"), "Berserk", 8);
        // Claymore in 3 lists, Hellsing in 2, Vagabond in 2 (same title, other id) but seen first.
        assert_eq!(ids(&m), ["2", "1", "4", "3"]);
        assert_eq!(merge_recs(&[ids_to(&["a", "b", "c"])], None, "", 2).len(), 2);
        assert!(merge_recs(&[], None, "x", 8).is_empty());
    }

    fn ids_to(v: &[&str]) -> Vec<MangaSearchResult> {
        v.iter().map(|i| r(i, &format!("Title {i}"))).collect()
    }
}
