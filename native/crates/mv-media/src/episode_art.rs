//! Per-episode stills for anime. Jikan has none and AniList only covers a few streaming
//! episodes, so a long show (One Piece) would show the series banner on every card.
//! Fills `thumbnail` from Kitsu (exact per-MAL-entry numbering), then from Cinemeta /
//! metahub through the IMDB id when that series lines up with the MAL entry.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

use crate::cinemeta::CinemetaClient;
use crate::jikan::JikanClient;
use crate::types::AnimeEpisode;

const KITSU: &str = "https://kitsu.io/api/edge";
/// Kitsu's largest page size.
const KITSU_PAGE: u32 = 20;
const TIMEOUT: Duration = Duration::from_secs(12);

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("MangaVox/", env!("CARGO_PKG_VERSION"), " (mv-media)"))
            .timeout(TIMEOUT)
            .build()
            .unwrap_or_default()
    })
}

async fn get_json(url: &str) -> Option<Value> {
    let res = http().get(url).header("Accept", "application/vnd.api+json").send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    res.json().await.ok()
}

type Cache<T> = Mutex<HashMap<String, T>>;

fn kitsu_ids() -> &'static Cache<Option<String>> {
    static C: OnceLock<Cache<Option<String>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// IMDB episodes in absolute order (title, still), per MAL id (`None`: no IMDB series).
type Stills = Arc<Vec<(Option<String>, String)>>;
fn imdb_cache() -> &'static Cache<Option<Stills>> {
    static C: OnceLock<Cache<Option<Stills>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn verdicts() -> &'static Cache<bool> {
    static C: OnceLock<Cache<bool>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Kitsu id for a MAL anime (cached; `None` when Kitsu has no mapping).
pub async fn kitsu_id(mal_id: &str) -> Option<String> {
    if let Some(hit) = kitsu_ids().lock().unwrap_or_else(|e| e.into_inner()).get(mal_id) {
        return hit.clone();
    }
    let url = format!("{KITSU}/mappings?filter[externalSite]=myanimelist/anime&filter[externalId]={mal_id}&include=item&fields[anime]=id");
    let id = get_json(&url).await.and_then(|v| {
        v.pointer("/data/0/relationships/item/data/id").and_then(Value::as_str).map(str::to_string)
    });
    kitsu_ids().lock().unwrap_or_else(|e| e.into_inner()).insert(mal_id.to_string(), id.clone());
    id
}

/// `number -> thumbnail` for Kitsu episodes `from..=to`.
async fn kitsu_stills(kitsu: &str, from: u32, to: u32) -> HashMap<u32, String> {
    let mut set = tokio::task::JoinSet::new();
    let mut offset = from.saturating_sub(1);
    while offset < to {
        let url = format!(
            "{KITSU}/anime/{kitsu}/episodes?page[limit]={KITSU_PAGE}&page[offset]={offset}&sort=number&fields[episodes]=number,thumbnail"
        );
        set.spawn(async move { get_json(&url).await });
        offset += KITSU_PAGE;
    }
    let mut out = HashMap::new();
    while let Some(res) = set.join_next().await {
        let Ok(Some(v)) = res else { continue };
        for ep in v.get("data").and_then(Value::as_array).into_iter().flatten() {
            let a = &ep["attributes"];
            let (Some(n), Some(t)) = (a["number"].as_u64(), a.pointer("/thumbnail/original").and_then(Value::as_str)) else { continue };
            out.insert(n as u32, t.to_string());
        }
    }
    out
}

/// Season of a Cinemeta episode id (`tt123:season:episode`).
fn season_of(ep: &AnimeEpisode) -> u32 {
    ep.id.split(':').nth(1).and_then(|s| s.parse().ok()).unwrap_or(1)
}

fn words(t: &str) -> Vec<String> {
    t.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3).map(str::to_lowercase).collect()
}

/// Two titles for the same episode (MAL vs TVDB wording) share most of their words.
fn same_title(a: &str, b: &str) -> bool {
    let (a, b) = (words(a), words(b));
    let n = a.len().min(b.len());
    n > 0 && a.iter().filter(|w| b.contains(w)).count() * 2 >= n
}

