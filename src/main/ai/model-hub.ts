// Model Hub: search Hugging Face, inspect GGUF files, download (resumable) and
// import into Ollama. Downloads land in ~/.mangavox/models/<owner>__<repo>/.
import { nativeFetch } from '../services/dns-fetch'
import { BrowserWindow, shell } from 'electron'
import { spawn } from 'child_process'
import { createWriteStream, existsSync, mkdirSync, readdirSync, renameSync, statSync, unlinkSync, writeFileSync, readFileSync } from 'fs'
import { basename, dirname, join, relative, sep } from 'path'
import type { HubDownload, HubModelDetail, HubModelFile, HubModelSummary, HubSystemFit, LocalModelFile } from '../../shared/ai-types'
import { AppPaths } from '../services/app-paths'
import { groupGgufFiles } from '../../shared/gguf-files'
import { getSystemProfile } from '../services/model-manager'
import { augmentedPath, resolveOllamaBinary } from '../ipc/text.ipc'

const HF = 'https://huggingface.co'
export const HUB_PROGRESS_EVENT = 'ai-hub:download-progress'

export function modelsDir(): string {
  const configured = readHubConfig().modelsDir
  const dir = typeof configured === 'string' && configured.trim() ? configured.trim() : join(AppPaths.root, 'models')
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true })
  return dir
}

function readHubConfig(): { hfToken?: string; modelsDir?: string } {
  try {
    return JSON.parse(readFileSync(AppPaths.config, 'utf-8'))?.aiAssistant?.hub ?? {}
  } catch {
    return {}
  }
}

function hfHeaders(): Record<string, string> {
  const token = readHubConfig().hfToken
  return token ? { authorization: `Bearer ${token}` } : {}
}

interface HfTreeEntry { type: string; path: string; size?: number; lfs?: { size?: number } }

async function hfJson<T>(path: string): Promise<T> {
  const res = await nativeFetch(`${HF}${path}`, { headers: hfHeaders(), signal: AbortSignal.timeout(15_000) })
  if (!res.ok) throw new Error(`Hugging Face ${res.status}${res.status === 401 || res.status === 403 ? ' — this repo may be gated; add a token in Model Hub settings' : ''}`)
  return (await res.json()) as T
}

// ── Search ──────────────────────────────────────────────────────────────────

interface HfListItem { id: string; downloads?: number; likes?: number; lastModified?: string; tags?: string[]; pipeline_tag?: string; gated?: boolean | string }

function toSummary(m: HfListItem): HubModelSummary {
  const [author, ...rest] = m.id.split('/')
  return {
    repo: m.id,
    author,
    name: rest.join('/') || m.id,
    downloads: m.downloads ?? 0,
    likes: m.likes ?? 0,
    updatedAt: m.lastModified,
    tags: m.tags ?? [],
    pipeline: m.pipeline_tag,
    gated: Boolean(m.gated),
  }
}

export async function searchHub(query: string, opts: { sort?: 'downloads' | 'likes' | 'lastModified' | 'trendingScore'; ggufOnly?: boolean; limit?: number } = {}): Promise<HubModelSummary[]> {
  const params = new URLSearchParams({
    sort: opts.sort ?? 'downloads',
    direction: '-1',
    limit: String(opts.limit ?? 40),
  })
  if (query.trim()) params.set('search', query.trim())
  if (opts.ggufOnly !== false) params.set('filter', 'gguf')
  const list = await hfJson<HfListItem[]>(`/api/models?${params}`)
  return list.map(toSummary)
}

// ── Detail ──────────────────────────────────────────────────────────────────

