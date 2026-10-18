# Debugging MangaVox

Every failure should leave a readable trace, and you should be able to drive and inspect the running app from a shell. This applies to people and to AI coding agents.

## TL;DR

```bash
npm run dev                                  # dev app; CDP on 127.0.0.1:9222 (MANGAVOX_CDP_PORT=<port>|off)
node scripts/mv.js status                    # is it up? current page, recent errors, main pid
node scripts/mv.js errors                    # renderer-captured errors + error lines from all logs
node scripts/mv.js logs ipc --level error    # failing IPC calls with args + stacks
node scripts/mv.js nav settings              # go to a page
node scripts/mv.js invoke source:health      # call any IPC channel
node scripts/mv.js shot out.png --width 360 --height 740   # phone-size screenshot
```

npm shortcuts: `npm run debug:status`, `npm run debug:logs -- ipc -n 20`, `npm run test:sources`.

## Running

| What | How |
|---|---|
| Dev app | `npm run dev` (runs `scripts/dev.js`, which clears `ELECTRON_RUN_AS_NODE` and runs `electron-vite dev`) |
| CDP port | Dev builds call `app.commandLine.appendSwitch('remote-debugging-port', …)` in `src/main/services/debug-bootstrap.ts`. The default is **9222**. Override it with `MANGAVOX_CDP_PORT=9361` or turn it off with `MANGAVOX_CDP_PORT=off`. A `--remote-debugging-port` already on the command line always wins. Packaged builds never open CDP. |
| Port busy? | Chromium fails silently when the port is taken by another instance. Pick another port, e.g. PowerShell `$env:MANGAVOX_CDP_PORT=9361; npm run dev`, then use `MANGAVOX_CDP_PORT=9361 node scripts/mv.js …` or `--port 9361`. |
| Renderer URL | Dev: `http://localhost:517x/` (Vite picks the next free port). Packaged: `file://…/index.html`. |
| Main-process changes | electron-vite rebuilds `out/main`. If the main process did not restart, kill **your own** instance and start it again. Renderer changes hot-reload. |
| Several instances | They all share `~/.mangavox` (DB, config, logs). Every log record carries `pid`. `mv status` prints `mainPid`, and `mv logs --pid N` filters on it. Ports 9876/9878 (stream servers) can only be bound by one instance, so `EADDRINUSE` on those is expected when a second instance runs. |

On Windows, dev builds also disable native window-occlusion tracking. Without that, a window covered by other windows counts as hidden: animations and rAF pause, and CDP screenshots hang. `mv shot` also falls back to main-process `capturePage` (dev IPC `debug:capture`), which briefly shows a hidden or minimised window without focusing it.

## `scripts/mv.js` (CDP command-line tool)

It auto-detects the app's page target (URL contains `localhost:517` or `index.html`). Hidden scraper windows (mangafire, nhentai…) are ignored. The port comes from `--port`, then `MANGAVOX_CDP_PORT`, then 9222. Exit codes: `0` ok, `1` command or JS error, `2` app not reachable.

| Command | Does |
|---|---|
| `status` | Reports up/down, Chromium version, main pid and uptime, page + pageParams, renderer error count and the last 5, and the last 5 log errors from that pid |
| `eval "<js>"` | Runs `Runtime.evaluate` in the renderer, awaits promises, and prints JSON. Example: `mv eval "__mv.state().ui"` |
| `nav <page> [paramsJson]` | Calls `__mv.navigate(page, params)` and prints the resulting page and any errors raised in the next 1.5 s |
| `invoke <channel> [argsJson]` | Calls `window.mangavox.invoke(channel, ...args)`. `argsJson` is a JSON array, or a single value. Prints `{ok, ms, result \| error}` |
| `shot [file] [--width W --height H] [--wait ms] [--no-show]` | Saves a PNG. With a size it uses `Emulation.setDeviceMetricsOverride`, then clears it. The default file is in the OS temp dir |
| `errors [-n 20]` | Lists renderer in-memory errors (`__mv.errors()`) and recent `error` lines from all three log files. The log part works even when the app is down |
| `logs [main\|renderer\|ipc\|all] [-n 50] [--level warn] [--scope x] [--grep re] [--pid N] [--json] [--full] [-f]` | Pretty-prints JSON-lines logs straight from disk. `-f` follows |
| `diag` | Prints the full diagnostics snapshot (`debug:snapshot`) |
| `sources [id,id]` | Runs the source harness `scripts/test-sources.js` against the running app |
| `reload` | Runs `Page.reload` and waits until `#root` renders again |
| `targets` | Lists all CDP targets |

