//! Seasons of an anime. MAL and AniList list every season as its own entry, joined by
//! SEQUEL / PREQUEL relations, while IMDb and Cinemeta list one series with seasons. This builds
//! the real season list of a show from those relations, and maps a season's own episode numbers
//! onto the IMDb series (by how many episodes the earlier seasons had), so S2E3 is never played
//! as S1E3.
//!
//! - Only TV, TV short and ONA entries are numbered seasons. Movies, OVAs and specials are walked
//!   through (a sequel can follow a movie) but kept out of the list.
//! - The AniList lookup costs one request per step along the chain and is cached for 6 hours under
//!   every entry of the franchise, so opening any season again costs nothing.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::anilist::AniListClient;
use crate::episode_art;
use crate::error::{MediaError, Result};
use crate::jikan::JikanClient;
use crate::title_match;

const CACHE_TTL: Duration = Duration::from_secs(6 * 3600);

/// A relation to another entry, as AniList reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// "SEQUEL", "PREQUEL", "SIDE_STORY", ...
    pub relation: String,
    pub anilist_id: u64,
    /// False for manga and novel adaptations.
    pub is_anime: bool,
}

/// One AniList entry with what the season list needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeasonNode {
    pub anilist_id: u64,
    pub mal_id: Option<u64>,
    pub title: String,
    /// English and romaji titles (the ones that carry "Season 2" / "Part 2").
    pub titles: Vec<String>,
    /// AniList format: "TV", "TV_SHORT", "ONA", "MOVIE", "OVA", "SPECIAL", ...
    pub format: Option<String>,
    pub episodes: Option<u32>,
    pub year: Option<i32>,
    /// (year, month, day) it started, 0 for a part AniList does not know. `None` when unknown.
    pub start: Option<(i32, u32, u32)>,
    pub links: Vec<Link>,
}

/// One numbered season of a show. `mal_id` opens it (its own episodes, sources and progress).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Season {
    pub mal_id: String,
    /// "Season 1", "Season 4 Part 2".
    pub label: String,
    pub title: String,
    /// Episodes it has (or will have); `None` when not announced.
    pub episodes: Option<u32>,
    pub year: Option<i32>,
}

/// Relations that lead to the next or previous season. Spin-offs, side stories and alternative
/// versions are other shows and are not followed.
pub fn is_chain_relation(relation: &str) -> bool {
    matches!(relation, "SEQUEL" | "PREQUEL")
}

fn is_season_format(format: Option<&str>) -> bool {
    matches!(format, Some("TV" | "TV_SHORT" | "ONA"))
}

/// Entries joined to `start` by sequel / prequel relations (through any format), by AniList id.
fn franchise(nodes: &[SeasonNode], start: u64) -> HashSet<u64> {
    let by_id: HashMap<u64, &SeasonNode> = nodes.iter().map(|n| (n.anilist_id, n)).collect();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = by_id.get(&id) else { continue };
        for l in node.links.iter().filter(|l| l.is_anime && is_chain_relation(&l.relation)) {
            queue.push_back(l.anilist_id);
        }
    }
    seen
}

/// MAL ids of every entry in the franchise of `start` (movies and OVAs too).
pub fn franchise_mal_ids(nodes: &[SeasonNode], start: u64) -> Vec<u64> {
    let ids = franchise(nodes, start);
    nodes.iter().filter(|n| ids.contains(&n.anilist_id)).filter_map(|n| n.mal_id).collect()
}

