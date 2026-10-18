//! MangaFire. The site is a React SPA whose chapter API needs a one-time signed `vrf` token the
//! page generates itself, so everything runs through a [`crate::WallSolver`] (a hidden WebView):
//! listings / details / reader are read from the rendered DOM, chapters come from the site's own
//! API signed in-page (the interceptor is borrowed from the site's ES module), with the DOM pager
//! rows as a fallback. Without a solver every method returns `SourceError::SolverRequired`.

use std::collections::HashSet;

use async_trait::async_trait;
use scraper::Html;
use serde_json::Value;

use crate::source::{Result, Source, SourceCtx, SourceError};
use crate::types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo, Tag};
use crate::util::{encode_component, sel, svg_icon, text_of};

pub const ID: &str = "mangafire";
const BASE: &str = "https://mangafire.to";

pub struct MangaFire {
    ctx: SourceCtx,
}

/// Selector that appears once a browse page has rendered its cards.
const CARDS_READY: &str = "a[href^=\"/title/\"] img";

impl MangaFire {
    pub fn new(ctx: SourceCtx) -> Self {
        Self { ctx }
    }

    pub fn source_info() -> SourceInfo {
        SourceInfo {
            id: ID.into(),
            name: "MangaFire".into(),
            version: "2.1.0".into(),
            lang: "en".into(),
            nsfw: false,
            requires_wall_solver: true,
            icon: Some(svg_icon("#E44D26", "MF", 10)),
        }
    }

    async fn rendered(&self, path_or_url: &str, wait_for: &str) -> Result<String> {
        let solver = self.ctx.solver("MangaFire")?;
        let url = if path_or_url.starts_with("http") { path_or_url.to_string() } else { format!("{BASE}{path_or_url}") };
        solver.fetch_html(&url, Some(wait_for)).await
    }

    async fn scrape_list(&self, path: String) -> Result<Vec<MangaSearchResult>> {
        let html = self.rendered(&path, CARDS_READY).await?;
        let cards = parse_cards(&html);
        if cards.is_empty() && !html.contains("No titles match") {
            return Err(SourceError::Parse("MangaFire: results did not load (site slow, blocked or Cloudflare challenge)".into()));
        }
        Ok(cards)
    }
}

pub fn browse_path(sort: &str, page: u32, keyword: Option<&str>, genre: Option<&str>) -> String {
    let mut q: Vec<String> = Vec::new();
    if let Some(k) = keyword {
        q.push(format!("keyword={}", encode_component(k)));
    }
    q.push(format!("sort={}", encode_component(sort)));
    if let Some(g) = genre {
        q.push(format!("genres_in={g}"));
    }
    q.push(format!("page={}", page.max(1)));
    format!("/browse?{}", q.join("&"))
}

fn protocol_fix(u: &str) -> String {
    match u.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => u.to_string(),
    }
}

/// Rendered browse page -> cards (`/title/{id}` anchors that contain an image).
pub fn parse_cards(html: &str) -> Vec<MangaSearchResult> {
    let doc = Html::parse_document(html);
    let a_sel = sel("a[href^=\"/title/\"]");
    let img_sel = sel("img");
    let title_sel = sel("[class$=\"__title\"]");
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for a in doc.select(&a_sel) {
        let href = a.value().attr("href").unwrap_or("");
        if href.contains("/chapter/") {
            continue;
        }
        let Some(img) = a.select(&img_sel).next() else { continue };
        let id = href.trim_start_matches("/title/").split(['?', '#']).next().unwrap_or("");
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        let title = a
            .select(&title_sel)
            .next()
            .map(text_of)
            .filter(|t| !t.is_empty())
            .or_else(|| img.value().attr("alt").map(str::to_string))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let cover = ["src", "data-src"].iter().find_map(|k| img.value().attr(k).filter(|s| !s.is_empty())).map(protocol_fix);
        out.push(MangaSearchResult { source_id: id.to_string(), title, cover_url: cover, ..Default::default() });
    }
    out
}

