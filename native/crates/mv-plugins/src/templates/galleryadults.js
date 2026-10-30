// Gallery sites (Mihon's "galleryadults" theme: HentaiFox, IMHentai, HentaiEra, HentaiRox, ...).
// Every gallery is one title with a single chapter; pages come from the gallery's
// load_dir / load_id / g_th fields.
THEMES.galleryadults = {
  LANGS: { en: 'english', ja: 'japanese', zh: 'chinese', ko: 'korean', es: 'spanish', fr: 'french', de: 'german', ru: 'russian', pt: 'portuguese', it: 'italian' },
  EXT: { j: 'jpg', p: 'png', b: 'bmp', g: 'gif', w: 'webp' },
  langPath: function () { var l = this.LANGS[CFG.lang]; return l ? '/language/' + l + '/' : '/'; },
  cards: function (html) {
    var as = H.find(html, 'a[href*="/gallery/"]');
    var byId = {}, order = [];
    for (var i = 0; i < as.length; i++) {
      var href = H.abs(H.attr(as[i], 'href')).replace(/[?#].*$/, '');
      if (!H.isSite(href) || !/\/gallery\/\d+\/?$/.test(href)) continue;
      var id = H.toId(href);
      var e = byId[id];
      if (!e) { e = byId[id] = { href: href, title: '', cover: '' }; order.push(id); }
      var img = H.first(as[i], 'img');
      if (!e.cover && img) e.cover = H.img(img);
      var t = H.line(as[i].html) || H.attr(as[i], 'title') || H.attr(img, 'alt');
      if (t && !e.title) e.title = t;
    }
    var out = [];
    for (var j = 0; j < order.length; j++) {
      var it = byId[order[j]];
      var m = H.manga(it.href, it.title, it.cover);
      if (m) out.push(m);
    }
    return out;
  },
  // `path` page `page`: `<path>pag/<n>/`, else `<path>?page=<n>`. A missing page (some sites
  // answer 404 with a page full of random galleries) is an empty list.
  list: async function (path, page) {
    var urls = page > 1 ? [STATE.base + path + 'pag/' + page + '/', STATE.base + path + '?page=' + page] : [STATE.base + path];
    for (var i = 0; i < urls.length; i++) {
      var r = await H.request(urls[i]);
      if (r.status && r.status < 400) { var l = this.cards(r.text); if (l.length) return l; }
    }
    return [];
  },
  getLatestManga: function (page) { return this.list(this.langPath(), page || 1); },
  getPopularManga: async function (page) {
    var l = await this.list(this.langPath() + 'popular/', page || 1);
    return l.length ? l : this.getLatestManga(page);
  },
  searchManga: async function (query, page) {
    return this.cards(await H.get(STATE.base + '/search/?q=' + H.enc(query) + '&page=' + (page || 1)));
  },
  getMangaDetails: async function (id) {
    var html = await H.get(H.withSlash(H.fromId(id)));
    var h1 = H.first(html, 'h1');
    // Links to /<kind>/<name>/ only (a /tag/group/ tag is not a group).
    var names = function (kind) {
      var out = [], els = H.find(html, 'a[href*="/' + kind + '/"]');
      var re = new RegExp('^(?:https?://[^/]+)?/' + kind + '/[^/]+/?$');
      for (var i = 0; i < els.length; i++) {
        if (!re.test(H.attr(els[i], 'href'))) continue;
        var n = H.line(els[i].html).replace(/\s+\d+$/, '');
        if (n && out.indexOf(n) < 0) out.push(n);
      }
      return out;
    };
    var pages = /Pages:\s*(\d+)/i.exec(H.text(html));
    var cover = H.img(H.first(html, '.cover') || H.first(html, '.left_cover')) || H.meta(html, 'og:image');
    var artists = names('artist');
    return {
      source_id: id,
      title: h1 ? H.line(h1.html) : (H.meta(html, 'og:title') || id),
      artist: artists.join(', ') || undefined,
      author: names('group').join(', ') || artists.join(', ') || undefined,
      description: pages ? pages[1] + ' pages' : undefined,
      cover_url: cover ? H.withHint(H.abs(cover)) : undefined,
      status: 'completed',
      genres: names('tag')
    };
  },
  getChapterList: async function (id) {
    return [H.chapter(H.fromId(id), 'Gallery', null, { number: 1 })];
  },
  getPageUrls: async function (cid) {
    var url = H.withSlash(H.fromId(cid));
    var html = await H.get(url);
    var val = function (name) { var el = H.first(html, '#' + name); return H.attr(el, 'value'); };
    var dir = val('load_dir'), gid = val('load_id'), count = parseInt(val('load_pages'), 10) || 0;
    var thumbs = H.imgs(html, '.gallery_thumb img, .gthumb img, .thumb img', url);
    var server = '';
    for (var i = 0; i < thumbs.length && !server; i++) {
      var m = /^(https?:\/\/[^\/]+)\//.exec(thumbs[i]);
      if (m && (!gid || thumbs[i].indexOf('/' + gid + '/') >= 0)) server = m[1];
    }
    var th = null;
    var tm = /g_th\s*=\s*\$\.parseJSON\(\s*'([^']+)'/.exec(html) || /g_th\s*=\s*(\{[^;]+\})\s*;/.exec(html);
    if (tm) { try { th = JSON.parse(tm[1]); } catch (e) { th = null; } }
    var out = [];
    if (server && dir && gid && (th || count)) {
      var n = count || Object.keys(th).length;
      for (var p = 1; p <= n; p++) {
        var t = th && th[String(p)] ? String(th[String(p)]).split(',')[0] : 'j';
        out.push(server + '/' + dir + '/' + gid + '/' + p + '.' + (this.EXT[t] || 'jpg'));
      }
      return out;
    }
    // No page data: full images sit next to the thumbnails (`3t.jpg` -> `3.jpg`).
    for (var k = 0; k < thumbs.length; k++) if (/\/\d+t\.\w+$/.test(thumbs[k])) out.push(thumbs[k].replace(/\/(\d+)t\.(\w+)$/, '/$1.$2'));
    return out;
  },
  getTagList: async function () {
    var html = await H.get(STATE.base + '/tags/popular/');
    var as = H.find(html, 'a[href*="/tag/"]');
    var out = [], seen = {};
    for (var i = 0; i < as.length; i++) {
      var m = /\/tag\/([^\/?#]+)\/?/.exec(H.attr(as[i], 'href'));
      var name = H.line(as[i].html).replace(/\s+\d+$/, '');
      if (m && name && !seen[m[1]]) { seen[m[1]] = 1; out.push({ id: m[1], name: name }); }
    }
    return out;
  },
  getMangaByTag: function (tag, page) { return this.list('/tag/' + tag + '/', page || 1); }
};
