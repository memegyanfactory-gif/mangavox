//! Reads public YouTube pages the way a browser does, no API key: the search results and a
//! channel's video list both carry their data as the `ytInitialData` JSON in the HTML. Search
//! results are `videoRenderer`s; channel tabs are newer `lockupViewModel`s. Also asks AniList
//! for the manga's other titles (English, romaji, Korean / Chinese / Japanese, synonyms).
use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use serde_json::{json, Value};

use super::logic::{parse_age_days, parse_views, Video};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
/// Videos-only search filter.
const ONLY_VIDEOS: &str = "EgIQAQ%3D%3D";

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(UA).timeout(Duration::from_secs(20)).build()?)
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

async fn page(url: &str) -> Result<String> {
    let r = client()?
        .get(url)
        .header("Accept-Language", "en-US,en;q=0.9")
        // Skips the EU cookie-consent interstitial.
        .header("Cookie", "SOCS=CAI; CONSENT=YES+")
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;
    let status = r.status();
    if !status.is_success() {
        return Err(anyhow!("YouTube answered {status}"));
    }
    Ok(r.text().await?)
}

/// The `ytInitialData` object of a YouTube page.
pub fn initial_data(html: &str) -> Option<Value> {
    let key = "ytInitialData";
    let at = html.find(&format!("var {key} = ")).map(|i| i + key.len() + 7).or_else(|| html.find(&format!("window[\"{key}\"] = ")).map(|i| i + key.len() + 13))?;
    serde_json::Deserializer::from_str(&html[at..]).into_iter::<Value>().next()?.ok()
}

fn collect<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == key {
                    out.push(x);
                } else {
                    collect(x, key, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect(x, key, out)),
        _ => {}
    }
}

/// `{"simpleText": …}` or `{"runs": [{"text": …}]}` as one string.
fn text(v: &Value) -> String {
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return s.to_string();
    }
    v.get("runs").and_then(Value::as_array).map(|r| r.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect()).unwrap_or_default()
}

fn from_renderer(r: &Value) -> Option<Video> {
    let id = r.get("videoId")?.as_str()?.to_string();
    let views_text = r.get("viewCountText").map(text).unwrap_or_default();
    let published = r.get("publishedTimeText").map(text).unwrap_or_default();
    let snippet = r
        .get("detailedMetadataSnippets")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|s| s.get("snippetText")).map(text).collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    Some(Video {
        id,
        title: r.get("title").map(text).unwrap_or_default(),
        channel: r.get("ownerText").or_else(|| r.get("longBylineText")).map(text).unwrap_or_default(),
        snippet,
        // Live streams say "1.2K watching": no view count to compare.
        views: parse_views(&views_text).filter(|_| !views_text.contains("watching"))?,
        age_days: parse_age_days(&published),
        published,
        length: r.get("lengthText").map(text).unwrap_or_default(),
        ..Default::default()
    })
}

fn from_lockup(l: &Value, channel: &str) -> Option<Video> {
    if l.get("contentType").and_then(Value::as_str) != Some("LOCKUP_CONTENT_TYPE_VIDEO") {
        return None;
    }
    let meta = l.pointer("/metadata/lockupMetadataViewModel")?;
    let parts: Vec<&Value> = meta.pointer("/metadata/contentMetadataViewModel/metadataRows").and_then(Value::as_array)?.iter().filter_map(|r| r.get("metadataParts")?.as_array()).flatten().collect();
    let part = |p: &Value| p.pointer("/text/content").and_then(Value::as_str).unwrap_or("").to_string();
    let label = |p: &Value| p.get("accessibilityLabel").and_then(Value::as_str).unwrap_or("").to_string();
    let views = parts.iter().find_map(|p| [label(p), part(p)].iter().find(|s| s.contains("view")).and_then(|s| parse_views(s)))?;
    let published = parts.iter().map(|p| part(p)).find(|s| s.contains("ago")).unwrap_or_default();
    let mut length = String::new();
    let mut badges = vec![];
    collect(l, "thumbnailBadgeViewModel", &mut badges);
    if let Some(b) = badges.first().and_then(|b| b.get("text")).and_then(Value::as_str) {
        length = b.to_string();
    }
    Some(Video {
        id: l.get("contentId")?.as_str()?.to_string(),
        title: meta.pointer("/title/content").and_then(Value::as_str).unwrap_or("").to_string(),
        channel: channel.to_string(),
        views,
        age_days: parse_age_days(&published),
        published,
        length,
        ..Default::default()
    })
}

