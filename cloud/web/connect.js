// MangaVox connector (native/crates/ui-phone/CLOUD.md, "On the phone").
//
// Opened from the QR code (`#p=<room>.<code>.<fingerprint>&l=<Wi-Fi link>`) it offers Same Wi-Fi or
// From anywhere; opened later it reconnects to the paired PC. It holds the relay WebSocket and the
// encrypted session, and answers the service worker's requests for the app frame through it.
import { KIND, hello, finish, Session, newDeviceKey, matchesFingerprint, sameBytes, b64, unb64 } from './tunnel.js';

const $ = id => document.getElementById(id);
const BASE = new URL('./', location.href).pathname;
const WS_URL = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + BASE + 'ws/phone?room=';
const PING_MS = 25_000;

/* ---------------- storage: IndexedDB, because a CryptoKey can be stored there but never read out */

function openDb() {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open('mangavox-cloud', 1);
    r.onupgradeneeded = () => r.result.createObjectStore('kv');
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

async function kv(mode, fn) {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const t = db.transaction('kv', mode);
    const r = fn(t.objectStore('kv'));
    t.oncomplete = () => resolve(r && r.result);
    t.onerror = () => reject(t.error);
  });
}

const loadPc = () => kv('readonly', s => s.get('pc')).catch(() => null);
const savePc = pc => kv('readwrite', s => s.put(pc, 'pc'));
const wipePc = () => kv('readwrite', s => s.delete('pc')).catch(() => {});
/** The service worker's copy of what the PC let the phone keep (sw.js): it belongs to one PC. */
const clearCache = () => caches.delete('mv-tunnel-1').catch(() => {});

/* ---------------- screens */

function show({ title = 'MangaVox', lead = '', busy = false, choose = false, retry = null, forget = false, paste = false }) {
  document.body.classList.remove('in-app', 'edge');
  $('app').hidden = true;
  $('title').textContent = title;
  $('lead').textContent = lead;
  $('busy').hidden = !busy;
  $('choose').hidden = !choose;
  $('paste').hidden = !paste;
  $('retry').hidden = !retry;
  $('retry').onclick = retry;
  $('forget').hidden = !forget;
  $('actions').hidden = !retry && !forget;
}

let appOpen = false;
/** The app frame's secret path segment, new for every page load (see sw.js). */
const appToken = b64(crypto.getRandomValues(new Uint8Array(16)));

async function openApp() {
  await navigator.serviceWorker.ready;
  const frame = $('app');
  if (!appOpen) {
    appOpen = true;
    frame.src = BASE + 'app/' + appToken + '/';
  }
  frame.hidden = false;
  document.body.classList.add('in-app');
}

function closeApp() {
  appOpen = false;
  const frame = $('app');
  frame.hidden = true;
  frame.src = 'about:blank';
}

/* ---------------- the QR link */

function privateIpv4(host) {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host);
  if (!m) return false;
  const [a, b] = [+m[1], +m[2]];
  return a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 169 && b === 254);
}

/** Reads the fragment and removes it from the address bar and history at once. */
function readLink() {
  const raw = location.hash.slice(1);
  if (!raw) return null;
  history.replaceState(null, '', location.pathname);
  return parseLink(raw);
}

/**
 * A QR link's fragment, or a home-screen setup link's (CLOUD.md, "Home-screen icon"), which adds
 * `h`: the paired phone that made it.
 */
function parseLink(raw) {
  const q = new URLSearchParams(raw);
  const p = (q.get('p') || '').split('.');
  if (p.length !== 3 || !/^[\w-]{22}$/.test(p[0]) || !/^[\w-]{43}$/.test(p[1]) || !/^[\w-]{22}$/.test(p[2])) return { bad: true };
  let lan = '';
  try {
    const u = new URL(q.get('l') || '');
    if (u.protocol === 'http:' && privateIpv4(u.hostname) && u.pathname === '/pair' && !u.username && !u.password) lan = u.href;
  } catch { /* no Wi-Fi link */ }
  const inv = /^[0-9a-f]{16}$/.test(q.get('h') || '') ? q.get('h') : '';
  return { rid: p[0], code: p[1], fp: p[2], lan, inv };
}

