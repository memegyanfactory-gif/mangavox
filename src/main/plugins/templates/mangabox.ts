/**
 * MangaBox family (Mangakakalot / Manganato / Mangabat / Natomanga ...).
 * Supports both the 2025+ layout (/manga-list/hot-manga, /api/manga/{slug}/chapters,
 * `cdns` + `chapterImages` JS arrays) and the classic one (panel-story-info,
 * ul.row-content-chapter, .container-chapter-reader img).
 */
export const MANGABOX_JS = String.raw`
THEMES.mangabox = {
  CARDS: 'div.list-comic-item-wrap,div.list-truyen-item-wrap,div.content-genres-item,div.story_item,div.search-story-item,div.itemupdate,div.truyen-list div.list-truyen-item-wrap',
  parseCards: function (html) {
    var cards = H.findAny(html, this.CARDS);
    var out = [];
    for (var i = 0; i < cards.length; i++) {
      var c = cards[i];
      var tA = H.first(c, 'h3 a, h2 a, .genres-item-name, .item-title, a.story_name, .story_name a') || H.first(c, 'a[href]');
      if (!tA) continue;
      var title = H.line(tA.html) || H.attr(tA, 'title') || H.attr(H.first(c, 'img'), 'alt');
      var m = H.manga(H.attr(tA, 'href'), title, H.img(H.first(c, 'img')));
      if (m) out.push(m);
    }
    if (!out.length) out = THEMES.generic.parseCards(html);
    return out;
  },
  listing: async function (page, kind) {
    page = page || 1;
    var urls = [
      STATE.base + '/manga-list/' + kind + '-manga?page=' + page,
      STATE.base + '/genre/all?type=' + (kind === 'hot' ? 'topview' : 'latest') + '&page=' + page,
      STATE.base + '/manga_list?type=' + (kind === 'hot' ? 'topview' : 'latest') + '&category=all&state=all&page=' + page
    ];
    for (var i = 0; i < urls.length; i++) {
      try { var list = this.parseCards(await H.get(urls[i])); if (list.length) return list; } catch (e) { STATE.lastError = e; }
    }
    return [];
  },
  getPopularManga: function (page) { return this.listing(page, 'hot'); },
  getLatestManga: function (page) { return this.listing(page, 'latest'); },
  searchManga: async function (query, page) {
    page = page || 1;
    var q = String(query).toLowerCase().replace(/[^a-z0-9\s]/g, ' ').trim().replace(/\s+/g, '_');
    var urls = [STATE.base + '/search/story/' + q + '?page=' + page, STATE.base + '/search/' + q + '?page=' + page];
    for (var i = 0; i < urls.length; i++) {
      try { var list = this.parseCards(await H.get(urls[i])); if (list.length) return list; } catch (e) { STATE.lastError = e; }
    }
    return [];
  },
  getMangaDetails: async function (id) {
    var url = H.fromId(id, 'manga');
    var html = await H.get(url);
    var INFO = 'ul.manga-info-text li, table.variations-tableInfo tr, div.story-info-right-extent p, div.info-right li';
    var tEl = H.first(html, 'ul.manga-info-text h1, div.story-info-right h1, div.panel-story-info h1, h1');
    var la = H.labeled(html, INFO, /^(authors?\s*(\(s\))?)/i);
    var ls = H.labeled(html, INFO, /^status/i);
    var genres = H.names(html, 'ul.manga-info-text li.genres a, table.variations-tableInfo tr td.table-value a[href*="genre"], li.genres a');
    if (!genres.length) { var lg = H.labeled(html, INFO, /^genres?/i); if (lg) genres = H.splitList(lg.value); }
    var descEl = H.first(html, '#contentBox, #panel-story-info-description, #noidungm, div.panel-story-info-description');
    var desc = '';
    if (descEl) desc = H.text(descEl.html.replace(/<h[1-6]\b[\s\S]*?<\/h[1-6]>/i, '')).replace(/^description\s*:\s*/i, '');
    var cover = H.img(H.first(html, 'div.manga-info-pic img, span.info-image img, div.story-info-left img')) || H.meta(html, 'og:image');
    var author = la ? la.value.replace(/\s*-\s*$/, '').replace(/\s*;\s*/g, ', ') : undefined;
    return {
      source_id: id, title: tEl ? H.line(tEl.html) : (H.meta(html, 'og:title') || id),
      author: author || undefined, description: desc || undefined,
      cover_url: cover ? H.withHint(H.abs(cover)) : undefined,
      status: ls ? H.status(ls.value) : undefined, genres: genres
    };
  },
  getChapterList: async function (id) {
    var url = H.fromId(id, 'manga');
    var html = await H.get(url);
    var out = [];
    var holder = H.first(html, '#chapter-list-container, [data-api-url]');
    var slug = H.attr(holder, 'data-comic-slug') || (/\/manga\/([^\/?#]+)/.exec(url) || [])[1];
    var api = H.attr(holder, 'data-api-url') || (STATE.base + '/api/manga/__SLUG__/chapters');
    var tpl = H.attr(holder, 'data-chapter-url-template') || (STATE.base + '/manga/__MANGA__/__CHAPTER__');
    if (slug && (holder || !/row-content-chapter|chapter-list/.test(html))) {
      try {
        var d = await H.json(api.replace('__SLUG__', slug) + '?limit=-1&offset=0', { headers: { 'Referer': url } });
        var chs = (d && d.data && (d.data.chapters || d.data)) || [];
        for (var i = 0; i < chs.length; i++) {
          var ch = chs[i];
          var href = tpl.replace('__MANGA__', slug).replace('__CHAPTER__', ch.chapter_slug);
          var c = H.chapter(href, ch.chapter_name, ch.updated_at || ch.created_at, { number: ch.chapter_num != null ? parseFloat(ch.chapter_num) : null });
          if (c) out.push(c);
        }
      } catch (e) { STATE.lastError = e; }
    }
    if (!out.length) {
      var rows = H.findAny(html, 'ul.row-content-chapter li, div.chapter-list div.row, div.chapter-list li');
      for (var j = 0; j < rows.length; j++) {
        var a = H.first(rows[j], 'a[href]');
        if (!a) continue;
        var spans = H.find(rows[j], 'span');
        var dateSpan = spans.length ? spans[spans.length - 1] : null;
        var date = dateSpan ? (H.attr(dateSpan, 'title') || H.line(dateSpan.html)) : '';
        var c2 = H.chapter(H.attr(a, 'href'), H.line(a.html) || H.attr(a, 'title'), date);
        if (c2) out.push(c2);
      }
    }
    return out;
  },
  getPageUrls: async function (cid) {
    var url = H.fromId(cid);
    var html = await H.get(url, { headers: { 'Referer': STATE.base + '/' } });
    var pages = [];
    var cm = /var\s+cdns\s*=\s*(\[[^\]]*\])/.exec(html), im = /var\s+chapterImages\s*=\s*(\[[\s\S]*?\])\s*;/.exec(html);
    if (im) {
      try {
        var cdns = cm ? JSON.parse(cm[1]) : [];
        var imgs = JSON.parse(im[1]);
        var cdn = cdns.length ? String(cdns[0]) : '';
        for (var i = 0; i < imgs.length; i++) {
          var p = String(imgs[i]);
          pages.push(/^https?:/i.test(p) ? p : (cdn.replace(/\/?$/, '/') + p.replace(/^\//, '')));
        }
      } catch (e) { pages = []; }
    }
    if (!pages.length) pages = H.imgs(html, 'div.container-chapter-reader img, div.vung-doc img, div#vungdoc img', url);
    return H.uniq(pages);
  },
  getTagList: async function () {
    var html = await H.get(STATE.base + '/');
    var links = H.find(html, 'a[href*="/genre/"], a[href*="category="]');
    var out = [], seen = {};
    for (var i = 0; i < links.length; i++) {
      var href = H.attr(links[i], 'href');
      var m = /\/genre\/([\w-]+)\/?$/.exec(href.replace(/[?#].*$/, '')) || /category=(\d+)/.exec(href);
      var name = H.line(links[i].html);
      if (!m || !name || m[1] === 'all' || seen[m[1]]) continue;
      seen[m[1]] = 1;
      out.push({ id: m[1], name: name });
    }
    return out;
  },
  getMangaByTag: async function (tag, page) {
    page = page || 1;
    var url = /^\d+$/.test(tag) ? STATE.base + '/manga_list?type=topview&category=' + tag + '&state=all&page=' + page
      : STATE.base + '/genre/' + H.enc(tag) + '?type=topview&page=' + page;
    return this.parseCards(await H.get(url));
  }
};
`
