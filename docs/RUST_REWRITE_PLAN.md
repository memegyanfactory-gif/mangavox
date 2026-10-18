# MangaVox — Native Rust Rewrite Plan

Goal: rebuild MangaVox as a native Rust desktop app — no Electron, no webview for the
UI — that looks and feels like Nexa: one quiet left sidebar, a slim title bar, a
dockable assistant, clean pages with very few icons. Same features, same user data,
much smaller and faster.

Progress: [RUST_REWRITE_TODO.md](RUST_REWRITE_TODO.md)

---

## 1. What has to be rebuilt (inventory, 2026-10-18)

| Area | Today (Electron + TypeScript) | Size |
|---|---|---|
| UI | React 19, Tailwind, framer-motion, 22 pages, ~60 components | ~25k lines of pages, ~75k total TS |
| Data | SQLite via better-sqlite3, migrations, 10 repos; `~/.mangavox/config.json` | |
| Manga sources | 7 native scrapers (MangaDex, ComicK, MangaFire, nhentai, e-hentai, Hentai2Read, WEBTOON) + JS plugin sandbox (`plugins/*/index.js`, `__fetch`) + repo manager + health checks | ~3k lines |
| Cloudflare-walled sources | Hidden Electron `BrowserWindow`s (ComicK, MangaFire, nhentai, browser-fetch) | 1.5k lines |
| Anime / movies | Jikan (MAL), Cinemeta, Stremio addon protocol, streaming providers, skip times (AniSkip) | ~3k lines |
| Playback | WebTorrent engine + local stream protocol, direct HTTP streams, HTML `<video>` player, subtitles (search, cache, styling), mini-player / PiP, playback sessions | ~3.5k lines |
| Downloads | Chapter downloads, bulk downloads, torrent downloads, storage folders | ~1.5k lines |
| Reader | Paged + webtoon strip reader, overlays | |
| Studio | Text extraction / OCR, narration (TTS: Kokoro, F5, Parler, XTTS), audio, video generation, pipeline queue — **Python** (25 scripts) via a JSON bridge | ~3k TS + Python |
| Assistant | Multi-provider AI (16 providers), app tools, Model Hub (just built), legacy chat page, research, memory, skills | ~6k lines |
| Platform | Tray, window controls, deep links, logging, debug bridge, privacy proxy (Tor), DNS-aware fetch | |

## 2. Architecture

```
mangavox-rs/                      (new Cargo workspace in ./native)
├─ app/                 binary: window, tray, startup, wiring
├─ crates/
│  ├─ mv-core           paths, config.json, SQLite (same schema + migrations), logging, events
│  ├─ mv-net            HTTP client (reqwest + rustls), DNS fallback, Tor proxy, rate limits
│  ├─ mv-sources        manga sources: native scrapers, JS plugin host, repo manager, health
│  ├─ mv-webwall        hidden WebView2 (wry) for Cloudflare-walled sources / cookies
│  ├─ mv-media          Jikan, Cinemeta, Stremio addons, streaming providers, AniSkip, history
│  ├─ mv-torrent        librqbit engine + local HTTP range server for playback
│  ├─ mv-player         libmpv (render API) — playback, subtitles, audio tracks
│  ├─ mv-downloads      chapter / bulk / torrent downloads, storage
│  ├─ mv-ai             providers, agent loop, app tools, Model Hub, MCP bridge
│  ├─ mv-studio         Python pipeline bridge (same JSON protocol), queue
│  └─ mv-ui             design system + every page (GPUI)
└─ python/              unchanged Python pipeline, shipped alongside
```

### Library choices

| Need | Choice | Why |
|---|---|---|
| UI toolkit | **GPUI** + `gpui-component` (fallback: Slint) | GPU-rendered, the toolkit behind Zed; has sidebar, dock, resizable panels, lists, inputs, markdown — the closest to Nexa's look. Confirmed or replaced in the Phase 0 spike. |
| Async runtime | tokio | standard |
| HTTP | reqwest (rustls, streaming, cookies) | replaces fetch / patched fetch |
| SQLite | rusqlite (bundled) | **reads the existing `mangavox.db`** — no data loss |
| HTML scraping | scraper (html5ever) + regex | replaces the TS string scraping |
| JS plugins | rquickjs (QuickJS) with `__fetch` bridged to reqwest | existing `plugins/*/index.js` keep working |
| Cloudflare walls | wry (WebView2) hidden window, cookie hand-off to reqwest | replaces hidden BrowserWindows |
| Torrent streaming | librqbit | Rust BitTorrent with streaming / sequential pieces; replaces WebTorrent |
| Video | libmpv via `libmpv2` render API into a GPUI surface | hardware decode, every codec, subtitles, audio tracks — better than `<video>` |
| Images | `image` + GPUI image cache, disk cover cache | |
| AI | port of the new TS provider layer: reqwest + SSE, tokio::process for CLIs | same 16 providers, same Model Hub |
| Python | tokio::process + existing JSON line protocol | Studio keeps working unchanged |
| Tray / window | GPUI window + `tray-icon` | |
| Packaging | `cargo-packager` (MSI/NSIS, DMG, AppImage) | |

