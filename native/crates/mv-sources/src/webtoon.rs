//! WEBTOON (English) by scraping webtoons.com (port of `plugins/webtoons/index.js`).
//!
//! Ids: title = `"{genre}|{slug}|{title_no}"`, chapter = `"{genre}|{slug}|{title_no}|{episode_no}"`.
//! Cover and page URLs carry a `#mvref=https%3A%2F%2Fwww.webtoons.com%2F` fragment: the webtoons
//! CDN rejects requests without that Referer.

use std::collections::HashSet;
use std::sync::LazyLock;

use async_trait::async_trait;
use mv_net::Opts;
use regex::Regex;
use scraper::{Html, Selector};

use crate::source::{Result, Source, SourceCtx, SourceError};
use crate::types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo};
use crate::util::{non_empty, sel, svg_icon, text_excluding, text_of, with_ref_hint};

pub const ID: &str = "webtoons";
const BASE: &str = "https://www.webtoons.com";
pub const REFERER: &str = "https://www.webtoons.com/";

static CARD_HREF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/en/([^/]+)/([^/]+)/list\?title_no=(\d+)").unwrap());

pub struct Webtoon {
    ctx: SourceCtx,
}

impl Webtoon {
    pub fn new(ctx: SourceCtx) -> Self {
        Self { ctx }
    }

    pub fn source_info() -> SourceInfo {
        SourceInfo {
            id: ID.into(),
            name: "WEBTOON".into(),
            version: "1.0.0".into(),
            lang: "en".into(),
            nsfw: false,
            requires_wall_solver: false,
            icon: Some(svg_icon("#00D564", "W", 18)),
        }
    }

    async fn html(&self, url: &str) -> Result<String> {
        let opts = Opts::new()
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .referer(REFERER);
        let r = self.ctx.fetch(ID, url, &opts).await?;
        if r.status != 200 {
            return Err(SourceError::Other(format!("WEBTOON HTTP {} for {url}", r.status)));
        }
        Ok(r.text())
    }
}

pub fn pack_title_id(genre: &str, slug: &str, title_no: &str) -> String {
    format!("{genre}|{slug}|{title_no}")
}

fn unpack_title_id(id: &str) -> Result<(&str, &str, &str)> {
    let mut p = id.split('|');
    match (p.next(), p.next(), p.next()) {
        (Some(g), Some(s), Some(n)) if !g.is_empty() && !s.is_empty() && !n.is_empty() => Ok((g, s, n)),
        _ => Err(SourceError::Parse(format!("WEBTOON: bad title id {id:?} (expected genre|slug|title_no)"))),
    }
}

fn unpack_chapter_id(id: &str) -> Result<(&str, &str, &str, &str)> {
    let mut p = id.split('|');
    match (p.next(), p.next(), p.next(), p.next()) {
        (Some(g), Some(s), Some(n), Some(e)) if !g.is_empty() && !e.is_empty() => Ok((g, s, n, e)),
        _ => Err(SourceError::Parse(format!("WEBTOON: bad chapter id {id:?}"))),
    }
}

/// Title cards on search / daily-schedule / originals pages. De-duplicated by `title_no`.
pub fn parse_title_cards(html: &str) -> Vec<MangaSearchResult> {
    let doc = Html::parse_document(html);
    let a_sel = sel("a[href*=\"/list?title_no=\"]");
    let title_sels: Vec<Selector> = ["strong[class*=title]", "p[class*=subj]", "span[class*=subj]", "h3[class*=subj]"].iter().map(|s| sel(s)).collect();
    let author_sel = sel("[class*=author]");
    let img_sel = sel("img[src]");
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for a in doc.select(&a_sel) {
        let href = a.value().attr("href").unwrap_or("");
        let Some(c) = CARD_HREF.captures(href) else { continue };
        let (genre, slug, no) = (&c[1], &c[2], &c[3]);
        if !seen.insert(no.to_string()) {
            continue;
        }
        let title = title_sels.iter().find_map(|s| a.select(s).next()).map(text_of).unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let author = a.select(&author_sel).next().map(text_of).and_then(|s| non_empty(&s));
        let cover = a.select(&img_sel).next().and_then(|i| i.value().attr("src")).map(|u| with_ref_hint(u, REFERER));
        out.push(MangaSearchResult {
            source_id: pack_title_id(genre, slug, no),
            title,
            alt_titles: Some(Vec::new()),
            cover_url: cover,
            author,
            status: None,
            adult: false,
        });
    }
    out
}

fn meta(doc: &Html, prop: &str) -> Option<String> {
    doc.select(&sel(&format!("meta[property=\"{prop}\"]"))).next().and_then(|m| m.value().attr("content")).and_then(non_empty)
}

