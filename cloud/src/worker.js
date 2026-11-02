// MangaVox cloud relay (native/crates/ui-phone/CLOUD.md, "Relay").
//
// Serves the phone connector page under /mangavox/ and passes sealed frames between a PC and its
// phones, one Durable Object per room. It never sees keys or plaintext: phone and PC encrypt end to
// end, so this code only routes bytes and enforces limits.
import { DurableObject } from 'cloudflare:workers';

const BASE = '/mangavox';
const ROOM_RE = /^[A-Za-z0-9_-]{22}$/;
const TOKEN_RE = /^Bearer ([A-Za-z0-9_-]{43})$/;
const MAX_PHONES = 8;
/** Phones one address may hold open in a room, so one client cannot fill it. */
const MAX_PHONES_PER_IP = 3;
/** A room whose PC has not connected for this long is deleted. */
const ROOM_TTL_MS = 180 * 24 * 3600 * 1000;
const MAX_PHONE_FRAME = 512 * 1024;
const MAX_PC_FRAME = 1024 * 1024 + 4;
const RATE_WINDOW_MS = 10_000;
const RATE_MAX = 400;

/**
 * The relay's own origin. `wrangler dev` rewrites request URLs to the production host, so local
 * runs name theirs with --var SELF_ORIGIN:http://localhost:8787 (package.json "dev").
 */
function selfUrl(req, env) {
  return env.SELF_ORIGIN ? new URL(env.SELF_ORIGIN) : new URL(req.url);
}

function securityHeaders(res, url) {
  const h = new Headers(res.headers);
  h.set('Content-Security-Policy', [
    "default-src 'none'",
    "script-src 'self'",
    "style-src 'self'",
    "img-src 'self' data:",
    `connect-src 'self' wss://${url.host}${url.protocol === 'http:' ? ` ws://${url.host}` : ''}`,
    "frame-src 'self'",
    "worker-src 'self'",
    "manifest-src 'self'",
    "base-uri 'none'",
    "form-action 'none'",
    "frame-ancestors 'none'",
  ].join('; '));
  h.set('X-Content-Type-Options', 'nosniff');
  h.set('X-Frame-Options', 'DENY');
  h.set('Referrer-Policy', 'no-referrer');
  h.set('Permissions-Policy', 'camera=(), microphone=(), geolocation=(), payment=(), usb=()');
  h.set('Cross-Origin-Opener-Policy', 'same-origin');
  if (url.protocol === 'https:') h.set('Strict-Transport-Security', 'max-age=31536000; includeSubDomains');
  // The connector must update the moment it changes: it is the trust anchor on the phone.
  h.set('Cache-Control', 'no-cache');
  return new Response(res.body, { status: res.status, statusText: res.statusText, headers: h });
}

export default {
  async fetch(req, env) {
    const path = new URL(req.url).pathname;
    const url = selfUrl(req, env);
    if (path === BASE) return Response.redirect(url.origin + BASE + '/', 301);
    if (path === BASE + '/ws/pc' || path === BASE + '/ws/phone') {
      if (req.headers.get('Upgrade') !== 'websocket') return new Response('Expected a WebSocket', { status: 426 });
      const room = new URL(req.url).searchParams.get('room') || '';
      if (!ROOM_RE.test(room)) return new Response('Bad room', { status: 400 });
      // Phones connect only from this page (a browser always sends Origin on a WebSocket).
      // `wrangler dev` rewrites Origin to the production host as well, so a local run also
      // accepts that one.
      const origins = env.SELF_ORIGIN ? [url.origin, new URL(req.url).origin] : [url.origin];
      if (path.endsWith('/phone') && !origins.includes(req.headers.get('Origin'))) return new Response('Forbidden', { status: 403 });
      return env.ROOMS.get(env.ROOMS.idFromName(room)).fetch(req);
    }
    if (!path.startsWith(BASE + '/')) return new Response('Not found', { status: 404 });
    if (path.startsWith(BASE + '/app/')) {
      // The app frame is answered by the service worker through the tunnel; reaching the network
      // means the worker is not running yet.
      const page = '<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width">' +
        '<meta name="color-scheme" content="dark"><p>Reload the page to connect.</p>';
      return securityHeaders(new Response(page, { status: 503, headers: { 'Content-Type': 'text/html; charset=utf-8' } }), url);
    }
    const res = await env.ASSETS.fetch(req);
    return securityHeaders(res, url);
  },
};

async function sha256hex(s) {
  const d = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(s));
  return [...new Uint8Array(d)].map(b => b.toString(16).padStart(2, '0')).join('');
}

