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
- Every other route except `/locked`, `/favicon.ico`, `/manifest.webmanifest`, `/icon.png`,
  `/icon-512.png`, `/logo.png` requires a valid `mv_phone` cookie. Without it: HTML routes return `/locked` (200, the locked page),
  `/api/*` returns 401 `{"error":"not paired"}`.
- State-changing requests (POST/DELETE) must also carry header `X-MangaVox: 1` (CSRF; the web app
  always sends it) and, when an `Origin` header is present, it must equal `http://<Host>`.
- Revoking a device (desktop Mobile page) makes its cookie invalid immediately.
- Request bodies are capped at 256 KiB (413). No CORS headers are ever sent.
- The phone never names a URL or a file to stream: `/api/watch` takes title (or chapter) ids only, watch
  sessions are 24 random bytes (base64url), belong to the device that started them (anyone else
  gets 404) and end after 30 min without requests.
- Recap narration clips are served only by plain `.wav` names the chapter's own narration lists,
  resolved inside its Studio `audio` folder.
- `/api/img` only fetches http(s) DNS names on the default port; IP literals, `localhost` and
  local-only names are refused (400), and every resolved address (also at connect time and after
  each of at most 3 redirects) must be public: no loopback, RFC 1918, link-local, CGNAT, ULA or
  v6 forms embedding a v4 address. Image types only (no SVG), 15 MB at most.
- Tests: `tests/security.rs` (every route unpaired, peer / Host / CSRF / Origin checks, code reuse,
  expiry and rate limit, revocation, media path traversal incl. junctions, image proxy SSRF,
  unknown watch sessions, recap clip names and recap video sessions).

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
- `GET /app.js`, `GET /app.css`, `GET /manifest.webmanifest`
- `GET /icon.png` (192 px), `GET /icon-512.png`, `GET /logo.png` (96 px, app bar): from
  `mv-ui/assets/phone/`, public.

## API
### Session
- `GET /api/me` -> `{ "device": {"id","name"}, "app": "MangaVox", "version": "8.0.x", "pc": "<computer name>" }`
- `POST /api/logout` -> forgets this device.

### Manga (api_manga.rs)
- `GET /api/library` -> `[{ "id", "title", "author", "cover": "/api/manga/<id>/cover" | null, "unread", "chapters", "last_read_at" }]`
  (library order: last read first; skips browse-only ids like the desktop library page)
- `GET /api/manga/<id>` -> `{ "id","title","author","description","genres":[..],"status","cover",
   "chapters": [{ "id","number","title","read": bool,"progress": 0..1,"pages","downloaded": bool,
                  "audio": bool, "video": bool, "recap": { "video": bool, "aloud": bool } | null }] }`
   (chapters in reading order; `recap` = what Studio left in the chapter's Studio folder (`ui_manga::recap`):
   `video_final.mp4` -> watch it, a voiced `sync_map.json` -> read aloud; null when neither)
- `GET /api/manga/<id>/cover` -> image bytes (cached cover file / downloaded)
- `GET /api/chapter/<id>` -> `{ "id","manga_id","title","number","pages": <n>, "start": <page index to resume>,
   "prev": <chapter id|null>, "next": <chapter id|null> }`
- `GET /api/chapter/<id>/page/<n>` -> image bytes (n is 0-based; downloaded file or fetched via the source and cached)
- `POST /api/chapter/<id>/progress` body `{ "page": n, "total": t }` -> `{}`
- `GET /api/chapter/<id>/recap` -> `{ "video": bool, "aloud": bool, "total_ms",
   "beats": [{ "page", "start_ms", "end_ms", "y0", "y1", "text" }],
   "clips": [{ "url": "/api/chapter/<id>/recap/audio/<file>", "start_ms", "end_ms" }] }` : the chapter's
   Studio narration as one timeline. `page` is an index into the reader's page list (`/page/<n>`; Studio's
   pages matched by file name), `y0..y1` the band of the page being read (0..1; the whole page when the
   passage is about no one panel). A clip's first sample plays at `start_ms`; clips are back to back.
   Empty `beats` / `clips` (and `aloud: false`) when there is only a video. 404 when Studio made nothing.
- `GET /api/chapter/<id>/recap/audio/<file>` -> one narration clip (WAV), with HTTP Range. `<file>` must be a
   plain `[A-Za-z0-9_.-]+.wav` name (else 400) that the chapter's narration lists, inside its `audio`
   folder (else 404).
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
- `GET /api/movies?tab=home|movies|series` -> `{ "hero": Item|null, "rows": [{ "id","title","top10": bool,"items": [Item] }], "error": string|null }`
  (rows that fail to load are left out, never a 5xx; `error` is set only when nothing loaded)
  `Item = { "id","imdb","kind": "movie"|"series","title","year","description","genres":[..],"score",
            "poster","background","logo" }`
- `GET /api/movies/search?q=` -> `[Item]`
- `GET /api/movies/<imdb>?kind=movie|series` -> `Item + { "cast":[..],"runtime","seasons": [{ "season", "episodes": [{ "episode","title","thumbnail","overview" }] }] }`
- `POST /api/movies/play` body `{ "imdb","kind","title","season"?, "episode"? }` -> `{}` : finds sources and
  starts playback ON THE PC (desktop player), like the desktop Play button. 404 `{"error"}` with a
  readable reason (no addons installed -> where to add one on the PC, no sources found).
