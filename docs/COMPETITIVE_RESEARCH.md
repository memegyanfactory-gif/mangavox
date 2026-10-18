# MangaVox Competitive Research

Date: 2026-10-18. Method: fetched each project's GitHub README (URLs below) and extracted the feature lists as written there. Cells marked "-" mean the README did not mention the feature. That does not prove the app lacks it. Recommendations in the second half are my own judgment, not facts from the READMEs. I did not audit MangaVox's current code, so check each item against what already exists before building.

## Sources actually visited

- Mihon: https://github.com/mihonapp/mihon
- Kotatsu (archived, project shut down per README): https://github.com/KotatsuApp/Kotatsu
- Suwayomi-Server: https://github.com/Suwayomi/Suwayomi-Server
- Komga: https://github.com/gotson/komga
- Kavita: https://github.com/Kareadita/Kavita
- Mangayomi: https://github.com/kodjodevf/mangayomi
- Seanime: https://github.com/5rahim/seanime
- Aniyomi: https://github.com/aniyomiorg/aniyomi
- Jellyfin: https://github.com/jellyfin/jellyfin
- Stremio Web: https://github.com/Stremio/stremio-web
- Miru: https://github.com/miru-project/miru-app
- Open WebUI: https://github.com/open-webui/open-webui
- LobeChat / LobeHub: https://github.com/lobehub/lobe-chat

## Per-app notes (from the READMEs)

- **Mihon**: configurable reader (several viewers, reading directions); library categories; trackers (MAL, AniList, Kitsu, MangaUpdates, Shikimori, Bangumi, others); scheduled library updates for new chapters; local or cloud backups; local-file reading.
- **Kotatsu**: 1,200+ sources; favorites categories; history, bookmarks and incognito mode; offline downloads and CBZ import; webtoon-optimised reader with gestures; new-chapter notifications, updates feed and recommendations; app lock (password or fingerprint); cross-device sync. Archived, so it is a reference only.
- **Suwayomi**: headless server that runs Mihon extensions; auto-check and auto-download of new chapters; automated Mihon-compatible backups; FlareSolverr for Cloudflare-protected sites; OPDS and OPDS-PSE; auto-updating web UI.
- **Komga**: multi-user permissions with age and label restrictions; collections and read lists; metadata editing and embedded-metadata import; duplicate file and page detection; REST API; OPDS v1 and v2; Kobo and KOReader sync; download of book, series or read list.
- **Kavita**: webtoon mode and continuous reading across chapters; customizable home dashboard and side nav driven by smart filters; "Want to Read" list; reading lists with CBL import; custom themes; scrobbling and ratings (Kavita+); EPUB annotations; Weblate localization; OIDC.
- **Mangayomi**: one app for manga, webtoons, novels, anime and movies; trackers incl. SIMKL and Trakt; backups; runs on Android, iOS, macOS, Linux and Windows.
- **Seanime**: desktop client with embedded libmpv player (ASS subtitles, shaders); AniList browse and manage; local library scan with no strict naming rules; torrent search and clients/debrid; stream torrents before the download finishes; auto-downloader with filters; extension marketplace; manga reader; transcoding or direct play; playlists; release schedule; Discord Rich Presence; offline mode; theming.
- **Aniyomi**: combined reader and mpv-based player; trackers; library categories; scheduled updates; backups.
- **Jellyfin**: first-run setup wizard; library and metadata providers; documented REST API with Swagger; no paid tiers.
- **Stremio Web**: add-on-driven catalogs; Continue Watching synced by account; Chromecast; subtitle add-ons with adjustable styling; full keyboard control of the player; 50+ languages; PWA.
- **Miru**: JavaScript extensions with debug logging and custom repositories; TMDB metadata; AniList tracking; HTTP/SOCKS proxy support; video, comics and novels in one app.
- **Open WebUI**: any OpenAI-compatible API plus Ollama; tools, MCP and OpenAPI tool servers; custom agents (instructions, tools, knowledge); persistent memory; local RAG with web search; voice calls with several STT/TTS providers; multi-model parallel chat; image generation; usage analytics; notes workspace; scheduled prompts.
- **LobeChat/LobeHub**: Agent Builder that creates an agent from a description; 10,000+ tools and MCP plugins; agent groups with shared context; structured, editable personal memory; multi-provider model config; TTS/STT hooks; plugin system for function calling.

