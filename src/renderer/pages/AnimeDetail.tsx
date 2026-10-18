import { useState, useEffect, useCallback, useRef, type MouseEvent } from 'react'
import { useUiStore } from '../stores/ui.store'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import {
  ArrowLeft, Play, Loader2, Calendar, Tv, Star, Users, HardDrive,
  ChevronDown, Film, MonitorPlay, AlertTriangle, Download, Heart, Check, Search, Building2,
} from 'lucide-react'
import { coverUrl as proxyCover } from '../utils/cover-url'
import { BulkDownloadModal } from '../components/BulkDownloadModal'
import { AnimeCard } from '../components/anime/AnimeCard'
import { AnimeShelf } from '../components/anime/AnimeShelf'
import { v4 as uuidv4 } from 'uuid'
import type { AnimeEpisode, AnimeTorrentStream, AnimeAddonInfo, AnimeSearchResult } from '../../shared/anime-addon-interface'
import { sortPlayableStreams } from '../../shared/stream-utils'
import { filterMatchingStreams, type ExpectedTitle } from '../../shared/stream-match'
import { useMediaStore } from '../stores/media.store'
import { toast } from '../components/Toaster'
import { WATCH_STATUS_LABELS, WATCH_STATUS_ORDER, type WatchStatus } from '../components/MediaPersonalStrips'
import { trackDownloadStart } from '../stores/downloads.store'