/** Opened as an installed home-screen app (which on iPhone has its own storage). */
const standalone = () => matchMedia('(display-mode: standalone)').matches || navigator.standalone === true;

/* ---------------- the connection */

let conn = null;
let retryMs = 1000;
let retryTimer = null;
let saved = null;

const REFUSED = {
  pair: ['This code no longer works', 'It expired or was already used. Show a new code in MangaVox → Mobile on your PC and scan it again.'],
  unknown: ['This phone was removed', 'It was removed on your PC. Scan the code in MangaVox → Mobile again to reconnect.'],
  denied: ['Not allowed', 'The request was declined on your PC. Scan the code again to ask once more.'],
  busy: ['Your PC is busy', 'Too many phones are connected right now. Try again in a moment.'],
  bad: ['Update needed', 'This page and MangaVox on your PC don’t match. Reload this page, and update MangaVox on your PC.'],
};

/**
 * Opens a session. target: { mode: 'pair', rid, code, fp } or { mode: 'resume', pc }.
 */
function connect(target) {
  clearTimeout(retryTimer);
  if (conn) conn.retire();
  const rid = target.mode === 'pair' ? target.rid : target.pc.rid;
  const ws = new WebSocket(WS_URL + rid);
  ws.binaryType = 'arraybuffer';
  const c = {
    ws, target, session: null, ok: false, ready: false, final: false, chain: Promise.resolve(), ping: null, device: null, state: null,
    retire() { this.final = true; clearInterval(this.ping); try { ws.close(); } catch { /* closed */ } if (this.session) this.session.fail(new Error('closed')); },
  };
  conn = c;
  if (!appOpen) show({ title: target.mode === 'pair' ? 'Pairing…' : 'Connecting…', lead: 'Securing the connection to your PC.', busy: true });

  ws.onopen = () => {
    c.chain = c.chain.then(async () => {
      if (target.mode === 'pair') {
        c.device = await newDeviceKey();
        const h = await hello({ kind: KIND.PAIR, rid, device: c.device, inv: target.inv });
        c.state = h.state;
        ws.send(h.frame);
      } else {
        c.device = { privateKey: target.pc.dkey, publicRaw: unb64(target.pc.dpub) };
        const h = await hello({ kind: KIND.RESUME, rid, device: c.device, id: target.pc.id });
        c.state = h.state;
        ws.send(h.frame);
      }
      c.ping = setInterval(() => { if (ws.readyState === 1) ws.send('ping'); }, PING_MS);
    }).catch(e => fail(c, e));
  };
  ws.onmessage = ev => {
    if (typeof ev.data === 'string') return;
    const f = new Uint8Array(ev.data);
    // Once the session exists, frames go straight to it: it keeps their order itself and opens
    // several at once (tunnel.js). The PC seals nothing before the fin, so none can still wait
    // in the chain then.
    if (c.session && f[0] !== KIND.REFUSE) {
      if (!c.final) c.session.receive(f);
      return;
    }
    c.chain = c.chain.then(() => onFrame(c, f)).catch(e => fail(c, e));
  };
  ws.onclose = ev => onClose(c, ev);
}

