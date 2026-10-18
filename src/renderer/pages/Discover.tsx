import { useState, useEffect, useCallback, useRef } from 'react'
import {
  motion, useMotionValue, useTransform, AnimatePresence,
  animate, useMotionTemplate,
} from 'framer-motion'
import { useUiStore } from '../stores/ui.store'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import {
  X, Heart, RotateCcw, Star, Loader2, Play, ChevronLeft,
  Tv, Film, Shuffle, AlertTriangle,
} from 'lucide-react'
import type { AnimeSearchResult, AnimeTorrentStream, AnimeAddonInfo } from '../../shared/anime-addon-interface'
import { useMediaStore } from '../stores/media.store'
import type { MediaKind } from '../../shared/types'

// ── Helpers ───────────────────────────────────────────────────────────────────
function discoverTitle(item: AnimeSearchResult, kind: MediaKind) {
  return {
    id: kind === 'anime' ? `anime:mal:${item.id}` : `${kind}:imdb:${item.id}`,
    kind,
    title: item.title,
    imdbId: kind === 'anime' ? undefined : item.imdbId || item.id,
    malId: kind === 'anime' ? Number(item.id) : undefined,
    coverUrl: item.coverUrl,
    bannerUrl: item.bannerUrl,
    year: item.year,
    genres: item.genres || [],
    languages: kind === 'anime' ? ['Japanese'] : [],
    description: item.description,
    score: item.score,
  }
}

