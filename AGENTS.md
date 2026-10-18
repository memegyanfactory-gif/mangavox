</think>

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

