// ── dispatcher ──
var LIST_METHODS = { searchManga: 1, getPopularManga: 1, getLatestManga: 1, getMangaByTag: 1 };
var MUST_HAVE = { getPopularManga: 1, getLatestManga: 1, getChapterList: 1, getPageUrls: 1 };

function __mvLinksTo(html, host) {
  var re = new RegExp('https?://(?:www\\.)?' + host.replace(/\./g, '\\.') + '[/"\']', 'gi');
  var m = html.match(re);
  return m ? m.length : 0;
}

async function __mvProbe(path, engine) {
  try {
    var r = await H.request(STATE.base + path, { noRender: true });
    return r.status > 0 && r.status < 400 && detectEngine(r.text || '') === engine;
  } catch (e) { return false; }
}

async function __mvDetect() {
  var html = '';
  var finalUrl = '';
  try { var r = await H.request(STATE.base + '/'); html = (r.status && r.status < 400) ? (r.text || '') : ''; finalUrl = r.url || ''; } catch (e) { html = ''; }
  if (html && H.isChallenge(200, html)) html = '';
  STATE.homeHtml = html;
  // Hosts that report the post-redirect URL tell us directly about domain moves.
  var fm = /^(https?:\/\/[^\/?#]+)/i.exec(finalUrl);
  if (html && fm && H.hostOf(fm[1]) && H.hostOf(fm[1]) !== H.hostOf(STATE.base)) STATE.base = fm[1];
  // Follow silent domain moves: if the page's canonical host differs and the
  // markup links to it more than to our configured host, switch base.
  var canon = detectCanonicalBase(html);
  if (canon && H.hostOf(canon) && H.hostOf(canon) !== H.hostOf(STATE.base)) {
    if (__mvLinksTo(html, H.hostOf(canon)) > __mvLinksTo(html, H.hostOf(STATE.base))) STATE.base = canon.replace(/\/+$/, '');
  }
  var e = html ? detectEngine(html) : 'generic';
  if (e === 'generic') {
    if (await __mvProbe('/?s=&post_type=wp-manga', 'madara')) e = 'madara';
    else if (await __mvProbe('/manga/?order=popular', 'mangathemesia')) e = 'mangathemesia';
    else if (/_next\/|__NEXT_DATA__|__nuxt|"series_slug"/i.test(html) || !html) {
      try {
        var api = 'https://api.' + H.hostOf(STATE.base);
        var d = await H.json(api + '/query?page=1&perPage=1');
        if (d && (d.data || []).length && d.data[0].series_slug) { STATE.apiBase = api; e = 'heancms'; }
      } catch (e2) { /* not hean */ }
    }
  }
  STATE.engine = e;
  return e;
}

async function __mvEnsureEngine() {
  if (STATE.engine) return STATE.engine;
  if (!STATE.detecting) STATE.detecting = __mvDetect();
  try { await STATE.detecting; } finally { STATE.detecting = null; }
  if (!STATE.engine) STATE.engine = 'generic';
  return STATE.engine;
}

function __mvEmpty(method, res) {
  if (method === 'getMangaDetails') return !res || !res.title;
  if (!MUST_HAVE[method]) return false;
  return !res || (Array.isArray(res) && res.length === 0);
}

async function __mvInvoke(engine, method, args) {
  var theme = THEMES[engine] || THEMES.generic;
  var fn = theme[method] || THEMES.generic[method];
  if (!fn) return method === 'getTagList' || LIST_METHODS[method] ? [] : null;
  return await fn.apply(theme, args);
}

function __mvShape(method, res, args) {
  if (LIST_METHODS[method]) return H.cleanList(res || []);
  if (method === 'getChapterList') return H.cleanChapters(res || []);
  if (method === 'getPageUrls') {
    var out = [], seen = {};
    for (var i = 0; i < (res || []).length; i++) {
      var u = res[i];
      if (!u || typeof u !== 'string') continue;
      u = H.abs(u);
      if (!/^https?:\/\//i.test(u) || seen[u]) continue;
      seen[u] = 1;
      out.push(H.withHint(u));
    }
    return out;
  }
  if (method === 'getMangaDetails') {
    var d = res || {};
    var o = { source_id: String(args[0]), title: d.title || String(args[0]), genres: Array.isArray(d.genres) ? d.genres : [] };
    var keys = ['author', 'artist', 'description', 'cover_url', 'status', 'alt_titles'];
    for (var k = 0; k < keys.length; k++) if (d[keys[k]] != null && d[keys[k]] !== '' && !(Array.isArray(d[keys[k]]) && !d[keys[k]].length)) o[keys[k]] = d[keys[k]];
    return o;
  }
  if (method === 'getTagList') return Array.isArray(res) ? res : [];
  return res;
}

async function __mvRun(method, args) {
  var engine = await __mvEnsureEngine();
  var res = null, err = null;
  try { res = await __mvInvoke(engine, method, args); } catch (e) { err = e; }
  var blocked = err && /anti-bot/i.test(String(err && err.message));
  if ((err || __mvEmpty(method, res)) && !blocked && !STATE.retried[method]) {
    STATE.retried[method] = true;
    var tries = [];
    var before = STATE.engine;
    if (!STATE.redetected) {
      STATE.redetected = true;
      try { await __mvDetect(); } catch (e3) { STATE.engine = before; }
      if (STATE.engine !== before) tries.push(STATE.engine);
    }
    if (engine !== 'generic' && tries.indexOf('generic') < 0) tries.push('generic');
    for (var i = 0; i < tries.length; i++) {
      try {
        var r2 = await __mvInvoke(tries[i], method, args);
        if (!__mvEmpty(method, r2) && (r2 || !MUST_HAVE[method])) { res = r2; err = null; break; }
      } catch (e2) { if (!err) err = e2; }
    }
  }
  if (err && (res == null || __mvEmpty(method, res))) {
    console.warn(CFG.name + ' [' + (STATE.engine || '?') + '] ' + method + ' failed: ' + (err && err.message ? err.message : err));
    throw err;
  }
  return __mvShape(method, res, args);
}

registerSource({
  searchManga: function (query, page) { return __mvRun('searchManga', [query, page || 1]); },
  getPopularManga: function (page) { return __mvRun('getPopularManga', [page || 1]); },
  getLatestManga: function (page) { return __mvRun('getLatestManga', [page || 1]); },
  getMangaDetails: function (id) { return __mvRun('getMangaDetails', [id]); },
  getChapterList: function (id) { return __mvRun('getChapterList', [id]); },
  getPageUrls: function (id) { return __mvRun('getPageUrls', [id]); },
  getTagList: function () { return __mvRun('getTagList', []); },
  getMangaByTag: function (tag, page) { return __mvRun('getMangaByTag', [tag, page || 1]); },
  // Diagnostics (not part of MangaSourcePlugin; used by tests).
  __engine: async function () { return await __mvEnsureEngine(); },
  __state: function () { return { engine: STATE.engine, base: STATE.base, mangaDir: STATE.mangaDir, apiBase: STATE.apiBase }; }
});
