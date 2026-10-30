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
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

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
      else if (k === 'href' || k === 'src') el.setAttribute(k, safeHref(v));
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
  tv: 'M3 7h18v13H3zM8 3l4 4 4-4',
  grid: 'M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z',
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
  star: 'FM12 3l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1L3.2 9.5l6.1-.9z',
  pc: 'M3 4h18v12H3zM8 20h8M12 16v4',
  sort: 'M7 4v16M3 8l4-4 4 4M17 20V4M13 16l4 4 4-4',
  alert: 'M12 3l10 18H2zM12 10v5M12 18h.01',
  logout: 'M14 4h5v16h-5M10 8l-4 4 4 4M6 12h11',
  list: 'M8 6h13M8 12h13M8 18h13M3.5 6h.01M3.5 12h.01M3.5 18h.01',
  fs: 'M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5',
  fsx: 'M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5',
  pip: 'M3 5h18v14H3zM12 12h7v5h-7z',
  cc: 'M3 5h18v14H3zM10.5 10.2a2.4 2.4 0 1 0 0 3.6M17.5 10.2a2.4 2.4 0 1 0 0 3.6',
  phone: 'M7 2h10v20H7zM11 18h2',
  pages: 'M4 4h7v16H4zM13 4h7v16h-7z',
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
const skipBtn = (d, onclick, cls) => h('button', { class: 'icon-btn skip' + (cls ? ' ' + cls : ''), type: 'button', 'aria-label': (d < 0 ? 'Back ' : 'Forward ') + Math.abs(d) + ' seconds', onclick },
  icon(d < 0 ? 'rotl' : 'rotr'), h('b', { 'aria-hidden': 'true' }, String(Math.abs(d))));

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
const arr = v => (Array.isArray(v) ? v : []);
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
    let msg = '';
    try { const j = await res.json(); if (j && j.error) msg = String(j.error); } catch { /* not json */ }
    // A 404 from the router (not from a handler) means the PC runs an older MangaVox.
    if (res.status === 404 && (!msg || /^not found$/i.test(msg))) msg = 'This needs a newer MangaVox on your PC. Update MangaVox there and try again.';
    throw new ApiError(msg || 'Something went wrong (' + res.status + ')', res.status);
  }
  if (raw) return res;
  const text = await res.text();
  if (!text) return {};
  try { return JSON.parse(text); } catch { throw new ApiError('Bad response from PC', res.status); }
}
const post = (path, body = {}, o = {}) => api(path, { ...o, method: 'POST', body });

// Small stale-while-revalidate cache so going back is instant. A view that fails to draw
// shows an error with retry instead of a blank page.
const cache = new Map();
function swr(ctx, path, draw, onErr) {
  const hit = cache.get(path);
  const run = (d, fromCache) => {
    try { draw(d, fromCache); return true; } catch (e) { console.error(e); return false; }
  };
  const shown = hit ? run(hit.data, true) : false;
  return api(path).then(data => {
    const json = JSON.stringify(data);
    cache.set(path, { data, json });
    if (ctx.alive && (!shown || hit.json !== json)) {
      if (!run(data, false) && !shown) onErr(new ApiError('Couldn’t show this page. Try again in a moment.', 0));
    }
    return data;
  }).catch(e => {
    if (!ctx.alive || e.status === 401 || e.name === 'AbortError') return;
    if (!shown) onErr(e); else if (e.offline) toast('Offline: showing saved data', 'err');
  });
}
const invalidate = prefix => { for (const k of cache.keys()) if (k.startsWith(prefix)) cache.delete(k); };

/* ---------- images ---------- */
// Remote artwork (Cinemeta, MAL) goes through the PC (`/api/img`): it is fetched and cached once
// there, so the phone never opens dozens of connections to image CDNs. Same-origin paths
// (manga covers, chapter pages) are used as they are.
const W = { poster: 342, card: 640, hero: 1080, thumb: 320, logo: 500 };
function imgUrl(u, w) {
  if (!u) return null;
  const s = String(u);
  if (/^\/(?!\/)/.test(s)) return s;
  if (/^https?:\/\//i.test(s)) return '/api/img?u=' + enc(s) + (w ? '&w=' + w : '');
  return null;
}
/** Artwork box: sized by CSS (so nothing jumps), image fades in, placeholder on failure. */
function art(src, w, cls, label, eager) {
  const box = h('div', { class: 'art' + (cls ? ' ' + cls : '') });
  const ph = () => label ? h('div', { class: 'art-ph' + (label.length <= 3 ? ' big' : '') }, label) : null;
  const u = imgUrl(src, w);
  if (!u) { add(box, [ph()]); return box; }
  const img = h('img', { alt: '', decoding: 'async', loading: eager ? 'eager' : 'lazy', referrerpolicy: 'no-referrer' });
  if (eager) img.setAttribute('fetchpriority', 'high');
  img.addEventListener('load', () => img.classList.add('ok'));
  img.addEventListener('error', () => { img.remove(); add(box, [ph()]); }, { once: true });
  img.src = u;
  box.append(img);
  return box;
}
/** Manga cover (same-origin), with initials fallback. */
const cover = (src, title, cls) => art(src, W.poster, 'cov' + (cls ? ' ' + cls : ''), initials(title));

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
let mePromise = null;
const getMe = () => mePromise || (mePromise = api('/api/me').then(d => { me = d || {}; $sub.textContent = subLine(); return me; }).catch(e => { mePromise = null; throw e; }));

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
  return h('div', { class: 'state' }, icon(ic), h('h2', null, title), text && h('p', null, text),
    actions.filter(Boolean).length ? h('div', { class: 'acts' }, actions) : null);
}
function errorState(e, retry, ...more) {
  const off = e && e.offline;
  return stateBox(off ? 'wifioff' : 'alert', off ? 'Can’t reach your PC' : 'Something went wrong',
    off ? 'Make sure MangaVox is running on your PC and this phone is on the same Wi‑Fi.' : (e instanceof ApiError && e.message) || 'Couldn’t load this. Try again in a moment.',
    retry && h('button', { class: 'btn primary', type: 'button', onclick: retry }, icon('refresh'), 'Try again'), ...more);
}
const spinner = label => h('div', { class: 'state' }, h('div', { class: 'spin-ring', role: 'progressbar', 'aria-label': label || 'Loading' }));

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
  dragToClose(s);
  return s;
}
function closeSheet(s, fromPop = false) {
  if (s.closed) return;
  s.closed = true;
  const i = sheets.indexOf(s); if (i >= 0) sheets.splice(i, 1);
  for (const f of s.onClose) { try { f(); } catch { /* ignore */ } }
  s.panel.classList.remove('drag');
  s.panel.style.transform = '';
  s.wrap.classList.remove('open');
  setTimeout(() => s.wrap.remove(), 320);
  if (!sheets.length) document.body.classList.remove('modal-open');
  if (!fromPop) { suppressPop++; history.back(); }
  if (s.prevFocus && s.prevFocus.focus) s.prevFocus.focus({ preventScroll: true });
}
/** Swipe a sheet down (from its top, when scrolled to the top) to close it. */
function dragToClose(s) {
  const p = s.panel;
  let y0 = null, dy = 0, on = false;
  p.addEventListener('touchstart', e => {
    y0 = p.scrollTop <= 0 && e.touches.length === 1 ? e.touches[0].clientY : null; dy = 0; on = false;
  }, { passive: true });
  p.addEventListener('touchmove', e => {
    if (y0 == null) return;
    dy = e.touches[0].clientY - y0;
    if (!on) { if (dy > 10 && p.scrollTop <= 0) { on = true; p.classList.add('drag'); } else if (dy < 0) { y0 = null; return; } }
    if (on) p.style.transform = 'translateY(' + Math.max(0, dy - 10) + 'px)';
  }, { passive: true });
  const end = () => {
    if (!on) { y0 = null; return; }
    p.classList.remove('drag');
    if (dy > 110) s.close(); else p.style.transform = '';
    y0 = null; on = false;
  };
  p.addEventListener('touchend', end);
  p.addEventListener('touchcancel', end);
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
function wakeLock() {
  let lock = null, want = false;
  const get = async () => {
    try { if (want && !lock && 'wakeLock' in navigator && document.visibilityState === 'visible') { lock = await navigator.wakeLock.request('screen'); lock.addEventListener('release', () => { lock = null; }); } }
    catch { lock = null; }
  };
  const onVis = () => { if (document.visibilityState === 'visible') get(); };
  document.addEventListener('visibilitychange', onVis);
  return {
    on() { want = true; get(); },
    off() { want = false; if (lock) lock.release().catch(() => {}); lock = null; },
    dispose() { this.off(); document.removeEventListener('visibilitychange', onVis); },
  };
}

/* ======================================================================
   Router
   ====================================================================== */
const ROUTES = [
  ['library', 'library', /^\/library$/, viewLibrary],
  ['manga', 'library', /^\/manga\/([^/]+)$/, viewManga],
  ['reader', 'library', /^\/read\/([^/]+)$/, viewReader],
  ['movies', 'movies', /^\/movies$/, viewMovies],
  ['anime', 'anime', /^\/anime$/, viewAnime],
  ['watch', '', /^\/watch$/, viewWatch],
  ['studio', 'studio', /^\/studio$/, viewStudio],
  ['more', 'more', /^\/more$/, viewMore],
  ['listen', 'more', /^\/listen$/, viewListen],
  ['remote', 'more', /^\/remote$/, viewRemote],
];
let ctx = null;
let navCount = 0;
const scrollMemo = new Map();
const tabs = [...document.querySelectorAll('#tabbar a')];
const tabLast = {};
let pendingReopen = null; // a detail sheet to reopen when coming back from the player

function route() {
  if (lockedOut) return;
  const raw = location.hash.slice(1) || '/library';
  const [path, qs] = raw.split('?');
  let match = null;
  for (const [name, tab, re, fn] of ROUTES) {
    const m = path.match(re);
    if (m) {
      try { match = { name, tab, fn, params: m.slice(1).map(decodeURIComponent) }; } catch { match = null; }
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
    key: raw, alive: true, cleanups: [], refresh: null,
    onLeave(f) { this.cleanups.push(f); },
    restoreScroll() { const y = scrollMemo.get(raw); if (y) scrollTo(0, y); },
  };
  ctx = c;
  navCount++;
  document.body.dataset.route = match.name;
  document.body.dataset.tab = match.tab;
  if (match.tab && match.name !== 'reader') { tabLast[match.tab] = raw; LS.set('mv.last', raw); }
  for (const a of tabs) {
    const on = a.dataset.tab === match.tab;
    a.classList.toggle('active', on);
    if (on) a.setAttribute('aria-current', 'page'); else a.removeAttribute('aria-current');
  }
  setActions();
  setBack(null);
  view.replaceChildren();
  scrollTo(0, 0);
  try { match.fn(c, ...match.params, new URLSearchParams(qs || '')); }
  catch (e) { console.error(e); view.append(errorState(e, () => route())); }
  if (pendingReopen && match.name !== 'watch') {
    const r = pendingReopen; pendingReopen = null;
    if (r.key === raw) setTimeout(() => { if (ctx === c && c.alive && !sheets.length) r.open(); }, 60);
  }
}

/* Pull to refresh on pages that set ctx.refresh. */
function setupPtr() {
  const el = document.getElementById('ptr');
  el.append(icon('refresh'));
  let y0 = null, x0 = 0, dy = 0, on = false, busy = false;
  const set = (d) => { el.style.transform = 'translateY(' + d + 'px) rotate(' + d * 3 + 'deg)'; el.style.opacity = String(Math.min(1, d / 60)); };
  const reset = () => { el.classList.add('back'); el.classList.remove('arm', 'spin'); set(0); setTimeout(() => el.classList.remove('back'), 260); };
  addEventListener('touchstart', e => {
    y0 = null;
    if (busy || !ctx || !ctx.refresh || sheets.length || scrollY > 2 || e.touches.length !== 1) return;
    y0 = e.touches[0].clientY; x0 = e.touches[0].clientX; dy = 0; on = false;
  }, { passive: true });
  addEventListener('touchmove', e => {
    if (y0 == null) return;
    const d = e.touches[0].clientY - y0, dx = Math.abs(e.touches[0].clientX - x0);
    if (!on) {
      if (d > 12 && d > dx * 1.6 && scrollY <= 2) { on = true; el.classList.remove('back'); }
      else if (dx > 12 || d < -6) { y0 = null; return; }
      else return;
    }
    dy = Math.min(110, (d - 12) * 0.55);
    set(dy);
    el.classList.toggle('arm', dy >= 62);
  }, { passive: true });
  const end = () => {
    if (y0 == null || !on) { y0 = null; return; }
    y0 = null; on = false;
    if (dy < 62 || !ctx || !ctx.refresh) { reset(); return; }
    busy = true;
    el.classList.add('back', 'spin'); el.style.transform = 'translateY(64px)'; el.style.opacity = '1';
    const done = () => { busy = false; reset(); };
    Promise.resolve().then(() => ctx.refresh()).then(done, done);
  };
  addEventListener('touchend', end, { passive: true });
  addEventListener('touchcancel', end, { passive: true });
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
const openPc = (body, label) => post('/api/pc/open', body).then(() => toast(label || 'Opened on your PC', 'ok'), e => toast(e.message, 'err'));

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
        h('div', { class: 'ccard-p' }, icon('play'), 'Continue reading')));
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
    list = arr(list);
    root.replaceChildren();
    if (!list.length) {
      root.append(stateBox('book', 'Your library is empty', 'Add manga to your library in MangaVox on your PC and they show up here.',
        h('button', { class: 'btn', type: 'button', onclick: () => openPc({ page: 'library' }, 'Opened Library on PC') }, icon('pc'), 'Open Library on PC')));
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
    root.append(h('section', { class: 'sec', style: { contentVisibility: 'visible' } },
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
  ctx.refresh = load;
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
    const chs = arr(m.chapters);
    const resume = pickResume(chs);
    const anyRead = chs.some(c => c.read || c.progress > 0);
    const refresh = h('button', { class: 'btn', type: 'button', onclick: async () => {
      refresh.classList.add('busy'); refresh.disabled = true;
      const before = chs.length;
      try {
        const d = await post('/api/manga/' + enc(id) + '/refresh');
        cache.set('/api/manga/' + enc(id), { data: d, json: JSON.stringify(d) });
        invalidate('/api/library');
        const added = arr(d.chapters).length - before;
        if (ctx.alive) { draw(d); toast(added > 0 ? added + ' new chapter' + (added > 1 ? 's' : '') : 'Up to date', 'ok'); }
      } catch (e) { toast(e.message, 'err'); refresh.classList.remove('busy'); refresh.disabled = false; }
    } }, icon('refresh'), 'Refresh');
    const list = h('div', { class: 'chlist' });
    const fillList = () => list.replaceChildren(...(desc ? chs.slice().reverse() : chs).map(c => chRow(c, false, () => recapSheet(m, c))));
    const sortLbl = h('span', null, desc ? 'Newest first' : 'Oldest first');
    const sortBtn = h('button', { class: 'sortbtn', type: 'button', 'aria-label': 'Change chapter order', onclick: () => {
      desc = !desc; LS.set('mv.chsort', desc ? 'desc' : 'asc'); sortLbl.textContent = desc ? 'Newest first' : 'Oldest first'; fillList();
    } }, icon('sort'), sortLbl);
    const descEl = m.description ? h('p', { class: 'desc', tabindex: '0', role: 'button', 'aria-label': 'Description, tap to expand' }, m.description) : null;
    if (descEl) {
      const tog = () => descEl.classList.toggle('open');
      descEl.addEventListener('click', tog);
      descEl.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); tog(); } });
    }
    const read = chs.filter(c => c.read).length;
    root.replaceChildren(
      h('div', { class: 'mhero' },
        h('div', { class: 'mhero-bg', 'aria-hidden': 'true' }, m.cover && h('img', { src: m.cover, alt: '' })),
        h('div', { class: 'mhero-in' },
          cover(m.cover, m.title),
          h('div', { class: 'mhero-meta' },
            h('h1', { class: 'mt' }, m.title),
            m.author && h('div', { class: 'muted' }, m.author),
            h('div', { class: 'chips' }, m.status && h('span', { class: 'chip' }, m.status), arr(m.genres).slice(0, 4).map(g => h('span', { class: 'chip' }, g)))))),
      h('div', { class: 'macts' },
        resume && h('a', { class: 'btn primary', href: '#/read/' + enc(resume.id) }, icon('play'),
          (anyRead ? 'Continue · ' : 'Start reading · ') + chLabel(resume).replace(/ · .*/, '')),
        refresh,
        h('button', { class: 'btn', type: 'button', onclick: () => openPc({ page: 'library', manga_id: id }) }, icon('pc'), 'On PC'),
        h('a', { class: 'btn', href: '#/studio?manga=' + enc(id) }, icon('sparkle'), 'Studio')),
      descEl,
      h('div', { class: 'sec-row', style: { marginTop: '16px', marginBottom: '6px' } },
        h('h2', { class: 'sec-h' }, 'Chapters', h('span', { class: 'count' }, chs.length ? read + ' / ' + chs.length + ' read' : '')), sortBtn),
      chs.length ? list : h('p', { class: 'pad muted' }, 'No chapters yet. Tap Refresh to check the source.'));
    fillList();
  }
  const load = () => swr(ctx, '/api/manga/' + enc(id), (d, fromCache) => { draw(d); if (fromCache) ctx.restoreScroll(); },
    e => root.replaceChildren(errorState(e, () => route())));
  ctx.refresh = load;
  load();
}
/** A chapter row. `onRecap` (title page) is called instead of opening the reader when Studio made
 *  a recap of the chapter, to offer it. */
