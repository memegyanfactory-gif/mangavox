// MangaVox connector service worker (native/crates/ui-phone/CLOUD.md, "On the phone").
//
// The app runs in a frame at /mangavox/app/<token>/, where the token is a secret the connector
// page makes for each load. Every request of that frame (its page, script, images, video, API
// calls) is handed to the connector page, which owns the encrypted session and checks the token,
// so another site that opens or frames /mangavox/app/... reaches nothing. The answer is streamed
// back with backpressure: the connector sends one chunk per pull and acks it to the PC, so a long
// video never piles up in memory. Nothing is cached here.
const SCOPE = new URL(self.registration.scope).pathname; // "/mangavox/"
const APP = SCOPE + 'app/';
const NULL_BODY = new Set([101, 204, 205, 304]);
const FORWARD = ['range', 'if-range', 'content-type', 'accept', 'x-mangavox'];
const TOKEN_RE = /^[A-Za-z0-9_-]{22}$/;

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
    if (app) return tunnel(req, app.tok, url.pathname + url.search);
    return fetch(req);
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
        resolve(new Response(stream, init));
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
