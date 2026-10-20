# Debugging MangaVox (native)

- Build: `cd native && cargo build -p mangavox`; run `native\target\debug\mangavox.exe` or `npm run dev`.
- Logs: JSON lines in `~/.mangavox/logs/` (`main.log`, rotated at ~5 MB, 3 backups). Override with `MANGAVOX_LOG_DIR`, level with `MANGAVOX_LOG_LEVEL` (debug builds default to debug).
- Studio Python workers: set `MANGAVOX_PYTHON_DIR` to point at `python/` if it is not found next to the exe or cwd.
- Check: `cd native && cargo check -p mangavox`; tests: `cargo test --workspace`.
- Kill only your own instance when several are running.

## Testing the welcome setup (first run)

- When it shows: `mv_core::onboarding` decides once per start, before anything writes to the data folder. A folder with no MangaVox data (no `mangavox.db`, `config.json`, plugins, addons, voice models, repos.json or library files) is a fresh install. A folder with data is recorded as done (`config.json` → `onboarding.existing: true`), so existing installs never see the setup. A setup left part-way resumes at its screen (`onboarding.status: "in_progress"`, `onboarding.step`).
- Try it without touching your real data: point a second instance at an empty folder. PowerShell: `$env:MANGAVOX_HOME="D:\tmp\mv-fresh"; native\target\debug\mangavox.exe`. cmd: `set MANGAVOX_HOME=D:\tmp\mv-fresh` then run the exe. It can run next to your normal copy; close only that window when done. Delete the folder to start fresh again.
- Force or hide it: `MANGAVOX_WELCOME=1` or `mangavox.exe --welcome` shows it at every start, whatever the folder holds. A repeated setup changes nothing until a screen is confirmed, and its "Close setup" leaves everything as it was. `MANGAVOX_WELCOME=0` never shows it.
- Run it again from the app: Settings › About › "Run welcome setup again".
- Reset the decision: remove the `onboarding` key from `<data folder>\config.json` (or the whole file in a test folder).
- What a fresh setup writes: on "Set up MangaVox" or "Start empty", `disabledSources` gets every built-in source id (so Browse shows nothing until a source is switched on); "Start empty" also writes `app.mode: "entertainment"` and `onboarding.status: "skipped"`. Finishing writes `onboarding.status: "done"`, `uses` and the per-screen results. The Anime and Movies catalogs are not prefetched while the setup is open, and not at later starts when the setup left out Anime and films.
- Voice downloads started in the setup run on the background runtime (`mv_studio::voice_setup`), keep going after the setup closes, and show in Settings › Audio. A killed app leaves `.part` files that the next download resumes.
- Tests: `cargo test -p mv-core onboarding` (fresh, existing, resume, forced, suppressed), `cargo test -p ui-welcome` (screen flow, sizes), `cargo test -p ui-manga` (clean-start seeding, repository filters).
