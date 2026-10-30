//! Async glue for tag browsing and recommendations: the cached tag lists, the "open Browse on this
//! tag" hand-off from the detail page, tag resolution and the "You may also like" queries.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use mv_sources::{MangaSearchResult, Tag};
use serde::{Deserialize, Serialize};

use crate::backend;
use crate::tags::{self, MAX_RECS, MIN_RECS};

/// Source used for tag search / recommendations when a title's own source has no tags.
pub const FALLBACK_SOURCE: &str = mv_sources::mangadex::ID;
/// Tag lists rarely change: keep them on disk for a week.
const TAG_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

// ---------------------------------------------------------------- tag list cache

/// A source's tags and whether it can combine several of them (AND).
#[derive(Clone, Default)]
pub struct SourceTags {
    pub tags: Arc<Vec<Tag>>,
    pub multi: bool,
}

static TAGS: Mutex<Option<HashMap<String, SourceTags>>> = Mutex::new(None);

#[derive(Serialize, Deserialize)]
struct DiskTags {
    saved: u64,
    tags: Vec<Tag>,
}

fn tags_file(source_id: &str) -> PathBuf {
    let safe: String = source_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    mv_core::paths::root().join("cache").join("tags").join(format!("{safe}.json"))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn read_disk(source_id: &str) -> Option<DiskTags> {
    serde_json::from_slice(&std::fs::read(tags_file(source_id)).ok()?).ok()
}

fn write_disk(source_id: &str, tags: &[Tag]) {
    let path = tags_file(source_id);
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(bytes) = serde_json::to_vec(&DiskTags { saved: now_secs(), tags: tags.to_vec() }) {
        let _ = std::fs::write(path, bytes);
    }
}

/// The source's tag list: memory, then a fresh disk copy, then the network (falling back to a
/// stale disk copy when the source is unreachable). An empty list means "no tag browsing".
pub async fn tag_list(source_id: &str) -> Result<SourceTags> {
    if let Some(t) = TAGS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(source_id).cloned()) {
        return Ok(t);
    }
    let src = backend::source(source_id).await?;
    let multi = src.supports_multi_tag();
    let disk = read_disk(source_id);
    let fresh = disk.as_ref().filter(|d| now_secs().saturating_sub(d.saved) < TAG_TTL.as_secs() && !d.tags.is_empty());
    let tags = match fresh {
        Some(d) => d.tags.clone(),
        None => match src.get_tag_list().await {
            Ok(list) => {
                if !list.is_empty() {
                    write_disk(source_id, &list);
                }
                list
            }
            Err(e) => match disk {
                Some(d) if !d.tags.is_empty() => d.tags,
                _ => return Err(anyhow!("{e}")),
            },
        },
    };
    let st = SourceTags { tags: Arc::new(tags), multi };
    TAGS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(source_id.to_string(), st.clone());
    Ok(st)
}

// ---------------------------------------------------------------- detail -> browse hand-off

/// A genre chip clicked on the detail page, waiting for Browse to pick it up.
#[derive(Clone, Debug, PartialEq)]
pub struct TagRequest {
    /// The title's own source.
    pub source_id: String,
    pub name: String,
}

static PENDING: Mutex<Option<TagRequest>> = Mutex::new(None);

pub fn request_tag(source_id: &str, name: &str) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(TagRequest { source_id: source_id.into(), name: name.into() });
}

pub fn take_request() -> Option<TagRequest> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Where a clicked genre leads.
#[derive(Clone, Debug)]
pub enum TagTarget {
    /// Browse `source_id` filtered by `tag`.
    Tag { source_id: String, tag: Tag },
    /// The genre is not a tag anywhere: plain text search on the title's source.
    Search { source_id: String, query: String },
}

async fn find_on(source_id: &str, name: &str) -> Option<Tag> {
    if backend::disabled_sources().contains(source_id) {
        return None;
    }
    let st = tag_list(source_id).await.ok()?;
    tags::find_tag(&st.tags, name).cloned()
}

/// The title's own source when it has this tag, else MangaDex's tag, else a text search.
pub async fn resolve_tag(req: TagRequest) -> TagTarget {
    if let Some(tag) = find_on(&req.source_id, &req.name).await {
        return TagTarget::Tag { source_id: req.source_id, tag };
    }
    if req.source_id != FALLBACK_SOURCE {
        if let Some(tag) = find_on(FALLBACK_SOURCE, &req.name).await {
            return TagTarget::Tag { source_id: FALLBACK_SOURCE.into(), tag };
        }
    }
    TagTarget::Search { source_id: req.source_id, query: req.name }
}

// ---------------------------------------------------------------- recommendations

/// "You may also like" for one title.
#[derive(Clone, Debug, Default)]
pub struct Recs {
    /// Source the recommended titles come from (open them with this source).
    pub source_id: String,
    pub source_name: String,
    /// Names of the tags the query used.
    pub basis: Vec<String>,
    pub items: Vec<MangaSearchResult>,
}

