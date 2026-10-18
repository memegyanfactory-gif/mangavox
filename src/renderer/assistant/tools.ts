// App-control tools the assistant can call. Each tool declares a risk level that
// the runtime checks against the chat's access mode before running it.
import type { AiToolSpec } from '../../shared/ai-types'
import type { TitleMatch, PlayTitleResult } from '../../shared/agent-types'
import type { AnimeSearchResult } from '../../shared/anime-addon-interface'
import { IPC } from '../../shared/ipc-channels'
import { THEME_PRESETS, type ThemePresetId } from '../../shared/appearance'
import type { Manga, Page } from '../../shared/types'
import { invoke } from '../hooks/useIpc'
import { useUiStore } from '../stores/ui.store'
import { useAppearanceStore } from '../stores/appearance.store'
import { useAppModeStore } from '../stores/app-mode.store'
import { useLibraryStore } from '../stores/library.store'
import { findAndPrepareManga } from '../utils/open-manga'

export type ToolRisk = 'read' | 'act' | 'destructive'

export interface AppTool {
  spec: AiToolSpec
  risk: ToolRisk | ((args: Record<string, any>) => ToolRisk)
  /** Short human label for the step row, e.g. "Opened Settings". */
  label: (args: Record<string, any>) => string
  run: (args: Record<string, any>) => Promise<unknown>
}

const PAGES: Page[] = ['library', 'browse', 'history', 'anime', 'movies', 'discover', 'youtube', 'subtitles', 'chat', 'settings', 'dashboard', 'queue', 'pipeline', 'audio', 'video', 'text']
const SETTINGS_TABS = ['providers', 'models', 'profile', 'playback', 'sources', 'extensions', 'addons', 'streaming', 'storage', 'themes', 'mode', 'chat', 'ai', 'pipeline', 'about']

const obj = (properties: Record<string, unknown>, required: string[] = []) => ({ type: 'object', properties, required })
const str = (description: string, extra: Record<string, unknown> = {}) => ({ type: 'string', description, ...extra })
const num = (description: string) => ({ type: 'number', description })

function compactAnime(a: AnimeSearchResult) {
  return { id: a.id, title: a.title, year: a.year, type: a.animeType, episodes: a.episodeCount, score: a.score, genres: a.genres?.slice(0, 4) }
}

async function searchAnime(query: string) {
  return (await invoke<AnimeSearchResult[]>(IPC.ANIME_SEARCH, query, 1)) ?? []
}

function openAnimeDetail(a: AnimeSearchResult) {
  useUiStore.getState().navigate('anime-detail', {
    animeId: a.id, title: a.title, coverUrl: a.coverUrl, year: a.year, status: a.status, genres: a.genres,
    description: a.description, animeType: a.animeType, episodeCount: a.episodeCount,
  })
}

function startPlayback(result: PlayTitleResult) {
  const stream = result.stream
  const catalog = (result.allStreams?.length ? result.allStreams : [stream]).filter((s) => s.infoHash)
  useUiStore.getState().navigate('anime-player', {
    infoHash: stream.infoHash,
    fileIdx: stream.fileIdx,
    streamTitle: stream.title,
    animeTitle: stream.matchTitle || stream.title,
    coverUrl: stream.coverUrl,
    httpUrl: stream.infoHash?.startsWith('http') ? stream.infoHash : undefined,
    streamQuality: stream.quality,
    imdbId: stream.imdbId,
    mediaKind: stream.category === 'anime' ? 'anime' : stream.category === 'movie' ? 'movie' : 'series',
    availableStreams: catalog.map((s) => ({ infoHash: s.infoHash!, fileIdx: s.fileIdx, title: s.title, quality: s.quality, seeders: s.seeders })),
    animeDetailParams: stream.malId
      ? { animeId: String(stream.malId), title: stream.matchTitle || stream.title }
      : stream.imdbId ? { animeId: stream.imdbId, title: stream.matchTitle || stream.title } : undefined,
  })
}

/** Channels the generic API tool may never call. */
const BLOCKED_CHANNELS = [/^agent:(exec|auto-fix|diagnose|chat)/, /^cache:reset-app$/, /^ai(-hub)?:/, /^debug:/]
const DESTRUCTIVE_CHANNEL = /(delete|remove|clear|reset|uninstall|kill|wipe|purge|cancel)/i
const READ_CHANNEL = /(:get|:list|:search|:status|:stats|:history|:profile|:catalog|:titles|:details|:popular|:latest|:tags|:health)/i

function allChannels(): string[] {
  return [...new Set(Object.values(IPC) as string[])].sort()
}

