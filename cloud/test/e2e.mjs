// End-to-end check of cloud access: the phone side (web/tunnel.js, the same file the browser runs)
// against a running relay and a PC (the `cloud_peer` example, or MangaVox with From anywhere on).
//
//   node test/e2e.mjs '<pairing link printed by cloud_peer>'
//
// Pairs, loads the app shell and /api/me through the tunnel, checks that remote control is refused
// and that a big response arrives intact with flow control, then reconnects with the stored key.
import { KIND, hello, finish, Session, newDeviceKey, matchesFingerprint, sameBytes, unb64 } from '../web/tunnel.js';

const link = process.argv[2];
if (!link || !link.includes('#p=')) {
  console.error('usage: node test/e2e.mjs <pairing link>');
  process.exit(2);
}
const [base, frag] = link.split('/#');
const q = new URLSearchParams(frag);
const [rid, code, fp] = q.get('p').split('.');
const origin = new URL(base).origin;
const wsBase = base.replace(/^http/, 'ws') + '/ws/phone?room=' + rid;

let failures = 0;
const check = (ok, what) => { console.log((ok ? 'ok   ' : 'FAIL ') + what); if (!ok) failures++; };

function openSocket(headers = { Origin: origin }) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsBase, { headers });
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => resolve(ws);
    ws.onerror = e => reject(new Error('socket error ' + (e.message || '')));
  });
}

/** Handshake; resolves { ws, session, ok } or rejects with the REFUSE body. */
async function connect(mode, device, pc) {
  const ws = await openSocket();
  const frames = [];
  let wake = null;
  ws.onmessage = ev => { if (typeof ev.data !== 'string') { frames.push(new Uint8Array(ev.data)); if (wake) wake(); } };
  const next = async () => { while (!frames.length) await new Promise(r => { wake = r; }); return frames.shift(); };
  const h = await hello(mode === 'pair' ? { kind: KIND.PAIR, rid, device } : { kind: KIND.RESUME, rid, device, id: pc.id });
  ws.send(h.frame);
  const reply = await next();
  if (reply[0] === KIND.REFUSE) throw JSON.parse(new TextDecoder().decode(reply.subarray(1)));
  const keys = await finish(h.state, reply, mode === 'pair'
    ? { salt: unb64(code), trust: s => matchesFingerprint(fp, s) }
    : { salt: new Uint8Array(32), trust: async s => sameBytes(s, pc.spub) });
  const session = new Session(keys, f => ws.send(f));
  const control = [];
  session.onJson = v => { control.push(v); if (wake) wake(); };
  ws.onmessage = ev => {
    if (typeof ev.data === 'string') return;
    const f = new Uint8Array(ev.data);
    if (f[0] === KIND.REFUSE) { control.push({ refused: JSON.parse(new TextDecoder().decode(f.subarray(1))) }); if (wake) wake(); return; }
    session.receive(f);
  };
  await session.json({ t: 'fin', ua: 'Mozilla/5.0 (Linux; Android 14; Pixel 8) Chrome/120.0 Mobile Safari/537.36' });
  for (;;) {
    while (!control.length) await new Promise(r => { wake = r; });
    const v = control.shift();
    if (v.refused) throw v.refused;
    if (v.t === 'ok') return { ws, session, ok: v, spub: keys.spub };
  }
}

/** A request as the app sends it (its CSRF header on changes, unless `csrf` is false). */
function request(session, method, path, body, csrf = true) {
  return new Promise((resolve, reject) => {
    let status = 0;
    let head = {};
    const parts = [];
    const headers = {};
    if (body) headers['content-type'] = 'application/json';
    if (csrf && method !== 'GET') headers['x-mangavox'] = '1';
    const h = session.request({ method, path, headers, body: body ? new TextEncoder().encode(JSON.stringify(body)) : null }, {
      onHead(st, hd) { status = st; head = hd; },
      onData(b) { parts.push(b); h.ack(b.byteLength); },
      onEnd() {
        const all = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
        let i = 0;
        for (const p of parts) { all.set(p, i); i += p.length; }
        resolve({ status, headers: head, bytes: all, text: new TextDecoder().decode(all) });
      },
      onError: reject,
    });
  });
}

// A socket from another site is refused by the relay.
try {
  await openSocket({ Origin: 'https://evil.example' });
  check(false, 'relay refuses a phone socket from another origin');
} catch {
  check(true, 'relay refuses a phone socket from another origin');
}

const device = await newDeviceKey();
const t0 = Date.now();
const a = await connect('pair', device);
check(!!a.ok.id, `paired as ${a.ok.name} (${a.ok.id}) on ${a.ok.pc}, MangaVox ${a.ok.version}, in ${Date.now() - t0} ms`);

const me = await request(a.session, 'GET', '/api/me');
check(me.status === 200 && me.text.includes(a.ok.id), `/api/me through the tunnel: ${me.status} ${me.text}`);
const shell = await request(a.session, 'GET', '/');
check(shell.status === 200 && shell.text.includes('/app.js'), `app shell: ${shell.status}, ${shell.bytes.length} bytes`);
const js = await request(a.session, 'GET', '/app.js');
check(js.status === 200 && js.bytes.length > 100_000 && js.text.trimEnd().endsWith('boot();'), `app.js in chunks with flow control: ${js.bytes.length} bytes intact`);
// The page names its script by content hash, and that address may be kept on the phone (sw.js).
const jsPath = (/src="(\/app\.js\?v=[0-9a-f]+)"/.exec(shell.text) || [])[1];
const hashed = jsPath ? await request(a.session, 'GET', jsPath) : { status: 0, headers: {} };
check(hashed.status === 200 && /immutable/.test(hashed.headers['cache-control'] || '') && hashed.bytes.length === js.bytes.length, `the script is cacheable by its hash: ${jsPath}`);
const mouse = await request(a.session, 'POST', '/api/mouse', { t: 'move', dx: 1, dy: 1 });
check(mouse.status === 403, `remote control refused without Allow control: ${mouse.status}`);
const forged = await request(a.session, 'POST', '/api/logout', {}, false);
check(forged.status === 403, `a change without the app's CSRF header is refused: ${forged.status}`);
const pair = await request(a.session, 'GET', '/pair?code=x');
check(pair.status === 404, `/pair is not reachable through the cloud: ${pair.status}`);
const many = await Promise.all(Array.from({ length: 10 }, () => request(a.session, 'GET', '/api/me')));
check(many.every(r => r.status === 200), 'ten requests in parallel');
a.ws.close();

// The code is used up: pairing again with it fails.
try {
  await connect('pair', await newDeviceKey());
  check(false, 'a used code cannot pair a second phone');
} catch (e) {
  check(e.code === 'pair', `a used code cannot pair a second phone (${e.code})`);
}

// Reconnect with the stored device key.
const pc = { id: a.ok.id, spub: a.spub };
const b = await connect('resume', device, pc);
const me2 = await request(b.session, 'GET', '/api/me');
check(me2.status === 200 && me2.text.includes(a.ok.id), 'resumed with the stored key');
b.ws.close();

// Someone with the device id but not the key gets nothing.
try {
  await connect('resume', await newDeviceKey(), pc);
  check(false, 'a different key cannot resume');
} catch (e) {
  check(e.code === 'unknown', `a different key cannot resume (${e.code})`);
}

console.log(failures ? `${failures} FAILED` : 'all passed');
process.exit(failures ? 1 : 0);