/// Title page (`/list?title_no=`): og: meta tags + author block + genre badge.
pub fn parse_details(html: &str, manga_id: &str) -> Result<MangaDetails> {
    let (_, slug, _) = unpack_title_id(manga_id)?;
    let doc = Html::parse_document(html);
    let author = doc.select(&sel(".author_area")).next().map(|a| text_excluding(a, "button")).and_then(|s| non_empty(&s));
    let genre = doc.select(&sel("h2[class*=genre]")).next().map(text_of).and_then(|s| non_empty(&s));
    Ok(MangaDetails {
        source_id: manga_id.to_string(),
        title: meta(&doc, "og:title").unwrap_or_else(|| slug.to_string()),
        author,
        artist: None,
        description: Some(meta(&doc, "og:description").unwrap_or_default()),
        cover_url: meta(&doc, "og:image").map(|u| with_ref_hint(&u, REFERER)),
        status: None,
        genres: Some(genre.into_iter().collect()),
    })
}

/// Episode rows of one `/list` page (free episodes only), in page order (newest first).
pub fn parse_episodes(html: &str, genre: &str, slug: &str, title_no: &str) -> Vec<ChapterInfo> {
    let doc = Html::parse_document(html);
    let li = sel("li._episodeItem[data-episode-no]");
    let subj_inner = sel("span.subj span");
    let subj = sel("span.subj");
    let date = sel("span.date");
    doc.select(&li)
        .filter_map(|row| {
            let ep = row.value().attr("data-episode-no")?;
            let n: f64 = ep.parse().ok()?;
            let title = row
                .select(&subj_inner)
                .next()
                .or_else(|| row.select(&subj).next())
                .map(text_of)
                .and_then(|s| non_empty(&s))
                .unwrap_or_else(|| format!("Episode {ep}"));
            Some(ChapterInfo {
                source_id: format!("{genre}|{slug}|{title_no}|{ep}"),
                number: Some(n),
                title: Some(title),
                language: "en".into(),
                scanlation_group: Some("WEBTOON".into()),
                published_at: row.select(&date).next().map(text_of).and_then(|s| non_empty(&s)),
                page_count: None,
            })
        })
        .collect()
}

/// Viewer page: every `<img class="_images" data-url=...>` (the lazy-load real URL).
pub fn parse_viewer_images(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    doc.select(&sel("img._images[data-url]"))
        .filter_map(|i| i.value().attr("data-url"))
        .map(|u| with_ref_hint(u, REFERER))
        .collect()
}

fn paged(mut cards: Vec<MangaSearchResult>, page: u32) -> Vec<MangaSearchResult> {
    let p = page.max(1) as usize - 1;
    if p * 20 >= cards.len() {
        return Vec::new();
    }
    cards.drain(..p * 20);
    cards.truncate(20);
    cards
}

#[async_trait]
impl Source for Webtoon {
    fn info(&self) -> SourceInfo {
        Self::source_info()
    }

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        // NOTE: do not pass searchType=WEBTOON - it redirects to a 3-result stub.
        let url = format!("{BASE}/en/search?keyword={}&page={}", crate::util::encode_component(query), page.max(1));
        Ok(parse_title_cards(&self.html(&url).await?))
    }

    async fn get_popular_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        let html = self.html(&format!("{BASE}/en/dailySchedule")).await?;
        Ok(paged(parse_title_cards(&html), page))
    }

    async fn get_latest_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        // The originals landing page surfaces the newest-updated batch.
        let html = match self.html(&format!("{BASE}/en/originals")).await {
            Ok(h) => h,
            Err(_) => self.html(&format!("{BASE}/en/dailySchedule")).await?,
        };
        Ok(paged(parse_title_cards(&html), page))
    }

    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails> {
        let (genre, slug, no) = unpack_title_id(manga_id)?;
        let html = self.html(&format!("{BASE}/en/{genre}/{slug}/list?title_no={no}")).await?;
        parse_details(&html, manga_id)
    }

    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>> {
        let (genre, slug, no) = unpack_title_id(manga_id)?;
        let mut all: Vec<ChapterInfo> = Vec::new();
        let mut seen = HashSet::new();
        for page in 1..100 {
            let url = format!("{BASE}/en/{genre}/{slug}/list?title_no={no}&page={page}");
            let html = match self.html(&url).await {
                Ok(h) => h,
                Err(e) if page == 1 => return Err(e),
                Err(_) => break,
            };
            let mut found = 0;
            for ch in parse_episodes(&html, genre, slug, no) {
                if seen.insert(ch.source_id.clone()) {
                    all.push(ch);
                    found += 1;
                }
            }
            if found == 0 {
                break;
            }
        }
        // WEBTOON lists newest first; flip so chapter 1 comes first.
        all.sort_by(|a, b| a.number.partial_cmp(&b.number).unwrap_or(std::cmp::Ordering::Equal));
        Ok(all)
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>> {
        let (genre, slug, no, ep) = unpack_chapter_id(chapter_id)?;
        // The episode slug in the URL is cosmetic: the server uses title_no + episode_no.
        let url = format!("{BASE}/en/{genre}/{slug}/ep/viewer?title_no={no}&episode_no={ep}");
        Ok(parse_viewer_images(&self.html(&url).await?))
    }

    fn download_headers(&self) -> Vec<(String, String)> {
        vec![("Referer".into(), REFERER.into())]
    }
}