### Rules
- **Same data folder and schema.** The Rust app opens `~/.mangavox` as-is; users switch with no migration step. Schema changes only through new numbered migrations both apps understand during the overlap.
- **Electron stays shippable until parity.** The Rust app grows in `./native`; nothing in `./src` is removed until Phase 9.
- **UI and logic separated.** Crates below `mv-ui` have no UI types, so they are unit-testable and the toolkit could be swapped.
- **One event bus.** Background work (downloads, torrents, AI streams, pipeline) publishes typed events; the UI subscribes.

## 3. Design system (Nexa-style)

- **Shell:** 48px title bar (sidebar toggle, back, breadcrumb; native window buttons on the right). Left sidebar 244px ⇄ 60px icon rail: section switch (Manga / Anime / Movies), the section's pages, Studio group, Assistant + recent chats, Settings + profile pinned to the bottom. Assistant docks left of the content, resizable.
- **Tokens:** one neutral ramp (base, surface 1–3, border, text 1–3), **one accent**, success / warning / error. Themes only swap tokens.
- **Type:** Inter; one scale (11 / 12.5 / 13.5 / 15 / 20 / 28). **Spacing:** 4-pt scale.
- **Icons:** Lucide set, used only where they carry meaning — nav items, primary actions. No decorative icons on toolbars; secondary actions go in a `⋯` menu.
- **Every list / grid** has loading (skeleton), empty, error and populated states.
- **Motion:** 120–180 ms, honours reduce-motion.

## 4. Phases

Each phase ends with a running build, screenshots checked against the design, and tests passing.

| # | Phase | Delivers | Rough size |
|---|---|---|---|
| 0 | **Spike & decide** | Workspace scaffold; GPUI window on this PC with sidebar + title bar + virtualized cover grid (1,000 images) + libmpv playing a local file inside the window. Measure startup, memory, scroll. Decide GPUI vs Slint. | 1 week |
| 1 | **Core** | mv-core (paths, config, SQLite opening the real DB, migrations, logging), mv-net, event bus, CI + tests | 1 week |
| 2 | **Shell & design system** | Title bar, sidebar (collapse, sections), dock framework, tokens + themes, buttons / inputs / menus / toasts / dialogs, Settings frame | 1–2 weeks |
| 3 | **Manga** | Sources (7 native + JS plugin host + repos + health), wall handling via wry, Browse, Library, History, Manga detail, chapter sync, Reader (paged + strip), chapter downloads | 3–4 weeks |
| 4 | **Anime & movies catalog** | Jikan, Cinemeta, addons, providers; Anime home / detail, Movies hub / detail modal, Discover, watchlist, favourites, history shelves | 2–3 weeks |
| 5 | **Playback** | librqbit + range server, direct streams, libmpv player UI (controls, tracks, subtitles search/cache/style, skip intro, next episode), mini-player, resume sessions, torrent + bulk downloads | 3–4 weeks |
| 6 | **Assistant** | Port the AI layer: 16 providers, agent loop, app tools (now Rust functions), approvals, Model Hub (HF search, quants, fit, resumable downloads, Ollama import), AI Providers settings, MCP bridge for CLI agents | 2–3 weeks |
| 7 | **Studio** | Python bridge, Text / OCR, narration voices, Audio, Video, Pipeline, Queue, Dashboard | 2–3 weeks |
| 8 | **Remaining settings & platform** | Profile, playback, storage, extensions, addons, themes, app mode, privacy proxy, tray, YouTube, Subtitles page, lock screen, first-run setup | 1–2 weeks |
| 9 | **Parity & switch** | Feature-parity checklist, performance pass, installers, auto-update, side-by-side beta, then retire Electron | 1–2 weeks |

**Total: roughly 4–6 months of focused work.** Order is chosen so a usable app exists early: after Phase 3 you can read manga in the native app; after Phase 5 you can watch.

## 5. Risks and how they're handled

| Risk | Mitigation |
|---|---|
| GPUI is young (API churn, thin docs, Windows newer than macOS) | Phase 0 spike on this PC decides; UI isolated in `mv-ui`; Slint fallback |
| Cloudflare-walled sites need a real browser | wry/WebView2 hidden window (same approach as today), cookies handed to reqwest |
| Video in a native GPU UI | libmpv render API; fallback: mpv child window parented to the app window |
| Torrent streaming behaviour differs from WebTorrent | librqbit supports streaming; keep the same range-server contract the player uses |
| JS plugins written for Electron's sandbox | QuickJS host exposing the same `__fetch` / helpers; run the existing plugin tests |
| Scope creep / long period with two apps | Strict phase gates, parity checklist, Electron untouched until Phase 9 |
| Data corruption during overlap | Both apps never write the DB at the same time (single-instance lock across apps); backups before migrations |

## 6. How each phase is verified

- `cargo test` for every crate (scraper fixtures, parsers, SQL repos, AI stream parsers ported from `tests/assistant.test.ts`).
- Launch the app on this PC and capture screenshots of every changed screen; compare against the design rules above.
- Parity checklist per page (every button / flow from the Electron page exists or is consciously dropped).
- Startup time, memory and scroll FPS recorded each phase (target: < 1 s cold start, < 250 MB idle).
