// WEBTOON (English) source plugin for MangaVox.
//
// Scrapes the free public English catalog at webtoons.com. Daily Pass / Fast
// Pass chapters are not in the free HTML and won't be returned.
//
// Image URLs are returned with a `#mvref=https%3A%2F%2Fwww.webtoons.com%2F`
// fragment. The cover-proxy (for in-app rendering) and the download manager
// (for saving to disk) both read this fragment and inject the Referer header
// the webtoons CDN requires.

var BASE = 'https://www.webtoons.com'
var REFERER = 'https://www.webtoons.com/'
var REFERER_FRAG = '#mvref=' + encodeURIComponent(REFERER)

var HTML_HEADERS = {
  'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36',
  'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
  'Accept-Language': 'en-US,en;q=0.9',
  'Referer': REFERER,
}

async function fetchHtml(url) {
  var resp = await __fetch(url, { headers: HTML_HEADERS })
  if (resp.status !== 200) throw new Error('WEBTOON HTTP ' + resp.status + ' for ' + url)
  return resp.text
}

function decodeEntities(s) {
  if (!s) return ''
  return s
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&apos;/g, "'")
    .replace(/&nbsp;/g, ' ')
    .replace(/&#(\d+);/g, function (_m, n) { return String.fromCharCode(parseInt(n, 10)) })
}
function stripTags(s) { return decodeEntities((s || '').replace(/<[^>]+>/g, '')).trim() }

//   source_id  = "{genre}|{slug}|{title_no}"
//   chapter_id = "{genre}|{slug}|{title_no}|{episode_no}"
function packTitleId(g, s, n) { return g + '|' + s + '|' + n }
function unpackTitleId(id) { var p = id.split('|'); return { genre: p[0], slug: p[1], titleNo: p[2] } }
function packChapterId(g, s, n, e) { return g + '|' + s + '|' + n + '|' + e }
function unpackChapterId(id) { var p = id.split('|'); return { genre: p[0], slug: p[1], titleNo: p[2], episodeNo: p[3] } }

// Walks `<a class="_card_item">` blocks. Accepts both absolute and relative
// hrefs since webtoons mixes both forms across pages.
function parseTitleCards(html) {
  var results = []
  var seen = {}
  // Capture the full <a ...> ... </a> for each card.
  var cardRe = /<a[^>]+href="((?:https?:\/\/www\.webtoons\.com)?\/en\/[^"]+\/list\?title_no=\d+)"[^>]*>([\s\S]*?)<\/a>/g
  var m
  while ((m = cardRe.exec(html)) !== null) {
    var href = m[1]
    var inner = m[2]
    var pathMatch = href.match(/\/en\/([^/]+)\/([^/]+)\/list\?title_no=(\d+)/)
    if (!pathMatch) continue
    var genre = pathMatch[1]
    var slug = pathMatch[2]
    var titleNo = pathMatch[3]
    if (seen[titleNo]) continue
    seen[titleNo] = true

    // <strong class="title">Title</strong> on the modern search page;
    // .subj/.subj_wrap on older pages — try both.
    var title = ''
    var t =
      inner.match(/<strong[^>]*class="[^"]*title[^"]*"[^>]*>([\s\S]*?)<\/strong>/) ||
      inner.match(/<p[^>]*class="[^"]*subj[^"]*"[^>]*>([\s\S]*?)<\/p>/) ||
      inner.match(/<span[^>]*class="[^"]*subj[^"]*"[^>]*>([\s\S]*?)<\/span>/) ||
      inner.match(/<h3[^>]*class="[^"]*subj[^"]*"[^>]*>([\s\S]*?)<\/h3>/)
    if (t) title = stripTags(t[1])
    if (!title) continue

    var author = null
    var a = inner.match(/<div[^>]*class="[^"]*author[^"]*"[^>]*>([\s\S]*?)<\/div>/)
    if (a) author = stripTags(a[1])

    var cover = null
    var img = inner.match(/<img[^>]+src="([^"]+)"/)
    if (img) cover = img[1]

    results.push({
      source_id: packTitleId(genre, slug, titleNo),
      title: title,
      alt_titles: [],
      cover_url: cover ? (cover + REFERER_FRAG) : null,
      author: author,
      status: null,
    })
  }
  return results
}

async function searchManga(query, page) {
  var p = page || 1
  // NOTE: do NOT pass searchType=WEBTOON — it redirects to a 3-result stub.
  var url = BASE + '/en/search?keyword=' + encodeURIComponent(query) + '&page=' + p
  var html = await fetchHtml(url)
  return parseTitleCards(html)
}

async function getPopularManga(page) {
  var url = BASE + '/en/dailySchedule'
  var html = await fetchHtml(url)
  var cards = parseTitleCards(html)
  var p = (page || 1) - 1
  return cards.slice(p * 20, (p + 1) * 20)
}

async function getLatestManga(page) {
  // The originals landing page surfaces the newest-updated batch.
  var url = BASE + '/en/originals'
  var html
  try { html = await fetchHtml(url) } catch (_e) { html = await fetchHtml(BASE + '/en/dailySchedule') }
  var cards = parseTitleCards(html)
  var p = (page || 1) - 1
  return cards.slice(p * 20, (p + 1) * 20)
}

