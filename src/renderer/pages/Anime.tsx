import { useState, useEffect, useCallback, useMemo, useRef } from 'react'
import { useUiStore } from '../stores/ui.store'
import { useMediaStore } from '../stores/media.store'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import {
  MonitorPlay, Search, Loader2, TrendingUp, Tv, Clock, Sparkles, ChevronDown, Download, CalendarDays, Sun,
  Home as HomeIcon, History as HistoryIcon, Trophy, Flame, Shapes,
} from 'lucide-react'
import { AnimeCard } from '../components/anime/AnimeCard'
import { AnimeShelf, ShelfSkeleton } from '../components/anime/AnimeShelf'
import { ContinueCard } from '../components/anime/ContinueCard'
import type { AnimeSearchResult } from '../../shared/anime-addon-interface'
import { HeroCarousel, type HeroSlide } from '../components/HeroCarousel'
import { MediaPersonalStrips } from '../components/MediaPersonalStrips'
import { DownloadsPanel } from '../components/DownloadsPanel'
import { useQuickDownload } from '../hooks/useQuickDownload'
import { useDownloadsStore } from '../stores/downloads.store'
import type { MediaTitle } from '../../shared/types'

type Tab = 'home' | 'popular' | 'airing' | 'upcoming' | 'genre' | 'downloads' | 'nsfw' | 'season' | 'schedule'

const SEASONS = ['winter', 'spring', 'summer', 'fall'] as const
const WEEKDAYS = ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday'] as const
type Weekday = (typeof WEEKDAYS)[number]
function currentSeason(): (typeof SEASONS)[number] {
  const m = new Date().getMonth()
  return m < 3 ? 'winter' : m < 6 ? 'spring' : m < 9 ? 'summer' : 'fall'
}
function todayWeekday(): Weekday {
  return WEEKDAYS[(new Date().getDay() + 6) % 7]
}
type ScheduleItem = AnimeSearchResult & { broadcast?: string }

const PAGE_SIZE = 25

const GENRE_CHIPS: { id: number; name: string }[] = [
  { id: 1, name: 'Action' },
  { id: 2, name: 'Adventure' },
  { id: 4, name: 'Comedy' },
  { id: 8, name: 'Drama' },
  { id: 10, name: 'Fantasy' },
  { id: 14, name: 'Horror' },
  { id: 7, name: 'Mystery' },
  { id: 22, name: 'Romance' },
  { id: 24, name: 'Sci-Fi' },
  { id: 36, name: 'Slice of Life' },
  { id: 37, name: 'Supernatural' },
  { id: 41, name: 'Suspense' },
  { id: 62, name: 'Isekai' },
  { id: 27, name: 'Shounen' },
  { id: 25, name: 'Shoujo' },
  { id: 30, name: 'Sports' },
]

const CRUNCHY_ORANGE = 'var(--accent)'

