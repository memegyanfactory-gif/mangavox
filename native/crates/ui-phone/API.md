# ui-phone HTTP contract

Everything is same-origin, served by `server.rs` on `0.0.0.0:<port>` (default 8790).
JSON bodies use `Content-Type: application/json`. Errors: HTTP 4xx/5xx with `{"error": "message"}`.

## Security (auth.rs, applied to every route by server.rs)
- Peer IP must be loopback, RFC1918 (10/8, 172.16/12, 192.168/16), link-local (169.254/16, fe80::/10)
  or IPv6 ULA (fc00::/7). Anything else: 403.
- `Host` header must be an IP literal (optionally with port) or `localhost`: blocks DNS rebinding. Else 403.
- Pairing: `GET /pair?code=<code>`. The code is 32 random bytes, base64url, single use, valid
  `PAIR_TTL_SECS` (300 s), and only the most recent code is valid (showing a new QR invalidates the old).
  Constant-time compare. Max 20 failed /pair attempts per IP per 10 minutes (then 429).
  On success: create a device (id = random 8 bytes hex, name from User-Agent, e.g. "iPhone · Safari"),
  a 32-byte random session secret; persist `sha256(secret)` in `<root>/phone/devices.json`;
  set cookie `mv_phone=<secret>; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000`; 303 to `/`.
- Every other route except `/locked`, `/favicon.ico`, `/manifest.webmanifest`, `/icon.png`
  requires a valid `mv_phone` cookie. Without it: HTML routes return `/locked` (200, the locked page),
  `/api/*` returns 401 `{"error":"not paired"}`.
- State-changing requests (POST/DELETE) must also carry header `X-MangaVox: 1` (CSRF; the web app
  always sends it) and, when an `Origin` header is present, it must equal `http://<Host>`.
- Revoking a device (desktop Mobile page) makes its cookie invalid immediately.
- Request bodies are capped at 256 KiB (413). No CORS headers are ever sent.
- Tests: `tests/security.rs` (every route unpaired, peer / Host / CSRF / Origin checks, code reuse,
  expiry and rate limit, revocation, media path traversal incl. junctions).

Known limits (design tradeoffs, not bugs):
- Plain HTTP on the LAN: someone who can sniff the Wi-Fi (open network, or a WPA2-PSK network whose
  password they know) can read the QR link as the phone opens it and the `mv_phone` cookie after, and
  reuse them. The cookie cannot be `Secure`. Remove a device from the Mobile page to cut it off.
- A paired phone controls the MangaVox window like someone sitting at it (remote screen + input),
  so it can do whatever the app's UI can (Settings, the Mobile page's QR code, Studio jobs). It cannot
  reach other windows: input is posted to the MangaVox window only, and the screen-copy fallback is
  used only when MangaVox is in front and not covered.
- A stream already open (a video download) keeps going after its device is revoked; new requests fail.
- No connection limit or header timeout beyond hyper's defaults (fine on a home LAN).

## Static (web/, embedded with include_str!/include_bytes!)
- `GET /` -> `web/index.html` (paired) ; `GET /locked` -> `web/locked.html`
- `GET /app.js`, `GET /app.css`, `GET /manifest.webmanifest`, `GET /icon.png`

## API
### Session
- `GET /api/me` -> `{ "device": {"id","name"}, "app": "MangaVox", "version": "8.0.x", "pc": "<computer name>" }`
- `POST /api/logout` -> forgets this device.

### Manga (api_manga.rs)
- `GET /api/library` -> `[{ "id", "title", "author", "cover": "/api/manga/<id>/cover" | null, "unread", "chapters", "last_read_at" }]`
  (library order: last read first; skips browse-only ids like the desktop library page)
- `GET /api/manga/<id>` -> `{ "id","title","author","description","genres":[..],"status","cover",
   "chapters": [{ "id","number","title","read": bool,"progress": 0..1,"pages","downloaded": bool,
                  "audio": bool, "video": bool }] }`   (chapters in reading order)