/// Rendered `/title/{id}` page -> details.
pub fn parse_details(html: &str, manga_id: &str) -> Result<MangaDetails> {
    let doc = Html::parse_document(html);
    let first = |css: &str| doc.select(&sel(css)).next();
    let title = first(".title-detail__title").map(text_of).filter(|t| !t.is_empty()).ok_or_else(|| SourceError::Parse("MangaFire: title page did not load".into()))?;
    let genres: Vec<String> = doc.select(&sel(".title-detail__tag")).map(text_of).filter(|s| !s.is_empty()).collect();
    let mut credits: Vec<String> = Vec::new();
    for c in doc.select(&sel(".title-detail__credit-link")).map(text_of) {
        if !credits.contains(&c) {
            credits.push(c);
        }
    }
    // Synopsis: <br> -> newline, drop the trailing "Original/Official ..." block.
    let desc = first(".title-detail__synopsis p").map(|p| {
        let mut s = String::new();
        for node in p.descendants() {
            if let Some(t) = node.value().as_text() {
                s.push_str(t);
            } else if node.value().as_element().map_or(false, |e| e.name() == "br") {
                s.push('\n');
            }
        }
        let cut = regex::Regex::new(r"\n\s*(Original|Official)[\s\S]*$").unwrap().replace(&s, "").trim().to_string();
        cut
    });
    let st = first(".badge--status").map(text_of).unwrap_or_default().to_lowercase();
    let status = if st.contains("releasing") || st.contains("ongoing") {
        "ongoing"
    } else if st.contains("finished") || st.contains("complete") {
        "completed"
    } else if st.contains("hiatus") {
        "hiatus"
    } else if st.contains("cancel") || st.contains("discontinued") {
        "cancelled"
    } else {
        "unknown"
    };
    Ok(MangaDetails {
        source_id: manga_id.to_string(),
        title,
        author: (!credits.is_empty()).then(|| credits.join(", ")),
        artist: None,
        description: desc.filter(|d| !d.is_empty()),
        cover_url: first(".title-detail__poster img").and_then(|i| i.value().attr("src")).map(protocol_fix),
        status: Some(status.into()),
        genres: Some(genres),
    })
}