/// Similar titles by the title's most telling 2-3 tags (own source when it knows them, else
/// MangaDex), most followed first, without the title itself.
pub async fn recommendations(own_source: String, own_id: String, title: String, genres: Vec<String>) -> Result<Recs> {
    let disabled = backend::disabled_sources();
    let mut candidates = vec![own_source.clone()];
    if own_source != FALLBACK_SOURCE && !disabled.contains(FALLBACK_SOURCE) {
        candidates.push(FALLBACK_SOURCE.into());
    }
    let mut chosen: Option<(String, SourceTags, Vec<Tag>)> = None;
    for sid in candidates {
        let Ok(st) = tag_list(&sid).await else { continue };
        let picked = tags::pick_rec_tags(&genres, &st.tags, 3);
        if !picked.is_empty() {
            chosen = Some((sid, st, picked));
            break;
        }
    }
    let Some((sid, st, picked)) = chosen else { return Ok(Recs::default()) };
    // Adult-rated titles only show up as recommendations for adult titles.
    let adult_ok = backend::source(&own_source).await.map(|s| s.info().nsfw).unwrap_or(false)
        || genres.iter().any(|g| ["hentai", "erotica", "pornographic", "adult", "smut"].contains(&g.to_lowercase().as_str()));
    let src = backend::source(&sid).await?;
    let plan = tags::query_plan(&picked, st.multi);
    let exclude = (sid == own_source).then_some(own_id);
    let mut lists: Vec<Vec<MangaSearchResult>> = Vec::new();
    let mut first_err: Option<String> = None;
    if st.multi {
        // Narrowest first; widen only while there are too few.
        for ids in &plan {
            match src.get_manga_by_tags(ids, 1).await {
                Ok(l) => lists.push(l),
                Err(e) => {
                    first_err.get_or_insert(e.to_string());
                }
            }
            if tags::merge_recs(&lists, exclude.as_deref(), &title, MAX_RECS).len() >= MIN_RECS {
                break;
            }
        }
    } else {
        let futs = plan.iter().map(|ids| src.get_manga_by_tags(ids, 1));
        for res in futures::future::join_all(futs).await {
            match res {
                Ok(l) => lists.push(l),
                Err(e) => {
                    first_err.get_or_insert(e.to_string());
                }
            }
        }
    }
    if lists.is_empty() {
        if let Some(e) = first_err {
            return Err(anyhow!(e));
        }
    }
    Ok(Recs {
        source_name: src.info().name,
        source_id: sid,
        basis: picked.into_iter().map(|t| t.name).collect(),
        items: {
            if !adult_ok {
                lists.iter_mut().for_each(|l| l.retain(|r| !r.adult));
            }
            tags::merge_recs(&lists, exclude.as_deref(), &title, MAX_RECS)
        },
    })
}

#[cfg(test)]
mod live {
    use super::*;

    /// `cargo test -p ui-manga live_ -- --ignored --nocapture` (network: MangaDex).
    #[test]
    #[ignore]
    fn live_tag_search_and_recommendations() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let st = tag_list(FALLBACK_SOURCE).await.expect("MangaDex tags");
            assert!(st.multi && st.tags.len() > 60, "{} tags", st.tags.len());
            let groups: Vec<String> = tags::group_tags(&st.tags).into_iter().map(|(g, v)| format!("{g} ({})", v.len())).collect();
            println!("groups: {}", groups.join(", "));

            let action = tags::find_tag(&st.tags, "Action").unwrap().clone();
            let romance = tags::find_tag(&st.tags, "Romance").unwrap().clone();
            let src = backend::source(FALLBACK_SOURCE).await.unwrap();
            let both = src.get_manga_by_tags(&[action.id.clone(), romance.id.clone()], 1).await.unwrap();
            println!("Action + Romance: {}", both.iter().take(8).map(|r| r.title.as_str()).collect::<Vec<_>>().join(" | "));
            assert!(!both.is_empty());
            let seinen = src.get_manga_by_tags(&["demographic:seinen".to_string(), action.id.clone()], 1).await.unwrap();
            println!("Seinen + Action: {}", seinen.iter().take(5).map(|r| r.title.as_str()).collect::<Vec<_>>().join(" | "));
            assert!(!seinen.is_empty());

            match resolve_tag(TagRequest { source_id: "webtoon".into(), name: "sci fi".into() }).await {
                TagTarget::Tag { source_id, tag } => println!("webtoon 'sci fi' -> {source_id} tag {} ({})", tag.name, tag.id),
                other => panic!("expected a MangaDex tag, got {other:?}"),
            }

            // Berserk (MangaDex) with its real genres.
            let genres: Vec<String> = ["Award Winning", "Action", "Adventure", "Drama", "Fantasy", "Horror", "Gore", "Monsters", "Seinen"].iter().map(|s| s.to_string()).collect();
            let r = recommendations(FALLBACK_SOURCE.into(), "801513ba-a712-498c-8f57-cae55b38cc92".into(), "Berserk".into(), genres).await.unwrap();
            println!("Berserk recs (by {}): {}", r.basis.join(" + "), r.items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>().join(" | "));
            assert!(r.items.len() >= MIN_RECS && r.items.len() <= MAX_RECS);
            assert!(r.items.iter().all(|i| i.source_id != "801513ba-a712-498c-8f57-cae55b38cc92" && i.title != "Berserk"));

            // A WEBTOON title (no tags there) falls back to MangaDex.
            let r = recommendations("webtoon".into(), "95".into(), "Tower of God".into(), vec!["Fantasy".into()]).await.unwrap();
            println!("Tower of God recs (from {}, by {}): {}", r.source_name, r.basis.join(" + "), r.items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>().join(" | "));
            assert_eq!(r.source_id, FALLBACK_SOURCE);
            assert!(!r.items.is_empty());
        });
    }
}

static SEARCH: Mutex<Option<String>> = Mutex::new(None);

/// Browse searches every installed source for `query` when it is shown next (the command
/// palette's "Search all sources").
pub fn request_search(query: &str) {
    *SEARCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(query.trim().to_string());
}

pub fn take_search() -> Option<String> {
    SEARCH.lock().unwrap_or_else(|e| e.into_inner()).take().filter(|q| !q.is_empty())
}
