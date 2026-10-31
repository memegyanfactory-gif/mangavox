//! Suggestions: candidates from several finders (shared tags, same author / artist, related
//! titles, the same series elsewhere), ranked against the taste profile ([`crate::taste`]), each
//! with a short "why". Only installed sources are asked, in parallel, and every answer is
//! cached for a while so reopening a page costs nothing.
//!
//! [`rank`] and the title helpers are pure (tested); the async finders are below them.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use mv_sources::{MangaSearchResult, Source, SourceInfo};

use crate::taste::{self, Kind, Profile, TitleRef};
use crate::tags::norm;
use crate::{backend, discover};

/// Suggestions on a manga page / in "For you".
pub const MAX_FOR_TITLE: usize = 12;
pub const MAX_FOR_YOU: usize = 30;
/// Source answers are reused for this long.
const CACHE_TTL: Duration = Duration::from_secs(6 * 3600);
/// One source's answer is given up on after this long.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);
/// "For you" is rebuilt after this long (or when the profile changes).
const FOR_YOU_TTL: Duration = Duration::from_secs(30 * 60);

/// Why a title was found.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// By `name`, who wrote (or drew, `artist`) `of`.
    Creator { name: String, artist: bool, of: String },
    /// Looks like a sequel, prequel or spin-off of `of` (shares its name).
    Related { of: String },
    /// `of` itself, on another source.
    OtherSource { of: String, source_name: String },
    /// Shares these tags with `of` (the open title), or with the user's favourite tags (None).
    Tags { basis: Vec<String>, of: Option<String> },
    /// Shares tags with `of`, a title the user liked.
    Liked { of: String, basis: Vec<String> },
}

/// A title one finder returned.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub source: String,
    pub item: MangaSearchResult,
    pub origin: Origin,
    /// Position in the finder's list (earlier = closer match / more popular).
    pub pos: usize,
}

/// A ranked suggestion.
#[derive(Clone, Debug)]
pub struct Suggestion {
    /// Source the title opens from.
    pub source: String,
    pub item: MangaSearchResult,
    pub why: String,
    pub score: f64,
}

// ---------------------------------------------------------------- titles

const SEQUEL_WORDS: [&str; 14] = ["season", "part", "sequel", "returns", "continuation", "after", "next", "ii", "iii", "iv", "2nd", "3rd", "zero", "origins"];

/// A title without its subtitle and part number ("Solo Leveling: Ragnarok" -> "solo leveling",
/// "Kaguya-sama Season 2" -> "kaguya sama"), for finding the rest of a series.
pub fn base_title(title: &str) -> String {
    let head = title.split([':', '(', '[', '~']).next().unwrap_or(title);
    let head = head.split(" - ").next().unwrap_or(head);
    let mut words: Vec<String> = mv_sources::util::title_key(head).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect();
    while let Some(last) = words.last() {
        let is_num = last.chars().all(|c| c.is_ascii_digit()) || ["ii", "iii", "iv", "2nd", "3rd"].contains(&last.as_str());
        if words.len() > 1 && (is_num || SEQUEL_WORDS.contains(&last.as_str())) {
            words.pop();
        } else {
            break;
        }
    }
    if words.len() > 1 && ["season", "part"].contains(&words[words.len() - 1].as_str()) {
        words.pop();
    }
    words.join(" ")
}

/// How a found title relates to `of` by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kin {
    Same,
    Related,
}

pub fn kin(of: &str, found: &str) -> Option<Kin> {
    let (a, b) = (mv_sources::util::title_key(of), mv_sources::util::title_key(found));
    if a.is_empty() || b.is_empty() {
        return None;
    }
    if a == b {
        return Some(Kin::Same);
    }
    let base = base_title(of);
    if base.len() < 4 {
        return None;
    }
    if base_title(found) == base {
        return Some(Kin::Related);
    }
    // A 1-word name ("Monster") inside a longer title would match half the catalogue.
    (base.contains(' ') && format!(" {b} ").contains(&format!(" {base} "))).then_some(Kin::Related)
}