async function onFrame(c, f) {
  if (c.final) return;
  if (f[0] === KIND.REFUSE) {
    let v = {};
    try { v = JSON.parse(new TextDecoder().decode(f.subarray(1))); } catch { /* unreadable */ }
    refused(c, v.code);
    return;
  }
  if (c.session) { await c.session.receive(f); return; }
  if (f[0] !== KIND.REPLY) throw new Error('unexpected frame');
  const t = c.target;
  const keys = await finish(c.state, f, t.mode === 'pair'
    ? { salt: unb64(t.code), trust: s => matchesFingerprint(t.fp, s) }
    : { salt: new Uint8Array(32), trust: async s => sameBytes(s, unb64(t.pc.spub)) });
  c.keys = keys;
  c.session = new Session(keys, frame => { if (c.ws.readyState === 1) c.ws.send(frame); });
  c.session.onJson = v => onControl(c, v);
  c.session.onFail = () => { try { c.ws.close(); } catch { /* closed */ } };
  await c.session.json({ t: 'fin', ua: navigator.userAgent });
  // Resuming, the PC answers "ok" before anything sent after the fin (CLOUD.md, "Handshake"), and
  // only it can open what this session seals: the app starts loading now, a round trip earlier.
  if (t.mode === 'resume' && !c.final) {
    c.ready = true;
    openApp();
  }
}

async function onControl(c, v) {
  if (v.t === 'wait') {
    show({ title: 'Confirm on your PC', lead: `Check that MangaVox on your PC shows the code ${c.keys.code}, then click Allow there.`, busy: true });
  } else if (v.t === 'ok' && !c.ok) {
    c.ok = true;
    c.ready = true;
    retryMs = 1000;
    const pc = c.target.mode === 'pair'
      ? { rid: c.target.rid, id: v.id, spub: b64(c.keys.spub), dkey: c.device.privateKey, dpub: b64(c.device.publicRaw), pc: v.pc || '' }
      : { ...c.target.pc, pc: v.pc || c.target.pc.pc };
    if (c.target.mode === 'pair') closeApp();
    saved = pc;
    // The new pairing first, then the old PC's cache: sw.js drops anything still being stored
    // for a pairing that is no longer the saved one.
    await savePc(pc);
    if (c.target.mode === 'pair') await clearCache();
    // Later reconnects resume with the stored key.
    c.target = { mode: 'resume', pc };
    await openApp();
  }
}

function refused(c, code) {
  c.final = true;
  try { c.ws.close(); } catch { /* closed */ }
  const [title, lead] = REFUSED[code] || REFUSED.bad;
  closeApp();
  if (code === 'pair' && c.target.inv) {
    show({ title: 'This setup link no longer works', lead: SETUP_LEAD, paste: true });
    return;
  }
  // Never wipe the key on this: a REFUSE is not authenticated (the relay could forge it).
  show({ title, lead, forget: code === 'unknown' && !!saved, retry: code === 'busy' ? () => connect(c.target) : null });
}

function fail(c, e) {
  console.warn('[mangavox]', e);
  if (c.final) return;
  if (String(e && e.message).includes('not the PC')) {
    c.final = true;
    try { c.ws.close(); } catch { /* closed */ }
    closeApp();
    show({ title: 'Wrong PC', lead: 'The PC that answered is not the one in the code. Nothing was sent. Scan the code again.' });
    return;
  }
  try { c.ws.close(); } catch { /* closed */ }
}

function onClose(c, ev) {
  clearInterval(c.ping);
  if (c.session) c.session.fail(new Error('closed'));
  if (conn !== c || c.final) return;
  conn = null;
  if (c.target.mode === 'pair' && !c.ok) {
    const offline = ev.code === 4004;
    show({
      title: offline ? 'Your PC is offline' : 'Couldn’t pair',
      lead: offline ? 'Open MangaVox on your PC, turn on From anywhere in Mobile, and try again.' : 'The connection dropped while pairing. Try again.',
      retry: () => connect(c.target),
    });
    return;
  }
  if (!appOpen) {
    show({
      title: ev.code === 4004 ? 'Your PC is offline' : 'Reconnecting…',
      lead: ev.code === 4004 ? 'Open MangaVox on your PC with From anywhere turned on. Trying again…' : 'Trying to reach your PC again.',
      busy: true, retry: () => connect(c.target), forget: true,
    });
  }
  retryTimer = setTimeout(() => connect(c.target), retryMs);
  retryMs = Math.min(retryMs * 2, 30_000);
}

