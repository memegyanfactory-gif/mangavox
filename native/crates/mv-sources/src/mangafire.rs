//! MangaFire. The site is a React SPA whose chapter API needs a one-time signed `vrf` token the
//! page generates itself, so everything runs through a [`crate::WallSolver`] (a hidden WebView):
//! listings / details / reader are read from the rendered DOM, chapters come from the site's own
//! API signed in-page (the interceptor is borrowed from the site's ES module), with the DOM pager
//! rows as a fallback. Page lists come from the same signed API (`/api/chapters/{id}`): the strip
//! reader only mounts the `<img>`s near the viewport, so a rendered DOM holds just the first few
//! pages. Without a solver every method returns `SourceError::SolverRequired`.

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
///
/// NB: the strip reader only mounts the `<img>` of a page while its `.reader__page` slot is near
/// the viewport, so a rendered DOM usually holds just the first handful of pages. Use
/// [`reader_slot_count`] to tell whether this list is complete; the full list comes from the
/// chapter API ([`api_pages_js`] / [`parse_api_pages`]).
pub fn parse_reader_images(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let mut seen = HashSet::new();
    doc.select(&sel("img.reader-img"))
        .filter_map(|i| ["data-src", "src"].iter().find_map(|k| i.value().attr(k).filter(|s| !s.is_empty())))
        .map(protocol_fix)
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// Number of page slots (`.reader__page`) in a rendered strip reader, rendered or not.
pub fn reader_slot_count(html: &str) -> usize {
    Html::parse_document(html).select(&sel(".reader__page")).count()
}

/// Page entries (`"url"` strings or `{url, width, height}` objects) -> unique URLs in order.
fn page_entries(list: &[Value]) -> Vec<String> {
    let mut seen = HashSet::new();
    list.iter()
        .filter_map(|p| match p {
            Value::String(s) => Some(s.as_str()),
            _ => ["url", "src", "image"].iter().find_map(|k| p.get(*k).and_then(Value::as_str)),
        })
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(protocol_fix)
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// The chapter API's answer (`GET /api/chapters/{id}` -> `{data: {pages: [{url, width, height}]}}`,
/// or the `{pages}` that [`api_pages_js`] resolves to) -> every page URL in reading order.
pub fn parse_api_pages(json: &Value) -> Vec<String> {
    ["/pages", "/data/pages", "/data/data/pages"]
        .iter()
        .find_map(|p| json.pointer(p).and_then(Value::as_array).filter(|a| !a.is_empty()))
        .map(|a| page_entries(a))
        .unwrap_or_default()
}

/// What [`READER_DOM_JS`] resolved to -> `(urls, complete)`. Complete means every page slot of the
/// reader gave a URL.
pub fn parse_reader_dom_result(json: &Value) -> (Vec<String>, bool) {
    let raw = json.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
    let slots = json.get("slots").and_then(Value::as_u64).unwrap_or(0) as usize;
    let missing = raw.iter().any(|p| p.as_str().map_or(true, |s| s.trim().is_empty()));
    let urls = page_entries(&raw);
    let complete = !urls.is_empty() && !missing && (slots == 0 || raw.len() >= slots);
    (urls, complete)
}

/// The numeric chapter id of a `{hid-slug}/chapter/{id}` source id.
pub fn chapter_api_id(chapter_id: &str) -> Option<&str> {
    let id = chapter_id.rsplit_once("/chapter/")?.1.split(['/', '?', '#']).next()?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())).then_some(id)
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
    api_chapters_js_in(hid, "en")
}

/// [`api_chapters_js`] for the chapters in `lang` (a code from [`LANGUAGES`]).
pub fn api_chapters_js_in(hid: &str, lang: &str) -> String {
    format!("{SIGNED_GET_JS}    const path = '/titles/' + {}{}", js_str(hid), API_CHAPTERS_JS_TAIL.replace("__MV_LANG__", &js_str(lang)))
}

/// Runs inside any mangafire.to page: the reader's own signed call, `GET /api/chapters/{id}`,
/// which lists every page of the chapter. Resolves to `{pages: [{url, width, height}]}` or
/// `{error}`; read it with [`parse_api_pages`].
pub fn api_pages_js(chapter_api_id: &str) -> String {
    format!("{SIGNED_GET_JS}{}", API_PAGES_JS_TAIL.replace("__MV_ID__", &js_str(chapter_api_id)))
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Chapter languages MangaFire serves, most common first.
pub const LANGUAGES: [&str; 7] = ["en", "es", "es-la", "fr", "ja", "pt", "pt-br"];

/// The order to try MangaFire's languages in: the preferred one (or its base, "pt" for "pt-br")
/// first, then the rest.
pub fn language_order(pref: Option<&str>) -> Vec<&'static str> {
    let pref = pref.unwrap_or("en").to_ascii_lowercase();
    let base = pref.split(['-', '_']).next().unwrap_or("").to_string();
    let first = LANGUAGES.iter().find(|l| **l == pref).or_else(|| LANGUAGES.iter().find(|l| **l == base));
    first.into_iter().chain(LANGUAGES.iter().filter(|l| Some(*l) != first)).copied().collect()
}

/// Opens an async IIFE + `try` and defines `get(path, params)`: a vrf-signed same-origin API GET.
/// Every user appends its body and closes with `} catch (e) { return { error } } })()`.
const SIGNED_GET_JS: &str = r##"(async () => {
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
"##;

