# Cloud access ("from anywhere")

A paired phone reaches this PC through the relay at `https://app.bhippi.com/mangavox` (the
`cloud/` Worker at the repository root) when it is not on the same Wi-Fi. The relay only passes
sealed bytes along: everything between phone and PC is end-to-end encrypted and authenticated, so
the relay (or anyone who breaks into it) can neither read nor forge traffic. The PC only dials out:
no port is opened on the router.

Code: `src/cloud.rs` (relay connection, sessions, tunnelled requests, approvals),
`src/cloud_crypto.rs` (handshake and sealing), `cloud/web/tunnel.js` (the phone side of both).

## Keys and identifiers

| What | Where | Notes |
|---|---|---|
| Room id `rid` | `<root>/phone/cloud.json` | 16 random bytes, base64url. Names the PC's room on the relay. Not secret. |
| Room token | `cloud.json` | 32 random bytes. The relay keeps only its SHA-256; it lets only this PC take the PC end of the room. |
| PC static key `S` | `cloud.json` | ECDH P-256. Phones pin its public key at pairing. |
| Device key `D` | phone, IndexedDB | ECDH P-256, created non-extractable in WebCrypto. The PC keeps the public key in `devices.json`. |
| Cloud pairing code `ps` | memory only | 32 random bytes, single use, 5 minutes, newest only (like the LAN code). |

## QR code

With cloud access on, the Mobile page's QR code is

    https://app.bhippi.com/mangavox/#p=<rid>.<ps>.<fp>&l=<LAN pair URL, URI-encoded>

`fp` = first 16 bytes of SHA-256(`S` public, 65-byte uncompressed), base64url. Everything is in
the fragment, which browsers never send to a server; the page reads it and removes it from the
address bar at once. The page offers **Same Wi-Fi** (opens the `l` URL, the existing LAN pairing)
and **From anywhere** (cloud pairing below).

## Relay (cloud/src/worker.js)

- PC: `GET <base>/ws/pc?room=<rid>` (WebSocket) with `Authorization: Bearer <token>`. The first
  PC to connect claims the room (SHA-256 of the token stored); later ones must match. A newer PC
  connection replaces the older one.
- Phone: `GET <base>/ws/phone?room=<rid>` (WebSocket); `Origin` must be the relay's own origin.
  Refused (close 4004) when the PC is not connected; at most 8 phones per room (4008).
- Relay to PC, text: `{"t":"open","c":<u32>}` (a phone connected), `{"t":"gone","c":<u32>}`.
  PC to relay, text: `{"t":"close","c":<u32>}`. Text `ping` is answered `pong` on both ends.
- Binary: phone frames reach the PC as `[c: u32 BE][frame]`; PC frames `[c][frame]` go to phone `c`.
- Limits: phone frames 512 KiB, PC frames 1 MiB, 400 phone frames per 10 s per phone, 3 phones per
  address per room. A room whose PC has not connected for 180 days is deleted.

## Frames (inside the relay's binary messages)

Byte 0 is the kind; the rest is the body.

| Kind | Dir | Body |
|---|---|---|
| 1 PAIR | phone→PC | JSON `{v:1, e, d, n, h?}`: ephemeral key, device key (base64url, 65-byte uncompressed points), 16-byte nonce; `h`: the phone whose home-screen invite this uses (below) |
| 2 RESUME | phone→PC | JSON `{v:1, id, e, n}`: device id |
| 3 REPLY | PC→phone | JSON `{v:1, e, s, n}`: PC ephemeral key, PC static key, nonce |
| 4 REFUSE | PC→phone | JSON `{code, error}`; the PC then closes the connection. `code`: `pair` (code wrong, used or expired), `unknown` (device removed), `denied`, `busy`, `bad` |
| 5 SEALED | both | `[ctr: u64 BE][AES-256-GCM ciphertext ‖ 16-byte tag]` |

### Handshake

Both sides compute, with ECDH results being the 32-byte x-coordinate:

    ikm  = ECDH(Ep, Ec) ‖ ECDH(Ep, S) ‖ ECDH(D, Ec)
    th   = SHA-256("MangaVox cloud v1" ‖ kind ‖ rid bytes ‖ Ep ‖ Ec ‖ S ‖ D ‖ np ‖ nc)
    salt = ps (PAIR) or 32 zero bytes (RESUME)
    okm  = HKDF-SHA256(salt, ikm, info = th, 64 bytes)
    phone→PC key = okm[0..32], PC→phone key = okm[32..64]