/// Whether absolute episode numbers of this MAL entry can index the IMDB series. One MAL
/// entry per season against one IMDB series for all of them would put stills on the wrong
/// episodes, so titles decide when both sides have them, episode counts otherwise.
#[cfg(test)]
fn lines_up(eps: &[AnimeEpisode], imdb: &[(Option<String>, String)], expected: Option<u32>) -> bool {
    titles_agree(eps, imdb).unwrap_or_else(|| counts_agree(eps, imdb.len(), expected))
}

/// `Some(verdict)` when enough episodes have titles on both sides to compare.
fn titles_agree(eps: &[AnimeEpisode], imdb: &[(Option<String>, String)]) -> Option<bool> {
    let pairs: Vec<bool> = eps
        .iter()
        .filter_map(|e| {
            let mine = e.title.as_deref()?;
            let theirs = imdb.get((e.number as usize).checked_sub(1)?)?.0.as_deref()?;
            Some(same_title(mine, theirs))
        })
        .take(8)
        .collect();
    (pairs.len() >= 2).then(|| pairs.iter().filter(|&&m| m).count() * 2 > pairs.len())
}

fn counts_agree(eps: &[AnimeEpisode], imdb_count: usize, expected: Option<u32>) -> bool {
    let n = imdb_count as u32;
    let highest = eps.iter().map(|e| e.number).max().unwrap_or(0);
    match expected.filter(|&e| e > 0) {
        Some(e) => n.abs_diff(e) <= (e / 20).max(2),
        // Still airing: IMDB may list some more than this page reaches.
        None => n >= highest && n - highest <= (highest / 10).max(5),
    }
}

async fn imdb_stills(mal_id: &str) -> Option<Stills> {
    if let Some(hit) = imdb_cache().lock().unwrap_or_else(|e| e.into_inner()).get(mal_id) {
        return hit.clone();
    }
    let fetched = async {
        let imdb = JikanClient::shared().ok()?.imdb_id(mal_id).await?;
        let mut eps = CinemetaClient::new().ok()?.series_episodes(&imdb).await.ok()?;
        eps.retain(|e| season_of(e) > 0);
        eps.sort_by_key(|e| (season_of(e), e.number));
        let list: Vec<(Option<String>, String)> = eps
            .into_iter()
            .map(|e| {
                let still = e.thumbnail.clone().unwrap_or_else(|| format!("https://episodes.metahub.space/{imdb}/{}/{}/w780.jpg", season_of(&e), e.number));
                (e.title, still)
            })
            .collect();
        (!list.is_empty()).then(|| Arc::new(list))
    }
    .await;
    imdb_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(mal_id.to_string(), fetched.clone());
    fetched
}

fn imdb_order() -> &'static Cache<Arc<Vec<(u32, u32)>>> {
    static C: OnceLock<Cache<Arc<Vec<(u32, u32)>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// IMDb `(season, episode)` for episode `n` of an anime numbered from 1 (MAL / Kitsu style).
/// IMDb often splits one long MAL entry into seasons, so `S1E30` would not exist: when season
/// 1 is shorter than `n`, the episode is found by absolute air order instead. `None` when the
/// series has no episode list or `n` is past its end.
pub async fn imdb_episode(imdb: &str, n: u32) -> Option<(u32, u32)> {
    let cached = imdb_order().lock().unwrap_or_else(|e| e.into_inner()).get(imdb).cloned();
    let order = match cached {
        Some(o) => o,
        None => {
            let mut eps = CinemetaClient::new().ok()?.series_episodes(imdb).await.ok()?;
            eps.retain(|e| season_of(e) > 0);
            eps.sort_by_key(|e| (season_of(e), e.number));
            let o = Arc::new(eps.iter().map(|e| (season_of(e), e.number)).collect::<Vec<_>>());
            if !o.is_empty() {
                imdb_order().lock().unwrap_or_else(|e| e.into_inner()).insert(imdb.to_string(), o.clone());
            }
            o
        }
    };
    absolute_to_season(&order, n)
}

fn absolute_to_season(order: &[(u32, u32)], n: u32) -> Option<(u32, u32)> {
    if n == 0 || order.is_empty() {
        return None;
    }
    let first_season = order.iter().filter(|(s, _)| *s == 1).count() as u32;
    if n <= first_season {
        return Some((1, n));
    }
    order.get(n as usize - 1).copied()
}