/// The found title carries a part / season marker, so it likely continues `of`.
pub fn continues(of: &str, found: &str) -> bool {
    let a: HashSet<String> = mv_sources::util::title_key(of).split(' ').map(str::to_string).collect();
    mv_sources::util::title_key(found).split(' ').filter(|w| !a.contains(*w)).any(|w| SEQUEL_WORDS.contains(&w) || w.chars().all(|c| c.is_ascii_digit()))
}

// ---------------------------------------------------------------- ranking

fn base_score(o: &Origin) -> f64 {
    match o {
        Origin::Related { .. } => 3.5,
        Origin::Creator { artist: false, .. } => 3.0,
        Origin::Creator { artist: true, .. } => 2.6,
        Origin::OtherSource { .. } => 2.0,
        Origin::Liked { basis, .. } => 1.6 + 0.2 * basis.len() as f64,
        Origin::Tags { basis, .. } => 1.0 + 0.25 * basis.len() as f64,
    }
}

fn tag_basis(o: &Origin) -> &[String] {
    match o {
        Origin::Tags { basis, .. } | Origin::Liked { basis, .. } => basis,
        _ => &[],
    }
}

/// A tag's affinity as a genre or a demographic, whichever the profile knows better.
fn tag_affinity(p: &Profile, tag: &str) -> f64 {
    let (g, d) = (p.affinity(Kind::Genre, tag), p.affinity(Kind::Demographic, tag));
    if g.abs() >= d.abs() {
        g
    } else {
        d
    }
}

fn lower_join(names: &[&str]) -> String {
    names.iter().map(|n| n.to_lowercase()).collect::<Vec<_>>().join(" ")
}

/// The short line under a suggestion.
pub fn why(o: &Origin, item: &MangaSearchResult, p: &Profile) -> String {
    match o {
        Origin::Creator { artist, of, .. } if !of.is_empty() => format!("Same {} as {of}", if *artist { "artist" } else { "author" }),
        Origin::Creator { name, .. } => format!("By {name}, a creator you like"),
        Origin::Related { of } if continues(of, &item.title) => format!("Continues {of}"),
        Origin::Related { of } => format!("More from the world of {of}"),
        Origin::OtherSource { source_name, .. } => format!("Same series on {source_name}"),
        Origin::Liked { of, .. } => format!("Because you liked {of}"),
        Origin::Tags { basis, of } => {
            let finished: Vec<&str> = basis.iter().filter(|b| p.stat(Kind::Genre, b).or_else(|| p.stat(Kind::Demographic, b)).is_some_and(|s| s.finishes_most())).map(String::as_str).take(2).collect();
            let liked: Vec<&str> = basis.iter().filter(|b| p.affinity(Kind::Genre, b) > 0.3).map(String::as_str).take(2).collect();
            if !finished.is_empty() {
                format!("You finish most {}", lower_join(&finished))
            } else if !liked.is_empty() {
                format!("You often read {}", liked.join(" + "))
            } else if let Some(of) = of {
                format!("Like {of}: {}", basis.iter().take(2).cloned().collect::<Vec<_>>().join(" + "))
            } else {
                format!("Popular in {}", basis.iter().take(2).cloned().collect::<Vec<_>>().join(" + "))
            }
        }
    }
}