## Feature matrix (Y = listed in README, - = not mentioned)

| Feature | Mihon | Kotatsu | Suwayomi | Komga | Kavita | Mangayomi | Seanime | Aniyomi | Jellyfin | Stremio | Miru | OpenWebUI | LobeChat |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Configurable reader modes/directions | Y | Y | - | Y | Y | Y | Y | Y | - | - | - | - | - |
| Webtoon / continuous scroll | - | Y | - | - | Y | - | - | - | - | - | - | - | - |
| Library categories | Y | Y | Y | Y (collections) | Y | Y | - | Y | - | - | - | - | - |
| Reading lists / Want-to-Read | - | - | - | Y | Y | - | - | - | - | - | - | - | - |
| Progress tracker sync (AniList/MAL) | Y | Y | Y | - | Y (scrobble) | Y | Y | Y | - | - | Y | - | - |
| Scheduled new-chapter checks | Y | Y | Y | - | - | - | Y | Y | - | - | - | - | - |
| Auto-download new items | - | - | Y | - | - | - | Y | - | - | - | - | - | - |
| Offline downloads | Y | Y | Y | Y | - | - | Y | - | - | - | - | - | - |
| Backup / restore | Y | - | Y | - | - | Y | - | Y | - | - | - | - | - |
| Extension / plugin system | - | - | Y | - | - | - | Y | - | - | Y (add-ons) | Y (JS) | Y | Y |
| Metadata editing / providers | - | - | - | Y | Y | - | Y | - | Y | Y | Y (TMDB) | - | - |
| Duplicate detection | - | - | - | Y | - | - | - | - | - | - | - | - | - |
| Multi-user / permissions | - | - | - | Y | Y | - | - | - | - | - | - | Y | - |
| Customizable home / dashboard | - | - | - | - | Y | - | Y | - | - | Y (Continue Watching) | - | - | - |
| Themes | Y | Y | - | - | Y | Y | Y | Y | - | - | - | - | - |
| Embedded video player w/ ASS subs | - | - | - | - | - | - | Y | Y (mpv) | - | Y (styled subs) | - | - | - |
| Keyboard-complete UX | - | - | - | - | - | - | - | - | - | Y | - | - | - |
| App lock / privacy (incognito) | - | Y | - | - | - | - | - | - | - | - | - | - | - |
| OPDS / external API | - | - | Y | Y | - | - | - | - | Y (API) | - | - | Y | - |
| Cloudflare bypass | - | - | Y | - | - | - | - | - | - | - | - | - | - |
| Proxy support | - | - | - | - | - | - | - | - | - | - | Y | - | - |
| Cross-device sync | - | Y | - | Y (Kobo/KOReader) | - | - | - | - | - | Y | - | - | - |
| First-run setup wizard | - | - | - | - | - | - | - | - | Y | - | - | - | - |
| Discord Rich Presence | - | - | - | - | - | - | Y | - | - | - | - | - | - |
| i18n / many languages | - | - | - | - | Y | - | - | - | - | Y (50+) | - | Y | - |
| AI chat with tools / MCP | - | - | - | - | - | - | - | - | - | - | - | Y | Y |
| Persistent / editable memory | - | - | - | - | - | - | - | - | - | - | - | Y | Y |
| RAG / web search | - | - | - | - | - | - | - | - | - | - | - | Y | - |
| Voice (TTS/STT) | - | - | - | - | - | - | - | - | - | - | - | Y | Y (hooks) |
| Multi-model compare | - | - | - | - | - | - | - | - | - | - | - | Y | - |
| Usage analytics | - | - | - | - | - | - | - | - | - | - | - | Y | - |
| Agent builder / groups | - | - | - | - | - | - | - | - | - | - | - | Y (custom agents) | Y |
| Scheduled AI tasks | - | - | - | - | - | - | - | - | - | - | - | Y | Y |
| Manga-to-audio narration | - | - | - | - | - | - | - | - | - | - | - | - | - |

