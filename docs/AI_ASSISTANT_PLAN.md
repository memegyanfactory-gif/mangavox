# MangaVox Assistant — Plan

The goal: one assistant that is always one keystroke away on every page, lives in a
clean collapsible side panel, can talk to any AI provider (cloud APIs, coding-agent
CLIs, local models), can download local models from anywhere (Hugging Face included),
and can actually operate the app through tools.

Progress is tracked in [AI_ASSISTANT_TODO.md](AI_ASSISTANT_TODO.md).

---

## 1. Where we start (audit, 2026-10-18)

| Area | Today | Problem |
|---|---|---|
| Chat page (`pages/Chat.tsx`, 2.8k lines) | Full page, history drawer, canvas, slash commands | Only reachable as a page; every message goes to **Ollama only**, even when an OpenRouter/Gemini model is picked |
| Providers | Raw `fetch` to Ollama; OpenRouter / Gemini / g0i only used by narration | No Claude, OpenAI, Codex, Gemini CLI, OpenCode, LM Studio chat |
| Tool calling | `terminal-agent.ts` regex protocol with coding tools (bash, edit_file…) | The model cannot navigate, search, play, change settings — nothing app-level |
| Local models | Ollama pull by name, `.gguf` import via Modelfile | No Hugging Face browser, no quant picker, no "fits this PC" |
| Messages | Plain text + code fences | No markdown, tables, lists |
| Security | Live-looking OpenRouter key hard-coded as a default (`text.ipc.ts`, `Settings.tsx`) | Must be removed |

Installed on this PC (checked): `claude`, `codex`, `gemini`, `opencode` (npm), `lms` (LM Studio), Ollama install dir.

## 2. Reference designs

- **Nexa** (screenshot 1): collapsible rail of threads, roomy chat column,
  quiet step summaries ("Read files · 2 steps ›"), floating composer.
- **Composer** (screenshot 2): one pill row — provider+model, effort, workflow,
  access mode, attach, send.
- **Model picker** (screenshot 3): provider rail on the left, searchable model list,
  favourites.
- **Usage card** (screenshot 4): provider health at a glance.
- **Model hub** (screenshot 5): curated picks + Hugging Face tab, filter chips
  (Fits this PC, MoE, Vision, Coding, Reasoning, Uncensored), model detail with
  params/arch/context/format/licence, capability chips, and download options per
  quant labelled Smaller / Balanced (Recommended) / Best quality with "Runs well".
- **T3 Code** (pingdotgg/t3code): one adapter per agent behind a shared event
  stream; composer with model / effort / access mode; threads in a sidebar.
  *Web access was blocked in this session, so T3 Code details are from prior
  knowledge and marked unverified; CLI flags are checked against the real
  binaries installed here.*

## 3. Architecture

```
Renderer                                   Main process
──────────────────────────────            ─────────────────────────────────────
AssistantDock (every page)                 ai/registry.ts  ── detects + lists providers
 ├─ ThreadRail (collapsible)               ai/providers/
 ├─ MessageList (markdown, tool steps)      ├─ ollama.ts          (HTTP, native tools)
 └─ Composer (provider·model·effort·access) ├─ openai-compat.ts   (OpenAI, OpenRouter, Groq,
                                            │                      DeepSeek, Mistral, xAI,
assistant.store.ts (threads, prefs)         │                      LM Studio, llama.cpp, custom)
assistant/runtime.ts  ← agent loop          ├─ anthropic.ts       (Messages API, tools)
assistant/tools.ts    ← app-control tools   ├─ gemini.ts          (Gemini API, function calls)
                                            └─ cli.ts             (Claude Code, Codex,
   ai:step ───────────────────────────────▶                        Gemini CLI, OpenCode)
   ◀── ai:event (text / reasoning / status)
Settings → AI Providers                    ai/model-hub.ts  ── HF search, file tree, resumable
Settings → Model Hub                                         downloads, import into Ollama
```

### Key decisions

1. **The agent loop runs in the renderer.** Main streams exactly one model step
   (`ai:step`) and returns the final text + tool calls. The renderer executes tools
   (they need `navigate`, stores, toasts) and calls the next step.
2. **One normalized event stream** (`ai:event`): `text`, `reasoning`, `status`,
   `done`, `error`, keyed by `requestId`. Every adapter maps into it.
3. **Native tool calling** for API/local providers (Ollama `tools`, OpenAI
   `tools`, Anthropic `tools`, Gemini `functionDeclarations`).
   CLI agents get app tools in Phase 5 through a local MCP bridge.
4. **Access modes**: `Ask` (approve every action), `Auto` (reads + safe actions
   run, risky ones ask), `Full` (nothing asks).
5. **Keys stay in main.** `aiAssistant.providers.<id>.apiKey` lives in
   `config.json`; the renderer only sees `hasKey`.
6. **No new heavy dependencies.** Plain `fetch` + `child_process`; a small
   in-house markdown renderer. Matches the codebase (no AI SDKs today).

## 4. Phases

### Phase 0 — Research & plan ✅

### Phase 1 — Provider core (main process)
Shared types, adapters, registry with detection + model lists, IPC
(`ai:providers`, `ai:models`, `ai:step`, `ai:stop`, `ai:settings-*`, `ai:test`),
redacted settings, remove hard-coded OpenRouter key.

### Phase 2 — Assistant dock (renderer)
Store (open, width, threads, prefs), dock on every page (resizable, collapses to a
floating launcher, `Ctrl+J`, TopNav button), thread rail, markdown messages,
reasoning + tool steps, composer with provider·model picker / effort / access,
context-aware suggestions.

### Phase 3 — App-control tools
Registry with risk levels (`read` / `act` / `destructive`): app state, navigate,
back, category, settings, search manga/anime/movies, library, history, open/play,
download, theme, toast, `list_app_api` / `call_app_api`. Agent loop (max 8
rounds) with inline approvals; live app context in the system prompt.

### Phase 4 — Settings: AI Providers + Model Hub
Providers tab (status, keys, test, defaults, install hints). Model Hub (curated
picks, live Hugging Face search, filter chips, detail pane, quant options with
fit-to-PC rating, resumable downloads with HF token + any URL, import into Ollama,
manage downloaded models).

### Phase 5 — MCP bridge for CLI agents
Local MCP server exposing the tool registry; wire Claude Code / Codex / Gemini /
OpenCode to it.

### Phase 6 — Chat page on the new runtime
Full Chat page uses the same providers, threads and tools.

### Phase 7 — Polish & verification
Shortcuts, narrow windows, themes, parser/gating tests, typecheck, live run.

## 5. Risks

- **CLI output formats drift between versions** → parsers accept several shapes
  and fall back to plain text; each provider has a Test button.
- **Windows `.cmd` shims** → prompts go over stdin, never through the shell.
- **Small local models are weak at tool calling** → short tool list, simple
  schemas, malformed calls reported back to the model.
- **Uncensored downloads** are the user's choice; the hub only filters/labels.
