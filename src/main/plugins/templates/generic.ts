/**
 * Heuristic fallback for unknown engines: Open Graph metadata, URL-shape
 * based manga/chapter link discovery and "largest same-directory image
 * group" reader extraction. Uses __render (when the host provides it) to
 * retry JS-rendered pages that come back empty.
 */
export const GENERIC_JS = String.raw`
THEMES.generic = {
  MANGA_RE: /\/(manga|mangas|manhwa|manhua|comic|comics|series|serie|title|titles|webtoon|webtoons|book|books|obra|obras|komik|work|works|novel|read-manga|reader)\/[^\/?#]+(?:\/[^\/?#]+)?\/?$/i,
  SKIP_RE: /\/(genres?|tags?|categor(y|ies)|search|user|users|login|register|account|profile|author|artist|page|wp-|feed|contact|about|privacy|terms|dmca|faq|news|blog|groups?|team|bookmarks?|history|latest|popular|ranking|random)(\/|$|\?)/i,
  CHAPTER_RE: /(chapter|chap|capitulo|cap-|episode|episodio|\/ch[-_]?\d|\/ep[-_]?\d|\/\d+(?:[.-]\d+)?\/?$)/i,
  NAV_RE: /^(home|manga|manhwa|manhua|comics?|series|next|prev(ious)?|read( now| more)?|more|view all|see all|show all|latest|popular|new|hot|\d+|»|«|login|register|bookmark|first|last)$/i,
  parseCards: function (html) {
    var anchors = H.find(html, 'a[href]');
    var byId = {}, order = [];
    for (var i = 0; i < anchors.length; i++) {
      var a = anchors[i];
      var href = H.abs(H.attr(a, 'href'));
      if (!H.isSite(href)) continue;
      var path = href.replace(/^https?:\/\/[^\/]+/i, '').replace(/[?#].*$/, '');
      if (!this.MANGA_RE.test(path) || this.CHAPTER_RE.test(path)) continue;
      var id = H.toId(href.replace(/[?#].*$/, ''));
      var imgEl = H.first(a, 'img') || H.first(a, '[style*="url("]');
      var title = H.attr(a, 'title') || H.line(a.html) || H.attr(imgEl, 'alt');
      title = (title || '').replace(/\s+/g, ' ').trim();
      if (this.NAV_RE.test(title) || title.length > 200) title = '';
      var e = byId[id];
      if (!e) { e = byId[id] = { href: href.replace(/[?#].*$/, ''), title: '', cover: '' }; order.push(id); }
      if (title && (!e.title || (e.title.length < 3 && title.length > e.title.length))) e.title = title.replace(/\s*(chapter|ch\.?)\s*\d[\s\S]*$/i, '').trim() || title;
      if (!e.cover && imgEl) e.cover = H.img(imgEl);
    }
    var out = [];
    for (var j = 0; j < order.length; j++) {
      var it = byId[order[j]];
      if (!it.title) continue;
      var m = H.manga(it.href, it.title, it.cover);
      if (m) out.push(m);
    }
    if (out.length < 4) {
      // No keyword in manga URLs (e.g. /some-title-1234/): treat repeated
      // cover-image links (an <a> wrapping an <img>) as cards.
      var alt = [], seen = {};
      for (var k = 0; k < anchors.length; k++) {
        var ak = anchors[k];
        var img = H.first(ak, 'img');
        if (!img) continue;
        var hk = H.abs(H.attr(ak, 'href')).replace(/[?#].*$/, '');
        if (!H.isSite(hk)) continue;
        var pk = hk.replace(/^https?:\/\/[^\/]+/i, '');
        if (pk === '/' || pk === '' || this.SKIP_RE.test(pk) || this.CHAPTER_RE.test(pk) || /\.(jpe?g|png|webp|gif|php)$/i.test(pk)) continue;
        if ((pk.replace(/\/+$/, '').match(/\//g) || []).length > 2) continue;
        var tk = H.attr(ak, 'title') || H.attr(img, 'alt') || H.line(ak.html);
        if (!tk || this.NAV_RE.test(tk) || /logo/i.test(tk)) continue;
        var idk = H.toId(hk);
        if (seen[idk]) continue;
        seen[idk] = 1;
        var mk = H.manga(hk, tk, H.img(img));
        if (mk) alt.push(mk);
      }
      if (alt.length >= 4 && alt.length > out.length) out = alt;
    }
    return out;
  },
  getHtml: async function (url, wantFn) {
    var html = '', err = null;
    try { html = await H.get(url); } catch (e) { err = e; STATE.lastError = e; }
    if (html && (!wantFn || wantFn(html))) return html;
    var r = await H.render(url);
    if (!r && !html) throw err || new Error('Empty response from ' + url);
    return r || html;
  },
  LIST_PATHS: ['/', '/manga/', '/series/', '/comics/', '/manga-list/', '/browse/', '/latest/'],
  listing: async function (page) {
    page = page || 1;
    var self = this;
    if (page === 1) {
      for (var i = 0; i < this.LIST_PATHS.length; i++) {
        var html = '';
        try { html = await this.getHtml(STATE.base + this.LIST_PATHS[i], function (h) { return self.parseCards(h).length >= 4; }); } catch (e) { if (i === 0) STATE.lastError = e; continue; }
        var list = this.parseCards(html);
        if (list.length >= 4) { STATE.genericList = this.LIST_PATHS[i]; return list; }
      }
      return [];
    }
    var path = STATE.genericList && STATE.genericList !== '/' ? STATE.genericList : '/manga/';
    var urls = [STATE.base + path + '?page=' + page, STATE.base + path + 'page/' + page + '/'];
    for (var k = 0; k < urls.length; k++) {
      try { var l2 = this.parseCards(await H.get(urls[k])); if (l2.length) return l2; } catch (e) { /* next */ }
    }
    return [];
  },
  getPopularManga: function (page) { return this.listing(page); },
  getLatestManga: function (page) { return this.listing(page); },
  searchManga: async function (query, page) {
    page = page || 1;
    var q = H.enc(query);
    var urls = ['/?s=' + q + (page > 1 ? '&paged=' + page : ''), '/search?q=' + q + '&page=' + page, '/search?query=' + q + '&page=' + page,
      '/search?keyword=' + q + '&page=' + page, '/search/' + q + '?page=' + page];
    var words = String(query).toLowerCase().split(/\s+/).filter(function (w) { return w.length > 1; });
    if (!words.length) words = [String(query).toLowerCase()];
    for (var i = 0; i < urls.length; i++) {
      var html = '';
      try { html = await H.get(STATE.base + urls[i]); } catch (e) { continue; }
      var list = this.parseCards(html);
      // Only trust a search URL that actually returned matching titles (many
      // sites ignore unknown params and just render the home page).
      var hits = list.filter(function (m) { var t = m.title.toLowerCase(); return words.some(function (w) { return t.indexOf(w) >= 0; }); });
      if (hits.length) return hits;
    }
    return [];
  },
  getMangaDetails: async function (id) {
    var url = H.fromId(id);
    var html = await this.getHtml(url, function (h) { return /<h1\b|og:title/i.test(h); });
    var title = H.decode(H.meta(html, 'og:title')).replace(/\s*[|–—-]\s*[^|–—-]*$/, '').trim();
    var siteName = (H.meta(html, 'og:site_name') || '').toLowerCase();
    var hostName = H.hostOf(STATE.base).replace(/\.[a-z]+$/, '');
    var h1s = H.find(html, 'h1'), h1 = null;
    for (var hi = 0; hi < h1s.length; hi++) {
      var ht = H.line(h1s[hi].html), hl = ht.toLowerCase();
      if (!ht || hl === siteName || hl.indexOf(hostName) >= 0) continue;
      h1 = h1s[hi]; title = ht; break;
    }
    var descEl = H.first(html, '[itemprop=description], div.summary, div.description, div.synopsis, div.desc, p.description, div.manga-summary');
    var desc = descEl ? H.text(descEl.html) : H.decode(H.meta(html, 'og:description') || H.meta(html, 'description'));
    var INFO = 'li, tr, p, dd, div.info-item, div.meta, span';
    var la = H.labeled(html, INFO, /^(authors?|autor(es)?|auteur|writer|story)\b/i);
    var lr = H.labeled(html, INFO, /^(artists?|artista|illustrator|art)\b/i);
    var ls = H.labeled(html, INFO, /^(status|estado|statut)\b/i);
    // Genre links after the title only (header/footer menus list every genre).
    var gl = H.find(html, 'a[href*="/genre/"], a[href*="/genres/"], a[href*="/tag/"], a[href*="/tags/"], a[href*="genre="], a[href*="/category/"]');
    var genres = [];
    for (var gi = 0; gi < gl.length; gi++) {
      if (h1 && gl[gi].abs < h1.abs) continue;
      var gn = H.line(gl[gi].html);
      if (gn && gn.length < 40 && genres.indexOf(gn) < 0) genres.push(gn);
    }
    var cover = H.meta(html, 'og:image') || H.img(H.first(html, '[itemprop=image], div.thumb img, div.cover img, img.cover'));
    var clean = function (v) { return v && v.length < 120 ? v : undefined; };
    return {
      source_id: id, title: title || id,
      author: clean(la && la.value), artist: clean(lr && lr.value),
      description: desc || undefined, cover_url: cover ? H.withHint(H.abs(cover)) : undefined,
      status: (ls && H.status(ls.value)) || H.statusGuess(html), genres: genres.slice(0, 30)
    };
  },
  parseChapters: function (html, mangaUrl) {
    var anchors = H.find(html, 'a[href]');
    var mangaPath = mangaUrl.replace(/^https?:\/\/[^\/]+/i, '').replace(/[?#].*$/, '').replace(/\/+$/, '');
    var out = [];
    for (var i = 0; i < anchors.length; i++) {
      var a = anchors[i];
      var href = H.abs(H.attr(a, 'href'));
      if (!H.isSite(href)) continue;
      var path = href.replace(/^https?:\/\/[^\/]+/i, '').replace(/[?#].*$/, '').replace(/\/+$/, '');
      if (path === mangaPath) continue;
      var under = mangaPath && path.indexOf(mangaPath + '/') === 0;
      if (!under && !/(chapter|chap|capitulo|episode|episodio)[-_\/]?\d/i.test(path)) continue;
      var text = H.line(a.html) || H.attr(a, 'title');
      if (!text || text.length > 160) continue;
      if (!/\d/.test(text + path)) continue;
      var dEl = H.first(a, 'time, .date, .chapter-date, .chapterdate');
      var date = dEl ? (H.attr(dEl, 'datetime') || H.line(dEl.html)) : '';
      var titleEl = H.first(a, '.chapter-title, .chapternum, .title, span');
      var c = H.chapter(href, titleEl && /\d/.test(H.line(titleEl.html)) ? H.line(titleEl.html) : text, date);
      if (c) out.push(c);
    }
    return out;
  },
  getChapterList: async function (id) {
    var url = H.fromId(id);
    var self = this;
    var html = await this.getHtml(url, function (h) { return self.parseChapters(h, url).length > 0; });
    return this.parseChapters(html, url);
  },
  READER: 'div#readerarea img, div.reading-content img, div.reader-area img, div.chapter-content img, div.container-chapter-reader img, div#chapter-images img, div.chapter-images img, div.page-break img, div#reader img, div.reader img, div.chapter-reader img, section.reader img, main img',
  BAD_IMG: /logo|icon|avatar|banner|\/ads?\/|emoji|gravatar|favicon|sprite|button|badge|social|discord|patreon|kofi|ko-fi|loading|spinner/i,
  pickPages: function (html, url) {
    var pages = H.imgs(html, this.READER, url).filter(function (u) { return !THEMES.generic.BAD_IMG.test(u); });
    if (pages.length >= 2) return pages;
    var all = H.imgs(html, 'img', url).filter(function (u) { return /^https?:/i.test(u) && !THEMES.generic.BAD_IMG.test(u); });
    var groups = {}, best = null;
    for (var i = 0; i < all.length; i++) {
      var dir = all[i].replace(/[?#].*$/, '').replace(/[^\/]*$/, '');
      (groups[dir] = groups[dir] || []).push(all[i]);
      if (!best || groups[dir].length > groups[best].length) best = dir;
    }
    return best && groups[best].length >= 2 ? groups[best] : pages;
  },
  getPageUrls: async function (cid) {
    var url = H.fromId(cid);
    var self = this;
    var html = await this.getHtml(url, function (h) { return self.pickPages(h, url).length >= 2; });
    return this.pickPages(html, url);
  },
  getTagList: async function () {
    var html = await H.get(STATE.base + '/');
    var links = H.find(html, 'a[href*="/genre/"], a[href*="/genres/"], a[href*="/tag/"], a[href*="/category/"]');
    var out = [], seen = {};
    for (var i = 0; i < links.length; i++) {
      var href = H.attr(links[i], 'href');
      var name = H.line(links[i].html).replace(/\s*\(?\d+\)?\s*$/, '');
      var path = H.toId(href);
      if (!/\/(genres?|tags?|category)\/[^\/?#]+|[?&]genre=./i.test(href)) continue;
      if (!name || seen[path] || name.length > 40) continue;
      seen[path] = 1;
      out.push({ id: path, name: name });
    }
    return out;
  },
  getMangaByTag: async function (tag, page) {
    var url = H.fromId(tag);
    if ((page || 1) > 1) url = H.setQuery(url, 'page', String(page));
    return this.parseCards(await this.getHtml(url));
  }
};
`
