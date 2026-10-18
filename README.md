# MangaVox

Manga-to-Voice Platform — browse, read, and convert manga to audio/video. All processing runs locally.

MangaVox is a self-hosted, offline-first macOS desktop app that lets you browse manga from plugin-based sources, read it in a built-in reader, and run an automated AI pipeline converting manga pages into audio narration with synced playback — all on your machine.

---

## Features

### Browse & Discover
- Search manga across multiple sources simultaneously
- Browse popular, latest, and by genre/tag
- Cross-source discovery — finds the same manga on all your installed sources
- Source tabs showing chapter counts per source (like Tachiyomi)

### Library Management
- Add manga to a personal library with grid or list view
- Favorites, notes, reading progress tracking
- Filter by title, author, genre; sort by title, date added, last read
- Reading history with timestamps

### Manga Reader
- Three reading modes: LTR page flip, RTL page flip, vertical scroll
- Image fit options: fit width, fit height, original size
- Keyboard navigation (arrow keys, spacebar, escape)
- Click zones (left/center/right) for page turning
- Auto-saves reading progress per chapter
- Auto-marks chapters as read
- Previous/Next chapter navigation
- Page slider for quick navigation

### Download Manager
- Queue-based concurrent downloads (max 3 simultaneous)
- Batch download: next 5, 10, 20, or all chapters
- Per-chapter progress tracking
- Downloaded chapters readable offline via local file paths

### AI Pipeline (OCR → TTS → Sync)
- **OCR**: Extract text from manga pages using PaddleOCR (multi-language) or manga-ocr (Japanese)
- **Text Cleanup**: Remove SFX artifacts, fix reading order, merge fragmented text
- **TTS**: Generate audio narration via AURA VOICE integration (Kokoro, Chatterbox, XTTS v2, VCTK engines)
- **Sync Map**: Create page-to-audio timing map for synchronized playback
- Real-time progress bars in the Queue page
- Cancel running jobs

### Plugin System
- Sandboxed JavaScript plugins running in V8 VM contexts
- Built-in MangaDex plugin (MangaDex API v5)
- Tachiyomi-compatible extension repository support
- Install extensions from repos or direct URLs
- Auto-generated scraper plugins for WordPress/Madara-based manga sites
- Plugins only get network access via a proxied `fetch()` — no filesystem, no require

### Settings
- Manage installed sources (add, remove, view)
- Browse and install from Tachiyomi extension repos
- Configure pipeline OCR engine, TTS engine, and voice

---

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Framework | Electron 28 |
| Frontend | React 19 + TypeScript 5 |
| Styling | Tailwind CSS 4 |
| State | Zustand 5 |
| Build | electron-vite + Vite 7 |
| Database | SQLite via better-sqlite3 |
| AI/ML | Python (PaddleOCR, manga-ocr, OpenCV) |
| TTS | AURA VOICE (Kokoro, Chatterbox, XTTS v2, VCTK) |
| IDs | UUID v4 |

---

## Project Structure