- `GET /api/manga/<id>/cover` -> image bytes (cached cover file / downloaded)
- `GET /api/chapter/<id>` -> `{ "id","manga_id","title","number","pages": <n>, "start": <page index to resume>,
   "prev": <chapter id|null>, "next": <chapter id|null> }`
- `GET /api/chapter/<id>/page/<n>` -> image bytes (n is 0-based; downloaded file or fetched via the source and cached)
- `POST /api/chapter/<id>/progress` body `{ "page": n, "total": t }` -> `{}`
- `POST /api/manga/<id>/refresh` -> same as `GET /api/manga/<id>` after refreshing chapters from the source

### Studio + Listen (api_studio.rs)
- `GET /api/studio` -> `{ "ready": bool, "reason": string|null, "manga": [{ "id","title","cover",
   "chapters": [{ "id","title","number","audio": bool,"video": bool }] }] }`  (downloaded chapters only)
- `POST /api/studio/generate` body `{ "chapters": [<chapter id>..], "what": "video" | "audio" | "full" }`
  -> `{ "jobs": [<id>..] }`  ("audio" = narration only, "video" = narrated video per chapter,
  "full" = full Read→Video pipeline incl. the joined recap, when available)
- `GET /api/studio/jobs` -> `[{ "id","title","stage","progress": 0..1,"state": "queued"|"running"|"done"|"failed"|"cancelled","error" }]`
- `POST /api/studio/jobs/<id>/cancel` -> `{}`
- `GET /api/media` -> `[{ "id": "<opaque>", "title", "manga", "chapter", "kind": "video"|"audio",
   "url": "/api/media/<opaque>", "size", "modified" }]`  (narration audio, chapter videos, full recaps; newest first)
- `GET /api/media/<opaque>` -> the file, with HTTP Range support (video/audio streaming + download).
  `<opaque>` maps to a path under `<root>/studio/projects` only (never a raw path from the client).

### Movies (api_media.rs)
- `GET /api/movies?tab=home|movies|series` -> `{ "hero": Item|null, "rows": [{ "id","title","top10": bool,"items": [Item] }] }`
  `Item = { "id","imdb","kind": "movie"|"series","title","year","description","genres":[..],"score",
            "poster","background","logo" }`
- `GET /api/movies/search?q=` -> `[Item]`
- `GET /api/movies/<imdb>?kind=movie|series` -> `Item + { "cast":[..],"runtime","seasons": [{ "season", "episodes": [{ "episode","title","thumbnail","overview" }] }] }`
- `POST /api/movies/play` body `{ "imdb","kind","title","season"?, "episode"? }` -> `{}` : finds sources and
  starts playback ON THE PC (desktop player), like the desktop Play button.
- `POST /api/pc/open` body `{ "page": "library"|"movies"|"anime"|"studio"|"settings"|"mobile",
   "manga_id"?, "imdb"?, "kind"? }` -> `{}` : navigates the desktop app.

### Remote screen (screen.rs)
- `GET /api/screen/frame?w=<max width>&q=<jpeg quality 30..90>` -> `image/jpeg` of the main window's
  client area, header `X-Frame-Size: <W>x<H>` (client-area pixel size the tap coordinates refer to).
- `POST /api/screen/input` body one of:
  `{ "t":"tap","x","y" }` (x,y in 0..1 of the client area) |
  `{ "t":"long","x","y" }` (right click) |
  `{ "t":"scroll","x","y","dy" }` (dy in wheel notches, +down) |
  `{ "t":"drag","x","y","x2","y2" }` |
  `{ "t":"key","key": "enter"|"escape"|"backspace"|"tab"|"left"|"right"|"up"|"down"|"space"|"pageup"|"pagedown"|"home"|"end", "ctrl"?:bool, "shift"?:bool, "alt"?:bool }` |
  `{ "t":"text","s":"hello" }`
  -> `{}`. Input goes to the MangaVox window only (posted window messages, the PC's cursor is not moved).