/// Give every episode without a thumbnail its own still where one can be found.
/// `expected_total` is the MAL episode count (`None` while airing). Never fails.
pub async fn fill_thumbnails(mal_id: &str, episodes: &mut [AnimeEpisode], expected_total: Option<u32>) {
    let missing = |eps: &[AnimeEpisode]| -> Vec<u32> { eps.iter().filter(|e| e.thumbnail.is_none()).map(|e| e.number).collect() };
    let want = missing(episodes);
    let (Some(&from), Some(&to)) = (want.iter().min(), want.iter().max()) else { return };
    if let Some(k) = kitsu_id(mal_id).await {
        let stills = kitsu_stills(&k, from, to).await;
        for e in episodes.iter_mut().filter(|e| e.thumbnail.is_none()) {
            e.thumbnail = stills.get(&e.number).cloned();
        }
    }
    if missing(episodes).is_empty() {
        return;
    }
    let Some(stills) = imdb_stills(mal_id).await else { return };
    // A title comparison is decisive and holds for every page of the show; pages without
    // titles (later pages, AniList fallback) reuse it.
    let verdict = match titles_agree(episodes, &stills) {
        Some(v) => {
            verdicts().lock().unwrap_or_else(|e| e.into_inner()).insert(mal_id.to_string(), v);
            v
        }
        None => match verdicts().lock().unwrap_or_else(|e| e.into_inner()).get(mal_id).copied() {
            Some(v) => v,
            None => counts_agree(episodes, stills.len(), expected_total),
        },
    };
    if !verdict {
        return;
    }
    for e in episodes.iter_mut().filter(|e| e.thumbnail.is_none()) {
        e.thumbnail = (e.number as usize).checked_sub(1).and_then(|i| stills.get(i)).map(|(_, t)| t.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(n: u32, t: Option<&str>) -> AnimeEpisode {
        AnimeEpisode { number: n, title: t.map(str::to_string), ..Default::default() }
    }
    fn imdb(titles: &[&str]) -> Vec<(Option<String>, String)> {
        titles.iter().map(|t| (Some(t.to_string()), String::new())).collect()
    }

    #[test]
    fn titles_decide_when_present() {
        let series = imdb(&["I'm Luffy! The Man Who's Gonna Be King of the Pirates!", "Enter the Great Swordsman! Pirate Hunter Roronoa Zoro!", "Morgan versus Luffy! Who's This Beautiful Young Girl?"]);
        let mal = [ep(1, Some("I'm Luffy! The Man Who's Gonna Be King of the Pirates!")), ep(2, Some("Enter the Great Swordsman! Pirate Hunter, Roronoa Zoro!")), ep(3, Some("Morgan vs. Luffy! Who's the Mysterious Pretty Girl?"))];
        assert!(lines_up(&mal, &series, None));
        // A later season numbered from 1 against the whole series: titles disagree.
        let s2 = [ep(1, Some("Beast Titan")), ep(2, Some("I'm Home")), ep(3, Some("Southwestward"))];
        assert!(!lines_up(&s2, &series, Some(12)));
    }

    #[test]
    fn absolute_episode_maps_into_later_seasons() {
        let order = [(1, 1), (1, 2), (1, 3), (2, 1), (2, 2)];
        assert_eq!(absolute_to_season(&order, 2), Some((1, 2)));
        assert_eq!(absolute_to_season(&order, 4), Some((2, 1)));
        assert_eq!(absolute_to_season(&order, 9), None);
        assert_eq!(absolute_to_season(&[], 1), None);
    }

    #[test]
    fn counts_decide_without_titles() {
        let n = |k: usize| vec![(None, String::new()); k];
        let eps: Vec<_> = (1..=12).map(|i| ep(i, None)).collect();
        assert!(lines_up(&eps, &n(12), Some(12)));
        assert!(!lines_up(&eps, &n(24), Some(12)));
        assert!(!lines_up(&eps, &n(30), None));
        let op: Vec<_> = (1101..=1150).map(|i| ep(i, None)).collect();
        assert!(lines_up(&op, &n(1179), None));
    }
}