```
MANGA VOX/
├── src/
│   ├── main/                       # Electron main process
│   │   ├── index.ts               # App lifecycle, window creation
│   │   ├── db/
│   │   │   ├── database.ts        # SQLite init, WAL, migrations
│   │   │   ├── manga.repo.ts      # Manga CRUD
│   │   │   ├── chapter.repo.ts    # Chapter CRUD + status tracking
│   │   │   ├── pipeline.repo.ts   # Pipeline job management
│   │   │   └── history.repo.ts    # Reading history
│   │   ├── ipc/
│   │   │   ├── library.ipc.ts     # Library + chapter handlers
│   │   │   ├── plugin.ipc.ts      # Plugin/source handlers
│   │   │   ├── download.ipc.ts    # Download + reader handlers
│   │   │   ├── pipeline.ipc.ts    # Pipeline job handlers
│   │   │   ├── repo.ipc.ts        # Extension repo handlers
│   │   │   └── index.ts           # Register all handlers
│   │   ├── plugins/
│   │   │   ├── plugin-loader.ts   # Scan & load plugins
│   │   │   ├── plugin-sandbox.ts  # VM sandbox execution
│   │   │   └── repo-manager.ts    # Tachiyomi repo support
│   │   ├── pipeline/
│   │   │   ├── orchestrator.ts    # Job state machine + progress
│   │   │   └── python-bridge.ts   # Python subprocess management
│   │   ├── download/
│   │   │   └── download-manager.ts # Concurrent download queue
│   │   └── services/
│   │       └── app-paths.ts       # ~/.mangavox/ path management
│   ├── renderer/                   # React frontend
│   │   ├── App.tsx                # Layout + page router
│   │   ├── pages/
│   │   │   ├── Browse.tsx         # Search & discover manga
│   │   │   ├── Library.tsx        # Personal manga library
│   │   │   ├── MangaDetail.tsx    # Manga info + chapter list
│   │   │   ├── Reader.tsx         # Full-screen manga reader
│   │   │   ├── Queue.tsx          # Pipeline job queue
│   │   │   ├── History.tsx        # Reading history
│   │   │   ├── Player.tsx         # Synced audio player (WIP)
│   │   │   ├── Dashboard.tsx      # Stats overview (WIP)
│   │   │   └── Settings.tsx       # Sources, extensions, config
│   │   ├── stores/
│   │   │   ├── ui.store.ts        # Navigation + page history
│   │   │   ├── library.store.ts   # Manga/chapter state
│   │   │   └── reader.store.ts    # Reader mode, fit, progress
│   │   ├── hooks/
│   │   │   └── useIpc.ts          # Typed IPC invoke + events
│   │   ├── components/layout/
│   │   │   └── Sidebar.tsx        # Navigation sidebar
│   │   └── utils/
│   │       └── cover-url.ts       # Cover image URL helpers
│   ├── preload/
│   │   └── index.ts               # Context bridge (IPC API)
│   └── shared/
│       ├── ipc-channels.ts        # All IPC channel constants
│       ├── types.ts               # Shared TypeScript types
│       └── plugin-interface.ts    # MangaSourcePlugin interface
├── plugins/
│   └── mangadex/                   # Built-in MangaDex plugin
│       ├── manifest.json
│       └── index.js
├── python/                         # AI Pipeline
│   ├── requirements.txt
│   ├── pipeline_worker.py         # Main entry (JSON job → stages)
│   ├── ocr_engine.py              # PaddleOCR / manga-ocr wrapper
│   ├── text_cleanup.py            # OCR text post-processing
│   ├── tts_bridge.py              # AURA VOICE subprocess bridge
│   └── sync_map.py                # Page-to-audio timing map
├── .vscode/
│   ├── launch.json                # F5 to run dev mode
│   └── tasks.json                 # Build/typecheck tasks
├── electron.vite.config.ts
├── tsconfig.json
└── package.json
```

---

## Getting Started

### Prerequisites
- Node.js 18+
- npm
- Python 3.10+ (for AI pipeline)
- FFmpeg (for future video generation)

### Install & Run

```bash
# Install dependencies
npm install

# Rebuild native modules for Electron
npx electron-rebuild

# Run in development mode
npm run dev
```

### VS Code
Press **F5** to launch the app in dev mode (uses the included launch.json config).

### Build for Production

```bash
npm run build       # Build all (main + preload + renderer)
npm run package     # Create macOS .dmg distribution
```

### Python Pipeline Setup

```bash
cd python
pip install -r requirements.txt
```

---

## App Data

All user data is stored locally at `~/.mangavox/`:

```
~/.mangavox/
├── mangavox.db          # SQLite database
├── library/             # Downloaded chapter pages
│   └── {manga_id}/
│       └── {chapter_id}/
│           ├── 0001.jpg
│           ├── 0002.jpg
│           └── ...
├── plugins/             # User-installed plugins
├── cache/covers/        # Cached cover images
└── repos.json           # Tachiyomi extension repos
```

---

## Database Schema

