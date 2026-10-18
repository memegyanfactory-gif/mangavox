# Native Rust Rewrite — To-do

Plan: [RUST_REWRITE_PLAN.md](RUST_REWRITE_PLAN.md) · `[x]` done · `[~]` in progress · `[ ]` not started

## Phase 0 — Spike & decide
- [x] Inventory the Electron app (pages, services, sources, playback, Studio, assistant)
- [x] Write the plan
- [x] Create `native/` Cargo workspace with the crate layout (app + mv-core, mv-net, mv-sources, mv-plugins, mv-media, mv-ai, mv-torrent)
- [x] GPUI window: title bar + collapsible sidebar + section switch + Library from the real database (builds and runs on this PC)
- [x] Virtualized cover grid with 1,000 cached covers — scrolls smoothly, only visible rows are built (checked on screen)
- [x] libmpv (loaded at runtime) plays video inside the window — needs an owned popup over the player area because GPUI's DirectComposition layer covers child windows (checked on screen)
- [x] Measured: first frame ~600 ms, 65 MB → 95 MB after 5 s. **Decision: GPUI + gpui-component**

## Built in the second wave (native app runs with all of these wired in)
- [x] ui-manga: Library (filters, search, sort), Browse (7 sources + plugins, popular/latest/search), Manga detail, Reader (paged + strip), History, Sources settings — Library/Browse/Detail/Reader checked on screen
- [x] ui-media: Anime home/detail, Movies home/detail, Discover, History, Favorites, Addons settings; addon registry + stream ranking in mv-media — Movies data checked on screen
- [x] ui-player: libmpv player with controls, tracks, speed, torrents via mv-torrent, subtitles, skip intro, resume (not yet run on screen)
- [x] ui-assistant: dock, agent loop + 15 app tools, AI Providers + Model Hub settings (not yet run on screen)
- [x] ui-settings: grouped Settings with Profile, Playback, Appearance, App mode, Storage, About + embedded panels (not yet run on screen)
- [x] mv-webwall: WebView2 Cloudflare solver, wired into the app at startup (Hentai2Read + ComicK live tests pass)
- [x] mv-studio: Python pipeline bridge + job queue + voice catalog (27 tests)

## Known issues found while testing on screen
- [ ] api.jikan.moe is unreachable from this network → Anime rows fail; add an AniList fallback in mv-media
- [~] Remote posters (movies/anime) didn't show → GPUI HTTP client added; needs a visual re-check
- [ ] Reader: tall pages are cut off — fit page to screen height in paged mode
- [ ] Reader: MangaDex chapter list includes non-English chapters — filter by preferred language
- [ ] Continue watching cards have no covers (history rows without cover URLs)
- [ ] Studio page (UI on top of mv-studio)

## Phase 1 — Core
- [x] Paths + `config.json` read/write (atomic writes, unknown keys kept) — mv-core
- [x] Open the existing `mangavox.db` with rusqlite; migrations 1–15 (idempotent, tested on a copy of the real DB) + manga, chapter, history, category, watchlist, media, media-chat repos
- [x] Logging to `~/.mangavox/logs` (same JSON-lines format, rotation, secret redaction)
- [~] HTTP client (retries, rate limits, cookies, proxy URL) + event bus done; DNS fallback and Tor list not ported
- [ ] CI: build + `cargo test`

## Phase 2 — Shell & design system
- [ ] Tokens, themes (all current presets), Inter type scale
- [x] Title bar, sidebar (collapse, sections), route-based navigation, left assistant dock slot (app/src/shell.rs)
- [~] mv-ui: tokens, page/header, buttons, chips, cover cards, skeletons, empty/error states, background runtime, navigation (inputs/menus/dialogs come from gpui-component)
- [ ] Settings frame (grouped list + content column)

## Phase 3 — Manga
- [x] Source trait + 7 native scrapers (MangaDex, WEBTOON, e-hentai, nhentai live-tested; Hentai2Read, ComicK, MangaFire need the Cloudflare solver)
- [~] QuickJS plugin host + repo index/install done (bundled plugins run unchanged); template repos + source health not yet
- [ ] wry wall handling for Cloudflare sources (`WallSolver` trait ready in mv-sources)
- [ ] Browse, Library, History, Manga detail, chapter sync
- [ ] Reader: paged + strip, preloading, progress
- [ ] Chapter downloads

## Phase 4 — Anime & movies catalog
- [~] Jikan, Cinemeta, Stremio addons, AniSkip clients done; installed-addon registry + stream ranking not yet; Jikan fixtures unrecorded (host timed out)
- [ ] Anime home / detail, Movies hub / detail, Discover
- [ ] Watchlist, favourites, history shelves, continue watching

## Phase 5 — Playback
- [x] librqbit engine + range server; direct HTTP streams (live-tested with a public-domain torrent)
- [ ] libmpv player UI: controls, tracks, subtitles (search, cache, style), skip intro, next episode
- [ ] Mini-player, resume sessions
- [ ] Torrent + bulk downloads, storage folders

## Phase 6 — Assistant
- [x] 16 providers (HTTP + CLI) ported; Model Hub backend ported (live OpenCode + Hugging Face tests pass)
- [ ] Agent loop, app tools as Rust functions, approvals, access modes
- [ ] Assistant dock UI (threads, markdown, model picker, composer)
- [ ] Model Hub + AI Providers settings
- [ ] MCP bridge for CLI agents

## Phase 7 — Studio
- [ ] Python bridge (same JSON protocol), queue
- [ ] Text / OCR, narration voices, Audio, Video, Pipeline, Dashboard

## Phase 8 — Remaining settings & platform
- [ ] Profile, playback, storage, extensions, addons, themes, app mode
- [ ] Privacy proxy, tray, YouTube, Subtitles, lock screen, first-run

## Phase 9 — Parity & switch
- [ ] Page-by-page parity checklist
- [ ] Performance targets (< 1 s start, < 250 MB idle)
- [ ] Installers + auto-update
- [ ] Side-by-side beta, then retire Electron
