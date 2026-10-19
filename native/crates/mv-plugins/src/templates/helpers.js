// ───────────────────────── MangaVox template runtime ─────────────────────────
var UA = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36';
var H = {};

// ── text ──
H.ENTITIES = {
  amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ', hellip: '\u2026', ndash: '\u2013',
  mdash: '\u2014', lsquo: '\u2018', rsquo: '\u2019', ldquo: '\u201c', rdquo: '\u201d', laquo: '\u00ab',
  raquo: '\u00bb', copy: '\u00a9', reg: '\u00ae', trade: '\u2122', middot: '\u00b7', bull: '\u2022',
  eacute: '\u00e9', egrave: '\u00e8', ecirc: '\u00ea', aacute: '\u00e1', agrave: '\u00e0', acirc: '\u00e2',
  iacute: '\u00ed', oacute: '\u00f3', ocirc: '\u00f4', uacute: '\u00fa', ntilde: '\u00f1', ccedil: '\u00e7',
  uuml: '\u00fc', ouml: '\u00f6', auml: '\u00e4', szlig: '\u00df', atilde: '\u00e3', otilde: '\u00f5',
  times: '\u00d7', deg: '\u00b0', shy: '', zwnj: '', zwj: '', lrm: '', rlm: '', ensp: ' ', emsp: ' ', thinsp: ' '
};
H.decode = function (s) {
  if (s == null) return '';
  s = String(s);
  if (s.indexOf('&') < 0) return s;
  return s.replace(/&(#[xX][0-9a-fA-F]+|#\d+|[a-zA-Z][a-zA-Z0-9]*);?/g, function (m, e) {
    if (e.charAt(0) === '#') {
      var code = (e.charAt(1) === 'x' || e.charAt(1) === 'X') ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
      if (!isNaN(code) && code > 0 && code < 0x110000) { try { return String.fromCodePoint(code); } catch (x) { return m; } }
      return m;
    }
    var v = H.ENTITIES[e] !== undefined ? H.ENTITIES[e] : H.ENTITIES[e.toLowerCase()];
    return v === undefined ? m : v;
  });
};
H.text = function (html) {
  if (html == null) return '';
  var s = String(html)
    .replace(/<script\b[\s\S]*?<\/script\s*>/gi, ' ')
    .replace(/<style\b[\s\S]*?<\/style\s*>/gi, ' ')
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<\/(p|div|li|h[1-6]|tr|blockquote)\s*>/gi, '\n')
    .replace(/<[^>]*>/g, ' ');
  s = H.decode(s).replace(/\u00a0/g, ' ');
  return s.replace(/[ \t\r\f\v]+/g, ' ').replace(/ *\n */g, '\n').replace(/\n{3,}/g, '\n\n').trim();
};
H.line = function (html) { return H.text(html).replace(/\s+/g, ' ').trim(); };
H.elText = function (el) { return el ? H.line(el.html) : ''; };

