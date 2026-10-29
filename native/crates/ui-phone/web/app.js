// MangaVox phone companion. Plain ES module, no dependencies, no build step.
// Every piece of text that comes from the network is inserted with textContent (via h()),
// never through innerHTML.

/* ======================================================================
   Basics
   ====================================================================== */
const view = document.getElementById('view');
const enc = encodeURIComponent;
const LS = {
  get(k, d) { try { const v = localStorage.getItem(k); return v == null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch { /* private mode */ } },
  del(k) { try { localStorage.removeItem(k); } catch { /* ignore */ } },
};
const PROPS = new Set(['value', 'checked', 'disabled', 'hidden', 'muted', 'indeterminate', 'selected']);

function safeHref(v) {
  const s = String(v);
  return /^(#|\/(?!\/)|https?:)/i.test(s) ? s : '#';
}

/** Safe element builder: strings always become text nodes. */
function h(tag, attrs, ...kids) {
  const el = document.createElement(tag);
  if (attrs) {
    for (const k in attrs) {
      const v = attrs[k];
      if (v == null || v === false) continue;
      if (k === 'class') el.className = v;
      else if (k === 'style') Object.assign(el.style, v);
      else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
      else if (PROPS.has(k)) el[k] = v;
      else if (k === 'href') el.setAttribute('href', safeHref(v));
      else el.setAttribute(k, v === true ? '' : String(v));
    }
  }
  add(el, kids);
  return el;
}
function add(el, kids) {
  for (const c of kids) {
    if (c == null || c === false || c === '') continue;
    if (Array.isArray(c)) add(el, c);
    else el.append(c instanceof Node ? c : String(c));
  }
}

const ICONS = {
  book: 'M4 19.5A2.5 2.5 0 0 1 6.5 17H20V3H6.5A2.5 2.5 0 0 0 4 5.5v14zM6.5 17A2.5 2.5 0 0 0 4 19.5 2.5 2.5 0 0 0 6.5 22H20v-5',
  sparkle: 'M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8zM19 15l.7 1.8 1.8.7-1.8.7L19 20l-.7-1.8-1.8-.7 1.8-.7z',
  headphones: 'M3 18v-6a9 9 0 0 1 18 0v6M21 19a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3zM3 19a2 2 0 0 0 2 2h1a2 2 0 0 0 2-2v-3a2 2 0 0 0-2-2H3z',
  film: 'M3 5h18v14H3zM7 5v14M17 5v14M3 9.5h4M3 14.5h4M17 9.5h4M17 14.5h4',
  remote: 'M3 4h18v12H3zM8 20h8M12 16v4M10 8.5l4 1.8-1.7.6-.6 1.7z',
  settings: 'M4 7h9M17 7h3M4 17h3M11 17h9M15 5v4M9 15v4',
  back: 'M15 18l-6-6 6-6',
  chev: 'M9 18l6-6-6-6',
  play: 'FM7 4.5v15l12.5-7.5z',
  pause: 'FM6.5 4.5h4v15h-4zM13.5 4.5h4v15h-4z',
  rotl: 'M4 12a8 8 0 1 0 2.4-5.7M4 4v4h4',
  rotr: 'M20 12a8 8 0 1 1-2.4-5.7M20 4v4h-4',
  close: 'M6 6l12 12M18 6L6 18',
  search: 'M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM20 20l-4-4',
  refresh: 'M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5',
  download: 'M12 4v11M7 10l5 5 5-5M5 20h14',
  check: 'M5 12l5 5 9-10',
  keyboard: 'M3 6h18v12H3zM7 10h.01M11 10h.01M15 10h.01M7 14h10',
  wave: 'M4 10v4M8 7v10M12 4v16M16 7v10M20 10v4',
  video: 'M3 6h12v12H3zM15 10l6-3v10l-6-3',
  next: 'FM6 5l10 7-10 7zM16.5 5h2v14h-2z',
  prev: 'FM18 5L8 12l10 7zM5.5 5h2v14h-2z',
  info: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 11v5M12 7.5h.01',
  wifioff: 'M3 3l18 18M8.5 16.5a5 5 0 0 1 7 0M5 12.6a10 10 0 0 1 5-2.7M14 9.9a10 10 0 0 1 5 2.7M2 8.8A15 15 0 0 1 7 6M12 20h.01',
  scroll: 'M12 3v18M8 7l4-4 4 4M8 17l4 4 4-4',
  drag: 'M12 3v18M3 12h18M9 6l3-3 3 3M9 18l3 3 3-3M6 9l-3 3 3 3M18 9l3 3-3 3',
  stop: 'M7 7h10v10H7z',
  star: 'FM12 3l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1L3.2 9.5l6.1-.9z',
  pc: 'M3 4h18v12H3zM8 20h8M12 16v4',
  sort: 'M7 4v16M3 8l4-4 4 4M17 20V4M13 16l4 4 4-4',
  alert: 'M12 3l10 18H2zM12 10v5M12 18h.01',
  logout: 'M14 4h5v16h-5M10 8l-4 4 4 4M6 12h11',
};
const SVGNS = 'http://www.w3.org/2000/svg';
function icon(name, cls) {
  const d = ICONS[name] || ICONS.info;
  const svg = document.createElementNS(SVGNS, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('class', 'ic' + (cls ? ' ' + cls : ''));
  const p = document.createElementNS(SVGNS, 'path');
  if (d[0] === 'F') { svg.setAttribute('fill', 'currentColor'); p.setAttribute('d', d.slice(1)); }
  else {
    svg.setAttribute('fill', 'none'); svg.setAttribute('stroke', 'currentColor'); svg.setAttribute('stroke-width', '2');
    svg.setAttribute('stroke-linecap', 'round'); svg.setAttribute('stroke-linejoin', 'round'); p.setAttribute('d', d);
  }
  svg.append(p);
  return svg;
}
const iconBtn = (name, label, onclick, cls) =>
  h('button', { class: 'icon-btn' + (cls ? ' ' + cls : ''), type: 'button', 'aria-label': label, title: label, onclick }, icon(name));

/* ---------- formatting ---------- */
function fmtTime(s) {
  if (!isFinite(s) || s < 0) s = 0;
  s = Math.floor(s);
  const hh = Math.floor(s / 3600), mm = Math.floor((s % 3600) / 60), ss = s % 60;
  return (hh ? hh + ':' + String(mm).padStart(2, '0') : mm) + ':' + String(ss).padStart(2, '0');
}
function fmtSize(b) {
  if (!b) return '';
  const u = ['B', 'KB', 'MB', 'GB']; let i = 0;
  while (b >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return (i < 2 ? Math.round(b) : b.toFixed(1)) + ' ' + u[i];
}
function toDate(v) {
  if (v == null || v === '') return null;
  if (typeof v === 'number') return new Date(v > 1e12 ? v : v * 1000);
  const n = Number(v);
  if (!Number.isNaN(n)) return toDate(n);
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? null : d;
}
function ago(v) {
  const d = toDate(v); if (!d) return '';
  const s = (Date.now() - d.getTime()) / 1000;
  if (s < 60) return 'just now';
  if (s < 3600) return Math.floor(s / 60) + ' min ago';
  if (s < 86400) return Math.floor(s / 3600) + ' h ago';
  if (s < 86400 * 7) return Math.floor(s / 86400) + ' d ago';
  return d.toLocaleDateString();
}
function initials(t) {
  return String(t || '?').split(/\s+/).filter(Boolean).slice(0, 2).map(w => w[0].toUpperCase()).join('') || '?';
}
function chLabel(c) {
  const num = c.number != null && c.number !== '' ? String(c.number) : '';
  const t = c.title ? String(c.title) : '';
  if (!num) return t || 'Chapter';
  if (!t || t === 'Chapter ' + num || t === 'Ch. ' + num) return 'Chapter ' + num;
  return 'Ch. ' + num + ' · ' + t;
}
function fmtRuntime(r) {
  if (r == null || r === '') return '';
  if (typeof r === 'number' || /^\d+$/.test(String(r))) {
    const m = Number(r); return m >= 60 ? Math.floor(m / 60) + 'h ' + (m % 60) + 'm' : m + 'm';
  }
  return String(r);
}
const debounce = (fn, ms) => { let t; return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); }; };

/* ======================================================================
   API
   ====================================================================== */
class ApiError extends Error {
  constructor(msg, status, offline) { super(msg); this.status = status; this.offline = !!offline; }
}
let lockedOut = false;
function goLocked() {
  if (lockedOut) return;
  lockedOut = true;
  location.replace('/locked');
}
async function api(path, opts = {}) {
  const { method = 'GET', body, signal, raw, keepalive } = opts;
  const headers = { 'X-MangaVox': '1' };
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  let res;
  try {
    res = await fetch(path, {
      method, headers, signal, keepalive, credentials: 'same-origin', cache: 'no-store',
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  } catch (e) {
    if (e && e.name === 'AbortError') throw e;
    setConn(false);
    throw new ApiError('Can’t reach your PC', 0, true);
  }
  setConn(true);
  if (res.status === 401) { goLocked(); throw new ApiError('This phone is not paired', 401); }
  if (!res.ok) {
    let msg = 'Something went wrong (' + res.status + ')';
    try { const j = await res.json(); if (j && j.error) msg = String(j.error); } catch { /* not json */ }
    throw new ApiError(msg, res.status);
  }
  if (raw) return res;
  const text = await res.text();
  if (!text) return {};
  try { return JSON.parse(text); } catch { throw new ApiError('Bad response from PC', res.status); }
}
const post = (path, body = {}, o = {}) => api(path, { ...o, method: 'POST', body });

// Small stale-while-revalidate cache so going back is instant.
const cache = new Map();
function swr(ctx, path, draw, onErr) {
  const hit = cache.get(path);
  if (hit) draw(hit.data, true);
  return api(path).then(data => {
    const json = JSON.stringify(data);
    cache.set(path, { data, json });
    if (ctx.alive && (!hit || hit.json !== json)) draw(data, false);
    return data;
  }).catch(e => {
    if (!ctx.alive || e.status === 401) return;
    if (!hit) onErr(e); else if (e.offline) toast('Offline: showing saved data', 'err');
  });
}
const invalidate = prefix => { for (const k of cache.keys()) if (k.startsWith(prefix)) cache.delete(k); };

/* ======================================================================
   Chrome: header, tabs, toasts, sheets, states
   ====================================================================== */
const $title = document.getElementById('title');
const $sub = document.getElementById('subtext');
const $dot = document.getElementById('dot');
const $actions = document.getElementById('actions');
const $back = document.getElementById('back');
const $toasts = document.getElementById('toasts');
let me = null;

function setTitle(t) { $title.textContent = t || 'MangaVox'; document.title = t ? t + ' · MangaVox' : 'MangaVox'; }
function setActions(...els) { $actions.replaceChildren(...els.filter(Boolean)); }
let backTarget = null;
function setBack(fallback) { backTarget = fallback; $back.hidden = !fallback; }
function goBack(fallback) {
  if (navCount > 1 && history.length > 1) history.back();
  else location.hash = fallback;
}
function setConn(ok) {
  $dot.className = 'dot ' + (ok ? 'on' : 'off');
  if (!ok) $sub.textContent = 'Can’t reach PC';
  else if (me) $sub.textContent = subLine();
}
const subLine = () => [me && me.pc, me && me.device && me.device.name].filter(Boolean).join(' · ') || 'Connected';

function toast(msg, type = 'info', ms = 2800) {
  const t = h('div', { class: 'toast ' + type, role: type === 'err' ? 'alert' : 'status' }, msg);
  $toasts.append(t);
  while ($toasts.children.length > 3) $toasts.firstChild.remove();
  requestAnimationFrame(() => requestAnimationFrame(() => t.classList.add('in')));
  setTimeout(() => { t.classList.remove('in'); setTimeout(() => t.remove(), 300); }, ms);
}

function stateBox(ic, title, text, ...actions) {
  return h('div', { class: 'state' }, icon(ic), h('h2', null, title), text && h('p', null, text), actions);
}
function errorState(e, retry) {
  const off = e && e.offline;
  return stateBox(off ? 'wifioff' : 'alert', off ? 'Can’t reach your PC' : 'Something went wrong',
    off ? 'Make sure MangaVox is running on your PC and this phone is on the same Wi‑Fi.' : (e && e.message) || '',
    retry && h('button', { class: 'btn primary', type: 'button', onclick: retry }, icon('refresh'), 'Try again'));
}
function spinnerRow() { return h('div', { class: 'state' }, h('div', { class: 'btn ghost busy', 'aria-label': 'Loading' })); }

/** Cover image with graceful fallback to initials. */
function cover(src, title, cls) {
  const box = h('div', { class: 'cov' + (cls ? ' ' + cls : '') });
  const ph = h('div', { class: 'cov-ph' }, initials(title));
  if (!src) { box.append(ph); return box; }
  const img = h('img', { src, alt: '', loading: 'lazy', decoding: 'async', referrerpolicy: 'no-referrer' });
  img.addEventListener('load', () => img.classList.add('ok'), { once: true });
  img.addEventListener('error', () => img.replaceWith(ph), { once: true });
  box.append(img);
  return box;
}
function webImg(src, cls, alt = '') {
  if (!src) return null;
  const img = h('img', { class: cls, src, alt, loading: 'lazy', decoding: 'async', referrerpolicy: 'no-referrer' });
  img.addEventListener('error', () => img.remove(), { once: true });
  return img;
}

/* Sheets participate in history so the Android back button closes them. */
const sheets = [];
let suppressPop = 0;
function openSheet(build, { cls = '', label = 'Dialog' } = {}) {
  const panel = h('div', { class: 'sheet ' + cls, role: 'dialog', 'aria-modal': 'true', 'aria-label': label, tabindex: '-1' });
  const s = { panel, closed: false, onClose: [], close: () => closeSheet(s) };
  const back = h('div', { class: 'sheet-back', onclick: () => s.close() });
  s.wrap = h('div', { class: 'sheet-wrap' }, back, panel);
  s.prevFocus = document.activeElement;
  build(panel, s);
  document.body.append(s.wrap);
  document.body.classList.add('modal-open');
  sheets.push(s);
  history.pushState({ mvSheet: sheets.length }, '');
  requestAnimationFrame(() => requestAnimationFrame(() => s.wrap.classList.add('open')));
  panel.focus({ preventScroll: true });
  return s;
}
function closeSheet(s, fromPop = false) {
  if (s.closed) return;
  s.closed = true;
  const i = sheets.indexOf(s); if (i >= 0) sheets.splice(i, 1);
  for (const f of s.onClose) { try { f(); } catch { /* ignore */ } }
  s.wrap.classList.remove('open');
  setTimeout(() => s.wrap.remove(), 320);
  if (!sheets.length) document.body.classList.remove('modal-open');
  if (!fromPop) { suppressPop++; history.back(); }
  if (s.prevFocus && s.prevFocus.focus) s.prevFocus.focus({ preventScroll: true });
}
addEventListener('popstate', () => {
  if (suppressPop) { suppressPop--; return; }
  const top = sheets[sheets.length - 1];
  if (top) closeSheet(top, true);
});
addEventListener('keydown', e => {
  if (e.key === 'Escape' && sheets.length) { e.preventDefault(); sheets[sheets.length - 1].close(); }
});

function segmented(options, value, onchange, label) {
  const wrap = h('div', { class: 'seg', role: 'group', 'aria-label': label });
  const btns = options.map(([v, text, ic]) => h('button', {
    type: 'button', 'aria-pressed': String(v === value), onclick: () => {
      for (const b of btns) b.setAttribute('aria-pressed', String(b === btn(v)));
      onchange(v);
    },
  }, ic && icon(ic), h('span', null, text)));
  const btn = v => btns[options.findIndex(o => o[0] === v)];
  wrap.append(...btns);
  return wrap;
}
function setRange(input) {
  const min = +input.min || 0, max = +input.max || 100;
  input.style.setProperty('--p', ((+input.value - min) / ((max - min) || 1)) * 100 + '%');
}

/* ======================================================================
   Router
   ====================================================================== */
const ROUTES = [
  ['library', /^\/library$/, viewLibrary],
  ['library', /^\/manga\/([^/]+)$/, viewManga],
  ['reader', /^\/read\/([^/]+)$/, viewReader],
  ['studio', /^\/studio$/, viewStudio],
  ['listen', /^\/listen$/, viewListen],
  ['movies', /^\/movies$/, viewMovies],
  ['remote', /^\/remote$/, viewRemote],
];
let ctx = null;
let navCount = 0;
const scrollMemo = new Map();
const tabs = [...document.querySelectorAll('#tabbar a')];

function route() {
  if (lockedOut) return;
  const raw = location.hash.slice(1) || '/library';
  const [path, qs] = raw.split('?');
  let match = null;
  for (const [tab, re, fn] of ROUTES) {
    const m = path.match(re);
    if (m) {
      try { match = { tab, fn, params: m.slice(1).map(decodeURIComponent) }; } catch { match = null; }
      break;
    }
  }
  if (!match) { location.replace('#/library'); return; }
  while (sheets.length) closeSheet(sheets[sheets.length - 1], true);
  if (ctx) {
    scrollMemo.set(ctx.key, scrollY);
    ctx.alive = false;
    for (const f of ctx.cleanups.splice(0)) { try { f(); } catch (e) { console.error(e); } }
  }
  const c = {
    key: raw, alive: true, cleanups: [],
    onLeave(f) { this.cleanups.push(f); },
    restoreScroll() { const y = scrollMemo.get(raw); if (y) scrollTo(0, y); },
  };
  ctx = c;
  navCount++;
  document.body.dataset.route = match.tab;
  const tabName = match.tab === 'reader' ? 'library' : match.tab;
  for (const a of tabs) {
    const on = a.dataset.tab === tabName;
    a.classList.toggle('active', on);
    if (on) a.setAttribute('aria-current', 'page'); else a.removeAttribute('aria-current');
  }
  setActions();
  setBack(null);
  view.replaceChildren();
  scrollTo(0, 0);
  try { match.fn(c, ...match.params, new URLSearchParams(qs || '')); }
  catch (e) { console.error(e); view.append(errorState(e, () => route())); }
}

/* ======================================================================
   Library
   ====================================================================== */
function getManga(id, fresh) {
  const p = '/api/manga/' + enc(id);
  const hit = cache.get(p);
  if (hit && !fresh) return Promise.resolve(hit.data);
  return api(p).then(d => { cache.set(p, { data: d, json: JSON.stringify(d) }); return d; });
}
function pickResume(chs) {
  if (!chs || !chs.length) return null;
  let last = -1;
  chs.forEach((c, i) => { if (c.read || (c.progress || 0) > 0) last = i; });
  if (last < 0) return chs[0];
  const c = chs[last];
  if (!c.read && (c.progress || 0) < 0.98) return c;
  return chs[last + 1] || c;
}
async function resumeManga(id) {
  try {
    const m = await getManga(id, true);
    const ch = pickResume(m.chapters);
    if (!ch) { location.hash = '#/manga/' + enc(id); return; }
    location.hash = '#/read/' + enc(ch.id);
  } catch (e) { if (e.status !== 401) toast(e.message, 'err'); }
}

function skelGrid(n = 9) {
  return h('div', { class: 'grid' }, Array.from({ length: n }, () =>
    h('div', null, h('div', { class: 'cov skel' }), h('div', { class: 'skel skel-line', style: { width: '80%' } }))));
}

function viewLibrary(ctx) {
  setTitle('Library');
  const root = h('div', { class: 'page' }, h('div', { class: 'skel', style: { height: '86px', margin: '8px 16px 24px' } }), skelGrid());
  view.append(root);
  let filter = '';
  const refreshBtn = iconBtn('refresh', 'Refresh library', () => load());

  function contCard(m) {
    return h('button', { class: 'ccard', type: 'button', onclick: () => resumeManga(m.id), 'aria-label': 'Continue ' + m.title },
      cover(m.cover, m.title),
      h('div', { class: 'ccard-b' },
        h('div', { class: 'ccard-t' }, m.title),
        h('div', { class: 'ccard-s' }, [ago(m.last_read_at), m.unread ? m.unread + ' unread' : ''].filter(Boolean).join(' · ')),
        h('div', { class: 'ccard-p' }, icon('play'), 'Continue')));
  }
  function mangaCard(m) {
    const unread = m.unread | 0;
    const c = cover(m.cover, m.title);
    if (unread > 0) c.append(h('span', { class: 'badge', 'aria-label': unread + ' unread' }, unread > 99 ? '99+' : String(unread)));
    return h('a', { class: 'mcard', href: '#/manga/' + enc(m.id) }, c,
      h('div', { class: 'mcard-t' }, m.title),
      h('div', { class: 'mcard-s' }, (m.chapters | 0) + ' chapters'));
  }
  function draw(list) {
    list = Array.isArray(list) ? list : [];
    root.replaceChildren();
    if (!list.length) {
      root.append(stateBox('book', 'Your library is empty', 'Add manga to your library in MangaVox on your PC and they show up here.',
        h('button', { class: 'btn', type: 'button', onclick: () => post('/api/pc/open', { page: 'library' }).then(() => toast('Opened Library on PC', 'ok'), e => toast(e.message, 'err')) }, icon('pc'), 'Open Library on PC')));
      return;
    }
    const recent = list.filter(m => m.last_read_at).slice(0, 12);
    if (recent.length) {
      root.append(h('section', { class: 'sec', 'aria-label': 'Continue reading' },
        h('h2', { class: 'sec-h' }, 'Continue reading'),
        h('div', { class: 'hscroll' }, recent.map(contCard))));
    }
    const grid = h('div', { class: 'grid' });
    const fill = () => {
      const q = filter.trim().toLowerCase();
      const items = list.filter(m => !q || String(m.title || '').toLowerCase().includes(q) || String(m.author || '').toLowerCase().includes(q));
      grid.replaceChildren(...items.map(mangaCard));
      if (!items.length) grid.append(h('p', { class: 'muted' }, 'No matches.'));
    };
    root.append(h('section', { class: 'sec' },
      h('h2', { class: 'sec-h' }, 'All manga', h('span', { class: 'count' }, String(list.length))),
      list.length > 6 && h('label', { class: 'search' }, icon('search'),
        h('input', { type: 'search', placeholder: 'Filter library', 'aria-label': 'Filter library', value: filter, oninput: e => { filter = e.target.value; fill(); } })),
      grid));
    fill();
  }
  function load() {
    refreshBtn.classList.add('busy');
    return swr(ctx, '/api/library', (d, fromCache) => { draw(d); if (fromCache) ctx.restoreScroll(); },
      e => root.replaceChildren(errorState(e, () => { root.replaceChildren(skelGrid()); load(); })))
      .finally(() => refreshBtn.classList.remove('busy'));
  }
  setActions(refreshBtn);
  load();
}

/* ---------- Title page ---------- */
function viewManga(ctx, id) {
  setTitle('');
  setBack('#/library');
  const root = h('div', { class: 'page' },
    h('div', { class: 'mhero' }, h('div', { class: 'mhero-in' }, h('div', { class: 'cov skel', style: { width: '116px' } }),
      h('div', { class: 'mhero-meta' }, h('div', { class: 'skel skel-line', style: { width: '80%', height: '20px' } }), h('div', { class: 'skel skel-line', style: { width: '40%' } })))),
    h('div', { class: 'pad' }, Array.from({ length: 6 }, () => h('div', { class: 'skel', style: { height: '46px', margin: '10px 0' } }))));
  view.append(root);
  let desc = LS.get('mv.chsort', 'desc') === 'desc';

  function draw(m) {
    setTitle(m.title);
    const chs = Array.isArray(m.chapters) ? m.chapters : [];
    const resume = pickResume(chs);
    const anyRead = chs.some(c => c.read || c.progress > 0);
    const refresh = h('button', { class: 'btn', type: 'button', onclick: async () => {
      refresh.classList.add('busy'); refresh.disabled = true;
      const before = chs.length;
      try {
        const d = await post('/api/manga/' + enc(id) + '/refresh');
        cache.set('/api/manga/' + enc(id), { data: d, json: JSON.stringify(d) });
        invalidate('/api/library');
        const added = (d.chapters || []).length - before;
        if (ctx.alive) { draw(d); toast(added > 0 ? added + ' new chapter' + (added > 1 ? 's' : '') : 'Up to date', 'ok'); }
      } catch (e) { toast(e.message, 'err'); refresh.classList.remove('busy'); refresh.disabled = false; }
    } }, icon('refresh'), 'Refresh');
    const list = h('div', { class: 'chlist' });
    const fillList = () => list.replaceChildren(...(desc ? chs.slice().reverse() : chs).map(chRow));
    const sortBtn = h('button', { class: 'sortbtn', type: 'button', 'aria-label': 'Change chapter order', onclick: () => {
      desc = !desc; LS.set('mv.chsort', desc ? 'desc' : 'asc'); sortLbl.textContent = desc ? 'Newest first' : 'Oldest first'; fillList();
    } }, icon('sort'));
    const sortLbl = h('span', null, desc ? 'Newest first' : 'Oldest first');
    sortBtn.append(sortLbl);
    const descEl = m.description ? h('p', { class: 'desc', tabindex: '0', role: 'button', 'aria-label': 'Description, tap to expand' }, m.description) : null;
    if (descEl) {
      const tog = () => descEl.classList.toggle('open');
      descEl.addEventListener('click', tog);
      descEl.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); tog(); } });
    }
    root.replaceChildren(
      h('div', { class: 'mhero' },
        h('div', { class: 'mhero-bg', 'aria-hidden': 'true' }, m.cover && h('img', { src: m.cover, alt: '' })),
        h('div', { class: 'mhero-in' },
          cover(m.cover, m.title),
          h('div', { class: 'mhero-meta' },
            h('h1', { class: 'mt' }, m.title),
            m.author && h('div', { class: 'muted' }, m.author),
            h('div', { class: 'chips' }, m.status && h('span', { class: 'chip' }, m.status), (m.genres || []).slice(0, 4).map(g => h('span', { class: 'chip' }, g)))))),
      h('div', { class: 'macts' },
        resume && h('a', { class: 'btn primary', href: '#/read/' + enc(resume.id) }, icon('play'),
          (anyRead ? 'Continue · ' : 'Start · ') + chLabel(resume).replace(/ · .*/, '')),
        refresh,
        h('button', { class: 'btn', type: 'button', onclick: () => post('/api/pc/open', { page: 'library', manga_id: id }).then(() => toast('Opened on your PC', 'ok'), e => toast(e.message, 'err')) }, icon('pc'), 'Open on PC'),
        h('a', { class: 'btn', href: '#/studio?manga=' + enc(id) }, icon('sparkle'), 'Studio')),
      descEl,
      h('div', { class: 'sec-row', style: { marginTop: '14px' } },
        h('h2', { class: 'sec-h', style: { marginBottom: '0' } }, 'Chapters', h('span', { class: 'count' }, String(chs.length))), sortBtn),
      chs.length ? list : h('p', { class: 'pad muted' }, 'No chapters yet. Tap Refresh to check the source.'));
    fillList();
  }
  swr(ctx, '/api/manga/' + enc(id), (d, fromCache) => { draw(d); if (fromCache) ctx.restoreScroll(); },
    e => root.replaceChildren(errorState(e, () => route())));
}
function chRow(c) {
  const pct = Math.round((c.progress || 0) * 100);
  const partial = !c.read && pct > 0 && pct < 100;
  return h('a', { class: 'chrow' + (c.read ? ' read' : ''), href: '#/read/' + enc(c.id) },
    h('div', { class: 'chrow-main' },
      h('div', { class: 'chrow-t' }, chLabel(c)),
      h('div', { class: 'chrow-s' }, [c.pages ? c.pages + ' pages' : '', c.read ? 'Read' : partial ? pct + '% read' : ''].filter(Boolean).join(' · ') || ' '),
      partial && h('div', { class: 'pbar sm' }, h('i', { style: { width: pct + '%' } }))),
    h('div', { class: 'chrow-b' },
      c.downloaded && h('span', { title: 'Downloaded', 'aria-label': 'Downloaded' }, icon('download')),
      c.audio && h('span', { class: 'b-ac', title: 'Narration audio', 'aria-label': 'Has narration audio' }, icon('wave')),
      c.video && h('span', { class: 'b-ac', title: 'Narrated video', 'aria-label': 'Has narrated video' }, icon('video'))));
}

