# Debugging MangaVox (native)

- Build: `cd native && cargo build -p mangavox`; run `native\target\debug\mangavox.exe` or `npm run dev`.
- Logs: JSON lines in `~/.mangavox/logs/` (`main.log`, rotated at ~5 MB, 3 backups). Override with `MANGAVOX_LOG_DIR`, level with `MANGAVOX_LOG_LEVEL` (debug builds default to debug).
- Studio Python workers: set `MANGAVOX_PYTHON_DIR` to point at `python/` if it is not found next to the exe or cwd.
- Check: `cd native && cargo check -p mangavox`; tests: `cargo test --workspace`.
- Kill only your own instance when several are running.