Output longer than 20k characters is truncated. Pass `--full` to get all of it.

### `window.__mv` (dev renderer only)

The object is defined in `src/renderer/debug/debug-bridge.ts`. You can use it from DevTools, `mv eval`, or any CDP script.

```js
__mv.navigate('manga-detail', { mangaId })   // ui.store navigate
__mv.state()        // { ui, library, appearance } — JSON-safe, long arrays cut to 25
__mv.invoke('library:list')
__mv.errors()       // last 100 captured renderer errors {t, level, kind, message, stack, page, …}
__mv.clearErrors()
__mv.snapshot()     // = invoke('debug:snapshot')
__mv.stores.ui.setState({...})   // raw zustand stores (ui, library, appearance)
```

## Logs

The files live in `~/.mangavox/logs/` (`MANGAVOX_LOG_DIR` overrides). Each one rotates at about 5 MB and keeps `.1` to `.3`.

| File | Contents |
|---|---|
| `main.log` | Every main-process `console.*` call. The scope is taken from a leading `[tag]`, e.g. `console.log('[addon] Loaded…')` gives scope `addon`. Also boot records, `uncaughtException`, `unhandledRejection`, `render-process-gone`, `child-process-gone`, window `unresponsive`, `did-fail-load` and `preload-error` |
| `renderer.log` | `window.onerror`, `unhandledrejection`, `console.error/warn`, and React `ErrorBoundary` errors (with `componentStack`). Each record carries `page`, `pageParams` and `url` |
| `ipc.log` | Every `ipcMain.handle` call: channel, duration, a redacted args summary. `debug` on success, `warn` for **SLOW** (over 3 s), `error` with stack on failure |

Format: one JSON object per line.

```json
{"t":"2026-10-17T22:24:43.665Z","lvl":"error","scope":"source:test","msg":"ERROR sourceId is required","pid":16788,"ch":"source:test","ms":4,"args":"[]","err":"sourceId is required","stack":"Error: …"}
```

- Levels are `debug < info < warn < error`. Dev writes `debug` and above, packaged writes `info` and above. Override with `MANGAVOX_LOG_LEVEL`.
- Redaction: keys like `apiKey`, `token`, `password`, `secret`, `cookie` and `authorization` become `[REDACTED]`. So do `sk-…` keys, `Bearer …` values and `?key=` query params. Numeric settings such as `token_budget` stay visible.
- In dev, `main` info+ and `ipc`/`renderer` warn+ records are mirrored to the terminal as `[file:scope] msg`.
- New main code can use `logger('scope').info(msg, extra)` from `src/main/services/logger.ts`. The older `log/logWarn/logError` still work.
- Renderer forwarding drops repeated identical errors into a `count`, and sends at most 30 per 10 s.

Raw tools still work too: `tail -f ~/.mangavox/logs/ipc.log | jq .`

## Diagnostics bundle

**Settings → About → Copy diagnostics** copies the `debug:snapshot` output plus renderer context. The snapshot contains app/OS/Electron/Chromium versions, memory, log paths, CDP port, `config.json` (redacted), a source-health summary (counts plus non-ok sources, from the cache with no network calls), the last 50 in-memory warnings, and the last 200 `error` lines across all logs. The renderer context adds page, params, user agent, viewport and the last 30 captured errors. From a shell, use `mv diag` or `mv invoke debug:snapshot '[{"errors":500,"minLevel":"warn"}]'`.