/* ======================================================================
   Reader (vertical continuous scroll)
   ====================================================================== */
async function viewReader(ctx, chId) {
  document.documentElement.classList.add('reading');
  ctx.onLeave(() => document.documentElement.classList.remove('reading'));
  const root = h('div', { class: 'reader' }, h('div', { class: 'state' }, h('div', { class: 'btn ghost busy', 'aria-label': 'Loading chapter' })));
  view.append(root);

  let ch;
  try { ch = await api('/api/chapter/' + enc(chId)); }
  catch (e) {
    if (!ctx.alive) return;
    root.replaceChildren(errorState(e, () => route()),
      h('div', { class: 'state' }, h('a', { class: 'btn', href: '#/library' }, 'Back to Library')));
    return;
  }
  if (!ctx.alive) return;
  const n = Math.max(0, ch.pages | 0);
  const mangaHref = ch.manga_id != null ? '#/manga/' + enc(ch.manga_id) : '#/library';
  let mangaTitle = '';
  const hit = ch.manga_id != null && cache.get('/api/manga/' + enc(ch.manga_id));
  if (hit) mangaTitle = hit.data.title;
  setTitle(chLabel(ch));

  const pageUrl = (id, i) => '/api/chapter/' + enc(id) + '/page/' + i;
  const slots = [];
  const list = h('div', { class: 'r-pages' });
  for (let i = 0; i < n; i++) {
    const s = h('div', { class: 'rp', 'data-i': String(i) }, h('div', { class: 'rp-n', 'aria-hidden': 'true' }, String(i + 1)));
    slots.push(s); list.append(s);
  }
  let ratio = 1.45;

  function load(i) {
    const s = slots[i];
    if (!s || s._state) return;
    s._state = 'loading';
    const img = new Image();
    img.decoding = 'async';
    img.alt = 'Page ' + (i + 1);
    img.onload = () => {
      if (!ctx.alive) return;
      const r0 = s.getBoundingClientRect();
      const above = r0.bottom <= 2;
      const oldH = r0.height;
      s.replaceChildren(img);
      s.style.aspectRatio = '';
      s.classList.add('ok');
      s._state = 'ok';
      if (img.naturalWidth) {
        ratio = img.naturalHeight / img.naturalWidth;
        for (let k = Math.max(i, cur) + 1; k < n; k++) if (!slots[k]._state) slots[k].style.aspectRatio = '1 / ' + ratio.toFixed(3);
      }
      if (above) { const dh = s.getBoundingClientRect().height - oldH; if (dh) scrollBy(0, dh); }
    };
    img.onerror = () => {
      if (!ctx.alive) return;
      s._state = 'err';
      s.replaceChildren(h('div', { class: 'rp-err' }, 'Page ' + (i + 1) + ' failed to load',
        h('button', { class: 'btn', type: 'button', onclick: ev => { ev.stopPropagation(); s._state = null; s.replaceChildren(h('div', { class: 'rp-n' }, String(i + 1))); load(i); } }, icon('refresh'), 'Retry')));
    };
    img.src = pageUrl(chId, i);
  }
  const io = new IntersectionObserver(entries => {
    for (const e of entries) if (e.isIntersecting) { const i = +e.target.dataset.i; for (let k = i; k <= i + 3; k++) load(k); }
  }, { rootMargin: '60% 0px 60% 0px' });
  slots.forEach(s => io.observe(s));
  ctx.onLeave(() => io.disconnect());

  // ----- overlay -----
  const pgLbl = h('span', { class: 'r-pg', 'aria-live': 'off' }, '1 / ' + n);
  const slider = h('input', { type: 'range', min: '1', max: String(Math.max(1, n)), value: '1', step: '1', 'aria-label': 'Page' });
  const ttlB = h('b', null, chLabel(ch));
  const ttlS = h('span', null, mangaTitle || ch.title || '');
  const go = id => location.replace('#/read/' + enc(id));
  const prevBtn = iconBtn('prev', 'Previous chapter', () => ch.prev != null && go(ch.prev));
  const nextBtn = iconBtn('next', 'Next chapter', () => ch.next != null && go(ch.next));
  prevBtn.disabled = ch.prev == null; nextBtn.disabled = ch.next == null;
  const top = h('div', { class: 'r-ov r-top' },
    iconBtn('back', 'Back to chapters', () => goBack(mangaHref)),
    h('div', { class: 'r-ttl' }, ttlB, ttlS),
    iconBtn('pc', 'Open this manga on PC', () => post('/api/pc/open', { page: 'library', manga_id: ch.manga_id }).then(() => toast('Opened on your PC', 'ok'), e => toast(e.message, 'err'))));
  const bot = h('div', { class: 'r-ov r-bot' }, prevBtn, slider, pgLbl, nextBtn);
  const end = h('div', { class: 'r-end' },
    h('h2', null, 'End of ' + chLabel(ch)),
    ch.next != null
      ? h('button', { class: 'btn primary', type: 'button', onclick: () => go(ch.next) }, 'Next chapter', icon('chev'))
      : h('p', { class: 'muted' }, 'You’re all caught up.'),
    h('a', { class: 'btn ghost', href: mangaHref }, 'Back to chapters'));
  root.replaceChildren(top, n ? list : h('div', { class: 'state' }, icon('book'), h('h2', null, 'No pages'), h('p', null, 'This chapter has no pages yet. Download it on the PC or try again.')), end, bot);

  let hideT;
  const showOv = (on) => {
    root.classList.toggle('hide-ov', !on);
    clearTimeout(hideT);
    if (on) hideT = setTimeout(() => root.classList.add('hide-ov'), 3500);
  };
  root.addEventListener('click', e => {
    if (e.target.closest('.r-ov, button, a')) { if (e.target.closest('.r-ov')) showOv(true); return; }
    showOv(root.classList.contains('hide-ov'));
  });
  showOv(true);
  ctx.onLeave(() => clearTimeout(hideT));

  if (!mangaTitle && ch.manga_id != null) {
    getManga(ch.manga_id).then(m => { if (ctx.alive && m && m.title) { ttlS.textContent = m.title; } }).catch(() => {});
  }

  // ----- current page + progress -----
  let cur = Math.min(Math.max(0, ch.start | 0), Math.max(0, n - 1));
  let sliding = false, saveT = null, saved = -1, preloaded = false;
  const preloadImgs = [];
  function save(now) {
    clearTimeout(saveT);
    const doSave = () => {
      if (!n || cur === saved) return;
      saved = cur;
      post('/api/chapter/' + enc(chId) + '/progress', { page: cur, total: n }, { keepalive: true })
        .catch(() => { saved = -1; });
    };
    if (now) doSave(); else saveT = setTimeout(doSave, 1200);
  }
  function updateUi() {
    pgLbl.textContent = (cur + 1) + ' / ' + n;
    if (!sliding) { slider.value = String(cur + 1); setRange(slider); }
  }
  function computeCur() {
    if (!n) return;
    const y = innerHeight * 0.35;
    let lo = 0, hi = n - 1, ans = 0;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (slots[mid].getBoundingClientRect().top <= y) { ans = mid; lo = mid + 1; } else hi = mid - 1;
    }
    if (end.getBoundingClientRect().top < innerHeight) ans = n - 1;
    if (ans !== cur) { cur = ans; updateUi(); save(false); }
    if (!preloaded && ch.next != null && cur >= n - 3) {
      preloaded = true;
      api('/api/chapter/' + enc(ch.next)).then(nx => {
        for (let i = 0; i < Math.min(3, nx.pages | 0); i++) { const im = new Image(); im.src = pageUrl(ch.next, i); preloadImgs.push(im); }
      }).catch(() => { preloaded = false; });
    }
  }
  let raf = 0;
  const onScroll = () => { if (!raf) raf = requestAnimationFrame(() => { raf = 0; computeCur(); }); };
  addEventListener('scroll', onScroll, { passive: true });
  ctx.onLeave(() => { removeEventListener('scroll', onScroll); cancelAnimationFrame(raf); save(true); });

  slider.addEventListener('input', () => { sliding = true; pgLbl.textContent = slider.value + ' / ' + n; setRange(slider); showOv(true); });
  slider.addEventListener('change', () => {
    sliding = false;
    const i = (+slider.value || 1) - 1;
    for (let k = i; k <= i + 2; k++) load(k);
    slots[i] && slots[i].scrollIntoView({ block: 'start' });
    cur = i; updateUi(); save(false);
  });

  // ----- wake lock -----
  let lock = null;
  const reqLock = async () => {
    try { if ('wakeLock' in navigator && document.visibilityState === 'visible') lock = await navigator.wakeLock.request('screen'); }
    catch { lock = null; }
  };
  const onVis = () => { if (document.visibilityState === 'visible') reqLock(); else save(true); };
  document.addEventListener('visibilitychange', onVis);
  reqLock();
  ctx.onLeave(() => { document.removeEventListener('visibilitychange', onVis); if (lock) lock.release().catch(() => {}); lock = null; });

  // ----- resume -----
  updateUi();
  saved = cur;
  if (n) {
    for (let k = cur; k <= cur + 3; k++) load(k);
    if (cur > 0) slots[cur].scrollIntoView({ block: 'start' });
  }
}