/// Every video on a search or channel page, in page order, without repeats.
pub fn videos_in(data: &Value, channel: &str) -> Vec<Video> {
    let (mut rs, mut ls) = (vec![], vec![]);
    collect(data, "videoRenderer", &mut rs);
    collect(data, "lockupViewModel", &mut ls);
    let mut out: Vec<Video> = rs.into_iter().filter_map(from_renderer).chain(ls.into_iter().filter_map(|l| from_lockup(l, channel))).collect();
    let mut seen = std::collections::HashSet::new();
    out.retain(|v| seen.insert(v.id.clone()));
    for v in out.iter_mut().filter(|v| v.channel.is_empty()) {
        v.channel = channel.to_string();
    }
    out
}

/// A channel page's name and subscriber text ("645K subscribers").
pub fn channel_header(data: &Value) -> (String, String) {
    let mut hs = vec![];
    collect(data, "pageHeaderViewModel", &mut hs);
    let Some(h) = hs.first() else { return Default::default() };
    let name = h.pointer("/title/dynamicTextViewModel/text/content").and_then(Value::as_str).unwrap_or("").to_string();
    let mut parts = vec![];
    collect(h, "content", &mut parts);
    let subs = parts.iter().filter_map(|p| p.as_str()).find(|s| s.contains("subscriber")).unwrap_or("").to_string();
    (name, subs)
}

/// YouTube search, videos only.
pub async fn search(query: &str) -> Result<Vec<Video>> {
    let html = page(&format!("https://www.youtube.com/results?search_query={}&sp={ONLY_VIDEOS}&hl=en&gl=US", enc(query))).await?;
    let data = initial_data(&html).ok_or_else(|| anyhow!("YouTube's search page changed; no results found in it"))?;
    Ok(videos_in(&data, ""))
}

/// A channel's latest uploads plus its name and subscriber count.
pub async fn channel_videos(handle: &str) -> Result<(String, String, Vec<Video>)> {
    let html = page(&format!("https://www.youtube.com/{handle}/videos?hl=en&gl=US")).await?;
    let data = initial_data(&html).ok_or_else(|| anyhow!("could not read {handle}'s page"))?;
    let (name, subs) = channel_header(&data);
    let name = if name.is_empty() { handle.trim_start_matches('@').to_string() } else { name };
    let videos = videos_in(&data, &name);
    Ok((name, subs, videos))
}

/// Search inside one channel.
pub async fn channel_search(handle: &str, query: &str, channel_name: &str) -> Result<Vec<Video>> {
    let html = page(&format!("https://www.youtube.com/{handle}/search?query={}&hl=en&gl=US", enc(query))).await?;
    Ok(initial_data(&html).map(|d| videos_in(&d, channel_name)).unwrap_or_default())
}

/// AniList's titles for a manga: English, romaji, native and synonyms (any language), best match first.
pub async fn anilist_titles(title: &str) -> Result<Vec<String>> {
    let q = "query($q:String){Page(perPage:3){media(search:$q,type:MANGA){title{romaji english native} synonyms}}}";
    let r: Value = client()?.post("https://graphql.anilist.co").json(&json!({"query": q, "variables": {"q": title}})).send().await?.error_for_status()?.json().await?;
    Ok(titles_from_anilist(&r, title))
}