SQLite with WAL mode and foreign keys. Migrations run automatically on startup.

**Tables**: `manga`, `chapter`, `pipeline_job`, `reading_history`, `_migrations`

Key relationships:
- `chapter.manga_id` → `manga.id` (CASCADE delete)
- `chapter.source_plugin` — tracks which plugin created each chapter
- `pipeline_job.chapter_id` → `chapter.id` (CASCADE delete)
- `reading_history.chapter_id` → `chapter.id` (CASCADE delete)

---

## Plugin Development

Plugins are sandboxed JavaScript files that call `registerSource({...})` with the MangaSourcePlugin interface:

```javascript
registerSource({
  searchManga: async function(query, page) {
    var resp = await __fetch('https://api.example.com/search?q=' + query)
    // ... parse response ...
    return [{ source_id, title, cover_url, author, status }]
  },

  getMangaDetails: async function(mangaId) {
    // Return { source_id, title, author, artist, description, cover_url, status, genres }
  },

  getChapterList: async function(mangaId) {
    // Return [{ source_id, number, title, language, page_count }]
  },

  getPageUrls: async function(chapterId) {
    // Return ['https://cdn.example.com/page1.jpg', ...]
  },

  // Optional browsing methods:
  getPopularManga: async function(page) { ... },
  getLatestManga: async function(page) { ... },
  getTagList: async function() { ... },
  getMangaByTag: async function(tagId, page) { ... },
})
```

**Available in sandbox**: `__fetch()`, `console`, `JSON`, `Promise`, `URL`, `URLSearchParams`, `Array`, `Object`, `String`, `Number`, `Boolean`, `Date`, `RegExp`, `Map`, `Set`, `Error`, `parseInt`, `parseFloat`, `encodeURIComponent`, `decodeURIComponent`

**NOT available**: `require`, `process`, `fs`, `child_process`, `eval`, `Function`, DOM APIs

---

## AI Pipeline Protocol

The Python pipeline worker communicates with Electron via JSON lines on stdout:

```json
{"type": "progress", "stage": "ocr", "current": 3, "total": 24, "percent": 12.5}
{"type": "stage_complete", "stage": "ocr", "data": {"text_count": 150}}
{"type": "progress", "stage": "tts", "current": 1, "total": 24, "percent": 4.2}
{"type": "stage_complete", "stage": "tts", "data": {"audio_files": 24}}
{"type": "done", "outputs": {"audio_dir": "/path", "sync_map": "/path/sync_map.json"}}
{"type": "error", "stage": "tts", "message": "Model load failed", "recoverable": false}
```

### AURA VOICE Integration

TTS is powered by AURA VOICE, called as a subprocess:

```python
python3 "/path/to/TTS software/tts_worker.py" <args.json>
# args.json: { engine, text, voice, output_path, speed, exaggeration, reference_wav }
# Returns: "done:/path/to/output.wav"
```

Available engines: Kokoro (fast English), Chatterbox (voice cloning), XTTS v2 (multilingual), VCTK (109 speakers)

---

## Scripts

| Script | Description |
|--------|------------|
| `npm run dev` | Start dev mode with hot reload |
| `npm run build` | Production build |
| `npm run preview` | Preview production build |
| `npm run package` | Create macOS .app/.dmg |
| `npm run typecheck` | TypeScript type checking |

---

## Architecture Decisions

- **Electron over Tauri**: Python subprocess management is central; Electron's Node.js gives direct `child_process.spawn` with stream control
- **Zustand over Redux**: 3 independent store domains (library, reader, UI) — Zustand's store-per-domain is cleaner
- **VM sandbox over isolated-vm**: Lighter weight for the plugin use case; plugins only need proxied fetch
- **SQLite with better-sqlite3**: Synchronous API, single-user desktop app, no ORM overhead
- **JSON-lines protocol**: Same pattern as AURA VOICE's tts_worker.py — proven and debuggable
- **Per-chapter source tracking**: Each chapter records which plugin created it, preventing source mismatch bugs

---

## License

ISC