/* ======================================================================
   Studio
   ====================================================================== */
function viewStudio(ctx, q) {
  setTitle('Studio');
  const focusManga = q.get('manga');
  const jobsBox = h('section', { class: 'jobs', 'aria-label': 'Jobs', 'aria-live': 'polite' });
  const body = h('div', null, h('div', { class: 'skel', style: { height: '62px', margin: '8px 16px 14px' } }),
    Array.from({ length: 4 }, () => h('div', { class: 'skel', style: { height: '64px', margin: '0 16px 10px' } })));
  view.append(h('div', { class: 'page' }, jobsBox, body));
  const selected = new Set();
  let what = LS.get('mv.studio.what', 'video');
  let data = null;

  // ----- jobs -----
  let pollT = null;
  function jobRow(j) {
    const st = String(j.state || 'queued');
    const active = st === 'queued' || st === 'running';
    const pct = Math.round(Math.min(1, Math.max(0, +j.progress || 0)) * 100);
    return h('div', { class: 'job' },
      h('div', { class: 'job-h' },
        h('div', { class: 'job-t' }, j.title || 'Job'),
        h('span', { class: 'state-chip ' + st }, st),
        active && iconBtn('close', 'Cancel ' + (j.title || 'job'), async ev => {
          ev.currentTarget.disabled = true;
          try { await post('/api/studio/jobs/' + enc(j.id) + '/cancel'); toast('Cancelled', 'ok'); poll(); }
          catch (e) { toast(e.message, 'err'); }
        })),
      h('div', { class: 'job-s' }, h('span', null, j.stage || (st === 'queued' ? 'Waiting…' : '')), active && st === 'running' && h('span', null, pct + '%')),
      (active || st === 'done') && h('div', { class: 'pbar' + (st === 'queued' ? ' indet' : '') }, h('i', { style: { width: (st === 'done' ? 100 : pct) + '%' } })),
      j.error && h('div', { class: 'job-e' }, j.error));
  }
  function drawJobs(jobs) {
    jobs = Array.isArray(jobs) ? jobs : [];
    const order = { running: 0, queued: 1, failed: 2, done: 3, cancelled: 4 };
    jobs = jobs.slice().sort((a, b) => (order[a.state] ?? 5) - (order[b.state] ?? 5)).slice(0, 20);
    jobsBox.replaceChildren(...(jobs.length ? [h('h2', { class: 'sec-h', style: { margin: '6px 0 4px' } }, 'Jobs',
      h('span', { class: 'count' }, jobs.filter(j => j.state === 'running' || j.state === 'queued').length + ' active'))] : []), ...jobs.map(jobRow));
  }
  async function poll() {
    clearTimeout(pollT);
    if (!ctx.alive) return;
    if (document.visibilityState === 'visible') {
      try { drawJobs(await api('/api/studio/jobs')); } catch { /* keep last */ }
    }
    if (ctx.alive) pollT = setTimeout(poll, 2000);
  }
  const onVis = () => { if (document.visibilityState === 'visible') poll(); };
  document.addEventListener('visibilitychange', onVis);
  ctx.onLeave(() => { clearTimeout(pollT); document.removeEventListener('visibilitychange', onVis); });
  poll();

  // ----- generate -----
  const genBtn = h('button', { class: 'btn primary block', type: 'button', onclick: generate });
  const hint = h('div', { class: 'hint' });
  const HINTS = {
    audio: 'Narration audio for each selected chapter.',
    video: 'A narrated recap video for each selected chapter.',
    full: 'Full Read → Video pipeline, including the joined recap.',
  };
  function updateGen() {
    const n = selected.size;
    genBtn.replaceChildren(icon('sparkle'), n ? 'Generate · ' + n + ' chapter' + (n > 1 ? 's' : '') : 'Select chapters to generate');
    genBtn.disabled = !n || !(data && data.ready);
    hint.textContent = HINTS[what] || '';
  }
  async function generate() {
    if (!selected.size) return;
    genBtn.classList.add('busy'); genBtn.disabled = true;
    try {
      const r = await post('/api/studio/generate', { chapters: [...selected], what });
      const k = (r.jobs || []).length;
      toast(k ? 'Queued ' + k + ' job' + (k > 1 ? 's' : '') + ' on your PC' : 'Sent to your PC', 'ok');
      selected.clear();
      drawManga();
      poll();
      scrollTo({ top: 0, behavior: 'smooth' });
    } catch (e) { toast(e.message, 'err'); }
    genBtn.classList.remove('busy');
    updateGen();
  }

  const open = new Set(focusManga ? [focusManga] : []);
  function mangaBlock(m) {
    const chs = Array.isArray(m.chapters) ? m.chapters : [];
    const box = h('div', { class: 'smanga' + (open.has(String(m.id)) ? ' open' : '') });
    const cnt = h('span', { class: 'selcount', hidden: true });
    const allBox = h('input', { type: 'checkbox', 'aria-label': 'Select all chapters of ' + m.title });
    const boxes = [];
    const sync = () => {
      const k = chs.filter(c => selected.has(c.id)).length;
      cnt.hidden = !k; cnt.textContent = String(k);
      allBox.checked = k > 0 && k === chs.length;
      allBox.indeterminate = k > 0 && k < chs.length;
      updateGen();
    };
    const head = h('button', { class: 'smanga-h', type: 'button', 'aria-expanded': String(open.has(String(m.id))), onclick: () => {
      const on = !box.classList.contains('open');
      box.classList.toggle('open', on); head.setAttribute('aria-expanded', String(on));
      if (on) open.add(String(m.id)); else open.delete(String(m.id));
      list.hidden = !on;
    } },
      cover(m.cover, m.title),
      h('div', { class: 'smanga-t' }, h('b', null, m.title),
        h('span', null, chs.length + ' downloaded · ' + chs.filter(c => c.audio).length + ' audio · ' + chs.filter(c => c.video).length + ' video')),
      cnt, icon('chev', 'chev'));
    allBox.addEventListener('change', () => {
      for (const c of chs) { if (allBox.checked) selected.add(c.id); else selected.delete(c.id); }
      boxes.forEach(b => { b.checked = allBox.checked; });
      sync();
    });
    const list = h('div', { class: 'sch', hidden: !open.has(String(m.id)) },
      chs.length > 1 && h('label', { class: 'crow all' }, allBox, h('span', { class: 't' }, 'Select all')),
      chs.map(c => {
        const cb = h('input', { type: 'checkbox', checked: selected.has(c.id) });
        cb.addEventListener('change', () => { if (cb.checked) selected.add(c.id); else selected.delete(c.id); sync(); });
        boxes.push(cb);
        return h('label', { class: 'crow' }, cb, h('span', { class: 't' }, chLabel(c)),
          c.audio && h('span', { title: 'Has narration audio', 'aria-label': 'Has narration audio' }, icon('wave')),
          c.video && h('span', { title: 'Has narrated video', 'aria-label': 'Has narrated video' }, icon('video')));
      }));
    box.append(head, list);
    sync();
    return box;
  }
  const mangaWrap = h('div');
  function drawManga() {
    const ms = (data && data.manga) || [];
    mangaWrap.replaceChildren(...ms.map(mangaBlock));
    if (!ms.length) mangaWrap.append(stateBox('download', 'No downloaded chapters', 'Download chapters on your PC first. Studio works from downloaded chapters.'));
  }
  function draw(d) {
    data = d || {};
    const ms = data.manga || [];
    if (!open.size && ms.length) open.add(String(ms[0].id));
    body.replaceChildren(
      data.ready
        ? h('div', { class: 'banner' }, icon('check'), h('div', null, h('b', null, 'Studio is ready'), 'Pick chapters and what to make. Jobs run on your PC.'))
        : h('div', { class: 'banner warn' }, icon('alert'), h('div', null, h('b', null, 'Studio isn’t ready'), data.reason || 'Finish Studio setup on your PC first.')),
      h('h2', { class: 'sec-h' }, 'Chapters'),
      mangaWrap,
      ms.length ? h('div', { class: 'genbar' },
        segmented([['audio', 'Narration audio'], ['video', 'Narrated video'], ['full', 'Full recap']], what, v => { what = v; LS.set('mv.studio.what', v); updateGen(); }, 'What to generate'),
        genBtn, hint) : null);
    drawManga();
    updateGen();
    if (focusManga) {
      const el = [...mangaWrap.children].find(c => c.classList.contains('open'));
      if (el) setTimeout(() => el.scrollIntoView({ block: 'start', behavior: 'smooth' }), 60);
    }
  }
  const refreshBtn = iconBtn('refresh', 'Refresh Studio', () => {
    refreshBtn.classList.add('busy');
    api('/api/studio').then(d => ctx.alive && draw(d), e => toast(e.message, 'err')).finally(() => refreshBtn.classList.remove('busy'));
  });
  setActions(refreshBtn);
  api('/api/studio').then(d => ctx.alive && draw(d), e => ctx.alive && e.status !== 401 && body.replaceChildren(errorState(e, () => route())));
}

