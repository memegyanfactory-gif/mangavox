// MangaVox connector service worker (native/crates/ui-phone/CLOUD.md, "On the phone").
//
// The app runs in a frame at /mangavox/app/<token>/, where the token is a secret the connector
// page makes for each load. Every request of that frame (its page, script, images, video, API
// calls) is handed to the connector page, which owns the encrypted session and checks the token,
// so another site that opens or frames /mangavox/app/... reaches nothing. The answer is streamed
// back with backpressure: the connector sends one chunk per pull and acks what was read to the PC
// (in batches, tunnel.js), so a long
// video never piles up in memory.
//
// What the PC marks cacheable for an hour or more (the versioned app script and styles, covers,
// artwork) is kept in a small cache on the phone and answered from there while fresh, so opening
// the app or the library again does not fetch it through the relay again. Chapter pages, ranges
// and anything the PC marks no-store or no-cache are never kept. connect.js clears the cache when
// the phone forgets its PC or pairs with another one, and an answer is kept only while the pairing
// it came through is still the saved one (store).
const SCOPE = new URL(self.registration.scope).pathname; // "/mangavox/"
const APP = SCOPE + 'app/';
const NULL_BODY = new Set([101, 204, 205, 304]);
const FORWARD = ['range', 'if-range', 'content-type', 'accept', 'x-mangavox'];
const TOKEN_RE = /^[A-Za-z0-9_-]{22}$/;
const CACHE = 'mv-tunnel-1';
const CACHE_ENTRIES = 400;
const CACHE_ITEM_MAX = 1 << 20;
const MIN_AGE = 3600;

/** { tok, rest } for a path under the app frame, or null. */
function appPath(pathname) {
  if (!pathname.startsWith(APP)) return null;
  const tail = pathname.slice(APP.length);
  const i = tail.indexOf('/');
  const tok = i < 0 ? tail : tail.slice(0, i);
  if (!TOKEN_RE.test(tok)) return null;
  return { tok, rest: i < 0 ? '/' : tail.slice(i) };
}

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', e => e.waitUntil(self.clients.claim()));

/* ---------------- the phone-side cache */

const cacheKey = path => location.origin + SCOPE + 'cache' + path;

/** Seconds the PC lets this response be kept, or 0. */
function keepFor(req, path, res) {
  if (req.method !== 'GET' || res.status !== 200 || req.headers.has('range') || path.startsWith('/api/chapter/')) return 0;
  const len = Number(res.headers.get('content-length'));
  if (!(len > 0 && len <= CACHE_ITEM_MAX)) return 0;
  const cc = (res.headers.get('cache-control') || '').toLowerCase();
  if (/no-store|no-cache/.test(cc)) return 0;
  const m = /max-age=(\d+)/.exec(cc);
  const age = m ? Number(m[1]) : 0;
  return age >= MIN_AGE ? age : 0;
}

async function fromCache(req, path) {
  if (req.method !== 'GET' || req.headers.has('range')) return null;
  try {
    const hit = await (await caches.open(CACHE)).match(cacheKey(path));
    if (hit && Number(hit.headers.get('x-mv-expires')) > Date.now()) return hit;
  } catch { /* no storage: go to the PC */ }
  return null;
}

/** The device id of the pairing connect.js keeps (its IndexedDB), or '' when there is none. */
function pairedId() {
  return new Promise(resolve => {
    const r = indexedDB.open('mangavox-cloud', 1);
    r.onupgradeneeded = () => r.result.createObjectStore('kv');
    r.onerror = () => resolve('');
    r.onsuccess = () => {
      try {
        const g = r.result.transaction('kv', 'readonly').objectStore('kv').get('pc');
        g.onsuccess = () => { r.result.close(); resolve((g.result && g.result.id) || ''); };
        g.onerror = () => { r.result.close(); resolve(''); };
      } catch { r.result.close(); resolve(''); }
    };
  });
}

/** Drops expired entries and the oldest beyond CACHE_ENTRIES. */
async function trim(cache, sweep) {
  const keys = await cache.keys();
  if (sweep) {
    const now = Date.now();
    for (const k of keys) {
      const hit = await cache.match(k);
      if (!hit || !(Number(hit.headers.get('x-mv-expires')) > now)) await cache.delete(k);
    }
  }
  const left = sweep ? await cache.keys() : keys;
  // Oldest first: keys() lists entries in the order they were added.
  for (const k of left.slice(0, Math.max(0, left.length - CACHE_ENTRIES))) await cache.delete(k);
}

let swept = false;
/**
 * Keeps a response from the PC paired as `who`. Forgetting the PC (or pairing another one)
 * changes the pairing in IndexedDB before it deletes the cache (connect.js), so checking it again
 * after the put removes anything a store still in flight wrote after that delete.
 */