function reconnectNow() {
  if (conn || !saved) return;
  retryMs = 1000;
  connect({ mode: 'resume', pc: saved });
}

async function forget() {
  if (conn) { conn.retire(); conn = null; }
  clearTimeout(retryTimer);
  saved = null;
  closeApp();
  // The pairing first, then the cache (see sw.js, store).
  await wipePc();
  await clearCache();
  await caches.delete(META).catch(() => {});
  show({ title: 'Connect your phone', lead: 'On your PC, open MangaVox → Mobile, turn on From anywhere, and scan the code with this phone.' });
}

/* ---------------- requests from the service worker (the app frame) */

function onSwRequest(ev) {
  const d = ev.data;
  const port = ev.ports && ev.ports[0];
  if (!d || d.type !== 'mv-req' || !port) return;
  // Only this page's own app frame (the token never leaves this page and its frame).
  if (!appOpen || d.tok !== appToken) { port.postMessage({ error: 'not this page' }); port.close(); return; }
  const s = conn && conn.ready && conn.session && !conn.session.dead ? conn.session : null;
  if (!s) { port.postMessage({ error: 'offline' }); port.close(); return; }
  // The pairing this answer comes through: sw.js keeps it only while that is still the saved one.
  const who = (conn.target.pc && conn.target.pc.id) || '';
  const queue = [];
  let pulls = 0, done = false, error = null, headSent = false, closed = false;
  const close = () => { closed = true; port.close(); };
  const flush = () => {
    while (!closed && pulls > 0 && (queue.length || done || error)) {
      pulls--;
      if (queue.length) {
        const chunk = queue.shift();
        const n = chunk.byteLength;
        port.postMessage({ chunk: chunk.buffer }, [chunk.buffer]);
        h.ack(n);
      } else if (error) { port.postMessage({ error }); close(); }
      else { port.postMessage({ end: true }); close(); }
    }
  };
  const h = s.request({ method: d.m, path: d.p, headers: d.h, body: d.b }, {
    onHead(status, headers) { headSent = true; port.postMessage({ head: { status, headers }, who }); },
    onData(bytes) { queue.push(bytes); flush(); },
    onEnd() { done = true; flush(); },
    onError(e) {
      if (!headSent) { port.postMessage({ error: e.message }); close(); return; }
      error = e.message;
      flush();
    },
  });
  port.onmessage = m => {
    if (m.data && m.data.pull) { pulls++; flush(); }
    else if (m.data && m.data.cancel) { h.cancel(); close(); }
  };
}

/* ---------------- messages from the app frame */

function onAppMessage(ev) {
  if (ev.origin !== location.origin || ev.source !== $('app').contentWindow) return;
  const m = ev.data && ev.data.mv;
  if (m === 'locked') {
    closeApp();
    show({ title: REFUSED.unknown[0], lead: REFUSED.unknown[1], forget: true });
    if (conn) { conn.retire(); conn = null; }
  } else if (m === 'forget') {
    forget();
  } else if (m === 'homescreen') {
    homeScreen(ev.data.link);
  } else if (m === 'edge') {
    // The app pads its own tab bar for the gesture bar, so the frame can reach the screen edge.
    document.body.classList.add('edge');
  }
}

/* ---------------- the home-screen icon (CLOUD.md, "Home-screen icon") */

const META = 'mv-meta';
const META_START = BASE + 'meta/start';

/**
 * The app made a setup link for this phone's home-screen icon (it is also on the clipboard). Put
 * it where "Add to Home Screen" picks the icon's address: this page's address, and the manifest's
 * start_url (sw.js serves it while the link works). The fragment never reaches the relay.
 */
