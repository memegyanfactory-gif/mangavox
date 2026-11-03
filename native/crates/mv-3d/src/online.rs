//! Free online assets from Poly Haven (CC0, public API): everyday models for generic props and
//! sky HDRIs for exteriors. Only model, texture and image files are downloaded (glTF + its
//! textures, .hdr); nothing downloaded is ever run.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::util::{read_json, tokens, write_json};

const API: &str = "https://api.polyhaven.com";
pub(crate) const LICENSE: &str = "CC0 (Poly Haven)";
const ALLOWED: [&str; 7] = ["gltf", "bin", "jpg", "jpeg", "png", "hdr", "exr"];
const MAX_FILE: u64 = 120 * 1024 * 1024;

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(concat!("MangaVox/", env!("CARGO_PKG_VERSION"), " (3D tab)")).timeout(Duration::from_secs(120)).build()?)
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Found {
    pub id: String,
    pub name: String,
}

/// The asset list of one type (`models` / `hdris`), cached for a day under `cache_dir`.
async fn list(kind: &str, cache_dir: &Path) -> Result<Value> {
    let cache = cache_dir.join(format!("polyhaven_{kind}.json"));
    let fresh = std::fs::metadata(&cache).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(86_400));
    if fresh {
        if let Some(v) = read_json::<Value>(&cache) {
            return Ok(v);
        }
    }
    let v: Value = get_json(&format!("{API}/assets?type={kind}")).await?;
    let _ = write_json(&cache, &v);
    Ok(v)
}

/// The best model for `name` + `tags`: its main word (the last word of the name) must be in the
/// model's name or tags, then the most shared words wins.
pub(crate) fn best_model(list: &Value, name: &str, tags: &[String]) -> Option<Found> {
    let want_name = tokens(name);
    let head = want_name.last()?.clone();
    let mut want = want_name.clone();
    for t in tags {
        want.extend(tokens(t));
    }
    let mut best: Option<(usize, Found)> = None;
    for (id, a) in list.as_object()? {
        let aname = a.get("name").and_then(|n| n.as_str()).unwrap_or(id);
        let mut have = tokens(aname);
        for k in ["tags", "categories"] {
            for t in a.get(k).and_then(|t| t.as_array()).into_iter().flatten().filter_map(|t| t.as_str()) {
                have.extend(tokens(t));
            }
        }
        let name_words = tokens(aname);
        if !name_words.contains(&head) && !have.contains(&head) {
            continue;
        }
        let score = want.iter().filter(|w| have.contains(w)).count() + 2 * name_words.contains(&head) as usize;
        let better = match &best {
            None => true,
            Some((s, f)) => score > *s || (score == *s && id.as_str() < f.id.as_str()),
        };
        if better {
            best = Some((score, Found { id: id.clone(), name: aname.to_string() }));
        }
    }
    best.map(|(_, f)| f)
}

/// A sky for a time of day and the location's words: night skies at night, sunsets at dusk and
/// dawn, otherwise daylight skies, preferring ones whose tags share words with the location.
pub(crate) fn best_hdri(list: &Value, time_of_day: &str, words: &[String]) -> Option<Found> {
    let want_cat = match time_of_day {
        "night" | "evening" => "night",
        "dusk" | "dawn" => "sunrise-sunset",
        _ => "skies",
    };
    let mut best: Option<(usize, Found)> = None;
    for (id, a) in list.as_object()? {
        let cats: Vec<&str> = a.get("categories").and_then(|c| c.as_array()).into_iter().flatten().filter_map(|c| c.as_str()).collect();
        if !cats.contains(&want_cat) || cats.contains(&"indoor") || cats.contains(&"studio") {
            continue;
        }
        let mut have: Vec<String> = cats.iter().flat_map(|c| tokens(c)).collect();
        for t in a.get("tags").and_then(|t| t.as_array()).into_iter().flatten().filter_map(|t| t.as_str()) {
            have.extend(tokens(t));
        }
        let score = words.iter().filter(|w| have.contains(w)).count();
        let better = match &best {
            None => true,
            Some((s, f)) => score > *s || (score == *s && id.as_str() < f.id.as_str()),
        };
        if better {
            best = Some((score, Found { id: id.clone(), name: a.get("name").and_then(|n| n.as_str()).unwrap_or(id).to_string() }));
        }
    }
    best.map(|(_, f)| f)
}