function readmeExcerpt(md: string): string {
  const body = md.replace(/^---[\s\S]*?---\s*/, '').replace(/!\[[^\]]*\]\([^)]*\)/g, '')
  return body
    .split(/\r?\n/)
    .filter((l) => l.trim() && !/^\s*(<|!\[|\||```)/.test(l))
    .slice(0, 14)
    .join('\n')
    .slice(0, 1800)
}

export async function hubDetail(repo: string): Promise<HubModelDetail> {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error('Invalid repo id')
  const [info, tree] = await Promise.all([
    hfJson<any>(`/api/models/${repo}`),
    hfJson<HfTreeEntry[]>(`/api/models/${repo}/tree/main?recursive=true`).catch(() => [] as HfTreeEntry[]),
  ])
  let description: string | undefined
  try {
    const res = await nativeFetch(`${HF}/${repo}/raw/main/README.md`, { headers: hfHeaders(), signal: AbortSignal.timeout(8000) })
    if (res.ok) description = readmeExcerpt(await res.text())
  } catch { /* README is optional */ }

  const files = groupGgufFiles(tree.filter((e) => e.type === 'file').map((e) => ({ path: e.path, size: e.lfs?.size ?? e.size ?? 0 })))
  const gguf = info.gguf ?? {}
  const params = gguf.total ?? info.safetensors?.total
  return {
    ...toSummary(info),
    license: info.cardData?.license ?? (info.tags ?? []).find((t: string) => t.startsWith('license:'))?.slice(8),
    arch: gguf.architecture ?? info.config?.model_type,
    paramsB: params ? Math.round((params / 1e9) * 10) / 10 : undefined,
    context: gguf.context_length,
    description,
    files,
  }
}

export function systemFit(): HubSystemFit {
  const p = getSystemProfile()
  return { vramGB: p.vramGb ?? 0, ramGB: p.ramGb, gpuName: p.gpu }
}

// ── Downloads ───────────────────────────────────────────────────────────────

const downloads = new Map<string, HubDownload & { controller?: AbortController }>()

function publicDownload(d: HubDownload & { controller?: AbortController }): HubDownload {
  const { controller: _c, ...rest } = d
  return rest
}

function broadcast(d: HubDownload) {
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) win.webContents.send(HUB_PROGRESS_EVENT, publicDownload(d))
  }
}

export function listDownloads(): HubDownload[] {
  return [...downloads.values()].map(publicDownload)
}

function safeName(s: string): string {
  return s.replace(/[^\w.-]+/g, '_')
}

/** Start downloading one file (or every shard of a split GGUF) from a repo. */
export function downloadHubFile(repo: string, file: HubModelFile): HubDownload {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error('Invalid repo id')
  const parts = file.parts?.length ? file.parts : [file.path]
  const dir = join(modelsDir(), safeName(repo.replace('/', '__')))
  const first = parts[0]
  const urls = parts.map((p) => ({ url: `${HF}/${repo}/resolve/main/${p.split('/').map(encodeURIComponent).join('/')}`, dest: join(dir, basename(p)) }))
  return startDownload({ repo, file: basename(first), total: file.size, urls })
}

/** Download any direct link (e.g. a .gguf from a website). */
export function downloadUrl(url: string): HubDownload {
  let u: URL
  try { u = new URL(url) } catch { throw new Error('That is not a valid URL') }
  if (!/^https?:$/.test(u.protocol)) throw new Error('Only http(s) links can be downloaded')
  const name = safeName(decodeURIComponent(basename(u.pathname)) || 'model.gguf')
  const dir = join(modelsDir(), 'direct')
  return startDownload({ file: name, total: 0, urls: [{ url: u.toString(), dest: join(dir, name) }] })
}

function startDownload(input: { repo?: string; file: string; total: number; urls: { url: string; dest: string }[] }): HubDownload {
  const existing = [...downloads.values()].find((d) => d.dest === input.urls[0].dest && (d.state === 'downloading' || d.state === 'queued'))
  if (existing) return publicDownload(existing)
  const d: HubDownload & { controller?: AbortController } = {
    id: `dl_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 6)}`,
    repo: input.repo,
    file: input.file,
    url: input.urls[0].url,
    dest: input.urls[0].dest,
    received: 0,
    total: input.total,
    bytesPerSec: 0,
    state: 'queued',
    controller: new AbortController(),
  }
  downloads.set(d.id, d)
  void runDownload(d, input.urls)
  return publicDownload(d)
}

async function runDownload(d: HubDownload & { controller?: AbortController }, urls: { url: string; dest: string }[]) {
  d.state = 'downloading'
  broadcast(d)
  let doneBefore = 0
  let lastEmit = 0
  let windowStart = Date.now()
  let windowBytes = 0
  try {
    for (const { url, dest } of urls) {
      mkdirSync(dirname(dest), { recursive: true })
      if (existsSync(dest)) { doneBefore += statSync(dest).size; continue }
      const part = `${dest}.part`
      const have = existsSync(part) ? statSync(part).size : 0
      const headers: Record<string, string> = { ...(url.startsWith(HF) ? hfHeaders() : {}) }
      if (have) headers.range = `bytes=${have}-`
      const res = await nativeFetch(url, { headers, signal: d.controller!.signal, redirect: 'follow' })
      if (!res.ok && res.status !== 206) {
        throw new Error(res.status === 401 || res.status === 403 ? 'Access denied — this model is gated. Accept its licence on huggingface.co and add your token.' : `Download failed (${res.status})`)
      }
      const resumed = res.status === 206
      const len = Number(res.headers.get('content-length') ?? 0)
      if (urls.length === 1 && !d.total && len) d.total = len + (resumed ? have : 0)
      let received = resumed ? have : 0
      const out = createWriteStream(part, { flags: resumed ? 'a' : 'w' })
      const reader = res.body!.getReader()
      try {
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          if (!out.write(value)) await new Promise((r) => out.once('drain', r))
          received += value.byteLength
          windowBytes += value.byteLength
          d.received = doneBefore + received
          const now = Date.now()
          if (now - windowStart >= 1000) {
            d.bytesPerSec = Math.round((windowBytes * 1000) / (now - windowStart))
            windowStart = now
            windowBytes = 0
          }
          if (now - lastEmit > 300) { lastEmit = now; broadcast(d) }
        }
      } finally {
        await new Promise<void>((r) => out.end(r))
      }
      renameSync(part, dest)
      doneBefore += received
    }
    d.received = doneBefore
    if (!d.total) d.total = doneBefore
    d.state = 'done'
    writeMeta(d)
  } catch (err) {
    d.state = d.controller?.signal.aborted ? 'cancelled' : 'error'
    if (d.state === 'error') d.error = (err as Error).message
  }
  d.bytesPerSec = 0
  broadcast(d)
}

export function cancelDownload(id: string): boolean {
  const d = downloads.get(id)
  if (!d || (d.state !== 'downloading' && d.state !== 'queued')) return false
  d.controller?.abort()
  return true
}

// ── Local files ─────────────────────────────────────────────────────────────

const META = 'mangavox-models.json'

function readMeta(): Record<string, { repo?: string; importedAs?: string }> {
  try { return JSON.parse(readFileSync(join(modelsDir(), META), 'utf-8')) } catch { return {} }
}

function writeMeta(d: HubDownload) {
  const meta = readMeta()
  meta[relative(modelsDir(), d.dest)] = { ...(meta[relative(modelsDir(), d.dest)] ?? {}), repo: d.repo }
  writeFileSync(join(modelsDir(), META), JSON.stringify(meta, null, 2))
}

export function listLocalModels(): LocalModelFile[] {
  const root = modelsDir()
  const meta = readMeta()
  const out: LocalModelFile[] = []
  const walk = (dir: string, depth: number) => {
    if (depth > 3) return
    for (const name of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, name.name)
      if (name.isDirectory()) walk(full, depth + 1)
      else if (/\.(gguf|safetensors|bin)$/i.test(name.name) && !/-0000[2-9]-of-|-000[1-9]\d-of-/i.test(name.name)) {
        const st = statSync(full)
        const rel = relative(root, full)
        out.push({ path: full, name: name.name, size: st.size, modifiedAt: st.mtimeMs, ...meta[rel] })
      }
    }
  }
  try { walk(root, 0) } catch { /* empty */ }
  return out.sort((a, b) => b.modifiedAt - a.modifiedAt)
}

function insideModelsDir(path: string): boolean {
  const rel = relative(modelsDir(), path)
  return Boolean(rel) && !rel.startsWith('..') && !rel.includes(`..${sep}`)
}

export function deleteLocalModel(path: string): boolean {
  if (!insideModelsDir(path) || !existsSync(path)) return false
  const shard = path.match(/^(.*)-00001-of-(\d{5})\.gguf$/i)
  if (shard) {
    const count = Number(shard[2])
    for (let i = 1; i <= count; i++) {
      const p = `${shard[1]}-${String(i).padStart(5, '0')}-of-${shard[2]}.gguf`
      if (existsSync(p)) unlinkSync(p)
    }
  } else {
    unlinkSync(path)
  }
  return true
}

export function revealModelsFolder(path?: string) {
  if (path && insideModelsDir(path)) shell.showItemInFolder(path)
  else void shell.openPath(modelsDir())
}

/** Register a downloaded GGUF with Ollama (`ollama create <name> -f Modelfile`). */
export function importIntoOllama(path: string, name?: string): Promise<{ ok: boolean; name?: string; error?: string }> {
  if (!insideModelsDir(path) || !existsSync(path)) return Promise.resolve({ ok: false, error: 'File not found in the models folder' })
  const modelName = (name || basename(path).replace(/(-00001-of-\d{5})?\.gguf$/i, ''))
    .toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60)
  const bin = resolveOllamaBinary()
  if (!bin) return Promise.resolve({ ok: false, error: 'Ollama is not installed. Get it from ollama.com.' })
  const modelfile = join(AppPaths.cache, `Modelfile.${modelName}`)
  mkdirSync(AppPaths.cache, { recursive: true })
  writeFileSync(modelfile, `FROM ${path}\n`, 'utf-8')
  return new Promise((resolve) => {
    const proc = spawn(bin, ['create', modelName, '-f', modelfile], { env: { ...process.env, PATH: augmentedPath() }, windowsHide: true })
    let stderr = ''
    proc.stderr.on('data', (c) => { stderr = (stderr + c.toString()).slice(-2000) })
    proc.on('error', (err) => resolve({ ok: false, error: err.message }))
    proc.on('close', (code) => {
      if (code !== 0) { resolve({ ok: false, error: stderr.trim().split(/\r?\n/).pop() || `ollama create exited with ${code}` }); return }
      const meta = readMeta()
      const rel = relative(modelsDir(), path)
      meta[rel] = { ...(meta[rel] ?? {}), importedAs: modelName }
      writeFileSync(join(modelsDir(), META), JSON.stringify(meta, null, 2))
      resolve({ ok: true, name: modelName })
    })
  })
}

export function setHubConfig(patch: { hfToken?: string; modelsDir?: string }) {
  let config: Record<string, any> = {}
  try { config = JSON.parse(readFileSync(AppPaths.config, 'utf-8')) } catch { /* new config */ }
  const ai = (config.aiAssistant ??= {})
  ai.hub = { ...(ai.hub ?? {}), ...patch }
  for (const k of Object.keys(ai.hub)) if (!ai.hub[k]) delete ai.hub[k]
  writeFileSync(AppPaths.config, JSON.stringify(config, null, 2))
}

export function hubConfigPublic(): { hasToken: boolean; modelsDir: string } {
  return { hasToken: Boolean(readHubConfig().hfToken), modelsDir: modelsDir() }
}