/// Score and explain candidates, one entry per title (several finders agreeing raise it), and
/// keep the best `max`. Titles the user knows or turned down, and `exclude` (the open title),
/// are left out, except the open title on another source.
pub fn rank(cands: Vec<Candidate>, p: &Profile, exclude: &str, max: usize) -> Vec<Suggestion> {
    let own = norm(exclude);
    // title key -> (scored origins, first candidate)
    let mut by_title: HashMap<String, Vec<(f64, Candidate)>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for c in cands {
        let key = norm(&c.item.title);
        if key.is_empty() || c.item.source_id.is_empty() || p.rejected.contains(&key) {
            continue;
        }
        let other_source = matches!(c.origin, Origin::OtherSource { .. });
        if !other_source && (key == own || p.seen.contains(&key)) {
            continue;
        }
        let mut s = base_score(&c.origin) * (1.0 - c.pos.min(20) as f64 * 0.02);
        let basis = tag_basis(&c.origin);
        if !basis.is_empty() {
            s += basis.iter().map(|b| tag_affinity(p, b)).sum::<f64>() * 0.8;
        }
        if let Some(a) = c.item.author.as_deref() {
            s += p.creator_affinity(a) * 1.5;
        }
        if !by_title.contains_key(&key) {
            order.push(key.clone());
        }
        by_title.entry(key).or_default().push((s, c));
    }
    let mut out: Vec<Suggestion> = order
        .into_iter()
        .filter_map(|key| {
            let mut hits = by_title.remove(&key)?;
            hits.sort_by(|a, b| b.0.total_cmp(&a.0));
            // Different finders agreeing count; the same finder twice (two sources) barely.
            let mut kinds: Vec<std::mem::Discriminant<Origin>> = Vec::new();
            let mut score = 0.0;
            for (i, (s, c)) in hits.iter().enumerate() {
                let d = std::mem::discriminant(&c.origin);
                score += if i == 0 { *s } else if kinds.contains(&d) { 0.1 } else { 0.5 * s.max(0.0) };
                kinds.push(d);
            }
            let (_, best) = hits.swap_remove(0);
            (score > 0.0).then(|| Suggestion { why: why(&best.origin, &best.item, p), source: best.source, item: best.item, score })
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(max);
    out
}

// ---------------------------------------------------------------- the profile

/// Build the taste profile from the database and the file, and keep its snapshot in the file.
/// Blocking.
pub fn load_profile() -> Result<Profile> {
    let file = taste::file();
    let (lang, hidden) = (backend::preferred_language(), backend::browse_only());
    let facts = backend::with_db(|c| crate::store::taste_facts(c, &lang, &hidden))?;
    let now = taste::now_secs();
    let p = taste::build(&facts, &file, now);
    // Refresh the kept snapshot at most hourly (it is for a quick look, not for ranking).
    if now - file.built_at > 3600 {
        taste::save_snapshot(p.snapshot(), now);
    }
    Ok(p)
}

// ---------------------------------------------------------------- finders (network)

type Cache = HashMap<String, (Instant, Vec<MangaSearchResult>)>;
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
static TAG_CACHE: Mutex<Option<HashMap<String, (Instant, discover::Recs)>>> = Mutex::new(None);

/// A source answer, from the cache when it is fresh. Failures are not cached.
async fn cached<F>(key: String, fetch: F) -> Vec<MangaSearchResult>
where
    F: std::future::Future<Output = Result<Vec<MangaSearchResult>>>,
{
    let hit = {
        let g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().and_then(|c| c.get(&key)).filter(|(at, _)| at.elapsed() < CACHE_TTL).map(|(_, v)| v.clone())
    };
    if let Some(v) = hit {
        return v;
    }
    // A slow source must not hold up the others' suggestions.
    let res = tokio::time::timeout(LOOKUP_TIMEOUT, fetch).await.unwrap_or_else(|_| Err(anyhow!("timed out")));
    match res {
        Ok(v) => {
            let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            let c = g.get_or_insert_with(Default::default);
            c.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
            c.insert(key, (Instant::now(), v.clone()));
            v
        }
        Err(e) => {
            eprintln!("[ui-manga] suggestion lookup failed: {e:#}");
            Vec::new()
        }
    }
}

/// Installed sources to ask (18+ ones only when `adult`), with the source objects.
async fn sources(adult: bool) -> Vec<(SourceInfo, Arc<dyn Source>)> {
    let reg = backend::registry().await;
    backend::installed_sources(reg.list()).into_iter().filter(|s| adult || !s.nsfw).filter_map(|s| reg.get(&s.id).map(|src| (s, src))).collect()
}

/// Titles by `name` on every installed source.
async fn by_creator(name: &str, artist: bool, of: &str, adult: bool) -> Vec<Candidate> {
    let srcs = sources(adult).await;
    let asks = srcs.into_iter().map(|(info, src)| {
        let name = name.to_string();
        async move {
            let key = format!("creator\u{1}{}\u{1}{}", info.id, norm(&name));
            let list = cached(key, async { src.get_manga_by_creator(&name, 1).await.map_err(|e| anyhow!("{e}")) }).await;
            (info.id, list)
        }
    });
    let mut out = Vec::new();
    for (sid, list) in futures::future::join_all(asks).await {
        out.extend(list.into_iter().enumerate().map(|(pos, item)| Candidate {
            source: sid.clone(),
            item,
            origin: Origin::Creator { name: name.to_string(), artist, of: of.to_string() },
            pos,
        }));
    }
    out
}

/// Titles sharing `title`'s name on every installed source: sequels / spin-offs, and the title
/// itself elsewhere (only when `same_too`).
async fn related(title: &str, own_source: &str, adult: bool, same_too: bool) -> Vec<Candidate> {
    let base = base_title(title);
    if base.len() < 4 {
        return Vec::new();
    }
    let srcs = sources(adult).await;
    let asks = srcs.into_iter().map(|(info, src)| {
        let base = base.clone();
        async move {
            let key = format!("search\u{1}{}\u{1}{base}", info.id);
            let list = cached(key, async { src.search_manga(&base, 1).await.map_err(|e| anyhow!("{e}")) }).await;
            (info, list)
        }
    });
    let mut out = Vec::new();
    for (info, list) in futures::future::join_all(asks).await {
        for (pos, item) in list.into_iter().enumerate() {
            let origin = match kin(title, &item.title) {
                Some(Kin::Related) => Origin::Related { of: title.to_string() },
                Some(Kin::Same) if same_too && info.id != own_source => Origin::OtherSource { of: title.to_string(), source_name: info.name.clone() },
                _ => continue,
            };
            out.push(Candidate { source: info.id.clone(), item, origin, pos });
        }
    }
    out
}

/// Titles sharing `genres` (the title's own source when it has those tags, else MangaDex).
async fn by_tags(source: String, id: String, title: String, genres: Vec<String>, liked: bool) -> Vec<Candidate> {
    let key = format!("{source}\u{1}{id}\u{1}{}", genres.iter().map(|g| norm(g)).collect::<Vec<_>>().join(","));
    let hit = {
        let g = TAG_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().and_then(|c| c.get(&key)).filter(|(at, _)| at.elapsed() < CACHE_TTL).map(|(_, r)| r.clone())
    };
    let r = match hit {
        Some(r) => r,
        None => match discover::recommendations(source.clone(), id.clone(), title.clone(), genres.clone()).await {
            Ok(r) => {
                let mut g = TAG_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                let c = g.get_or_insert_with(Default::default);
                c.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
                c.insert(key, (Instant::now(), r.clone()));
                r
            }
            Err(e) => {
                eprintln!("[ui-manga] tag suggestions failed: {e:#}");
                return Vec::new();
            }
        },
    };
    let of = (!title.is_empty()).then(|| title.clone());
    r.items
        .into_iter()
        .enumerate()
        .map(|(pos, item)| Candidate {
            source: r.source_id.clone(),
            item,
            origin: match (&of, liked) {
                (Some(of), true) => Origin::Liked { of: of.clone(), basis: r.basis.clone() },
                _ => Origin::Tags { basis: r.basis.clone(), of: of.clone() },
            },
            pos,
        })
        .collect()
}

/// What a manga page knows about the open title.
#[derive(Clone, Debug, Default)]
pub struct TitleCtx {
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub author: Option<String>,
    pub artist: Option<String>,
    pub genres: Vec<String>,
    /// Sources the title's chapters already come from (no "Same series on" for those).
    pub linked: Vec<String>,
}

/// "You may also like" for one title: shared tags, the same author / artist, related titles and
/// the same series on sources it is not linked to yet, ranked by the user's taste.
pub async fn for_title(t: TitleCtx) -> Result<Vec<Suggestion>> {
    let adult = backend::source(&t.source).await.map(|s| s.info().nsfw).unwrap_or(false)
        || t.genres.iter().any(|g| ["hentai", "erotica", "pornographic", "adult", "smut"].contains(&norm(g).as_str()));
    let people: Vec<(String, bool)> = {
        let authors = t.author.as_deref().map(mv_sources::util::split_people).unwrap_or_default();
        let artists = t.artist.as_deref().map(mv_sources::util::split_people).unwrap_or_default();
        let mut v: Vec<(String, bool)> = authors.iter().map(|a| (a.clone(), false)).collect();
        v.extend(artists.into_iter().filter(|a| !authors.iter().any(|x| norm(x) == norm(a))).map(|a| (a, true)));
        v.truncate(3);
        v
    };
    let profile = mv_ui::bg::blocking(load_profile);
    let tags = by_tags(t.source.clone(), t.source_id.clone(), t.title.clone(), t.genres.clone(), false);
    let creators = futures::future::join_all(people.iter().map(|(n, artist)| by_creator(n, *artist, &t.title, adult)));
    let rel = related(&t.title, &t.source, adult, true);
    let (profile, tags, creators, rel) = futures::join!(profile, tags, creators, rel);
    let profile = profile.and_then(|r| r).unwrap_or_default();
    let mut cands: Vec<Candidate> = rel.into_iter().filter(|c| !matches!(c.origin, Origin::OtherSource { .. }) || !t.linked.contains(&c.source)).collect();
    // The title itself on its own source (another id with the same name) is no suggestion.
    cands.retain(|c| !(c.source == t.source && matches!(c.origin, Origin::OtherSource { .. })));
    cands.extend(creators.into_iter().flatten());
    cands.extend(tags);
    // Adult-rated titles only show up as suggestions for adult titles.
    cands.retain(|c| adult || !c.item.adult);
    let mut out = rank(cands, &profile, &t.title, MAX_FOR_TITLE);
    out.retain(|s| !(s.source == t.source && s.item.source_id == t.source_id));
    Ok(out)
}

/// "For you" and the profile it came from.
#[derive(Clone, Debug, Default)]
pub struct ForYou {
    pub items: Vec<Suggestion>,
    pub profile: Profile,
}

static FOR_YOU: Mutex<Option<(Instant, u64, ForYou)>> = Mutex::new(None);

/// The last "For you" when it is still fresh (same profile, under 30 minutes old).
pub fn for_you_cached() -> Option<ForYou> {
    let g = FOR_YOU.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().filter(|(at, v, _)| at.elapsed() < FOR_YOU_TTL && *v == taste::version()).map(|(_, _, f)| f.clone())
}

/// Suggestions from the whole profile: titles like the ones the user liked most, by their
/// favourite authors and artists, sequels of what they finish, and popular titles in their
/// favourite genres.
pub async fn for_you() -> Result<ForYou> {
    let version = taste::version();
    let profile = mv_ui::bg::blocking(load_profile).await??;
    let seeds: Vec<TitleRef> = profile.seeds.iter().map(|(t, _)| t.clone()).collect();
    let installed: HashSet<String> = sources(true).await.into_iter().map(|(i, _)| i.id).collect();
    // Titles like the 3 strongest seeds (from their own source, which must still be installed).
    let liked = futures::future::join_all(
        seeds.iter().filter(|s| installed.contains(&s.source) && !s.genres.is_empty()).take(3).map(|s| by_tags(s.source.clone(), s.source_id.clone(), s.title.clone(), s.genres.clone(), true)),
    );
    // Popular in the favourite genres (any installed source with tags).
    let top_genres: Vec<String> = profile.top(&[Kind::Genre, Kind::Demographic], true, 3).iter().map(|s| s.name.clone()).collect();
    let tag_source = installed.iter().find(|s| *s == discover::FALLBACK_SOURCE).or_else(|| seeds.iter().map(|s| &s.source).find(|s| installed.contains(*s))).cloned();
    let genres = async {
        match tag_source {
            Some(src) if !top_genres.is_empty() => by_tags(src, String::new(), String::new(), top_genres.clone(), false).await,
            _ => Vec::new(),
        }
    };
    // Favourite creators (2 strongest).
    let creators: Vec<(String, bool, String)> =
        profile.top(&[Kind::Author, Kind::Artist], true, 2).iter().map(|s| (s.name.clone(), s.kind == Kind::Artist, s.examples.first().cloned().unwrap_or_default())).collect();
    let by_people = futures::future::join_all(creators.iter().map(|(n, artist, of)| by_creator(n, *artist, of, false)));
    // The rest of the 2 strongest series.
    let rel = futures::future::join_all(seeds.iter().take(2).map(|s| related(&s.title, &s.source, false, false)));
    let (liked, genres, by_people, rel) = futures::join!(liked, genres, by_people, rel);
    let mut cands: Vec<Candidate> = rel.into_iter().flatten().collect();
    cands.extend(by_people.into_iter().flatten());
    cands.extend(liked.into_iter().flatten());
    cands.extend(genres);
    cands.retain(|c| !c.item.adult);
    let items = rank(cands, &profile, "", MAX_FOR_YOU);
    let fy = ForYou { items, profile };
    *FOR_YOU.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), version, fy.clone()));
    Ok(fy)
}

