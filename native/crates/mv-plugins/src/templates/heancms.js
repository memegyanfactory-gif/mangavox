THEMES.heancms = {
  api: function () { return STATE.apiBase || ('https://api.' + CFG.host); },
  media: function (u) { if (!u) return ''; u = String(u); return /^https?:/i.test(u) ? u : this.api() + '/' + u.replace(/^\/+/, ''); },
  query: async function (page, orderBy, q, tagIds) {
    var url = this.api() + '/query?query_string=' + H.enc(q || '') + '&series_status=All&order=desc&orderBy=' + orderBy +
      '&series_type=Comic&page=' + (page || 1) + '&perPage=20&tags_ids=' + H.enc('[' + (tagIds || '') + ']') + '&adult=true';
    var d = await H.json(url);
    var arr = (d && (d.data || d)) || [];
    var out = [];
    for (var i = 0; i < arr.length; i++) {
      var s = arr[i];
      if (!s || !s.series_slug) continue;
      var m = H.manga(STATE.base + '/series/' + s.series_slug, s.title, this.media(s.thumbnail), { status: H.status(s.status) });
      if (m) out.push(m);
    }
    return out;
  },
  getPopularManga: function (page) { return this.query(page, 'total_views'); },
  getLatestManga: function (page) { return this.query(page, 'latest'); },
  searchManga: function (query, page) { return this.query(page, 'total_views', query); },
  slug: function (id) { var m = /series\/([^\/?#]+)/.exec(id); return m ? m[1] : String(id).replace(/^\/+|\/+$/g, ''); },
  getMangaDetails: async function (id) {
    var s = await H.json(this.api() + '/series/' + this.slug(id));
    var tags = [];
    for (var i = 0; i < (s.tags || []).length; i++) if (s.tags[i] && s.tags[i].name) tags.push(s.tags[i].name);
    return {
      source_id: id, title: s.title || id, author: s.author || undefined, artist: s.studio || undefined,
      description: s.description ? H.text(s.description) : undefined,
      cover_url: s.thumbnail ? H.withHint(this.media(s.thumbnail)) : undefined,
      status: H.status(s.status), genres: tags,
      alt_titles: s.alternative_names ? H.splitList(s.alternative_names) : undefined
    };
  },
  getChapterList: async function (id) {
    var slug = this.slug(id);
    var s = await H.json(this.api() + '/series/' + slug);
    var d = await H.json(this.api() + '/chapter/query?page=1&perPage=9999&series_id=' + s.id);
    var arr = (d && (d.data || d)) || [];
    var out = [];
    for (var i = 0; i < arr.length; i++) {
      var c = arr[i];
      if (!c || !c.chapter_slug) continue;
      if (c.price && c.price > 0) continue; // paywalled
      var title = c.chapter_name + (c.chapter_title ? ' - ' + c.chapter_title : '');
      var ch = H.chapter(STATE.base + '/series/' + slug + '/' + c.chapter_slug, title, c.created_at, { number: c.index != null && !isNaN(parseFloat(c.index)) ? parseFloat(c.index) : null });
      if (ch) out.push(ch);
    }
    return out;
  },
  getPageUrls: async function (cid) {
    var m = /series\/([^\/?#]+)\/([^\/?#]+)/.exec(cid);
    if (!m) return [];
    var d = await H.json(this.api() + '/chapter/' + m[1] + '/' + m[2]);
    var data = (d && d.chapter && d.chapter.chapter_data) || (d && d.data) || {};
    var imgs = data.images || data.files || [];
    var out = [];
    for (var i = 0; i < imgs.length; i++) { var u = typeof imgs[i] === 'string' ? imgs[i] : (imgs[i] && (imgs[i].url || imgs[i].src)); if (u) out.push(this.media(u)); }
    if (!out.length && d && d.paywall) throw new Error('This chapter is paywalled');
    return out;
  },
  getTagList: async function () {
    var d = await H.json(this.api() + '/tags');
    var arr = Array.isArray(d) ? d : (d && d.data) || [];
    var out = [];
    for (var i = 0; i < arr.length; i++) if (arr[i] && arr[i].id != null) out.push({ id: String(arr[i].id), name: arr[i].name });
    return out;
  },
  getMangaByTag: function (tag, page) { return this.query(page, 'total_views', '', tag); }
};