async function getMangaDetails(mangaId) {
  var ids = unpackTitleId(mangaId)
  var url = BASE + '/en/' + ids.genre + '/' + ids.slug + '/list?title_no=' + ids.titleNo
  var html = await fetchHtml(url)

  // og:* meta tags are the most stable surface for these fields.
  var title = ''
  var cover = null
  var description = ''
  var og

  og = html.match(/<meta[^>]+property="og:title"[^>]+content="([^"]+)"/)
  if (og) title = decodeEntities(og[1])

  og = html.match(/<meta[^>]+property="og:image"[^>]+content="([^"]+)"/)
  if (og) cover = og[1]

  og = html.match(/<meta[^>]+property="og:description"[^>]+content="([^"]+)"/)
  if (og) description = decodeEntities(og[1])

  // Author lives in <div class="author_area">NAME<button.../></div> — strip
  // the button text and any surrounding whitespace.
  var author = null
  var a = html.match(/<div[^>]*class="[^"]*author_area[^"]*"[^>]*>([\s\S]*?)<\/div>/)
  if (a) {
    var raw = a[1].replace(/<button[\s\S]*?<\/button>/g, '')
    author = stripTags(raw) || null
  }

  // Genre badge (single value on webtoons).
  var genres = []
  var g = html.match(/<h2[^>]*class="[^"]*genre[^"]*"[^>]*>([\s\S]*?)<\/h2>/)
  if (g) {
    var name = stripTags(g[1])
    if (name) genres.push(name)
  }

  return {
    source_id: mangaId,
    title: title || ids.slug,
    author: author,
    artist: null,
    description: description,
    cover_url: cover ? (cover + REFERER_FRAG) : null,
    status: null,
    genres: genres,
  }
}

async function getChapterList(mangaId) {
  var ids = unpackTitleId(mangaId)
  var all = []
  var seen = {}
  var page = 1
  // Cap at 50 list pages (~500 episodes). Long-running webtoons all fit.
  while (page < 50) {
    var url = BASE + '/en/' + ids.genre + '/' + ids.slug + '/list?title_no=' + ids.titleNo + '&page=' + page
    var html
    try { html = await fetchHtml(url) } catch (_e) { break }

    var found = 0
    // Each free row: <li class="_episodeItem" id="episode_N" data-episode-no="N">
    // containing an <a href="...episode_no=N"> with nested .subj/.date spans.
    var rowRe = /<li[^>]*class="[^"]*_episodeItem[^"]*"[^>]*data-episode-no="(\d+)"[^>]*>([\s\S]*?)<\/li>/g
    var r
    while ((r = rowRe.exec(html)) !== null) {
      var epNo = r[1]
      var body = r[2]
      if (seen[epNo]) continue

      // <span class="subj"><span>title</span></span> — capture the inner text.
      var titleMatch =
        body.match(/<span[^>]*class="[^"]*subj[^"]*"[^>]*>\s*<span[^>]*>([\s\S]*?)<\/span>/) ||
        body.match(/<span[^>]*class="[^"]*subj[^"]*"[^>]*>([\s\S]*?)<\/span>/)
      var chTitle = titleMatch ? stripTags(titleMatch[1]) : ('Episode ' + epNo)
      var dateMatch = body.match(/<span[^>]*class="[^"]*date[^"]*"[^>]*>([\s\S]*?)<\/span>/)

      seen[epNo] = true
      all.push({
        source_id: packChapterId(ids.genre, ids.slug, ids.titleNo, epNo),
        number: parseFloat(epNo),
        title: chTitle,
        language: 'en',
        scanlation_group: 'WEBTOON',
        published_at: dateMatch ? stripTags(dateMatch[1]) : null,
        page_count: null,
      })
      found++
    }

    if (found === 0) break
    page++
  }

  // Webtoons lists newest-first; flip so chapter 1 comes first.
  all.sort(function (a, b) { return (a.number || 0) - (b.number || 0) })
  return all
}

async function getPageUrls(chapterId) {
  var ids = unpackChapterId(chapterId)
  // The episode slug in the URL is cosmetic — the server uses title_no+episode_no.
  var url = BASE + '/en/' + ids.genre + '/' + ids.slug + '/ep/viewer?title_no=' + ids.titleNo + '&episode_no=' + ids.episodeNo
  var html = await fetchHtml(url)
  // <img ... class="_images" data-url="https://webtoon-phinf.pstatic.net/..."> —
  // `_thumbnailImages` is a similar but distinct class for the bottom nav; the
  // exact match below excludes it.
  var urls = []
  var re = /<img[^>]+class="_images"[^>]+data-url="([^"]+)"/g
  var m
  while ((m = re.exec(html)) !== null) urls.push(m[1] + REFERER_FRAG)
  if (urls.length === 0) {
    var re2 = /<img[^>]+data-url="([^"]+)"[^>]+class="_images"(?:["\s])/g
    var m2
    while ((m2 = re2.exec(html)) !== null) urls.push(m2[1] + REFERER_FRAG)
  }
  return urls
}

registerSource({
  searchManga: searchManga,
  getPopularManga: getPopularManga,
  getLatestManga: getLatestManga,
  getMangaDetails: getMangaDetails,
  getChapterList: getChapterList,
  getPageUrls: getPageUrls,
})
