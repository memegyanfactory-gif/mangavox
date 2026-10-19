THEMES.foolslide = {
  root: async function () {
    if (STATE.fsRoot) return STATE.fsRoot;
    var root = STATE.base;
    try {
      var html = STATE.homeHtml || await H.get(STATE.base + '/');
      if (!/\/(directory|series|latest)\//.test(html) || !/class=["']group["']/.test(html)) {
        var links = H.find(html, 'a[href]');
        for (var i = 0; i < links.length; i++) {
          var href = H.abs(H.attr(links[i], 'href'));
          var m = /^(https?:\/\/[^?#]+?)\/(directory|series|latest|read)\//.exec(href);
          if (m && H.isSite(m[1])) { root = m[1]; break; }
          var m2 = /^(https?:\/\/[^?#]+?\/(?:reader|slide|foolslide|fs|manga))\/?$/i.exec(href);
          if (m2 && H.isSite(m2[1])) root = m2[1];
        }
      }
    } catch (e) { /* default */ }
    STATE.fsRoot = root.replace(/\/+$/, '');
    return STATE.fsRoot;
  },
  page: async function (url) {
    var html = await H.get(url);
    if (/name=["']adult["']/.test(html) && /<form\b/i.test(html) && !/class=["']group["']|class=["']element["']/.test(html)) {
      var r = await H.post(url, { adult: 'true' });
      html = H.stripComments(r.text);
    }
    return html;
  },
  parseGroups: function (html) {
    var groups = H.find(html, 'div.group');
    var out = [];
    for (var i = 0; i < groups.length; i++) {
      var g = groups[i];
      var a = H.first(g, 'div.title a') || H.first(g, 'a[href*="/series/"]');
      if (!a) continue;
      var href = H.attr(a, 'href');
      if (!/\/series\//.test(href)) { var s = H.first(g, 'a[href*="/series/"]'); if (s) { a = s; href = H.attr(s, 'href'); } }
      var cover = H.img(H.first(g, 'img.preview') || H.first(g, 'img'));
      if (/\/assets\/|glyphish|icon/i.test(cover)) cover = '';
      var m = H.manga(href, H.attr(a, 'title') || H.line(a.html), cover);
      if (m) out.push(m);
    }
    return out;
  },
  getPopularManga: async function (page) { return this.parseGroups(await this.page((await this.root()) + '/directory/' + (page || 1) + '/')); },
  getLatestManga: async function (page) { return this.parseGroups(await this.page((await this.root()) + '/latest/' + (page || 1) + '/')); },
  searchManga: async function (query, page) {
    if ((page || 1) > 1) return [];
    var r = await H.post((await this.root()) + '/search/', { search: query });
    return this.parseGroups(H.stripComments(r.text));
  },
  getMangaDetails: async function (id) {
    var url = H.withSlash(H.fromId(id));
    var html = await this.page(url);
    var info = H.first(html, 'div.info');
    var infoHtml = info ? info.html : '';
    var field = function (re) {
      var m = re.exec(infoHtml);
      if (!m) return '';
      var rest = infoHtml.slice(m.index + m[0].length);
      var end = rest.search(/<br\s*\/?>|<b>|<li|<\/li>/i);
      return H.line(end >= 0 ? rest.slice(0, end) : rest).replace(/^[:\s]+/, '').trim();
    };
    var title = H.line((H.first(html, 'h1.title') || {}).html || '') || field(/<b>\s*(title|t[ií]tulo)\s*<\/b>/i) || id;
    var desc = field(/<b>\s*(description|synopsis|sinopsis|descripci[oó]n|r[eé]sum[eé])\s*<\/b>/i);
    return {
      source_id: id, title: title,
      author: field(/<b>\s*(authors?|autor(es)?|auteur)\s*<\/b>/i) || undefined,
      artist: field(/<b>\s*(artists?|artista|dessinateur)\s*<\/b>/i) || undefined,
      description: desc || undefined,
      cover_url: (function () { var c = H.img(H.first(html, 'div.thumbnail img')); return c ? H.withHint(c) : undefined; })(),
      status: undefined, genres: []
    };
  },
  getChapterList: async function (id) {
    var url = H.withSlash(H.fromId(id));
    var html = await this.page(url);
    var els = H.find(html, 'div.list div.element, div.group div.element');
    var out = [];
    for (var i = 0; i < els.length; i++) {
      var a = H.first(els[i], 'div.title a') || H.first(els[i], 'a[href*="/read/"]');
      if (!a) continue;
      var meta = H.first(els[i], 'div.meta_r');
      var metaText = meta ? H.line(meta.html) : '';
      var groupA = meta ? H.first(meta, 'a') : null;
      var dm = /(\d{4}\.\d{1,2}\.\d{1,2})/.exec(metaText);
      var href = H.attr(a, 'href');
      var title = H.attr(a, 'title') || H.line(a.html);
      var num = null;
      var rm = /\/read\/[^\/]+\/[^\/]+\/(\d+)\/(\d+)\/(?:(\d+)\/)?/.exec(href);
      if (rm) num = parseFloat(rm[2] + (rm[3] && rm[3] !== '0' ? '.' + rm[3] : ''));
      if (!/chapter|ch\.|cap|#/i.test(title) || num == null) { var n2 = H.chapterNumber(title, null); if (num == null) num = n2; }
      var c = H.chapter(href, title, dm ? dm[1].replace(/\./g, '-') : '', { number: num, group: groupA ? H.line(groupA.html) : '' });
      if (c) out.push(c);
    }
    return out;
  },
  getPageUrls: async function (cid) {
    var url = H.withSlash(H.fromId(cid));
    var html = await this.page(url.replace(/\/page\/\d+\/?$/, '/') + 'page/1');
    var pages = [];
    var arr = null;
    var b = /var\s+pages\s*=\s*JSON\.parse\(\s*atob\(\s*["']([A-Za-z0-9+\/=]+)["']/.exec(html);
    if (b) { try { arr = JSON.parse(H.atob(b[1])); } catch (e) { arr = null; } }
    if (!arr) {
      var idx = html.search(/var\s+pages\s*=\s*\[/);
      if (idx >= 0) { var js = H.balancedJson(html, html.indexOf('[', idx)); try { arr = JSON.parse(js); } catch (e2) { arr = null; } }
    }
    if (arr) for (var i = 0; i < arr.length; i++) { var u = arr[i] && (arr[i].url || arr[i].src); if (u) pages.push(H.abs(u, url)); }
    if (!pages.length) pages = H.imgs(html, 'div.inner img, #page img, img.open', url);
    return H.uniq(pages);
  }
};