function chRow(c, current, onRecap) {
  const pct = Math.round((c.progress || 0) * 100);
  const partial = !c.read && pct > 0 && pct < 100;
  const rc = c.recap || null;
  const onclick = rc && (rc.video || rc.aloud) && onRecap ? e => { e.preventDefault(); onRecap(); } : null;
  return h('a', { class: 'chrow' + (c.read ? ' read' : '') + (current ? ' cur' : ''), href: '#/read/' + enc(c.id), 'aria-current': current ? 'true' : null, onclick },
    h('div', { class: 'chrow-main' },
      h('div', { class: 'chrow-t' }, chLabel(c)),
      h('div', { class: 'chrow-s' }, [current ? 'Reading now' : '', c.pages ? c.pages + ' pages' : '', c.read ? 'Read' : partial ? pct + '% read' : ''].filter(Boolean).join(' · ') || ' '),
      partial && h('div', { class: 'pbar sm' }, h('i', { style: { width: pct + '%' } }))),
    h('div', { class: 'chrow-b' },
      c.read && h('span', { title: 'Read', 'aria-label': 'Read' }, icon('check')),
      c.downloaded && h('span', { title: 'Downloaded', 'aria-label': 'Downloaded' }, icon('download')),
      (c.audio || (rc && rc.aloud)) && h('span', { class: 'b-ac', title: 'Narration audio', 'aria-label': 'Has narration audio' }, icon('wave')),
      (c.video || (rc && rc.video)) && h('span', { class: 'b-ac', title: 'Narrated video', 'aria-label': 'Has narrated video' }, icon('video'))));
}

/* Read aloud plays through one shared element: the tap that picks "Read aloud" touches it, which
   lets iPhones start it after the reader has loaded. */
const raAudio = new Audio();
raAudio.preload = 'auto';

/** A chapter Studio made a recap of: watch the video, read along with the narration, or just read. */
function recapSheet(m, c) {
  const rc = c.recap || {};
  const nav = href => { if (sheets.length) location.replace(href); else location.hash = href; };
  const opt = (ic, title, sub, onclick, primary) => h('button', { class: 'rc-opt' + (primary ? ' primary' : ''), type: 'button', onclick },
    h('span', { class: 'rc-ic' }, icon(ic)), h('span', { class: 'rc-tx' }, h('b', null, title), h('small', null, sub)), icon('chev'));
  openSheet(p => {
    p.append(h('div', { class: 'grip', 'aria-hidden': 'true' }),
      h('h2', { class: 'sheet-h' }, chLabel(c)),
      h('p', { class: 'muted small rc-lead' }, 'Studio made a recap of this chapter.'),
      rc.aloud && opt('headphones', 'Read aloud', 'Hear the narration while the pages scroll along', () => {
        try { raAudio.load(); } catch { /* best effort */ }
        nav('#/read/' + enc(c.id) + '?aloud=1');
      }, true),
      rc.video && opt('video', 'Watch recap video', 'Play the narrated recap on this phone', () => {
        startWatch({ kind: 'recap', chapter: c.id, manga: m.id, title: m.title, ept: 'Recap · ' + chLabel(c), bg: m.cover });
      }, !rc.aloud),
      opt('book', 'Read', 'Open the chapter in the reader', () => nav('#/read/' + enc(c.id))));
  }, { label: 'Recap of ' + chLabel(c) });
}

/* ======================================================================
   Reader: vertical (webtoon) scroll or one page at a time
   ====================================================================== */