/// The numbered seasons of the show `start` belongs to, in the order they aired, labelled.
/// Empty when `nodes` has no season. A show with one season gives a list of one.
pub fn build_seasons(nodes: &[SeasonNode], start: u64) -> Vec<Season> {
    let ids = franchise(nodes, start);
    let mut picks: Vec<&SeasonNode> = nodes.iter().filter(|n| ids.contains(&n.anilist_id) && n.mal_id.is_some() && is_season_format(n.format.as_deref())).collect();
    // Air order; entries with no date go last (announced, not out yet).
    picks.sort_by_key(|n| (n.start.is_none(), n.start.unwrap_or_default(), n.anilist_id));
    let mut out: Vec<Season> = Vec::new();
    let mut number = 0;
    let mut previous: Option<&SeasonNode> = None;
    for n in picks {
        let part = n.titles.iter().filter_map(|t| title_match::part_marker(t)).find(|&p| p >= 2);
        // "Part 2" of the entry just before it keeps that season's number; a different show that
        // has "Part 5" in its name is a season of its own.
        let continues = previous.is_some_and(|p| p.titles.iter().any(|a| n.titles.iter().any(|b| title_match::continues(a, b))));
        let label = match part {
            Some(p) if continues && number > 0 => format!("Season {number} Part {p}"),
            _ => {
                number += 1;
                format!("Season {number}")
            }
        };
        out.push(Season { mal_id: n.mal_id.unwrap_or_default().to_string(), label, title: n.title.clone(), episodes: n.episodes.filter(|&e| e > 0), year: n.year });
        previous = Some(n);
    }
    out
}

type Cache = Mutex<HashMap<String, (Instant, Arc<Vec<Season>>)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn cache_get(mal: &str) -> Option<Arc<Vec<Season>>> {
    let c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.get(mal).filter(|(at, _)| at.elapsed() < CACHE_TTL).map(|(_, l)| l.clone())
}

/// How long a failed lookup is remembered, so a slow or unreachable AniList is not asked again
/// on every episode click.
const FAIL_TTL: Duration = Duration::from_secs(60);

fn failures() -> &'static Mutex<HashMap<String, Instant>> {
    static F: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    F.get_or_init(Default::default)
}

fn recently_failed(mal: &str) -> bool {
    let f = failures().lock().unwrap_or_else(|e| e.into_inner());
    f.get(mal).is_some_and(|at| at.elapsed() < FAIL_TTL)
}

/// The seasons of the show that MAL entry `mal` belongs to (cached; one AniList request per step
/// along the sequel chain on a miss). The list can be empty or lack `mal` itself when `mal` is a
/// movie or special: callers then show no season switcher.
/// Lookups run one at a time and re-check the cache once it is their turn, so the page, the
/// source search and the thumbnails opening together share one crawl instead of three.
pub async fn seasons_for(mal: &str) -> Result<Arc<Vec<Season>>> {
    if let Some(hit) = cache_get(mal) {
        return Ok(hit);
    }
    static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _turn = TURN.lock().await;
    if let Some(hit) = cache_get(mal) {
        return Ok(hit);
    }
    if recently_failed(mal) {
        return Err(MediaError::NotFound(format!("seasons of anime {mal} (lookup failed a moment ago)")));
    }
    let (start, nodes) = match AniListClient::shared()?.season_nodes(mal).await {
        Ok(found) => found,
        Err(e) => {
            failures().lock().unwrap_or_else(|e| e.into_inner()).insert(mal.to_string(), Instant::now());
            return Err(e);
        }
    };
    let list = Arc::new(build_seasons(&nodes, start));
    let mut members: Vec<String> = franchise_mal_ids(&nodes, start).iter().map(|m| m.to_string()).collect();
    members.push(mal.to_string());
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    for m in members {
        c.insert(m, (now, list.clone()));
    }
    Ok(list)
}

/// The seasons before `mal` in `list`; `None` when `mal` is not one of them (a movie or special).
fn earlier<'a>(list: &'a [Season], mal: &str) -> Option<&'a [Season]> {
    let at = list.iter().position(|s| s.mal_id == mal)?;
    Some(&list[..at])
}