/* ======================================================================
   Listen: media list, mini player, video player
   ====================================================================== */
const posKey = id => 'mv.pos.' + id;
function coverForManga(name) {
  const lib = cache.get('/api/library');
  if (!lib || !name) return null;
  const m = lib.data.find(x => x.title === name);
  return m && m.cover ? new URL(m.cover, location.href).href : null;
}

function createPlayer() {
  const root = document.getElementById('player');
  const audio = new Audio();
  audio.preload = 'metadata';
  let queue = [], idx = -1, lastSave = 0, seeking = false, lastPos = 0;
  const RATES = [1, 1.25, 1.5, 2];
  let rate = RATES.includes(LS.get('mv.rate', 1)) ? LS.get('mv.rate', 1) : 1;
  const listeners = new Set();
  const tEl = h('div', { class: 'pl-t' }), sEl = h('div', { class: 'pl-s' });
  const playBtn = h('button', { class: 'icon-btn pl-play', type: 'button', 'aria-label': 'Play', onclick: () => toggle() }, icon('play'));
  const seek = h('input', { type: 'range', min: '0', max: '1000', value: '0', step: '1', 'aria-label': 'Seek' });
  const curT = h('span', { class: 'pl-time' }, '0:00'), durT = h('span', { class: 'pl-time' }, '0:00');
  const rateBtn = h('button', { class: 'pl-rate', type: 'button', 'aria-label': 'Playback speed', onclick: () => {
    rate = RATES[(RATES.indexOf(rate) + 1) % RATES.length];
    LS.set('mv.rate', rate); audio.playbackRate = rate; audio.defaultPlaybackRate = rate; rateBtn.textContent = rate + '×';
  } }, rate + '×');
  const skipBtn = (d) => h('button', { class: 'icon-btn skip', type: 'button', 'aria-label': (d < 0 ? 'Back ' : 'Forward ') + Math.abs(d) + ' seconds', onclick: () => skip(d) },
    icon(d < 0 ? 'rotl' : 'rotr'), h('b', { 'aria-hidden': 'true' }, String(Math.abs(d))));
  root.append(
    h('div', { class: 'pl-row' }, h('div', { class: 'pl-info' }, tEl, sEl), skipBtn(-15), playBtn, skipBtn(15), rateBtn,
      h('button', { class: 'icon-btn', type: 'button', 'aria-label': 'Close player', onclick: () => stop() }, icon('close'))),
    h('div', { class: 'pl-row2' }, curT, seek, durT));

  const cur = () => queue[idx] || null;
  const emit = () => listeners.forEach(f => { try { f(cur()); } catch { /* ignore */ } });
  function savePos(force) {
    const it = cur(); if (!it || !isFinite(audio.duration)) return;
    const now = Date.now();
    if (!force && now - lastSave < 4000) return;
    lastSave = now;
    if (audio.currentTime > 3 && audio.currentTime < audio.duration - 5) LS.set(posKey(it.id), { t: audio.currentTime, d: audio.duration });
  }
  function setIcon() {
    const playing = !audio.paused;
    playBtn.replaceChildren(icon(playing ? 'pause' : 'play'));
    playBtn.setAttribute('aria-label', playing ? 'Pause' : 'Play');
    if ('mediaSession' in navigator) navigator.mediaSession.playbackState = playing ? 'playing' : 'paused';
  }
  function ui() {
    const d = audio.duration;
    if (!seeking) {
      seek.value = isFinite(d) && d > 0 ? String(Math.round((audio.currentTime / d) * 1000)) : '0';
      setRange(seek);
      curT.textContent = fmtTime(audio.currentTime);
    }
    durT.textContent = isFinite(d) ? fmtTime(d) : '–:––';
  }
  function session(it) {
    if (!('mediaSession' in navigator)) return;
    try {
      const art = coverForManga(it.manga) || new URL('/icon.png', location.href).href;
      navigator.mediaSession.metadata = new MediaMetadata({
        title: it.title || 'Narration', artist: it.manga || 'MangaVox',
        album: it.chapter != null && it.chapter !== '' ? 'Chapter ' + it.chapter : 'MangaVox Studio',
        artwork: [{ src: art, sizes: '512x512' }],
      });
    } catch { /* unsupported */ }
  }
  function posState() {
    if (!('mediaSession' in navigator) || !navigator.mediaSession.setPositionState) return;
    if (!isFinite(audio.duration) || audio.duration <= 0) return;
    try { navigator.mediaSession.setPositionState({ duration: audio.duration, position: Math.min(audio.currentTime, audio.duration), playbackRate: audio.playbackRate }); } catch { /* ignore */ }
  }
  function load(i) {
    savePos(true);
    idx = i;
    const it = cur(); if (!it) return stop();
    tEl.textContent = it.title || 'Narration';
    sEl.textContent = [it.manga, it.chapter != null && it.chapter !== '' ? 'Ch. ' + it.chapter : ''].filter(Boolean).join(' · ');
    audio.src = it.url;
    audio.playbackRate = rate; audio.defaultPlaybackRate = rate;
    root.hidden = false;
    document.body.classList.add('has-player');
    session(it);
    seek.value = '0'; setRange(seek); curT.textContent = '0:00'; durT.textContent = '–:––';
    audio.play().catch(() => setIcon());
    emit();
  }
  function toggle() { if (!cur()) return; if (audio.paused) audio.play().catch(e => toast('Can’t play: ' + e.message, 'err')); else audio.pause(); }
  function skip(d) { if (!isFinite(audio.duration)) return; audio.currentTime = Math.max(0, Math.min(audio.duration - 0.25, audio.currentTime + d)); ui(); posState(); }
  function stop() {
    savePos(true);
    audio.pause(); audio.removeAttribute('src'); audio.load();
    queue = []; idx = -1;
    root.hidden = true;
    document.body.classList.remove('has-player');
    if ('mediaSession' in navigator) { navigator.mediaSession.metadata = null; navigator.mediaSession.playbackState = 'none'; }
    emit();
  }
  audio.addEventListener('loadedmetadata', () => {
    const it = cur();
    const p = it && LS.get(posKey(it.id), null);
    if (p && p.t > 3 && p.t < audio.duration - 8) audio.currentTime = p.t;
    ui(); posState();
  });
  audio.addEventListener('timeupdate', () => {
    ui(); savePos(false);
    if (Math.abs(audio.currentTime - lastPos) > 5) { lastPos = audio.currentTime; posState(); }
  });
  audio.addEventListener('play', setIcon);
  audio.addEventListener('pause', () => { setIcon(); savePos(true); });
  audio.addEventListener('ended', () => {
    const it = cur(); if (it) LS.del(posKey(it.id));
    if (idx + 1 < queue.length) load(idx + 1); else { setIcon(); emit(); }
  });
  audio.addEventListener('error', () => { if (audio.getAttribute('src')) { toast('Couldn’t play this file', 'err'); setIcon(); } });
  seek.addEventListener('input', () => {
    seeking = true; setRange(seek);
    if (isFinite(audio.duration)) curT.textContent = fmtTime((+seek.value / 1000) * audio.duration);
  });
  seek.addEventListener('change', () => {
    if (isFinite(audio.duration)) audio.currentTime = (+seek.value / 1000) * audio.duration;
    seeking = false; ui(); posState();
  });
  addEventListener('pagehide', () => savePos(true));
  if ('mediaSession' in navigator) {
    const ms = navigator.mediaSession;
    const set = (a, f) => { try { ms.setActionHandler(a, f); } catch { /* unsupported */ } };
    set('play', () => audio.play());
    set('pause', () => audio.pause());
    set('seekbackward', d => skip(-(d && d.seekOffset || 15)));
    set('seekforward', d => skip(d && d.seekOffset || 15));
    set('seekto', d => { if (d && isFinite(d.seekTime)) { audio.currentTime = d.seekTime; ui(); posState(); } });
    set('previoustrack', () => { if (audio.currentTime > 5 || idx === 0) audio.currentTime = 0; else load(idx - 1); });
    set('nexttrack', () => { if (idx + 1 < queue.length) load(idx + 1); });
  }
  return {
    play(items, i) { queue = items.slice(); load(i); },
    pause() { audio.pause(); },
    current: cur,
    playing: () => !audio.paused,
    on(f) { listeners.add(f); return () => listeners.delete(f); },
  };
}
let Player = null;