let readerJump = null; // { id, page | 'last' } for the next reader render
async function viewReader(ctx, chId, q) {
  const wantAloud = !!q && q.get('aloud') === '1';
  document.documentElement.classList.add('reading');
  ctx.onLeave(() => document.documentElement.classList.remove('reading'));
  const root = h('div', { class: 'reader' }, spinner('Loading chapter'));
  view.append(root);

  let ch;
  try { ch = await api('/api/chapter/' + enc(chId)); }
  catch (e) {
    if (!ctx.alive) return;
    root.replaceChildren(errorState(e, () => route(), h('a', { class: 'btn', href: '#/library' }, 'Back to Library')));
    return;
  }
  if (!ctx.alive) return;
  const n = Math.max(0, ch.pages | 0);
  const paged = LS.get('mv.reader.mode', 'vertical') === 'paged';
  let rtl = LS.get('mv.reader.dir', 'rtl') !== 'ltr';
  root.classList.add(paged ? 'paged' : 'vertical');
  root.classList.toggle('rtl', rtl);
  const mangaKey = ch.manga_id != null ? '/api/manga/' + enc(ch.manga_id) : null;
  const mangaHref = ch.manga_id != null ? '#/manga/' + enc(ch.manga_id) : '#/library';
  const hit = mangaKey && cache.get(mangaKey);
  setTitle(chLabel(ch));

  let start = ch.start | 0;
  if (readerJump && String(readerJump.id) === String(chId)) start = readerJump.page === 'last' ? n - 1 : readerJump.page | 0;
  readerJump = null;
  let cur = clamp(start, 0, Math.max(0, n - 1));

  const pageUrl = (id, i) => '/api/chapter/' + enc(id) + '/page/' + i;
  const go = (id, page) => { if (id == null) return; if (page != null) readerJump = { id, page }; location.replace('#/read/' + enc(id)); };

  // ----- overlay -----
  const pgLbl = h('span', { class: 'r-pg', 'aria-live': 'off' }, '1 / ' + n);
  const slider = h('input', { type: 'range', min: '1', max: String(Math.max(1, n)), value: '1', step: '1', 'aria-label': 'Page' });
  const ttlS = h('span', null, (hit && hit.data.title) || ch.title || '');
  const prevBtn = iconBtn('prev', 'Previous chapter', () => go(ch.prev));
  const nextBtn = iconBtn('next', 'Next chapter', () => go(ch.next));
  prevBtn.disabled = ch.prev == null; nextBtn.disabled = ch.next == null;
  let ra = null; // the read-aloud controller while narration is on
  const aloudBtn = iconBtn('headphones', 'Read aloud', () => { if (ra) ra.close(); else startAloud(); });
  aloudBtn.hidden = true;
  const top = h('div', { class: 'r-ov r-top' },
    iconBtn('back', 'Back to chapters', () => goBack(mangaHref)),
    h('div', { class: 'r-ttl' }, h('b', null, chLabel(ch)), ttlS),
    aloudBtn,
    ch.manga_id != null && iconBtn('list', 'Chapters', () => chaptersSheet()),
    iconBtn('settings', 'Reading settings', () => settingsSheet()));
  const bot = h('div', { class: 'r-ov r-bot' }, prevBtn, slider, pgLbl, nextBtn);
  const end = h('div', { class: 'r-end' },
    h('h2', null, 'End of ' + chLabel(ch)),
    ch.next != null
      ? h('button', { class: 'btn primary', type: 'button', onclick: () => go(ch.next) }, 'Next chapter', icon('chev'))
      : h('p', { class: 'muted' }, 'You’re all caught up.'),
    h('a', { class: 'btn ghost', href: mangaHref }, 'Back to chapters'));

  let hideT;
  const showOv = (on) => {
    root.classList.toggle('hide-ov', !on);
    clearTimeout(hideT);
    if (on) hideT = setTimeout(() => root.classList.add('hide-ov'), 3500);
  };
  ctx.onLeave(() => clearTimeout(hideT));

  // The title, and whether this chapter has a narration to read along with.
  const fromManga = m => {
    if (!ctx.alive || !m) return;
    if (!ttlS.textContent && m.title) ttlS.textContent = m.title;
    const c = arr(m.chapters).find(x => String(x.id) === String(chId));
    if (c && c.recap && c.recap.aloud) aloudBtn.hidden = false;
  };
  if (hit) fromManga(hit.data);
  else if (ch.manga_id != null) getManga(ch.manga_id).then(fromManga).catch(() => {});

  // ----- progress -----
  let sliding = false, saveT = null, saved = cur, preloaded = false;
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
  function setCur(i) {
    if (i === cur) return;
    cur = i; updateUi(); save(false);
    if (!preloaded && ch.next != null && cur >= n - 3) {
      preloaded = true;
      api('/api/chapter/' + enc(ch.next)).then(nx => {
        for (let k = 0; k < Math.min(3, nx.pages | 0); k++) { const im = new Image(); im.src = pageUrl(ch.next, k); preloadImgs.push(im); }
      }).catch(() => { preloaded = false; });
    }
  }
  ctx.onLeave(() => {
    save(true);
    // Keep the chapter list's read state current without waiting for a refetch.
    const m = mangaKey && cache.get(mangaKey);
    const c = m && arr(m.data.chapters).find(x => String(x.id) === String(chId));
    if (c && n) {
      c.progress = Math.max(c.progress || 0, (cur + 1) / n);
      if (cur >= n - 1) c.read = true;
      m.json = '';
    }
  });

  const engine = paged ? pagedEngine() : verticalEngine();
  root.replaceChildren(top, ...(n ? engine.els : [h('div', { class: 'state' }, icon('book'), h('h2', null, 'No pages'), h('p', null, 'This chapter has no pages yet. Download it on the PC or try again.')), end]), bot);

  root.addEventListener('click', e => {
    if (e.target.closest('.r-ov')) { showOv(true); return; }
    if (e.target.closest('button, a, input, .ra')) return;
    if (engine.swallow && engine.swallow()) return;
    const x = e.clientX / innerWidth, y = e.clientY / innerHeight;
    if (paged) {
      if (x < 0.3) { showOv(false); rtl ? engine.next() : engine.prev(); return; }
      if (x > 0.7) { showOv(false); rtl ? engine.prev() : engine.next(); return; }
    } else if (y < 0.2 || y > 0.8) {
      showOv(false);
      if (ra) ra.nudge();
      scrollBy({ top: (y < 0.2 ? -1 : 1) * innerHeight * 0.8, behavior: 'smooth' });
      return;
    }
    showOv(root.classList.contains('hide-ov'));
  });
  const onKey = e => {
    if (sheets.length || e.target.closest('input')) return;
    const k = e.key;
    if (paged && (k === 'ArrowLeft' || k === 'ArrowRight')) { e.preventDefault(); (k === 'ArrowLeft') === rtl ? engine.next() : engine.prev(); }
    else if (paged && (k === ' ' || k === 'PageDown')) { e.preventDefault(); engine.next(); }
    else if (paged && k === 'PageUp') { e.preventDefault(); engine.prev(); }
  };
  document.addEventListener('keydown', onKey);
  ctx.onLeave(() => document.removeEventListener('keydown', onKey));

  slider.addEventListener('input', () => { sliding = true; pgLbl.textContent = slider.value + ' / ' + n; setRange(slider); showOv(true); });
  slider.addEventListener('change', () => {
    sliding = false;
    const i = (+slider.value || 1) - 1;
    engine.goTo(i);
    if (ra) ra.userPage(i);
  });

  const wl = wakeLock();
  wl.on();
  const onVis = () => { if (document.visibilityState !== 'visible') save(true); };
  document.addEventListener('visibilitychange', onVis);
  ctx.onLeave(() => { wl.dispose(); document.removeEventListener('visibilitychange', onVis); });

  updateUi();
  showOv(true);
  engine.start();
  if (wantAloud && n) startAloud();

  // ----- engines -----
  function verticalEngine() {
    const slots = [];
    const list = h('div', { class: 'r-pages' });
    for (let i = 0; i < n; i++) {
      const s = h('div', { class: 'rp', 'data-i': String(i) }, h('div', { class: 'rp-n', 'aria-hidden': 'true' }, String(i + 1)));
      slots.push(s); list.append(s);
    }
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
        s.classList.add('ok');
        s._state = 'ok';
        if (img.naturalWidth) {
          const ratio = img.naturalHeight / img.naturalWidth;
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
    }, { rootMargin: '80% 0px 120% 0px' });
    slots.forEach(s => io.observe(s));
    ctx.onLeave(() => io.disconnect());
    function computeCur() {
      if (!n) return;
      const y = innerHeight * 0.35;
      let lo = 0, hi = n - 1, ans = 0;
      while (lo <= hi) {
        const mid = (lo + hi) >> 1;
        if (slots[mid].getBoundingClientRect().top <= y) { ans = mid; lo = mid + 1; } else hi = mid - 1;
      }
      if (end.getBoundingClientRect().top < innerHeight) ans = n - 1;
      setCur(ans);
    }
    let raf = 0;
    const onScroll = () => { if (!raf) raf = requestAnimationFrame(() => { raf = 0; computeCur(); }); };
    addEventListener('scroll', onScroll, { passive: true });
    ctx.onLeave(() => { removeEventListener('scroll', onScroll); cancelAnimationFrame(raf); });
    return {
      els: [list, end],
      slot: i => slots[i] || null,
      start() {
        if (!n) return;
        for (let k = cur; k <= cur + 3; k++) load(k);
        if (cur > 0) slots[cur].scrollIntoView({ block: 'start' });
      },
      goTo(i) {
        for (let k = i; k <= i + 2; k++) load(k);
        slots[i] && slots[i].scrollIntoView({ block: 'start' });
        setCur(i);
      },
      next() { if (ra) ra.nudge(); scrollBy({ top: innerHeight * 0.8, behavior: 'smooth' }); },
      prev() { if (ra) ra.nudge(); scrollBy({ top: -innerHeight * 0.8, behavior: 'smooth' }); },
    };
  }

  function pagedEngine() {
    const stage = h('div', { class: 'pg' });
    const imgs = [];
    let shown = -1, swiped = false;
    const getImg = i => {
      if (i < 0 || i >= n) return null;
      if (!imgs[i]) {
        const im = new Image(); im.decoding = 'async'; im.alt = 'Page ' + (i + 1); im.draggable = false;
        im.src = pageUrl(chId, i); imgs[i] = im;
      }
      return imgs[i];
    };
    function show(i, dir) {
      i = clamp(i, 0, n);
      shown = i;
      if (i === n) { stage.replaceChildren(end); return; }
      const im = getImg(i);
      const wrap = h('div', { class: 'pg-page' + (dir ? ' in-' + dir : '') });
      const put = () => { if (shown === i) wrap.replaceChildren(im); };
      if (im.complete && im.naturalWidth) wrap.append(im);
      else {
        wrap.append(h('div', { class: 'pg-load' }, h('div', { class: 'spin-ring' }), 'Page ' + (i + 1)));
        im.onload = put;
        im.onerror = () => {
          if (shown !== i) return;
          wrap.replaceChildren(h('div', { class: 'rp-err', style: { position: 'static' } }, 'Page ' + (i + 1) + ' failed to load',
            h('button', { class: 'btn', type: 'button', onclick: ev => { ev.stopPropagation(); imgs[i] = null; show(i); } }, icon('refresh'), 'Retry')));
        };
      }
      stage.replaceChildren(wrap);
      for (const k of [i + 1, i + 2, i + 3, i - 1]) getImg(k);
      for (let k = 0; k < imgs.length; k++) if (imgs[k] && Math.abs(k - i) > 6) imgs[k] = null;
      setCur(i);
    }
    let sx = null, sy = 0, st = 0;
    stage.addEventListener('pointerdown', e => { if (e.isPrimary) { sx = e.clientX; sy = e.clientY; st = Date.now(); } });
    stage.addEventListener('pointerup', e => {
      if (sx == null) return;
      const dx = e.clientX - sx, dy = e.clientY - sy; sx = null;
      if (Math.abs(dx) > 50 && Math.abs(dx) > Math.abs(dy) * 1.3 && Date.now() - st < 900) {
        swiped = true; setTimeout(() => { swiped = false; }, 350);
        showOv(false);
        (dx < 0) !== rtl ? api2.next() : api2.prev();
      }
    });
    stage.addEventListener('pointercancel', () => { sx = null; });
    const api2 = {
      els: [stage],
      swallow: () => swiped,
      start() { if (n) show(cur); },
      goTo(i) { show(i); },
      // Flipping pages by hand also moves the narration to the page.
      next() { if (shown < n) { show(shown + 1, 'next'); if (ra && shown < n) ra.userPage(shown); } else if (ch.next != null) go(ch.next); },
      prev() { if (shown > 0) { show(shown - 1, 'prev'); if (ra) ra.userPage(shown); } else if (ch.prev != null) go(ch.prev, 'last'); },
    };
    return api2;
  }

  // ----- read aloud: the chapter's Studio narration, with the reader following along -----
  async function startAloud() {
    if (ra || aloudBtn.classList.contains('busy')) return;
    aloudBtn.hidden = false;
    aloudBtn.classList.add('busy');
    let rc = null;
    try { rc = await api('/api/chapter/' + enc(chId) + '/recap'); }
    catch (e) { if (ctx.alive && e.status !== 401) toast(e.status === 404 ? 'This chapter has no narration yet' : e.message, 'err'); }
    aloudBtn.classList.remove('busy');
    if (!ctx.alive || ra || !rc) return;
    if (!rc.aloud || !arr(rc.clips).length || !arr(rc.beats).length) { toast('This chapter has no narration yet', 'err'); return; }
    ra = readAloud(rc);
  }

  /** Plays the narration clips back to back as one timeline and keeps the passage being read on
   *  screen (the PC reader's `recap::Narration::focus` and `recap::scroll_target`, ported). Scrolling
   *  by hand stops following until ~6 s without touching, or "Back to narration". */
  function readAloud(rc) {
    const beats = arr(rc.beats).slice().sort((a, b) => a.start_ms - b.start_ms);
    const clips = arr(rc.clips).slice().sort((a, b) => a.start_ms - b.start_ms);
    const total = Math.max(+rc.total_ms || 0, clips[clips.length - 1].end_ms, beats[beats.length - 1].end_ms);
    const key = 'mv.aloud.' + chId;
    const audio = raAudio;
    const next = new Audio(); // warms the next clip
    next.preload = 'auto';
    const sig = new AbortController();
    const on = (t, ev, f, o) => t.addEventListener(ev, f, { ...o, signal: sig.signal });
    let ci = -1, pending = null, follow = true, lastUser = 0, raf = 0, lastTs = 0, fy = scrollY;
    let shownBeat = -1, shownSec = -1, lastSave = 0, done = false, snap = true;

    // ----- timeline (ms) -----
    const pos = () => {
      if (ci < 0) return 0;
      const t = pending != null ? pending : audio.currentTime || 0;
      return Math.min(total, clips[ci].start_ms + t * 1000);
    };
    const beatAt = ms => { let lo = 0, hi = beats.length - 1, k = 0; while (lo <= hi) { const mid = (lo + hi) >> 1; if (beats[mid].start_ms <= ms) { k = mid; lo = mid + 1; } else hi = mid - 1; } return k; };
    const frac = (t, a, z) => (z > a ? (clamp(t, a, z) - a) / (z - a) : 0);
    const whole = (x, page) => x.page === page && x.y0 <= 0 && x.y1 >= 1;
    function focus(ms) {
      const i = beatAt(ms), b = beats[i];
      if (b.y0 > 0 || b.y1 < 1) return { page: b.page, y: b.y0 + (b.y1 - b.y0) * frac(ms, b.start_ms, b.end_ms) };
      // Whole-page passages run down the page together, like someone reading along.
      let a = i, z = i;
      while (a > 0 && whole(beats[a - 1], b.page)) a--;
      while (z + 1 < beats.length && whole(beats[z + 1], b.page)) z++;
      return { page: b.page, y: frac(ms, beats[a].start_ms, beats[z].end_ms) };
    }
    /** Where narration of a reader page starts (or of the next narrated page after it). */
    function pageStart(p) {
      let best = null;
      for (const b of beats) if (b.page >= p && (!best || b.page < best.page || (b.page === best.page && b.start_ms < best.start_ms))) best = b;
      return best ? best.start_ms : null;
    }
    function scrollTarget(top, ph, vh, f) {
      if (ph <= vh) return top - (vh - ph) / 2;
      return clamp(top + ph * clamp(f, 0, 1) - vh * 0.4, top, top + ph - vh);
    }

    // ----- mini player -----
    const playBtn = h('button', { class: 'icon-btn ra-play', type: 'button', 'aria-label': 'Play', onclick: () => toggle() }, icon('play'));
    const cap = h('div', { class: 'ra-cap' }, beats[0].text || 'Narration');
    const time = h('div', { class: 'ra-time' }, '0:00 / ' + fmtTime(total / 1000));
    const bar = h('i');
    const pills = h('div', { class: 'ra-pills', hidden: true },
      h('button', { class: 'wp-pill', type: 'button', onclick: () => attach() }, icon('headphones'), 'Back to narration'),
      h('button', { class: 'wp-pill', type: 'button', onclick: () => fromHere() }, icon('play'), 'Read from here'));
    const el = h('div', { class: 'ra', role: 'region', 'aria-label': 'Read aloud' }, pills,
      h('div', { class: 'ra-prog', 'aria-hidden': 'true' }, bar),
      h('div', { class: 'ra-row' }, playBtn, h('div', { class: 'ra-info' }, cap, time), iconBtn('close', 'Stop reading aloud', () => close())));
    root.append(el);
    root.classList.add('aloud');
    aloudBtn.classList.add('on');
    aloudBtn.setAttribute('aria-pressed', 'true');
    if (Player) Player.pause();

    function setIcon() {
      const playing = !audio.paused;
      playBtn.replaceChildren(icon(playing ? 'pause' : done ? 'rotl' : 'play'));
      playBtn.setAttribute('aria-label', playing ? 'Pause' : done ? 'Read again' : 'Play');
      try { if ('mediaSession' in navigator) navigator.mediaSession.playbackState = playing ? 'playing' : 'paused'; } catch { /* ignore */ }
    }
    function save(force) {
      const now = Date.now();
      if (!force && now - lastSave < 5000) return;
      lastSave = now;
      const ms = pos();
      if (done || ms < 3000 || ms > total - 3000) LS.del(key); else LS.set(key, Math.floor(ms));
    }

    // ----- playback -----
    function load(i, off, play) {
      ci = i; pending = off > 0.05 ? off : null;
      audio.src = clips[i].url;
      if (play) audio.play().catch(blocked);
      const nx = clips[i + 1];
      if (nx) { next.src = nx.url; try { next.load(); } catch { /* ignore */ } }
    }
    function blocked(e) {
      if (!ctx.alive || !e || e.name !== 'NotAllowedError') return;
      setIcon();
      toast('Tap play to start reading aloud', 'info', 3200);
    }
    function seek(ms, play) {
      ms = clamp(ms, 0, Math.max(0, total - 1));
      done = false;
      let i = clips.findIndex(c => c.end_ms > ms);
      if (i < 0) i = clips.length - 1;
      const off = Math.max(0, (ms - clips[i].start_ms) / 1000);
      if (i === ci && pending == null && audio.readyState >= 1) {
        audio.currentTime = off;
        if (play && audio.paused) audio.play().catch(blocked);
      } else load(i, off, play);
      snap = true;
      setIcon();
    }
    function toggle() {
      if (audio.paused) {
        if (ci < 0 || done) seek(done ? 0 : pos(), true);
        else audio.play().catch(e => { blocked(e); if (e && e.name !== 'NotAllowedError') toast('Can’t play: ' + e.message, 'err'); });
      } else audio.pause();
    }
    on(audio, 'loadedmetadata', () => { if (pending != null) { try { audio.currentTime = pending; } catch { /* ignore */ } pending = null; } });
    on(audio, 'play', setIcon);
    on(audio, 'pause', () => { setIcon(); save(true); });
    on(audio, 'ended', () => {
      if (ci + 1 < clips.length) { load(ci + 1, 0, true); return; }
      done = true; follow = true; pills.hidden = true;
      cap.textContent = 'End of the narration';
      save(true); setIcon();
    });
    on(audio, 'error', () => { if (ctx.alive && audio.getAttribute('src')) { toast('Couldn’t play the narration', 'err'); setIcon(); } });

    // ----- following -----
    function attach() { follow = true; pills.hidden = true; fy = scrollY; snap = true; }
    function detach() { if (paged || !follow) return; follow = false; pills.hidden = false; }
    function nudge() { lastUser = performance.now(); detach(); }
    function fromHere() {
      const ms = pageStart(cur);
      if (ms == null) { toast('The narration ends before this page', 'info'); return; }
      seek(ms, true);
      attach();
    }
    function userPage(i) {
      const ms = pageStart(i);
      if (ms == null) { audio.pause(); return; }
      seek(ms, !audio.paused);
      attach();
    }
    const userKeys = ['ArrowDown', 'ArrowUp', 'PageDown', 'PageUp', 'Home', 'End', ' '];
    on(window, 'touchmove', e => { if (!sheets.length && !el.contains(e.target)) nudge(); }, { passive: true });
    on(window, 'wheel', () => { if (!sheets.length) nudge(); }, { passive: true });
    on(window, 'keydown', e => { if (!paged && userKeys.includes(e.key) && !(e.target.closest && e.target.closest('input'))) nudge(); });
    // Momentum after the finger lifts still counts as the reader's own scrolling.
    on(window, 'scroll', () => { if (!follow) lastUser = performance.now(); }, { passive: true });

    function followTo(ms, dt) {
      const f = focus(ms);
      if (paged) { if (f.page !== cur) engine.goTo(f.page); return; }
      const s = engine.slot && engine.slot(f.page);
      if (!s) return;
      const r = s.getBoundingClientRect();
      const vh = innerHeight - el.offsetHeight - 12;
      const max = Math.max(0, document.documentElement.scrollHeight - innerHeight);
      const target = clamp(scrollTarget(r.top + scrollY, r.height, vh, f.y), 0, max);
      // Something else moved the page (an image above it loaded): start from where it is.
      if (Math.abs(scrollY - fy) > 3) fy = scrollY;
      // Long jumps go straight there instead of gliding past every page in between.
      if (snap && Math.abs(target - fy) > innerHeight * 2.5) fy = target;
      else {
        const k = 1 - Math.exp(-dt * 5);
        fy += (target - fy) * k;
        if (Math.abs(target - fy) < 0.5) fy = target;
      }
      snap = false;
      if (Math.abs(fy - scrollY) >= 0.5) scrollTo({ top: fy, behavior: 'instant' });
    }
    function tick(ts) {
      raf = requestAnimationFrame(tick);
      const dt = lastTs ? Math.min(0.1, (ts - lastTs) / 1000) : 1 / 60;
      lastTs = ts;
      const ms = pos();
      const sec = Math.floor(ms / 1000);
      if (sec !== shownSec) {
        shownSec = sec;
        time.textContent = fmtTime(sec) + ' / ' + fmtTime(total / 1000);
        bar.style.width = (total ? clamp(ms / total, 0, 1) * 100 : 0) + '%';
      }
      const b = beatAt(ms);
      if (b !== shownBeat && !done) { shownBeat = b; cap.textContent = beats[b].text || '…'; }
      const playing = !audio.paused;
      if (!follow && playing && performance.now() - lastUser > 6000) attach();
      if (follow && (playing || snap)) followTo(ms, dt);
      if (playing) save(false);
    }
    raf = requestAnimationFrame(tick);

    // ----- lock screen -----
    if ('mediaSession' in navigator) {
      const ms = navigator.mediaSession;
      try {
        const m = mangaKey && cache.get(mangaKey);
        const cov = m && m.data.cover ? new URL(m.data.cover, location.href).href : new URL('/icon-512.png', location.href).href;
        ms.metadata = new MediaMetadata({ title: chLabel(ch), artist: ttlS.textContent || 'MangaVox', album: 'Read aloud', artwork: [{ src: cov, sizes: '512x512' }] });
      } catch { /* unsupported */ }
      const set = (a, f) => { try { ms.setActionHandler(a, f); } catch { /* unsupported */ } };
      set('play', () => toggle());
      set('pause', () => audio.pause());
      set('seekbackward', () => seek(pos() - 10000, !audio.paused));
      set('seekforward', () => seek(pos() + 10000, !audio.paused));
      set('seekto', d => { if (d && isFinite(d.seekTime)) seek(d.seekTime * 1000, !audio.paused); });
      set('previoustrack', null);
      set('nexttrack', null);
    }

    function close() {
      if (sig.signal.aborted) return;
      save(true);
      sig.abort();
      cancelAnimationFrame(raf);
      audio.pause(); audio.removeAttribute('src'); audio.load();
      next.removeAttribute('src');
      el.remove();
      root.classList.remove('aloud');
      aloudBtn.classList.remove('on');
      aloudBtn.removeAttribute('aria-pressed');
      if (ra === api3) ra = null;
      if (Player) Player.bindSession();
    }
    ctx.onLeave(close);

    // Pick up where the narration was left, else from the page on screen.
    const saved = +LS.get(key, 0) || 0;
    const startMs = saved > 3000 && saved < total - 3000 ? saved : cur > 0 ? pageStart(cur) || 0 : 0;
    seek(startMs, true);
    const api3 = { close, nudge, userPage };
    return api3;
  }

  // ----- sheets -----
  function settingsSheet() {
    openSheet((p, s) => {
      const dirBox = h('div', { hidden: !paged },
        h('h3', { class: 'sheet-h2' }, 'Page direction'),
        segmented([['rtl', 'Right to left'], ['ltr', 'Left to right']], rtl ? 'rtl' : 'ltr', v => {
          rtl = v === 'rtl'; LS.set('mv.reader.dir', v); root.classList.toggle('rtl', rtl);
        }, 'Page direction'),
        h('p', { class: 'muted small' }, rtl ? 'Manga style: tap or swipe left for the next page.' : 'Comic style: tap or swipe right for the next page.'));
      p.append(h('div', { class: 'grip', 'aria-hidden': 'true' }),
        h('h2', { class: 'sheet-h' }, 'Reading'),
        h('h3', { class: 'sheet-h2' }, 'Mode'),
        segmented([['vertical', 'Vertical scroll', 'scroll'], ['paged', 'Pages', 'pages']], paged ? 'paged' : 'vertical', v => {
          LS.set('mv.reader.mode', v);
          readerJump = { id: chId, page: cur };
          s.close();
          setTimeout(() => { if (ctx.alive) route(); }, 30);
        }, 'Reading mode'),
        dirBox,
        h('button', { class: 'btn ghost block', type: 'button', onclick: () => openPc({ page: 'library', manga_id: ch.manga_id }) }, icon('pc'), 'Open this manga on PC'));
    }, { label: 'Reading settings' });
  }
  function chaptersSheet() {
    openSheet((p, s) => {
      const list = h('div', { class: 'chlist' }, spinner('Loading chapters'));
      p.append(iconBtn('close', 'Close', () => s.close(), 'sheet-x'), h('h2', { class: 'sheet-h', style: { padding: '18px 16px 0' } }, 'Chapters'), list);
      getManga(ch.manga_id).then(m => {
        if (s.closed) return;
        const chs = arr(m.chapters).slice().reverse();
        list.replaceChildren(...chs.map(c => {
          const r = chRow(c, String(c.id) === String(chId));
          r.addEventListener('click', e => { e.preventDefault(); location.replace('#/read/' + enc(c.id)); });
          return r;
        }));
        const curEl = list.querySelector('.cur');
        if (curEl) requestAnimationFrame(() => curEl.scrollIntoView({ block: 'center' }));
      }).catch(e => { if (!s.closed) list.replaceChildren(errorState(e)); });
    }, { cls: 'tall', label: 'Chapters' });
  }
}

