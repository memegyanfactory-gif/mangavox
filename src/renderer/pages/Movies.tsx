import { useState, useEffect, useLayoutEffect, useCallback, useRef, useMemo } from 'react'
import { useUiStore } from '../stores/ui.store'
import { useMediaStore } from '../stores/media.store'
import { invoke } from '../hooks/useIpc'
import { coverUrl } from '../utils/cover-url'
import { IPC } from '../../shared/ipc-channels'
import {
  Film, Search, Loader2, Tv, Upload, RotateCcw, Download, ChevronDown, X, Clapperboard, Star,
} from 'lucide-react'
import type { AnimeSearchResult } from '../../shared/anime-addon-interface'
import type { ImportMatch, MediaKind, MediaTitle, StreamingProvider } from '../../shared/types'
import { toast } from '../components/Toaster'
import { MediaPersonalStrips } from '../components/MediaPersonalStrips'
import { DownloadsPanel } from '../components/DownloadsPanel'
import { useQuickDownload } from '../hooks/useQuickDownload'
import { useDownloadsStore } from '../stores/downloads.store'
import { getSkin, titleKey, type Skin } from '../components/movies/skins'
import { Rail, ContinueRail, RailSkeleton, TitleGrid, type CardActions } from '../components/movies/MovieRails'
import { HeroBillboard } from '../components/movies/HeroBillboard'
import { TitleDetailModal } from '../components/movies/TitleDetailModal'

type Tab = 'top' | 'popular' | 'year' | 'imdbRating' | 'downloads'
type HubKind = 'movie' | 'series'

const PAGE_SIZE = 100 // Cinemeta returns 100 per page
const TMDB_PAGE_SIZE = 20 // TMDB discover (provider catalogs) returns 20 per page
const CATALOG_TTL_MS = 10 * 60 * 1000
const CATALOG_CACHE_MAX = 40

/**
 * Module-level catalog cache: survives the page unmounting, so returning from a
 * detail page renders instantly (no skeleton, no refetch) and can restore the
 * scroll position. Keyed by every parameter that changes the catalog request.
 */
interface CatalogSnapshot { results: AnimeSearchResult[]; page: number; hasMore: boolean; notice: string; at: number }
const catalogCache = new Map<string, CatalogSnapshot>()
const scrollMemory = new Map<string, number>()

function catalogKey(hubKind: HubKind, providerId: string, region: string, tab: Tab, genre: string | null): string {
  return [hubKind, providerId, region, tab, genre || ''].join('|')
}

function readCatalog(key: string): CatalogSnapshot | null {
  const snap = catalogCache.get(key)
  if (!snap) return null
  if (Date.now() - snap.at > CATALOG_TTL_MS) {
    catalogCache.delete(key)
    scrollMemory.delete(key)
    return null
  }
  return snap
}

function writeCatalog(key: string, snap: CatalogSnapshot): void {
  catalogCache.delete(key)
  catalogCache.set(key, snap)
  while (catalogCache.size > CATALOG_CACHE_MAX) {
    const oldest = catalogCache.keys().next().value
    if (oldest === undefined) break
    catalogCache.delete(oldest)
    scrollMemory.delete(oldest)
  }
}

/** Page sizes differ per backend: Cinemeta 100, TMDB discover 20, addon catalogs vary. */
function catalogHasMore(data: AnimeSearchResult[], prev: AnimeSearchResult[], providerId: string): boolean {
  if (data.length === 0) return false
  if (prev.length > 0) {
    const seen = new Set(prev.map((item) => item.id))
    if (data.every((item) => seen.has(item.id))) return false // backend ignored the page number
  }
  if (providerId === 'all') return data.length >= PAGE_SIZE
  if (data[0]?.catalogSource === 'tmdb') return data.length >= TMDB_PAGE_SIZE
  return data.length >= 10
}

const GENRE_CHIPS: string[] = [
  'Action', 'Adventure', 'Animation', 'Comedy', 'Crime', 'Documentary',
  'Drama', 'Family', 'Fantasy', 'History', 'Horror', 'Music', 'Mystery',
  'Romance', 'Sci-Fi', 'Sport', 'Thriller', 'War', 'Western',
]

function mediaToResult(item: MediaTitle): AnimeSearchResult {
  return {
    id: item.imdbId || item.id, imdbId: item.imdbId, title: item.title, coverUrl: item.coverUrl,
    bannerUrl: item.bannerUrl, year: item.year, genres: item.genres, description: item.description,
    score: item.score, animeType: item.kind === 'movie' ? 'Movie' : 'TV',
  }
}