function formatSize(bytes?: number): string {
  if (!bytes) return ''
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`
  return `${(bytes / 1e3).toFixed(0)} KB`
}

function qualityColor(q?: string): string {
  if (!q) return '#6B7280'
  if (q.includes('2160') || q.includes('4K')) return '#F59E0B'
  if (q.includes('1080')) return '#3B82F6'
  if (q.includes('720')) return '#c8501a'
  return '#6B7280'
}

// Parse language hints out of a messy torrent title and return flag emoji.
// Covers the most common patterns that anime/movie release groups use.
// Returns up to 3 flags (deduplicated) — keeps the UI clean.
function extractLanguageFlags(title: string): string[] {
  const t = ' ' + title.toUpperCase().replace(/[._\-\[\](),]/g, ' ') + ' '
  const langs: { flag: string; re: RegExp }[] = [
    { flag: '🇯🇵', re: /\b(JAPANESE|JPN|JAP|JP|JA)\b/ },
    { flag: '🇺🇸', re: /\b(ENGLISH|ENG|EN)\b/ },
    { flag: '🇮🇳', re: /\b(HINDI|HIN|HI)\b/ },
    { flag: '🇪🇸', re: /\b(SPANISH|ESPANOL|CASTELLANO|LATINO|ESP|SPA|ES)\b/ },
    { flag: '🇫🇷', re: /\b(FRENCH|TRUEFRENCH|VF|VFQ|VFF|FRE|FRA|FR)\b/ },
    { flag: '🇩🇪', re: /\b(GERMAN|DEUTSCH|GER|DEU|DE)\b/ },
    { flag: '🇮🇹', re: /\b(ITALIAN|ITA|IT)\b/ },
    { flag: '🇵🇹', re: /\b(PORTUGUESE|BRAZILIAN|POR|PT|BR)\b/ },
    { flag: '🇷🇺', re: /\b(RUSSIAN|RUS|RU)\b/ },
    { flag: '🇨🇳', re: /\b(CHINESE|MANDARIN|CHI|CHN|ZH)\b/ },
    { flag: '🇰🇷', re: /\b(KOREAN|KOR|KR)\b/ },
    { flag: '🇸🇦', re: /\b(ARABIC|ARA|AR)\b/ },
    { flag: '🇹🇷', re: /\b(TURKISH|TUR|TR)\b/ },
  ]
  const found: string[] = []
  for (const { flag, re } of langs) {
    if (re.test(t) && !found.includes(flag)) found.push(flag)
  }
  // "DUAL" / "MULTI" with no explicit langs → assume JP + EN (anime default)
  const isDual = /\b(DUAL|MULTI)(\s*-?\s*(AUDIO|SUB|LANG))?\b/.test(t)
  if (isDual && found.length === 0) return ['🇯🇵', '🇺🇸']
  // If DUAL detected alongside one match, make sure we show at least 2
  if (isDual && found.length === 1) {
    if (!found.includes('🇯🇵')) found.unshift('🇯🇵')
    else if (!found.includes('🇺🇸')) found.push('🇺🇸')
  }
  return found.slice(0, 3)
}

// True if the title indicates a dubbed release (audio in another language),
// vs sub-only. Rendered as a tiny "DUB" pill next to the flags.
function hasDub(title: string): boolean {
  return /\b(DUB|DUBBED|DUAL[\s-]?AUDIO|MULTI[\s-]?AUDIO)\b/i.test(title)
}

// True if the title explicitly indicates a subbed release.
function hasSub(title: string): boolean {
  return /\b(SUB|SUBBED|SUBTITLED|HARDSUB|SOFTSUB)\b/i.test(title)
}

function typeBadgeColor(t?: string): string {
  const colors: Record<string, string> = {
    TV: '#3B82F6', Movie: '#c8501a', OVA: '#F59E0B', ONA: '#c8501a', Special: '#a63f12',
  }
  return colors[t || ''] || '#6B7280'
}

export function AnimeDetail() {
  const { pageParams, navigate, goBack } = useUiStore()
  const {
    animeId, title, coverUrl, year, status, genres, description,
    animeType, episodeCount, score, mediaKind, providerId,
  } = pageParams
  const providers = useMediaStore((state) => state.providers)
  const hydrateMedia = useMediaStore((state) => state.hydrate)
  const recordMedia = useMediaStore((state) => state.record)
  const selectedProvider = providers.find((provider) => provider.id === providerId)

  const [episodes, setEpisodes] = useState<AnimeEpisode[]>([])
  const [loading, setLoading] = useState(true)
  const [selectedEp, setSelectedEp] = useState<string | null>(null)
  const [streams, setStreams] = useState<AnimeTorrentStream[]>([])
  const [loadingStreams, setLoadingStreams] = useState(false)
  const [addons, setAddons] = useState<AnimeAddonInfo[]>([])
  const [noAddons, setNoAddons] = useState(false)
  const [imdbId, setImdbId] = useState<string | null>(null)
  const [imdbLoading, setImdbLoading] = useState(true)
  const [seasons, setSeasons] = useState<{ mal_id: number; name: string; relation: string }[]>([])
  const [selectedSeason, setSelectedSeason] = useState<string>(animeId)
  const [seasonDropdownOpen, setSeasonDropdownOpen] = useState(false)
  const [similarItems, setSimilarItems] = useState<AnimeSearchResult[]>([])
  const [epFilter, setEpFilter] = useState('')
  const [bulkOpen, setBulkOpen] = useState(false)
  const [synopsisOpen, setSynopsisOpen] = useState(false)
  const [details, setDetails] = useState<(AnimeSearchResult & { studio?: string }) | null>(null)
  const allHistory = useMediaStore((state) => state.history)
  const liked = useMediaStore((state) => state.liked)
  const removeFavorite = useMediaStore((state) => state.removeFavorite)
  const effectiveKind = (mediaKind || 'anime') as 'anime' | 'movie' | 'series'
  const mediaIdForTitle = effectiveKind === 'anime' ? `anime:mal:${animeId}` : `${effectiveKind}:imdb:${animeId}`
  const isLiked = liked.some((l) => l.id === mediaIdForTitle)
  const lastWatch = allHistory.find((h) => h.kind === effectiveKind && (h.mediaId === mediaIdForTitle || h.mediaId.endsWith(`:${animeId}`)))
  const [watchStatus, setWatchStatusState] = useState<WatchStatus | null>(null)
  const [statusMenuOpen, setStatusMenuOpen] = useState(false)
  useEffect(() => {
    let alive = true
    setWatchStatusState(null)
    invoke<{ mediaId: string; status: WatchStatus }[]>(IPC.WATCHLIST_LIST, effectiveKind)
      .then((rows) => { if (alive) setWatchStatusState((rows || []).find((x) => x.mediaId === mediaIdForTitle)?.status ?? null) })
      .catch(() => {})
    return () => { alive = false }
  }, [mediaIdForTitle, effectiveKind])
  const chooseStatus = (st: WatchStatus | null) => {
    setStatusMenuOpen(false)
    if (st === null) {
      setWatchStatusState(null)
      void invoke(IPC.WATCHLIST_REMOVE, mediaIdForTitle)
      if (isLiked) void removeFavorite(mediaIdForTitle, effectiveKind)
      return
    }
    setWatchStatusState(st)
    void invoke(IPC.WATCHLIST_SET, mediaIdForTitle, effectiveKind, st)
    if (!isLiked) toggleFavorite() // statuses live on the favorites shelf too
  }
  const toggleFavorite = () => {
    if (isLiked) { void removeFavorite(mediaIdForTitle, effectiveKind); return }
    void recordMedia({
      kind: effectiveKind, type: 'like', providerId,
      title: {
        id: mediaIdForTitle, kind: effectiveKind, title, coverUrl, year, genres: genres || [], languages: [],
        description, score, malId: effectiveKind === 'anime' && /^\d+$/.test(String(animeId)) ? Number(animeId) : undefined,
        imdbId: effectiveKind !== 'anime' ? animeId : undefined,
      },
    })
  }

  const isMovie = animeType === 'Movie'
  // "Play" from a card/hero: start the best source as soon as it's found
  // instead of making the user pick from the list (Netflix-style one click).
  const [autoPlayPending, setAutoPlayPending] = useState(() => !!pageParams.autoPlay && isMovie)
  const autoPlayPendingRef = useRef(autoPlayPending)
  autoPlayPendingRef.current = autoPlayPending
  const streamQueryRef = useRef<string | null>(null)
  // If animeId already looks like an IMDB ID (Cinemeta/real movies), skip Jikan lookup
  const isDirectImdb = typeof animeId === 'string' && animeId.startsWith('tt')
  const isNumeric = (val: string) => /^\d+$/.test(val)
  const isAddonId = typeof animeId === 'string' && !isDirectImdb && !isNumeric(animeId) && !animeId.startsWith('hl4u:')

  // Reset all local state when animeId changes (e.g. when clicking a recommendation)
  useEffect(() => {
    setEpisodes([])
    setLoading(true)
    setSelectedEp(null)
    setStreams([])
    setLoadingStreams(false)
    setImdbId(null)
    setImdbLoading(true)
    setSeasons([])
    setSelectedSeason(animeId)
    setSimilarItems([])
    setEpFilter('')
    setSynopsisOpen(false)
    setDetails(null)
    streamQueryRef.current = null
  }, [animeId])

  /** What a source for `streamId` must be: this title (and this season/episode). */
  const expectedFor = (streamId: string): ExpectedTitle => {
    const se = streamId.match(/:(\d+):(\d+)$/)
    return {
      titles: [title, ...(details?.altTitles || [])].filter(Boolean) as string[],
      year,
      kind: effectiveKind === 'anime' ? 'anime' : isMovie ? 'movie' : 'series',
      season: se ? Number(se[1]) : undefined,
      episode: se ? Number(se[2]) : undefined,
    }
  }
  const expectedForRef = useRef(expectedFor)
  expectedForRef.current = expectedFor
  /** Playable sources that really are this title — other movies/episodes are dropped. */
  const verifiedStreams = (streamId: string, list: AnimeTorrentStream[]) => {
    const checked = filterMatchingStreams(sortPlayableStreams(list), expectedForRef.current(streamId))
    if (checked.removed) console.log(`[anime-detail] hid ${checked.removed} source(s) for a different title/episode`)
    return checked
  }

  // Slow addons (scrapers) finish after the first answer — merge them in live.
  useEffect(() => onEvent(IPC.ADDON_STREAMS_UPDATED, ((update: { resourceId: string; streams: AnimeTorrentStream[] }) => {
    if (!update?.resourceId || update.resourceId !== streamQueryRef.current) return
    setStreams(verifiedStreams(update.resourceId, update.streams).streams)
  }) as (...args: unknown[]) => void), [])

  useEffect(() => {
    if (mediaKind === 'movie' || mediaKind === 'series') void hydrateMedia(mediaKind, providerId)
  }, [mediaKind, providerId, hydrateMedia])

  useEffect(() => {
    if (!animeId) return
    invoke<AnimeSearchResult[]>(IPC.MEDIA_SIMILAR_ITEMS, animeId, mediaKind || 'anime')
      .then((items) => setSimilarItems(items || []))
      .catch(() => setSimilarItems([]))
  }, [animeId, mediaKind])

  // Fill in synopsis / banner / genres when the caller only passed a title + cover (e.g. from history).
  useEffect(() => {
    if (!animeId || effectiveKind !== 'anime' || !/^d+$/.test(String(animeId))) return
    let alive = true
    invoke<AnimeSearchResult & { studio?: string }>(IPC.ANIME_DETAILS, animeId)
      .then((d) => { if (alive && d) setDetails(d) })
      .catch(() => {})
    return () => { alive = false }
  }, [animeId, effectiveKind])

  const shownDesc: string = description || details?.description || ''
  const shownGenres: string[] = (genres && genres.length > 0 ? genres : details?.genres) || []
  const shownScore = score ?? details?.score

  const handleOpenSimilar = (item: AnimeSearchResult) => {
    navigate('anime-detail', {
      animeId: item.imdbId || item.id,
      title: item.title,
      coverUrl: item.coverUrl,
      year: item.year,
      status: item.status,
      genres: item.genres,
      description: item.description,
      animeType: item.animeType,
      score: item.score,
      mediaKind: mediaKind,
      providerId: providerId,
      returnRoute: pageParams.returnRoute,
    })
  }

  // Load IMDB ID, episodes, addons, and relations
  useEffect(() => {
    if (!animeId) return

    // Get IMDB ID for Stremio stream queries
    if (isDirectImdb) {
      setImdbId(animeId)
      setImdbLoading(false)
    } else if (isAddonId) {
      setImdbId(null)
      setImdbLoading(false)
    } else {
      setImdbLoading(true)
      invoke<string | null>(IPC.ANIME_GET_IMDB, animeId)
        .then((id) => {
          setImdbId(id)
          if (id) console.log(`[anime-detail] MAL ${animeId} → IMDB ${id}`)
          else console.warn(`[anime-detail] No IMDB mapping for MAL ${animeId}`)
        })
        .catch(() => setImdbId(null))
        .finally(() => setImdbLoading(false))
    }

    // Load episodes from Jikan for anime or Cinemeta for live-action IMDb series.
    if (!isMovie) {
      setLoading(true)
      invoke<AnimeEpisode[]>(isDirectImdb ? IPC.SERIES_EPISODES : IPC.ANIME_EPISODES, animeId)
        .then(setEpisodes)
        .catch((err) => console.error('[anime-detail] Failed to load episodes:', err))
        .finally(() => setLoading(false))
    } else {
      setEpisodes([])
      setLoading(false)
    }

    // Load installed stream addons
    invoke<AnimeAddonInfo[]>(IPC.ADDON_LIST)
      .then((list) => {
        const streamAddons = list.filter((addon) => addon.resources.includes('stream'))
        setAddons(streamAddons)
        setNoAddons(streamAddons.length === 0)
      })
      .catch(() => setNoAddons(true))

    // Load relations (skip for direct IMDB ids)
    if (isDirectImdb) return
    invoke<{ relation: string; entries: { mal_id: number; type: string; name: string }[] }[]>(
      IPC.ANIME_RELATIONS, animeId
    ).then((relations) => {
      const sequels: { mal_id: number; name: string; relation: string }[] = []
      const seen = new Set<number>()
      for (const r of relations) {
        if (r.relation === 'Sequel' || r.relation === 'Prequel' || r.relation === 'Alternative version') {
          for (const e of r.entries) {
            if (e.type === 'anime' && !seen.has(e.mal_id)) {
              seen.add(e.mal_id)
              sequels.push({ mal_id: e.mal_id, name: e.name, relation: r.relation })
            }
          }
        }
      }
      setSeasons(sequels)
    }).catch(() => {})
  }, [animeId, isMovie, isDirectImdb])

  // Movies have no episode list, so the stream panel is never triggered by a
  // click. Auto-load the available sources/links as soon as we have the IMDB
  // id and at least one stream addon installed.
  useEffect(() => {
    if (!isMovie) return
    if (imdbLoading || loadingStreams) return
    if (addons.length === 0) return
    if (!imdbId && providerId !== 'indian' && !animeId?.startsWith('hl4u:') && !isAddonId) return
    if (streams.length > 0) return
    void loadStreams(undefined, false)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isMovie, imdbId, imdbLoading, addons.length, providerId, isAddonId])

  // Reload episodes when season changes
  useEffect(() => {
    if (selectedSeason === animeId || isMovie) return
    setLoading(true)
    invoke<AnimeEpisode[]>(IPC.ANIME_EPISODES, selectedSeason)
      .then(setEpisodes)
      .catch(() => setEpisodes([]))
      .finally(() => setLoading(false))

    // Also get IMDB for the new season
    setImdbLoading(true)
    invoke<string | null>(IPC.ANIME_GET_IMDB, selectedSeason)
      .then(setImdbId)
      .catch(() => setImdbId(null))
      .finally(() => setImdbLoading(false))
  }, [selectedSeason, animeId, isMovie])

  const loadStreams = useCallback(async (ep?: AnimeEpisode, autoPlay = true) => {
    setLoadingStreams(true)
    setStreams([])
    if (ep) setSelectedEp(ep.id)

    if (addons.length === 0) {
      setLoadingStreams(false)
      return
    }

    let streamId = ''
    
    if (isAddonId) {
      if (isMovie) {
        streamId = animeId
      } else {
        const currentEp = ep || episodes.find((e) => e.id === selectedEp) || episodes[0]
        streamId = currentEp ? currentEp.id : `${animeId}:1:1`
      }
    } else if (imdbId) {
      // Use IMDB ID if available
      streamId = isMovie ? imdbId : `${imdbId}:1:${ep?.number || 1}`
    } else if (animeId?.startsWith('hl4u:')) {
      streamId = animeId
    } else if (providerId === 'indian') {
      // For Indian content without IMDB ID, search by title
      // This allows Torrentio to find streams for Indian titles by name
      const yearStr = year ? `:${year}` : ''
      streamId = isMovie ? `${title}${yearStr}` : `${title}:1:${ep?.number || 1}`
      console.log('[anime-detail] Searching Indian content by title:', streamId)
    } else {
      console.warn('[anime-detail] No IMDB ID available — cannot query streams')
      setLoadingStreams(false)
      return
    }

    const type = isMovie ? 'movie' : 'series'
    streamQueryRef.current = streamId
    const allStreams = await invoke<AnimeTorrentStream[]>(IPC.ADDON_FIND_STREAMS, streamId, type)
    const { streams: playable, allMismatched } = verifiedStreams(streamId, allStreams)
    setStreams(playable)
    setLoadingStreams(false)
    // Episode clicks auto-play. Movies list their sources unless the user hit
    // "Play" (autoPlayPending), in which case the best-ranked source starts now.
    const playMovieNow = isMovie && autoPlayPendingRef.current
    if (playMovieNow) setAutoPlayPending(false)
    // Never auto-start a source whose name points at a different title.
    if (allMismatched && playable.length > 0) {
      if (playMovieNow || (autoPlay && !isMovie)) toast.error('Sources may not match', 'None of the sources found are clearly this title — check the list before playing')
    } else if (playable.length > 0 && ((autoPlay && !isMovie) || playMovieNow)) playStream(playable[0], ep, playable)
    else if (playMovieNow) toast.error('No sources found', 'Pick a source below or try again in a moment')
  }, [addons, imdbId, animeId, isMovie, providerId, title, year, isAddonId, episodes, selectedEp])

  // Nothing to search with (no addons / no id) — don't leave the "Starting…" overlay up.
  useEffect(() => {
    if (!autoPlayPending) return
    const noLookupId = !imdbLoading && !imdbId && providerId !== 'indian' && !animeId?.startsWith('hl4u:') && !isAddonId
    if (noAddons || noLookupId) setAutoPlayPending(false)
  }, [autoPlayPending, noAddons, imdbLoading, imdbId, providerId, animeId, isAddonId])

  const downloadStream = async (stream: AnimeTorrentStream, e?: MouseEvent) => {
    e?.stopPropagation()
    if (!stream.infoHash || stream.infoHash.startsWith('http')) {
      toast.error('Download unavailable', 'This stream type cannot be saved offline')
      return
    }
    const ep = isMovie ? undefined : episodes.find((x) => x.id === selectedEp)
    const downloadId = uuidv4()
    const category = (mediaKind || 'anime') as 'anime' | 'movie' | 'series'
    trackDownloadStart({
      id: downloadId,
      title: title || stream.title,
      coverUrl,
      category,
    })
    // Folder + file naming are resolved by the main process from Settings → Download Folders.
    const res = await invoke<{ started: boolean; error?: string } | undefined>(IPC.MEDIA_TORRENT_DOWNLOAD, {
      id: downloadId,
      infoHash: stream.infoHash,
      fileIdx: stream.fileIdx,
      title: title || stream.title,
      coverUrl,
      category,
      year,
      season: ep ? 1 : undefined,
      episode: ep?.number,
      episodeTitle: ep?.title,
      imdbId: imdbId || undefined,
      malId: effectiveKind === 'anime' && isNumeric(String(animeId)) ? String(animeId) : undefined,
    })
    if (res && res.started === false) {
      toast.error('Download failed', res.error || 'Could not start the download')
      return
    }
    toast.success('Download started', ep ? `${title} · E${ep.number}` : title)
  }

  /** Stream lookup key for an episode (same scheme loadStreams uses). */
  const streamKeyFor = (e: AnimeEpisode): string | undefined => {
    if (isAddonId) return e.id
    if (imdbId) return `${imdbId}:1:${e.number}`
    if (animeId?.startsWith('hl4u:')) return animeId
    return undefined
  }

  // `sourceList` lets loadStreams pass the fresh list — `streams` state is
  // still stale in the same tick it was set.
  const playStream = async (stream: AnimeTorrentStream, selectedEpisode?: AnimeEpisode, sourceList = streams) => {
    // Never carry the one-shot autoPlay flag into routes we return to.
    const { autoPlay: _autoPlay, ...detailParams } = pageParams
    const ep = selectedEpisode || episodes.find((e) => e.id === selectedEp)
    const epIndex = ep ? episodes.findIndex((e) => e.id === ep.id) : -1

    // Resolve a source for the NEXT episode now so "Next episode" never replays the current file.
    // Later episodes carry only their streamKey; the player resolves them via ADDON_FIND_STREAMS.
    let nextStream: AnimeTorrentStream | undefined
    const nextEp = epIndex >= 0 ? episodes[epIndex + 1] : undefined
    const nextKey = nextEp ? streamKeyFor(nextEp) : undefined
    if (nextEp && nextKey && !stream.infoHash?.startsWith('http')) {
      try {
        const found = await Promise.race([
          invoke<AnimeTorrentStream[]>(IPC.ADDON_FIND_STREAMS, nextKey, 'series'),
          new Promise<null>((resolve) => setTimeout(() => resolve(null), 3500)),
        ])
        const checked = found ? verifiedStreams(nextKey, found) : null
        const playable = checked && !checked.allMismatched ? checked.streams : []
        nextStream = playable.find((c) => c.quality === stream.quality && c.source === stream.source)
          || playable.find((c) => c.quality === stream.quality)
          || playable[0]
      } catch { /* player falls back to streamKey lookup */ }
    }

    const epList = episodes.map((e, i) => {
      const isCurrent = ep != null && e.id === ep.id
      const src = isCurrent ? stream : (i === epIndex + 1 ? nextStream : undefined)
      return {
        id: e.id,
        number: e.number,
        title: e.title,
        thumbnail: e.thumbnail,
        season: 1,
        airDate: e.airDate,
        streamKey: streamKeyFor(e),
        infoHash: src?.infoHash,
        fileIdx: src?.fileIdx,
        streamTitle: src?.title,
        streamQuality: src?.quality,
      }
    })

    navigate('anime-player', {
      infoHash: stream.infoHash,
      fileIdx: stream.fileIdx,
      streamTitle: stream.title,
      animeTitle: title,
      episodeNumber: ep?.number,
      episodeTitle: ep?.title,
      coverUrl,
      httpUrl: stream.infoHash?.startsWith('http') ? stream.infoHash : undefined,
      streamQuality: stream.quality,
      availableStreams: sourceList,
      episodeList: epList,
      currentEpIndex: epIndex >= 0 ? epIndex : undefined,
      animeDetailParams: detailParams,
      imdbId: imdbId || undefined,
      seasonNumber: ep != null ? 1 : undefined,
      mediaKind: mediaKind || 'anime',
      providerId,
      genres,
      year,
      description,
      score,
      returnRoute: { page: 'anime-detail', params: detailParams },
    })
  }

  const renderStreamPanel = () => {
    if (noAddons) {
      return (
        <div className="flex items-center gap-2 px-4 py-3 rounded-xl text-xs" style={{ background: 'rgba(251,191,36,0.08)', border: '1px solid rgba(251,191,36,0.15)' }}>
          <AlertTriangle size={14} className="text-amber-400 shrink-0" />
          <span className="text-amber-400/70">Install a Stremio addon (like Torrentio) in Settings → Addons to stream.</span>
        </div>
      )
    }
    if (imdbLoading) {
      return (
        <div className="flex items-center gap-2 text-xs text-[color:var(--text-3)] py-3">
          <Loader2 size={12} className="animate-spin" /> Resolving IMDB ID...
        </div>
      )
    }
    if (!imdbId && !isAddonId) {
      return (
        <div className="flex items-center gap-2 px-4 py-3 rounded-xl text-xs" style={{ background: 'rgba(239,68,68,0.08)', border: '1px solid rgba(239,68,68,0.15)' }}>
          <AlertTriangle size={14} className="text-[color:var(--accent-hover)] shrink-0" />
          <span className="text-[color:var(--accent-hover)]">No IMDB mapping found for this anime. Torrent streams require IMDB IDs.</span>
        </div>
      )
    }
    if (loadingStreams) {
      return (
        <div className="flex items-center gap-2 text-xs text-[color:var(--text-3)] py-3">
          <Loader2 size={12} className="animate-spin" /> Searching {addons.length} addon{addons.length > 1 ? 's' : ''} for streams...
        </div>
      )
    }
    if (streams.length === 0) {
      return (
        <p className="text-xs text-[color:var(--text-3)] py-2">No sources found. This {isMovie ? 'title' : 'anime'} may not be available on your installed addons.</p>
      )
    }

    return (
      <div>
        <p className="text-[10px] text-[color:var(--text-3)] uppercase tracking-wider font-semibold mb-2 px-0.5">
          {streams.length} stream{streams.length !== 1 ? 's' : ''}
        </p>
        <div className="grid grid-cols-1 gap-2">
          {streams.map((s, i) => {
            const flags = extractLanguageFlags(s.title)
            const isDub = hasDub(s.title)
            const isSub = !isDub && hasSub(s.title)
            return (
              <div
                key={`${s.infoHash}-${i}`}
                className="relative flex flex-col gap-2 px-3.5 py-3 rounded-xl transition-all hover:bg-white/[0.08] group text-left cursor-pointer animate-fade-in"
                style={{ background: 'rgba(255,255,255,0.015)', border: '1px solid var(--panel-border)' }}
                onClick={() => playStream(s)}
              >
                {/* Row 1: Play button + Addon source + Quality + Info */}
                <div className="flex items-center gap-2 w-full min-w-0 pr-9 sm:pr-7">
                  {/* Play dot */}
                  <div
                    className="w-5 h-5 rounded-md flex items-center justify-center shrink-0 transition-all group-hover:scale-110 group-hover:bg-[rgba(var(--accent-rgb),0.3)]"
                    style={{ background: 'rgba(200, 80, 26,0.12)' }}
                  >
                    <Play size={9} className="text-[color:var(--accent-hover)] translate-x-[0.5px]" fill="currentColor" />
                  </div>

                  {s.source && (
                    <span className="text-[10px] text-[color:var(--text-2)] font-bold uppercase tracking-wider bg-white/5 px-1.5 py-0.5 rounded leading-none shrink-0">
                      {s.source}
                    </span>
                  )}

                  {s.quality && (
                    <span
                      className="px-1.5 py-0.5 rounded text-[10px] font-bold leading-none shrink-0"
                      style={{ background: `${qualityColor(s.quality)}15`, color: qualityColor(s.quality) }}
                    >
                      {s.quality}
                    </span>
                  )}

                  <div className="flex-1" />

                  {s.size != null && s.size > 0 && (
                    <span className="text-[10px] text-[color:var(--text-2)] font-medium tabular-nums shrink-0">
                      {formatSize(s.size)}
                    </span>
                  )}

                  {s.seeders != null && (
                    <span
                      className={`flex items-center gap-0.5 text-[10px] font-semibold tabular-nums shrink-0 ${
                        s.seeders > 50 ? 'text-green-400' : s.seeders > 10 ? 'text-yellow-400' : 'text-[color:var(--accent-hover)]'
                      }`}
                      title={`${s.seeders} seeders`}
                    >
                      <Users size={9} /> {s.seeders}
                    </span>
                  )}
                </div>

                {/* Row 2: Stream Title (e.g. Dacoit A Love Story 2026 Hindi 1080p WEBRip x264 AAC-5.1.mkv) */}
                <p className="text-[11px] text-[color:var(--text-1)] leading-snug pl-7 pr-6 break-all font-medium line-clamp-2">
                  {s.title}
                </p>

                {/* Row 3: flags + SUB/DUB badge */}
                {(flags.length > 0 || isDub || isSub) && (
                  <div className="flex items-center gap-1.5 pl-7">
                    {flags.length > 0 && (
                      <span className="flex items-center gap-0.5 text-[13px] leading-none">
                        {flags.map((f, idx) => <span key={idx}>{f}</span>)}
                      </span>
                    )}
                    {isDub && (
                      <span
                        className="px-1 py-0.5 rounded text-[8px] font-bold leading-none"
                        style={{ background: 'rgba(200, 80, 26,0.12)', color: 'var(--accent-hover)', border: '1px solid rgba(200, 80, 26,0.2)' }}
                      >
                        DUB
                      </span>
                    )}
                    {isSub && (
                      <span
                        className="px-1 py-0.5 rounded text-[8px] font-bold leading-none"
                        style={{ background: 'rgba(56,189,248,0.12)', color: '#38BDF8', border: '1px solid rgba(56,189,248,0.2)' }}
                      >
                        SUB
                      </span>
                    )}
                  </div>
                )}
                <button
                  type="button"
                  onClick={(ev) => void downloadStream(s, ev)}
                  className="absolute top-1 right-1 sm:top-2.5 sm:right-2.5 w-10 h-10 sm:w-auto sm:h-auto sm:p-1.5 flex items-center justify-center rounded-md opacity-100 sm:opacity-0 sm:group-hover:opacity-100 transition-opacity hover:bg-white/10"
                  title="Download"
                >
                  <Download size={13} className="text-[color:var(--text-2)]" />
                </button>
              </div>
            )
          })}
        </div>
      </div>
    )
  }

  const heroBg = details?.bannerUrl || coverUrl
  const resumeTarget = (() => {
    if (isMovie || episodes.length === 0) return null
    const lastNum = lastWatch?.episodeNumber
    const resume = lastWatch && !lastWatch.completed && lastNum != null ? episodes.find((e) => e.number === lastNum) : undefined
    const next = resume || (lastNum != null ? episodes.find((e) => e.number > lastNum) : undefined) || episodes[0]
    if (!next) return null
    const label = resume ? `Resume E${next.number}` : lastNum != null ? `Next: E${next.number}` : `Start Watching E${next.number}`
    return { ep: next, label }
  })()
  const filteredEpisodes = episodes.filter((ep) => {
    const q = epFilter.trim().toLowerCase()
    return !q || String(ep.number) === q || (ep.title || '').toLowerCase().includes(q)
  })
  const bulkEpisodes = episodes.map((e) => ({ number: e.number, title: e.title, season: 1 }))

  return (
    <div className={`relative flex flex-col h-full overflow-x-hidden page-enter ${autoPlayPending ? 'overflow-y-hidden' : 'overflow-y-auto'}`}>
      {autoPlayPending && (
        <div className="absolute inset-0 z-40 flex flex-col items-center justify-center gap-5 text-center px-6" style={{ background: 'var(--base)' }}>
          {heroBg && (
            <div
              className="absolute inset-0 bg-cover bg-center"
              style={{ backgroundImage: `url(${proxyCover(heroBg) || ''})`, opacity: 0.35, filter: 'blur(2px) saturate(1.1)' }}
            />
          )}
          <div className="absolute inset-0" style={{ background: 'radial-gradient(ellipse at center, rgba(8,8,9,.55) 0%, rgba(8,8,9,.92) 70%)' }} />
          <div className="relative flex flex-col items-center gap-4">
            <h1 className="text-2xl sm:text-4xl font-black text-white drop-shadow-lg max-w-2xl">{title}</h1>
            <div className="flex items-center gap-2 text-sm text-white/75">
              <Loader2 size={16} className="animate-spin" />
              {loadingStreams || imdbLoading ? 'Finding the best source…' : 'Starting playback…'}
            </div>
            <button
              onClick={() => setAutoPlayPending(false)}
              className="mt-2 px-4 py-1.5 rounded-full text-xs font-semibold text-white/80 hover:text-white transition-colors"
              style={{ background: 'rgba(255,255,255,0.1)', border: '1px solid rgba(255,255,255,0.18)' }}
            >
              Choose a source instead
            </button>
          </div>
        </div>
      )}
      {/* Banner */}
      <div className="relative shrink-0 overflow-hidden">
        {heroBg && (
          <div
            className="absolute inset-0 bg-cover bg-center"
            style={{ backgroundImage: `url(${proxyCover(heroBg) || ''})`, filter: details?.bannerUrl ? 'saturate(1.1)' : 'blur(30px) saturate(1.25)', transform: 'scale(1.25)', opacity: 0.55 }}
          />
        )}
        <div className="absolute inset-0" style={{ background: 'linear-gradient(90deg, rgba(8,8,9,.88) 0%, rgba(8,8,9,.55) 55%, rgba(8,8,9,.25) 100%)' }} />
        <div className="absolute inset-x-0 bottom-0 h-40" style={{ background: 'linear-gradient(0deg, var(--base) 0%, transparent 100%)' }} />

        <div className="relative px-4 sm:px-6 pt-3 pb-6 sm:pb-8">
          <button
            onClick={() => {
              const ret = pageParams.returnRoute
              if (ret?.page) {
                navigate(ret.page, ret.params || {})
              } else {
                goBack()
              }
            }}
            className="min-h-10 -ml-1 px-1 flex items-center gap-2 text-xs text-[color:var(--text-2)] hover:text-white transition-colors mb-2 sm:mb-4"
          >
            <ArrowLeft size={14} /> Back
          </button>

          <div className="flex flex-col sm:flex-row gap-4 sm:gap-6 max-w-[1400px] mx-auto">
            {coverUrl && (
              <div className="relative shrink-0 self-center sm:self-start">
                <img
                  src={coverUrl}
                  alt={title}
                  className="w-32 sm:w-36 md:w-44 aspect-[2/3] rounded-xl object-cover shadow-2xl"
                  style={{ border: '1px solid rgba(255,255,255,0.12)' }}
                />
                {animeType && (
                  <span
                    className="absolute top-2 left-2 px-2 py-0.5 rounded text-[10px] font-bold uppercase tracking-wide"
                    style={{ background: typeBadgeColor(animeType), color: 'white' }}
                  >
                    {animeType}
                  </span>
                )}
              </div>
            )}

            <div className="flex-1 min-w-0 pt-1">
              <h1 className="text-[22px] sm:text-[28px] break-words font-extrabold tracking-tight mb-2 leading-tight text-white drop-shadow-[0_4px_24px_rgba(0,0,0,0.6)]">
                {title || 'Unknown Anime'}
              </h1>

              <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-[color:var(--text-2)] mb-3">
                {shownScore != null && (
                  <span className="flex items-center gap-1 text-amber-400 font-bold">
                    <Star size={13} fill="currentColor" /> {shownScore}
                  </span>
                )}
                {year && (
                  <span className="flex items-center gap-1"><Calendar size={12} /> {year}</span>
                )}
                {animeType && (
                  <span className="flex items-center gap-1">
                    {animeType === 'Movie' ? <Film size={12} /> : <Tv size={12} />}
                    {animeType}
                  </span>
                )}
                {episodeCount && !isMovie && <span>{episodeCount} episodes</span>}
                {details?.studio && <span className="flex items-center gap-1"><Building2 size={12} /> {details.studio}</span>}
                {status && (
                  <span className="px-2 py-0.5 rounded-full" style={{ background: 'rgba(var(--accent-rgb),0.2)', color: 'var(--accent-hover)' }}>
                    {status}
                  </span>
                )}
                {imdbId && <span className="text-[color:var(--text-3)] font-mono text-[10px]">{imdbId}</span>}
              </div>

              {shownGenres.length > 0 && (
                <div className="flex flex-wrap gap-1.5 mb-3">
                  {shownGenres.map((g: string) => (
                    <span key={g} className="px-2.5 py-0.5 rounded-full text-[11px] font-medium text-white" style={{ background: 'rgba(255,255,255,0.12)', border: '1px solid rgba(255,255,255,0.14)' }}>
                      {g}
                    </span>
                  ))}
                </div>
              )}

              {shownDesc && (
                <div className="max-w-3xl mb-4">
                  <p className={`text-[13px] text-[color:var(--text-1)] leading-relaxed ${synopsisOpen ? '' : 'line-clamp-3'}`}>
                    {shownDesc}
                  </p>
                  {shownDesc.length > 220 && (
                    <button
                      onClick={() => setSynopsisOpen((v) => !v)}
                      className="mt-1 text-[11px] font-bold uppercase tracking-wide"
                      style={{ color: 'var(--accent-hover)' }}
                    >
                      {synopsisOpen ? 'Less' : 'More'}
                    </button>
                  )}
                </div>
              )}

              <div className="flex items-center gap-2.5 flex-wrap">
                {resumeTarget && (
                  <button
                    onClick={() => void loadStreams(resumeTarget.ep)}
                    disabled={loadingStreams || noAddons}
                    className="w-full sm:w-auto justify-center min-h-10 flex items-center gap-2 px-5 py-2 rounded-lg text-[13px] font-bold text-white disabled:opacity-50 transition-transform hover:scale-[1.03]"
                    style={{ background: 'var(--accent)' }}
                  >
                    {loadingStreams ? <Loader2 size={14} className="animate-spin" /> : <Play size={14} fill="currentColor" />} {resumeTarget.label}
                  </button>
                )}
                {isMovie && streams.length > 0 && (
                  <button
                    onClick={() => playStream(streams[0])}
                    className="w-full sm:w-auto justify-center min-h-10 flex items-center gap-2 px-5 py-2 rounded-lg text-[13px] font-bold text-white transition-transform hover:scale-[1.03]"
                    style={{ background: 'var(--accent)' }}
                  >
                    <Play size={14} fill="currentColor" /> Play
                  </button>
                )}
                {effectiveKind !== 'series' ? (
                  <div className="relative">
                    <button
                      onClick={() => setStatusMenuOpen((v) => !v)}
                      className="min-h-10 flex items-center gap-1.5 px-3.5 py-2 rounded-lg text-xs font-semibold transition-colors hover:bg-white/20"
                      style={{ background: 'rgba(255,255,255,0.12)', border: '1px solid rgba(255,255,255,0.18)', color: watchStatus || isLiked ? '#f472b6' : '#fff' }}
                    >
                      <Heart size={13} fill={isLiked || watchStatus ? 'currentColor' : 'none'} />
                      {watchStatus ? WATCH_STATUS_LABELS[watchStatus] : isLiked ? 'In My List' : 'Add to My List'}
                      <ChevronDown size={12} />
                    </button>
                    {statusMenuOpen && (
                      <>
                        <div className="fixed inset-0 z-40" onClick={() => setStatusMenuOpen(false)} />
                        <div className="absolute left-0 top-full mt-1 z-50 min-w-[160px] rounded-xl p-1 shadow-xl" style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
                          {WATCH_STATUS_ORDER.map((st) => (
                            <button
                              key={st}
                              onClick={() => chooseStatus(st)}
                              className="w-full min-h-10 sm:min-h-0 text-left px-2.5 py-1.5 rounded-lg text-xs hover:bg-white/10 transition-colors"
                              style={{ color: watchStatus === st ? 'var(--accent)' : 'var(--text-1)', fontWeight: watchStatus === st ? 700 : 500 }}
                            >
                              {WATCH_STATUS_LABELS[st]}
                            </button>
                          ))}
                          {(isLiked || watchStatus) && (
                            <button onClick={() => chooseStatus(null)} className="w-full text-left px-2.5 py-1.5 rounded-lg text-xs text-red-300 hover:bg-red-500/10 transition-colors border-t mt-1" style={{ borderColor: 'var(--panel-border)' }}>
                              Remove from list
                            </button>
                          )}
                        </div>
                      </>
                    )}
                  </div>
                ) : (
                  <button
                    onClick={toggleFavorite}
                    className="min-h-10 flex items-center gap-1.5 px-3.5 py-2 rounded-lg text-xs font-semibold transition-colors hover:bg-white/20"
                    style={{ background: 'rgba(255,255,255,0.12)', border: '1px solid rgba(255,255,255,0.18)', color: isLiked ? '#f472b6' : '#fff' }}
                  >
                    <Heart size={13} fill={isLiked ? 'currentColor' : 'none'} /> {isLiked ? 'In My List' : 'Add to My List'}
                  </button>
                )}
                <button
                  onClick={() => setBulkOpen(true)}
                  className="min-h-10 flex items-center gap-1.5 px-3.5 py-2 rounded-lg text-xs font-semibold text-white transition-colors hover:bg-white/20"
                  style={{ background: 'rgba(255,255,255,0.12)', border: '1px solid rgba(255,255,255,0.18)' }}
                >
                  <Download size={13} /> Bulk download
                </button>
                {lastWatch && (
                  <span className="text-[11px] text-[color:var(--text-3)]">
                    {lastWatch.completed ? 'Completed' : 'Watching'}{lastWatch.episodeNumber != null ? ` · up to E${lastWatch.episodeNumber}` : ''}
                  </span>
                )}
              </div>

              {(mediaKind === 'movie' || mediaKind === 'series') && (
                <div className="mt-4">
                  <p className="text-[10px] uppercase tracking-wider text-[color:var(--text-3)] mb-2">Official services</p>
                  <div className="flex flex-wrap gap-2">
                    {(selectedProvider ? [selectedProvider] : providers.slice(0, 8)).map((provider) => (
                      <button key={provider.id} onClick={() => {
                        void invoke(IPC.MEDIA_OPEN_PROVIDER, provider.id, title, 'IN')
                        void recordMedia({
                          kind: mediaKind, type: 'play', providerId: provider.id,
                          title: {
                            id: `${mediaKind}:imdb:${animeId}`, kind: mediaKind, title, imdbId: animeId,
                            coverUrl, year, genres: genres || [], languages: [], description, score,
                          },
                        })
                      }} className="min-h-10 px-3 py-2 rounded-lg text-xs font-semibold text-white"
                        style={{ background: provider.theme.accent }}>
                        Open on {provider.name}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {noAddons && (
                <div className="mt-4 px-4 py-3 rounded-xl text-xs max-w-xl" style={{ background: 'rgba(251,191,36,0.08)', border: '1px solid rgba(251,191,36,0.15)' }}>
                  <p className="text-amber-400/80 font-medium mb-1">No stream addons installed</p>
                  <p className="text-amber-400/50">
                    Install a Stremio addon (like Torrentio) in Settings → Addons to stream episodes.
                  </p>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* Content area */}
      <div className="px-4 sm:px-6 pb-8 flex-1 min-w-0">
        <div className="max-w-[1400px] mx-auto">
          {isMovie ? (
            /* ── Movie: Direct stream panel (no episode list) ── */
            <div>
              <h2 className="text-sm font-semibold text-[color:var(--text-1)] mb-1">Community Sources</h2>
              <p className="text-xs text-[color:var(--text-3)] mb-4">Optional streams from installed addons, independent of official services.</p>
              <div className="rounded-xl p-4" style={{ background: 'var(--surface-2)' }}>
                {renderStreamPanel()}
              </div>
            </div>
          ) : (
            /* ── Series: season selector + episode cards with expandable streams ── */
            <div>
              <div className="flex items-center gap-3 mb-3 flex-wrap">
                {seasons.length > 0 ? (
                  <div className="relative">
                    <button
                      onClick={() => setSeasonDropdownOpen(!seasonDropdownOpen)}
                      className="min-h-10 max-w-full flex items-center gap-2 px-3.5 py-2 rounded-lg text-[13px] font-bold text-[color:var(--text-1)] transition-all hover:text-white"
                      style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                    >
                      <MonitorPlay size={14} />
                      <span className="max-w-[55vw] sm:max-w-[320px] truncate">
                        {selectedSeason === animeId ? title : seasons.find((s) => String(s.mal_id) === selectedSeason)?.name || title}
                      </span>
                      <ChevronDown size={14} className={`transition-transform ${seasonDropdownOpen ? 'rotate-180' : ''}`} />
                    </button>
                    {seasonDropdownOpen && (
                      <>
                        <div className="fixed inset-0 z-40" onClick={() => setSeasonDropdownOpen(false)} />
                        <div
                          className="absolute top-full left-0 mt-1 py-1 rounded-lg shadow-xl z-50 w-[min(320px,calc(100vw-2rem))] sm:w-auto sm:min-w-[320px] max-h-60 overflow-y-auto"
                          style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}
                        >
                          <button
                            onClick={() => { setSelectedSeason(animeId); setSeasonDropdownOpen(false) }}
                            className={`w-full min-h-10 sm:min-h-0 text-left px-4 py-2 text-xs transition-colors ${
                              selectedSeason === animeId ? 'text-[color:var(--accent-hover)]' : 'text-[color:var(--text-2)] hover:text-white hover:bg-white/[0.04]'
                            }`}
                          >
                            {title} (Current)
                          </button>
                          {seasons.map((s) => (
                            <button
                              key={s.mal_id}
                              onClick={() => { setSelectedSeason(String(s.mal_id)); setSeasonDropdownOpen(false) }}
                              className={`w-full min-h-10 sm:min-h-0 text-left px-4 py-2 text-xs transition-colors ${
                                selectedSeason === String(s.mal_id) ? 'text-[color:var(--accent-hover)]' : 'text-[color:var(--text-2)] hover:text-white hover:bg-white/[0.04]'
                              }`}
                            >
                              {s.name} <span className="text-[color:var(--text-3)]">({s.relation})</span>
                            </button>
                          ))}
                        </div>
                      </>
                    )}
                  </div>
                ) : (
                  <h2 className="text-[15px] font-bold text-[color:var(--text-1)]">Episodes</h2>
                )}
                <span className="text-[11px] text-[color:var(--text-3)]">
                  {!loading && episodes.length > 0 && `${episodes.length} episodes`}
                </span>
                {episodes.length > 12 && (
                  <div className="relative w-full sm:w-auto sm:ml-auto">
                    <Search size={11} className="absolute left-2 top-1/2 -translate-y-1/2 text-[color:var(--text-3)]" />
                    <input
                      value={epFilter}
                      onChange={(e) => setEpFilter(e.target.value)}
                      placeholder="Find episode..."
                      className="h-10 sm:h-7 w-full sm:w-44 pl-7 pr-2 rounded-md text-[11px] text-[color:var(--text-1)] outline-none"
                      style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                    />
                  </div>
                )}
              </div>

              {loading ? (
                <div className="flex items-center gap-2 text-sm text-[color:var(--text-3)]">
                  <Loader2 size={16} className="animate-spin" /> Loading episodes...
                </div>
              ) : episodes.length === 0 ? (
                <div className="flex flex-col items-center gap-2 py-12">
                  <Tv size={24} className="text-[color:var(--text-3)]" />
                  <p className="text-sm text-[color:var(--text-3)]">No episode data available.</p>
                  <p className="text-xs text-[color:var(--text-3)]">This anime may not have individual episodes listed yet.</p>
                </div>
              ) : (
                <div className="grid grid-cols-1 xl:grid-cols-2 gap-3">
                  {filteredEpisodes.map((ep) => {
                    const isOpen = selectedEp === ep.id
                    const lw = lastWatch?.episodeNumber
                    const watched = lw != null && (ep.number < lw || (ep.number === lw && lastWatch?.completed))
                    const inProgress = lw === ep.number && !lastWatch?.completed && (lastWatch?.durationSecs ?? 0) > 0
                    const pct = watched ? 100 : inProgress ? Math.min(100, ((lastWatch?.positionSecs ?? 0) / (lastWatch?.durationSecs || 1)) * 100) : 0
                    const thumb = ep.thumbnail || coverUrl
                    return (
                      <div key={ep.id} className="contents">
                        <button
                          onClick={() => {
                            if (isOpen) {
                              setSelectedEp(null)
                              setStreams([])
                            } else {
                              void loadStreams(ep)
                            }
                          }}
                          className="group w-full min-w-0 flex gap-3 p-2 rounded-xl text-left transition-colors hover:bg-white/[0.05]"
                          style={isOpen ? { background: 'rgba(var(--accent-rgb),0.12)', border: '1px solid rgba(var(--accent-rgb),0.35)' } : { background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}
                        >
                          <div className="relative w-[124px] min-[420px]:w-[150px] sm:w-[176px] aspect-video shrink-0 rounded-lg overflow-hidden" style={{ background: 'var(--surface-3)' }}>
                            {thumb && (
                              ep.thumbnail ? (
                                <img src={ep.thumbnail} alt="" loading="lazy" className="w-full h-full object-cover" />
                              ) : (
                                <>
                                  <div className="absolute inset-0 bg-cover bg-center" style={{ backgroundImage: `url(${proxyCover(thumb) || ''})`, filter: 'blur(12px)', transform: 'scale(1.3)', opacity: 0.6 }} />
                                  <img src={thumb} alt="" loading="lazy" className="absolute left-1/2 -translate-x-1/2 h-full aspect-[2/3] object-cover" />
                                </>
                              )
                            )}
                            <span className="absolute top-1 left-1 px-1.5 py-px rounded text-[10px] font-bold text-white" style={{ background: 'rgba(0,0,0,0.72)' }}>E{ep.number}</span>
                            <div className="absolute inset-0 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity" style={{ background: 'rgba(0,0,0,0.4)' }}>
                              <span className="w-9 h-9 rounded-full flex items-center justify-center text-white" style={{ background: 'var(--accent)' }}>
                                {isOpen && loadingStreams ? <Loader2 size={15} className="animate-spin" /> : <Play size={15} fill="currentColor" className="translate-x-px" />}
                              </span>
                            </div>
                            {pct > 0 && (
                              <div className="absolute inset-x-0 bottom-0 h-1" style={{ background: 'rgba(255,255,255,0.22)' }}>
                                <div className="h-full" style={{ width: `${pct}%`, background: 'var(--accent)' }} />
                              </div>
                            )}
                          </div>
                          <div className="flex-1 min-w-0 py-0.5 flex flex-col">
                            <p className={`text-[12px] sm:text-[13px] break-words font-semibold leading-snug line-clamp-2 ${watched ? 'text-[color:var(--text-3)]' : 'text-[color:var(--text-1)] group-hover:text-white'}`}>
                              E{ep.number} – {ep.title || `Episode ${ep.number}`}
                            </p>
                            <div className="flex items-center gap-2 mt-auto text-[11px] text-[color:var(--text-3)]">
                              {ep.airDate && <span>{ep.airDate.slice(0, 10)}</span>}
                              {watched && <span className="flex items-center gap-0.5" style={{ color: 'var(--accent-hover)' }}><Check size={11} /> Watched</span>}
                              {inProgress && <span style={{ color: 'var(--accent-hover)' }}>{Math.round(pct)}% watched</span>}
                              <ChevronDown size={14} className={`ml-auto transition-transform ${isOpen ? 'rotate-180' : ''}`} />
                            </div>
                          </div>
                        </button>
                        {isOpen && (
                          <div className="col-span-full min-w-0 rounded-xl p-2 sm:p-3 max-h-[360px] overflow-y-auto" style={{ background: 'var(--surface-2)' }}>
                            {renderStreamPanel()}
                          </div>
                        )}
                      </div>
                    )
                  })}
                </div>
              )}

              {seasons.length > 0 && (
                <section className="mt-8">
                  <h3 className="text-[14px] font-bold text-[color:var(--text-1)] mb-2">Related Seasons</h3>
                  <div className="flex gap-2 flex-wrap">
                    {seasons.map((s) => (
                      <button
                        key={s.mal_id}
                        onClick={() => { setSelectedSeason(String(s.mal_id)); window.scrollTo?.(0, 0) }}
                        className="min-h-10 max-w-full px-3 py-1.5 rounded-lg text-xs text-left transition-colors hover:bg-white/10"
                        style={{
                          background: selectedSeason === String(s.mal_id) ? 'rgba(var(--accent-rgb),0.2)' : 'var(--surface-2)',
                          border: '1px solid var(--panel-border)',
                          color: 'var(--text-1)',
                        }}
                      >
                        <span className="block font-semibold truncate max-w-[70vw] sm:max-w-[260px]">{s.name}</span>
                        <span className="text-[10px] text-[color:var(--text-3)]">{s.relation}</span>
                      </button>
                    ))}
                  </div>
                </section>
              )}
            </div>
          )}

          {similarItems.length > 0 && (
            <div className="mt-8">
              <AnimeShelf
                title={mediaKind === 'movie' || mediaKind === 'series' ? 'More Like This' : 'Recommended For You'}
                itemCount={similarItems.length}
              >
                {similarItems.slice(0, 20).map((item) => (
                  <AnimeCard key={item.id} anime={item} onOpen={handleOpenSimilar} />
                ))}
              </AnimeShelf>
            </div>
          )}
        </div>
      </div>

      <BulkDownloadModal
        open={bulkOpen}
        onClose={() => setBulkOpen(false)}
        kind={effectiveKind}
        title={title || 'Unknown'}
        imdbId={imdbId || undefined}
        malId={effectiveKind === 'anime' && isNumeric(String(animeId)) ? String(animeId) : undefined}
        year={year != null ? String(year) : undefined}
        coverUrl={coverUrl}
        episodes={bulkEpisodes}
      />
    </div>
  )
}