/// Drop one title from the kept "For you" (after "Not interested") without rebuilding it.
pub fn forget_cached(source: &str, source_id: &str) {
    if let Some((_, v, fy)) = FOR_YOU.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        fy.items.retain(|s| !(s.source == source && s.item.source_id == source_id));
        *v = taste::version();
    }
}

/// Feedback given on the kept "For you" itself does not make it stale.
pub fn keep_cached() {
    if let Some((_, v, _)) = FOR_YOU.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        *v = taste::version();
    }
}

/// Replace the kept "For you" (an undone "Not interested"), keeping its age.
pub fn put_cached(fy: ForYou) {
    if let Some((_, v, kept)) = FOR_YOU.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        *kept = fy;
        *v = taste::version();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::{build, FeedbackKind, TasteFile, TitleFacts};

    fn item(id: &str, title: &str, author: Option<&str>) -> MangaSearchResult {
        MangaSearchResult { source_id: id.into(), title: title.into(), author: author.map(Into::into), ..Default::default() }
    }

    fn cand(src: &str, id: &str, title: &str, origin: Origin, pos: usize) -> Candidate {
        Candidate { source: src.into(), item: item(id, title, None), origin, pos }
    }

    fn tags(basis: &[&str]) -> Origin {
        Origin::Tags { basis: basis.iter().map(|s| s.to_string()).collect(), of: Some("Berserk".into()) }
    }

    fn profile() -> Profile {
        let now = 1_800_000_000;
        let f = |title: &str, author: &str, genres: &[&str], read: usize, chapters: usize, days: i64| TitleFacts {
            title: TitleRef { title: title.into(), source: "s".into(), source_id: title.into(), author: Some(author.into()), genres: genres.iter().map(|s| s.to_string()).collect(), ..Default::default() },
            chapters,
            read,
            last_read: Some(now - days * 86_400),
            added: now - 500 * 86_400,
            in_library: true,
            ..Default::default()
        };
        let mut file = TasteFile::default();
        file.set_verdict(
            TitleRef { title: "Overlord".into(), author: Some("Maruyama".into()), genres: vec!["Isekai".into()], ..Default::default() },
            Some(FeedbackKind::NotInterested),
            now,
        );
        build(
            &[
                f("Berserk", "Miura", &["Horror", "Action"], 380, 380, 5),
                f("Claymore", "Yagi", &["Horror", "Action"], 150, 155, 20),
                f("Nana", "Yazawa", &["Romance", "Drama"], 3, 84, 200),
            ],
            &file,
            now,
        )
    }

    #[test]
    fn base_title_and_kin() {
        assert_eq!(base_title("Solo Leveling: Ragnarok"), "solo leveling");
        assert_eq!(base_title("Kaguya-sama: Love is War Season 2"), "kaguya sama");
        assert_eq!(base_title("Tower of God Part 3"), "tower of god");
        assert_eq!(base_title("Attack on Titan - Before the Fall"), "attack on titan");
        assert_eq!(base_title("Monster"), "monster");
        assert_eq!(kin("Solo Leveling", "SOLO LEVELING"), Some(Kin::Same));
        assert_eq!(kin("Solo Leveling", "Solo Leveling: Ragnarok"), Some(Kin::Related));
        assert_eq!(kin("Solo Leveling: Ragnarok", "Solo Leveling"), Some(Kin::Related));
        assert_eq!(kin("Tower of God", "The Tower of God Side Story"), Some(Kin::Related));
        assert_eq!(kin("Solo Leveling", "Leveling Up Alone"), None);
        assert_eq!(kin("Monster", "Monster Musume"), None, "a one-word name needs the whole title");
        assert_eq!(kin("Berserk", "Berserk of Gluttony"), None);
        assert!(continues("Solo Leveling", "Solo Leveling Season 2"));
        assert!(!continues("Attack on Titan", "Attack on Titan: Junior High"));
    }

    #[test]
    fn rank_prefers_taste_and_drops_known_and_rejected() {
        let p = profile();
        let cands = vec![
            cand("md", "1", "Gantz", tags(&["Action"]), 0),
            cand("md", "2", "Hellsing", tags(&["Horror", "Action"]), 1),
            cand("md", "3", "Claymore", tags(&["Horror"]), 2),       // already read
            cand("md", "4", "Overlord", tags(&["Action"]), 3),       // not interested
            cand("md", "5", "Berserk", tags(&["Horror"]), 4),        // the open title
            cand("md", "6", "Kimi ni Todoke", tags(&["Romance"]), 5), // avoided genre
            Candidate { source: "mf".into(), item: item("7", "Berserk: The Prototype", Some("Miura")), origin: Origin::Related { of: "Berserk".into() }, pos: 0 },
            Candidate { source: "md".into(), item: item("8", "Duranki", Some("Miura")), origin: Origin::Creator { name: "Miura".into(), artist: false, of: "Berserk".into() }, pos: 0 },
            Candidate { source: "mf".into(), item: item("9", "Berserk", None), origin: Origin::OtherSource { of: "Berserk".into(), source_name: "MangaFire".into() }, pos: 0 },
        ];
        let r = rank(cands, &p, "Berserk", 10);
        let titles: Vec<&str> = r.iter().map(|s| s.item.title.as_str()).collect();
        assert_eq!(titles[..2], ["Berserk: The Prototype", "Duranki"], "related and same-author titles by a loved author lead");
        assert!(titles.contains(&"Berserk"), "the open title on another source stays");
        assert!(!titles.contains(&"Claymore") && !titles.contains(&"Overlord"));
        let pos = |t: &str| titles.iter().position(|x| *x == t);
        assert!(pos("Hellsing") < pos("Gantz"), "a liked tag pair beats one broad tag");
        assert!(pos("Kimi ni Todoke").is_none_or(|k| k > pos("Gantz").unwrap()), "avoided genres sink");
        let why = |t: &str| r.iter().find(|s| s.item.title == t).unwrap().why.clone();
        assert_eq!(why("Duranki"), "Same author as Berserk");
        assert_eq!(why("Berserk: The Prototype"), "More from the world of Berserk");
        assert_eq!(why("Berserk"), "Same series on MangaFire");
        assert_eq!(why("Hellsing"), "You finish most horror action");
    }

    #[test]
    fn rank_merges_finders_and_explains() {
        let p = profile();
        let liked = Origin::Liked { of: "Claymore".into(), basis: vec!["Horror".into()] };
        let cands = vec![
            cand("md", "a", "Hellsing", tags(&["Action"]), 0),
            cand("md", "b", "Gantz", tags(&["Action"]), 1),
            cand("mf", "b2", "GANTZ", liked.clone(), 0),
            cand("md", "c", "Solo Leveling Season 2", Origin::Related { of: "Solo Leveling".into() }, 0),
        ];
        let r = rank(cands, &p, "", 3);
        assert_eq!(r.len(), 3);
        assert_eq!(r.iter().find(|s| s.item.source_id == "c").unwrap().why, "Continues Solo Leveling");
        let g = r.iter().find(|s| norm(&s.item.title) == "gantz").unwrap();
        assert_eq!(g.why, "Because you liked Claymore", "the strongest finder explains it");
        assert!(g.score > r.iter().find(|s| s.item.title == "Hellsing").unwrap().score, "two finders agreeing rank higher");
        // Empty profile: popular-in-tags wording, nothing dropped.
        let r = rank(vec![cand("md", "x", "X", Origin::Tags { basis: vec!["Mystery".into()], of: None }, 0)], &Profile::default(), "", 5);
        assert_eq!(r[0].why, "Popular in Mystery");
        assert!(rank(vec![cand("md", "", "No id", tags(&["Action"]), 0)], &Profile::default(), "", 5).is_empty());
    }
}