function openVideo(item) {
  Player.pause();
  openSheet((p, s) => {
    const v = h('video', { controls: true, playsinline: true, 'webkit-playsinline': true, preload: 'metadata', autoplay: true, src: item.url, 'aria-label': item.title || 'Video' });
    let last = 0;
    const save = () => { if (isFinite(v.duration) && v.currentTime > 3 && v.currentTime < v.duration - 5) LS.set(posKey(item.id), { t: v.currentTime, d: v.duration }); };
    v.addEventListener('loadedmetadata', () => { const pos = LS.get(posKey(item.id), null); if (pos && pos.t < v.duration - 8) v.currentTime = pos.t; });
    v.addEventListener('timeupdate', () => { const now = Date.now(); if (now - last > 4000) { last = now; save(); } });
    v.addEventListener('pause', save);
    v.addEventListener('ended', () => LS.del(posKey(item.id)));
    v.addEventListener('error', () => { if (!s.closed) toast('Couldn’t play this video', 'err'); });
    s.onClose.push(() => { save(); v.pause(); v.removeAttribute('src'); v.load(); });
    p.append(
      h('div', { class: 'vp-top' }, iconBtn('back', 'Close video', () => s.close()), h('b', null, item.title || 'Video'),
        h('a', { class: 'icon-btn', href: dlUrl(item.url), download: '', 'aria-label': 'Download video' }, icon('download'))),
      h('div', { class: 'vp-box' }, v));
    v.play().catch(() => { /* user can tap play */ });
  }, { cls: 'video', label: item.title || 'Video player' });
}
const dlUrl = u => String(u).startsWith('/api/') ? u + (String(u).includes('?') ? '&' : '?') + 'download=1' : '#';