/// The titles of the AniList entry that matches `title` (by any of its names), deduplicated.
pub fn titles_from_anilist(r: &Value, title: &str) -> Vec<String> {
    let media = r.pointer("/data/Page/media").and_then(Value::as_array).cloned().unwrap_or_default();
    for m in media {
        let mut names: Vec<String> = ["english", "romaji", "native"].iter().filter_map(|k| m.pointer(&format!("/title/{k}")).and_then(Value::as_str)).map(str::to_string).collect();
        names.extend(m.get("synonyms").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
        if names.iter().any(|n| super::logic::name_score(n, title) >= 0.85 || super::logic::name_score(title, n) >= 0.85) {
            let mut out: Vec<String> = vec![];
            for n in names {
                let n = n.trim().to_string();
                if !n.is_empty() && !out.iter().any(|o| super::logic::normalize(o) == super::logic::normalize(&n)) {
                    out.push(n);
                }
            }
            return out;
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = r#"<html><script>var ytInitialData = {"contents":{"x":[{"videoRenderer":{"videoId":"abc","title":{"runs":[{"text":"[1-25] Sword Devouring Swordmaster!"}]},"ownerText":{"runs":[{"text":"Zett Manhwa"}]},"viewCountText":{"simpleText":"14,099 views"},"publishedTimeText":{"simpleText":"2 weeks ago"},"lengthText":{"simpleText":"1:02:03"},"detailedMetadataSnippets":[{"snippetText":{"runs":[{"text":"Arhan "},{"text":"Karavan","bold":true}]}}]}},{"videoRenderer":{"videoId":"live","title":{"runs":[{"text":"Live"}]},"viewCountText":{"runs":[{"text":"1.2K"},{"text":" watching"}]}}},{"videoRenderer":{"videoId":"abc","title":{"runs":[{"text":"dup"}]},"viewCountText":{"simpleText":"1 view"}}}]}};</script>"#;

    #[test]
    fn reads_search_results() {
        let d = initial_data(SEARCH).unwrap();
        let v = videos_in(&d, "");
        assert_eq!(v.len(), 1, "live stream and repeat dropped: {v:?}");
        assert_eq!(v[0].id, "abc");
        assert_eq!(v[0].title, "[1-25] Sword Devouring Swordmaster!");
        assert_eq!((v[0].channel.as_str(), v[0].views, v[0].age_days), ("Zett Manhwa", 14_099, Some(14)));
        assert_eq!((v[0].snippet.as_str(), v[0].length.as_str()), ("Arhan Karavan", "1:02:03"));
        assert!(v[0].thumb().contains("/vi/abc/"));
    }

    #[test]
    fn reads_channel_lockups_and_header() {
        let html = r#"var ytInitialData = {"header":{"pageHeaderViewModel":{"title":{"dynamicTextViewModel":{"text":{"content":"Manhwa Fresh"}}},"metadata":{"contentMetadataViewModel":{"metadataRows":[{"metadataParts":[{"text":{"content":"@Manhwa_Fresh"}}]},{"metadataParts":[{"text":{"content":"645K subscribers"}},{"text":{"content":"240 videos"}}]}]}}}},
        "tabs":[{"richItemRenderer":{"content":{"lockupViewModel":{"contentId":"Cba","contentType":"LOCKUP_CONTENT_TYPE_VIDEO","contentImage":{"thumbnailViewModel":{"overlays":[{"thumbnailBottomOverlayViewModel":{"badges":[{"thumbnailBadgeViewModel":{"text":"2:25:04"}}]}}]}},
        "metadata":{"lockupMetadataViewModel":{"title":{"content":"When a Genius Blacksmith Humiliates Everyone!"},"metadata":{"contentMetadataViewModel":{"metadataRows":[{"metadataParts":[{"text":{"content":"95K"},"accessibilityLabel":"95 thousand views"},{"text":{"content":"12h ago"}}]}]}}}}}}}},
        {"lockupViewModel":{"contentId":"PL1","contentType":"LOCKUP_CONTENT_TYPE_PLAYLIST"}}]};"#;
        let d = initial_data(html).unwrap();
        assert_eq!(channel_header(&d), ("Manhwa Fresh".into(), "645K subscribers".into()));
        let v = videos_in(&d, "Manhwa Fresh");
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].id.as_str(), v[0].views, v[0].published.as_str(), v[0].length.as_str()), ("Cba", 95_000, "12h ago", "2:25:04"));
        assert_eq!(v[0].channel, "Manhwa Fresh");
    }

    #[test]
    fn no_data_no_panic() {
        assert!(initial_data("<html>nothing</html>").is_none());
        assert!(initial_data("var ytInitialData = {broken").is_none());
        assert_eq!(channel_header(&json!({})), (String::new(), String::new()));
    }

    #[test]
    fn anilist_titles_of_the_matching_entry() {
        let r = json!({"data":{"Page":{"media":[
            {"title":{"romaji":"Other","english":"Another Manga","native":null},"synonyms":[]},
            {"title":{"romaji":"Geom Meongneun Swordmaster","english":"The Sword-Eating Swordmaster","native":"검 먹는 소드마스터"},"synonyms":["Sword-Devouring Swordmaster","噬劍劍聖","The Sword-Eating Swordmaster"]}
        ]}}});
        assert_eq!(
            titles_from_anilist(&r, "The Sword-Eating Swordmaster"),
            vec!["The Sword-Eating Swordmaster", "Geom Meongneun Swordmaster", "검 먹는 소드마스터", "Sword-Devouring Swordmaster", "噬劍劍聖"]
        );
        assert!(titles_from_anilist(&r, "Unrelated Thing").is_empty());
        assert!(titles_from_anilist(&json!({}), "x").is_empty());
    }
}