const API_PAGES_JS_TAIL: &str = r##"    const j = await get('/chapters/' + __MV_ID__, {});
    const d = (j && j.data && !Array.isArray(j.data) ? j.data : j) || {};
    const pages = d.pages || (d.data && d.data.pages) || [];
    if (!pages.length) return { error: 'no pages in the chapter API answer' };
    return { pages: pages.map(function (p) { return typeof p === 'string' ? p : { url: p.url || p.src || '', width: p.width || 0, height: p.height || 0 } }) };
  } catch (e) { return { error: String(e && e.message || e) } }
})()"##;

/// Runs on a rendered reader page (fallback when the API call fails). The strip reader mounts a
/// page's `<img>` only while its `.reader__page` slot is in view, but every slot's React props
/// carry the page record, so read the URLs from there; slots that still have no URL are scrolled
/// into view one by one (time-boxed). Resolves to `{pages: [url | ''], slots}`.
pub const READER_DOM_JS: &str = r##"(async () => {
  const sleep = function (ms) { return new Promise(function (r) { setTimeout(r, ms) }) };
  const t0 = Date.now();
  while (Date.now() - t0 < 10000 && !document.querySelector('.reader__page, img.reader-img')) await sleep(250);
  const slots = [].slice.call(document.querySelectorAll('.reader__page'));
  const fromFiber = function (el) {
    const k = Object.keys(el).find(function (k) { return k.indexOf('__reactFiber$') === 0 || k.indexOf('__reactInternalInstance$') === 0 });
    let f = k ? el[k] : null;
    for (let i = 0; f && i < 10; i++, f = f.return) {
      const p = f.memoizedProps;
      if (p && p.page && typeof p.page.url === 'string' && p.page.url) return p.page.url;
    }
    return '';
  };
  const imgOf = function (el) { const i = el.querySelector('img.reader-img'); return i ? (i.getAttribute('src') || i.getAttribute('data-src') || '') : '' };
  if (!slots.length) {
    const imgs = [].slice.call(document.querySelectorAll('img.reader-img')).map(function (i) { return i.getAttribute('src') || i.getAttribute('data-src') || '' });
    return { pages: imgs, slots: 0 };
  }
  const pages = slots.map(function (el) { return fromFiber(el) || imgOf(el) });
  const t1 = Date.now();
  for (let i = 0; i < slots.length && Date.now() - t1 < 15000; i++) {
    if (pages[i]) continue;
    slots[i].scrollIntoView({ block: 'center' });
    for (let w = 0; w < 10 && !pages[i]; w++) { await sleep(60); pages[i] = imgOf(slots[i]) || fromFiber(slots[i]) }
  }
  return { pages: pages, slots: slots.length };
})()"##;

const API_CHAPTERS_JS_TAIL: &str = r##" + '/chapters';
    const base = { language: __MV_LANG__, sort: 'number', order: 'desc', limit: 200 };
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
            // The preferred chapter language first; a title with none in it lists another.
            for lang in language_order(crate::source::chapter_language().as_deref()) {
                let Ok(v) = solver.eval_json(&page, &api_chapters_js_in(hid, lang)).await else { break };
                let Some(items) = v.get("items").and_then(Value::as_array) else { break };
                let mut list = parse_chapter_items(manga_id, items);
                if !list.is_empty() {
                    list.iter_mut().for_each(|c| c.language = lang.into());
                    return Ok(list);
                }
            }
        }
        let html = solver.fetch_html(&page, Some(".title-detail__row-link")).await?;
        Ok(parse_pager_rows(&html))
    }

    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>> {
        // chapter id looks like "{hid-slug}/chapter/{chapterId}"
        let solver = self.ctx.solver("MangaFire")?;
        let reader = format!("{BASE}/title/{chapter_id}");
        // 1) The reader's own API call lists every page. Signed in-page; run on the title page,
        //    which the hidden browser usually still has open from the chapter list.
        if let Some(cid) = chapter_api_id(chapter_id) {
            let manga = chapter_id.split("/chapter/").next().unwrap_or("");
            let host = if manga.is_empty() { reader.clone() } else { format!("{BASE}/title/{manga}") };
            if let Ok(v) = solver.eval_json(&host, &api_pages_js(cid)).await {
                let urls = parse_api_pages(&v);
                if !urls.is_empty() {
                    return Ok(urls);
                }
            }
        }
        // 2) The rendered reader. Its strip view only mounts the images near the viewport (the
        //    old DOM read stopped at ~6 pages), so read every slot's page record instead.
        let mut best: Vec<String> = Vec::new();
        if let Ok(v) = solver.eval_json(&reader, READER_DOM_JS).await {
            let (urls, complete) = parse_reader_dom_result(&v);
            if complete {
                return Ok(urls);
            }
            best = urls;
        }
        // 3) Last resort: the DOM as rendered. Only trusted when no page slot is left without an image.
        let html = self.rendered(&format!("/title/{chapter_id}"), "img.reader-img").await?;
        let urls = parse_reader_images(&html);
        let slots = reader_slot_count(&html);
        if !urls.is_empty() && (slots == 0 || urls.len() >= slots) && urls.len() >= best.len() {
            return Ok(urls);
        }
        let found = urls.len().max(best.len());
        if found == 0 {
            return Err(SourceError::Parse("MangaFire: no page images found (chapter may be unavailable or the site is blocking)".into()));
        }
        // A partial list would be saved as a "complete" download with pages missing: refuse it.
        Err(SourceError::Parse(format!(
            "MangaFire: only {found} of {} pages could be read for this chapter (site slow or blocking); try again",
            slots.max(found + 1)
        )))
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
        Ok(genres.into_iter().map(|(_, n)| Tag::new(slug(&n), n)).collect())
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