/* ======================================================================
   Studio
   ====================================================================== */
function viewStudio(ctx, q) {
  setTitle('Studio');
  const focusManga = q.get('manga');
  if (focusManga) setBack('#/manga/' + enc(focusManga));
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
    const pct = Math.round(clamp(+j.progress || 0, 0, 1) * 100);
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
    jobs = arr(jobs);
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
      const k = arr(r.jobs).length;
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
    const chs = arr(m.chapters);
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
    const ms = arr(data && data.manga);
    mangaWrap.replaceChildren(...ms.map(mangaBlock));
    if (!ms.length) mangaWrap.append(stateBox('download', 'No downloaded chapters', 'Download chapters on your PC first. Studio works from downloaded chapters.'));
  }
  function draw(d) {
    data = d || {};
    const ms = arr(data.manga);
    if (!open.size && ms.length) open.add(String(ms[0].id));
    body.replaceChildren(
      data.ready
        ? h('div', { class: 'banner' }, icon('check'), h('div', null, h('b', null, 'Studio is ready'), 'Pick chapters and what to make. Jobs run on your PC.'))
        : h('div', { class: 'banner warn' }, icon('alert'), h('div', null, h('b', null, 'Studio isn’t ready'), data.reason || 'Finish Studio setup on your PC first.')),
      h('h2', { class: 'sec-h' }, 'Chapters'),
      mangaWrap,
      ms.length ? h('div', { class: 'genbar' },
        segmented([['audio', 'Narration'], ['video', 'Video'], ['full', 'Full recap']], what, v => { what = v; LS.set('mv.studio.what', v); updateGen(); }, 'What to generate'),
        genBtn, hint) : null);
    drawManga();
    updateGen();
    if (focusManga) {
      const el = [...mangaWrap.children].find(c => c.classList.contains('open'));
      if (el) setTimeout(() => el.scrollIntoView({ block: 'start', behavior: 'smooth' }), 60);
    }
  }
  const load = () => api('/api/studio').then(d => { if (ctx.alive) draw(d); }, e => { if (ctx.alive && e.status !== 401) { if (data) toast(e.message, 'err'); else body.replaceChildren(errorState(e, () => route())); } });
  const refreshBtn = iconBtn('refresh', 'Refresh Studio', () => { refreshBtn.classList.add('busy'); load().finally(() => refreshBtn.classList.remove('busy')); });
  ctx.refresh = () => Promise.all([load(), poll()]);
  setActions(refreshBtn);
  load();
}

/* ======================================================================
   Listen: media list, mini player, studio video sheet
   ====================================================================== */
