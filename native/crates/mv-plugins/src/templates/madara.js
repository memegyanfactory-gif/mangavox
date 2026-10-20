THEMES.madara = {
  dir: function () { return STATE.mangaDir || 'manga'; },
  parseCards: function (html) {
    var cards = H.findAny(html, 'div.page-item-detail,div.c-tabs-item__content,div.manga__item,div.manga-item');
    var out = [];
    for (var i = 0; i < cards.length; i++) {
      var c = cards[i];
      var a = H.first(c, '.post-title a, .manga-title a, h3 a, h4 a, .item-summary a, .manga__content a, a[title]') || H.first(c, 'a');
      if (!a) continue;
      var href = H.attr(a, 'href');
      var title = H.line(a.html) || H.attr(a, 'title');
      if (!title) { var im = H.first(c, 'img'); title = H.attr(im, 'alt') || H.attr(im, 'title'); }
      var m = H.manga(href, title, H.img(H.first(c, 'img')));
      if (m) out.push(m);
    }
    // Heavily customised child themes: fall back to URL-shape card discovery.
    if (!out.length) out = THEMES.generic.parseCards(html);
    this.learnDir(out);
    return out;
  },
  learnDir: function (list) {
    if (STATE.mangaDir || !list.length) return;
    var counts = {};
    for (var i = 0; i < list.length; i++) {
      var seg = /^([^\/?#]+)\/[^\/?#]+\/?$/.exec(list[i].source_id);
      if (seg) counts[seg[1]] = (counts[seg[1]] || 0) + 1;
    }
    var best = null, n = 0;
    for (var k in counts) if (counts[k] > n) { n = counts[k]; best = k; }
    if (best && n >= Math.max(2, list.length / 2)) STATE.mangaDir = best;
  },
  loadMore: async function (page, metaKey, extraVars) {
    var body = {
      action: 'madara_load_more', page: String(page - 1),
      template: 'madara-core/content/content-archive',
      'vars[paged]': '1', 'vars[orderby]': 'meta_value_num', 'vars[template]': 'archive',
      'vars[sidebar]': 'full', 'vars[post_type]': 'wp-manga', 'vars[post_status]': 'publish',
      'vars[meta_key]': metaKey, 'vars[order]': 'desc', 'vars[meta_query][relation]': 'AND',
      'vars[manga_archives_item_layout]': 'big_thumbnail'
    };
    if (extraVars) for (var k in extraVars) body[k] = extraVars[k];
    var r = await H.post(STATE.base + '/wp-admin/admin-ajax.php', body);
    return this.parseCards(r.text);
  },
  listing: async function (page, order, metaKey) {
    page = page || 1;
    var url = STATE.base + '/' + this.dir() + '/' + (page > 1 ? 'page/' + page + '/' : '') + '?m_orderby=' + order;
    var list = [];
    try { list = this.parseCards(await H.get(url)); } catch (e) { if (page === 1) STATE.lastError = e; }
    if (!list.length) { try { list = await this.loadMore(page, metaKey); } catch (e2) { /* ignore */ } }
    return list;
  },
  getPopularManga: function (page) { return this.listing(page, 'views', '_wp_manga_views'); },
  getLatestManga: function (page) { return this.listing(page, 'latest', '_latest_update'); },
  searchManga: async function (query, page) {
    page = page || 1;
    var url = STATE.base + '/' + (page > 1 ? 'page/' + page + '/' : '') + '?s=' + H.enc(query) + '&post_type=wp-manga';
    var list = this.parseCards(await H.get(url));
    if (!list.length && page === 1) {
      // Some installs only answer the live-search ajax endpoint.
      try {
        var r = await H.post(STATE.base + '/wp-admin/admin-ajax.php', { action: 'wp-manga-search-manga', title: query });
        var d = typeof r.data === 'object' && r.data ? r.data : JSON.parse(r.text);
        var arr = (d && d.data) || [];
        for (var i = 0; i < arr.length; i++) { var m = H.manga(arr[i].url, arr[i].title); if (m) list.push(m); }
      } catch (e) { /* ignore */ }
    }
    return list;
  },
  getMangaDetails: async function (id) {
    var url = H.withSlash(H.fromId(id, this.dir()));
    var html = await H.get(url);
    var tEl = H.first(html, 'div.post-title h1, div.post-title h3, #manga-title h1, div.post-title, h1.entry-title, h1');
    var titleHtml = tEl ? tEl.html.replace(/<span\b[^>]*class=["'][^"']*manga-title-badges[\s\S]*?<\/span>/gi, '') : '';
    var title = H.line(titleHtml) || H.meta(html, 'og:title').replace(/\s*[\-–|].*$/, '');
    var authors = H.names(html, 'div.author-content a');
    var artists = H.names(html, 'div.artist-content a');
    var INFO = 'div.post-content_item, div.post-status div.post-content_item, .manga-info-row, li';
    if (!authors.length) { var la = H.labeled(html, INFO, /^(authors?|autor(es)?|auteur)\b/i); if (la) authors = [la.value]; }
    if (!artists.length) { var lr = H.labeled(html, INFO, /^(artists?|artista|dessinateur)\b/i); if (lr) artists = [lr.value]; }
    var descEl = H.first(html, 'div.description-summary div.summary__content, div.summary__content, div.manga-excerpt, div.post-content div.manga-summary, div.post-content div.desc, div.c-page__content div.summary__content, div.manga-about, div.description-summary');
    var desc = descEl ? H.text(descEl.html) : '';
    if (!desc) desc = H.decode(H.meta(html, 'og:description') || H.meta(html, 'description'));
    var cover = H.img(H.first(html, 'div.summary_image img, div.summary_image, .tab-summary img')) || H.meta(html, 'og:image');
    var st = H.labeled(html, INFO, /^(status|estado|statut|durum)\b/i);
    var status = st ? H.status(st.value) : undefined;
    if (!status) status = H.statusGuess(html);
    var genres = H.names(html, 'div.genres-content a');
    if (!genres.length) { var lg = H.labeled(html, INFO, /^(genres?|g[eê]neros?|tags?)\b/i); if (lg) genres = H.splitList(lg.value); }
    if (!genres.length) genres = H.wpTerms(html, 'wp-manga-genre');
    var alt = H.labeled(html, INFO, /^(alternative|alt\.?\s*names?|other names?|nombres? alternativos?)\b/i);
    return {
      source_id: id, title: title || id,
      author: authors.join(', ') || undefined, artist: artists.join(', ') || undefined,
      description: desc || undefined, cover_url: cover ? H.withHint(H.abs(cover)) : undefined,
      status: status, genres: genres,
      alt_titles: alt ? H.splitList(alt.value) : undefined
    };
  },
  parseChapters: function (html) {
    var lis = H.findAny(html, 'li.wp-manga-chapter, div.listing-chapters_wrap li, ul.version-chap li, ul.main.version-chap li, div.chapter-list li');
    var out = [];
    for (var i = 0; i < lis.length; i++) {
      var li = lis[i];
      var a = H.first(li, 'a');
      if (!a || !H.attr(a, 'href') || H.attr(a, 'href') === '#') continue;
      var dEl = H.first(li, 'span.chapter-release-date i, span.chapter-release-date, .chapter-release-date, span.date');
      var date = '';
      if (dEl) { date = H.line(dEl.html); if (!date) { var na = H.first(dEl, 'a'); date = H.attr(na, 'title'); } }
      var titleEl = H.first(a, '.chapter-title, .chapternum') || a;
      var c = H.chapter(H.attr(a, 'href'), H.line(titleEl.html) || H.attr(a, 'title'), date);
      if (c) out.push(c);
    }
    return out;
  },
  getChapterList: async function (id) {
    var url = H.withSlash(H.fromId(id, this.dir()));
    var html = await H.get(url);
    var list = this.parseChapters(html);
    if (!list.length) {
      try {
        var r = await H.post(url.replace(/\?.*$/, '') + 'ajax/chapters/', '', { 'Referer': url });
        list = this.parseChapters(r.text);
      } catch (e) { /* older installs */ }
    }
    if (!list.length) {
      var holder = H.first(html, '#manga-chapters-holder, div[data-id]');
      var mid = H.attr(holder, 'data-id');
      if (!mid) { var mm = /["']?manga_id["']?\s*[:=]\s*["']?(\d+)/.exec(html) || /[?&]p=(\d+)["']/.exec(html) || /postid-(\d+)/.exec(html); if (mm) mid = mm[1]; }
      if (mid) {
        try {
          var r2 = await H.post(STATE.base + '/wp-admin/admin-ajax.php', { action: 'manga_get_chapters', manga: mid }, { 'Referer': url });
          list = this.parseChapters(r2.text);
        } catch (e2) { /* ignore */ }
      }
    }
    return list;
  },
  getPageUrls: async function (cid) {
    var url = H.setQuery(H.fromId(cid), 'style', 'list');
    var html = await H.get(url);
    var pages = H.imgs(html, 'div.page-break img');
    if (!pages.length) pages = H.imgs(html, 'li.blocks-gallery-item img, div.reading-content img, div.read-container img, div.text-left img');
    if (!pages.length) {
      var pm = /chapter_preloaded_images\s*=\s*(\[[\s\S]*?\])/.exec(html);
      if (pm) { try { var arr = JSON.parse(pm[1]); for (var i = 0; i < arr.length; i++) pages.push(H.abs(typeof arr[i] === 'string' ? arr[i] : arr[i].src)); } catch (e) { /* ignore */ } }
    }
    if (!pages.length && /chapter_data|wpmangaprotectornonce|chapter-protector/i.test(html)) {
      throw new Error('This chapter is protected (encrypted reader) and is not supported yet');
    }
    return pages;
  },
  getTagList: async function () {
    var out = [], seen = {}, html = '';
    try {
      html = await H.get(STATE.base + '/?s=&post_type=wp-manga');
      var boxes = H.find(html, 'div.checkbox-group div.checkbox');
      for (var i = 0; i < boxes.length; i++) {
        var input = H.first(boxes[i], 'input');
        var v = H.attr(input, 'value'), n = H.line(boxes[i].html);
        if (v && n && !seen[v]) { seen[v] = 1; out.push({ id: v, name: n }); }
      }
    } catch (e) { /* fall through */ }
    var pages = [html];
    for (var p = 0; p < 2 && !out.length; p++) {
      if (p === 1) { try { pages[1] = STATE.homeHtml || await H.get(STATE.base + '/'); } catch (e2) { break; } }
      var links = H.find(pages[p], 'a[href*="-genre/"], a[href*="/genre/"], a[href*="/genres/"]');
      for (var j = 0; j < links.length; j++) {
        var m = /\/(?:[\w-]*genres?)\/([^\/?#]+)/.exec(H.attr(links[j], 'href'));
        var name = H.line(links[j].html).replace(/\s*\(?\d+\)?\s*$/, '');
        if (m && name && !seen[m[1]]) { seen[m[1]] = 1; out.push({ id: m[1], name: name }); }
      }
    }
    return out;
  },
  getMangaByTag: async function (tag, page) {
    page = page || 1;
    var list = [];
    try {
      list = this.parseCards(await H.get(STATE.base + '/manga-genre/' + H.enc(tag) + '/' + (page > 1 ? 'page/' + page + '/' : '') + '?m_orderby=views'));
    } catch (e) { /* try search filter */ }
    if (!list.length) {
      list = this.parseCards(await H.get(STATE.base + '/' + (page > 1 ? 'page/' + page + '/' : '') + '?s=&post_type=wp-manga&genre%5B%5D=' + H.enc(tag) + '&op=&author=&artist=&release=&adult=&m_orderby=views'));
    }
    return list;
  }
};
