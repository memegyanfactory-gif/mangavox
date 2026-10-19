# MangaVox

Native Rust / GPUI desktop app: read manga, watch anime and movies, and turn manga into narrated video (Studio).

## Run

```bash
cd native
cargo build -p mangavox
target\debug\mangavox.exe        # or: npm run dev (from the repo root)
```

`npm run dev` / `npm start` runs `cargo run -p mangavox`; `npm run build` makes a release build; `npm test` runs the Rust workspace tests. No npm dependencies are needed.

## Install and updates

Download `MangaVox-Setup-<version>.exe` from the [latest release](https://github.com/memegyanfactory-gif/mangavox/releases/latest) and run it. It installs per user to `%LOCALAPPDATA%\Programs\MangaVox` (no admin rights), adds Start Menu and Desktop shortcuts, and lists MangaVox under Settings → Apps for uninstalling.

Every push to `main` publishes a new release (`.github/workflows/release.yml`; the patch number goes up each time). The installed app checks GitHub on start and every 30 minutes. When a newer version exists, the badge next to Settings reads **Update to v…**; clicking it checks again and asks whether to download. The update installs in place and MangaVox restarts by itself, with no installer. Settings → About has the same check.

Build the package locally with `pwsh scripts/package.ps1` (writes `dist/MangaVox-<version>-win64.zip` and `dist/MangaVox-Setup-<version>.exe`).

## Layout

| Path | Purpose |
|------|---------|
| `native/app` | the `mangavox` binary (shell, window, tray) |
| `native/crates/mv-*` | core, media, AI, studio, torrent, plugins (no UI) |
| `native/crates/ui-*` | GPUI pages (reader, media, studio, settings, assistant) |
| `python/` | Studio pipeline workers (OCR, narration, TTS) run by `mv-studio` |
| `docs/` | design notes and debugging guide |

## Modes

Entertainment mode is the default. Turn on **Studio mode** in Settings > Mode to show the Studio and Assistant workspace.

## Data and logs

User data lives in `~/.mangavox` (`config.json`, library, caches, `logs/`).

---

Made by Aayush Datta.