export const APP_TOOLS: AppTool[] = [
  {
    spec: { name: 'get_app_state', description: 'Current page, its parameters, media category, theme and app mode. Call this first when you need to know where the user is.', parameters: obj({}) },
    risk: 'read',
    label: () => 'Checked what is on screen',
    run: async () => {
      const ui = useUiStore.getState()
      const params = Object.fromEntries(Object.entries(ui.pageParams).filter(([, v]) => ['string', 'number', 'boolean'].includes(typeof v)))
      return {
        page: ui.currentPage,
        params,
        category: ui.category,
        theme: useAppearanceStore.getState().preset,
        appMode: useAppModeStore.getState().mode,
        librarySize: useLibraryStore.getState().manga.length,
      }
    },
  },
  {
    spec: {
      name: 'navigate',
      description: `Open a page of the app. Pages: ${PAGES.join(', ')}. Studio-only pages (dashboard, queue, pipeline, audio, video, text) need Studio mode.`,
      parameters: obj({ page: str('Page id', { enum: PAGES }) }, ['page']),
    },
    risk: 'act',
    label: (a) => `Opened ${a.page}`,
    run: async ({ page }) => {
      if (!PAGES.includes(page)) throw new Error(`Unknown page "${page}"`)
      useUiStore.getState().navigate(page as Page)
      return { ok: true, page }
    },
  },
  {
    spec: { name: 'go_back', description: 'Go back to the previous page.', parameters: obj({}) },
    risk: 'act',
    label: () => 'Went back',
    run: async () => { useUiStore.getState().goBack(); return { ok: true, page: useUiStore.getState().currentPage } },
  },
  {
    spec: { name: 'set_category', description: 'Switch the top-level media section.', parameters: obj({ category: str('Section', { enum: ['manga', 'anime', 'movies'] }) }, ['category']) },
    risk: 'act',
    label: (a) => `Switched to ${a.category}`,
    run: async ({ category }) => { useUiStore.getState().setCategory(category); return { ok: true } },
  },
  {
    spec: {
      name: 'open_settings',
      description: `Open Settings on a tab. Tabs: ${SETTINGS_TABS.join(', ')} ("providers" = AI providers and keys, "models" = download local models).`,
      parameters: obj({ tab: str('Settings tab', { enum: SETTINGS_TABS }) }, ['tab']),
    },
    risk: 'act',
    label: (a) => `Opened Settings → ${a.tab}`,
    run: async ({ tab }) => { useUiStore.getState().navigate('settings', { tab }); return { ok: true } },
  },
  {
    spec: { name: 'list_themes', description: 'List the available colour themes.', parameters: obj({}) },
    risk: 'read',
    label: () => 'Listed themes',
    run: async () => THEME_PRESETS.map((t) => ({ id: t.id, name: t.name })),
  },
  {
    spec: { name: 'set_theme', description: 'Change the app colour theme. Use list_themes for ids.', parameters: obj({ id: str('Theme id') }, ['id']) },
    risk: 'act',
    label: (a) => `Changed theme to ${a.id}`,
    run: async ({ id }) => {
      if (!THEME_PRESETS.some((t) => t.id === id)) throw new Error(`Unknown theme "${id}"`)
      await useAppearanceStore.getState().setPreset(id as ThemePresetId)
      return { ok: true }
    },
  },
  {
    spec: { name: 'list_library', description: "List manga in the user's library, optionally filtered by a text query.", parameters: obj({ query: str('Optional filter') }) },
    risk: 'read',
    label: () => 'Read your library',
    run: async ({ query }) => {
      const all = await invoke<Manga[]>(IPC.LIBRARY_LIST)
      const q = String(query ?? '').toLowerCase()
      return (all ?? [])
        .filter((m) => !q || m.title.toLowerCase().includes(q))
        .slice(0, 40)
        .map((m: any) => ({ id: m.id, title: m.title, status: m.status, author: m.author }))
    },
  },
  {
    spec: { name: 'get_watch_history', description: 'Recently watched anime, movies and series.', parameters: obj({ kind: str('Optional filter', { enum: ['anime', 'movie', 'series'] }), limit: num('Max items (default 15)') }) },
    risk: 'read',
    label: () => 'Read your watch history',
    run: async ({ kind, limit }) => {
      const rows = await invoke<any[]>(IPC.MEDIA_HISTORY, kind || undefined, undefined, Math.min(Number(limit) || 15, 50))
      return (rows ?? []).map((r) => ({ title: r.title ?? r.title_json?.title, kind: r.kind, episode: r.episodeNumber ?? r.episode_number, updated: r.updatedAt ?? r.updated_at }))
    },
  },
  {
    spec: { name: 'search_anime', description: 'Search anime by title. Returns ids you can pass to open_anime.', parameters: obj({ query: str('Title or keywords') }, ['query']) },
    risk: 'read',
    label: (a) => `Searched anime for “${a.query}”`,
    run: async ({ query }) => (await searchAnime(query)).slice(0, 8).map(compactAnime),
  },
  {
    spec: { name: 'open_anime', description: 'Open the detail page of an anime. Pass id from search_anime, or just a title.', parameters: obj({ id: str('Anime id from search_anime'), title: str('Title (used if no id)') }) },
    risk: 'act',
    label: (a) => `Opened ${a.title || 'anime'}`,
    run: async ({ id, title }) => {
      const results = await searchAnime(String(title || id))
      const hit = results.find((r) => String(r.id) === String(id)) ?? results[0]
      if (!hit) throw new Error(`No anime found for "${title || id}"`)
      openAnimeDetail(hit)
      return { ok: true, opened: hit.title }
    },
  },
  {
    spec: { name: 'search_movies', description: 'Search movies and TV series by title.', parameters: obj({ query: str('Title or keywords') }, ['query']) },
    risk: 'read',
    label: (a) => `Searched movies for “${a.query}”`,
    run: async ({ query }) => {
      const rows = (await invoke<AnimeSearchResult[]>(IPC.MOVIE_SEARCH, query)) ?? []
      return rows.slice(0, 8).map((m) => ({ title: m.title, year: m.year, type: m.animeType, imdbId: m.imdbId, score: m.score }))
    },
  },
  {
    spec: { name: 'show_movie_search', description: 'Open the Movies page with a search filled in so the user sees the results.', parameters: obj({ query: str('Search text') }, ['query']) },
    risk: 'act',
    label: (a) => `Showed movie results for “${a.query}”`,
    run: async ({ query }) => { useUiStore.getState().navigate('movies', { searchQuery: query }); return { ok: true } },
  },
  {
    spec: { name: 'open_manga', description: 'Find a manga on the installed sources, add it to the library and open it.', parameters: obj({ title: str('Manga title') }, ['title']) },
    risk: 'act',
    label: (a) => `Opened ${a.title}`,
    run: async ({ title }) => {
      const found = await findAndPrepareManga(String(title), useLibraryStore.getState().addToLibrary)
      if (!found) throw new Error(`"${title}" was not found on the installed manga sources`)
      useUiStore.getState().navigate('manga-detail', { mangaId: found.manga.id, sourceId: found.sourceId })
      return { ok: true, opened: found.manga.title, source: found.sourceId }
    },
  },
  {
    spec: {
      name: 'play_title',
      description: 'Find a stream for an anime, movie or series and start playing it.',
      parameters: obj({ title: str('Title to play'), category: str('Kind', { enum: ['anime', 'movie', 'series', 'all'] }) }, ['title']),
    },
    risk: 'act',
    label: (a) => `Started ${a.title}`,
    run: async ({ title, category }) => {
      const matches = (await invoke<TitleMatch[]>(IPC.AGENT_MATCH_TITLES, title, category || 'all')) ?? []
      if (!matches.length) throw new Error(`No title matched "${title}"`)
      const result = await invoke<PlayTitleResult | null>(IPC.AGENT_PLAY_TITLE, matches[0], title)
      if (!result?.stream?.infoHash) throw new Error(`Found ${matches[0].title}, but no playable stream right now`)
      startPlayback(result)
      return { ok: true, playing: matches[0].title, quality: result.stream.quality }
    },
  },
  {
    spec: {
      name: 'list_app_api',
      description: 'List internal app API channels (for anything the other tools do not cover). Optionally filter by a word, e.g. "download" or "subtitle".',
      parameters: obj({ filter: str('Optional word to filter by') }),
    },
    risk: 'read',
    label: () => 'Looked up app functions',
    run: async ({ filter }) => {
      const f = String(filter ?? '').toLowerCase()
      return allChannels().filter((c) => !BLOCKED_CHANNELS.some((r) => r.test(c)) && (!f || c.includes(f))).slice(0, 120)
    },
  },
  {
    spec: {
      name: 'call_app_api',
      description: 'Call an internal app API channel from list_app_api with positional arguments. Use only when no dedicated tool fits.',
      parameters: obj({ channel: str('Channel name, e.g. "watchlist:list"'), args: { type: 'array', description: 'Positional arguments', items: {} } }, ['channel']),
    },
    risk: (a) => DESTRUCTIVE_CHANNEL.test(String(a.channel)) ? 'destructive' : READ_CHANNEL.test(String(a.channel)) ? 'read' : 'act',
    label: (a) => `Called ${a.channel}`,
    run: async ({ channel, args }) => {
      const ch = String(channel)
      if (!allChannels().includes(ch)) throw new Error(`Unknown channel "${ch}". Use list_app_api.`)
      if (BLOCKED_CHANNELS.some((r) => r.test(ch))) throw new Error(`"${ch}" is not available to the assistant`)
      if (ch === IPC.CONFIG_SET && ['user', 'aiAssistant'].includes(String(args?.[0]))) throw new Error('That setting is protected')
      const result = await invoke<unknown>(ch, ...(Array.isArray(args) ? args : []))
      return trimResult(result)
    },
  },
]

/** Keep tool results small enough for small local context windows. */
export function trimResult(value: unknown, max = 6000): unknown {
  const text = JSON.stringify(value ?? null)
  if (text.length <= max) return value ?? null
  if (Array.isArray(value)) return { items: value.slice(0, 15), note: `${value.length} items, first 15 shown` }
  return { truncated: text.slice(0, max) }
}

export function toolByName(name: string): AppTool | undefined {
  return APP_TOOLS.find((t) => t.spec.name === name)
}

export function toolRisk(tool: AppTool, args: Record<string, any>): ToolRisk {
  return typeof tool.risk === 'function' ? tool.risk(args) : tool.risk
}
