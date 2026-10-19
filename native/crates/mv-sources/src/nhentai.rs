//! nhentai through its official JSON API v2 (`https://nhentai.net/api/v2`).
//!
//! The Electron app scraped the HTML in a hidden window; the site now asks scrapers to use the
//! API, which answers plain HTTP clients, so this source needs no browser. A Cloudflare wall
//! still falls back to the attached [`crate::WallSolver`] (cookies / in-browser fetch).
//!
//! Ids are numeric gallery ids; a gallery is one chapter with the same id. Tag ids are numeric
//! nhentai tag ids (from [`Source::get_tag_list`]).

use async_trait::async_trait;
use mv_net::Opts;
use serde_json::Value;

use crate::source::{Result, Source, SourceCtx, SourceError};
use crate::types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo, Tag};
use crate::util::{encode_component, svg_icon};

pub const ID: &str = "nhentai";
const API: &str = "https://nhentai.net/api/v2";
const IMAGE_HOST: &str = "https://i1.nhentai.net";
const THUMB_HOST: &str = "https://t1.nhentai.net";

pub struct NHentai {
    ctx: SourceCtx,
}

impl NHentai {
    pub fn new(ctx: SourceCtx) -> Self {
        Self { ctx }
    }

    pub fn source_info() -> SourceInfo {
        SourceInfo {
            id: ID.into(),
            name: "nhentai".into(),
            version: "2.0.0".into(),
            lang: "en".into(),
            nsfw: true,
            requires_wall_solver: false,
            icon: Some(svg_icon("#ED2553", "NH", 12)),
        }
    }

    async fn api(&self, path: &str) -> Result<Value> {
        let opts = Opts::new().header("Accept", "application/json").referer("https://nhentai.net/");
        let r = self.ctx.fetch(ID, &format!("{API}{path}"), &opts).await?;
        if r.status == 404 {
            return Err(SourceError::NotFound(format!("nhentai: not found ({path})")));
        }
        Ok(r.error_for_status()?.json()?)
    }
}

fn numeric_id(id: &str) -> Result<String> {
    let d: String = id.chars().filter(char::is_ascii_digit).collect();
    if d.is_empty() {
        Err(SourceError::Parse(format!("nhentai: bad gallery id {id:?}")))
    } else {
        Ok(d)
    }
}

fn thumb_url(path: &str) -> String {
    format!("{THUMB_HOST}/{}", path.trim_start_matches('/'))
}

/// A search / popular / latest / tagged listing: `{result:[...]}` or a bare array.
pub fn parse_list(json: &Value) -> Vec<MangaSearchResult> {
    let items = json.get("result").and_then(Value::as_array).or_else(|| json.as_array());
    items
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let id = g.get("id").and_then(Value::as_u64)?;
            let title = ["english_title", "japanese_title"].iter().find_map(|k| g.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))?;
            Some(MangaSearchResult {
                source_id: id.to_string(),
                title: title.to_string(),
                cover_url: g.get("thumbnail").and_then(Value::as_str).filter(|s| !s.is_empty()).map(thumb_url),
                ..Default::default()
            })
        })
        .collect()
}

fn tag_names(g: &Value, ty: &str) -> Vec<String> {
    g.get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|t| t.get("type").and_then(Value::as_str) == Some(ty))
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn gallery_title(g: &Value) -> String {
    ["english", "pretty", "japanese"]
        .iter()
        .find_map(|k| g.pointer(&format!("/title/{k}")).and_then(Value::as_str).filter(|s| !s.is_empty()))
        .unwrap_or("Untitled")
        .to_string()
}

/// `GET /galleries/{id}` -> details.
pub fn parse_details(g: &Value, manga_id: &str) -> MangaDetails {
    let tags = tag_names(g, "tag");
    let artists = tag_names(g, "artist");
    let groups = tag_names(g, "group");
    let pages = g.get("num_pages").and_then(Value::as_u64).map(|n| n.to_string()).unwrap_or_else(|| "?".into());
    MangaDetails {
        source_id: manga_id.to_string(),
        title: gallery_title(g),
        author: if !artists.is_empty() { Some(artists.join(", ")) } else if !groups.is_empty() { Some(groups.join(", ")) } else { None },
        artist: (!artists.is_empty()).then(|| artists.join(", ")),
        description: Some(format!("Tags: {}\nPages: {pages}", tags.join(", "))),
        cover_url: g.pointer("/cover/path").and_then(Value::as_str).map(thumb_url),
        status: Some("completed".into()),
        genres: Some(tags),
    }
}

pub fn parse_chapter(g: &Value, manga_id: &str) -> ChapterInfo {
    ChapterInfo {
        source_id: manga_id.to_string(),
        number: Some(1.0),
        title: Some(gallery_title(g)),
        language: "en".into(),
        page_count: g.get("num_pages").and_then(Value::as_u64).map(|n| n as u32),
        ..Default::default()
    }
}

/// `GET /galleries/{id}` -> full-size page URLs in order.
pub fn parse_pages(g: &Value) -> Vec<String> {
    let mut pages: Vec<(u64, String)> = g
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p.get("number").and_then(Value::as_u64)?, format!("{IMAGE_HOST}/{}", p.get("path").and_then(Value::as_str)?.trim_start_matches('/')))))
        .collect();
    pages.sort_by_key(|(n, _)| *n);
    pages.into_iter().map(|(_, u)| u).collect()
}

/// `GET /tags/tag?sort=popular` -> tags (numeric id as string).
pub fn parse_tags(json: &Value) -> Vec<Tag> {
    json.get("result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| Some(Tag::new(t.get("id").and_then(Value::as_u64)?.to_string(), t.get("name").and_then(Value::as_str)?.to_string())))
        .collect()
}

#[async_trait]
impl Source for NHentai {
    fn info(&self) -> SourceInfo {
        Self::source_info()
    }

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        Ok(parse_list(&self.api(&format!("/search?query={}&page={}", encode_component(query), page.max(1))).await?))
    }

    async fn get_popular_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        Ok(parse_list(&self.api(&format!("/search?query={}&sort=popular&page={}", encode_component("*"), page.max(1))).await?))
    }

    async fn get_latest_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        Ok(parse_list(&self.api(&format!("/galleries?page={}", page.max(1))).await?))
    }

    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails> {
        Ok(parse_details(&self.api(&format!("/galleries/{}", numeric_id(manga_id)?)).await?, manga_id))
    }

    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>> {
        Ok(vec![parse_chapter(&self.api(&format!("/galleries/{}", numeric_id(manga_id)?)).await?, manga_id)])
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>> {
        let pages = parse_pages(&self.api(&format!("/galleries/{}", numeric_id(chapter_id)?)).await?);
        if pages.is_empty() {
            return Err(SourceError::Parse("nhentai: no pages found".into()));
        }
        Ok(pages)
    }

    async fn get_tag_list(&self) -> Result<Vec<Tag>> {
        Ok(parse_tags(&self.api("/tags/tag?sort=popular&page=1").await?))
    }

    async fn get_manga_by_tag(&self, tag_id: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        let id = numeric_id(tag_id)?;
        Ok(parse_list(&self.api(&format!("/galleries/tagged?tag_id={id}&sort=popular&page={}", page.max(1))).await?))
    }

    fn download_headers(&self) -> Vec<(String, String)> {
        vec![("Referer".into(), "https://nhentai.net/".into())]
    }
}
