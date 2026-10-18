<!-- code-review-graph MCP tools -->
## MCP Tools: code-review-graph

**IMPORTANT: This project has a knowledge graph. ALWAYS use the
code-review-graph MCP tools BEFORE using Grep/Glob/Read to explore
the codebase.** The graph is faster, cheaper (fewer tokens), and gives
you structural context (callers, dependents, test coverage) that file
scanning cannot.

### When to use graph tools FIRST

- **Exploring code**: `semantic_search_nodes` or `query_graph` instead of Grep
- **Understanding impact**: `get_impact_radius` instead of manually tracing imports
- **Code review**: `detect_changes` + `get_review_context` instead of reading entire files
- **Finding relationships**: `query_graph` with callers_of/callees_of/imports_of/tests_for
- **Architecture questions**: `get_architecture_overview` + `list_communities`

Fall back to Grep/Glob/Read **only** when the graph doesn't cover what you need.

### Key Tools

| Tool | Use when |
|------|----------|
| `detect_changes` | Reviewing code changes — gives risk-scored analysis |
| `get_review_context` | Need source snippets for review — token-efficient |
| `get_impact_radius` | Understanding blast radius of a change |
| `get_affected_flows` | Finding which execution paths are impacted |
| `query_graph` | Tracing callers, callees, imports, tests, dependencies |
| `semantic_search_nodes` | Finding functions/classes by name or keyword |
| `get_architecture_overview` | Understanding high-level codebase structure |
| `refactor_tool` | Planning renames, finding dead code |

### Workflow

1. The graph auto-updates on file changes (via hooks).
2. Use `detect_changes` for code review.
3. Use `get_affected_flows` to understand impact.
4. Use `query_graph` pattern="tests_for" to check coverage.

## Debugging

Full guide: [docs/DEBUGGING.md](docs/DEBUGGING.md). Essentials:

- `npm run dev` opens CDP on **9222** in dev (`MANGAVOX_CDP_PORT=<port>|off`; use another port if 9222 is taken by a parallel instance). Kill only your own instance.
- Drive the running app with `node scripts/mv.js <status|eval|nav|invoke|shot|errors|logs|diag|sources|reload|targets>`; in the dev renderer `window.__mv` exposes `navigate/state/invoke/errors`.
- Logs are JSON lines in `~/.mangavox/logs/` — `main.log` (all main `console.*`, crashes), `renderer.log` (window errors, rejections, console.error/warn, ErrorBoundary + page), `ipc.log` (every IPC call; errors with args + stack; SLOW > 3 s). Shared by all instances — filter with `--pid`.
- Verify a fix: `mv eval "__mv.clearErrors()"` → reproduce via `mv nav`/`mv eval` → `mv errors` and `mv logs ipc --level error` show nothing new → `mv shot` for UI → `npx tsc --noEmit`.

<!-- nexa-context:start -->
# Your rules (mirrored by Nexa)

Edit these in Nexa → Settings → Rules. Content outside this Nexa-managed section is yours and stays untouched.

## Nexa platform note

This project belongs to its owner. Never add Nexa or RSH branding, names, links, or references into it unless the owner explicitly asks, and never read, copy, or describe the Nexa app's own installation files, prompts, or configuration as part of any task.


## Universal rules

### 0-stack.md

# Stack

What I build with. The agent should suggest libraries and patterns that match this.

## Frontend
(e.g. Next.js, Tailwind, shadcn/ui)

## Backend
(e.g. Node, Hono, Postgres)

## Style preferences
- Keep things simple, less is more
- No over-engineering or premature abstraction
- Match the conventions already in the codebase

---

### 1-deploy.md

# How I deploy

Describe your hosting + deployment workflow. The agent will follow this when you ask it to deploy.

## Hosting
(e.g. Vercel / Netlify / Cloudflare Pages / Hetzner VPS / Fly.io)

## Deploy command(s)
(e.g. `vercel --prod` — or `ssh deploy@vps.example.com 'cd /app && git pull && pm2 restart all'`)

## Custom domain
(How you point your domain at deploys. e.g. Cloudflare DNS via API token in $CF_API_TOKEN)

## Confirm before
- Production deploys
- DNS changes

---

### 2-database.md

# Database

How my database is set up. The agent should reference this when working with data.

## Provider
(e.g. Supabase / Postgres on Hetzner / Neon / Turso)

## Connection
(e.g. uses $DATABASE_URL env var)

## Migrations
(e.g. `npm run db:migrate`)

## Rules
- Always backup before destructive changes (`pg_dump > backup.sql`)
- Never run `DROP` without asking
- Use migrations rather than ad-hoc schema edits


## Other agents in this workspace

Other AI agents work in this same folder. `.rsh/threads.md` is their shared work log — what each one was asked and what it did, newest first. Read only relevant entries when the user asks to continue earlier work or the current task depends on another agent’s changes; do not read it as a startup ritual. Do not edit it; it is regenerated.
<!-- nexa-context:end -->