`ECDH(Ep, S)` proves the PC holds `S`; `ECDH(D, Ec)` proves the phone holds `D`; `ECDH(Ep, Ec)`
gives forward secrecy; `ps` in the salt proves a pairing phone saw the QR code. The phone checks
`S` against `fp` (PAIR) or the pinned key (RESUME) before going on.

Verification code: `u32 BE(th[0..4]) mod 1 000 000` as "123 456". A pairing phone shows it while
it waits, and the PC's Allow prompt shows it too, so someone else who saw the QR code cannot pass
for the owner's phone.

1. Phone sends PAIR or RESUME. The PC answers REPLY (or REFUSE for an unknown device / no live code).
2. Phone sends SEALED `{t:"fin"}` (PAIR also `name`, e.g. "iPhone · Safari"). The PC opening it is
   the proof; a PAIR that fails it counts towards the pairing rate limit (20 per 10 minutes).
3. PAIR only: the PC sends SEALED `{t:"wait"}` and asks on its Mobile page "Allow <name> to
   connect from anywhere?" with the verification code, waiting up to 2 minutes. The code is used up
   on the first valid `fin`. Only the phone leaving or its own sealed `{t:"cancel"}` ends the wait
   early: frames that do not open are ignored.
4. The PC sends SEALED `{t:"ok", id, name, pc, version}`. A pairing phone trusts the session only
   after opening it. A resuming phone may send requests right after its `fin`: the PC sends `ok`
   before it reads them, and only the PC holding `S` can open them (the connector starts loading
   the app a round trip earlier this way).

The PC drops a connection that has not finished the handshake in 20 s (plus the approval wait).

### Sealed messages

Nonce = 4-byte direction (`00 00 00 01` phone→PC, `00 00 00 02` PC→phone) ‖ `ctr` (u64 BE).
`ctr` starts at 0 and must be exactly one more than the last one received; any failure ends the
session. Plaintext byte 0:

- `0` JSON message
- `1` DATA `[stream: u32 BE][bytes]` (≤ 128 KiB; older PCs send ≤ 60 KiB)
- `2` END `[stream: u32 BE]`

JSON from the phone: `{t:"fin"}`, `{t:"req", s, m, p, h, b?}` (stream id, method, path + query
starting with a single `/`, headers, base64 body ≤ 256 KiB), `{t:"ack", s, n}` (n bytes of
stream s consumed), `{t:"cancel", s}`. From the PC: `{t:"ok", …}`, `{t:"head", s, st, h}`
(status, headers), `{t:"err", s, error}` (a stream that failed after its head).

Forwarded request headers: `range`, `if-range`, `content-type`, `accept`, `x-mangavox`. Response
headers: `content-type`, `content-length`, `content-range`, `accept-ranges`, `cache-control`,
`etag`, `last-modified`, `content-disposition`, `content-security-policy`, `x-frame-options`,
`x-content-type-options`, `referrer-policy`. Never cookies.

Flow control: each stream may have 1 MiB sent but not acked; the phone acks as the page reads,
once 256 KiB of a stream are read (never for the tail of a finished stream), so a big response
costs a few phone frames rather than one per chunk. At most 32 open streams per phone (more get
a 503 head).

## On the PC

A tunnelled request runs through the same router and guard as a LAN request, carrying a
`CloudPeer` extension instead of a peer address (it cannot come from the network). The guard
then checks the device still exists and is a cloud device, and that state-changing requests
carry `X-MangaVox: 1` like on the Wi-Fi; cookies, the LAN-peer and Host checks do not apply (a
request can only enter through the authenticated session). Remote screen
and touchpad (`/api/screen/*`, `/api/mouse`) answer 403 until "Allow control" is turned on for
that phone on the Mobile page. Removing a phone ends its sessions at once.

Sending: messages wait in two lanes in front of the relay socket and are sealed only as they
leave, so counters stay in order. Handshake frames, JSON messages and whole responses of a known
size up to 64 KiB (API answers, icons) take the control lane, which always goes first; bigger
bodies (images, video) take the bulk lane one chunk at a time, and the socket's own queue holds
two messages. So a tap never waits behind a chapter's worth of images, and one phone's download
cannot hold up another phone's handshake. A stream keeps its whole body on one lane, after its
head. The socket runs without Nagle's algorithm (it held the end of every frame for the relay's
delayed ACK).

## On the phone (cloud/web)