// ── tag / attribute parsing ──
H.VOID = { area: 1, base: 1, br: 1, col: 1, embed: 1, hr: 1, img: 1, input: 1, link: 1, meta: 1, param: 1, source: 1, track: 1, wbr: 1 };
H.RAW = { script: 1, style: 1, textarea: 1, template: 1 };
H.attrs = function (s) {
  var out = {};
  if (!s) return out;
  var re = /([^\s=\/"'<>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g, m;
  while ((m = re.exec(s)) !== null) {
    var k = m[1].toLowerCase();
    if (out[k] !== undefined) continue;
    var v = m[2] !== undefined ? m[2] : (m[3] !== undefined ? m[3] : (m[4] !== undefined ? m[4] : ''));
    out[k] = H.decode(v);
  }
  return out;
};
H.stripComments = function (html) { return String(html || '').replace(/<!--[\s\S]*?-->/g, ''); };

// Find the end of the element whose open tag ends at 'from'. Returns [innerEnd, outerEnd].
H._close = function (html, tag, from) {
  var re = new RegExp('<(\\/?)' + tag + '(?=[\\s>\\/])(?:[^>"\']|"[^"]*"|\'[^\']*\')*>', 'gi');
  re.lastIndex = from;
  var depth = 1, m;
  while ((m = re.exec(html)) !== null) {
    if (m[1]) { depth--; if (depth === 0) return [m.index, m.index + m[0].length]; }
    else if (!/\/>$/.test(m[0])) depth++;
  }
  // Unclosed (e.g. optional </li>) — end at the next sibling open of same tag, or the end.
  var re2 = new RegExp('<' + tag + '(?=[\\s>\\/])', 'gi');
  re2.lastIndex = from;
  var n = re2.exec(html);
  var end = n ? n.index : html.length;
  return [end, end];
};

// Iterate every opening tag in html (skipping raw-text element bodies).
H._each = function (html, cb) {
  var re = /<([a-zA-Z][a-zA-Z0-9:-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>/g, m;
  while ((m = re.exec(html)) !== null) {
    var tag = m[1].toLowerCase();
    var stop = cb(tag, m[2], m.index, m.index + m[0].length, m[0]);
    if (stop === true) return;
    if (H.RAW[tag] && !/\/\s*$/.test(m[2])) {
      var endRe = new RegExp('<\\/' + tag, 'gi');
      endRe.lastIndex = re.lastIndex;
      var em = endRe.exec(html);
      if (!em) return;
      re.lastIndex = em.index;
    }
  }
};

H._mkEl = function (html, tag, attrs, attrStr, start, openEnd) {
  var selfClose = H.VOID[tag] || /\/\s*$/.test(attrStr);
  var inner = '', outerEnd = openEnd;
  if (!selfClose) {
    var c = H._close(html, tag, openEnd);
    inner = html.slice(openEnd, c[0]);
    outerEnd = c[1];
  }
  return { tag: tag, attrs: attrs, html: inner, outer: html.slice(start, outerEnd), start: start, openEnd: openEnd, end: outerEnd };
};

// ── selectors ──
H._selCache = {};
H._parseSel = function (sel) {
  if (H._selCache[sel]) return H._selCache[sel];
  var groups = [];
  var parts = String(sel).split(',');
  for (var i = 0; i < parts.length; i++) {
    var p = parts[i].replace(/\s*>\s*/g, ' ').trim();
    if (!p) continue;
    var steps = [];
    var toks = p.match(/(?:[^\s\[]+|\[[^\]]*\])+/g) || [];
    for (var j = 0; j < toks.length; j++) {
      var t = toks[j], st = { tag: null, id: null, cls: [], at: [] }, m;
      var tm = /^([a-zA-Z][a-zA-Z0-9-]*|\*)/.exec(t);
      if (tm) { st.tag = tm[1] === '*' ? null : tm[1].toLowerCase(); t = t.slice(tm[0].length); }
      var re = /(\.[\w-]+)|(#[\w-]+)|\[\s*([\w:-]+)\s*(?:([*^$~|]?=)\s*(?:"([^"]*)"|'([^']*)'|([^\]\s]*)))?\s*\]/g;
      while ((m = re.exec(t)) !== null) {
        if (m[1]) st.cls.push(m[1].slice(1));
        else if (m[2]) st.id = m[2].slice(1);
        else st.at.push({ n: m[3].toLowerCase(), op: m[4] || null, v: m[5] !== undefined ? m[5] : (m[6] !== undefined ? m[6] : m[7]) });
      }
      steps.push(st);
    }
    if (steps.length) groups.push(steps);
  }
  H._selCache[sel] = groups;
  return groups;
};
H._matchAttrs = function (st, attrs) {
  if (st.id && attrs.id !== st.id) return false;
  if (st.cls.length) {
    var c = ' ' + String(attrs['class'] || '').replace(/\s+/g, ' ') + ' ';
    for (var i = 0; i < st.cls.length; i++) if (c.indexOf(' ' + st.cls[i] + ' ') < 0) return false;
  }
  for (var j = 0; j < st.at.length; j++) {
    var a = st.at[j], v = attrs[a.n];
    if (v === undefined) return false;
    if (!a.op) continue;
    var want = a.v || '';
    if (a.op === '=' && v !== want) return false;
    if (a.op === '*=' && v.indexOf(want) < 0) return false;
    if (a.op === '^=' && v.indexOf(want) !== 0) return false;
    if (a.op === '$=' && v.slice(-want.length) !== want) return false;
    if (a.op === '~=' && (' ' + v + ' ').indexOf(' ' + want + ' ') < 0) return false;
    if (a.op === '|=' && v !== want && v.indexOf(want + '-') !== 0) return false;
  }
  return true;
};
H._findStep = function (html, st, out) {
  H._each(html, function (tag, attrStr, start, openEnd) {
    if (st.tag && st.tag !== tag) return;
    if (st.id && attrStr.indexOf(st.id) < 0) return;
    if (st.cls.length && attrStr.indexOf(st.cls[0]) < 0) return;
    var attrs = H.attrs(attrStr);
    if (!H._matchAttrs(st, attrs)) return;
    out.push(H._mkEl(html, tag, attrs, attrStr, start, openEnd));
  });
};
H._root = function (x) { return x == null ? '' : (typeof x === 'string' ? x : (x.html || '')); };
/** All elements matching selector inside x (html string or element). */
H.find = function (x, sel) {
  var root = H._root(x);
  if (!root) return [];
  var groups = H._parseSel(sel), all = [], seen = {};
  for (var g = 0; g < groups.length; g++) {
    var ctx = [{ html: root, innerAbs: 0 }];
    var steps = groups[g];
    for (var s = 0; s < steps.length && ctx.length; s++) {
      var next = [], nseen = {};
      for (var c = 0; c < ctx.length; c++) {
        var found = [];
        H._findStep(ctx[c].html, steps[s], found);
        for (var f = 0; f < found.length; f++) {
          // absolute offsets inside the root, for dedupe/ordering
          var el = found[f];
          el.abs = ctx[c].innerAbs + el.start;
          el.innerAbs = ctx[c].innerAbs + el.openEnd;
          if (nseen[el.abs]) continue;
          nseen[el.abs] = 1;
          next.push(el);
        }
      }
      ctx = next;
    }
    for (var k = 0; k < ctx.length; k++) {
      if (seen[ctx[k].abs]) continue;
      seen[ctx[k].abs] = 1;
      all.push(ctx[k]);
    }
  }
  if (groups.length > 1) all.sort(function (a, b) { return a.abs - b.abs; });
  return all;
};
/** First element matching (comma-separated alternatives are tried in order). */
H.first = function (x, sel) {
  var parts = String(sel).split(',');
  for (var i = 0; i < parts.length; i++) {
    var r = H.find(x, parts[i]);
    if (r.length) return r[0];
  }
  return null;
};
/** First non-empty list among comma-separated alternatives (keeps preference order). */
H.findAny = function (x, sel) {
  var parts = String(sel).split(',');
  for (var i = 0; i < parts.length; i++) {
    var r = H.find(x, parts[i]);
    if (r.length) return r;
  }
  return [];
};
H.attr = function (el, name) { return el && el.attrs ? (el.attrs[name] || '') : ''; };
H.meta = function (html, prop) {
  var re = /<meta\b((?:[^>"']|"[^"]*"|'[^']*')*)>/gi, m;
  while ((m = re.exec(html)) !== null) {
    var a = H.attrs(m[1]);
    if (a.property === prop || a.name === prop || a.itemprop === prop) return (a.content || '').trim();
  }
  return '';
};

// ── URLs ──
H.abs = function (u, base) {
  if (u == null) return '';
  u = H.decode(String(u)).replace(/[\r\n\t]/g, '').trim();
  if (!u) return '';
  if (/^(javascript|mailto|tel|data):/i.test(u)) return u;
  if (u.indexOf('//') === 0) u = 'https:' + u;
  try { return new URL(u, base || (CFG.base + '/')).href; } catch (e) { return u; }
};
H.hostOf = function (u) { var m = /^https?:\/\/([^\/?#:]+)/i.exec(u || ''); return m ? m[1].toLowerCase().replace(/^www\./, '') : ''; };
H.isSite = function (u) {
  var h = H.hostOf(u);
  if (!h) return false;
  return h === CFG.host || h === H.hostOf(STATE.base);
};
/** Stable id for a site URL: path relative to the site root (survives domain changes). */
H.toId = function (url) {
  var u = H.abs(url, STATE.base + '/');
  if (!H.isSite(u)) return u.replace(/#.*$/, '');
  var m = /^https?:\/\/[^\/?#]+([^#]*)/i.exec(u);
  var p = (m ? m[1] : '').replace(/^\/+/, '');
  return p || '/';
};
/** Resolve an id produced by toId (or a legacy bare slug / absolute URL). */
H.fromId = function (id, slugPrefix) {
  id = String(id || '').trim();
  if (/^https?:\/\//i.test(id)) {
    // Legacy absolute URL on an old domain → rewrite to current base.
    if (H.hostOf(id) === CFG.host && H.hostOf(STATE.base) !== CFG.host) return STATE.base + id.replace(/^https?:\/\/[^\/?#]+/i, '');
    return id;
  }
  if (id.indexOf('/') < 0 && id.indexOf('?') < 0 && !slugPrefix && CFG.legacyIds) slugPrefix = 'manga';
  if (id.indexOf('/') < 0 && id.indexOf('?') < 0 && slugPrefix) return STATE.base + '/' + slugPrefix + '/' + id + '/';
  return STATE.base + '/' + id.replace(/^\/+/, '');
};
H.withSlash = function (url) { var q = url.indexOf('?'); var p = q < 0 ? url : url.slice(0, q); var r = q < 0 ? '' : url.slice(q); return (/\/$/.test(p) ? p : p + '/') + r; };
H.setQuery = function (url, k, v) {
  var hash = '', hi = url.indexOf('#');
  if (hi >= 0) { hash = url.slice(hi); url = url.slice(0, hi); }
  var re = new RegExp('([?&])' + k + '=[^&]*');
  if (re.test(url)) return url.replace(re, '$1' + k + '=' + encodeURIComponent(v)) + hash;
  return url + (url.indexOf('?') < 0 ? '?' : '&') + k + '=' + encodeURIComponent(v) + hash;
};
H.enc = function (s) { return encodeURIComponent(String(s == null ? '' : s)); };
H.form = function (obj) {
  var out = [];
  for (var k in obj) {
    if (!Object.prototype.hasOwnProperty.call(obj, k)) continue;
    var v = obj[k];
    if (Array.isArray(v)) for (var i = 0; i < v.length; i++) out.push(H.enc(k) + '=' + H.enc(v[i]));
    else out.push(H.enc(k) + '=' + H.enc(v));
  }
  return out.join('&');
};
H.refHint = function () { return 'mvref=' + encodeURIComponent(STATE.base + '/'); };
H.withHint = function (u) {
  if (!u || /^data:/i.test(u)) return u;
  if (u.indexOf('#') >= 0) return u.indexOf('mvref=') >= 0 ? u : u + '&' + H.refHint();
  return u + '#' + H.refHint();
};

// ── images ──
H.PLACEHOLDER = /(?:^|\/)(?:[\w-]*[-_])?(?:blank|placeholder|lazy|loading|loader|spacer|pixel|1x1|transparent|dflazy|grey|gray)[\w.-]*\.(?:gif|png|svg|jpe?g|webp)(?:[?#]|$)|\.svg(?:[?#]|$)/i;
H.bestSrcset = function (s) {
  if (!s) return '';
  var best = '', bestW = -1;
  var parts = String(s).split(/,\s+(?=\S)/);
  for (var i = 0; i < parts.length; i++) {
    var bits = parts[i].trim().split(/\s+/);
    if (!bits[0]) continue;
    var w = 1, d = bits[1] || '';
    if (/w$/i.test(d)) w = parseFloat(d); else if (/x$/i.test(d)) w = parseFloat(d) * 1000;
    if (isNaN(w)) w = 1;
    if (w > bestW) { bestW = w; best = bits[0]; }
  }
  return best;
};
/** Best image URL for an <img> (or the first <img> inside a container). */
H.img = function (el, base) {
  if (!el) return '';
  if (el.tag !== 'img' && el.tag !== 'source') {
    var inner = H.first(el, 'img');
    if (!inner) {
      var bgEl = /url\(/i.test(H.attr(el, 'style')) ? el : H.first(el, '[style*="url("]');
      var bm = bgEl ? /url\(\s*['"]?([^'")]+)/i.exec(H.attr(bgEl, 'style')) : null;
      return bm && !H.PLACEHOLDER.test(bm[1]) ? H.abs(bm[1], base) : '';
    }
    el = inner;
  }
  var a = el.attrs || {};
  var cands = [a['data-src'], a['data-lazy-src'], a['data-original'], a['data-cfsrc'], a['data-url'], a['data-lazy'],
    a['data-echo'], a['data-orig-file'], a['data-wpfc-original-src'], H.bestSrcset(a['data-srcset'] || a['data-lazy-srcset']), H.bestSrcset(a.srcset), a.src];
  for (var i = 0; i < cands.length; i++) {
    var c = cands[i];
    if (!c) continue;
    c = String(c).replace(/[\r\n\t]/g, '').trim();
    if (!c || /^data:/i.test(c) || H.PLACEHOLDER.test(c)) continue;
    return H.abs(c, base);
  }
  return '';
};
H.imgs = function (x, sel, base) {
  var els = H.find(x, sel || 'img'), out = [], seen = {};
  for (var i = 0; i < els.length; i++) {
    var u = H.img(els[i], base);
    if (!u || seen[u]) continue;
    seen[u] = 1;
    out.push(u);
  }
  return out;
};
H.uniq = function (arr) { var seen = {}, out = []; for (var i = 0; i < arr.length; i++) { if (!arr[i] || seen[arr[i]]) continue; seen[arr[i]] = 1; out.push(arr[i]); } return out; };

// ── numbers / dates / status ──
H.chapterNumber = function (title, url) {
  var t = H.line(title || '');
  var m = /(?:chapter|chapitre|cap[i\u00ed]tulo|kapitel|bab|chap|ch|episode|episodio|ep|cap|#)\s*\.?\s*[:\-]?\s*(\d+(?:\.\d+|,\d{1,2}(?!\d))?)/i.exec(t);
  if (m) return parseFloat(m[1].replace(',', '.'));
  if (url) {
    var path = String(url).replace(/[?#].*$/, '').replace(/\/+$/, '');
    var seg = path.slice(path.lastIndexOf('/') + 1);
    m = /(?:chapter|chap|ch|episode|ep|capitulo|cap|bab)[-_ .]?(\d+)(?:[-_.](\d{1,2})(?!\d))?/i.exec(seg);
    if (m) return parseFloat(m[1] + (m[2] ? '.' + m[2] : ''));
  }
  m = /^\s*(\d+(?:\.\d+)?)\s*(?:$|[:\-\u2013\u2014.]\s)/.exec(t);
  if (m) return parseFloat(m[1]);
  if (url) {
    var p2 = String(url).replace(/[?#].*$/, '').replace(/\/+$/, '');
    var last = p2.slice(p2.lastIndexOf('/') + 1);
    m = /^(\d+(?:[.-]\d{1,2})?)$/.exec(last);
    if (m) return parseFloat(m[1].replace('-', '.'));
  }
  m = /(\d+(?:\.\d+)?)/.exec(t);
  if (m && !/^(19|20)\d\d$/.test(m[1])) return parseFloat(m[1]);
  return null;
};
H.MONTHS = {
  jan: 0, january: 0, feb: 1, february: 1, mar: 2, march: 2, apr: 3, april: 3, may: 4, jun: 5, june: 5, jul: 6, july: 6,
  aug: 7, august: 7, sep: 8, sept: 8, september: 8, oct: 9, october: 9, nov: 10, november: 10, dec: 11, december: 11,
  enero: 0, ene: 0, febrero: 1, marzo: 2, abril: 3, abr: 3, mayo: 4, junio: 5, julio: 6, agosto: 7, ago: 7, septiembre: 8, setiembre: 8,
  octubre: 9, noviembre: 10, diciembre: 11, dic: 11, janeiro: 0, fevereiro: 1, fev: 1, 'mar\u00e7o': 2, maio: 4, mai: 4, junho: 5,
  julho: 6, setembro: 8, set: 8, outubro: 9, out: 9, novembro: 10, dezembro: 11, dez: 11, januari: 0, februari: 1, maret: 2,
  mei: 4, juni: 5, juli: 6, agustus: 7, agu: 7, okt: 9, oktober: 9, desember: 11, des: 11, janvier: 0, 'f\u00e9vrier': 1, fevrier: 1,
  mars: 2, avril: 3, avr: 3, juin: 5, juillet: 6, juil: 6, 'ao\u00fbt': 7, aout: 7, septembre: 8, octobre: 9, novembre: 10,
  'd\u00e9cembre': 11, decembre: 11
};
H._iso = function (ms) { var d = new Date(ms); return isNaN(d.getTime()) ? null : d.toISOString(); };
H._mk = function (y, mo, d) {
  if (y < 100) y += 2000;
  if (mo < 0 || mo > 11 || d < 1 || d > 31 || y < 1990 || y > 2100) return null;
  return H._iso(Date.UTC(y, mo, d, 12));
};
H.parseDate = function (s) {
  if (s == null) return null;
  var t = H.line(String(s)).toLowerCase();
  if (!t) return null;
  var now = Date.now();
  if (/^(just now|now|today|hoy|hari ini|aujourd|heute|bug[u\u00fc]n)/.test(t)) return H._iso(now);
  if (/^(yesterday|ayer|kemarin|hier\b|gestern|d[u\u00fc]n\b|ontem)/.test(t)) return H._iso(now - 864e5);
  var rel = /(\d+|an?|one)\s*(seconds?|secs?|minutes?|mins?|hours?|hrs?|h\b|days?|d\b|weeks?|wks?|months?|mons?|years?|yrs?|detik|menit|jam|hari|minggu|bulan|tahun|segundos?|minutos?|horas?|d[i\u00ed]as?|semanas?|mes(?:es)?|a[n\u00f1]os?|secondes?|heures?|jours?|semaines?|mois|ans?)\b/.exec(t);
  if (rel && (/ago|lalu|hace|atr[a\u00e1]s|il y a|\u00f6nce|vor\b|yang lalu/.test(t) || /^\d+\s*[a-z]+$/.test(t))) {
    var n = /^\d+$/.test(rel[1]) ? parseInt(rel[1], 10) : 1, u = rel[2], ms = 0;
    if (/^(sec|detik|segund|seconde)/.test(u)) ms = 1e3;
    else if (/^(min|menit|minut)/.test(u) && !/^mon/.test(u)) ms = 6e4;
    else if (/^(h|hour|hr|jam|hora|heure)/.test(u)) ms = 36e5;
    else if (/^(d|day|hari|d[i\u00ed]a|jour)/.test(u)) ms = 864e5;
    else if (/^(w|week|wk|minggu|semana|semaine)/.test(u)) ms = 6048e5;
    else if (/^(mon|month|bulan|mes|mois)/.test(u)) ms = 2592e6;
    else if (/^(y|year|yr|tahun|a[n\u00f1]o|an)/.test(u)) ms = 31536e6;
    if (ms) return H._iso(now - n * ms);
  }
  var m = /(\d{4})[-\/.](\d{1,2})[-\/.](\d{1,2})/.exec(t);
  if (m) return H._mk(+m[1], +m[2] - 1, +m[3]);
  m = /(\d{1,2})[-\/.](\d{1,2})[-\/.](\d{2,4})/.exec(t);
  if (m) {
    var a = +m[1], b = +m[2], y = +m[3];
    return a > 12 ? H._mk(y, b - 1, a) : H._mk(y, a - 1, b);
  }
  m = /([a-z\u00e0-\u00ff]+)\.?\s+(\d{1,2})(?:st|nd|rd|th)?\s*,?\s+(\d{2,4})/.exec(t);
  if (m && H.MONTHS[m[1]] != null) return H._mk(+m[3], H.MONTHS[m[1]], +m[2]);
  m = /(\d{1,2})(?:st|nd|rd|th|er)?\.?\s+(?:de\s+)?([a-z\u00e0-\u00ff]+)\.?\s*,?\s+(?:de\s+)?(\d{4})/.exec(t);
  if (m && H.MONTHS[m[2]] != null) return H._mk(+m[3], H.MONTHS[m[2]], +m[1]);
  var d = Date.parse(s);
  if (!isNaN(d)) return H._iso(d);
  return null;
};
H.status = function (s) {
  var t = H.line(s || '').toLowerCase();
  if (!t) return undefined;
  if (/hiatus|haitus|on hold|pausad|paused|en pausa|em pausa|suspend/.test(t)) return 'hiatus';
  if (/cancel|dropped|abandon|discontinu|droped|annul/.test(t)) return 'cancelled';
  if (/complete|finish|finaliz|terminad|termin\u00e9|tamat|conclu|end(ed)?\b|bitti|fini\b|full/.test(t)) return 'completed';
  if (/ongoing|on going|releasing|publishing|curso|emisi|activ|berjalan|lan\u00e7amento|lancamento|en cours|serializ|updating|airing|devam|update|on-going/.test(t)) return 'ongoing';
  return undefined;
};
/** Last-resort status detection from CSS classes / "Status: X" text anywhere in the page. */
H.statusGuess = function (html) {
  var h = String(html || '');
  var m = /class=["'][^"']*\b(?:is|st|status|manga-status|series-status)[-_](on-?going|completed?|finished|hiatus|cancell?ed|dropped)\b/i.exec(h);
  if (m) return H.status(m[1].replace(/-/g, ''));
  var body = h.replace(/<script\b[\s\S]*?<\/script\s*>/gi, ' ').replace(/<style\b[\s\S]*?<\/style\s*>/gi, ' ');
  var re = /\b(?:status|estado|statut)\s*(?:<[^>]*>\s*)*[:\-]?\s*(?:<[^>]*>\s*)*([A-Za-zÀ-ÿ][A-Za-zÀ-ÿ \-]{2,20})/gi, mm;
  while ((mm = re.exec(body)) !== null) { var s = H.status(mm[1]); if (s) return s; }
  return undefined;
};
/** WordPress post_class taxonomy terms of the main article, e.g. wp-manga-genre-action. */
H.wpTerms = function (html, taxonomy) {
  var m = /class=["']([^"']*\btype-wp-manga\b[^"']*)["']/i.exec(String(html || ''));
  if (!m) return [];
  var re = new RegExp('\\b' + taxonomy + '-([a-z0-9-]+)', 'gi'), t, out = [];
  while ((t = re.exec(m[1])) !== null) {
    var name = decodeURIComponent(t[1]).replace(/-/g, ' ').replace(/\b[a-z]/g, function (c) { return c.toUpperCase(); });
    if (out.indexOf(name) < 0) out.push(name);
  }
  return out;
};
/** Find a "Label: value" pair inside any of the container elements. */
H.labeled = function (x, containerSel, labelRe) {
  var els = H.find(x, containerSel);
  for (var i = 0; i < els.length; i++) {
    if (els[i].html.length > 4000) continue;
    var txt = H.line(els[i].html);
    if (!txt || txt.length > 600) continue;
    var m = labelRe.exec(txt);
    if (m && m.index === 0) {
      var v = txt.slice(m[0].length).replace(/^[\s:\-\u2013\u2014|]+/, '').trim();
      if (v) return { el: els[i], value: v };
    }
  }
  return null;
};
H.names = function (x, sel) {
  var out = [], els = H.find(x, sel);
  for (var i = 0; i < els.length; i++) { var t = H.line(els[i].html); if (t && out.indexOf(t) < 0) out.push(t); }
  return out;
};
H.splitList = function (s) {
  if (!s) return [];
  var parts = String(s).split(/\s*[,;\/|\u3001]\s*|\s{2,}/), out = [];
  for (var i = 0; i < parts.length; i++) { var p = parts[i].trim(); if (p && p.length < 60 && out.indexOf(p) < 0) out.push(p); }
  return out;
};
H.balancedJson = function (src, startIdx) {
  // startIdx points at '{' or '['. Returns the substring of the balanced JSON value.
  var open = src.charAt(startIdx), close = open === '{' ? '}' : ']';
  var depth = 0, inStr = false, q = '', esc = false;
  for (var i = startIdx; i < src.length; i++) {
    var ch = src.charAt(i);
    if (inStr) { if (esc) esc = false; else if (ch === '\\') esc = true; else if (ch === q) inStr = false; continue; }
    if (ch === '"' || ch === "'") { inStr = true; q = ch; continue; }
    if (ch === open) depth++;
    else if (ch === close) { depth--; if (depth === 0) return src.slice(startIdx, i + 1); }
  }
  return null;
};
H.B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
H.atob = function (s) {
  s = String(s).replace(/[^A-Za-z0-9+\/]/g, '');
  var out = '', buf = 0, bits = 0;
  for (var i = 0; i < s.length; i++) {
    buf = (buf << 6) | H.B64.indexOf(s.charAt(i)); bits += 6;
    if (bits >= 8) { bits -= 8; out += String.fromCharCode((buf >> bits) & 0xff); }
  }
  try { return decodeURIComponent(out.replace(/[%\x80-\xff]/g, function (c) { return '%' + ('0' + c.charCodeAt(0).toString(16)).slice(-2); })); } catch (e) { return out; }
};

// ── network ──
H.headers = function (extra) {
  var h = {
    'User-Agent': UA,
    'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8',
    'Accept-Language': 'en-US,en;q=0.9',
    'Referer': STATE.base + '/'
  };
  if (extra) for (var k in extra) if (Object.prototype.hasOwnProperty.call(extra, k)) h[k] = extra[k];
  return h;
};
H.isChallenge = function (status, text) {
  var t = String(text || '').slice(0, 20000);
  if (/<title>\s*(just a moment|attention required|ddos-guard|checking your browser|please wait|one more step|access denied|security check)/i.test(t)) return true;
  if (/window\._cf_chl_opt|cf-browser-verification|id=["']challenge-form|ddos-guard\/js-challenge|__ddg1|cf-turnstile-wrapper/i.test(t) && (status === 403 || status === 503 || status === 429 || t.length < 15000)) return true;
  return false;
};
H.request = async function (url, opts) {
  opts = opts || {};
  var o = { method: opts.method || 'GET', headers: H.headers(opts.headers) };
  if (opts.body != null) o.body = opts.body;
  var r = null, err = null;
  try { r = await __fetch(url, o); } catch (e) { err = e; }
  var status = r ? (r.status || 0) : 0;
  var text = '';
  if (r) {
    if (typeof r.text === 'string') text = r.text;
    else if (typeof r.data === 'string') text = r.data;
    else if (r.data != null) { try { text = JSON.stringify(r.data); } catch (e2) { text = ''; } }
  }
  if (o.method === 'GET' && !opts.noRender && typeof __render === 'function' && (err || H.isChallenge(status, text))) {
    try {
      var rr = await __render(url, { waitForSelector: opts.waitForSelector, timeoutMs: opts.timeoutMs || 45000 });
      if (rr && rr.html) { text = rr.html; status = rr.status || 200; err = null; r = { status: status, data: text, text: text, url: rr.url }; }
    } catch (e3) { /* keep original */ }
  }
  if (err && !r) throw err;
  return { status: status, text: text, data: r ? r.data : null, url: (r && typeof r.url === 'string' && r.url) || url };
};
H.get = async function (url, opts) {
  var r = await H.request(url, opts);
  if (!r.status || r.status >= 400) throw new Error('HTTP ' + r.status + ' for ' + url);
  if (H.isChallenge(r.status, r.text)) throw new Error('Blocked by an anti-bot challenge at ' + url);
  return H.stripComments(r.text);
};
H.post = async function (url, body, headers) {
  var hdr = { 'Content-Type': 'application/x-www-form-urlencoded; charset=UTF-8', 'X-Requested-With': 'XMLHttpRequest', 'Origin': STATE.base };
  if (headers) for (var k in headers) hdr[k] = headers[k];
  var r = await H.request(url, { method: 'POST', body: typeof body === 'string' ? body : H.form(body || {}), headers: hdr, noRender: true });
  if (!r.status || r.status >= 400) throw new Error('HTTP ' + r.status + ' for POST ' + url);
  return r;
};
H.json = async function (url, opts) {
  opts = opts || {};
  var hdr = { 'Accept': 'application/json, text/plain, */*' };
  if (opts.headers) for (var k in opts.headers) hdr[k] = opts.headers[k];
  var r = await H.request(url, { method: opts.method, body: opts.body, headers: hdr, noRender: true });
  if (!r.status || r.status >= 400) throw new Error('HTTP ' + r.status + ' for ' + url);
  var d = r.data;
  if (typeof d === 'string' || d == null) { try { d = JSON.parse(r.text); } catch (e) { throw new Error('Invalid JSON from ' + url); } }
  return d;
};
/** Explicit JS render (if the host supports it). Returns html or ''. */
H.render = async function (url, waitForSelector) {
  if (typeof __render !== 'function') return '';
  try {
    var rr = await __render(url, { waitForSelector: waitForSelector, timeoutMs: 45000 });
    return rr && rr.html ? H.stripComments(rr.html) : '';
  } catch (e) { return ''; }
};

// ── result shaping ──
// Plugins regenerated from the pre-template generator keep its id shapes
// (manga = bare slug, chapter = absolute URL) so library entries stay linked.
H.mangaId = function (href) {
  var id = H.toId(href);
  if (!CFG.legacyIds) return id;
  var m = /^manga\/([^\/?#]+)\/?$/.exec(id);
  return m ? m[1] : id;
};
H.chapterId = function (href) {
  if (!CFG.legacyIds) return H.toId(href);
  return H.abs(href, STATE.base + '/').replace(/#.*$/, '');
};
H.manga = function (href, title, cover, extra) {
  var t = H.line(title || '');
  if (!href || !t) return null;
  var o = { source_id: H.mangaId(href), title: t };
  if (cover) o.cover_url = H.withHint(cover);
  if (extra) for (var k in extra) if (extra[k]) o[k] = extra[k];
  return o;
};
H.cleanList = function (list) {
  var out = [], seen = {};
  for (var i = 0; i < (list || []).length; i++) {
    var m = list[i];
    if (!m || !m.source_id || !m.title) continue;
    if (seen[m.source_id]) {
      var prev = seen[m.source_id];
      if (!prev.cover_url && m.cover_url) prev.cover_url = m.cover_url;
      continue;
    }
    seen[m.source_id] = m;
    out.push(m);
  }
  return out;
};
H.chapter = function (href, title, date, extra) {
  if (!href) return null;
  var t = H.line(title || '');
  var num = extra && extra.number != null && !isNaN(extra.number) ? extra.number : H.chapterNumber(t, href);
  var o = { source_id: H.chapterId(href), number: num, title: t || (num != null ? 'Chapter ' + num : 'Chapter'), language: CFG.lang };
  var d = date ? H.parseDate(date) : null;
  if (d) o.published_at = d;
  if (extra && extra.group) o.scanlation_group = extra.group;
  return o;
};
H.cleanChapters = function (list) {
  var out = [], seen = {};
  for (var i = 0; i < (list || []).length; i++) {
    var c = list[i];
    if (!c || !c.source_id || seen[c.source_id]) continue;
    seen[c.source_id] = 1;
    out.push(c);
  }
  // Fill missing numbers from position (lists are usually newest-first).
  var missing = 0;
  for (var j = 0; j < out.length; j++) if (out[j].number == null) missing++;
  if (missing === out.length && out.length) {
    for (var k = 0; k < out.length; k++) out[k].number = out.length - k;
  }
  out.sort(function (a, b) {
    var x = a.number == null ? -1 : a.number, y = b.number == null ? -1 : b.number;
    return x - y;
  });
  return out;
};