export function Anime() {
  const { navigate, pageParams } = useUiStore()
  const shelf = pageParams.shelf as 'history' | 'favorites' | 'watched' | undefined
  const { quickDownload, resolving } = useQuickDownload()
  const activeDownloadCount = useDownloadsStore((s) =>
    Object.values(s.entries).filter((e) => e.status === 'downloading' || e.status === 'starting').length)
  const [tab, setTab] = useState<Tab>('home')
  const [results, setResults] = useState<AnimeSearchResult[]>([])
  const [loading, setLoading] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [isSearching, setIsSearching] = useState(false)
  const [selectedGenre, setSelectedGenre] = useState<number | null>(null)
  const [selectedGenreName, setSelectedGenreName] = useState('')
  const [format, setFormat] = useState<string>('')

  // Season browsing
  const [seasonYear, setSeasonYear] = useState<number>(new Date().getFullYear())
  const [seasonName, setSeasonName] = useState<(typeof SEASONS)[number]>(currentSeason())
  const seasonRef = useRef({ year: seasonYear, season: seasonName as string })
  seasonRef.current = { year: seasonYear, season: seasonName }
  // Airing schedule
  const [schedule, setSchedule] = useState<Partial<Record<Weekday, ScheduleItem[]>>>({})
  const [scheduleLoading, setScheduleLoading] = useState(false)
  const [scheduleDay, setScheduleDay] = useState<Weekday | 'all'>('all')
  const today = todayWeekday()

  // Home shelves
  const [home, setHome] = useState<{ top: AnimeSearchResult[]; topMore: AnimeSearchResult[]; airing: AnimeSearchResult[]; season: AnimeSearchResult[]; today: ScheduleItem[] } | null>(null)
  const [homeGenre, setHomeGenre] = useState<{ id: number; name: string }>(GENRE_CHIPS[0])
  const [homeGenreItems, setHomeGenreItems] = useState<AnimeSearchResult[] | null>(null)
  const homeLoadedRef = useRef(false)

  // Pagination
  const [page, setPage] = useState(1)
  const [loadingMore, setLoadingMore] = useState(false)
  const [hasMore, setHasMore] = useState(true)

  const loadTab = useCallback(async (t: Tab, genreId?: number, type?: string, pageNum = 1) => {
    if (pageNum === 1) {
      setLoading(true)
      setIsSearching(false)
    } else {
      setLoadingMore(true)
    }
    try {
      let data: AnimeSearchResult[]
      switch (t) {
        case 'home':
        case 'downloads':
          setLoading(false)
          setLoadingMore(false)
          return
        case 'popular':
          data = await invoke<AnimeSearchResult[]>(IPC.ANIME_TOP, pageNum, type)
          break
        case 'airing':
          data = await invoke<AnimeSearchResult[]>(IPC.ANIME_AIRING, pageNum)
          break
        case 'upcoming':
          data = await invoke<AnimeSearchResult[]>(IPC.ANIME_UPCOMING, pageNum)
          break
        case 'nsfw':
          data = await invoke<AnimeSearchResult[]>('anime:nsfw', pageNum)
          break
        case 'season':
          data = await invoke<AnimeSearchResult[]>(IPC.ANIME_SEASON, seasonRef.current.year, seasonRef.current.season, pageNum)
          break
        case 'schedule':
          setLoading(false)
          setLoadingMore(false)
          return
        case 'genre':
          data = genreId
            ? await invoke<AnimeSearchResult[]>(IPC.ANIME_BY_GENRE, genreId, pageNum, type)
            : []
          break
        default:
          data = []
      }
      if (pageNum === 1) {
        setResults(data)
      } else {
        setResults((prev) => [...prev, ...data])
      }
      setHasMore(data.length >= PAGE_SIZE)
      setPage(pageNum)
    } catch (err) {
      console.error('[anime] Load failed:', err)
      if (pageNum === 1) setResults([])
      setHasMore(false)
    } finally {
      setLoading(false)
      setLoadingMore(false)
    }
  }, [])

  const loadHome = useCallback(async () => {
    if (homeLoadedRef.current) return
    homeLoadedRef.current = true
    const day = todayWeekday()
    const [top, topMore, airing, season, today] = await Promise.all([
      invoke<AnimeSearchResult[]>(IPC.ANIME_TOP, 1).catch(() => [] as AnimeSearchResult[]),
      invoke<AnimeSearchResult[]>(IPC.ANIME_TOP, 2).catch(() => [] as AnimeSearchResult[]),
      invoke<AnimeSearchResult[]>(IPC.ANIME_AIRING, 1).catch(() => [] as AnimeSearchResult[]),
      invoke<AnimeSearchResult[]>(IPC.ANIME_SEASON, 'now', undefined, 1).catch(() => [] as AnimeSearchResult[]),
      invoke<ScheduleItem[]>(IPC.ANIME_SCHEDULE, day).catch(() => [] as ScheduleItem[]),
    ])
    const norm = <T,>(x: T[] | null | undefined) => (Array.isArray(x) ? x : [])
    setHome({ top: norm(top), topMore: norm(topMore), airing: norm(airing), season: norm(season), today: norm(today) })
  }, [])

  useEffect(() => {
    let alive = true
    setHomeGenreItems(null)
    invoke<AnimeSearchResult[]>(IPC.ANIME_BY_GENRE, homeGenre.id, 1)
      .then((d) => { if (alive) setHomeGenreItems(Array.isArray(d) ? d : []) })
      .catch(() => { if (alive) setHomeGenreItems([]) })
    return () => { alive = false }
  }, [homeGenre.id])

  // Initial load
  useEffect(() => { void loadHome() }, [loadHome])

  const handleTabChange = (t: Tab) => {
    setTab(t)
    setSearchQuery('')
    setIsSearching(false)
    setPage(1)
    setHasMore(true)
    if (t === 'home') { setSelectedGenre(null); return }
    if (t === 'schedule') { void loadSchedule(); return }
    if (t !== 'genre' && t !== 'downloads') {
      setSelectedGenre(null)
      loadTab(t, undefined, format || undefined)
    }
  }

  const scheduleRef = useRef(schedule)
  scheduleRef.current = schedule
  const loadSchedule = useCallback(async () => {
    setScheduleLoading(true)
    const order: Weekday[] = [today, ...WEEKDAYS.filter((d) => d !== today)]
    for (const day of order) {
      if (scheduleRef.current[day]) continue
      try {
        const items = await invoke<ScheduleItem[]>(IPC.ANIME_SCHEDULE, day)
        setSchedule((prev) => ({ ...prev, [day]: items || [] }))
      } catch { setSchedule((prev) => ({ ...prev, [day]: [] })) }
    }
    setScheduleLoading(false)
  }, [today])

  const changeSeason = (year: number, season: (typeof SEASONS)[number]) => {
    setTab('season')
    setSelectedGenre(null)
    setSearchQuery('')
    setIsSearching(false)
    setSeasonYear(year)
    setSeasonName(season)
    seasonRef.current = { year, season }
    setPage(1)
    setHasMore(true)
    void loadTab('season')
  }

  const handleGenreClick = (genreId: number, genreName: string) => {
    setTab('genre')
    setSelectedGenre(genreId)
    setSelectedGenreName(genreName)
    setSearchQuery('')
    setIsSearching(false)
    setPage(1)
    setHasMore(true)
    loadTab('genre', genreId, format || undefined)
  }

  const handleFormatChange = (f: string) => {
    setFormat(f)
    setPage(1)
    setHasMore(true)
    if (tab === 'home' || tab === 'season' || tab === 'airing' || tab === 'upcoming' || tab === 'schedule' || tab === 'downloads' || tab === 'nsfw' || isSearching) {
      setTab('popular'); setSelectedGenre(null); setSearchQuery(''); setIsSearching(false)
      loadTab('popular', undefined, f || undefined)
    } else if (tab === 'popular') loadTab('popular', undefined, f || undefined)
    else if (tab === 'genre' && selectedGenre) loadTab('genre', selectedGenre, f || undefined)
  }

  const handleSearch = useCallback(async () => {
    if (!searchQuery.trim()) return
    setLoading(true)
    setIsSearching(true)
    setPage(1)
    setHasMore(true)
    try {
      const data = await invoke<AnimeSearchResult[]>(IPC.ANIME_SEARCH, searchQuery.trim(), 1)
      setResults(data)
      setHasMore(data.length >= PAGE_SIZE)
    } catch (err) {
      console.error('[anime] Search failed:', err)
      setResults([])
      setHasMore(false)
    } finally {
      setLoading(false)
    }
  }, [searchQuery])

  const loadMore = useCallback(async () => {
    if (loadingMore || !hasMore) return
    const nextPage = page + 1
    setLoadingMore(true)

    try {
      let data: AnimeSearchResult[]

      if (isSearching && searchQuery.trim()) {
        data = await invoke<AnimeSearchResult[]>(IPC.ANIME_SEARCH, searchQuery.trim(), nextPage)
      } else if (tab === 'genre' && selectedGenre) {
        data = await invoke<AnimeSearchResult[]>(IPC.ANIME_BY_GENRE, selectedGenre, nextPage, format || undefined)
      } else {
        switch (tab) {
          case 'popular':
            data = await invoke<AnimeSearchResult[]>(IPC.ANIME_TOP, nextPage, format || undefined)
            break
          case 'airing':
            data = await invoke<AnimeSearchResult[]>(IPC.ANIME_AIRING, nextPage)
            break
          case 'upcoming':
            data = await invoke<AnimeSearchResult[]>(IPC.ANIME_UPCOMING, nextPage)
            break
          case 'season':
            data = await invoke<AnimeSearchResult[]>(IPC.ANIME_SEASON, seasonYear, seasonName, nextPage)
            break
          case 'nsfw':
            data = await invoke<AnimeSearchResult[]>('anime:nsfw', nextPage)
            break
          default:
            data = []
        }
      }

      setResults((prev) => [...prev, ...data])
      setHasMore(data.length >= PAGE_SIZE)
      setPage(nextPage)
    } catch (err) {
      console.error('[anime] Load more failed:', err)
      setHasMore(false)
    } finally {
      setLoadingMore(false)
    }
  }, [page, loadingMore, hasMore, tab, isSearching, searchQuery, selectedGenre, format, seasonYear, seasonName])

  const allHistory = useMediaStore((s) => s.history)
  const liked = useMediaStore((s) => s.liked)
  const historyEntries = allHistory.filter((entry) => entry.kind === 'anime')
  const watchedTitles: MediaTitle[] = historyEntries
    .filter((h) => h.completed || h.positionSecs > 30)
    .map((h) => ({
      id: h.mediaId, kind: 'anime' as const, title: h.title, coverUrl: h.coverUrl, genres: [], languages: ['Japanese'],
    }))
  const hydrateMedia = useMediaStore((s) => s.hydrate)
  const recordMedia = useMediaStore((s) => s.record)
  const removeHistory = useMediaStore((s) => s.removeHistory)
  const removeFavorite = useMediaStore((s) => s.removeFavorite)
  const clearShelf = useMediaStore((s) => s.clearShelf)
  useEffect(() => { void hydrateMedia('anime') }, [hydrateMedia])

  const listedIds = useMemo(() => liked.map((l) => l.id), [liked])
  const isListed = (a: { id: string }) => listedIds.includes(`anime:mal:${a.id}`)

  const heroPool = useMemo(() => {
    if (!home) return [] as AnimeSearchResult[]
    const seen = new Set<string>()
    const pool = [...home.airing.slice(0, 12), ...home.season.slice(0, 12), ...home.top.slice(0, 8)].filter((a) => {
      if (seen.has(a.id) || !a.coverUrl || !a.description) return false
      seen.add(a.id)
      return true
    })
    // Banner-bearing titles first, then highest score.
    return pool.sort((x, y) => (Number(!!y.bannerUrl) - Number(!!x.bannerUrl)) || ((y.score ?? 0) - (x.score ?? 0))).slice(0, 5)
  }, [home])

  const heroSlides: HeroSlide[] = heroPool.map((a) => ({
    id: a.id,
    title: a.title,
    description: a.description,
    imageUrl: a.bannerUrl || a.coverUrl,
    imageIsPoster: !a.bannerUrl,
    badge: a.animeType === 'TV' ? 'NEW EPISODES' : a.animeType || 'ANIME',
    meta: [a.year, a.animeType, a.episodeCount ? `${a.episodeCount} eps` : ''].filter(Boolean).join(' | '),
    genres: a.genres,
    score: a.score,
    kind: 'anime' as const,
  }))

  const addToList = (anime: AnimeSearchResult) => {
    if (isListed(anime)) return
    const mediaId = `anime:mal:${anime.id}`
    void recordMedia({ kind: 'anime', type: 'like', title: {
      id: mediaId, kind: 'anime', title: anime.title, malId: Number(anime.id),
      coverUrl: anime.coverUrl, bannerUrl: anime.bannerUrl, year: anime.year, genres: anime.genres || [],
      languages: ['Japanese'], description: anime.description, score: anime.score,
    } })
    void invoke(IPC.WATCHLIST_SET, mediaId, 'anime', 'planned')
  }

  const downloadAnime = (anime: AnimeSearchResult) => {
    void quickDownload({
      id: String(anime.id), title: anime.title, coverUrl: anime.coverUrl,
      imdbId: anime.imdbId, malId: String(anime.id), category: 'anime',
    })
  }

  const openAnime = (anime: AnimeSearchResult) => {
    void recordMedia({ kind: 'anime', type: 'open', title: {
      id: `anime:mal:${anime.id}`, kind: 'anime', title: anime.title, malId: Number(anime.id),
      coverUrl: anime.coverUrl, bannerUrl: anime.bannerUrl, year: anime.year, genres: anime.genres || [],
      languages: ['Japanese'], description: anime.description, score: anime.score,
    } })
    navigate('anime-detail', {
      animeId: anime.id,
      title: anime.title,
      coverUrl: anime.coverUrl,
      year: anime.year,
      status: anime.status,
      genres: anime.genres,
      description: anime.description,
      animeType: anime.animeType,
      episodeCount: anime.episodeCount,
      score: anime.score,
    })
  }

  const openFromMediaId = (mediaId: string) => {
    const fromHistory = allHistory.find((h) => h.mediaId === mediaId)
    const fromLiked = liked.find((l) => l.id === mediaId)
    const fromWatched = watchedTitles.find((w) => w.id === mediaId)
    const item = fromHistory
      ? { id: mediaId.replace('anime:mal:', ''), title: fromHistory.title, coverUrl: fromHistory.coverUrl }
      : fromLiked
        ? { id: String(fromLiked.malId || mediaId.replace('anime:mal:', '')), title: fromLiked.title, coverUrl: fromLiked.coverUrl, genres: fromLiked.genres, year: fromLiked.year, description: fromLiked.description, score: fromLiked.score }
        : fromWatched
          ? { id: String(fromWatched.malId || mediaId.replace('anime:mal:', '')), title: fromWatched.title, coverUrl: fromWatched.coverUrl, genres: fromWatched.genres, year: fromWatched.year, description: fromWatched.description, score: fromWatched.score }
          : null
    if (!item?.title) {
      openAnime({ id: mediaId.replace('anime:mal:', ''), title: 'Anime', coverUrl: item?.coverUrl } as AnimeSearchResult)
      return
    }
    openAnime(item as AnimeSearchResult)
  }

  if (shelf) {
    return (
      <div className="flex flex-col h-full page-enter px-4 sm:px-5 pt-3 pb-6 overflow-y-auto overflow-x-hidden" style={{ background: 'var(--base)' }}>
        <h2 className="text-base font-bold text-white mb-3">
          {shelf === 'history' ? 'Watch History' : shelf === 'favorites' ? 'Favorites' : 'Watched'}
        </h2>
        <MediaPersonalStrips
          kind="anime"
          history={shelf === 'history' ? allHistory : []}
          liked={shelf === 'favorites' ? liked : []}
          watched={shelf === 'watched' ? watchedTitles : []}
          onOpenHistory={openFromMediaId}
          onOpenLiked={(item) => openAnime({ id: String(item.malId || item.id.replace('anime:mal:', '')), title: item.title, coverUrl: item.coverUrl } as AnimeSearchResult)}
          onOpenWatched={(item) => openAnime({ id: String(item.malId || item.id.replace('anime:mal:', '')), title: item.title, coverUrl: item.coverUrl } as AnimeSearchResult)}
          onRemoveHistory={(id) => void removeHistory(id, 'anime')}
          onRemoveLiked={(id) => void removeFavorite(id, 'anime')}
          onRemoveWatched={(id) => void removeHistory(id, 'anime')}
          onClearHistory={() => void clearShelf('history', 'anime')}
          onClearLiked={() => void clearShelf('favorites', 'anime')}
          onClearWatched={() => void clearShelf('watched', 'anime')}
        />
      </div>
    )
  }

  const continueEntries = historyEntries
    .filter((h) => !h.completed)
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .slice(0, 20)

  const topRated = (() => {
    if (!home) return [] as AnimeSearchResult[]
    const seen = new Set<string>()
    return [...home.top, ...home.topMore]
      .filter((a) => (seen.has(a.id) ? false : (seen.add(a.id), true)))
      .sort((a, b) => (b.score ?? 0) - (a.score ?? 0))
      .slice(0, 24)
  })()

  // Last 10 seasons + the upcoming one for the season dropdown.
  const seasonOptions = (() => {
    const out: { year: number; season: (typeof SEASONS)[number] }[] = []
    let y = new Date().getFullYear()
    let i = SEASONS.indexOf(currentSeason()) + 1
    if (i > 3) { i = 0; y++ }
    for (let n = 0; n < 12; n++) {
      out.push({ year: y, season: SEASONS[i] })
      i--
      if (i < 0) { i = 3; y-- }
    }
    return out
  })()

  const selectCls = 'h-10 sm:h-7 w-full sm:w-auto min-w-0 px-2 rounded-md text-[11px] outline-none text-[color:var(--text-1)] cursor-pointer'
  const selectStyle = { background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }
  const onHome = tab === 'home' && !isSearching

  const renderCard = (anime: AnimeSearchResult, extra?: { badge?: string; fluid?: boolean }) => (
    <AnimeCard
      key={`${anime.id}-${extra?.badge ?? ''}`}
      anime={anime}
      onOpen={openAnime}
      onAdd={addToList}
      listed={isListed(anime)}
      badge={extra?.badge}
      fluid={extra?.fluid}
      onDownload={downloadAnime}
      downloading={resolving.has(String(anime.id))}
    />
  )

  return (
    <div className="flex flex-col h-full page-enter" style={{ background: 'var(--base)' }}>
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        {onHome && home && heroSlides.length > 0 && (
          <HeroCarousel
            cinematic
            slides={heroSlides}
            accent={CRUNCHY_ORANGE}
            playLabel="Start Watching"
            addLabel="Add to My List"
            addedIds={heroPool.filter(isListed).map((a) => a.id)}
            onPlay={(slide) => { const a = heroPool.find((r) => r.id === slide.id); if (a) openAnime(a) }}
            onInfo={(slide) => { const a = heroPool.find((r) => r.id === slide.id); if (a) openAnime(a) }}
            onAdd={(slide) => { const a = heroPool.find((r) => r.id === slide.id); if (a) addToList(a) }}
          />
        )}

        {/* Sticky browse + filter bar */}
        <div
          className="sm:sticky top-0 z-30 px-4 sm:px-5 py-2 flex justify-center backdrop-blur-md"
          style={{ background: 'color-mix(in srgb, var(--base) 90%, transparent)', borderBottom: '1px solid var(--panel-border)' }}
        >
          <div className="max-w-[1600px] w-full min-w-0 flex items-center gap-x-3 gap-y-1.5 flex-wrap">
            <div className="relative w-full sm:w-56">
              <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[color:var(--text-3)]" />
              <input
                type="text"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
                placeholder="Search anime..."
                className="w-full h-10 sm:h-8 pl-8 pr-3 rounded-lg text-xs text-[color:var(--text-1)] placeholder-zinc-600 outline-none"
                style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
              />
            </div>
            <div className="flex items-center gap-1 w-full sm:w-auto min-w-0 overflow-x-auto sm:overflow-visible sm:flex-wrap snap-x" style={{ scrollbarWidth: 'none' }}>
              {([
                { id: 'home' as Tab, label: 'Home', icon: <HomeIcon size={12} /> },
                { id: 'popular' as Tab, label: 'Popular', icon: <TrendingUp size={12} /> },
                { id: 'airing' as Tab, label: 'Airing Now', icon: <Tv size={12} /> },
                { id: 'upcoming' as Tab, label: 'Upcoming', icon: <Clock size={12} /> },
                { id: 'season' as Tab, label: 'Seasons', icon: <Sun size={12} /> },
                { id: 'schedule' as Tab, label: 'Schedule', icon: <CalendarDays size={12} /> },
                { id: 'nsfw' as Tab, label: '18+', icon: <Sparkles size={12} /> },
                { id: 'downloads' as Tab, label: activeDownloadCount > 0 ? `Downloads (${activeDownloadCount})` : 'Downloads', icon: <Download size={12} /> },
              ]).map(({ id, label, icon }) => {
                const active = tab === id && !isSearching
                return (
                  <button
                    key={id}
                    onClick={() => handleTabChange(id)}
                    className={`shrink-0 snap-start min-h-10 sm:min-h-0 flex items-center gap-1 px-2.5 py-1 rounded-md text-[11px] font-medium whitespace-nowrap transition-all ${
                      active ? 'text-white' : 'text-[color:var(--text-3)] hover:text-[color:var(--text-1)]'
                    }`}
                    style={active ? { background: 'rgba(var(--accent-rgb),0.25)', color: '#fff' } : {}}
                  >
                    {icon} {label}
                  </button>
                )
              })}
              {isSearching && (
                <span className="shrink-0 max-w-[60vw] truncate flex items-center gap-1 px-2.5 py-1 rounded-md text-[11px] font-medium text-white" style={{ background: 'rgba(var(--accent-rgb),0.15)' }}>
                  <Search size={12} /> "{searchQuery}"
                </span>
              )}
            </div>
            <div className="grid grid-cols-3 gap-1.5 w-full sm:w-auto sm:flex sm:items-center sm:ml-auto">
              <select
                aria-label="Genre"
                value={tab === 'genre' && !isSearching && selectedGenre ? selectedGenre : ''}
                onChange={(e) => {
                  const id = Number(e.target.value)
                  const g = GENRE_CHIPS.find((c) => c.id === id)
                  if (g) handleGenreClick(g.id, g.name)
                  else handleTabChange('home')
                }}
                className={selectCls}
                style={selectStyle}
              >
                <option value="">Genre: All</option>
                {GENRE_CHIPS.map((g) => <option key={g.id} value={g.id}>{g.name}</option>)}
              </select>
              <select
                aria-label="Season"
                value={tab === 'season' && !isSearching ? `${seasonYear}|${seasonName}` : ''}
                onChange={(e) => {
                  const [y, sn] = e.target.value.split('|')
                  if (y && sn) changeSeason(Number(y), sn as (typeof SEASONS)[number])
                }}
                className={`${selectCls} capitalize`}
                style={selectStyle}
              >
                <option value="">Season: Any</option>
                {tab === 'season' && !seasonOptions.some((o) => o.year === seasonYear && o.season === seasonName) && (
                  <option value={`${seasonYear}|${seasonName}`}>{seasonName} {seasonYear}</option>
                )}
                {seasonOptions.map((o) => (
                  <option key={`${o.year}|${o.season}`} value={`${o.year}|${o.season}`}>{o.season[0].toUpperCase() + o.season.slice(1)} {o.year}</option>
                ))}
              </select>
              <select
                aria-label="Format"
                value={format}
                onChange={(e) => handleFormatChange(e.target.value)}
                className={`${selectCls} uppercase`}
                style={selectStyle}
              >
                <option value="">Format: All</option>
                {['tv', 'movie', 'ova', 'ona', 'special'].map((f) => <option key={f} value={f}>{f}</option>)}
              </select>
            </div>
          </div>
        </div>

        {tab === 'season' && !isSearching && (
          <div className="px-4 sm:px-5 mt-2 flex justify-center">
            <div className="max-w-[1600px] w-full flex items-center gap-1.5 flex-wrap">
              <span className="text-[10px] uppercase tracking-wider text-[color:var(--text-3)] mr-1">Season</span>
              {SEASONS.map((sn) => (
                <button
                  key={sn}
                  onClick={() => changeSeason(seasonYear, sn)}
                  className="min-h-10 sm:min-h-0 px-3 sm:px-2.5 py-0.5 rounded-md text-[11px] font-medium capitalize transition-colors"
                  style={seasonName === sn ? { background: 'rgba(var(--accent-rgb),0.25)', color: '#fff' } : { background: 'var(--surface-2)', color: 'var(--text-3)' }}
                >
                  {sn}
                </button>
              ))}
              <select
                value={seasonYear}
                onChange={(e) => changeSeason(Number(e.target.value), seasonName)}
                className="ml-2 h-10 sm:h-6 px-2 rounded-md text-[11px] outline-none text-[color:var(--text-1)]"
                style={selectStyle}
              >
                {Array.from({ length: new Date().getFullYear() + 2 - 1980 }, (_, i) => new Date().getFullYear() + 1 - i).map((y) => (
                  <option key={y} value={y}>{y}</option>
                ))}
              </select>
            </div>
          </div>
        )}

        <div className="px-4 sm:px-5 pt-3 pb-8">
          <div className="max-w-[1600px] mx-auto w-full">
            {onHome ? (
              !home ? (
                <>
                  <ShelfSkeleton title="New This Season" />
                  <ShelfSkeleton title="Popular" />
                </>
              ) : (
                <>
                  {continueEntries.length > 0 && (
                    <AnimeShelf
                      title="Continue Watching"
                      icon={<HistoryIcon size={15} style={{ color: 'var(--accent-hover)' }} />}
                      itemCount={continueEntries.length}
                      actionLabel="History"
                      onAction={() => navigate('anime', { shelf: 'history' })}
                    >
                      {continueEntries.map((h) => (
                        <ContinueCard key={h.mediaId} entry={h} onOpen={openFromMediaId} onRemove={(id) => void removeHistory(id, 'anime')} />
                      ))}
                    </AnimeShelf>
                  )}

                  {(home.season.length > 0 || home.airing.length > 0) && (
                    <AnimeShelf
                      title="New This Season"
                      icon={<Sun size={15} style={{ color: 'var(--accent-hover)' }} />}
                      itemCount={(home.season.length || home.airing.length)}
                      actionLabel="Browse seasons"
                      onAction={() => changeSeason(seasonYear, seasonName)}
                    >
                      {(home.season.length > 0 ? home.season : home.airing).slice(0, 24).map((a) => renderCard(a))}
                    </AnimeShelf>
                  )}

                  {home.top.length > 0 && (
                    <AnimeShelf
                      title="Popular"
                      icon={<Flame size={15} style={{ color: 'var(--accent-hover)' }} />}
                      itemCount={home.top.length}
                      onAction={() => handleTabChange('popular')}
                    >
                      {home.top.slice(0, 24).map((a) => renderCard(a))}
                    </AnimeShelf>
                  )}

                  {topRated.length > 0 && (
                    <AnimeShelf
                      title="Top Rated"
                      icon={<Trophy size={15} className="text-amber-400" />}
                      itemCount={topRated.length}
                      onAction={() => handleTabChange('popular')}
                    >
                      {topRated.map((a) => renderCard(a))}
                    </AnimeShelf>
                  )}

                  {home.today.length > 0 && (
                    <AnimeShelf
                      title="Airing Today"
                      icon={<CalendarDays size={15} style={{ color: 'var(--accent-hover)' }} />}
                      itemCount={home.today.length}
                      actionLabel="Full schedule"
                      onAction={() => handleTabChange('schedule')}
                    >
                      {home.today.slice(0, 24).map((a) => renderCard(a, { badge: a.broadcast ? a.broadcast.replace(/^\w+s at /i, '').replace(/\s*\(.*\)/, '') : 'Today' }))}
                    </AnimeShelf>
                  )}

                  <div className="flex items-center gap-2 mb-1.5 px-1">
                    <Shapes size={15} style={{ color: 'var(--accent-hover)' }} />
                    <h3 className="text-[14px] font-bold tracking-tight text-[color:var(--text-1)]">Browse by Genre</h3>
                  </div>
                  <div className="flex gap-1.5 overflow-x-auto snap-x snap-mandatory sm:snap-none overscroll-x-contain pb-2 px-1 mb-1" style={{ scrollbarWidth: 'none' }}>
                    {GENRE_CHIPS.map((g) => {
                      const active = homeGenre.id === g.id
                      return (
                        <button
                          key={g.id}
                          onClick={() => setHomeGenre(g)}
                          className="shrink-0 snap-start min-h-10 sm:min-h-0 px-3.5 sm:px-3 py-1 rounded-full text-[11px] font-semibold whitespace-nowrap transition-colors"
                          style={active
                            ? { background: 'var(--accent)', color: '#fff' }
                            : { background: 'var(--surface-2)', color: 'var(--text-2)', border: '1px solid var(--panel-border)' }}
                        >
                          {g.name}
                        </button>
                      )
                    })}
                  </div>
                  {homeGenreItems === null ? (
                    <ShelfSkeleton title={homeGenre.name} />
                  ) : homeGenreItems.length > 0 ? (
                    <AnimeShelf
                      title={homeGenre.name}
                      itemCount={homeGenreItems.length}
                      onAction={() => handleGenreClick(homeGenre.id, homeGenre.name)}
                    >
                      {homeGenreItems.slice(0, 24).map((a) => renderCard(a))}
                    </AnimeShelf>
                  ) : null}

                  {home.top.length === 0 && home.season.length === 0 && home.airing.length === 0 && (
                    <div className="flex flex-col items-center justify-center gap-3 py-20">
                      <MonitorPlay size={32} className="text-[color:var(--text-3)]" />
                      <p className="text-sm text-[color:var(--text-3)]">No anime available right now.</p>
                    </div>
                  )}

                  <div className="mt-2">
                    <MediaPersonalStrips
                      kind="anime"
                      history={[]}
                      liked={liked}
                      watched={watchedTitles}
                      onOpenHistory={openFromMediaId}
                      onOpenLiked={(item) => openAnime({ id: String(item.malId || item.id.replace('anime:mal:', '')), title: item.title, coverUrl: item.coverUrl } as AnimeSearchResult)}
                      onOpenWatched={(item) => openAnime({ id: String(item.malId || item.id.replace('anime:mal:', '')), title: item.title, coverUrl: item.coverUrl } as AnimeSearchResult)}
                      onRemoveHistory={(id) => void removeHistory(id, 'anime')}
                      onRemoveLiked={(id) => void removeFavorite(id, 'anime')}
                      onRemoveWatched={(id) => void removeHistory(id, 'anime')}
                      onClearHistory={() => void clearShelf('history', 'anime')}
                      onClearLiked={() => void clearShelf('favorites', 'anime')}
                      onClearWatched={() => void clearShelf('watched', 'anime')}
                      onOpenDownloads={() => setTab('downloads')}
                    />
                  </div>
                </>
              )
            ) : tab === 'downloads' && !isSearching ? (
              <DownloadsPanel accent="#c8501a" />
            ) : tab === 'schedule' && !isSearching ? (
              <div className="pb-4">
                <div className="flex items-center gap-1.5 mb-3 flex-wrap">
                  {(['all', ...WEEKDAYS] as const).map((d) => (
                    <button
                      key={d}
                      onClick={() => setScheduleDay(d)}
                      className="min-h-10 sm:min-h-0 px-3 sm:px-2.5 py-0.5 rounded-md text-[11px] font-medium capitalize transition-colors"
                      style={scheduleDay === d ? { background: 'rgba(var(--accent-rgb),0.25)', color: '#fff' } : { background: 'var(--surface-2)', color: 'var(--text-3)' }}
                    >
                      {d === 'all' ? 'All week' : d.slice(0, 3)}{d === today ? ' •' : ''}
                    </button>
                  ))}
                  {scheduleLoading && <Loader2 size={13} className="animate-spin ml-2" style={{ color: 'var(--accent)' }} />}
                </div>
                {(scheduleDay === 'all' ? [...WEEKDAYS] : [scheduleDay]).map((day) => {
                  const items = schedule[day]
                  const isToday = day === today
                  return (
                    <section key={day} className="mb-5">
                      <div className="flex items-center gap-2 mb-2">
                        <h3 className="text-xs font-bold capitalize" style={{ color: isToday ? 'var(--accent)' : 'var(--text-1)' }}>{day}</h3>
                        {isToday && <span className="px-1.5 py-px rounded text-[9px] font-bold uppercase text-white" style={{ background: 'var(--accent)' }}>Today</span>}
                        {items && <span className="text-[10px] text-[color:var(--text-3)]">{items.length}</span>}
                      </div>
                      {!items ? (
                        <div className="h-24 rounded-xl animate-pulse" style={{ background: 'var(--surface-2)' }} />
                      ) : items.length === 0 ? (
                        <p className="text-[11px] text-[color:var(--text-3)]">Nothing scheduled.</p>
                      ) : (
                        <div
                          className="grid grid-cols-2 min-[420px]:grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8 gap-3 p-2 rounded-xl"
                          style={isToday ? { background: 'rgba(var(--accent-rgb),0.07)', border: '1px solid rgba(var(--accent-rgb),0.35)' } : undefined}
                        >
                          {items.map((anime) => renderCard(anime, {
                            fluid: true,
                            badge: anime.broadcast ? anime.broadcast.replace(/^\w+s at /i, '').replace(/\s*\(.*\)/, '') : undefined,
                          }))}
                        </div>
                      )}
                    </section>
                  )
                })}
              </div>
            ) : loading ? (
              <div className="flex items-center justify-center py-12">
                <Loader2 size={20} className="animate-spin" style={{ color: 'var(--accent)' }} />
              </div>
            ) : results.length === 0 ? (
              <div className="flex flex-col items-center justify-center gap-3 py-20">
                <MonitorPlay size={32} className="text-[color:var(--text-3)]" />
                <p className="text-sm text-[color:var(--text-3)]">
                  {isSearching ? 'No results found' : 'No anime available'}
                </p>
              </div>
            ) : (
              <>
                <h3 className="text-[14px] font-bold tracking-tight text-[color:var(--text-1)] mb-2 px-1">
                  {isSearching ? `Results for "${searchQuery}"`
                    : tab === 'genre' ? selectedGenreName
                    : tab === 'season' ? `${seasonName[0].toUpperCase()}${seasonName.slice(1)} ${seasonYear}`
                    : tab === 'airing' ? 'Airing Now'
                    : tab === 'upcoming' ? 'Upcoming'
                    : tab === 'nsfw' ? '18+'
                    : 'Popular'}
                </h3>
                <div className="grid grid-cols-2 min-[420px]:grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8 gap-x-3 gap-y-4">
                  {results.map((anime, idx) => (
                    <div key={`${anime.id}-${idx}`}>{renderCard(anime, { fluid: true })}</div>
                  ))}
                </div>

                {hasMore && (
                  <div className="flex justify-center mt-5">
                    <button
                      onClick={loadMore}
                      disabled={loadingMore}
                      className="flex items-center gap-2 min-h-10 px-5 py-1.5 rounded-lg text-xs font-medium text-[color:var(--text-1)] hover:text-white transition-all disabled:opacity-50"
                      style={{ background: 'rgba(var(--accent-rgb),0.1)', border: '1px solid rgba(var(--accent-rgb),0.2)' }}
                    >
                      {loadingMore ? (
                        <>
                          <Loader2 size={14} className="animate-spin" /> Loading...
                        </>
                      ) : (
                        <>
                          <ChevronDown size={14} /> Load More
                        </>
                      )}
                    </button>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