async function homeScreen(link) {
  const i = String(link || '').indexOf('#');
  const v = i < 0 ? null : parseLink(link.slice(i + 1));
  if (!v || v.bad || !v.inv || !saved || v.rid !== saved.rid) return;
  const frag = '#' + link.slice(i + 1);
  history.replaceState(null, '', BASE + frag);
  try {
    const until = Date.now() + 30 * 60 * 1000;
    await (await caches.open(META)).put(META_START, new Response(JSON.stringify({ start: BASE + frag, until }), { headers: { 'content-type': 'application/json' } }));
  } catch { /* the address bar still has it */ }
}

/** Pairs this icon from a setup link the user pastes (when its address did not carry one). */
async function pasteLink(text) {
  const s = String(text || '').trim();
  const i = s.indexOf('#');
  const v = i < 0 ? null : parseLink(s.slice(i + 1));
  if (!v || v.bad) {
    show({ title: 'That isn’t a setup link', lead: SETUP_LEAD, paste: true });
    return;
  }
  if (saved && saved.rid === v.rid) { connect({ mode: 'resume', pc: saved }); return; }
  connect({ mode: 'pair', rid: v.rid, code: v.code, fp: v.fp, inv: v.inv });
}

const SETUP_LEAD = 'On iPhone this icon keeps its own storage. In Safari, open MangaVox (app.bhippi.com/mangavox) and tap More → Add to Home Screen; it copies a setup link. Then come back here and tap Paste setup link.';

/* ---------------- start */

async function main() {
  $('forget').onclick = () => forget();
  if (!('serviceWorker' in navigator) || !window.indexedDB || !(window.crypto && crypto.subtle) || !window.isSecureContext) {
    show({ title: 'This browser can’t connect', lead: 'Open this page in Safari or Chrome (not a private window or an in-app browser).' });
    return;
  }
  navigator.serviceWorker.addEventListener('message', onSwRequest);
  addEventListener('message', onAppMessage);
  try {
    await navigator.serviceWorker.register(BASE + 'sw.js', { scope: BASE });
  } catch {
    show({ title: 'This browser can’t connect', lead: 'It blocked what this page needs. Turn off private browsing and try again.' });
    return;
  }
  addEventListener('online', reconnectNow);
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') reconnectNow(); });

  const link = readLink();
  saved = await loadPc();
  if (link && link.bad) {
    show({ title: 'This code is incomplete', lead: 'Scan the code in MangaVox → Mobile on your PC again.' });
    return;
  }
  $('paste-btn').onclick = async () => {
    try { await pasteLink(await navigator.clipboard.readText()); }
    catch { $('paste-in').hidden = false; $('paste-go').hidden = false; $('paste-in').focus(); }
  };
  $('paste-go').onclick = () => pasteLink($('paste-in').value);
  if (link && link.inv) {
    // A home-screen setup link: already paired with this PC (the icon opened again) resumes;
    // otherwise this icon pairs itself, without a question on the PC.
    if (saved && saved.rid === link.rid) { connect({ mode: 'resume', pc: saved }); return; }
    connect({ mode: 'pair', rid: link.rid, code: link.code, fp: link.fp, inv: link.inv });
    return;
  }
  if (link) {
    const lan = $('opt-lan');
    lan.hidden = !link.lan;
    lan.href = link.lan || '#';
    $('opt-cloud').onclick = () => connect({ mode: 'pair', rid: link.rid, code: link.code, fp: link.fp });
    show({ title: 'Connect to your PC', lead: 'How should this phone reach MangaVox?', choose: true });
    return;
  }
  if (saved) { connect({ mode: 'resume', pc: saved }); return; }
  if (standalone()) { show({ title: 'Set up this icon', lead: SETUP_LEAD, paste: true }); return; }
  show({ title: 'Connect your phone', lead: 'On your PC, open MangaVox → Mobile, turn on From anywhere, and scan the code with this phone.' });
}

main();