pub(crate) async fn find_model(name: &str, tags: &[String], cache_dir: &Path) -> Result<Option<Found>> {
    let v = list("models", cache_dir).await?;
    Ok(best_model(&v, name, tags))
}

pub(crate) async fn find_hdri(time_of_day: &str, words: &[String], cache_dir: &Path) -> Result<Option<Found>> {
    let v = list("hdris", cache_dir).await?;
    Ok(best_hdri(&v, time_of_day, words))
}

/// Downloads a model's 1k glTF and its textures into `dir`; returns the .gltf path.
pub(crate) async fn download_model(id: &str, dir: &Path) -> Result<PathBuf> {
    let files: Value = get_json(&format!("{API}/files/{id}")).await?;
    let res = files.get("gltf").and_then(|g| g.get("1k").or_else(|| g.get("2k"))).and_then(|r| r.get("gltf")).ok_or_else(|| anyhow!("{id} has no glTF download"))?;
    let url = res.get("url").and_then(|u| u.as_str()).ok_or_else(|| anyhow!("{id}: no file url"))?;
    let main = dir.join(file_name(url)?);
    fetch(url, &main).await?;
    for (rel, f) in res.get("include").and_then(|i| i.as_object()).into_iter().flatten() {
        let Some(u) = f.get("url").and_then(|u| u.as_str()) else { continue };
        fetch(u, &dir.join(safe_rel(rel)?)).await?;
    }
    Ok(main)
}

/// Downloads a sky's 1k .hdr into `dir`.
pub(crate) async fn download_hdri(id: &str, dir: &Path) -> Result<PathBuf> {
    let files: Value = get_json(&format!("{API}/files/{id}")).await?;
    let url = files.get("hdri").and_then(|h| h.get("1k")).and_then(|r| r.get("hdr")).and_then(|f| f.get("url")).and_then(|u| u.as_str()).ok_or_else(|| anyhow!("{id} has no 1k HDR"))?;
    let path = dir.join(file_name(url)?);
    fetch(url, &path).await?;
    Ok(path)
}

fn file_name(url: &str) -> Result<String> {
    let name = url.rsplit('/').next().unwrap_or_default().split('?').next().unwrap_or_default().to_string();
    check_ext(&name)?;
    Ok(name)
}

fn check_ext(name: &str) -> Result<()> {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    if !ALLOWED.contains(&ext.as_str()) {
        bail!("refusing to download {name}: not a model, texture or image file");
    }
    Ok(())
}

/// A path inside the asset folder: relative, no `..`, an allowed file type.
pub(crate) fn safe_rel(rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    if p.components().any(|c| !matches!(c, Component::Normal(_))) {
        bail!("refusing unsafe path {rel}");
    }
    check_ext(rel)?;
    Ok(p.to_path_buf())
}

/// One GET, tried again once after a dropped connection.
async fn get_json(url: &str) -> Result<Value> {
    let mut last = None;
    for _ in 0..2 {
        match client()?.get(url).send().await.and_then(|r| r.error_for_status()) {
            Ok(r) => return Ok(r.json().await?),
            Err(e) => last = Some(e),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err(last.map(anyhow::Error::from).unwrap_or_else(|| anyhow!("request failed")))
}

async fn fetch(url: &str, to: &Path) -> Result<()> {
    if !url.starts_with("https://") {
        bail!("refusing non-https download {url}");
    }
    let resp = client()?.get(url).send().await?.error_for_status()?;
    if resp.content_length().is_some_and(|n| n > MAX_FILE) {
        bail!("{url} is too large");
    }
    let bytes = resp.bytes().await?;
    if bytes.len() as u64 > MAX_FILE {
        bail!("{url} is too large");
    }
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(to, &bytes).with_context(|| format!("write {}", to.display()))
}
