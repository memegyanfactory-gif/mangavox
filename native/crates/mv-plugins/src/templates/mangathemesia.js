THEMES.mangathemesia = {
  dir: function () { return STATE.mangaDir || 'manga'; },
  parseCards: function (html) {
    var cards = H.findAny(html, 'div.bsx,div.listupd div.bs,div.utao div.uta div.imgu,div.listo div.bs,div.bs,article.bs');
    var out = [];
    for (var i = 0; i < cards.length; i++) {
      var c = cards[i];
      var a = H.first(c, 'a[href]');
      if (!a) continue;
      var tEl = H.first(c, 'div.tt, .ntitle, .bigor .tt, h2, h3, h4');
      var title = H.attr(a, 'title') || (tEl ? H.line(tEl.html) : '') || H.attr(H.first(c, 'img'), 'alt');
      var m = H.manga(H.attr(a, 'href'), title, H.img(H.first(c, 'img')));
      if (m) out.push(m);
    }
    if (!out.length) out = THEMES.generic.parseCards(html);
    THEMES.madara.learnDir.call(this, out);
    return out;
  },
  listing: async function (page, order) {
    page = page || 1;
    var html = await H.get(STATE.base + '/' + this.dir() + '/?page=' + page + '&order=' + order);
    return this.parseCards(html);
  },
  getPopularManga: function (page) { return this.listing(page, 'popular'); },
  getLatestManga: function (page) { return this.listing(page, 'update'); },
  searchManga: async function (query, page) {
    page = page || 1;
    var list = [];
    try { list = this.parseCards(await H.get(STATE.base + (page > 1 ? '/page/' + page + '/' : '/') + '?s=' + H.enc(query))); } catch (e) { /* try filter form */ }
    if (!list.length) {
      try { list = this.parseCards(await H.get(STATE.base + '/' + this.dir() + '/?title=' + H.enc(query) + '&page=' + page + '&order=')); } catch (e2) { /* ignore */ }
    }
    return list;
  },
  INFO: 'div.imptdt, div.fmed, table.infotable tr, div.tsinfo div.imptdt, div.spe span, div.infox div.flex-wrap div, div.wd-full, li, span',
  getMangaDetails: async function (id) {
    var url = H.withSlash(H.fromId(id, this.dir()));
    var html = await H.get(url);
    var tEl = H.first(html, 'h1.entry-title, h1');
    var title = tEl ? H.line(tEl.html) : H.meta(html, 'og:title');
    var la = H.labeled(html, this.INFO, /^(authors?|autor(es)?|auteur|yazar|penulis|pengarang)\b/i);
    var lr = H.labeled(html, this.INFO, /^(artists?|artista|ilustrador|illustrator|dessinateur|art)\b/i);
    var ls = H.labeled(html, this.INFO, /^(status|estado|statut|durum)\b/i);
    var genres = H.names(html, 'div.gnr a, span.mgen a, div.mgen a, .seriestugenre a, div.wd-full .mgen a');
    if (!genres.length) { var lg = H.labeled(html, this.INFO, /^(genres?|g[eê]neros?)\b/i); if (lg) genres = H.splitList(lg.value); }
    var descEl = H.first(html, 'div.entry-content[itemprop=description], div[itemprop=description], div.synp div.entry-content, div.desc, div.summary');
    var desc = descEl ? H.text(descEl.html) : H.decode(H.meta(html, 'og:description'));
    var cover = H.img(H.first(html, 'div.thumb img, div.infomanga div[itemprop=image] img, div.thumbook img, div.sertothumb img')) || H.meta(html, 'og:image');
    var alt = H.first(html, 'span.alternative, div.wd-full span.alter, div.seriestualt');
    var clean = function (v) { v = (v || '').trim(); return v && !/^[-–]+$|^n\/?a$|^unknown$/i.test(v) ? v : undefined; };
    return {
      source_id: id, title: title || id,
      author: clean(la && la.value), artist: clean(lr && lr.value),
      description: desc || undefined, cover_url: cover ? H.withHint(H.abs(cover)) : undefined,
      status: (ls && H.status(ls.value)) || H.statusGuess(html), genres: genres,
      alt_titles: alt ? H.splitList(H.line(alt.html)) : undefined
    };
  },
  getChapterList: async function (id) {
    var url = H.withSlash(H.fromId(id, this.dir()));
    var html = await H.get(url);
    var lis = H.findAny(html, '#chapterlist li, div.bxcl li, div.cl li, div.eplister li');
    var out = [];
    for (var i = 0; i < lis.length; i++) {
      var li = lis[i];
      var a = H.first(li, 'a[href]');
      if (!a) continue;
      var tEl = H.first(li, '.chapternum, .lch a, .epl-num');
      var title = tEl ? H.line(tEl.html) : H.line(a.html);
      var dEl = H.first(li, '.chapterdate, .epl-date');
      var dn = parseFloat(H.attr(li, 'data-num'));
      var num = !isNaN(dn) ? dn : null;
      if (num == null) num = H.chapterNumber(title, H.attr(a, 'href'));
      var c = H.chapter(H.attr(a, 'href'), title, dEl ? H.line(dEl.html) : '', { number: num });
      if (c) out.push(c);
    }
    return out;
  },
  readerJson: function (html) {
    var srcs = [html];
    var re = /src=["']data:text\/javascript;base64,([A-Za-z0-9+\/=]+)["']/g, m;
    while ((m = re.exec(html)) !== null) srcs.push(H.atob(m[1]));
    for (var i = 0; i < srcs.length; i++) {
      var idx = srcs[i].indexOf('ts_reader.run(');
      if (idx < 0) continue;
      var start = srcs[i].indexOf('{', idx);
      var js = start >= 0 ? H.balancedJson(srcs[i], start) : null;
      if (!js) continue;
      try { return JSON.parse(js); } catch (e) { /* malformed */ }
    }
    return null;
  },
  getPageUrls: async function (cid) {
    var url = H.fromId(cid);
    var html = await H.get(url);
    var pages = [];
    var data = this.readerJson(html);
    if (data && data.sources && data.sources.length) {
      var imgs = data.sources[0].images || [];
      for (var i = 0; i < imgs.length; i++) if (imgs[i]) pages.push(H.abs(String(imgs[i]), url));
    }
    if (!pages.length) pages = H.imgs(html, 'div#readerarea img, div.reading-content img, div#chapter_imgs img', url);
    return H.uniq(pages);
  },
  getTagList: async function () {
    var html = await H.get(STATE.base + '/' + this.dir() + '/');
    var out = [], seen = {};
    var lis = H.find(html, 'ul.genrez li, div.genrez li');
    for (var i = 0; i < lis.length; i++) {
      var v = H.attr(H.first(lis[i], 'input'), 'value'), n = H.line(lis[i].html);
      if (v && n && !seen[v]) { seen[v] = 1; out.push({ id: v, name: n }); }
    }
    if (!out.length) {
      var links = H.find(html, 'a[href*="/genres/"], a[href*="/genre/"]');
      for (var j = 0; j < links.length; j++) {
        var m = /\/genres?\/([^\/?#]+)/.exec(H.attr(links[j], 'href'));
        var name = H.line(links[j].html).replace(/\s*\(?\d+\)?\s*$/, '');
        if (m && name && !seen[m[1]]) { seen[m[1]] = 1; out.push({ id: m[1], name: name }); }
      }
    }
    return out;
  },
  getMangaByTag: async function (tag, page) {
    page = page || 1;
    var list = [];
    try { list = this.parseCards(await H.get(STATE.base + '/' + this.dir() + '/?genre%5B%5D=' + H.enc(tag) + '&page=' + page + '&order=popular')); } catch (e) { /* fall back */ }
    if (!list.length) {
      try { list = this.parseCards(await H.get(STATE.base + '/genres/' + H.enc(tag) + '/' + (page > 1 ? 'page/' + page + '/' : ''))); } catch (e2) { /* ignore */ }
    }
    return list;
  }
};