const posKey = id => 'mv.pos.' + id;
function coverForManga(name) {
  const lib = cache.get('/api/library');
  if (!lib || !name) return null;
  const m = arr(lib.data).find(x => x.title === name);
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
  root.append(
    h('div', { class: 'pl-row' }, h('div', { class: 'pl-info' }, tEl, sEl), skipBtn(-15, () => skip(-15)), playBtn, skipBtn(15, () => skip(15)), rateBtn,
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
      const art = coverForManga(it.manga) || new URL('/icon-512.png', location.href).href;
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
    bindSession();
    session(it);
    seek.value = '0'; setRange(seek); curT.textContent = '0:00'; durT.textContent = '–:––';
    audio.play().catch(() => setIcon());
    emit();
  }
  function toggle() { if (!cur()) return; if (audio.paused) audio.play().catch(e => toast('Can’t play: ' + e.message, 'err')); else audio.pause(); }
  function skip(d) { if (!isFinite(audio.duration)) return; audio.currentTime = clamp(audio.currentTime + d, 0, audio.duration - 0.25); ui(); posState(); }
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
  function bindSession() {
    if (!('mediaSession' in navigator)) return;
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
  bindSession();
  return {
    play(items, i) { queue = items.slice(); load(i); },
    pause() { audio.pause(); },
    current: cur,
    playing: () => !audio.paused,
    bindSession() {
      bindSession();
      const it = cur();
      if (it) session(it);
      else if ('mediaSession' in navigator) { try { navigator.mediaSession.metadata = null; navigator.mediaSession.playbackState = 'none'; } catch { /* ignore */ } }
    },
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
  setBack('#/more');
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
    items = arr(d);
    root.replaceChildren(
      h('div', { class: 'filters' }, segmented([['all', 'All'], ['audio', 'Audio'], ['video', 'Video']], filter, v => { filter = v; LS.set('mv.listen.filter', v); fill(); }, 'Filter media')),
      listBox);
    fill();
  }
  const load = () => swr(ctx, '/api/media', (d, fromCache) => { draw(d); if (fromCache) ctx.restoreScroll(); }, e => root.replaceChildren(errorState(e, () => route())));
  const refreshBtn = iconBtn('refresh', 'Refresh', () => { refreshBtn.classList.add('busy'); load().finally(() => refreshBtn.classList.remove('busy')); });
  ctx.refresh = load;
  setActions(refreshBtn);
  load();
}

/* ======================================================================
   Movies + Anime catalogs (shared pieces)
   ====================================================================== */
function metaLine(it, extra) {
  const score = Number(it.score);
  const genres = arr(it.genres).slice(0, 3);
  const anime = it.kind === 'anime';
  const eps = Number(it.episodes);
  return h('div', { class: 'meta' },
    score > 0 && h('span', { class: 'star' }, icon('star'), score.toFixed(1)),
    it.year && h('span', null, String(it.year)),
    extra && extra.runtime && h('span', null, fmtRuntime(extra.runtime)),
    anime
      ? [it.format && h('span', { class: 'tag' }, String(it.format)), eps > 0 && h('span', null, eps + (eps === 1 ? ' episode' : ' episodes')), it.status && h('span', null, String(it.status))]
      : h('span', { class: 'tag' }, it.kind === 'series' ? 'Series' : 'Movie'),
    genres.length > 0 && h('span', null, genres.join(' · ')));
}
function titleOrLogo(it, tag = 'h2') {
  const t = h(tag, null, it.title || '');
  const u = imgUrl(it.logo, W.logo);
  if (!u) return t;
  const img = h('img', { class: 'logo', alt: it.title || '', decoding: 'async', referrerpolicy: 'no-referrer' });
  img.addEventListener('error', () => img.replaceWith(t), { once: true });
  img.src = u;
  return img;
}
function posterCard(it, onOpen, caption) {
  const score = Number(it.score);
  return h('button', { class: 'pcard', type: 'button', 'aria-label': it.title || 'Title', onclick: () => onOpen(it) },
    art(it.poster || it.background, W.poster, '', it.title || '?'),
    caption && score > 0 && h('span', { class: 'rib' }, icon('star'), score.toFixed(1)),
    caption && h('div', { class: 'pcard-t' }, it.title || ''),
    caption && h('div', { class: 'pcard-s' }, [it.year, it.kind === 'anime' ? it.format : it.kind === 'series' ? 'Series' : 'Movie'].filter(Boolean).join(' · ')));
}
function top10Card(it, i, onOpen) {
  return h('button', { class: 'top10', type: 'button', 'aria-label': '#' + (i + 1) + ' ' + (it.title || ''), onclick: () => onOpen(it) },
    h('span', { class: 'num', 'aria-hidden': 'true' }, String(i + 1)), art(it.poster || it.background, W.poster, '', it.title || '?'));
}
function heroEl(it, cfg) {
  const el = h('div', { class: 'hero', role: 'group', 'aria-label': 'Featured: ' + (it.title || '') },
    art(it.background || it.poster, W.hero, '', null, true),
    h('div', { class: 'hero-in' },
      titleOrLogo(it),
      metaLine(it),
      it.description && h('p', null, it.description),
      h('div', { class: 'hero-acts' },
        h('button', { class: 'btn white', type: 'button', onclick: e => { e.stopPropagation(); cfg.play(it); } }, icon('play'), 'Play'),
        h('button', { class: 'btn glass', type: 'button', onclick: e => { e.stopPropagation(); cfg.open(it); } }, icon('info'), 'Details'))));
  el.addEventListener('click', () => cfg.open(it));
  return el;
}
const catState = {
  movies: { tab: LS.get('mv.movies.tab', 'home'), q: '' },
  anime: { tab: LS.get('mv.anime.tab', 'home'), q: '' },
};

function catalogView(ctx, cfg) {
  const st = catState[cfg.key];
  if (!cfg.tabs.some(t => t[0] === st.tab)) st.tab = cfg.tabs[0][0];
  setTitle(cfg.title);
  const content = h('div');
  const searchIn = h('input', { type: 'search', placeholder: cfg.placeholder, 'aria-label': cfg.placeholder, value: st.q, enterkeyhint: 'search', autocomplete: 'off', autocapitalize: 'off', spellcheck: 'false' });
  const clearBtn = h('button', { class: 'search-x', type: 'button', 'aria-label': 'Clear search', hidden: !st.q, onclick: () => { searchIn.value = ''; doSearch(); searchIn.focus(); } }, icon('close'));
  const tabsBox = h('div', { class: 'cat-tabs' },
    segmented(cfg.tabs, st.tab, v => { st.tab = v; LS.set('mv.' + cfg.key + '.tab', v); load(); scrollTo(0, 0); }, cfg.title + ' sections'));
  view.append(h('div', { class: 'page' }, h('label', { class: 'search' }, icon('search'), searchIn, clearBtn), tabsBox, content));
  const skel = () => h('div', { 'aria-busy': 'true' }, h('div', { class: 'hero skel' }),
    [0, 1].map(() => h('div', { class: 'sec' }, h('div', { class: 'skel skel-line', style: { width: '40%', margin: '0 16px 12px', height: '16px' } }),
      h('div', { class: 'hscroll' }, [0, 1, 2, 3].map(() => h('div', { class: 'pcard' }, h('div', { class: 'art skel', style: { aspectRatio: '2 / 3' } })))))));

  function rowEl(r) {
    const items = arr(r.items);
    if (!items.length) return null;
    return h('section', { class: 'sec', 'aria-label': r.title || '' },
      h('h2', { class: 'sec-h' }, r.title || ''),
      h('div', { class: 'hscroll' }, r.top10 ? items.slice(0, 10).map((it, i) => top10Card(it, i, cfg.open)) : items.map(it => posterCard(it, cfg.open))));
  }
  function drawHome(d) {
    const rows = arr(d && d.rows);
    const heroItem = (d && d.hero) || (rows[0] && arr(rows[0].items)[0]);
    const els = [];
    if (heroItem) { try { els.push(heroEl(heroItem, cfg)); } catch (e) { console.error(e); } }
    for (const r of rows) { try { const el = rowEl(r); if (el) els.push(el); } catch (e) { console.error(e); } }
    content.replaceChildren(...els);
    if (!els.length) content.append(stateBox(cfg.icon, 'Nothing to show yet', (d && d.error && String(d.error)) || cfg.empty, h('button', { class: 'btn', type: 'button', onclick: () => load() }, icon('refresh'), 'Try again')));
  }
  let searchSeq = 0;
  const runSearch = async () => {
    const q = searchIn.value.trim();
    st.q = q;
    clearBtn.hidden = !q;
    if (!q) { tabsBox.hidden = false; load(); return; }
    tabsBox.hidden = true;
    const seq = ++searchSeq;
    content.replaceChildren(h('div', { class: 'pgrid', 'aria-busy': 'true' }, Array.from({ length: 6 }, () => h('div', { class: 'pcard' }, h('div', { class: 'art skel', style: { aspectRatio: '2 / 3' } })))));
    try {
      const list = arr(await api(cfg.searchPath(q)));
      if (!ctx.alive || seq !== searchSeq) return;
      content.replaceChildren(list.length
        ? h('div', null, h('h2', { class: 'sec-h' }, 'Results', h('span', { class: 'count' }, String(list.length))), h('div', { class: 'pgrid' }, list.map(it => posterCard(it, cfg.open, true))))
        : stateBox('search', 'No results', 'Nothing matched “' + q + '”.'));
    } catch (e) { if (ctx.alive && seq === searchSeq) content.replaceChildren(errorState(e, runSearch)); }
  };
  const doSearch = debounce(runSearch, 350);
  searchIn.addEventListener('input', () => { clearBtn.hidden = !searchIn.value; doSearch(); });
  searchIn.addEventListener('keydown', e => { if (e.key === 'Enter') { searchIn.blur(); runSearch(); } });

  function load() {
    searchSeq++;
    tabsBox.hidden = false;
    const path = cfg.homePath(st.tab);
    if (!cache.get(path)) content.replaceChildren(skel());
    return swr(ctx, path, (d, fromCache) => { if (!st.q) { drawHome(d); if (fromCache) ctx.restoreScroll(); } }, e => content.replaceChildren(errorState(e, load)));
  }
  ctx.refresh = () => (st.q ? runSearch() : load());
  if (st.q) runSearch(); else load();
}

/** Full-screen player route for a title. Opening it from a detail sheet replaces the sheet's
 *  history entry, and the sheet comes back when the player closes. */
function startWatch(p, reopen) {
  if (Player) Player.pause();
  const q = new URLSearchParams();
  for (const k in p) if (p[k] != null && p[k] !== '') q.set(k, String(p[k]));
  const href = '#/watch?' + q;
  pendingReopen = reopen && ctx ? { key: ctx.key, open: reopen } : null;
  if (sheets.length) location.replace(href); else location.hash = href;
}
async function playOnPc(kind, b) {
  toast('Finding sources on your PC…', 'info', 2200);
  try {
    if (kind === 'anime') await post('/api/anime/play', { mal: b.mal, title: b.title, episode: b.episode || 1 });
    else await post('/api/movies/play', { imdb: b.imdb, kind: kind === 'series' ? 'series' : 'movie', title: b.title, season: b.season, episode: b.episode });
    toast('Playing on your PC: ' + (b.label || b.title || ''), 'ok', 3200);
    return true;
  } catch (e) { toast(e.message, 'err', 4500); return false; }
}

/* ======================================================================
   Movies
   ====================================================================== */
const itemKey = it => it.imdb || it.id;
function playMovie(it) {
  if (it.kind === 'series') { openMovie(it); return; }
  startWatch({ kind: 'movie', imdb: itemKey(it), title: it.title, bg: it.background || it.poster });
}
function viewMovies(ctx) {
  catalogView(ctx, {
    key: 'movies', title: 'Movies', icon: 'film', placeholder: 'Search movies & series',
    tabs: [['home', 'Home'], ['movies', 'Movies'], ['series', 'Series']],
    homePath: tab => '/api/movies?tab=' + enc(tab),
    searchPath: q => '/api/movies/search?q=' + enc(q),
    empty: 'The catalog is still loading on your PC. Try again in a moment.',
    open: openMovie, play: playMovie,
  });
}

function detailShell(p, s, it) {
  const body = h('div', { class: 'md' });
  p.append(iconBtn('close', 'Close', () => s.close(), 'sheet-x'), body);
  return body;
}
function clampP(text) {
  if (!text) return null;
  const el = h('p', { class: 'clamp', role: 'button', tabindex: '0', 'aria-label': 'Description, tap to expand' }, text);
  el.addEventListener('click', () => el.classList.toggle('open'));
  return el;
}
function epRow({ n, title, sub, thumb, filler, onPlay, onPc, label }) {
  return h('div', { class: 'ep' },
    h('button', { class: 'ep-main', type: 'button', 'aria-label': 'Play ' + label + ' on this phone', onclick: onPlay },
      h('div', { class: 'ep-th' }, thumb ? art(thumb, W.thumb) : h('div', { class: 'ep-n', 'aria-hidden': 'true' }, String(n)), icon('play')),
      h('div', { class: 'ep-b' }, h('b', null, n + '. ' + (title || 'Episode ' + n)),
        (sub || filler) && h('span', null, filler && h('span', { class: 'filler' }, 'Filler'), filler && sub ? ' · ' : '', sub || ''))),
    iconBtn('pc', 'Play ' + label + ' on PC', onPc));
}

function openMovie(it) {
  openSheet((p, s) => {
    const body = detailShell(p, s, it);
    let season = null;
    const reopen = () => openMovie(it);
    function draw(d, full) {
      const series = d.kind === 'series';
      const seasons = arr(d.seasons).filter(x => x && Array.isArray(x.episodes));
      if (season == null && seasons.length) season = (seasons.find(x => +x.season > 0) || seasons[0]).season;
      const base = { imdb: itemKey(d), title: d.title, bg: d.background || d.poster };
      const watchEp = (sn, ep, t) => startWatch({ ...base, kind: 'series', season: sn, episode: ep, ept: t }, reopen);
      const pcEp = (sn, ep) => playOnPc('series', { ...base, season: sn, episode: ep, label: d.title + ' S' + sn + 'E' + ep });
      const eps = h('div');
      const fillEps = () => {
        const se = seasons.find(x => x.season === season);
        eps.replaceChildren(...arr(se && se.episodes).map(ep => epRow({
          n: ep.episode, title: ep.title, sub: ep.overview, thumb: ep.thumbnail, label: 'S' + season + ' E' + ep.episode,
          onPlay: () => watchEp(+season, +ep.episode, ep.title), onPc: () => pcEp(+season, +ep.episode),
        })));
      };
      const chips = seasons.map(x => {
        const b = h('button', { class: 'chip', type: 'button', 'aria-pressed': String(x.season === season), onclick: () => {
          season = x.season; chips.forEach(c => c.setAttribute('aria-pressed', String(c === b))); fillEps();
        } }, +x.season === 0 ? 'Specials' : 'Season ' + x.season);
        return b;
      });
      const first = seasons.length && arr((seasons.find(x => x.season === season) || {}).episodes)[0];
      const cast = arr(d.cast).slice(0, 8).map(c => typeof c === 'string' ? c : (c && c.name) || '').filter(Boolean);
      const play = series
        ? (first ? h('button', { class: 'btn primary block', type: 'button', onclick: () => watchEp(+season, +first.episode, first.title) }, icon('play'), 'Play S' + season + ' E' + first.episode)
          : h('button', { class: 'btn primary block', type: 'button', disabled: true }, icon('play'), full ? 'No episodes listed' : 'Loading episodes…'))
        : h('button', { class: 'btn primary block', type: 'button', onclick: () => startWatch({ ...base, kind: 'movie' }, reopen) }, icon('play'), 'Play');
      body.replaceChildren(
        h('div', { class: 'md-hero' }, art(d.background || d.poster, W.hero, '', null, true), h('div', { class: 'hero-in' }, titleOrLogo(d))),
        h('div', { class: 'md-body' },
          metaLine(d, d),
          h('div', { class: 'md-acts' }, play,
            h('div', { class: 'row' },
              h('button', { class: 'btn', type: 'button', onclick: () => series && first ? pcEp(+season, +first.episode) : playOnPc(d.kind, { ...base, label: d.title }) }, icon('pc'), 'Play on PC'),
              h('button', { class: 'btn', type: 'button', onclick: () => openPc({ page: 'movies', imdb: itemKey(d), kind: d.kind, title: d.title }) }, icon('info'), 'Open on PC'))),
          clampP(d.description),
          cast.length > 0 && h('div', { class: 'cast' }, h('b', null, 'Cast: '), cast.join(', ')),
          !full && h('div', null, h('div', { class: 'skel skel-line', style: { width: '60%' } }), h('div', { class: 'skel skel-line', style: { width: '45%' } })),
          seasons.length > 1 && h('div', { class: 'seasons' }, h('div', { class: 'hscroll' }, chips)),
          seasons.length > 0 && h('h3', { class: 'eps-h' }, 'Episodes'),
          seasons.length > 0 && eps));
      fillEps();
    }
    draw(it, false);
    api('/api/movies/' + enc(itemKey(it)) + (it.kind ? '?kind=' + enc(it.kind) : ''))
      .then(d => { if (!s.closed) draw({ ...it, ...d }, true); })
      .catch(e => { if (!s.closed) { draw(it, true); if (e.status !== 404) toast(e.message, 'err'); } });
  }, { cls: 'tall', label: it.title || 'Details' });
}

/* ======================================================================
   Anime
   ====================================================================== */
const malOf = it => String(it.mal || it.id || '');
function viewAnime(ctx) {
  catalogView(ctx, {
    key: 'anime', title: 'Anime', icon: 'tv', placeholder: 'Search anime',
    tabs: [['home', 'Home'], ['airing', 'Airing'], ['top', 'Top'], ['movies', 'Movies']],
    homePath: tab => '/api/anime?tab=' + enc(tab),
    searchPath: q => '/api/anime/search?q=' + enc(q),
    empty: 'Anime is still loading on your PC. Try again in a moment.',
    open: openAnime,
    play: it => startWatch({ kind: 'anime', mal: malOf(it), title: it.title, episode: 1, bg: it.background || it.poster }),
  });
}
function openAnime(it) {
  openSheet((p, s) => {
    const body = detailShell(p, s, it);
    const reopen = () => openAnime(it);
    const mal = malOf(it);
    let eps = [], nextPage = null, loadingMore = false, err = null;
    const base = { mal, title: it.title, bg: it.background || it.poster };
    const watchEp = (n, t) => startWatch({ ...base, kind: 'anime', episode: n, ept: t }, reopen);
    const pcEp = n => playOnPc('anime', { ...base, episode: n, label: it.title + ' · Episode ' + n });
    const list = h('div');
    const moreBtn = h('button', { class: 'btn ghost block more-eps', type: 'button', onclick: loadMore }, 'More episodes');
    function row(e) {
      const n = +e.episode;
      const aired = e.aired ? toDate(e.aired) : null;
      return epRow({ n, title: e.title, sub: aired ? aired.toLocaleDateString() : (e.aired ? String(e.aired) : ''), thumb: e.thumbnail, filler: !!e.filler,
        label: 'episode ' + n, onPlay: () => watchEp(n, e.title), onPc: () => pcEp(n) });
    }
    async function loadMore() {
      if (loadingMore || nextPage == null) return;
      loadingMore = true; moreBtn.classList.add('busy'); moreBtn.disabled = true;
      try {
        const r = await api('/api/anime/' + enc(mal) + '/episodes?page=' + enc(nextPage));
        if (s.closed) return;
        const add2 = arr(r.episodes);
        eps = eps.concat(add2);
        list.append(...add2.map(row));
        nextPage = r.next_page ?? null;
      } catch (e) { toast(e.message, 'err'); }
      loadingMore = false; moreBtn.classList.remove('busy'); moreBtn.disabled = false;
      moreBtn.hidden = nextPage == null;
    }
    function jumpForm(count) {
      const inp = h('input', { type: 'number', inputmode: 'numeric', min: '1', max: count > 0 ? String(count) : null, placeholder: count > 0 ? 'Episode 1–' + count : 'Episode number', 'aria-label': 'Episode number', enterkeyhint: 'go' });
      return h('form', { class: 'ep-jump', onsubmit: e => {
        e.preventDefault();
        const n = parseInt(inp.value, 10);
        if (!(n > 0) || (count > 0 && n > count)) { toast(count > 0 ? 'Pick an episode from 1 to ' + count : 'Enter an episode number', 'err'); return; }
        const known = eps.find(x => +x.episode === n);
        inp.blur();
        watchEp(n, known && known.title);
      } }, inp, h('button', { class: 'btn sm', type: 'submit' }, icon('play'), 'Play'));
    }
    function draw(d, full) {
      const count = Number(d.episodes) || 0;
      if (full && !eps.length && count > 0 && !err) eps = Array.from({ length: Math.min(count, 500) }, (_, i) => ({ episode: i + 1 }));
      const first = eps[0] ? +eps[0].episode : 1;
      const movie = /movie/i.test(String(d.format || ''));
      list.replaceChildren(...eps.map(row));
      moreBtn.hidden = nextPage == null;
      body.replaceChildren(
        h('div', { class: 'md-hero' }, art(d.background || d.poster, W.hero, '', null, true), h('div', { class: 'hero-in' }, h('h2', null, d.title || ''))),
        h('div', { class: 'md-body' },
          metaLine({ ...d, kind: 'anime' }),
          h('div', { class: 'md-acts' },
            h('button', { class: 'btn primary block', type: 'button', onclick: () => watchEp(first, eps[0] && eps[0].title) }, icon('play'), movie || count === 1 ? 'Play' : 'Play episode ' + first),
            h('div', { class: 'row' },
              h('button', { class: 'btn', type: 'button', onclick: () => pcEp(first) }, icon('pc'), 'Play on PC'),
              h('button', { class: 'btn', type: 'button', onclick: () => openPc({ page: 'anime', mal, title: d.title }) }, icon('info'), 'Open on PC'))),
          clampP(d.description),
          !full && h('div', null, h('div', { class: 'skel skel-line', style: { width: '60%' } }), h('div', { class: 'skel skel-line', style: { width: '45%' } })),
          err && h('p', { class: 'muted small' }, err),
          eps.length > 1 && h('h3', { class: 'eps-h' }, 'Episodes', count > 0 && h('span', { class: 'count' }, String(count))),
          (nextPage != null || eps.length > 50) && jumpForm(count),
          eps.length > 1 && list,
          eps.length > 1 && moreBtn));
    }
    draw(it, false);
    api('/api/anime/' + enc(mal))
      .then(d => { if (s.closed) return; eps = arr(d.episodes_list); nextPage = d.next_page ?? null; draw({ ...it, ...d, episodes: d.episodes ?? it.episodes }, true); })
      .catch(e => { if (s.closed) return; err = e.status === 404 ? '' : 'Couldn’t load the episode list: ' + e.message; draw(it, true); });
  }, { cls: 'tall', label: it.title || 'Details' });
}

/* ======================================================================
   Watch on this phone: full-screen player
   ====================================================================== */
function viewWatch(ctx, q) {
  const P = {};
  for (const [k, v] of q) P[k] = v;
  const kind = ['movie', 'series', 'anime', 'recap'].includes(P.kind) ? P.kind : 'movie';
  // A chapter's Studio recap: a file on the PC, so no sources, "Play on PC" or watch history.
  const recap = kind === 'recap';
  const season = P.season ? +P.season : null, episode = P.episode ? +P.episode : null;
  const epLabel = kind === 'series' && season != null && episode != null ? 'S' + season + ' · E' + episode : kind === 'anime' && episode != null ? 'Episode ' + episode : '';
  const fallback = recap ? (P.manga ? '#/manga/' + enc(P.manga) : '#/library') : kind === 'anime' ? '#/anime' : '#/movies';
  const recapKey = posKey('recap.' + (P.chapter || ''));
  setTitle(P.title || 'Watch');
  if (Player) Player.pause();

  const v = h('video', { playsinline: true, 'webkit-playsinline': true, preload: 'auto', 'x-webkit-airplay': 'allow', 'aria-label': P.title || 'Video' });
  const subsEl = h('div', { class: 'wp-subs', 'aria-live': 'off' });
  const gest = h('div', { class: 'wp-gest' });
  const loadT = h('b', null, recap ? 'Opening the recap…' : 'Finding the best source…');
  const loadS = h('span');
  const loadPc = h('button', { class: 'btn glass sm', type: 'button', hidden: true, onclick: () => fallbackPc() }, icon('pc'), 'Play on PC instead');
  const loader = h('div', { class: 'wp-load', role: 'status' }, h('div', { class: 'spin-ring' }), loadT, loadS, !recap && loadPc);
  const msg = h('div', { class: 'wp-msg', hidden: true });
  const fxL = h('div', { class: 'wp-fx l', 'aria-hidden': 'true' }, h('b', null, icon('rotl'), h('span')));
  const fxR = h('div', { class: 'wp-fx r', 'aria-hidden': 'true' }, h('b', null, icon('rotr'), h('span')));
  const playBig = h('button', { class: 'icon-btn wp-big', type: 'button', 'aria-label': 'Play', onclick: () => toggle() }, icon('play'));
  const scrub = h('input', { type: 'range', min: '0', max: '1000', value: '0', step: '1', 'aria-label': 'Seek', disabled: true });
  const curT = h('span', { class: 'wp-time' }, '0:00');
  const durT = h('span', { class: 'wp-time' }, '–:––');
  const fsBtn = iconBtn('fs', 'Full screen', () => toggleFs());
  const pipBtn = iconBtn('pip', 'Picture in picture', () => togglePip());
  const ccBtn = iconBtn('cc', 'Subtitles and speed', () => optionsSheet());
  const nextBtn = h('button', { class: 'wp-pill', type: 'button', hidden: true, onclick: () => goNext() }, icon('next'), 'Next episode');
  const ttl = h('div', { class: 'wp-ttl' }, h('b', null, P.title || 'Video'), h('span', null, [epLabel, P.ept].filter(Boolean).join(' · ')));
  const upNext = h('div', { class: 'wp-next', hidden: true });
  const bg = h('div', { class: 'wp-bg', 'aria-hidden': 'true' }, P.bg ? art(P.bg, W.hero, '', null, true) : null);
  const root = h('div', { class: 'wp ui loading', role: 'region', 'aria-label': 'Video player' },
    bg, v, subsEl, gest, fxL, fxR,
    h('div', { class: 'wp-ui' },
      h('div', { class: 'wp-top' }, iconBtn('back', 'Close player', () => leave()), ttl, ccBtn, pipBtn, !recap && iconBtn('pc', 'Play on PC instead', () => fallbackPc())),
      h('div', { class: 'wp-mid' }, skipBtn(-10, () => skip(-10)), playBig, skipBtn(10, () => skip(10))),
      h('div', { class: 'wp-bot' },
        h('div', { class: 'wp-scrub' }, scrub),
        h('div', { class: 'wp-row' }, curT, h('span', { class: 'wp-time', 'aria-hidden': 'true' }, '/'), durT, h('span', { class: 'sp' }), nextBtn, fsBtn))),
    loader, upNext, msg);
  view.append(root);
  const pipOk = !!(document.pictureInPictureEnabled || (v.webkitSupportsPresentationMode && typeof v.webkitSetPresentationMode === 'function'));
  pipBtn.hidden = !pipOk;

  let sess = null, stopped = false, offset = 0, startAt = 0, started = false, rate = 1;
  let subList = [], subIdx = -1, cues = null, subSeq = 0;
  let lastRep = 0, statT = null, loadSince = 0, zeroSince = 0, scrubbing = false;
  const direct = () => !sess || sess.mode === 'direct';
  const pos = () => (direct() ? v.currentTime : offset + v.currentTime) || 0;
  const dur = () => (sess && +sess.duration) || (direct() && isFinite(v.duration) ? v.duration : 0);
  const leave = () => goBack(fallback);
  const sid = () => enc(sess.id);

  // ----- loading / buffering overlay -----
  function setLoading(on, text) {
    root.classList.toggle('loading', on);
    if (text) loadT.textContent = text;
    if (on) {
      if (!loadSince) loadSince = Date.now();
      loadPc.hidden = Date.now() - loadSince < 12000;
      if (sess && !statT) pollStatus();
    } else { loadSince = 0; zeroSince = 0; loadS.textContent = ''; loadPc.hidden = true; clearTimeout(statT); statT = null; }
  }
  async function pollStatus() {
    statT = null;
    if (!ctx.alive || !sess || !root.classList.contains('loading')) return;
    try {
      const s = await api('/api/watch/' + sid() + '/status');
      if (!ctx.alive) return;
      if (s.state === 'error') { fail('Couldn’t play this', s.error || 'The source stopped working.', true); return; }
      const bits = [];
      if (s.peers != null) bits.push(s.peers + (s.peers === 1 ? ' peer' : ' peers'));
      if (s.down_bps) bits.push(fmtSize(s.down_bps) + '/s');
      if (s.progress != null && s.progress >= 0.01 && s.progress < 1) bits.push(Math.round(s.progress * 100) + '% downloaded');
      loadS.textContent = bits.join(' · ');
      if (s.peers === 0) {
        zeroSince = zeroSince || Date.now();
        if (Date.now() - zeroSince > 20000) loadT.textContent = 'Waiting for peers… this source may have no seeders right now.';
      } else zeroSince = 0;
    } catch { /* status is best effort */ }
    loadPc.hidden = Date.now() - loadSince < 12000;
    if (ctx.alive && root.classList.contains('loading') && !statT) statT = setTimeout(pollStatus, 1500);
  }

  // ----- messages -----
  function card(ic, title, text, actions, bad) {
    msg.replaceChildren(h('div', { class: 'wp-card', role: 'alertdialog', 'aria-label': title }, icon(ic, bad ? 'bad' : ''), h('h2', null, title), text && h('p', null, text), h('div', { class: 'wp-acts' }, actions)));
    msg.hidden = false;
    const b = msg.querySelector('button'); if (b) b.focus({ preventScroll: true });
  }
  function fail(title, text, canPc, retry) {
    canPc = canPc && !recap;
    setLoading(false);
    v.pause();
    showUi(true);
    card('alert', title, text, [
      canPc && h('button', { class: 'btn primary block', type: 'button', onclick: () => fallbackPc() }, icon('pc'), 'Play on PC instead'),
      retry && h('button', { class: 'btn block', type: 'button', onclick: retry }, icon('refresh'), 'Try again'),
      h('button', { class: 'btn ghost block', type: 'button', onclick: leave }, 'Back'),
    ], true);
  }
  async function fallbackPc() {
    const b = { imdb: P.imdb, mal: P.mal, title: P.title, season, episode, label: [P.title, epLabel].filter(Boolean).join(' · ') };
    stopSession();
    v.pause();
    if (await playOnPc(kind, b)) leave();
  }

  // ----- session -----
  async function begin() {
    msg.hidden = true;
    setLoading(true, recap ? 'Opening the recap…' : 'Finding the best source…');
    const body = { kind, title: P.title || '' };
    if (P.imdb) body.imdb = P.imdb;
    if (P.mal) body.mal = P.mal;
    if (P.chapter) body.chapter = P.chapter;
    if (season != null) body.season = season;
    if (episode != null) body.episode = episode;
    let s;
    const slow = setTimeout(() => { if (ctx.alive && !sess && !recap) { loadPc.hidden = false; loadT.textContent = 'Still looking… torrents can take a minute to start.'; } }, 12000);
    try { s = await post('/api/watch', body); }
    catch (e) {
      clearTimeout(slow);
      if (!ctx.alive) return;
      if (e.offline) fail('Can’t reach your PC', 'Make sure MangaVox is running on your PC and this phone is on the same Wi‑Fi.', false, begin);
      else if (e.status === 503) fail('FFmpeg is needed', e.message || 'Install FFmpeg on your PC to watch this on your phone.', true, begin);
      else if (e.status === 404) fail('Can’t play this here', e.message || 'No sources found for this title.', true, begin);
      else fail('Couldn’t start playback', e.message, true, begin);
      return;
    }
    clearTimeout(slow);
    if (!ctx.alive) { if (s && s.id) post('/api/watch/' + enc(s.id) + '/stop', {}, { keepalive: true }).catch(() => {}); return; }
    if (!s || !s.id || !s.url) { fail('Couldn’t start playback', 'Your PC sent an unexpected answer.', true, begin); return; }
    sess = s; stopped = false;
    if (s.title) ttl.firstChild.textContent = P.title || s.title;
    subList = arr(s.subtitles).filter(t => t && t.url);
    const pref = LS.get('mv.sub', null);
    // Remembered language: only auto-load online subtitles. Embedded tracks are extracted from
    // the torrent on the PC, which is slow, so those load only when picked in the menu.
    if (pref) { const i = subList.findIndex(t => (t.lang === pref || t.label === pref) && !/^e/.test(String(t.id || ''))); if (i >= 0) pickSub(i, true); }
    nextBtn.hidden = !s.next;
    scrub.disabled = !dur();
    durT.textContent = dur() ? fmtTime(dur()) : '–:––';
    mediaSession();
    // A recap's place is kept on this phone only (it is not watch history on the PC).
    const r = (recap ? +(LS.get(recapKey, null) || {}).t : +s.resume) || 0, d = +s.duration || 0;
    if (r > 20 && (!d || r < d - 40)) {
      setLoading(false);
      card('play', 'Resume watching?', 'You stopped at ' + fmtTime(r) + (d ? ' of ' + fmtTime(d) : '') + '.', [
        h('button', { class: 'btn primary block', type: 'button', onclick: () => play(r) }, icon('play'), 'Resume from ' + fmtTime(r)),
        h('button', { class: 'btn block', type: 'button', onclick: () => play(0) }, icon('rotl'), 'Start over'),
      ]);
    } else play(0);
  }
  function srcAt(t) {
    const u = String(sess.url);
    return t > 0 ? u + (u.includes('?') ? '&' : '?') + 'start=' + Math.floor(t) : u;
  }
  function play(t) {
    msg.hidden = true;
    setLoading(true, t > 0 ? 'Resuming…' : 'Starting…');
    if (direct()) { startAt = t; v.src = sess.url; }
    else { offset = Math.floor(t); v.src = srcAt(offset); }
    v.playbackRate = rate;
    v.play().catch(e => {
      if (e && e.name === 'NotAllowedError') { setLoading(false); showUi(true); }
    });
  }
  function seekTo(t) {
    if (!sess) return;
    const d = dur();
    t = clamp(t, 0, d ? d - 2 : Infinity);
    hideUpNext();
    if (direct()) { v.currentTime = t; return; }
    offset = Math.floor(t);
    setLoading(true, 'Seeking…');
    const wasPaused = v.paused && started;
    v.src = srcAt(offset);
    v.playbackRate = rate;
    if (wasPaused) v.addEventListener('loadeddata', () => setLoading(false), { once: true });
    else v.play().catch(() => {});
    updateTime();
  }
  function report(force) {
    if (!sess || stopped) return;
    const p = pos(), d = dur();
    if (!(p > 1)) return;
    const now = Date.now();
    if (!force && now - lastRep < 10000) return;
    lastRep = now;
    if (recap) { if (!d || p < d - 30) LS.set(recapKey, { t: Math.floor(p), d: Math.floor(d) }); else LS.del(recapKey); return; }
    post('/api/watch/' + sid() + '/progress', { pos: Math.floor(p), duration: Math.floor(d) || 0 }, { keepalive: true }).catch(() => {});
  }
  function stopSession() {
    if (!sess || stopped) return;
    report(true);
    stopped = true;
    post('/api/watch/' + sid() + '/stop', {}, { keepalive: true }).catch(() => {});
  }

  // ----- controls -----
  let hideT = null;
  function showUi(stay) {
    root.classList.add('ui');
    clearTimeout(hideT);
    if (!stay && !v.paused && !sheets.length) hideT = setTimeout(() => { if (!scrubbing) root.classList.remove('ui'); }, 3200);
  }
  function toggleUi() { if (root.classList.contains('ui')) { clearTimeout(hideT); root.classList.remove('ui'); } else showUi(); }
  function toggle() {
    if (!sess) return;
    if (v.paused) { if (!v.getAttribute('src')) play(pos()); else v.play().catch(e => toast('Can’t play: ' + e.message, 'err')); }
    else v.pause();
  }
  function updateTime(at) {
    const d = dur(), p = at != null ? at : pos();
    if (!scrubbing) {
      curT.textContent = fmtTime(p);
      scrub.value = d ? String(Math.round((p / d) * 1000)) : '0';
      setRange(scrub);
    }
    if (d) {
      let b = 0;
      try { const br = v.buffered; for (let i = 0; i < br.length; i++) if (br.start(i) <= v.currentTime + 1) b = Math.max(b, br.end(i)); } catch { /* ignore */ }
      scrub.style.setProperty('--b', clamp(((direct() ? 0 : offset) + b) / d, 0, 1) * 100 + '%');
    }
  }
  let skipTarget = null, skipT = null, burst = 0, burstT = null;
  function skip(dlt) {
    if (!sess) return;
    const d = dur();
    skipTarget = clamp((skipTarget != null ? skipTarget : pos()) + dlt, 0, d ? d - 2 : Infinity);
    burst += dlt;
    clearTimeout(burstT); burstT = setTimeout(() => { burst = 0; }, 800);
    const el = dlt < 0 ? fxL : fxR;
    el.querySelector('span').textContent = (burst < 0 ? '−' : '+') + Math.abs(burst) + 's';
    el.classList.remove('on'); void el.offsetWidth; el.classList.add('on');
    updateTime(skipTarget);
    clearTimeout(skipT);
    skipT = setTimeout(() => { const t = skipTarget; skipTarget = null; seekTo(t); }, direct() ? 0 : 650);
  }
  // Single tap toggles the controls; double tap on the left / right third seeks.
  let lastTap = 0, tapT = null;
  gest.addEventListener('click', e => {
    const now = performance.now();
    const x = e.clientX / innerWidth;
    const side = x < 0.36 ? -1 : x > 0.64 ? 1 : 0;
    if (side && now - lastTap < 300) {
      clearTimeout(tapT); lastTap = now;
      skip(side * 10);
      return;
    }
    lastTap = now;
    clearTimeout(tapT);
    if (side) tapT = setTimeout(toggleUi, 260); else toggleUi();
  });
  gest.addEventListener('dblclick', e => e.preventDefault());
  root.addEventListener('pointerdown', e => { if (e.target.closest('.wp-top, .wp-mid, .wp-bot')) showUi(); });

  scrub.addEventListener('input', () => {
    scrubbing = true; setRange(scrub); showUi(true);
    const d = dur(); if (d) curT.textContent = fmtTime((+scrub.value / 1000) * d);
  });
  scrub.addEventListener('change', () => {
    scrubbing = false;
    const d = dur(); if (d) seekTo((+scrub.value / 1000) * d);
    showUi();
  });

  // ----- fullscreen / orientation / PiP -----
  const fsEl = () => document.fullscreenElement || document.webkitFullscreenElement;
  async function toggleFs() {
    try {
      if (fsEl()) { await (document.exitFullscreen || document.webkitExitFullscreen).call(document); return; }
      const de = document.documentElement;
      if (de.requestFullscreen) {
        await de.requestFullscreen({ navigationUI: 'hide' });
        try { if (screen.orientation && screen.orientation.lock) await screen.orientation.lock('landscape'); } catch { /* not allowed */ }
      } else if (de.webkitRequestFullscreen) de.webkitRequestFullscreen();
      else if (v.webkitEnterFullscreen) {
        // iPhone: the system player. Our subtitle overlay is not visible there, so hand it the
        // selected subtitles as a native track (in sync for direct streams only).
        const t = subList[subIdx];
        if (t && direct()) {
          const tr = h('track', { kind: 'subtitles', label: t.label || t.lang || 'Subtitles', srclang: t.lang || 'und', src: t.url, default: true });
          v.append(tr);
          tr.track.mode = 'showing';
        }
        v.webkitEnterFullscreen();
      }
    } catch { toast('Full screen isn’t available here', 'err'); }
  }
  const onFs = () => {
    const on = !!fsEl();
    fsBtn.replaceChildren(icon(on ? 'fsx' : 'fs'));
    fsBtn.setAttribute('aria-label', on ? 'Exit full screen' : 'Full screen');
    if (!on) { try { screen.orientation && screen.orientation.unlock && screen.orientation.unlock(); } catch { /* ignore */ } }
  };
  document.addEventListener('fullscreenchange', onFs);
  document.addEventListener('webkitfullscreenchange', onFs);
  v.addEventListener('webkitendfullscreen', () => { for (const tr of v.querySelectorAll('track')) tr.remove(); });
  async function togglePip() {
    try {
      if (document.pictureInPictureElement) await document.exitPictureInPicture();
      else if (v.requestPictureInPicture) await v.requestPictureInPicture();
      else if (v.webkitSetPresentationMode) v.webkitSetPresentationMode(v.webkitPresentationMode === 'picture-in-picture' ? 'inline' : 'picture-in-picture');
    } catch { toast('Picture in picture isn’t available right now', 'err'); }
  }

  // ----- subtitles -----
  // Fetched and drawn by us against pos(): no <track> elements, because a text track that is
  // still loading (the PC extracting an embedded track from a torrent) holds playback back, and
  // native cues would be out of sync after a remux seek (the stream restarts at 0).
  function parseVtt(text) {
    const out = [];
    const ts = s => {
      const m = String(s).trim().match(/^(?:(\d+):)?(\d{1,2}):(\d{2})[.,](\d{1,3})/);
      return m ? (+(m[1] || 0)) * 3600 + (+m[2]) * 60 + (+m[3]) + (+m[4].padEnd(3, '0')) / 1000 : NaN;
    };
    for (const b of String(text).replace(/\r/g, '').split(/\n{2,}/)) {
      const lines = b.split('\n');
      const i = lines.findIndex(l => l.includes('-->'));
      if (i < 0) continue;
      const [a, z] = lines[i].split('-->');
      const s0 = ts(a), e0 = ts(z);
      if (!(e0 > s0)) continue;
      const t = lines.slice(i + 1).join('\n').replace(/<[^>]*>/g, '')
        .replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&').trim();
      if (t) out.push({ s: s0, e: e0, t });
    }
    return out.sort((x, y) => x.s - y.s);
  }
  async function pickSub(i, quiet) {
    const seq = ++subSeq;
    subIdx = i; cues = null; lastCue = ''; subsEl.replaceChildren();
    const t = subList[i];
    if (!t) return;
    if (t.cues) { cues = t.cues; return; }
    if (!quiet) toast('Loading subtitles…', 'info', 1800);
    try {
      const res = await api(t.url, { raw: true });
      const parsed = parseVtt(await res.text());
      if (!ctx.alive) return;
      t.cues = parsed;
      if (seq === subSeq) { cues = parsed; if (!parsed.length && !quiet) toast('These subtitles are empty', 'err'); }
    } catch (e) { if (ctx.alive && seq === subSeq && !quiet) toast('Couldn’t load subtitles: ' + e.message, 'err'); }
  }
  let lastCue = '', subRaf = 0, subLast = 0;
  function subTick(ts) {
    subRaf = requestAnimationFrame(subTick);
    if (ts - subLast < 120) return;
    subLast = ts;
    subUpdate();
  }
  function subUpdate() {
    let text = '';
    if (cues && cues.length) {
      const t = pos();
      let lo = 0, hi = cues.length - 1, idx = -1;
      while (lo <= hi) { const mid = (lo + hi) >> 1; if (cues[mid].s <= t) { idx = mid; lo = mid + 1; } else hi = mid - 1; }
      const lines = [];
      for (let k = idx; k >= 0 && k > idx - 6; k--) if (cues[k].e > t) lines.unshift(cues[k].t);
      text = lines.join('\n');
    }
    if (text !== lastCue) { lastCue = text; subsEl.replaceChildren(...(text ? [h('span', null, text)] : [])); }
  }
  subRaf = requestAnimationFrame(subTick);
  function optionsSheet() {
    showUi(true);
    openSheet((p, s) => {
      const opt = (label, on, f) => h('button', { class: 'opt', type: 'button', role: 'menuitemradio', 'aria-checked': String(on), onclick: () => { f(); s.close(); } }, h('span', null, label), icon('check'));
      const subs = [opt('Off', subIdx < 0, () => { pickSub(-1); LS.del('mv.sub'); })];
      const seen = {};
      subList.forEach((t, i) => {
        const base = t.label || t.lang || 'Track ' + (i + 1);
        seen[base] = (seen[base] || 0) + 1;
        subs.push(opt(seen[base] > 1 ? base + ' · ' + seen[base] : base, subIdx === i, () => { pickSub(i); LS.set('mv.sub', t.lang || t.label); }));
      });
      p.append(h('div', { class: 'grip', 'aria-hidden': 'true' }),
        h('h2', { class: 'sheet-h' }, 'Subtitles'),
        subList.length ? h('div', { role: 'menu' }, subs) : h('p', { class: 'muted small' }, 'No subtitles were found for this video.'),
        h('h3', { class: 'sheet-h2' }, 'Speed'),
        segmented([['0.75', '0.75×'], ['1', '1×'], ['1.25', '1.25×'], ['1.5', '1.5×'], ['2', '2×']], String(rate), x => { rate = +x; v.playbackRate = rate; }, 'Playback speed'));
      s.onClose.push(() => showUi());
    }, { label: 'Subtitles and speed' });
  }

  // ----- next episode -----
  function nextParams() {
    const n = sess && sess.next;
    if (!n) return null;
    return { ...P, season: n.season ?? season, episode: n.episode, ept: n.title || '' };
  }
  function goNext() {
    const np = nextParams(); if (!np) return;
    report(true);
    const q2 = new URLSearchParams();
    for (const k in np) if (np[k] != null && np[k] !== '') q2.set(k, String(np[k]));
    location.replace('#/watch?' + q2);
  }
  let upT = null, upShown = false;
  function showUpNext(auto) {
    const n = sess && sess.next;
    if (!n || upShown) return;
    upShown = true;
    const label = [kind === 'series' ? 'S' + (n.season ?? season) + ' · E' + n.episode : 'Episode ' + n.episode, n.title].filter(Boolean).join(' · ');
    const playNow = h('button', { class: 'btn primary', type: 'button', onclick: () => goNext() }, icon('play'), 'Play');
    upNext.replaceChildren(h('small', null, 'Up next'), h('b', null, label),
      h('div', { class: 'row' }, playNow, h('button', { class: 'btn', type: 'button', onclick: () => hideUpNext(true) }, 'Dismiss')));
    upNext.hidden = false;
    if (auto) {
      let left = 10;
      const tick = () => { playNow.replaceChildren(icon('play'), 'Play in ' + left); if (left-- <= 0) goNext(); else upT = setTimeout(tick, 1000); };
      tick();
    }
  }
  function hideUpNext(keep) { clearTimeout(upT); upNext.hidden = true; if (!keep) upShown = false; }

  // ----- video events -----
  let errCount = 0;
  const layoutSubs = () => {
    const vw = v.videoWidth, vh = v.videoHeight;
    if (!vw || !vh) return;
    const H = root.clientHeight, Wd = root.clientWidth;
    root.style.setProperty('--vb', Math.max(0, (H - Math.min(H, Wd * vh / vw)) / 2) + 'px');
  };
  addEventListener('resize', layoutSubs);
  ctx.onLeave(() => removeEventListener('resize', layoutSubs));
  v.addEventListener('resize', layoutSubs);
  v.addEventListener('loadedmetadata', () => {
    layoutSubs();
    if (direct() && startAt > 0) { try { v.currentTime = startAt; } catch { /* ignore */ } startAt = 0; }
    scrub.disabled = !dur();
    durT.textContent = dur() ? fmtTime(dur()) : '–:––';
  });
  v.addEventListener('waiting', () => setLoading(true, started ? 'Buffering…' : 'Starting…'));
  v.addEventListener('canplay', () => { if (v.paused && started) setLoading(false); });
  v.addEventListener('playing', () => {
    started = true; errCount = 0;
    root.classList.add('started');
    setLoading(false);
    msg.hidden = true;
    wl.on();
    showUi();
  });
  v.addEventListener('play', () => { playBig.replaceChildren(icon('pause')); playBig.setAttribute('aria-label', 'Pause'); msPlayback('playing'); showUi(); });
  v.addEventListener('pause', () => {
    playBig.replaceChildren(icon('play')); playBig.setAttribute('aria-label', 'Play');
    msPlayback('paused'); wl.off(); report(true);
    if (!root.classList.contains('loading')) showUi(true);
  });
  v.addEventListener('timeupdate', () => {
    updateTime(); report(false); subUpdate();
    const d = dur();
    if (d && sess && sess.next && pos() > d - 25 && !v.paused) showUpNext(false);
  });
  v.addEventListener('ended', () => {
    const d = dur();
    report(true);
    if (!direct() && d && pos() < d - 60) {
      // The stream ended early (the PC's encoder stopped): pick up where it left off.
      if (errCount++ < 2) { seekTo(pos()); return; }
      fail('Playback stopped', 'The video stream from your PC ended early.', true, () => { errCount = 0; seekTo(pos()); });
      return;
    }
    showUi(true);
    if (sess && sess.next) { upShown = false; showUpNext(true); }
  });
  v.addEventListener('error', () => {
    if (!ctx.alive || !v.getAttribute('src')) return;
    const code = v.error && v.error.code;
    const at = pos();
    if (code === 2 && errCount++ < 2) { setTimeout(() => { if (ctx.alive) seekTo(at); }, 800); return; }
    if (code === 4) fail('This video can’t play on this phone', 'Its format isn’t supported by this browser.', true, () => { errCount = 0; play(at); });
    else fail('Playback failed', code === 2 ? 'Lost the connection to your PC.' : 'Something went wrong while playing.', true, () => { errCount = 0; play(at); });
  });

  // ----- media session -----
  function mediaSession() {
    if (!('mediaSession' in navigator)) return;
    const ms = navigator.mediaSession;
    try {
      const artUrl = imgUrl(P.bg, W.card);
      ms.metadata = new MediaMetadata({ title: [P.title, epLabel].filter(Boolean).join(' · ') || 'MangaVox', artist: P.ept || 'MangaVox', album: 'MangaVox',
        artwork: [{ src: new URL(artUrl || '/icon-512.png', location.href).href, sizes: artUrl ? '640x360' : '512x512' }] });
    } catch { /* unsupported */ }
    const set = (a, f) => { try { ms.setActionHandler(a, f); } catch { /* unsupported */ } };
    set('play', () => v.play());
    set('pause', () => v.pause());
    set('seekbackward', () => skip(-10));
    set('seekforward', () => skip(10));
    set('seekto', d => { if (d && isFinite(d.seekTime)) seekTo(d.seekTime); });
    set('previoustrack', null);
    set('nexttrack', sess && sess.next ? () => goNext() : null);
  }
  const msPlayback = st => { try { if ('mediaSession' in navigator) navigator.mediaSession.playbackState = st; } catch { /* ignore */ } };

  // ----- keyboard -----
  const onKey = e => {
    if (sheets.length || e.target.closest('input:not([type=range]), textarea') || e.ctrlKey || e.metaKey || e.altKey) return;
    const k = e.key;
    if (k === ' ' || k === 'k' || k === 'K') { e.preventDefault(); toggle(); showUi(); }
    else if (k === 'ArrowLeft' || k === 'j' || k === 'J') { e.preventDefault(); skip(-10); showUi(); }
    else if (k === 'ArrowRight' || k === 'l' || k === 'L') { e.preventDefault(); skip(10); showUi(); }
    else if (k === 'f' || k === 'F') { e.preventDefault(); toggleFs(); }
    else if (k === 'm' || k === 'M') { v.muted = !v.muted; toast(v.muted ? 'Muted' : 'Sound on', 'info', 1200); }
    else if (k === 'c' || k === 'C') optionsSheet();
    else if (k === 'n' || k === 'N') goNext();
    else if (k === 'Escape' && !fsEl()) leave();
  };
  document.addEventListener('keydown', onKey);

  // ----- lifecycle -----
  const wl = wakeLock();
  const onHide = () => { if (document.visibilityState !== 'visible') report(true); };
  document.addEventListener('visibilitychange', onHide);
  const onPageHide = () => stopSession();
  addEventListener('pagehide', onPageHide);
  ctx.onLeave(() => {
    stopSession();
    clearTimeout(hideT); clearTimeout(statT); clearTimeout(skipT); clearTimeout(tapT); clearTimeout(upT); clearTimeout(burstT);
    cancelAnimationFrame(subRaf);
    document.removeEventListener('keydown', onKey);
    document.removeEventListener('visibilitychange', onHide);
    document.removeEventListener('fullscreenchange', onFs);
    document.removeEventListener('webkitfullscreenchange', onFs);
    removeEventListener('pagehide', onPageHide);
    wl.dispose();
    try { if (document.pictureInPictureElement === v) document.exitPictureInPicture().catch(() => {}); } catch { /* ignore */ }
    try { if (fsEl()) (document.exitFullscreen || document.webkitExitFullscreen).call(document); } catch { /* ignore */ }
    v.pause(); v.removeAttribute('src'); v.load();
    if (Player) Player.bindSession();
  });
  showUi(true);
  begin();
}

/* ======================================================================
   Remote screen
   ====================================================================== */
function viewRemote(ctx) {
  setTitle('Remote');
  setBack('#/more');
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
    const W0 = img.naturalWidth, H0 = img.naturalHeight;
    if (!W0 || !H0) return;
    const sw = stage.clientWidth, sh = stage.clientHeight;
    const k = Math.min(sw / W0, sh / H0);
    img.style.width = Math.floor(W0 * k) + 'px';
    img.style.height = Math.floor(H0 * k) + 'px';
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
      const w = clamp(Math.round(stage.clientWidth * (devicePixelRatio || 1)), 320, 1200);
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
    return { x: clamp(x, 0, 1), y: clamp(y, 0, 1), inside: x >= 0 && x <= 1 && y >= 0 && y <= 1 };
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
   More: Listen, Remote, PC shortcuts, this phone
   ====================================================================== */
function viewMore(ctx) {
  setTitle('More');
  const cell = (href, ic, t, s) => h('a', { class: 'cell', href },
    h('span', { class: 'cell-ic' }, icon(ic)), h('span', { class: 'cell-b' }, h('b', null, t), s && h('span', null, s)), icon('chev', 'cell-chev'));
  const group = (title, ...kids) => h('section', { class: 'group' }, h('h2', { class: 'group-h' }, title), kids);
  const kv = (k, v) => { const val = h('span', null, v || '—'); return [h('div', { class: 'kv' }, h('span', null, k), val), val]; };
  const [devRow, devV] = kv('Device', me && me.device && me.device.name);
  const [pcRow, pcV] = kv('Connected to', me && me.pc);
  const [appRow, appV] = kv('App', me ? [me.app || 'MangaVox', me.version].filter(Boolean).join(' ') : '');
  const hdSub = h('span', null, me ? subLine() : 'Your PC companion');
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
  ctx.onLeave(() => clearTimeout(armT));
  const PAGES = [['library', 'Library'], ['movies', 'Movies'], ['anime', 'Anime'], ['studio', 'Studio'], ['settings', 'Settings']];
  view.append(h('div', { class: 'page' },
    h('div', { class: 'more-hd' }, h('img', { src: '/icon.png', alt: '', width: '56', height: '56' }), h('div', null, h('b', null, 'MangaVox'), hdSub)),
    group('Media',
      h('div', { class: 'cells' },
        cell('#/listen', 'headphones', 'Listen', 'Narration audio and recap videos'),
        cell('#/remote', 'remote', 'Remote control', 'See and control MangaVox on your PC'))),
    group('Open on your PC',
      h('div', { class: 'pc-chips' }, PAGES.map(([p, l]) => h('button', { class: 'chip', type: 'button', onclick: () => openPc({ page: p }, 'Opened ' + l + ' on your PC') }, l)))),
    group('This phone',
      h('div', { class: 'cells' }, devRow, pcRow, appRow),
      h('p', { class: 'muted small', style: { margin: '10px 4px 0' } }, 'This phone stays paired until you forget it here or remove it in MangaVox → Mobile on your PC.'),
      forget)));
  getMe().then(m => {
    if (!ctx.alive) return;
    devV.textContent = (m.device && m.device.name) || '—';
    pcV.textContent = m.pc || '—';
    appV.textContent = [m.app || 'MangaVox', m.version].filter(Boolean).join(' ');
    hdSub.textContent = subLine();
  }).catch(() => {});
}

/* ======================================================================
   Boot
   ====================================================================== */
function boot() {
  for (const a of tabs) {
    a.prepend(h('span', { class: 'tb-ic' }, icon(a.dataset.ic)));
    a.addEventListener('click', e => {
      e.preventDefault();
      const t = a.dataset.tab, home = a.getAttribute('href');
      if (document.body.dataset.tab === t) {
        if (location.hash !== home) location.hash = home;
        else scrollTo({ top: 0, behavior: 'smooth' });
        return;
      }
      location.hash = tabLast[t] ? '#' + tabLast[t] : home;
    });
  }
  $back.append(icon('back'));
  $back.addEventListener('click', () => goBack(backTarget || '#/library'));
  const brand = document.getElementById('brand');
  brand.addEventListener('error', () => {
    if (!brand.dataset.f) { brand.dataset.f = '1'; brand.src = '/icon.png'; return; }
    brand.replaceWith(h('div', { class: 'brand-txt', 'aria-hidden': 'true' }, 'MV'));
  });
  if ('scrollRestoration' in history) history.scrollRestoration = 'manual';
  const off = document.getElementById('offline');
  const net = () => { off.hidden = navigator.onLine; };
  addEventListener('online', () => { net(); toast('Back online', 'ok'); });
  addEventListener('offline', net);
  net();
  Player = createPlayer();
  getMe().catch(() => {});
  if (!location.hash || location.hash === '#' || location.hash === '#/') {
    const last = String(LS.get('mv.last', '/library'));
    history.replaceState(null, '', '#' + (/^\/[a-z]/.test(last) ? last : '/library'));
  }
  addEventListener('hashchange', route);
  addEventListener('pageshow', e => { if (e.persisted && document.body.dataset.route === 'watch') route(); });
  setupPtr();
  route();
}
boot();