- `POST /api/pc/open` body `{ "page": "library"|"movies"|"anime"|"studio"|"settings"|"mobile",
   "manga_id"?, "imdb"?, "kind"?, "title"?, "mal"? }` -> `{}` : navigates the desktop app
   (`anime` + `mal` opens that anime's detail page).

### Anime (api_anime.rs; the same Jikan caches as the desktop Anime pages)
- `GET /api/anime?tab=home|airing|top|movies` -> `{ "hero": A|null, "rows": [{ "id","title","top10","items":[A] }], "error": string|null }`
  `A = { "id": "<mal id>", "mal", "kind": "anime", "title","year","description","genres":[..],"score",
         "poster","background","episodes": n|null, "format": "TV"|"Movie"|.., "status" }`
- `GET /api/anime/search?q=` -> `[A]`
- `GET /api/anime/<mal>` -> `A + { "episodes_list": [{ "episode","title","thumbnail"|null,"aired"|null,"filler": bool }], "next_page": n|null }`
- `GET /api/anime/<mal>/episodes?page=n` -> `{ "episodes": [...], "next_page": n|null }` (100 per page)
- `POST /api/anime/play` body `{ "mal","title","episode"? }` -> `{}` : plays on the PC (like /api/movies/play).

### Watch on phone (api_watch.rs)
- `POST /api/watch` body `{ "imdb"?: "tt..", "mal"?: "<mal id>", "kind": "movie"|"series"|"anime", "title",
   "season"?, "episode"? }` -> finds sources like the desktop Play button (phone-friendly releases first:
   no 4K / remux / HEVC when there is a choice), opens the best on the PC's torrent engine (or its Range
   proxy for http links; NOT the desktop player), probes it with ffprobe, and answers
   `{ "id", "title", "duration": s|null, "mode": "direct"|"remux"|"transcode", "url": "/api/watch/<id>/stream",
      "resume": s|null, "subtitles": [{ "id","label","lang","url": "/api/watch/<id>/sub/<sid>.vtt" }],
      "next": { "season","episode","title" }|null }`.
   Up to 3 sources are tried within 90 s (torrents wait for metadata). Errors: 400 bad ids, 404 no
   sources / no addons, 503 `"Install FFmpeg on your PC to watch this on your phone"`, 504 nothing
   started in time, 502 otherwise. Records a "play" event (Continue Watching on the PC).
   A device keeps at most 3 sessions; a 4th stops the oldest.
- `POST /api/watch` body `{ "kind": "recap", "chapter": "<chapter id>", "title"? }` -> the same session shape
   for that chapter's Studio recap video (`video_final.mp4` on the PC; `direct` unless ffprobe says it needs
   converting), with `resume`, `next` null and no subtitles. Nothing is recorded: `progress` is accepted
   and ignored. 400 no chapter id, 404 unknown chapter or no recap video.
- `GET /api/watch/<id>/stream?start=<s>`:
   - `direct` (MP4 with 8-bit H.264 + AAC/MP3, or .webm with VP8/VP9/AV1 + Opus/Vorbis): the file as is,
     HTTP Range forwarded (206, Content-Range, Content-Length, Accept-Ranges); `start` ignored. A recap is
     served straight from its file, with Range.
   - `remux` (H.264 in another container or with other audio: video copied) / `transcode` (libx264
     veryfast CRF 23, at most 1280 wide): fragmented MP4 (H.264 + AAC stereo 160k) from ffmpeg starting
     at `start`, no Range or Content-Length. A newer stream request for the session, `stop`, or the
     phone disconnecting kills the previous ffmpeg.
- `GET /api/watch/<id>/status` -> `{ "state": "ready"|"error", "peers": n|null, "down_bps": n|null,
   "progress": 0..1|null, "error": string|null }` (torrent stats; nulls for http sources).
- `POST /api/watch/<id>/progress` body `{ "pos": s, "duration": s }` -> `{}` (same history / resume as the
   desktop player; past 90% counts as watched).
- `POST /api/watch/<id>/stop` -> `{}` (kills ffmpeg, releases the torrent).
- `GET /api/watch/<id>/sub/<sid>.vtt` -> `text/vtt`: `e<n>` embedded text tracks (SRT/ASS/mov_text/WebVTT,
  extracted with ffmpeg; this reads the whole file, so on a torrent it can take minutes), `x<n>` online
  subtitles from the desktop's OpenSubtitles lookup (movies / series, English first), SRT converted.
  4 MB cap, cached per session.
- Unknown or another device's session id: 404.

### Images (api_img.rs)
- `GET /api/img?u=<url-encoded http(s) image url>&w=<max width>` -> image bytes,
  `Cache-Control: private, max-age=604800`. Fetched by the PC and cached in `<root>/cache/phone-img`
  (sha256 of url + w); JPEG / PNG wider than `w` (64..1920) are scaled down. 400 for URLs that are not
  allowed (see Security), 502 when the fetch fails or the answer is not an image.

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