function sameHex(a, b) {
  if (typeof a !== 'string' || a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

function safeClose(ws, code, reason) {
  try { ws.close(code, reason); } catch { /* already closed */ }
}

/** One PC's room: its socket and its phones'. Hibernates while idle. */
export class Room extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair('ping', 'pong'));
    this.rate = new Map();
  }

  pc() {
    return this.ctx.getWebSockets('pc')[0] || null;
  }

  accept(tags, attachment) {
    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server, tags);
    server.serializeAttachment(attachment);
    return { client, server };
  }

  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname.endsWith('/pc')) {
      const m = TOKEN_RE.exec(req.headers.get('Authorization') || '');
      if (!m) return new Response('Unauthorized', { status: 401 });
      const hash = await sha256hex(m[1]);
      const stored = await this.ctx.storage.get('token');
      if (!stored) await this.ctx.storage.put('token', hash);
      else if (!sameHex(stored, hash)) return new Response('Forbidden', { status: 403 });
      // A newer connection of the same PC replaces the old one; phones re-handshake with it.
      for (const old of this.ctx.getWebSockets('pc')) safeClose(old, 4000, 'replaced');
      for (const p of this.ctx.getWebSockets('phone')) safeClose(p, 4004, 'pc reconnected');
      await this.ctx.storage.put('seen', Date.now());
      await this.ctx.storage.setAlarm(Date.now() + ROOM_TTL_MS);
      const { client } = this.accept(['pc'], { role: 'pc' });
      return new Response(null, { status: 101, webSocket: client });
    }

    const pc = this.pc();
    const phones = this.ctx.getWebSockets('phone');
    const ip = req.headers.get('CF-Connecting-IP') || '';
    const fromIp = phones.filter(p => (p.deserializeAttachment() || {}).ip === ip).length;
    let c;
    do { c = crypto.getRandomValues(new Uint32Array(1))[0]; } while (this.ctx.getWebSockets('c' + c).length);
    const { client, server } = this.accept(['phone', 'c' + c], { role: 'phone', c, ip });
    if (!pc) safeClose(server, 4004, 'pc offline');
    else if (phones.length >= MAX_PHONES || fromIp >= MAX_PHONES_PER_IP) safeClose(server, 4008, 'too many phones');
    else pc.send(JSON.stringify({ t: 'open', c }));
    return new Response(null, { status: 101, webSocket: client });
  }

  allow(c) {
    const now = Date.now();
    let r = this.rate.get(c);
    if (!r || now - r.start > RATE_WINDOW_MS) { r = { start: now, n: 0 }; this.rate.set(c, r); }
    return ++r.n <= RATE_MAX;
  }

  webSocketMessage(ws, msg) {
    const a = ws.deserializeAttachment() || {};
    if (a.role === 'phone') {
      if (typeof msg === 'string') return;
      if (msg.byteLength > MAX_PHONE_FRAME) return safeClose(ws, 1009, 'too big');
      if (!this.allow(a.c)) return safeClose(ws, 4029, 'slow down');
      const pc = this.pc();
      if (!pc) return safeClose(ws, 4004, 'pc offline');
      const out = new Uint8Array(4 + msg.byteLength);
      new DataView(out.buffer).setUint32(0, a.c);
      out.set(new Uint8Array(msg), 4);
      pc.send(out);
      return;
    }
    if (a.role !== 'pc') return;
    if (typeof msg === 'string') {
      let j;
      try { j = JSON.parse(msg); } catch { return; }
      if (j && j.t === 'close') for (const p of this.ctx.getWebSockets('c' + (j.c >>> 0))) safeClose(p, 4000, 'closed by pc');
      return;
    }
    if (msg.byteLength < 5 || msg.byteLength > MAX_PC_FRAME) return;
    const c = new DataView(msg).getUint32(0);
    const p = this.ctx.getWebSockets('c' + c)[0];
    if (p) p.send(msg.slice(4));
  }

  /** Deletes a room its PC has abandoned (any id can create one, so they must not pile up). */
  async alarm() {
    const seen = (await this.ctx.storage.get('seen')) || 0;
    if (this.pc() || Date.now() - seen < ROOM_TTL_MS) {
      await this.ctx.storage.setAlarm(Math.max(Date.now(), seen) + ROOM_TTL_MS);
      return;
    }
    await this.ctx.storage.deleteAll();
  }

  webSocketClose(ws) {
    this.gone(ws);
  }

  webSocketError(ws) {
    this.gone(ws);
  }

  gone(ws) {
    const a = ws.deserializeAttachment() || {};
    if (a.role === 'phone') {
      this.rate.delete(a.c);
      const pc = this.pc();
      if (pc) try { pc.send(JSON.stringify({ t: 'gone', c: a.c })); } catch { /* closing */ }
    } else if (a.role === 'pc' && !this.ctx.getWebSockets('pc').some(s => s !== ws)) {
      for (const p of this.ctx.getWebSockets('phone')) safeClose(p, 4004, 'pc offline');
    }
  }
}