async function store(path, res, age, who) {
  const headers = new Headers(res.headers);
  headers.set('x-mv-expires', String(Date.now() + age * 1000));
  const body = await res.arrayBuffer();
  if (!who || (await pairedId()) !== who) return;
  const cache = await caches.open(CACHE);
  await cache.put(cacheKey(path), new Response(body, { status: 200, headers }));
  if ((await pairedId()) !== who) {
    await cache.delete(cacheKey(path));
    return;
  }
  // Once per worker start also drop what expired (the worker stops when idle).
  await trim(cache, !swept);
  swept = true;
}

self.addEventListener('fetch', e => {
  const req = e.request;
  const url = new URL(req.url);
  if (url.origin !== location.origin) return;
  if (req.mode === 'navigate') {
    const app = appPath(url.pathname);
    // Only the connector's own frame: a top-level page load (a link or form from another site)
    // never goes through.
    if (app) e.respondWith(req.destination === 'iframe' ? tunnel(req, app.tok, app.rest + url.search) : Response.error());
    else if (url.pathname.startsWith(APP)) e.respondWith(Response.error());
    return;
  }
  if (!e.clientId) return;
  e.respondWith((async () => {
    const client = await self.clients.get(e.clientId);
    const app = client && appPath(new URL(client.url).pathname);
    if (!app) return fetch(req);
    const path = url.pathname + url.search;
    // Only for the app frame's own requests: its document exists only after the connector checked
    // its token (navigations always go to the connector).
    const hit = await fromCache(req, path);
    if (hit) return hit;
    const res = await tunnel(req, app.tok, path);
    const age = keepFor(req, path, res);
    if (!age) return res;
    const [mine, kept] = res.body.tee();
    e.waitUntil(store(path, new Response(kept, { status: res.status, headers: res.headers }), age, sentBy.get(res)).catch(() => {}));
    return new Response(mine, { status: res.status, headers: res.headers });
  })());
});

/** Connector pages (top-level /mangavox/), most recently focused first. */
async function connectors() {
  const wins = await self.clients.matchAll({ type: 'window' });
  return wins.filter(w => {
    const p = new URL(w.url).pathname;
    return w.frameType !== 'nested' && (p === SCOPE || p === SCOPE + 'index.html');
  });
}

async function tunnel(req, tok, path) {
  if (path.startsWith('//')) return Response.error();
  const headers = {};
  for (const k of FORWARD) {
    const v = req.headers.get(k);
    if (v) headers[k] = v;
  }
  const body = req.method === 'GET' || req.method === 'HEAD' ? null : await req.arrayBuffer();
  for (const win of await connectors()) {
    const res = await ask(win, { type: 'mv-req', tok, m: req.method, p: path, h: headers, b: body }, req);
    if (res) return res;
  }
  return Response.error();
}

/** Which pairing (device id) each tunnelled response came through, for store(). */
const sentBy = new WeakMap();

/** Asks one connector; resolves to a Response, or null when it has no session. */
function ask(win, msg, req) {
  return new Promise(resolve => {
    const ch = new MessageChannel();
    const port = ch.port1;
    let head = null;
    let pull = null;
    const timer = setTimeout(() => { port.postMessage({ cancel: true }); port.close(); resolve(null); }, 120_000);
    port.onmessage = ev => {
      const d = ev.data || {};
      if (!head) {
        clearTimeout(timer);
        if (!d.head) { port.close(); resolve(null); return; }
        head = d.head;
        const headers = new Headers(head.headers);
        headers.set('x-content-type-options', 'nosniff');
        const init = { status: head.status, headers };
        if (NULL_BODY.has(head.status) || req.method === 'HEAD') {
          port.postMessage({ cancel: true });
          port.close();
          resolve(new Response(null, init));
          return;
        }
        const stream = new ReadableStream({
          pull(ctrl) {
            return new Promise(done => {
              pull = { ctrl, done };
              port.postMessage({ pull: true });
            });
          },
          cancel() {
            port.postMessage({ cancel: true });
            port.close();
          },
        }, new CountQueuingStrategy({ highWaterMark: 2 }));
        if (req.signal) req.signal.addEventListener('abort', () => { port.postMessage({ cancel: true }); port.close(); });
        const res = new Response(stream, init);
        if (typeof d.who === 'string') sentBy.set(res, d.who);
        resolve(res);
        return;
      }
      const p = pull;
      pull = null;
      if (!p) return;
      try {
        if (d.chunk) p.ctrl.enqueue(new Uint8Array(d.chunk));
        else if (d.end) { p.ctrl.close(); port.close(); }
        else p.ctrl.error(new Error(d.error || 'disconnected'));
      } catch { /* the reader went away */ }
      p.done();
    };
    const transfer = [ch.port2];
    if (msg.b) transfer.push(msg.b);
    win.postMessage(msg, transfer);
  });
}
