# MangaVox Assistant — To-do

Plan: [AI_ASSISTANT_PLAN.md](AI_ASSISTANT_PLAN.md) · Legend: `[x]` done · `[~]` partly done · `[ ]` not started
Last updated: 2026-10-18

## Phase 0 — Research & plan ✅
- [x] Audit existing chat, providers, settings, layout, tools
- [x] Study reference screenshots (Nexa layout, composer, model picker, model hub)
- [x] Detect installed CLIs on this PC (claude 2.1.302, codex 0.163.0, gemini 0.60.0, opencode 1.19.3, lms)
- [x] Confirm real CLI output formats (Codex `--json`, OpenCode `--format json`, stdin prompts)
- [ ] Verify T3 Code source details (web access was blocked in this session)
- [x] Write plan + this checklist

## Phase 1 — Provider core ✅
- [x] Shared AI types (`src/shared/ai-types.ts`)
- [x] Ollama adapter (stream + native tools + model list, falls back when a model has no tool support)
- [x] OpenAI-compatible adapter (OpenAI, OpenRouter, Groq, DeepSeek, Mistral, xAI, LM Studio, llama.cpp, custom)
- [x] Anthropic API adapter (adaptive thinking + effort, tools, thinking blocks replayed unchanged)
- [x] Gemini API adapter (stream + function calls + thinking budget)
- [x] Claude Code CLI adapter — **tested live** (streams)
- [x] Codex CLI adapter — tested live: error surfaces cleanly (your `~/.codex/config.toml` default model isn't allowed on a ChatGPT account; pick another model)
- [x] Gemini CLI adapter (not signed in on this PC → shown as "Needs setup")
- [x] OpenCode CLI adapter — **tested live** (free `opencode/big-pickle`)
- [x] Registry: detection, versions, model lists, caching, friendly errors, Test button
- [x] Keys stay in main; `config:get` redacts them; legacy OpenRouter/Gemini keys reused
- [x] IPC handlers + `ai:event` stream
- [x] Use the runtime's native fetch (the app's patched fetch buffers whole responses — would break streaming and big downloads)
- [x] Removed the hard-coded OpenRouter key (6 places) — **revoke it on openrouter.ai; it is still in git history**

## Phase 2 — Assistant dock ✅
- [x] Store (open, width, threads, prefs) persisted in localStorage
- [x] Dock on every page, resizable (drag edge, double-click resets), collapses to a floating launcher
- [x] Floats over the page on small windows and on reader/player pages
- [x] `Ctrl+J` toggle + TopNav button; download widget moves aside when docked
- [x] Thread drawer: new, search, rename, delete
- [x] Markdown (headings, lists, tables, code with copy, quotes, links open in the browser)
- [x] Reasoning, tool steps and CLI activity as quiet collapsible rows
- [x] Composer: provider·model picker (provider rail, search, favourites, custom id, `Ctrl+K`), effort, access mode, stop (`Esc`)
- [x] Page-aware suggestions; auto-picks a ready model on first run
- [x] Errors with Retry

## Layout overhaul (Nexa-style) ✅
- [x] Slim title bar (sidebar toggle, back, breadcrumb) replaces the crowded top bar on desktop
- [x] Collapsible left sidebar: section switch, pages, Studio group, assistant + recent chats, settings, profile
- [x] Assistant panel docked on the left, resizable from its right edge
- [x] Removed clutter: search, model, themes, fake notifications and floating launcher icons (all still reachable from the sidebar/settings)
- [x] Settings: grouped list on the left, one consistent centered content column
- [ ] Per-page cleanup (Library/Anime toolbars still dense)
- [ ] Decide on Rust: Tauri port vs full rewrite (see chat)

## Phase 3 — App-control tools ✅ (loop not yet tested with a tool-calling model)
- [x] Tool registry with risk levels (read / act / destructive)
- [x] Navigation, back, category, settings, theme tools
- [x] Search anime / movies, library, watch history — **tested live**
- [x] Open anime / manga, play title, movie search page
- [x] `list_app_api` / `call_app_api` with block list (no app reset, no shell, no AI settings) — **tested live**
- [x] Agent loop (8 rounds) with inline Allow / Deny cards per access mode
- [x] Live app context in the system prompt
- [ ] Live end-to-end run with Ollama or an API key (no tool-capable model available on this PC yet)

## Phase 4 — Settings: AI Providers + Model Hub ✅
- [x] AI Providers tab (all app modes): status, keys, URL, default model, test, enable, "Use in assistant"
- [x] Model Hub: curated picks resolved live from trusted quantizers
- [x] Hugging Face search (trending by default) + filters incl. Uncensored, Fits this PC
- [x] Model detail: params, arch, context, licence, capabilities, README
- [x] Quant options grouped Smaller / Balanced (Recommended) / Higher / Best with fit-to-PC rating
- [x] Resumable downloads (HTTP Range), split GGUF shards, progress, cancel, HF token, any URL — **tested live**
- [x] One-click "Add to Ollama" (Modelfile import) and "Use in assistant"
- [x] Downloaded list: reveal, delete, models folder
- [ ] Choose a custom models folder from the UI (backend supports `modelsDir`)

## Phase 5 — MCP bridge for CLI agents
- [ ] Local MCP server exposing the app tools
- [ ] Wire Claude Code / Codex / Gemini / OpenCode to it

## Phase 6 — Chat page on new runtime
- [ ] Route the full Chat page through the new providers
- [ ] Share threads between page and dock

## Phase 7 — Polish & verification
- [x] Keyboard: `Ctrl+J`, `Ctrl+K`, `Enter`, `Shift+Enter`, `Esc`
- [x] Parser tests (GGUF quants/shards, CLI transcript, markdown) in `npm test`
- [x] Typecheck clean
- [x] Live run screenshots (dock, reply table, model picker, Providers, Model Hub)
- [ ] Light-theme pass (hover tints assume dark themes)
- [x] Gemini CLI sign-in detection checked after an app restart