`index.html` + `connect.js` hold the relay WebSocket and the session; `sw.js` (scope
`/mangavox/`) sends every request of the app frame (`/mangavox/app/<token>/`, the PC's own web
app loaded through the tunnel) to the connector page, which tunnels it. The token is 16 random
bytes made for each page load and never leaves the page and its frame: the connector answers only
requests carrying it, and the worker tunnels navigations only into an iframe, so another site that
opens, frames or posts a form to `/mangavox/app/...` reaches nothing. So the app's HTML, script and
data all come from the PC: the relay serves only the connector, and the phone app always matches
the MangaVox version on the PC.

The service worker keeps what the PC marks cacheable for an hour or more (`cache-control`
max-age ≥ 3600, not `no-store`/`no-cache`, a known size up to 1 MiB, no range; never chapter
pages) in the Cache API, at most 400 entries, oldest dropped first, and answers the app frame's
requests for it while fresh. The PC names its script and styles by content hash
(`/app.js?v=<hash>`, kept a year) while the page itself is `no-cache`, so a phone loads the new
app right after a MangaVox update and otherwise only the page and the API go through the relay.
The cache is cleared when the phone forgets its PC or pairs with one; the pairing in IndexedDB
changes first, and the worker keeps an answer only while the pairing it came through is still the
saved one (checked again after each write), so nothing stored in flight survives a forget. Every
write drops the oldest entries beyond 400, and the first write after the worker starts also drops
expired ones.

## Home-screen icon

On iPhone a home-screen web app has its own storage, separate from Safari's: the device key a
phone paired with in Safari is not there when it is opened from its icon. So a paired cloud
phone can set up its icon:

1. More → Add to Home Screen asks `POST /api/cloud/invite` through the session (only a request
   carrying `CloudPeer`; a phone on the Wi-Fi gets 403). The PC makes an invite: 32 random bytes,
   in memory only, single use, 30 minutes, the newest only per phone, and dead once that phone is
   removed. It answers `{link}`: `<relay>/#p=<rid>.<invite>.<fp>&h=<device id>`.
2. The app writes the link to the clipboard (inside the tap) and hands it to the connector, which
   puts it in its address bar (`history.replaceState`) and stores it for `sw.js`, which serves the
   manifest with it as `start_url` while it works. Either way "Add to Home Screen" saves an icon
   that opens with the link. It is only ever in a fragment, the clipboard or the phone's storage,
   never sent to the relay.
3. The icon opens with the link (or, when the browser saved another address, its "Set up this
   icon" screen pastes it) and pairs: PAIR with `h`, the invite as `ps`. The handshake is the same;
   the PC then adds it as "<name> (home screen)" without asking, since a paired phone vouched for
   it. A failed fin counts towards the pairing rate limit like a wrong QR code. Opened again with
   the same link while paired with that PC, the icon just resumes.

## Threat notes

- Relay compromised: it sees room ids, timing and sizes, can drop or delay traffic, but cannot
  read, forge, replay or reorder anything, and cannot pair (no `ps`) or resume (no `D`, no `S`).
- Whoever can deploy to `app.bhippi.com` (the bhippi.com Cloudflare account) is not covered:
  the connector page is the trust anchor on the phone, and a changed page could use the device
  key. So "end-to-end" protects against a broken or snooping relay, not against the account that
  publishes the page. It is kept tiny, served with a strict CSP and no third-party code, and
  `app.bhippi.com` must not host anything else that runs script (same origin = same IndexedDB).
- The PC's app runs on the connector's origin: a script injection bug in the app would reach the
  device key too. The app inserts all network text with `textContent` (app.js header).
- A cloud phone with "Allow control" can see the Mobile page, so it could also allow new phones.
- The phone's cache holds the app's script and styles, covers and artwork in the clear, like the
  browser cache does on the Wi-Fi: someone holding the unlocked phone can see them. Only the
  service worker writes it, from tunnelled answers, and only the app frame reads it.
- Anyone who knows a room id (the relay, phones paired to it, someone who saw an old QR code) can
  hold up to 3 sockets from one address, or burn the cloud pairing rate limit (20 failures per 10
  minutes); paired phones' resume is not rate limited.
- A home-screen setup link pairs without a question on the PC: whoever gets it within its 30
  minutes, before the icon uses it, can add a phone (shown on the Mobile page, where it can be
  removed). It is made only for a phone that is already paired through the cloud, and it never
  passes the relay, which would still need the device key or the invite to pair.
- A photographed or screen-shared QR code: the PC still asks before a cloud phone is added, and
  shows the verification code to compare.