function viewListen(ctx) {
  setTitle('Listen');
  const root = h('div', { class: 'page' },
    Array.from({ length: 6 }, () => h('div', { class: 'skel', style: { height: '56px', margin: '10px 16px' } })));
  view.append(root);
  let filter = LS.get('mv.listen.filter', 'all');
  let items = [];
  const listBox = h('div');
  const rows = new Map();
  function markPlaying(it) {
    for (const [id, r] of rows) r.classList.toggle('playing', !!it && it.id === id);
  }
  ctx.onLeave(Player.on(markPlaying));

  function row(it, playlist) {
    const isVideo = it.kind === 'video';
    const pos = LS.get(posKey(it.id), null);
    const pct = pos && pos.d ? Math.round((pos.t / pos.d) * 100) : 0;
    const r = h('div', { class: 'mrow' },
      h('button', { class: 'mrow-main', type: 'button', 'aria-label': (isVideo ? 'Watch ' : 'Play ') + (it.title || ''), onclick: () => {
        if (isVideo) openVideo(it);
        else { const audios = playlist.filter(x => x.kind !== 'video'); Player.play(audios, audios.indexOf(it)); }
      } },
        h('div', { class: 'mrow-ic' + (isVideo ? ' video' : '') }, icon(isVideo ? 'video' : 'wave')),
        h('div', { class: 'mrow-b' },
          h('div', { class: 'mrow-t' }, it.title || 'Untitled'),
          h('div', { class: 'mrow-s' }, [isVideo ? 'Video' : 'Audio', it.chapter != null && it.chapter !== '' ? 'Ch. ' + it.chapter : '', fmtSize(it.size), ago(it.modified)].filter(Boolean).join(' · ')),
          pct > 0 && h('div', { class: 'pbar sm' }, h('i', { style: { width: pct + '%' } })))),
      h('a', { class: 'icon-btn', href: dlUrl(it.url), download: '', 'aria-label': 'Download ' + (it.title || ''), title: 'Download' }, icon('download')));
    rows.set(it.id, r);
    return r;
  }
  function fill() {
    rows.clear();
    const shown = items.filter(it => filter === 'all' || it.kind === filter);
    const groups = new Map();
    for (const it of shown) { const g = it.manga || 'Other'; if (!groups.has(g)) groups.set(g, []); groups.get(g).push(it); }
    const playlist = [...groups.values()].flat();
    listBox.replaceChildren();
    for (const [g, list] of groups) listBox.append(h('h2', { class: 'grp-h' }, g), ...list.map(it => row(it, playlist)));
    if (!shown.length) listBox.append(stateBox(filter === 'video' ? 'video' : 'headphones', 'Nothing here yet',
      'Generate narration audio or videos in Studio and they show up here.', h('a', { class: 'btn', href: '#/studio' }, icon('sparkle'), 'Open Studio')));
    markPlaying(Player.current());
  }
  function draw(d) {
    items = Array.isArray(d) ? d : [];
    root.replaceChildren(
      h('div', { class: 'filters' }, segmented([['all', 'All'], ['audio', 'Audio'], ['video', 'Video']], filter, v => { filter = v; LS.set('mv.listen.filter', v); fill(); }, 'Filter media')),
      listBox);
    fill();
  }
  const refreshBtn = iconBtn('refresh', 'Refresh', () => { refreshBtn.classList.add('busy'); load().finally(() => refreshBtn.classList.remove('busy')); });
  setActions(refreshBtn);
  const load = () => swr(ctx, '/api/media', (d, fromCache) => { draw(d); if (fromCache) ctx.restoreScroll(); }, e => root.replaceChildren(errorState(e, () => route())));
  load();
}

/* ======================================================================
   Movies
   ====================================================================== */
let moviesTab = 'home';
let moviesQuery = '';
async function playOnPc(body) {
  toast('Finding sources on your PC…', 'info', 2200);
  try { await post('/api/movies/play', body); toast('Playing on your PC: ' + (body.title || ''), 'ok', 3200); }
  catch (e) { toast(e.message, 'err', 4000); }
}
const openOnPc = (it) => post('/api/pc/open', { page: 'movies', imdb: it.imdb || it.id, kind: it.kind }).then(() => toast('Opened on your PC', 'ok'), e => toast(e.message, 'err'));
const itemKey = it => it.imdb || it.id;
function movieMeta(it, extra) {
  const score = Number(it.score);
  return h('div', { class: 'meta' },
    score > 0 && h('span', { class: 'star' }, icon('star'), score.toFixed(1)),
    it.year && h('span', null, String(it.year)),
    extra && extra.runtime && h('span', null, fmtRuntime(extra.runtime)),
    h('span', null, it.kind === 'series' ? 'Series' : 'Movie'),
    (it.genres || []).slice(0, 3).length > 0 && h('span', null, (it.genres || []).slice(0, 3).join(' · ')));
}
function titleOrLogo(it, cls, tag = 'h2') {
  const t = h(tag, null, it.title || '');
  if (!it.logo) return t;
  const img = h('img', { class: cls || 'logo', src: it.logo, alt: it.title || '', referrerpolicy: 'no-referrer' });
  img.addEventListener('error', () => img.replaceWith(t), { once: true });
  return img;
}
function card16(it) {
  return h('button', { class: 'card16', type: 'button', 'aria-label': it.title || 'Title', onclick: () => openMovie(it) },
    webImg(it.background || it.poster, ''),
    h('div', { class: 'cl' }, it.logo ? titleOrLogo(it, '', 'b') : h('b', null, it.title || ''), it.year && h('span', null, String(it.year))));
}
function top10Card(it, i) {
  return h('button', { class: 'top10', type: 'button', 'aria-label': '#' + (i + 1) + ' ' + (it.title || ''), onclick: () => openMovie(it) },
    h('span', { class: 'num', 'aria-hidden': 'true' }, String(i + 1)), cover(it.poster || it.background, it.title));
}

