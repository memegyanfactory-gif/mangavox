THEMES.keyoapp = {
  cards: function (html) { return THEMES.generic.parseCards(html); },
  getPopularManga: async function (page) { return (page || 1) > 1 ? [] : this.cards(await H.get(STATE.base + '/series/')); },
  getLatestManga: async function (page) { return this.cards(await H.get(STATE.base + '/latest/' + ((page || 1) > 1 ? '?page=' + page : ''))); },
  searchManga: async function (query, page) {
    if ((page || 1) > 1) return [];
    // The series page filters client-side, so filter here too.
    var words = String(query).toLowerCase().split(/\s+/).filter(function (w) { return w; });
    return this.cards(await H.get(STATE.base + '/series/?q=' + H.enc(query))).filter(function (m) {
      var t = m.title.toLowerCase();
      return words.every(function (w) { return t.indexOf(w) >= 0; });
    });
  },
  getMangaDetails: async function (id) {
    var url = H.withSlash(H.fromId(id, 'series'));
    var html = await H.get(url);
    var h1 = H.first(html, 'h1');
    var genres = [];
    var links = H.find(html, 'a[href*="?genre="]');
    var seen = {};
    // Genre chips of the series sit right after the title; ignore site-wide menus by keeping the last cluster.
    for (var i = 0; i < links.length; i++) {
      if (h1 && links[i].abs < h1.abs) continue;
      var g = H.line(links[i].html).replace(/[\s,;]+$/, '');
      if (g && !seen[g]) { seen[g] = 1; genres.push(g); }
    }
    var p = null;
    if (h1) { var after = html.slice(h1.end); p = H.first(after, 'p'); }
    var desc = p ? H.text(p.html) : H.decode(H.meta(html, 'og:description'));
    var st = H.labeled(html, 'div', /^status\b/i);
    var author = H.labeled(html, 'div', /^author\b/i);
    var artist = H.labeled(html, 'div', /^artist\b/i);
    var cover = H.meta(html, 'og:image');
    if (!cover) { var bg = /background-image:\s*url\(\s*['"]?([^'")]+)/i.exec(html); if (bg) cover = bg[1]; }
    var short = function (x) { return x && x.value.length < 80 ? x.value : undefined; };
    return {
      source_id: id, title: h1 ? H.line(h1.html) : (H.meta(html, 'og:title') || id),
      author: short(author), artist: short(artist), description: desc || undefined,
      cover_url: cover ? H.withHint(H.abs(cover)) : undefined,
      status: (st && H.status(st.value)) || H.statusGuess(html), genres: genres
    };
  },
  getChapterList: async function (id) {
    var url = H.withSlash(H.fromId(id, 'series'));
    var html = await H.get(url);
    var box = H.first(html, '#chapters') || { html: html };
    var as = H.find(box, 'a[href*="/chapter/"]');
    var out = [];
    for (var i = 0; i < as.length; i++) {
      var a = as[i];
      if (/Coin\.svg|lock\.svg|Upcoming/i.test(a.html)) continue;
      var title = H.attr(a, 'title') || H.attr(a, 'alt') || H.line(a.html);
      var c = H.chapter(H.attr(a, 'href'), title, H.attr(a, 'd'));
      if (c) out.push(c);
    }
    return out;
  },
  getPageUrls: async function (cid) {
    var url = H.withSlash(H.fromId(cid));
    var html = await H.get(url);
    var tm = /realUrl\s*=\s*\x60([^\x60]*?)[$][{]uid[}]/.exec(html);
    var prefix = tm ? tm[1] : ('https://cdn.' + H.hostOf(STATE.base) + '/uploads/');
    if (/^\/\//.test(prefix)) prefix = 'https:' + prefix;
    var imgs = H.find(html, '#pages img');
    var out = [];
    for (var i = 0; i < imgs.length; i++) {
      var uid = H.attr(imgs[i], 'uid');
      if (uid) out.push(prefix + uid);
      else { var u = H.img(imgs[i], url); if (u) out.push(u); }
    }
    return out;
  },
  getTagList: async function () {
    var html = await H.get(STATE.base + '/series/');
    var links = H.find(html, 'a[href*="?genre="]');
    var out = [], seen = {};
    for (var i = 0; i < links.length; i++) {
      var m = /[?&]genre=([^&#"]+)/.exec(H.attr(links[i], 'href'));
      var name = H.line(links[i].html).replace(/[\s,;]+$/, '');
      if (m && name && !seen[m[1]]) { seen[m[1]] = 1; out.push({ id: decodeURIComponent(m[1]), name: name }); }
    }
    return out;
  },
  getMangaByTag: async function (tag, page) {
    if ((page || 1) > 1) return [];
    return this.cards(await H.get(STATE.base + '/series/?genre=' + H.enc(tag)));
  }
};