/// Episodes of the earlier seasons that sit in the same IMDb series (`imdbs[i]` is the IMDb id of
/// `earlier[i]`). `None` when one of them has no known episode count or no IMDb id.
fn shared_episodes(earlier: &[Season], imdbs: &[Option<String>], imdb: &str) -> Option<u32> {
    let mut total = 0u32;
    for (s, i) in earlier.iter().zip(imdbs) {
        match i.as_deref() {
            Some(other) if other != imdb => {}
            Some(_) => total = total.saturating_add(s.episodes?),
            // Unmapped (or the lookup failed): it may well belong to this series, so the
            // offset is unknown rather than guessed.
            None => return None,
        }
    }
    Some(total)
}

/// How many episodes of IMDb series `imdb` come before the first episode of MAL entry `mal`:
/// the episode counts of the earlier seasons that map to the same IMDb series. `Some(0)` for a
/// first season (or a season IMDb lists as its own series). `None` when it cannot be known
/// (the season list is unavailable, `mal` is not a numbered season, or an earlier season has no
/// episode count), so callers skip the IMDb id rather than guess the wrong episode.
pub async fn imdb_offset(mal: &str, imdb: &str) -> Option<u32> {
    let list = seasons_for(mal).await.ok()?;
    let before = earlier(&list, mal)?;
    if before.is_empty() {
        return Some(0);
    }
    let jikan = JikanClient::shared().ok()?;
    let imdbs = await_in_order(before.iter().map(|s| jikan.imdb_id(&s.mal_id))).await;
    shared_episodes(before, &imdbs, imdb)
}

/// Awaits the futures one after another (arm.haglund answers from the cache after the first).
async fn await_in_order<F: std::future::Future<Output = Option<String>>>(items: impl Iterator<Item = F>) -> Vec<Option<String>> {
    let mut out = Vec::new();
    for f in items {
        out.push(f.await);
    }
    out
}