function viewMovies(ctx) {
  setTitle('Movies');
  const content = h('div');
  const searchIn = h('input', { type: 'search', placeholder: 'Search movies & series', 'aria-label': 'Search movies and series', value: moviesQuery, enterkeyhint: 'search' });
  const top = h('div', { class: 'mv-top' },
    segmented([['home', 'Home'], ['movies', 'Movies'], ['series', 'Series']], moviesTab, v => { moviesTab = v; moviesQuery = ''; searchIn.value = ''; load(); }, 'Catalog'));
  const searchBar = h('label', { class: 'search' }, icon('search'), searchIn);
  view.append(h('div', { class: 'page' }, searchBar, top, content));
  const skel = () => h('div', null, h('div', { class: 'hero skel' }),
    [0, 1].map(() => h('div', { class: 'sec' }, h('div', { class: 'skel skel-line', style: { width: '40%', margin: '0 16px 10px' } }),
      h('div', { class: 'hscroll' }, [0, 1, 2].map(() => h('div', { class: 'card16 skel' }))))));

  function hero(it) {
    return h('div', { class: 'hero' },
      webImg(it.background || it.poster, ''),
      h('div', { class: 'hero-in' },
        titleOrLogo(it),
        movieMeta(it),
        it.description && h('p', null, it.description),
        h('div', { class: 'hero-acts' },
          h('button', { class: 'btn primary', type: 'button', onclick: () => playOnPc({ imdb: itemKey(it), kind: it.kind, title: it.title }) }, icon('play'), 'Play on PC'),
          h('button', { class: 'btn', type: 'button', onclick: () => openMovie(it) }, icon('info'), 'More info'))));
  }
  function drawHome(d) {
    const rows = (d && d.rows) || [];
    const heroItem = (d && d.hero) || (rows[0] && rows[0].items && rows[0].items[0]);
    content.replaceChildren(
      heroItem ? hero(heroItem) : null,
      ...rows.filter(r => r.items && r.items.length).map(r => h('section', { class: 'sec', 'aria-label': r.title },
        h('h2', { class: 'sec-h' }, r.title),
        h('div', { class: 'hscroll' }, r.top10 ? r.items.slice(0, 10).map(top10Card) : r.items.map(card16)))));
    if (!heroItem && !rows.length) content.append(stateBox('film', 'Nothing to show', 'The catalog is empty right now. Check your sources on the PC.'));
  }
  let searchSeq = 0;
  const doSearch = debounce(async () => {
    const q = searchIn.value.trim();
    moviesQuery = q;
    if (!q) { top.hidden = false; load(); return; }
    top.hidden = true;
    const seq = ++searchSeq;
    content.replaceChildren(h('div', { class: 'sgrid' }, [0, 1, 2, 3].map(() => h('div', { class: 'card16 skel' }))));
    try {
      const res = await api('/api/movies/search?q=' + enc(q));
      if (!ctx.alive || seq !== searchSeq) return;
      const list = Array.isArray(res) ? res : [];
      content.replaceChildren(list.length
        ? h('div', null, h('h2', { class: 'sec-h' }, 'Results', h('span', { class: 'count' }, String(list.length))), h('div', { class: 'sgrid' }, list.map(card16)))
        : stateBox('search', 'No results', 'Nothing matched “' + q + '”.'));
    } catch (e) { if (ctx.alive && seq === searchSeq) content.replaceChildren(errorState(e, doSearch)); }
  }, 350);
  searchIn.addEventListener('input', doSearch);
  searchIn.addEventListener('keydown', e => { if (e.key === 'Enter') searchIn.blur(); });

  function load() {
    searchSeq++;
    top.hidden = false;
    const path = '/api/movies?tab=' + enc(moviesTab);
    if (!cache.get(path)) content.replaceChildren(skel());
    return swr(ctx, path, (d, fromCache) => { if (!moviesQuery) { drawHome(d); if (fromCache) ctx.restoreScroll(); } }, e => content.replaceChildren(errorState(e, load)));
  }
  if (moviesQuery) doSearch(); else load();
}

function openMovie(it) {
  openSheet((p, s) => {
    const body = h('div', { class: 'md' });
    p.append(iconBtn('close', 'Close', () => s.close(), 'sheet-x'), body);
    let season = null;
    function draw(d, full) {
      const seasons = Array.isArray(d.seasons) ? d.seasons.filter(x => x && Array.isArray(x.episodes)) : [];
      if (season == null && seasons.length) season = (seasons.find(x => +x.season > 0) || seasons[0]).season;
      const eps = h('div');
      const fillEps = () => {
        const se = seasons.find(x => x.season === season);
        eps.replaceChildren(...((se && se.episodes) || []).map(ep => h('button', { class: 'ep', type: 'button', 'aria-label': 'Play episode ' + ep.episode + ' on PC',
          onclick: () => playOnPc({ imdb: itemKey(d), kind: 'series', title: d.title + ' S' + season + 'E' + ep.episode, season: +season, episode: +ep.episode }) },
          h('div', { class: 'ep-th' }, webImg(ep.thumbnail, ''), icon('play')),
          h('div', { class: 'ep-b' }, h('b', null, ep.episode + '. ' + (ep.title || 'Episode ' + ep.episode)), ep.overview && h('span', null, ep.overview)))));
      };
      const chips = seasons.map(x => {
        const b = h('button', { class: 'chip', type: 'button', 'aria-pressed': String(x.season === season), onclick: () => {
          season = x.season; chips.forEach(c => c.setAttribute('aria-pressed', String(c === b))); fillEps();
        } }, +x.season === 0 ? 'Specials' : 'Season ' + x.season);
        return b;
      });
      const cast = Array.isArray(d.cast) ? d.cast.slice(0, 8).map(c => typeof c === 'string' ? c : (c && c.name) || '').filter(Boolean) : [];
      body.replaceChildren(
        h('div', { class: 'md-hero' }, webImg(d.background || d.poster, ''), h('div', { class: 'hero-in' }, titleOrLogo(d))),
        h('div', { class: 'md-body' },
          d.logo && h('h2', null, d.title || ''),
          movieMeta(d, d),
          h('div', { class: 'md-acts' },
            h('button', { class: 'btn primary', type: 'button', onclick: () => playOnPc({ imdb: itemKey(d), kind: d.kind, title: d.title }) }, icon('play'), 'Play on PC'),
            h('button', { class: 'btn', type: 'button', onclick: () => openOnPc(d) }, icon('pc'), 'Open on PC')),
          d.description && h('p', null, d.description),
          cast.length > 0 && h('div', { class: 'cast' }, h('b', null, 'Cast: '), cast.join(', ')),
          !full && h('div', null, h('div', { class: 'skel skel-line', style: { width: '60%' } }), h('div', { class: 'skel skel-line', style: { width: '45%' } })),
          seasons.length > 0 && h('div', { class: 'seasons' }, h('div', { class: 'hscroll' }, chips)),
          seasons.length > 0 && eps));
      fillEps();
    }
    draw(it, false);
    api('/api/movies/' + enc(itemKey(it)) + '?kind=' + enc(it.kind || 'movie'))
      .then(d => { if (!s.closed) draw({ ...it, ...d }, true); })
      .catch(e => { if (!s.closed) { draw(it, true); if (e.status !== 404) toast(e.message, 'err'); } });
  }, { cls: 'tall', label: it.title || 'Details' });
}

/* ======================================================================
   Remote screen
   ====================================================================== */