/// Chapter rows from the in-page API call (`{items:[{id,number,name,createdAt}]}`) -> list.
pub fn parse_chapter_items(manga_id: &str, items: &[Value]) -> Vec<ChapterInfo> {
    let mut out: Vec<ChapterInfo> = Vec::new();
    for c in items {
        let num_s = c.get("number").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
        let n = num_s.trim().parse::<f64>().ok().unwrap_or(0.0);
        let id = c.get("id").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
        let name = c.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        // The site lists several uploads per chapter number: keep the first (top-listed) of each.
        if out.iter().any(|o| o.number == Some(n)) {
            continue;
        }
        out.push(ChapterInfo {
            source_id: format!("{manga_id}/chapter/{id}"),
            number: Some(n),
            title: Some(if name.is_empty() { format!("Ch. {num_s}") } else { format!("Ch. {num_s}: {name}") }),
            language: "en".into(),
            published_at: c.get("createdAt").and_then(Value::as_i64).map(unix_to_iso),
            ..Default::default()
        });
    }
    out.sort_by(|a, b| a.number.partial_cmp(&b.number).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Fallback: rows of the rendered title page (`.title-detail__row`), first pager page only.
pub fn parse_pager_rows(html: &str) -> Vec<ChapterInfo> {
    let doc = Html::parse_document(html);
    let link = sel(".title-detail__row-link");
    let num = sel(".title-detail__row-num");
    let num_re = regex::Regex::new(r"(\d+(?:\.\d+)?)").unwrap();
    let mut out: Vec<ChapterInfo> = Vec::new();
    for row in doc.select(&sel(".title-detail__row")) {
        let Some(a) = row.select(&link).next() else { continue };
        let href = a.value().attr("href").unwrap_or("");
        let id = href.strip_prefix("/title/").unwrap_or(href).to_string();
        let label = row.select(&num).next().map(text_of).unwrap_or_default();
        let n = num_re.captures(&label).and_then(|c| c[1].parse::<f64>().ok()).unwrap_or(0.0);
        if out.iter().any(|o| o.number == Some(n)) {
            continue;
        }
        out.push(ChapterInfo {
            source_id: id,
            number: Some(n),
            title: Some(if label.is_empty() { format!("Chapter {n}") } else { label }),
            language: "en".into(),
            ..Default::default()
        });
    }
    out.sort_by(|a, b| a.number.partial_cmp(&b.number).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Rendered reader page -> unique `img.reader-img` URLs in order.
pub fn parse_reader_images(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let mut seen = HashSet::new();
    doc.select(&sel("img.reader-img"))
        .filter_map(|i| ["data-src", "src"].iter().find_map(|k| i.value().attr(k).filter(|s| !s.is_empty())))
        .map(protocol_fix)
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// Seconds since the epoch -> `YYYY-MM-DDTHH:MM:SSZ` (civil-from-days, no chrono dependency).
pub fn unix_to_iso(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Genre snapshot of `/api/filter-options`, used when the live list cannot be fetched.
pub const STATIC_GENRES: &[(u32, &str)] = &[
    (1, "Action"), (268929, "Adult"), (78, "Adventure"), (3, "Avant Garde"), (4, "Boys Love"), (5, "Comedy"),
    (268921, "Crime"), (77, "Demons"), (6, "Drama"), (7, "Ecchi"), (79, "Fantasy"), (9, "Girls Love"),
    (10, "Gourmet"), (268930, "Hentai"), (268922, "Historical"), (530, "Horror"), (13, "Isekai"),
    (531, "Iyashikei"), (15, "Josei"), (532, "Kids"), (539, "Magic"), (268923, "Magical Girls"),
    (533, "Mahou Shoujo"), (534, "Martial Arts"), (268931, "Mature"), (19, "Mecha"), (268924, "Medical"),
    (535, "Military"), (21, "Music"), (22, "Mystery"), (23, "Parody"), (268925, "Philosophical"),
    (536, "Psychological"), (26, "Romance"), (73, "School"), (28, "Sci-Fi"), (537, "Seinen"), (30, "Shoujo"),
    (31, "Shounen"), (538, "Slice of Life"), (268932, "Smut"), (33, "Space"), (34, "Sports"), (75, "Super Power"),
    (268926, "Superhero"), (76, "Supernatural"), (37, "Suspense"), (38, "Thriller"), (268927, "Tragedy"),
    (39, "Vampire"), (268928, "Wuxia"),
];

pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    out
}

/// `/api/filter-options` JSON -> `(id, name)` genres.
pub fn parse_genres(json: &Value) -> Vec<(u32, String)> {
    json.pointer("/data/genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let id = g.get("id").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))?.parse::<u32>().ok()?;
            Some((id, g.get("name").and_then(Value::as_str)?.to_string()))
        })
        .collect()
}

/// Runs inside a mangafire.to page: borrows the site's axios vrf-signing request interceptor from
/// its own ES module, then pulls every chapter page of `/api/titles/{hid}/chapters` (200 per page).
/// Resolves to `{items, total}` or `{error}`.
pub fn api_chapters_js(hid: &str) -> String {
    let hid_json = serde_json::to_string(hid).unwrap_or_else(|_| "\"\"".into());
    format!("{API_CHAPTERS_JS_HEAD}{hid_json}{API_CHAPTERS_JS_TAIL}")
}

const API_CHAPTERS_JS_HEAD: &str = r##"(async () => {
  try {
    if (!window.__mvSign) {
      const urls = [].concat(
        performance.getEntriesByType('resource').map(function (e) { return e.name }),
        [].map.call(document.querySelectorAll('script[src], link[rel="modulepreload"][href]'), function (e) { return e.src || e.href })
      ).filter(function (u, i, a) { return /\/assets\/[^/?#]+\.js(\?|$)/.test(u) && !/\/(main|rolldown-runtime)-/.test(u) && a.indexOf(u) === i });
      urls.sort(function (a, b) { return (/polyfill-/.test(b) ? 1 : 0) - (/polyfill-/.test(a) ? 1 : 0) });
      for (const u of urls) {
        let m; try { m = await import(u) } catch (e) { continue }
        const keys = Object.keys(m).sort(function (a, b) { return a === 'a' ? -1 : b === 'a' ? 1 : 0 });
        for (const k of keys) {
          const fn = m[k];
          if (typeof fn !== 'function' || fn.length !== 1) continue;
          let cap = null;
          try { fn({ interceptors: { request: { use: function (f) { cap = cap || f } }, response: { use: function () {} } }, defaults: { headers: { common: {} } } }) } catch (e) {}
          if (typeof cap === 'function') { window.__mvSign = cap; break }
        }
        if (window.__mvSign) break;
      }
    }
    const sign = window.__mvSign;
    if (!sign) return { error: 'vrf signer not found' };
    const get = async (path, params) => {
      const c = (await sign({ url: path, baseURL: '/api', method: 'get', params: Object.assign({}, params), headers: {} })) || {};
      const q = new URLSearchParams(c.params || params).toString();
      const h = Object.assign({ Accept: 'application/json', 'X-Requested-With': 'XMLHttpRequest' }, c.headers || {});
      const r = await fetch('/api' + (c.url || path) + (q ? '?' + q : ''), { credentials: 'include', headers: h });
      if (!r.ok) throw new Error('HTTP ' + r.status + ' for ' + path);
      return r.json();
    };
    const path = '/titles/' + "##;

const API_CHAPTERS_JS_TAIL: &str = r##" + '/chapters';
    const base = { language: 'en', sort: 'number', order: 'desc', limit: 200 };
    const first = await get(path, Object.assign({ page: 1 }, base));
    const pages = [first.items || []];
    const last = Math.min((first.meta && first.meta.lastPage) || 1, 200);
    let next = 2;
    const worker = async () => { while (next <= last) { const p = next++; const j = await get(path, Object.assign({ page: p }, base)); pages[p - 1] = j.items || [] } };
    await Promise.all([worker(), worker(), worker(), worker()]);
    const items = [].concat.apply([], pages).map(function (c) { return { id: c.id, number: c.number, name: c.name || '', createdAt: c.createdAt } });
    return { items: items, total: first.meta ? first.meta.total : items.length };
  } catch (e) { return { error: String(e && e.message || e) } }
})()"##;

#[async_trait]
impl Source for MangaFire {
    fn info(&self) -> SourceInfo {
        Self::source_info()
    }

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.scrape_list(browse_path("relevance:desc", page, Some(query), None)).await
    }

    async fn get_popular_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.scrape_list(browse_path("views_total:desc", page, None, None)).await
    }

    async fn get_latest_manga(&self, page: u32) -> Result<Vec<MangaSearchResult>> {
        self.scrape_list(browse_path("chapter_updated_at:desc", page, None, None)).await
    }

    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails> {
        let html = self.rendered(&format!("/title/{manga_id}"), ".title-detail__title").await?;
        parse_details(&html, manga_id)
    }

    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>> {
        let solver = self.ctx.solver("MangaFire")?;
        let page = format!("{BASE}/title/{manga_id}");
        let hid = manga_id.split('-').next().unwrap_or("");
        if !hid.is_empty() && hid.chars().all(|c| c.is_ascii_alphanumeric()) {
            if let Ok(v) = solver.eval_json(&page, &api_chapters_js(hid)).await {
                if let Some(items) = v.get("items").and_then(Value::as_array) {
                    let list = parse_chapter_items(manga_id, items);
                    if !list.is_empty() {
                        return Ok(list);
                    }
                }
            }
        }
        let html = solver.fetch_html(&page, Some(".title-detail__row-link")).await?;
        Ok(parse_pager_rows(&html))
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>> {
        // chapter id looks like "{hid-slug}/chapter/{chapterId}"
        let html = self.rendered(&format!("/title/{chapter_id}"), "img.reader-img").await?;
        let urls = parse_reader_images(&html);
        if urls.is_empty() {
            return Err(SourceError::Parse("MangaFire: no page images found (chapter may be unavailable or the site is blocking)".into()));
        }
        Ok(urls)
    }

    async fn get_tag_list(&self) -> Result<Vec<Tag>> {
        let live = match self.ctx.solver.as_ref() {
            Some(s) => s
                .fetch_text(&format!("{BASE}/api/filter-options"), &[("Accept".into(), "application/json".into()), ("X-Requested-With".into(), "XMLHttpRequest".into())])
                .await
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .map(|j| parse_genres(&j))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let genres: Vec<(u32, String)> = if live.is_empty() { STATIC_GENRES.iter().map(|(i, n)| (*i, n.to_string())).collect() } else { live };
        Ok(genres.into_iter().map(|(_, n)| Tag { id: slug(&n), name: n }).collect())
    }

    async fn get_manga_by_tag(&self, tag_id: &str, page: u32) -> Result<Vec<MangaSearchResult>> {
        let t = tag_id.trim();
        let gid = if t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty() {
            Some(t.to_string())
        } else {
            let s = slug(t);
            STATIC_GENRES.iter().find(|(_, n)| slug(n) == s).map(|(i, _)| i.to_string())
        };
        match gid {
            Some(g) => self.scrape_list(browse_path("views_total:desc", page, None, Some(&g))).await,
            None => Ok(Vec::new()),
        }
    }

    fn download_headers(&self) -> Vec<(String, String)> {
        vec![("Referer".into(), format!("{BASE}/"))]
    }
}