## Reproduce → fix → verify loop

1. `mv status` and `mv errors`: find the failure and the page it happened on.
2. `mv logs ipc --level error -n 20`: if a handler threw, you get the channel, args and stack. Re-run it in isolation with `mv invoke <channel> '<args from the log>'`.
3. Reproduce the UI path with `mv nav <page> '<params>'` plus `mv eval` (click with `document.querySelector(...).click()`), then `mv shot`.
4. Fix it. Renderer changes hot-reload. Main changes need a restart if electron-vite didn't restart it.
5. Verify: run `mv eval "__mv.clearErrors()"`, repeat step 3, and check that `mv errors` and `mv logs ipc --level error -n 5 --pid <mainPid>` show nothing new. Take a `mv shot` for visual changes. Check narrow layouts with `--width 360 --height 740`.
6. `npx tsc --noEmit` (and `npm test` when touching tested modules).

## Source harness

`node scripts/mv.js sources [mangadex,comick]` (or `npm run test:sources`, env `SOURCES=…`, `CDP_PORT`/`MANGAVOX_CDP_PORT`) runs these checks through IPC in the running app: popular, latest, search, tags, details, chapters and pages. Each step has a 40 s timeout. It prints `step:OK(n)` or `step:FAIL[reason]` for every source. The in-app equivalents are `source:health` (light, cached for 10 min) and `source:test <id>` (full flow). Dev-only `source:debug` (`{op:'fetch'|'render'|'headers'|'solve'|'sandbox', url}`) exercises the anti-bot fetch layer: `mv invoke source:debug '[{"op":"fetch","url":"https://…"}]'`.

## Common failure signatures

| Signature | Meaning / where to look |
|---|---|
| `Error invoking remote method 'x': Error: …` (renderer) | The handler threw. `mv logs ipc --grep x` has the args and the main-side stack |
| `ipc … SLOW 12.3s` | The handler blocked for more than 3 s: network timeout, sync FS, or a Python/ffmpeg child |
| `Too Many Requests (429)` / `Response was an unexpected content type: text/html` | The source is rate-limiting or returned an HTML wall. Several instances hitting MangaDex at once makes this worse |
| `wall` / `verify` status, `Just a moment…` titles | Cloudflare or anti-bot challenge. Use Settings → Sources → Verify, or `source:verify-open` |
| `net::ERR_HTTP2_PROTOCOL_ERROR`, `ERR_NAME_NOT_RESOLVED`, `ENOTFOUND` | Network or DNS (the dev network often blocks some hosts). Not a code bug unless it's consistent |
| `listen EADDRINUSE 127.0.0.1:9878` | Another instance owns the stream server. Harmless in parallel dev |
| `render-process-gone: crashed/oom` (scope `crash`) | The renderer died. The window auto-reloads. Check `renderer.log` just before it |
| `window unresponsive` | A long sync task in the renderer. Check what `ipc.log` was doing |
| `renderer:boundary …` with `componentStack` | A React render error contained by `ErrorBoundary`. The stack names the component |
| `did-fail-load -102/-105/-106` | The page or scraper window failed to load (connection refused, DNS, offline) |
| `preload-error` | The preload script threw. `window.mangavox` will be missing |

## Key files

- `src/main/services/debug-bootstrap.ts`: imported first by `src/main/index.ts`. Console capture, crash handlers, IPC tracing, CDP port, `debug:log`, `debug:snapshot`, `debug:capture`.
- `src/main/services/logger.ts`: JSON-lines writer, rotation, redaction, `logger(scope)`, `readLogTail`.
- `src/main/services/ipc-trace.ts`: the global `ipcMain.handle` / `handleOnce` wrapper.
- `src/renderer/debug/debug-bridge.ts`: imported first by `src/renderer/main.tsx`. Error forwarding and `window.__mv`.
- `src/renderer/components/DiagnosticsCard.tsx`: the Settings → About card.
- `scripts/mv.js` and `scripts/test-sources.js`: shell tooling.