function viewRemote(ctx) {
  setTitle('Remote');
  let mode = LS.get('mv.remote.mode', 'scroll');
  const img = h('img', { class: 'rm-img', alt: 'Live view of MangaVox on your PC', draggable: 'false' });
  const msg = h('div', { class: 'rm-msg', hidden: true });
  const stage = h('div', { class: 'rm-stage' }, img, msg);
  const fpsEl = h('span', { class: 'rm-fps', 'aria-hidden': 'true' }, '');
  const kbBtn = iconBtn('keyboard', 'Keyboard', () => toggleKb());
  kbBtn.setAttribute('aria-pressed', 'false');
  const textIn = h('input', { type: 'text', placeholder: 'Type to send to PC', 'aria-label': 'Text to send', autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false', enterkeyhint: 'send' });
  const KEYS = [['escape', 'Esc'], ['tab', 'Tab'], ['left', '←'], ['up', '↑'], ['down', '↓'], ['right', '→'], ['enter', 'Enter'], ['backspace', '⌫'], ['space', 'Space'], ['pageup', 'PgUp'], ['pagedown', 'PgDn'], ['home', 'Home'], ['end', 'End']];
  const LABEL = { escape: 'Escape', left: 'Left arrow', up: 'Up arrow', down: 'Down arrow', right: 'Right arrow', backspace: 'Backspace', pageup: 'Page up', pagedown: 'Page down' };
  const kb = h('div', { class: 'rm-kb', hidden: true },
    h('form', { onsubmit: e => { e.preventDefault(); const s = textIn.value; if (s) { send({ t: 'text', s }); textIn.value = ''; } else send({ t: 'key', key: 'enter' }); } },
      textIn, h('button', { class: 'btn primary', type: 'submit' }, 'Send')),
    h('div', { class: 'keys', role: 'toolbar', 'aria-label': 'Special keys' },
      KEYS.map(([k, l]) => h('button', { type: 'button', 'aria-label': LABEL[k] || l, onclick: () => send({ t: 'key', key: k }) }, l))));
  const PAGES = [['library', 'Library'], ['movies', 'Movies'], ['anime', 'Anime'], ['studio', 'Studio'], ['mobile', 'Mobile']];
  const nav = h('div', { class: 'rm-nav hscroll', role: 'toolbar', 'aria-label': 'Open on PC' },
    PAGES.map(([p, l]) => h('button', { class: 'chip', type: 'button', onclick: () => post('/api/pc/open', { page: p }).then(() => { toast('PC → ' + l, 'ok', 1500); kick(); }, e => toast(e.message, 'err')) }, l)));
  const bar = h('div', { class: 'rm-bar' },
    segmented([['scroll', 'Scroll', 'scroll'], ['drag', 'Drag', 'drag']], mode, v => { mode = v; LS.set('mv.remote.mode', v); }, 'Gesture mode'),
    kbBtn, fpsEl);
  view.append(h('div', { class: 'remote' }, bar, stage, nav, kb));
  function toggleKb(force) {
    const on = force ?? kb.hidden;
    kb.hidden = !on; kbBtn.setAttribute('aria-pressed', String(on));
    if (on) textIn.focus(); else textIn.blur();
    requestAnimationFrame(fit);
  }

  // ----- frames -----
  let url = null, timer = null, busy = false, errs = 0, frames = 0, fpsT = performance.now();
  const ac = new AbortController();
  function fit() {
    const W = img.naturalWidth, H = img.naturalHeight;
    if (!W || !H) return;
    const sw = stage.clientWidth, sh = stage.clientHeight;
    const k = Math.min(sw / W, sh / H);
    img.style.width = Math.floor(W * k) + 'px';
    img.style.height = Math.floor(H * k) + 'px';
  }
  function showMsg(e) {
    let title = 'Can’t show your PC', text = e.message || '';
    if (e.offline) { title = 'Can’t reach your PC'; text = 'Make sure MangaVox is running and this phone is on the same Wi‑Fi.'; }
    else if (e.status === 409) title = 'MangaVox isn’t visible';
    else if (e.status === 501 || e.status === 404) title = 'Remote view unavailable';
    msg.replaceChildren(icon(e.offline ? 'wifioff' : 'pc'), h('b', null, title), text && h('span', null, text),
      h('span', { class: 'small muted' }, 'Retrying automatically…'),
      h('button', { class: 'btn', type: 'button', onclick: () => kick() }, icon('refresh'), 'Retry now'));
    msg.hidden = false;
  }
  async function frame() {
    timer = null;
    if (!ctx.alive || busy) return;
    if (document.visibilityState !== 'visible') return; // resumed by visibilitychange
    busy = true;
    const t0 = performance.now();
    let wait = 0;
    try {
      const w = Math.max(320, Math.min(1200, Math.round(stage.clientWidth * (devicePixelRatio || 1))));
      const res = await api('/api/screen/frame?w=' + w + '&q=55', { raw: true, signal: ac.signal });
      const blob = await res.blob();
      if (!ctx.alive) return;
      const u = URL.createObjectURL(blob);
      await new Promise((ok, bad) => { img.onload = ok; img.onerror = () => bad(new ApiError('Bad frame', 0)); img.src = u; });
      if (url) URL.revokeObjectURL(url);
      url = u;
      fit();
      msg.hidden = true;
      errs = 0;
      frames++;
      const now = performance.now();
      if (now - fpsT > 1000) { fpsEl.textContent = (frames * 1000 / (now - fpsT)).toFixed(1) + ' fps'; frames = 0; fpsT = now; }
      wait = Math.max(0, 125 - (now - t0));
    } catch (e) {
      if (e && e.name === 'AbortError') return;
      if (!ctx.alive) return;
      errs++;
      showMsg(e);
      fpsEl.textContent = '';
      wait = Math.min(4000, 600 * errs);
    } finally { busy = false; }
    if (ctx.alive && !timer) timer = setTimeout(frame, wait);
  }
  function kick(delay = 0) {
    if (!ctx.alive) return;
    if (timer) { clearTimeout(timer); timer = null; }
    if (busy) return; // next frame is scheduled after the in-flight one
    timer = setTimeout(frame, delay);
  }
  const onVis = () => { if (document.visibilityState === 'visible') kick(); };
  document.addEventListener('visibilitychange', onVis);
  const onResize = () => fit();
  addEventListener('resize', onResize);
  ctx.onLeave(() => {
    ac.abort(); clearTimeout(timer); timer = null;
    document.removeEventListener('visibilitychange', onVis); removeEventListener('resize', onResize);
    if (url) URL.revokeObjectURL(url);
  });
  frame();

  // ----- input -----
  let errAt = 0;
  function send(o) {
    return post('/api/screen/input', o).then(() => kick(60), e => {
      const now = Date.now();
      if (now - errAt > 2500) { errAt = now; toast(e.message, 'err'); }
    });
  }
  function norm(e) {
    const r = img.getBoundingClientRect();
    if (!r.width || !r.height) return null;
    const x = (e.clientX - r.left) / r.width, y = (e.clientY - r.top) / r.height;
    return { x: Math.min(1, Math.max(0, x)), y: Math.min(1, Math.max(0, y)), inside: x >= 0 && x <= 1 && y >= 0 && y <= 1 };
  }
  function ripple(e, cls) {
    const r = stage.getBoundingClientRect();
    const d = h('span', { class: 'ripple' + (cls ? ' ' + cls : ''), style: { left: (e.clientX - r.left) + 'px', top: (e.clientY - r.top) + 'px' } });
    stage.append(d);
    setTimeout(() => d.remove(), 900);
  }
  const STEP = 36; // px of finger travel per wheel notch
  let g = null, scrollAcc = 0, scrollBusy = false;
  function flushScroll() {
    if (scrollBusy || !g) return;
    const n = Math.trunc(scrollAcc);
    if (!n) return;
    scrollAcc -= n;
    scrollBusy = true;
    post('/api/screen/input', { t: 'scroll', x: g.p0.x, y: g.p0.y, dy: n })
      .catch(e => { const now = Date.now(); if (now - errAt > 2500) { errAt = now; toast(e.message, 'err'); } })
      .finally(() => { scrollBusy = false; kick(30); if (Math.abs(scrollAcc) >= 1) flushScroll(); });
  }
  stage.addEventListener('pointerdown', e => {
    if (e.target.closest('.rm-msg') || !img.naturalWidth) return;
    if (g) return; // ignore extra fingers
    const p = norm(e);
    if (!p || !p.inside) return;
    stage.setPointerCapture(e.pointerId);
    g = { id: e.pointerId, sx: e.clientX, sy: e.clientY, ly: e.clientY, p0: p, moved: false, long: false, t: performance.now() };
    scrollAcc = 0;
    g.timer = setTimeout(() => {
      if (!g || g.moved) return;
      g.long = true;
      ripple(e, 'long');
      if (navigator.vibrate) navigator.vibrate(25);
      send({ t: 'long', x: p.x, y: p.y });
    }, 500);
  });
  stage.addEventListener('pointermove', e => {
    if (!g || e.pointerId !== g.id) return;
    if (!g.moved && Math.hypot(e.clientX - g.sx, e.clientY - g.sy) > 10) { g.moved = true; clearTimeout(g.timer); }
    if (g.moved && !g.long && mode === 'scroll') {
      scrollAcc += (g.ly - e.clientY) / STEP; // finger up -> content down (+dy)
      g.ly = e.clientY;
      flushScroll();
    }
  });
  const end = e => {
    if (!g || e.pointerId !== g.id) return;
    clearTimeout(g.timer);
    const cur = g; g = null;
    if (e.type === 'pointercancel' || cur.long) return;
    if (!cur.moved) { ripple(e); send({ t: 'tap', x: cur.p0.x, y: cur.p0.y }); return; }
    if (mode === 'drag') { const p = norm(e); if (p) { ripple(e); send({ t: 'drag', x: cur.p0.x, y: cur.p0.y, x2: p.x, y2: p.y }); } }
  };
  stage.addEventListener('pointerup', end);
  stage.addEventListener('pointercancel', end);
  stage.addEventListener('contextmenu', e => e.preventDefault());
}

/* ======================================================================
   Settings + boot
   ====================================================================== */
function openSettings() {
  openSheet((p, s) => {
    let armed = false, armT;
    const forget = h('button', { class: 'btn danger block', type: 'button', onclick: async () => {
      if (!armed) {
        armed = true; forget.classList.add('confirm'); forget.replaceChildren(icon('logout'), 'Tap again to forget this phone');
        armT = setTimeout(() => { armed = false; forget.classList.remove('confirm'); forget.replaceChildren(icon('logout'), 'Forget this phone'); }, 4000);
        return;
      }
      clearTimeout(armT);
      forget.classList.add('busy'); forget.disabled = true;
      try { await post('/api/logout'); } catch (e) { if (e.status !== 401) { toast(e.message, 'err'); forget.classList.remove('busy'); forget.disabled = false; return; } }
      try { localStorage.clear(); } catch { /* ignore */ }
      lockedOut = true;
      location.replace('/locked');
    } }, icon('logout'), 'Forget this phone');
    s.onClose.push(() => clearTimeout(armT));
    const kv = (k, v) => h('div', { class: 'kv' }, h('span', null, k), h('span', null, v || '—'));
    p.append(h('div', { class: 'grip', 'aria-hidden': 'true' }),
      h('h2', { class: 'sheet-h' }, 'This phone'),
      kv('Device', me && me.device && me.device.name),
      kv('Connected to', me && me.pc),
      kv('App', me ? [me.app || 'MangaVox', me.version].filter(Boolean).join(' ') : ''),
      h('p', { class: 'muted small' }, 'This phone stays paired until you forget it here or revoke it in MangaVox → Mobile on your PC.'),
      h('button', { class: 'btn ghost block', type: 'button', onclick: () => post('/api/pc/open', { page: 'mobile' }).then(() => toast('Opened Mobile on PC', 'ok'), e => toast(e.message, 'err')) }, icon('pc'), 'Open Mobile page on PC'),
      forget);
  }, { label: 'Settings' });
}

function boot() {
  for (const a of tabs) {
    a.prepend(icon(a.dataset.ic));
    a.addEventListener('click', e => {
      if (a.getAttribute('href') === (location.hash || '#/library')) { e.preventDefault(); scrollTo({ top: 0, behavior: 'smooth' }); }
    });
  }
  $back.append(icon('back'));
  $back.addEventListener('click', () => goBack(backTarget || '#/library'));
  const st = document.getElementById('settings');
  st.append(icon('settings'));
  st.addEventListener('click', openSettings);
  if ('scrollRestoration' in history) history.scrollRestoration = 'manual';
  const off = document.getElementById('offline');
  const net = () => { off.hidden = navigator.onLine; };
  addEventListener('online', () => { net(); toast('Back online', 'ok'); });
  addEventListener('offline', net);
  net();
  Player = createPlayer();
  api('/api/me').then(d => { me = d || {}; $sub.textContent = subLine(); }).catch(() => {});
  addEventListener('hashchange', route);
  route();
}
boot();
