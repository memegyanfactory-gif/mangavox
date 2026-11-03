// The phone side of the cloud protocol (native/crates/ui-phone/CLOUD.md): handshake, sealing and
// request streams over one relay WebSocket. WebCrypto only, so it runs in the browser and in Node
// (test/e2e.mjs). The device key is created non-extractable: script can use it but never read it.
const subtle = globalThis.crypto.subtle;
const te = new TextEncoder();
const td = new TextDecoder();

export const KIND = { PAIR: 1, RESUME: 2, REPLY: 3, REFUSE: 4, SEALED: 5 };
const MSG = { JSON: 0, DATA: 1, END: 2 };
const DIR_PHONE_TO_PC = 1;
const DIR_PC_TO_PHONE = 2;
const ECDH = { name: 'ECDH', namedCurve: 'P-256' };

/** Standard base64 (request bodies). */
function b64std(bytes) {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

/** base64url without padding (keys, nonces). */
export function b64(bytes) {
  return b64std(bytes).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function unb64(str) {
  const s = atob(String(str).replace(/-/g, '+').replace(/_/g, '/'));
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

function concat(parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let i = 0;
  for (const p of parts) { out.set(p, i); i += p.length; }
  return out;
}

const random = n => globalThis.crypto.getRandomValues(new Uint8Array(n));
const sha256 = async bytes => new Uint8Array(await subtle.digest('SHA-256', bytes));
const importPoint = raw => subtle.importKey('raw', raw, ECDH, false, []);
const ecdh = async (priv, pubRaw) => new Uint8Array(await subtle.deriveBits({ name: 'ECDH', public: await importPoint(pubRaw) }, priv, 256));

function sameBytes(a, b) {
  if (a.length !== b.length) return false;
  let d = 0;
  for (let i = 0; i < a.length; i++) d |= a[i] ^ b[i];
  return d === 0;
}

/** A new device key: { privateKey (non-extractable CryptoKey), publicRaw (65 bytes) }. */
export async function newDeviceKey() {
  const k = await subtle.generateKey(ECDH, false, ['deriveBits']);
  return { privateKey: k.privateKey, publicRaw: new Uint8Array(await subtle.exportKey('raw', k.publicKey)) };
}

/** First 16 bytes of SHA-256 of a public key, base64url (what the QR code pins). */
export async function fingerprint(raw) {
  return b64((await sha256(raw)).slice(0, 16));
}

/**
 * The first frame (PAIR or RESUME) and what is needed to finish.
 * pairing: { rid, code, fp } from the QR code, plus `inv` (the phone that made the link) for a
 * home-screen setup link; resuming: { rid, id, spub }.
 */
export async function hello({ kind, rid, device, id, inv }) {
  const eph = await subtle.generateKey(ECDH, false, ['deriveBits']);
  const ep = new Uint8Array(await subtle.exportKey('raw', eph.publicKey));
  const np = random(16);
  const body = kind === KIND.PAIR
    ? { v: 1, e: b64(ep), d: b64(device.publicRaw), n: b64(np), ...(inv ? { h: inv } : {}) }
    : { v: 1, id, e: b64(ep), n: b64(np) };
  const frame = concat([Uint8Array.of(kind), te.encode(JSON.stringify(body))]);
  return { frame, state: { kind, rid: unb64(rid), eph, ep, np, device } };
}

/**
 * Finishes the handshake from the PC's REPLY. `trust(sRaw)` must say whether the PC's static key
 * is the expected one (the QR fingerprint, or the key pinned at pairing). Throws on any mismatch.
 */
export async function finish(state, reply, { salt, trust }) {
  const v = JSON.parse(td.decode(reply.subarray(1)));
  const ec = unb64(v.e), s = unb64(v.s), nc = unb64(v.n);
  if (v.v !== 1 || ec.length !== 65 || s.length !== 65 || nc.length !== 16) throw new Error('bad reply');
  if (!(await trust(s))) throw new Error('This is not the PC you paired with');
  const ee = await ecdh(state.eph.privateKey, ec);
  const se = await ecdh(state.eph.privateKey, s);
  const de = await ecdh(state.device.privateKey, ec);
  const th = await sha256(concat([te.encode('MangaVox cloud v1'), Uint8Array.of(state.kind), state.rid, state.ep, ec, s, state.device.publicRaw, state.np, nc]));
  const ikm = await subtle.importKey('raw', concat([ee, se, de]), 'HKDF', false, ['deriveBits']);
  const okm = new Uint8Array(await subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt, info: th }, ikm, 512));
  const aes = raw => subtle.importKey('raw', raw, 'AES-GCM', false, ['encrypt', 'decrypt']);
  // The code the PC's Allow prompt shows too (cloud_crypto.rs, verify_code).
  const n = new DataView(th.buffer).getUint32(0) % 1_000_000;
  const code = String(Math.floor(n / 1000)).padStart(3, '0') + ' ' + String(n % 1000).padStart(3, '0');
  return { send: await aes(okm.slice(0, 32)), recv: await aes(okm.slice(32, 64)), spub: s, code };
}

function nonce(dir, ctr) {
  const n = new Uint8Array(12);
  const v = new DataView(n.buffer);
  v.setUint32(0, dir);
  v.setBigUint64(4, BigInt(ctr));
  return n;
}

/**
 * Consumed bytes of a stream are acked once this many have piled up (and never more than the PC's
 * window, 1 MiB), so a big response costs a handful of phone frames instead of one per chunk.
 */
const ACK_EVERY = 256 * 1024;

/**
 * An established session over a relay WebSocket: requests become streams, responses come back
 * in order. Each frame's counter is taken when it is sent or arrives, its sealing or opening
 * starts at once, and only handing the results on is chained, so frames keep their counter order
 * while WebCrypto works on several at a time.
 */
export class Session {
  constructor(keys, sendFrame) {
    this.keys = keys;
    this.sendFrame = sendFrame;
    this.sendCtr = 0;
    this.recvCtr = 0;
    this.out = Promise.resolve();
    this.in = Promise.resolve();
    this.streams = new Map();
    this.nextStream = 1;
    this.onJson = null;
    this.onFail = null;
    this.dead = false;
  }

  send(plain) {
    if (this.dead) return Promise.resolve();
    const ctr = this.sendCtr++;
    const sealing = subtle.encrypt({ name: 'AES-GCM', iv: nonce(DIR_PHONE_TO_PC, ctr) }, this.keys.send, plain);
    this.out = this.out.then(async () => {
      const ct = new Uint8Array(await sealing);
      if (this.dead) return;
      const head = new Uint8Array(9);
      head[0] = KIND.SEALED;
      new DataView(head.buffer).setBigUint64(1, BigInt(ctr));
      this.sendFrame(concat([head, ct]));
    }).catch(e => this.fail(e));
    return this.out;
  }

  json(v) {
    return this.send(concat([Uint8Array.of(MSG.JSON), te.encode(JSON.stringify(v))]));
  }

  /** A sealed frame from the PC; anything that does not open ends the session. */
  receive(frame) {
    if (this.dead) return this.in;
    let opening;
    if (frame[0] !== KIND.SEALED || frame.length < 25) opening = Promise.reject(new Error('bad frame'));
    else {
      const ctr = Number(new DataView(frame.buffer, frame.byteOffset + 1, 8).getBigUint64(0));
      if (ctr !== this.recvCtr) opening = Promise.reject(new Error('out of order'));
      else {
        this.recvCtr++;
        opening = subtle.decrypt({ name: 'AES-GCM', iv: nonce(DIR_PC_TO_PHONE, ctr) }, this.keys.recv, frame.subarray(9));
      }
    }
    opening.catch(() => {}); // handled in order below
    this.in = this.in.then(async () => {
      if (this.dead) return;
      try {
        this.dispatch(new Uint8Array(await opening));
      } catch (e) {
        this.fail(e);
      }
    });
    return this.in;
  }

  dispatch(p) {
    if (p[0] === MSG.JSON) {
      const v = JSON.parse(td.decode(p.subarray(1)));
      const st = v.s != null ? this.streams.get(v.s) : null;
      if (v.t === 'head' && st) st.onHead(v.st, v.h || {});
      else if (v.t === 'err' && st) { this.streams.delete(v.s); st.onError(new Error(v.error || 'failed')); }
      else if (this.onJson) this.onJson(v);
      return;
    }
    const s = new DataView(p.buffer, p.byteOffset + 1, 4).getUint32(0);
    const st = this.streams.get(s);
    if (!st) return;
    if (p[0] === MSG.DATA) st.onData(p.slice(5));
    else if (p[0] === MSG.END) { this.streams.delete(s); st.onEnd(); }
  }

  /**
   * Starts a request. `handlers` get onHead(status, headers), onData(Uint8Array), onEnd(),
   * onError(err). Call `ack(n)` after consuming n bytes (flow control; sent in batches of
   * ACK_EVERY) and `cancel()` to stop.
   */
  request({ method, path, headers, body }, handlers) {
    const s = this.nextStream++;
    this.streams.set(s, handlers);
    const req = { t: 'req', s, m: method, p: path, h: headers || {} };
    if (body && body.byteLength) req.b = b64std(new Uint8Array(body));
    this.json(req);
    let owed = 0;
    return {
      ack: n => {
        owed += n;
        // A finished stream needs no more credit.
        if (owed >= ACK_EVERY && this.streams.has(s)) { this.json({ t: 'ack', s, n: owed }); owed = 0; }
      },
      cancel: () => { if (this.streams.delete(s)) this.json({ t: 'cancel', s }); },
    };
  }

  fail(e) {
    if (this.dead) return;
    this.dead = true;
    for (const st of this.streams.values()) st.onError(new Error('disconnected'));
    this.streams.clear();
    if (this.onFail) this.onFail(e);
  }
}

/** Checks a QR fingerprint against the PC's static key. */
export async function matchesFingerprint(fp, raw) {
  return sameBytes(te.encode(await fingerprint(raw)), te.encode(fp));
}

export { sameBytes };