export function Movies() {
  const { navigate, pageParams } = useUiStore()
  const shelf = pageParams.shelf as 'history' | 'favorites' | 'watched' | undefined
  const [hubKind, setHubKind] = useState<HubKind>(pageParams.hubKind || 'movie')
  const [providerId, setProviderId] = useState<string>(pageParams.providerId || 'all')
  const region = useMediaStore((s) => s.region)
  const [tab, setTab] = useState<Tab>(pageParams.tab || 'top')
  // Seed from the catalog cache so coming back from a detail page paints the
  // previous catalog on the first frame instead of a skeleton.
  const [initialSnap] = useState(() => (tab === 'downloads' ? null
    : readCatalog(catalogKey(hubKind, providerId, region, tab, pageParams.selectedGenre || null))))
  const [results, setResults] = useState<AnimeSearchResult[]>(initialSnap?.results ?? [])
  const [loading, setLoading] = useState(!initialSnap)
  const [searchQuery, setSearchQuery] = useState(pageParams.searchQuery || '')
  const [isSearching, setIsSearching] = useState(false)
  const [selectedGenre, setSelectedGenre] = useState<string | null>(pageParams.selectedGenre || null)

  // Pagination
  const [page, setPage] = useState(initialSnap?.page ?? 1)
  const [loadingMore, setLoadingMore] = useState(false)
  const [hasMore, setHasMore] = useState(initialSnap?.hasMore ?? true)
  const [catalogNotice, setCatalogNotice] = useState(initialSnap?.notice ?? '')
  const [loadError, setLoadError] = useState('')
  const requestRef = useRef(0)
  const resultsRef = useRef(results)
  resultsRef.current = results
  const scrollRef = useRef<HTMLDivElement>(null)

  // Detail modal + live search
  const [selected, setSelected] = useState<AnimeSearchResult | null>(null)
  const [liveResults, setLiveResults] = useState<AnimeSearchResult[]>([])
  const [liveOpen, setLiveOpen] = useState(false)
  const liveRef = useRef(0)

  const loadCatalog = useCallback(async (t: Tab, genre: string | null, pageNum: number) => {
    if (t === 'downloads') { setLoading(false); return } // local tab - nothing to fetch
    const key = catalogKey(hubKind, providerId, region, t, genre)
    if (pageNum === 1) {
      const snap = readCatalog(key)
      if (snap) {
        ++requestRef.current // drop any in-flight load for the previous view
        setResults(snap.results)
        setCatalogNotice(snap.notice)
        setHasMore(snap.hasMore)
        setPage(snap.page)
        setIsSearching(false)
        setLoadError('')
        setLoading(false)
        setLoadingMore(false)
        return
      }
    }
    const requestId = ++requestRef.current
    if (pageNum === 1) {
      setLoading(true)
      setIsSearching(false)
      setLoadError('')
    } else {
      setLoadingMore(true)
    }
    try {
      let data: AnimeSearchResult[]
      let notice = ''
      if (providerId === 'all') {
        data = await invoke<AnimeSearchResult[]>(
          hubKind === 'movie' ? IPC.MOVIE_CATALOG : IPC.SERIES_CATALOG, t, pageNum, genre || undefined
        )
      } else if (providerId === 'indian' || providerId === 'mx-player') {
        // MX Player is an Indian platform - route both to the Indian content addon
        const label = providerId === 'indian' ? 'Indian content' : 'MX Player'
        data = await invoke<AnimeSearchResult[]>(
          IPC.MEDIA_PROVIDER_CATALOG, 'indian', hubKind, pageNum, region, genre || undefined
        )
        notice = data.length === 0
          ? `${label} not available. Ensure Indian addons are installed.`
          : `${label} (${data.length} titles available${providerId === 'mx-player' ? ' in India' : ''})`
      } else {
        data = await invoke<AnimeSearchResult[]>(
          IPC.MEDIA_PROVIDER_CATALOG, providerId, hubKind, pageNum, region, genre || undefined
        )
        if (data.length === 0) {
          notice = providerId === 'prime-video'
            ? 'Prime Video content not available. Try "All Services" for broader availability.'
            : 'This service has no catalog data in your region.'
        } else {
          const source = data[0]?.catalogSource
          notice = source === 'addon'
            ? `Catalog from installed ${data[0].catalogSourceName || 'streaming addon'}`
            : source === 'tmdb'
              ? 'Current regional availability catalog'
              : 'Service-specific catalog'
        }
      }
      if (requestId !== requestRef.current) return
      const more = catalogHasMore(data, pageNum === 1 ? [] : resultsRef.current, providerId)
      if (pageNum === 1) {
        setResults(data)
        setCatalogNotice(notice)
        if (data.length > 0) writeCatalog(key, { results: data, page: 1, hasMore: more, notice, at: Date.now() })
      } else {
        setResults((prev) => [...prev, ...data])
        const snap = catalogCache.get(key)
        if (snap) writeCatalog(key, { ...snap, results: [...snap.results, ...data], page: pageNum, hasMore: more })
      }
      setHasMore(more)
      setPage(pageNum)
    } catch (err) {
      if (requestId !== requestRef.current) return
      console.error('[movies] Load failed:', err)
      if (pageNum === 1) setResults([])
      setLoadError(err instanceof Error ? err.message : 'The catalog could not be loaded.')
      setHasMore(false)
    } finally {
      setLoading(false)
      setLoadingMore(false)
    }
  }, [hubKind, providerId, region])

  useEffect(() => { void loadCatalog(tab, selectedGenre, 1) }, [loadCatalog, tab, selectedGenre])

  // Restore the scroll position when returning to a cached catalog (e.g. back from a detail page).
  const viewKey = catalogKey(hubKind, providerId, region, tab, selectedGenre)
  useLayoutEffect(() => {
    if (!initialSnap || !scrollRef.current) return
    const top = scrollMemory.get(viewKey)
    if (top) scrollRef.current.scrollTop = top
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // The effect above reloads whenever tab/genre change, so these handlers only
  // fetch directly when nothing they set would re-trigger it.
  const handleTabChange = (t: Tab) => {
    const reloadsViaEffect = t !== tab || selectedGenre !== null
    setTab(t)
    setSearchQuery('')
    setIsSearching(false)
    setSelectedGenre(null)
    setPage(1)
    setHasMore(true)
    if (!reloadsViaEffect) void loadCatalog(t, null, 1)
  }

  const handleGenreClick = (genre: string) => {
    // `next` always differs from selectedGenre, so the effect performs the load.
    const next = selectedGenre === genre ? null : genre
    setSelectedGenre(next)
    setSearchQuery('')
    setIsSearching(false)
    setLiveOpen(false)
    if (tab === 'downloads') setTab('top')
    setPage(1)
    setHasMore(true)
  }
  const genreClickRef = useRef(handleGenreClick)
  genreClickRef.current = handleGenreClick

  const handleSearch = useCallback(async () => {
    const q = searchQuery.trim()
    if (!q) return
    ++requestRef.current // a catalog load still in flight must not overwrite search results
    setLiveOpen(false)
    setLoading(true)
    setIsSearching(true)
    setPage(1)
    setHasMore(false) // Search API doesn't paginate
    if (tab === 'downloads') setTab('top')
    try {
      const data = await invoke<AnimeSearchResult[]>(hubKind === 'movie' ? IPC.MOVIE_SEARCH : IPC.SERIES_SEARCH, q)
      setResults(data)
    } catch (err) {
      console.error('[movies] Search failed:', err)
      setResults([])
    } finally {
      setLoading(false)
    }
  }, [searchQuery, hubKind, tab])

  const clearSearch = () => {
    setSearchQuery('')
    setLiveResults([])
    setLiveOpen(false)
    if (isSearching) void loadCatalog(tab, selectedGenre, 1)
  }

  // Live search suggestions while typing
  useEffect(() => {
    const q = searchQuery.trim()
    if (q.length < 2) { setLiveResults([]); return }
    const id = ++liveRef.current
    const timer = window.setTimeout(async () => {
      try {
        const data = await invoke<AnimeSearchResult[]>(hubKind === 'movie' ? IPC.MOVIE_SEARCH : IPC.SERIES_SEARCH, q)
        if (id === liveRef.current) setLiveResults((data || []).slice(0, 7))
      } catch { if (id === liveRef.current) setLiveResults([]) }
    }, 350)
    return () => window.clearTimeout(timer)
  }, [searchQuery, hubKind])

  const loadMore = useCallback(async () => {
    if (loadingMore || !hasMore || isSearching) return
    await loadCatalog(tab, selectedGenre, page + 1)
  }, [tab, selectedGenre, page, loadingMore, hasMore, isSearching, loadCatalog])

  const { quickDownload, resolving } = useQuickDownload()
  const activeDownloadCount = useDownloadsStore((s) =>
    Object.values(s.entries).filter((e) => e.status === 'downloading' || e.status === 'starting').length)

  const providers = useMediaStore((s) => s.providers)
  const allHistory = useMediaStore((s) => s.history)
  const liked = useMediaStore((s) => s.liked)
  const recommendations = useMediaStore((s) => s.recommendations)
  const profile = useMediaStore((s) => s.profile)
  const hydrateMedia = useMediaStore((s) => s.hydrate)
  const recordMedia = useMediaStore((s) => s.record)
  const removeHistory = useMediaStore((s) => s.removeHistory)
  const removeFavorite = useMediaStore((s) => s.removeFavorite)
  const clearShelf = useMediaStore((s) => s.clearShelf)
  const resetMedia = useMediaStore((s) => s.reset)
  const activeProvider: StreamingProvider | undefined = providers.find((provider) => provider.id === providerId)

  const skin: Skin = useMemo(() => {
    const s = getSkin(activeProvider)
    return providerId === 'indian' ? { ...s, label: 'MangaVox • Indian catalog' } : s
  }, [activeProvider, providerId])
  const providerMode = !!activeProvider
  const rowsFirstTop10 = skin.family === 'netflix' || skin.family === 'disney'

  const historyEntries = useMemo(() => allHistory.filter((entry) => entry.kind === hubKind), [allHistory, hubKind])
  const resumable = useMemo(
    () => historyEntries.filter((h) => !h.completed && h.positionSecs > 20 && h.durationSecs > 0),
    [historyEntries],
  )
  const watchedTitles = useMemo(() => historyEntries
    .filter((h) => h.completed || h.positionSecs > 30)
    .map((h) => ({
      id: h.mediaId, kind: hubKind, title: h.title, coverUrl: h.coverUrl, genres: [], languages: [],
    })), [historyEntries, hubKind])
  const likedKeys = useMemo(() => new Set(liked.map((l) => l.id)), [liked])

  useEffect(() => {
    void hydrateMedia(hubKind, providerId === 'all' ? undefined : providerId)
  }, [hubKind, providerId, hydrateMedia])

  const openMovie = async (picked: AnimeSearchResult, options: { autoPlay?: boolean } = {}) => {
    setSelected(null)
    // Only block on TMDB when we still need the IMDb id to look up streams —
    // otherwise the click would feel dead until TMDB answers.
    const movie = picked.tmdbId && !picked.imdbId
      ? (await invoke<AnimeSearchResult | null>(IPC.MEDIA_TMDB_DETAILS, picked.tmdbId, hubKind)) || picked
      : picked
    const kind: MediaKind = hubKind
    void recordMedia({ kind, type: 'open', providerId: providerId === 'all' ? undefined : providerId, title: {
      id: movie.imdbId ? `${kind}:imdb:${movie.imdbId}` : `${kind}:tmdb:${movie.tmdbId}`,
      kind, title: movie.title, imdbId: movie.imdbId, tmdbId: movie.tmdbId,
      coverUrl: movie.coverUrl, bannerUrl: movie.bannerUrl, year: movie.year, genres: movie.genres || [],
      languages: [], description: movie.description, score: movie.score,
    } })
    const returnRoute = {
      page: 'movies',
      params: { hubKind, providerId, tab, searchQuery, selectedGenre },
    }
    // The detail page auto-loads streams for movies. "Play" (autoPlay) starts
    // the best-ranked source straight away; opening a title lists them all.
    navigate('anime-detail', {
      autoPlay: options.autoPlay || undefined,
      animeId: movie.imdbId || movie.id, // tt1234567
      title: movie.title,
      coverUrl: movie.coverUrl,
      year: movie.year,
      status: movie.status,
      genres: movie.genres,
      description: movie.description,
      animeType: hubKind === 'movie' ? 'Movie' : 'TV',
      score: movie.score,
      mediaKind: hubKind,
      providerId: providerId === 'all' ? undefined : providerId,
      returnRoute,
    })
  }

  const importHistory = async () => {
    const result = await invoke<{ path: string | null; matches: ImportMatch[] }>(IPC.MEDIA_IMPORT_PICK, hubKind)
    if (!result.path || result.matches.length === 0) return
    const matched = result.matches.filter((item) => item.status === 'matched').length
    const review = result.matches.filter((item) => item.status === 'review').length
    const unmatched = result.matches.length - matched - review
    if (!window.confirm(
      `Import ${result.matches.length} entries?\n\n${matched} high-confidence matches\n${review} possible matches\n${unmatched} local title-only entries`
    )) return
    const source = providerId === 'all' ? 'import' : providerId
    const committed = await invoke<{ imported: number }>(IPC.MEDIA_IMPORT_COMMIT, source, hubKind, result.matches)
    await hydrateMedia(hubKind, providerId === 'all' ? undefined : providerId)
    toast.success('Viewing history imported', `${committed.imported} titles added to your profile.`)
  }

  const openOfficial = async (pid: string, title: string) => {
    await invoke(IPC.MEDIA_OPEN_PROVIDER, pid, title, 'IN')
  }

  const openFromMediaId = (mediaId: string) => {
    const fromHistory = allHistory.find((h) => h.mediaId === mediaId)
    const fromLiked = liked.find((l) => l.id === mediaId)
    const fromWatched = watchedTitles.find((w) => w.id === mediaId)
    if (fromLiked) { void openMovie(mediaToResult(fromLiked)); return }
    if (fromWatched) { void openMovie(mediaToResult(fromWatched)); return }
    const imdbId = mediaId.replace(`${hubKind}:imdb:`, '')
    void openMovie({
      id: imdbId, imdbId, title: fromHistory?.title || 'Title', coverUrl: fromHistory?.coverUrl,
    } as AnimeSearchResult)
  }

  const toggleList = (item: AnimeSearchResult) => {
    const key = titleKey(hubKind, item)
    if (likedKeys.has(key)) { void removeFavorite(key, hubKind); return }
    void recordMedia({
      kind: hubKind, type: 'like', providerId: providerId === 'all' ? undefined : providerId,
      title: {
        id: key, kind: hubKind, title: item.title, imdbId: item.imdbId, tmdbId: item.tmdbId,
        coverUrl: item.coverUrl, bannerUrl: item.bannerUrl, year: item.year, genres: item.genres || [],
        languages: [], description: item.description, score: item.score,
      },
    })
    toast.success('Added to My List', item.title)
  }

  const startDownload = (item: AnimeSearchResult) => {
    void quickDownload({
      id: item.id, title: item.title, coverUrl: item.coverUrl, imdbId: item.imdbId, category: hubKind,
    })
  }

  // Card handlers go through refs so `actions` keeps one identity across renders
  // (memoised rails/cards skip re-rendering) while still calling the latest
  // openMovie / toggleList / startDownload / openOfficial.
  const openMovieRef = useRef(openMovie)
  openMovieRef.current = openMovie
  const handlersRef = useRef({ toggleList, startDownload, openOfficial })
  handlersRef.current = { toggleList, startDownload, openOfficial }
  const activeProviderId = activeProvider?.id
  const actions: CardActions = useMemo(() => ({
    onDetails: setSelected,
    onPlay: (item) => void openMovieRef.current(item, { autoPlay: true }),
    onToggleList: (item) => handlersRef.current.toggleList(item),
    inList: (item) => likedKeys.has(titleKey(hubKind, item)),
    onDownload: (item) => handlersRef.current.startDownload(item),
    downloading: (item) => resolving.has(item.id),
    onOfficial: activeProviderId ? (item) => void handlersRef.current.openOfficial(activeProviderId, item.title) : undefined,
  }), [likedKeys, hubKind, resolving, activeProviderId])

  // Home rail slices (memoised so Rail/TitleCard memo holds across renders)
  const heroItems = useMemo(() => results.slice(0, 5), [results])
  const top10 = useMemo(() => results.slice(0, 10), [results])
  const trending = useMemo(() => results.slice(5, 27), [results])
  const forYou = useMemo(() => recommendations.filter((row) => row.id !== 'liked')
    .map((row) => ({ ...row, items: row.items.filter((i) => i.kind === hubKind).map(mediaToResult) }))
    .filter((row) => row.items.length > 0), [recommendations, hubKind])
  const myList = useMemo(() => liked.filter((l) => l.kind === hubKind).map(mediaToResult), [liked, hubKind])
  const genreRows = useMemo(() => {
    const map = new Map<string, AnimeSearchResult[]>()
    for (const item of results.slice(10)) {
      for (const g of (item.genres || []).slice(0, 2)) {
        const list = map.get(g) || []
        list.push(item)
        map.set(g, list)
      }
    }
    return [...map.entries()].filter(([, l]) => l.length >= 5).sort((a, b) => b[1].length - a[1].length).slice(0, 4)
  }, [results])
  const genreSeeMore = useMemo(
    () => new Map(genreRows.map(([genre]) => [genre, () => genreClickRef.current(genre)])),
    [genreRows],
  )

  if (shelf) {
    return (
      <div className="flex flex-col h-full page-enter px-3 sm:px-6 pt-4 pb-6 overflow-y-auto overflow-x-hidden">
        <h2 className="text-lg font-bold text-[color:var(--text-1)] mb-3">
          {shelf === 'history' ? 'Watch History' : shelf === 'favorites' ? 'My List' : 'Watched'}
        </h2>
        <MediaPersonalStrips
          kind={hubKind}
          history={shelf === 'history' ? allHistory : []}
          liked={shelf === 'favorites' ? liked : []}
          watched={shelf === 'watched' ? watchedTitles : []}
          onOpenHistory={openFromMediaId}
          onOpenLiked={(item) => openMovie(mediaToResult(item))}
          onOpenWatched={(item) => openMovie(mediaToResult(item))}
          onRemoveHistory={(id) => void removeHistory(id, hubKind)}
          onRemoveLiked={(id) => void removeFavorite(id, hubKind)}
          onRemoveWatched={(id) => void removeHistory(id, hubKind)}
          onClearHistory={() => void clearShelf('history', hubKind)}
          onClearLiked={() => void clearShelf('favorites', hubKind)}
          onClearWatched={() => void clearShelf('watched', hubKind)}
        />
      </div>
    )
  }

  const selectProvider = (id: string) => {
    setProviderId(id)
    setSearchQuery('')
    setIsSearching(false)
    setSelectedGenre(null)
    setTab('top')
    setPage(1)
  }

  const kindSwitch = (next: HubKind) => {
    setHubKind(next)
    setSearchQuery('')
    setIsSearching(false)
    setSelectedGenre(null)
    if (tab === 'downloads') setTab('top')
  }

  const navTabs: Array<{ id: Tab; label: string }> = providerMode
    ? [{ id: 'top', label: 'Home' }, { id: 'year', label: 'New & Popular' }, { id: 'downloads', label: activeDownloadCount > 0 ? `Downloads (${activeDownloadCount})` : 'Downloads' }]
    : [
      { id: 'top', label: 'Home' }, { id: 'popular', label: 'Popular' }, { id: 'imdbRating', label: 'Top rated' },
      { id: 'year', label: 'New' }, { id: 'downloads', label: activeDownloadCount > 0 ? `Downloads (${activeDownloadCount})` : 'Downloads' },
    ]

  const isHome = tab === 'top' && !selectedGenre && !isSearching
  const accentText = skin.accent === '#ffffff' || skin.accent === '#f5f5f7' ? '#fff' : skin.accent
  const accentSolid = skin.family === 'netflix' || skin.family === 'apple' || skin.family === 'poster'
    ? { background: '#fff', color: '#000' } : { background: skin.accent, color: skin.accentFg }

  const rowTop10 = (
    <Rail key="top10" title={skin.top10Title(hubKind)} items={top10} skin={skin} actions={actions} ranked limit={10} />
  )
  const rowTrending = (
    <Rail key="trending" title={providerMode ? skin.trendingTitle : `${skin.trendingTitle} in ${hubKind === 'movie' ? 'movies' : 'series'}`}
      items={trending} skin={skin} actions={actions} />
  )

  const tabTitle = isSearching ? `Results for "${searchQuery}"`
    : selectedGenre ? selectedGenre
      : tab === 'popular' ? 'Popular' : tab === 'imdbRating' ? 'Highest rated' : tab === 'year' ? 'New & recent' : 'Catalog'

  return (
    <div className="flex flex-col h-full page-enter" style={{
      background: skin.bgGradient ? `${skin.bgGradient}, ${skin.bg}` : skin.bg,
      color: skin.text,
    }}>
      {/* Header: catalog label, nav, kind switch and live search */}
      <div className="px-3 sm:px-6 pt-3 pb-1 flex justify-center shrink-0">
        <div className="max-w-[1600px] w-full min-w-0 flex flex-wrap lg:flex-nowrap items-center gap-x-3 sm:gap-x-5 gap-y-2">
          <div className="shrink min-w-0 leading-none">
            <p className="text-[17px] truncate" style={{ ...skin.wordmarkStyle, color: providerMode ? accentText : skin.text }}>
              {providerMode ? activeProvider!.name : 'Movies & Series'}
            </p>
            <p className="text-[9px] uppercase tracking-[0.2em] mt-1" style={{ color: skin.muted }}>{skin.label}</p>
          </div>
          <nav className="order-last lg:order-none w-full lg:w-auto flex items-center gap-1 text-[12px] font-medium overflow-x-auto scrollbar-none">
            {navTabs.map((n) => {
              const active = tab === n.id && !isSearching && !(n.id === 'top' && selectedGenre)
              return (
                <button key={n.id} onClick={() => handleTabChange(n.id)}
                  className="shrink-0 whitespace-nowrap px-3 py-1.5 min-h-[40px] lg:min-h-0 rounded-full transition-colors"
                  style={{
                    color: active ? skin.text : skin.muted,
                    background: active ? 'rgba(255,255,255,.1)' : 'transparent',
                    fontWeight: active ? 700 : 500,
                  }}>
                  {n.id === 'downloads' && <Download size={12} className="inline -mt-0.5 mr-1" />}{n.label}
                </button>
              )
            })}
          </nav>
          <div className="flex-1" />
          <div className="flex shrink-0 rounded-full p-0.5" style={{ background: 'rgba(255,255,255,.08)' }}>
            {([['movie', 'Films', <Film key="f" size={12} />], ['series', 'Series', <Tv key="s" size={12} />]] as const).map(([k, label, icon]) => (
              <button key={k} onClick={() => kindSwitch(k)} title={label} aria-label={label} className="flex items-center gap-1.5 px-3 py-1 min-h-[36px] sm:min-h-0 rounded-full text-[11.5px] font-semibold transition-colors"
                style={hubKind === k ? { ...accentSolid } : { color: skin.muted }}>
                {icon} <span className="hidden sm:inline">{label}</span>
              </button>
            ))}
          </div>
          <div className="relative order-last lg:order-none w-full lg:w-[280px]">
            <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2" style={{ color: skin.muted }} />
            <input
              data-stream-search
              type="text"
              value={searchQuery}
              onChange={(e) => { setSearchQuery(e.target.value); setLiveOpen(true) }}
              onFocus={() => setLiveOpen(true)}
              onBlur={() => window.setTimeout(() => setLiveOpen(false), 150)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void handleSearch()
                if (e.key === 'Escape') setLiveOpen(false)
              }}
              placeholder={`Search ${hubKind === 'movie' ? 'movies' : 'series'}...`}
              className="w-full h-10 lg:h-8 pl-9 pr-8 rounded-full text-[12px] outline-none"
              style={{ background: 'rgba(255,255,255,.08)', border: `1px solid ${skin.border}`, color: skin.text }}
            />
            {searchQuery && (
              <button onClick={clearSearch} aria-label="Clear search" className="absolute right-1 lg:right-2.5 top-1/2 -translate-y-1/2 w-9 h-9 lg:w-auto lg:h-auto flex items-center justify-center" style={{ color: skin.muted }}>
                <X size={13} />
              </button>
            )}
            {liveOpen && searchQuery.trim().length >= 2 && liveResults.length > 0 && (
              <div className="absolute right-0 left-0 top-full mt-1.5 z-50 rounded-xl overflow-hidden shadow-2xl"
                style={{ background: skin.bg.startsWith('var') ? 'var(--surface-2)' : skin.surface, border: `1px solid ${skin.border}` }}>
                {liveResults.map((r) => (
                  <button key={r.id} onMouseDown={(e) => e.preventDefault()}
                    onClick={() => { setLiveOpen(false); setSelected(r) }}
                    className="w-full flex items-center gap-2.5 px-2.5 py-1.5 text-left hover:bg-white/10 transition-colors">
                    <div className="w-8 h-11 rounded overflow-hidden shrink-0" style={{ background: 'rgba(255,255,255,.08)' }}>
                      {r.coverUrl && <img src={coverUrl(r.coverUrl) || ''} alt="" className="w-full h-full object-cover" />}
                    </div>
                    <div className="min-w-0">
                      <p className="text-[12px] font-semibold truncate" style={{ color: skin.text }}>{r.title}</p>
                      <p className="text-[10px] flex items-center gap-1.5" style={{ color: skin.muted }}>
                        {r.year}{r.score ? <><Star size={9} fill="currentColor" className="text-amber-400" />{r.score.toFixed(1)}</> : null}
                      </p>
                    </div>
                  </button>
                ))}
                <button onMouseDown={(e) => e.preventDefault()} onClick={() => void handleSearch()}
                  className="w-full px-3 py-1.5 text-[11px] font-semibold text-left border-t hover:bg-white/10" style={{ color: accentText, borderColor: skin.border }}>
                  See all results for "{searchQuery.trim()}"
                </button>
              </div>
            )}
          </div>
          {!providerMode && (
            <div className="flex items-center gap-1 shrink-0">
              <button onClick={importHistory} title="Import viewing history" className="p-2 sm:p-2 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center rounded-full hover:bg-white/10" style={{ color: skin.muted }}><Upload size={14} /></button>
              <button onClick={() => { void resetMedia(); toast.success('Personalization reset') }} title="Reset personalization"
                className="p-2 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center rounded-full hover:bg-white/10" style={{ color: skin.muted }}><RotateCcw size={14} /></button>
            </div>
          )}
        </div>
      </div>

      {/* Service switcher - plain text pills, no brand artwork */}
      <div className="px-3 sm:px-6 pb-1.5 flex justify-center shrink-0">
        <div className="max-w-[1600px] w-full min-w-0 flex items-center gap-1.5 overflow-x-auto" style={{ scrollbarWidth: 'none' }}>
          {[{ id: 'all', name: 'All services', color: 'var(--accent)' } as const,
            ...providers.map((p) => ({ id: p.id, name: p.name, color: p.theme.accent })),
            { id: 'indian', name: 'Indian content', color: '#FF6B35' } as const,
          ].map((p) => {
            const active = providerId === p.id
            return (
              <button key={p.id} onClick={() => selectProvider(p.id)}
                className="shrink-0 flex items-center gap-1.5 px-3 py-1 min-h-[36px] sm:min-h-0 rounded-full text-[11.5px] font-medium transition-all"
                style={{
                  background: active ? 'rgba(255,255,255,.14)' : 'transparent',
                  color: active ? skin.text : skin.muted,
                  border: `1px solid ${active ? 'rgba(255,255,255,.28)' : 'transparent'}`,
                }}>
                {p.id === 'all' ? <Clapperboard size={12} /> : <span className="w-2 h-2 rounded-full" style={{ background: p.color }} />}
                {p.name}
              </button>
            )
          })}
        </div>
      </div>

      {/* Genre chips */}
      <div className="px-3 sm:px-6 pb-2 flex justify-center shrink-0">
        <div className="max-w-[1600px] w-full min-w-0 flex gap-1.5 overflow-x-auto" style={{ scrollbarWidth: 'none' }}>
          {GENRE_CHIPS.map((g) => {
            const active = selectedGenre === g && !isSearching
            return (
              <button key={g} onClick={() => handleGenreClick(g)}
                className="shrink-0 px-2.5 py-0.5 min-h-[32px] sm:min-h-0 rounded-full text-[11px] font-medium transition-all"
                style={active
                  ? { ...accentSolid }
                  : { background: 'rgba(255,255,255,.07)', color: skin.muted }}>
                {g}
              </button>
            )
          })}
        </div>
      </div>

      {/* Content */}
      <div ref={scrollRef} className="flex-1 overflow-y-auto overflow-x-hidden px-3 sm:px-6 pb-10"
        onScroll={(e) => {
          // Plain Map write (no state) - remembered for when we come back to this catalog.
          if (!loading && !isSearching && tab !== 'downloads') scrollMemory.set(viewKey, e.currentTarget.scrollTop)
        }}>
        <div className="max-w-[1600px] mx-auto w-full pt-1">
          {catalogNotice && providerMode && tab !== 'downloads' && !loading && (
            <div className="mb-3 flex items-center gap-2 text-[10px]" style={{ color: skin.muted }}>
              <span className="w-1.5 h-1.5 rounded-full" style={{ background: skin.accent }} />
              <span>{catalogNotice}</span>
            </div>
          )}

          {tab === 'downloads' && !isSearching ? (
            <DownloadsPanel />
          ) : loading ? (
            <>
              <div className={`${skin.heroH} mb-6 animate-pulse`} style={{ background: skin.surface, borderRadius: skin.radius + 2 }} />
              <RailSkeleton skin={skin} />
            </>
          ) : loadError ? (
            <div className="flex flex-col items-center justify-center gap-3 py-20 text-center">
              <Film size={32} className="text-rose-400/60" />
              <p className="text-sm" style={{ color: skin.text }}>Catalog unavailable</p>
              <p className="text-xs max-w-md" style={{ color: skin.muted }}>{loadError}</p>
              <button onClick={() => void loadCatalog(tab, selectedGenre, 1)}
                className="px-4 py-2 rounded-lg text-xs font-semibold" style={accentSolid}>Try again</button>
            </div>
          ) : results.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-3 py-20">
              <Film size={32} style={{ color: skin.muted }} />
              <p className="text-sm" style={{ color: skin.muted }}>
                {isSearching
                  ? 'No results found'
                  : activeProvider
                    ? `No ${activeProvider.name} titles available right now`
                    : `No ${hubKind === 'movie' ? 'movies' : 'shows'} available`}
              </p>
              {providerMode && <button onClick={() => selectProvider('all')} className="px-4 py-2 rounded-lg text-xs bg-white/10">Browse all services</button>}
            </div>
          ) : isHome ? (
            <div>
              <HeroBillboard
                items={heroItems} skin={skin} kind={hubKind}
                onPlay={(i) => void openMovie(i, { autoPlay: true })} onInfo={setSelected}
                onToggleList={toggleList} inList={actions.inList}
                onOfficial={activeProvider ? (i) => void openOfficial(activeProvider.id, i.title) : undefined}
                officialName={activeProvider?.name}
              />
              {!providerMode && profile?.favoriteGenres?.length ? (
                <div className="mb-3 flex flex-wrap items-center gap-2 text-[11px]" style={{ color: skin.muted }}>
                  Your taste:
                  {profile.favoriteGenres.slice(0, 5).map((genre) => (
                    <span key={genre.name} className="px-2 py-0.5 rounded-full bg-white/8">{genre.name}</span>
                  ))}
                </div>
              ) : null}
              <ContinueRail entries={resumable} skin={skin} onResume={openFromMediaId} onRemove={(id) => void removeHistory(id, hubKind)} />
              {rowsFirstTop10 ? <>{rowTop10}{rowTrending}</> : <>{rowTrending}{rowTop10}</>}
              {!providerMode && forYou.map((row) => (
                <Rail key={row.id} title={row.title} subtitle={row.reason} items={row.items} skin={skin} actions={actions} />
              ))}
              {myList.length > 0 && (
                <Rail title="My List" items={myList} skin={skin} actions={actions} />
              )}
              {genreRows.map(([genre, items]) => (
                <Rail key={genre} title={providerMode ? `${genre} on ${activeProvider!.name}` : genre} items={items} skin={skin} actions={actions}
                  seeMore={genreSeeMore.get(genre)} />
              ))}
              <MediaPersonalStrips
                kind={hubKind}
                history={allHistory}
                liked={[]}
                watched={watchedTitles}
                onOpenHistory={openFromMediaId}
                onOpenLiked={(item) => openMovie(mediaToResult(item))}
                onOpenWatched={(item) => openMovie(mediaToResult(item))}
                onRemoveHistory={(id) => void removeHistory(id, hubKind)}
                onRemoveLiked={(id) => void removeFavorite(id, hubKind)}
                onRemoveWatched={(id) => void removeHistory(id, hubKind)}
                onClearHistory={() => void clearShelf('history', hubKind)}
                onClearLiked={() => void clearShelf('favorites', hubKind)}
                onClearWatched={() => void clearShelf('watched', hubKind)}
                onOpenDownloads={() => setTab('downloads')}
              />
              {hasMore && (
                <div className="flex justify-center mt-4">
                  <button onClick={loadMore} disabled={loadingMore}
                    className="px-6 py-2 rounded-full text-xs font-semibold disabled:opacity-60" style={accentSolid}>
                    {loadingMore ? 'Loading…' : 'Load more titles'}
                  </button>
                </div>
              )}
            </div>
          ) : (
            <>
              <h2 className="text-[18px] font-bold tracking-tight mb-3 mt-2" style={{ color: skin.text }}>{tabTitle}</h2>
              <TitleGrid items={results} skin={skin} actions={actions} />
              {hasMore && !isSearching && (
                <div className="flex justify-center mt-8">
                  <button onClick={loadMore} disabled={loadingMore}
                    className="flex items-center gap-2 px-6 py-2.5 rounded-full text-xs font-semibold disabled:opacity-60 bg-white/10 hover:bg-white/15 transition-colors"
                    style={{ color: skin.text }}>
                    {loadingMore ? <><Loader2 size={14} className="animate-spin" /> Loading...</> : <><ChevronDown size={14} /> Load more</>}
                  </button>
                </div>
              )}
            </>
          )}
        </div>
      </div>

      <TitleDetailModal
        item={selected}
        kind={hubKind}
        skin={skin}
        providers={providers}
        region={region}
        activeProvider={activeProvider}
        inList={selected ? likedKeys.has(titleKey(hubKind, selected)) : false}
        downloading={selected ? resolving.has(selected.id) : false}
        onClose={() => setSelected(null)}
        onPlay={(item) => void openMovie(item, { autoPlay: true })}
        onToggleList={toggleList}
        onDownload={startDownload}
        onOfficial={(pid, title) => void openOfficial(pid, title)}
      />
    </div>
  )
}