/// IMDb `(season, episode)` of episode `episode` of MAL entry `mal`, which IMDb series `imdb`
/// holds together with the other seasons. `None` when it cannot be placed.
pub async fn imdb_episode_of(mal: &str, imdb: &str, episode: u32) -> Option<(u32, u32)> {
    let offset = imdb_offset(mal, imdb).await?;
    match episode_art::imdb_episode(imdb, offset.saturating_add(episode)).await {
        Some(found) => Some(found),
        // IMDb's list is not reachable: a first season is still S1 E{n}.
        None if offset == 0 => Some((1, episode)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(relation: &str, id: u64) -> Link {
        Link { relation: relation.into(), anilist_id: id, is_anime: true }
    }

    fn node(id: u64, title: &str, format: &str, episodes: u32, start: (i32, u32, u32), links: Vec<Link>) -> SeasonNode {
        SeasonNode {
            anilist_id: id,
            mal_id: Some(id * 10),
            title: title.into(),
            titles: vec![title.into()],
            format: Some(format.into()),
            episodes: Some(episodes),
            year: Some(start.0),
            start: Some(start),
            links,
        }
    }

    /// S1 -> S2 -> movie -> S3 -> S3 Part 2, plus an unrelated spin-off and an OVA.
    fn show() -> Vec<SeasonNode> {
        vec![
            node(3, "Show Season 3", "TV", 12, (2019, 1, 1), vec![link("PREQUEL", 9), link("SEQUEL", 4)]),
            node(1, "Show", "TV", 25, (2013, 4, 1), vec![link("SEQUEL", 2), link("SIDE_STORY", 7)]),
            node(4, "Show Season 3 Part 2", "TV", 10, (2019, 7, 1), vec![link("PREQUEL", 3)]),
            node(2, "Show Season 2", "TV", 12, (2017, 4, 1), vec![link("PREQUEL", 1), link("SEQUEL", 9)]),
            node(9, "Show: The Movie", "MOVIE", 1, (2018, 6, 1), vec![link("PREQUEL", 2), link("SEQUEL", 3)]),
            node(7, "Show: Junior High", "TV", 12, (2015, 1, 1), vec![link("SIDE_STORY", 1)]),
            node(8, "Show OVA", "OVA", 2, (2014, 1, 1), vec![link("SIDE_STORY", 1)]),
        ]
    }

    #[test]
    fn seasons_follow_sequels_through_movies_and_skip_extras() {
        let list = build_seasons(&show(), 2);
        let got: Vec<(&str, &str)> = list.iter().map(|s| (s.mal_id.as_str(), s.label.as_str())).collect();
        assert_eq!(got, [("10", "Season 1"), ("20", "Season 2"), ("30", "Season 3"), ("40", "Season 3 Part 2")]);
        // The same list from any entry of the franchise, the movie included.
        assert_eq!(build_seasons(&show(), 4), list);
        assert_eq!(build_seasons(&show(), 9), list);
        assert_eq!(list[1].episodes, Some(12));
        assert_eq!(list[0].year, Some(2013));
    }

    #[test]
    fn side_stories_are_other_shows() {
        // The spin-off is not reached by sequel / prequel relations: its own list is empty.
        assert_eq!(build_seasons(&show(), 7).iter().map(|s| s.mal_id.as_str()).collect::<Vec<_>>(), ["70"]);
        assert_eq!(franchise_mal_ids(&show(), 1).len(), 5);
    }

    #[test]
    fn a_lone_show_is_one_season_and_unknown_dates_go_last() {
        let one = vec![node(1, "Solo", "TV", 12, (2020, 1, 1), vec![])];
        assert_eq!(build_seasons(&one, 1).len(), 1);
        let mut nodes = show();
        nodes.push(SeasonNode { start: None, year: None, episodes: None, ..node(5, "Show Season 4", "TV", 0, (0, 0, 0), vec![link("PREQUEL", 4)]) });
        nodes.iter_mut().find(|n| n.anilist_id == 4).unwrap().links.push(link("SEQUEL", 5));
        let list = build_seasons(&nodes, 1);
        assert_eq!(list.last().map(|s| (s.label.as_str(), s.episodes)), Some(("Season 4", None)));
    }

    #[test]
    fn part_two_of_another_show_is_its_own_season() {
        let nodes = vec![
            node(1, "JoJo: Diamond is Unbreakable", "TV", 39, (2016, 4, 1), vec![link("SEQUEL", 2)]),
            node(2, "JoJo Part 5: Golden Wind", "TV", 39, (2018, 10, 1), vec![link("PREQUEL", 1)]),
        ];
        let labels: Vec<_> = build_seasons(&nodes, 1).into_iter().map(|s| s.label).collect();
        assert_eq!(labels, ["Season 1", "Season 2"]);
    }

    #[test]
    fn a_movie_alone_has_no_seasons_and_is_not_in_the_list() {
        let movie = vec![node(1, "Film", "MOVIE", 1, (2020, 1, 1), vec![])];
        assert!(build_seasons(&movie, 1).is_empty());
    }

    #[test]
    fn offset_counts_earlier_seasons_of_the_same_imdb_series() {
        let list = build_seasons(&show(), 1);
        assert_eq!(earlier(&list, "10").map(<[Season]>::len), Some(0));
        let before = earlier(&list, "30").unwrap();
        assert_eq!(before.len(), 2);
        let same = vec![Some("tt1".to_string()), Some("tt1".to_string())];
        assert_eq!(shared_episodes(before, &same, "tt1"), Some(25 + 12));
        // Season 2 is listed as its own IMDb series: it does not count.
        let split = vec![Some("tt1".to_string()), Some("tt2".to_string())];
        assert_eq!(shared_episodes(before, &split, "tt1"), Some(25));
        // Not a numbered season (the movie), or an earlier season without a count.
        assert!(earlier(&list, "90").is_none());
        let mut open = list.clone();
        open[0].episodes = None;
        assert_eq!(shared_episodes(&open[..2], &same, "tt1"), None);
        // An earlier season with no IMDb id may belong to this series: unknown, not zero.
        let gap = vec![None, Some("tt1".to_string())];
        assert_eq!(shared_episodes(before, &gap, "tt1"), None);
    }
}