function qualityColor(q?: string) {
  if (!q) return '#6B7280'
  if (q.includes('2160') || q.includes('4K')) return '#F59E0B'
  if (q.includes('1080')) return '#3B82F6'
  if (q.includes('720')) return '#c8501a'
  return '#6B7280'
}
function formatSize(bytes?: number) {
  if (!bytes) return ''
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`
  return ''
}

// ── Single swipeable card ─────────────────────────────────────────────────────
interface SwipeCardProps {
  item: AnimeSearchResult
  isTop: boolean
  stackIndex: number           // 0 = top, 1 = second, 2 = third
  onSwipeLeft: () => void
  onSwipeRight: () => void
}

function SwipeCard({ item, isTop, stackIndex, onSwipeLeft, onSwipeRight }: SwipeCardProps) {
  const x = useMotionValue(0)
  const rotate = useTransform(x, [-220, 220], [-18, 18])

  // Overlay opacities driven by drag distance
  const leftOpacity  = useTransform(x, [-120, -30, 0],  [1, 0.4, 0])
  const rightOpacity = useTransform(x, [0, 30, 120],    [0, 0.4, 1])

  // Grayscale: full when dragged far left, none at center/right
  const grayAmount = useTransform(x, [-200, -40, 0], [1, 0.3, 0])
  const grayFilter = useMotionTemplate`grayscale(${grayAmount})`

  // Background cards: scale + vertical offset based on position in stack
  const scale = isTop ? 1 : 1 - stackIndex * 0.05
  const translateY = isTop ? 0 : stackIndex * 14

  const handleDragEnd = (_: any, info: any) => {
    const threshold = 60
    const vThreshold = 300
    if (info.offset.x < -threshold || info.velocity.x < -vThreshold) {
      animate(x, -600, { duration: 0.3, ease: [0.4, 0, 0.2, 1] })
      setTimeout(onSwipeLeft, 220)
    } else if (info.offset.x > threshold || info.velocity.x > vThreshold) {
      animate(x, 600, { duration: 0.3, ease: [0.4, 0, 0.2, 1] })
      setTimeout(onSwipeRight, 220)
    } else {
      animate(x, 0, { type: 'spring', stiffness: 300, damping: 28 })
    }
  }

  return (
    <motion.div
      style={{
        position: 'absolute',
        width: '100%', height: '100%',
        scale,
        y: translateY,
        originY: 1,
        zIndex: 10 - stackIndex,
      }}
      animate={{ scale, y: translateY }}
      transition={{ type: 'spring', stiffness: 260, damping: 30 }}
    >
      <motion.div
        style={{ x, rotate, filter: grayFilter as any, width: '100%', height: '100%', borderRadius: 12, overflow: 'hidden', cursor: isTop ? 'grab' : 'default', touchAction: 'none' }}
        drag={isTop ? 'x' : false}
        dragConstraints={{ left: -400, right: 400 }}
        dragElastic={0.12}
        onDragEnd={isTop ? handleDragEnd : undefined}
        whileTap={isTop ? { cursor: 'grabbing' } : undefined}
      >
        {/* Cover image */}
        <div className="relative w-full h-full select-none">
          {item.coverUrl ? (
            <img
              src={item.coverUrl}
              alt={item.title}
              draggable={false}
              className="w-full h-full object-cover"
            />
          ) : (
            <div className="w-full h-full flex items-center justify-center" style={{ background: 'var(--surface-2)' }}>
              {item.animeType === 'Movie' ? <Film size={48} className="text-[color:var(--text-3)]" /> : <Tv size={48} className="text-[color:var(--text-3)]" />}
            </div>
          )}

          {/* Bottom info gradient */}
          <div
            className="absolute inset-x-0 bottom-0 px-5 pb-5 pt-16"
            style={{ background: 'linear-gradient(to top, rgba(0,0,0,0.92) 0%, rgba(0,0,0,0.6) 60%, transparent 100%)' }}
          >
            {/* Type badge */}
            {item.animeType && (
              <span
                className="inline-block px-2 py-0.5 rounded text-[10px] font-bold uppercase tracking-wide mb-2"
                style={{ background: item.animeType === 'Movie' ? '#c8501a' : '#3B82F6', color: 'white' }}
              >
                {item.animeType}
              </span>
            )}
            <h2 className="text-white font-bold text-lg leading-tight line-clamp-2 mb-1">{item.title}</h2>
            <div className="flex items-center gap-3 text-xs">
              {item.score && (
                <span className="flex items-center gap-1 text-amber-400 font-semibold">
                  <Star size={11} fill="currentColor" /> {item.score.toFixed(1)}
                </span>
              )}
              {item.year && <span className="text-[color:var(--text-1)]">{item.year}</span>}
              {item.episodeCount && item.animeType !== 'Movie' && (
                <span className="text-[color:var(--text-2)]">{item.episodeCount} eps</span>
              )}
            </div>
            {item.genres && item.genres.length > 0 && (
              <div className="flex flex-wrap gap-1 mt-2">
                {item.genres.slice(0, 3).map((g) => (
                  <span key={g} className="px-2 py-0.5 rounded-full text-[10px] text-white/70" style={{ background: 'rgba(255,255,255,0.1)' }}>
                    {g}
                  </span>
                ))}
              </div>
            )}
          </div>

          {/* NOPE overlay — left swipe */}
          {isTop && (
            <motion.div
              style={{ opacity: leftOpacity }}
              className="absolute inset-0 flex items-center justify-center pointer-events-none"
              aria-hidden
            >
              <div className="absolute inset-0" style={{ background: 'rgba(239,68,68,0.25)' }} />
              <div
                className="relative border-4 border-[var(--accent)] rounded-2xl px-6 py-3 -rotate-12"
                style={{ background: 'rgba(239,68,68,0.15)' }}
              >
                <span className="text-[color:var(--accent-hover)] font-black text-4xl tracking-widest drop-shadow-lg">NOPE</span>
              </div>
            </motion.div>
          )}

          {/* LIKE overlay — right swipe */}
          {isTop && (
            <motion.div
              style={{ opacity: rightOpacity }}
              className="absolute inset-0 flex items-center justify-center pointer-events-none"
              aria-hidden
            >
              <div className="absolute inset-0" style={{ background: 'rgba(34,197,94,0.2)' }} />
              <div
                className="relative border-4 border-green-400 rounded-2xl px-6 py-3 rotate-12"
                style={{ background: 'rgba(34,197,94,0.12)' }}
              >
                <span className="text-green-400 font-black text-4xl tracking-widest drop-shadow-lg">LIKE</span>
              </div>
            </motion.div>
          )}
        </div>
      </motion.div>
    </motion.div>
  )
}

// ── Stream panel (shown after liking a movie) ─────────────────────────────────
interface StreamPanelProps {
  item: AnimeSearchResult
  onClose: () => void
  onPlay: (stream: AnimeTorrentStream) => void
}

function StreamPanel({ item, onClose, onPlay }: StreamPanelProps) {
  const [streams, setStreams] = useState<AnimeTorrentStream[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const load = async () => {
      setLoading(true)
      try {
        const addons = await invoke<AnimeAddonInfo[]>(IPC.ADDON_LIST)
        if (!addons.length) { setError('No stream addons installed. Add one in Settings → Addons.'); setLoading(false); return }

        // For movies from Cinemeta, id is already an IMDB id (tt…)
        const imdbId = item.id.startsWith('tt') ? item.id : null
        if (!imdbId) { setError('No IMDB ID available for this title.'); setLoading(false); return }

        const all: AnimeTorrentStream[] = []
        for (const addon of addons) {
          try {
            const res = await invoke<AnimeTorrentStream[]>(IPC.ADDON_GET_STREAMS, addon.id, item.id, imdbId, 'movie')
            if (res?.length) all.push(...res.map(s => ({ ...s, source: s.source || addon.name })))
          } catch { /* addon failed, skip */ }
        }

        // Sort: H.264 first, then by quality, then seeders
        const isHevc = (t?: string) => !!t && /\b(hevc|h\.?265|x265)\b/i.test(t)
        const qOrder = (q?: string) => q?.includes('2160') ? 4 : q?.includes('1080') ? 3 : q?.includes('720') ? 2 : q?.includes('480') ? 1 : 0
        all.sort((a, b) => {
          const hd = (isHevc(a.title) ? 1 : 0) - (isHevc(b.title) ? 1 : 0)
          if (hd) return hd
          return qOrder(b.quality) - qOrder(a.quality) || (b.seeders ?? 0) - (a.seeders ?? 0)
        })
        setStreams(all)
      } catch (e: any) {
        setError(e?.message || 'Failed to load streams')
      } finally {
        setLoading(false)
      }
    }
    load()
  }, [item.id])

  return (
    <motion.div
      initial={{ y: '100%', opacity: 0 }}
      animate={{ y: 0, opacity: 1 }}
      exit={{ y: '100%', opacity: 0 }}
      transition={{ type: 'spring', stiffness: 300, damping: 32 }}
      className="absolute inset-x-0 bottom-0 z-50 rounded-t-3xl overflow-hidden"
      style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}
    >
      {/* Handle */}
      <div className="flex justify-center pt-3 pb-1">
        <div className="w-10 h-1 rounded-full" style={{ background: 'rgba(255,255,255,0.15)' }} />
      </div>

      <div className="px-4 pb-2 flex items-center justify-between gap-3">
        <div className="min-w-0">
          <h3 className="text-white font-bold text-sm line-clamp-1">{item.title}</h3>
          <p className="text-[color:var(--text-3)] text-xs mt-0.5">Choose a stream</p>
        </div>
        <button onClick={onClose} aria-label="Close" className="w-10 h-10 shrink-0 flex items-center justify-center rounded-lg text-[color:var(--text-3)] hover:text-white transition-colors" style={{ background: 'var(--surface-2)' }}>
          <X size={16} />
        </button>
      </div>

      <div className="px-4 pb-5">
        {loading && (
          <div className="flex items-center justify-center gap-2 py-8 text-[color:var(--text-3)] text-sm">
            <Loader2 size={16} className="animate-spin" /> Searching streams...
          </div>
        )}
        {!loading && error && (
          <div className="flex items-center gap-2 px-4 py-3 rounded-xl text-xs text-amber-300" style={{ background: 'rgba(251,191,36,0.08)', border: '1px solid rgba(251,191,36,0.15)' }}>
            <AlertTriangle size={13} /> {error}
          </div>
        )}
        {!loading && !error && streams.length === 0 && (
          <p className="text-[color:var(--text-3)] text-sm text-center py-8">No streams found for this title.</p>
        )}
        {!loading && streams.length > 0 && (
          <div className="flex flex-col gap-1.5 mt-1">
            {streams.slice(0, 6).map((s, i) => (
              <button
                key={`${s.infoHash}-${i}`}
                onClick={() => onPlay(s)}
                className="min-h-10 flex items-center gap-3 px-3 py-2.5 rounded-xl text-left transition-all hover:bg-white/[0.06] active:scale-[0.98]"
                style={{ background: 'rgba(255,255,255,0.04)', border: '1px solid var(--panel-border)' }}
              >
                <div className="w-7 h-7 rounded-lg flex items-center justify-center shrink-0" style={{ background: 'rgba(200, 80, 26,0.15)' }}>
                  <Play size={10} className="text-[color:var(--accent-hover)] translate-x-px" fill="currentColor" />
                </div>
                <div className="flex-1 min-w-0">
                  <p className="text-xs text-[color:var(--text-1)] line-clamp-1 leading-tight">{s.title}</p>
                </div>
                <div className="flex items-center gap-2 shrink-0">
                  {s.quality && (
                    <span className="px-1.5 py-0.5 rounded text-[10px] font-bold" style={{ background: `${qualityColor(s.quality)}22`, color: qualityColor(s.quality) }}>
                      {s.quality}
                    </span>
                  )}
                  {s.size != null && s.size > 0 && (
                    <span className="text-[10px] text-[color:var(--text-3)]">{formatSize(s.size)}</span>
                  )}
                  {s.seeders != null && (
                    <span className="text-[10px] text-[color:var(--text-3)]">{s.seeders}↑</span>
                  )}
                </div>
              </button>
            ))}
          </div>
        )}
      </div>
    </motion.div>
  )
}

// ── Main Discover page ────────────────────────────────────────────────────────
const STACK_SIZE = 3   // visible cards at once
const PREFETCH  = 8   // how many cards to keep loaded ahead

export function Discover() {
  const { navigate } = useUiStore()

  const [scope, setScope] = useState<'anime' | 'streaming'>('anime')
  const [deck, setDeck] = useState<AnimeSearchResult[]>([])
  const [fetching, setFetching] = useState(false)
  const [empty, setEmpty] = useState(false)

  const seen = useMediaStore((state) => state.seen)
  const hydrateMedia = useMediaStore((state) => state.hydrate)
  const recordMedia = useMediaStore((state) => state.record)
  const seenRef = useRef<Set<string>>(new Set())

  // Undo — store the last skipped card (restored once per skip)
  const [undoCard, setUndoCard] = useState<AnimeSearchResult | null>(null)

  // Stream panel for movies
  const [streamTarget, setStreamTarget] = useState<AnimeSearchResult | null>(null)

  // ── Data loading ────────────────────────────────────────────────────────────
  const animePageRef = useRef(1)
  const moviePageRef = useRef(1)
  const seriesPageRef = useRef(1)

  useEffect(() => {
    void hydrateMedia(scope === 'anime' ? 'anime' : undefined)
  }, [scope, hydrateMedia])

  useEffect(() => {
    seenRef.current = new Set(seen.map((item) =>
      item.kind === 'anime' ? String(item.malId || item.id.replace('anime:mal:', '')) : String(item.imdbId || item.id.split(':').pop())
    ))
  }, [seen])

  const fetchBatch = useCallback(async () => {
    if (fetching) return
    setFetching(true)
    try {
      const items: AnimeSearchResult[] = []

      if (scope === 'anime') {
        const animeData = await invoke<AnimeSearchResult[]>(IPC.ANIME_TOP, animePageRef.current, undefined)
        animePageRef.current++
        items.push(...animeData.filter(a => (a.score ?? 0) >= 8.0))
      } else {
        const movieData = await invoke<AnimeSearchResult[]>(IPC.MOVIE_CATALOG, 'imdbRating', moviePageRef.current, undefined)
        const seriesData = await invoke<AnimeSearchResult[]>(IPC.SERIES_CATALOG, 'imdbRating', seriesPageRef.current, undefined)
        moviePageRef.current++
        seriesPageRef.current++
        items.push(...movieData.filter(m => (m.score ?? 0) >= 8.0), ...seriesData.filter(m => (m.score ?? 0) >= 8.0))
      }

      // Filter already-seen IDs, shuffle
      const fresh = items.filter(it => !seenRef.current.has(it.id))
      for (let i = fresh.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        [fresh[i], fresh[j]] = [fresh[j], fresh[i]]
      }

      setDeck(prev => {
        const combined = [...prev, ...fresh]
        if (combined.length === 0) setEmpty(true)
        return combined
      })
    } catch (e) {
      console.error('[discover] fetch failed:', e)
    } finally {
      setFetching(false)
    }
  }, [scope, fetching])

  // Initial load + reload when filter changes
  useEffect(() => {
    animePageRef.current = 1
    moviePageRef.current = 1
    seriesPageRef.current = 1
    setDeck([])
    setEmpty(false)
    setUndoCard(null)
  }, [scope])

  useEffect(() => {
    if (deck.length === 0 && !fetching) fetchBatch()
  }, [deck.length, fetching, scope])

  // Prefetch more when deck runs low
  useEffect(() => {
    if (deck.length < PREFETCH && !fetching && !empty) fetchBatch()
  }, [deck.length])

  // ── Swipe handlers ──────────────────────────────────────────────────────────
  const swipeLeft = useCallback((item: AnimeSearchResult) => {
    seenRef.current.add(item.id)
    const kind: MediaKind = scope === 'anime' ? 'anime' : item.animeType === 'Movie' ? 'movie' : 'series'
    void recordMedia({ kind, type: 'skip', title: discoverTitle(item, kind) })
    setUndoCard(item)
    setDeck(prev => prev.filter(c => c.id !== item.id))
  }, [recordMedia, scope])

  const swipeRight = useCallback((item: AnimeSearchResult) => {
    seenRef.current.add(item.id)
    const kind: MediaKind = scope === 'anime' ? 'anime' : item.animeType === 'Movie' ? 'movie' : 'series'
    void recordMedia({ kind, type: 'like', title: discoverTitle(item, kind) })
    setUndoCard(null)
    setDeck(prev => prev.filter(c => c.id !== item.id))

    if (scope === 'streaming' && item.animeType === 'Movie') {
      // Movie — show stream picker in-page
      setStreamTarget(item)
    } else {
      navigate('anime-detail', {
        animeId: item.id,
        title: item.title,
        coverUrl: item.coverUrl,
        year: item.year,
        status: item.status,
        genres: item.genres,
        description: item.description,
        animeType: item.animeType,
        episodeCount: item.episodeCount,
        score: item.score,
        mediaKind: scope === 'anime' ? 'anime' : 'series',
      })
    }
  }, [navigate, recordMedia, scope])

  const handleUndo = useCallback(() => {
    if (!undoCard) return
    seenRef.current.delete(undoCard.id)
    setDeck(prev => [undoCard, ...prev])
    setUndoCard(null)
  }, [undoCard])

  const playStream = (stream: AnimeTorrentStream) => {
    if (!streamTarget) return
    navigate('anime-player', {
      infoHash: stream.infoHash,
      fileIdx: stream.fileIdx,
      streamTitle: stream.title,
      animeTitle: streamTarget.title,
      coverUrl: streamTarget.coverUrl,
      httpUrl: stream.infoHash?.startsWith('http') ? stream.infoHash : undefined,
      streamQuality: stream.quality,
      imdbId: streamTarget.id.startsWith('tt') ? streamTarget.id : undefined,
      mediaKind: 'movie',
      returnRoute: { page: 'discover', params: {} },
    })
  }

  // Top 3 visible cards (index 0 = top)
  const visible = deck.slice(0, STACK_SIZE)
  const topCard = visible[0] ?? null

  return (
    <div className="flex flex-col h-full overflow-x-hidden" style={{ background: 'var(--base)' }}>
      {/* Header */}
      <div className="titlebar-drag px-4 sm:px-5 pt-3 pb-2 flex flex-wrap items-center justify-between gap-2 shrink-0">
        <div className="flex items-center gap-3">
          <h2 className="text-lg font-bold tracking-tight"
            style={{ background: 'linear-gradient(135deg, var(--accent-hover), var(--accent))', WebkitBackgroundClip: 'text', WebkitTextFillColor: 'transparent' }}
          >
            Discover
          </h2>
          <span className="text-xs text-[color:var(--text-3)] font-medium mt-0.5">8.0+ rated</span>
        </div>

        <div className="flex flex-wrap items-center gap-2 sm:gap-3" style={{ WebkitAppRegion: 'no-drag' } as any}>
          {/* Undo */}
          <button
            onClick={handleUndo}
            disabled={!undoCard}
            className="min-h-10 sm:min-h-0 flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all disabled:opacity-30 disabled:cursor-not-allowed"
            style={{ background: 'var(--surface-2)', color: 'var(--text-2)' }}
            title="Undo last skip"
          >
            <RotateCcw size={13} /> Undo
          </button>

          <div className="flex rounded-lg p-0.5" style={{ background: 'var(--surface-2)' }}>
            <button onClick={() => setScope('anime')} className="min-h-10 sm:min-h-0 flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs whitespace-nowrap"
              style={scope === 'anime' ? { background: 'var(--accent)', color: 'white' } : { color: 'var(--text-3)' }}>
              <Tv size={13} /> Anime
            </button>
            <button onClick={() => setScope('streaming')} className="min-h-10 sm:min-h-0 flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs whitespace-nowrap"
              style={scope === 'streaming' ? { background: 'var(--accent)', color: 'white' } : { color: 'var(--text-3)' }}>
              <Film size={13} /> <span className="hidden min-[400px]:inline">Movies & Shows</span><span className="min-[400px]:hidden">Movies</span>
            </button>
          </div>
        </div>
      </div>

      {/* Card stack area */}
      <div className="flex-1 min-h-0 flex flex-col items-center justify-center px-4 sm:px-6 pb-4 overflow-hidden">

        {/* Loading first batch */}
        {fetching && deck.length === 0 && (
          <div className="flex flex-col items-center gap-3 text-[color:var(--text-3)]">
            <Loader2 size={28} className="animate-spin text-[color:var(--accent-hover)]" />
            <p className="text-sm">Finding great titles...</p>
          </div>
        )}

        {/* Empty state */}
        {!fetching && empty && deck.length === 0 && (
          <div className="flex flex-col items-center gap-4 text-center">
            <div className="w-20 h-20 rounded-3xl flex items-center justify-center" style={{ background: 'var(--surface-2)' }}>
              <Shuffle size={32} className="text-[color:var(--text-3)]" />
            </div>
            <div>
              <p className="text-white font-semibold text-lg mb-1">You've seen everything!</p>
              <p className="text-[color:var(--text-3)] text-sm">No more 8+ rated titles to show right now.</p>
            </div>
            <button
              onClick={() => {
                seenRef.current.clear()
                animePageRef.current = 1; moviePageRef.current = 1
                setEmpty(false); setDeck([])
              }}
              className="px-5 py-2.5 rounded-xl text-sm font-medium text-white transition-all hover:opacity-90"
              style={{ background: 'var(--accent-gradient)' }}
            >
              Start over
            </button>
          </div>
        )}

        {/* The card deck */}
        {deck.length > 0 && (
          <div className="relative w-full shrink" style={{ maxWidth: 'min(360px, 100%, max(180px, calc((100vh - 330px) * 2 / 3)))', aspectRatio: '2/3' }}>
            <AnimatePresence>
              {visible.map((item, idx) => (
                <SwipeCard
                  key={item.id}
                  item={item}
                  isTop={idx === 0}
                  stackIndex={idx}
                  onSwipeLeft={() => swipeLeft(item)}
                  onSwipeRight={() => swipeRight(item)}
                />
              ))}
            </AnimatePresence>
          </div>
        )}

        {/* Action buttons */}
        {topCard && (
          <div className="flex items-center gap-6 mt-4 sm:mt-6 shrink-0">
            {/* Skip (swipe left) */}
            <motion.button
              whileHover={{ scale: 1.08 }}
              whileTap={{ scale: 0.92 }}
              onClick={() => swipeLeft(topCard)}
              className="w-16 h-16 rounded-full flex items-center justify-center shadow-xl transition-shadow hover:shadow-red-500/20"
              style={{ background: 'rgba(239,68,68,0.1)', border: '2px solid rgba(239,68,68,0.3)' }}
            >
              <X size={26} className="text-[color:var(--accent-hover)]" strokeWidth={2.5} />
            </motion.button>

            {/* Like / Play (swipe right) */}
            <motion.button
              whileHover={{ scale: 1.08 }}
              whileTap={{ scale: 0.92 }}
              onClick={() => swipeRight(topCard)}
              className="w-16 h-16 rounded-full flex items-center justify-center shadow-xl transition-shadow hover:shadow-green-500/20"
              style={{ background: 'rgba(34,197,94,0.1)', border: '2px solid rgba(34,197,94,0.3)' }}
            >
              <Heart size={26} className="text-green-400" strokeWidth={2.5} />
            </motion.button>
          </div>
        )}

        {/* Description peek */}
        {topCard?.description && (
          <p className="mt-4 text-xs text-[color:var(--text-3)] text-center max-w-xs line-clamp-2 leading-relaxed">
            {topCard.description}
          </p>
        )}
      </div>

      {/* Stream panel bottom sheet */}
      <AnimatePresence>
        {streamTarget && (
          <>
            {/* Backdrop */}
            <motion.div
              initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}
              className="absolute inset-0 z-40"
              style={{ background: 'rgba(0,0,0,0.6)' }}
              onClick={() => setStreamTarget(null)}
            />
            <StreamPanel
              item={streamTarget}
              onClose={() => setStreamTarget(null)}
              onPlay={playStream}
            />
          </>
        )}
      </AnimatePresence>
    </div>
  )
}