Key observation: none of the 13 READMEs mention manga-to-audio narration. That is MangaVox's differentiator, so improvements should make narration feel native to the reader and library rather than a separate tool.

## Top 15 recommendations (ranked by impact / effort)

Effort: S = days, M = 1-2 weeks, L = more. Check each against current MangaVox code first (the repo already has download manager, deep research, token analytics and a narration wizard per recent commits).

1. **Library categories plus Continue Reading/Listening home row** (Impact high, S). Seen in Mihon, Kotatsu, Kavita, Stremio. Hint: add a `category` table and a home strip query ordered by last-read/last-played timestamp across both reading and narration progress.
2. **Unified read-and-listen progress** (high, M). Differentiator, nobody else has it. Hint: store one position per chapter (page index plus audio timestamp) and map between them with per-page audio offsets so the user can switch modes mid-chapter.
3. **Keyboard-complete reader and player** (high, S). Stremio's player is fully keyboard-operable. Hint: central hotkey map (space play/pause narration, arrows page, `[`/`]` chapter, `+`/`-` speed) shown in a help overlay.
4. **Webtoon / continuous-scroll mode with cross-chapter flow** (high, M). Kavita and Kotatsu list it. Hint: virtualized vertical list that appends the next chapter near the end, with narration highlighting the active panel.
5. **Scheduled new-chapter checks plus auto-download and auto-narrate** (high, M). Mihon, Suwayomi and Seanime do check/auto-download. Hint: a background job per library entry that queues download then the TTS pipeline, with an updates feed.
6. **AniList/MAL tracker sync** (high, M). Nearly every manga/anime app has it. Hint: OAuth for AniList first (GraphQL), push progress on chapter completion, including chapters finished by listening.
7. **Persistent, editable assistant memory** (high, M). Open WebUI and LobeChat both offer it. Hint: a memory table of short facts, shown in a viewer where the user can edit or delete entries, injected into the system prompt (MangaVox already has a MemoryViewer component, so extend it).
8. **Assistant tool calling over the library (MCP-style)** (high, M). Open WebUI and LobeChat expose tools/MCP. Hint: expose tools such as `search_library`, `queue_narration`, `get_progress`, `change_voice` so the assistant acts instead of just chatting.
9. **Backup/restore plus scheduled auto-backup** (medium, S). Mihon, Suwayomi, Mangayomi. Hint: export DB plus settings as a single zip on a timer, with an optional user-chosen cloud-synced folder.
10. **Extension/source plugin system with debug logging** (high, L). Miru, Seanime, Suwayomi, Stremio. Hint: sandboxed JS source plugins with manifest, custom repo URLs and a console log panel. Large effort, so scope a minimal version.
11. **Subtitle/caption styling and in-player quality** (medium, S-M). Seanime (libmpv, ASS) and Stremio (adjustable subtitle styling). Hint: expose font/size/outline controls and remember them per user; reuse for narration captions.
12. **First-run setup wizard with dependency checks** (medium, S). Jellyfin has one; MangaVox has a FirstRunSetup component. Hint: add checks for Python/TTS models, GPU, API keys and library folder, with a "fix it" button for each.
13. **Duplicate detection and metadata editing** (medium, M). Komga. Hint: hash chapter folders and page images to flag duplicates; allow editing title/author/cover and persist overrides over provider data.
14. **Cloudflare/proxy handling for sources** (medium, S-M). Suwayomi supports FlareSolverr; Miru supports HTTP/SOCKS proxies. Hint: per-source proxy setting and an optional FlareSolverr URL field in settings.
15. **OPDS server, Discord Rich Presence, app lock** (low-medium, S each). Suwayomi/Komga (OPDS), Seanime (Rich Presence), Kotatsu (lock/incognito). Hint: pick Rich Presence ("Narrating Chapter X") and incognito mode first because they are cheapest.

## Caveats

- README feature lists are marketing summaries; a "-" in the matrix means "not mentioned". Some apps surely have features not listed.
- Kotatsu is archived, so treat it as a UX reference, not a live competitor.
- Open WebUI and LobeChat are server/web products; borrow patterns (memory, tools, voice), not architecture.
