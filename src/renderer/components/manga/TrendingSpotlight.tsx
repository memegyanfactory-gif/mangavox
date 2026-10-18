import { useEffect, useMemo, useRef, useState } from 'react'
import { BookOpen, Flame, Loader2, Star, Users, Heart, ChevronLeft, ChevronRight } from 'lucide-react'
import { invoke } from '../../hooks/useIpc'
import { IPC } from '../../../shared/ipc-channels'
import { coverUrl } from '../../utils/cover-url'
import { formatCount } from '../../utils/open-manga'

export interface TrendingManga {
  malId: string
  title: string
  coverUrl?: string
  genres: string[]
  score?: number
  type?: string
  members?: number
  favorites?: number
  rank?: number
  synopsis?: string
}

/** Best pick = high score weighted by how widely it is read. */
const spotlightScore = (m: TrendingManga) => (m.score ?? 6) * Math.log10((m.members ?? 1000) + 10)
function pickSpotlight(items: TrendingManga[]): TrendingManga | null {
  if (!items.length) return null
  return [...items].sort((a, b) => spotlightScore(b) - spotlightScore(a))[0]
}

export function TrendingSpotlight({ onOpen, openingId }: { onOpen: (m: TrendingManga) => void; openingId: string | null }) {
  const [items, setItems] = useState<TrendingManga[]>([])
  const [loading, setLoading] = useState(true)
  const railRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let cancelled = false
    invoke<TrendingManga[]>(IPC.MANGA_TRENDING)
      .then((r) => { if (!cancelled) setItems(r || []) })
      .catch(() => { if (!cancelled) setItems([]) })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [])

  const spotlight = useMemo(() => pickSpotlight(items), [items])
  const scrollRail = (dir: number) => railRef.current?.scrollBy({ left: dir * 520, behavior: 'smooth' })

  if (loading) return <div className="h-[260px] rounded-xl mb-6 skeleton" />
  if (!spotlight) return null

  const cover = coverUrl(spotlight.coverUrl) || ''
  return (
    <div className="mb-7">
      <section className="relative rounded-2xl overflow-hidden min-h-[260px]" style={{ border: '1px solid var(--panel-border)', background: 'var(--surface-1)' }}>
        {cover && <img src={cover} alt="" className="absolute inset-0 w-full h-full object-cover scale-125 blur-2xl opacity-40" />}
        <div className="absolute inset-0" style={{ background: 'linear-gradient(90deg, var(--base) 8%, rgba(var(--accent-rgb),0.10) 55%, transparent 100%)' }} />
        <div className="absolute inset-0" style={{ background: 'linear-gradient(0deg, var(--base) 0%, transparent 45%)', opacity: 0.8 }} />

        <div className="relative z-10 flex items-center gap-7 p-4 sm:p-6 sm:pr-8">
          <div className="hidden sm:block shrink-0 w-[150px] aspect-[2/3] rounded-xl overflow-hidden shadow-2xl" style={{ border: '1px solid var(--panel-border-strong)' }}>
            {cover && <img src={cover} alt={spotlight.title} className="w-full h-full object-cover" />}
          </div>
          <div className="min-w-0 max-w-2xl">
            <span className="inline-flex items-center gap-1.5 text-[10px] uppercase tracking-[0.22em] font-bold mb-2.5" style={{ color: 'var(--accent-hover)' }}>
              <Flame size={12} /> Spotlight · most-read right now
            </span>
            <h3 className="text-[22px] sm:text-[28px] font-black leading-tight break-words tracking-tight mb-2 line-clamp-2" style={{ color: 'var(--text-1)' }}>{spotlight.title}</h3>
            <div className="flex flex-wrap items-center gap-3 text-xs mb-3" style={{ color: 'var(--text-2)' }}>
              {spotlight.score ? <span className="flex items-center gap-1 font-bold" style={{ color: 'var(--warning)' }}><Star size={12} fill="currentColor" />{spotlight.score.toFixed(2)}</span> : null}
              <span className="flex items-center gap-1"><Users size={12} />{formatCount(spotlight.members)} readers</span>
              {spotlight.favorites ? <span className="flex items-center gap-1"><Heart size={12} />{formatCount(spotlight.favorites)} favorites</span> : null}
              {spotlight.rank ? <span>Rank #{spotlight.rank}</span> : null}
            </div>
            <div className="flex flex-wrap gap-1.5 mb-3">
              {spotlight.genres.slice(0, 4).map((g) => (
                <span key={g} className="px-2 py-0.5 rounded-full text-[10px] font-semibold" style={{ background: 'rgba(var(--accent-rgb),0.14)', color: 'var(--accent-hover)', border: '1px solid rgba(var(--accent-rgb),0.25)' }}>{g}</span>
              ))}
            </div>
            {spotlight.synopsis && <p className="text-[12.5px] leading-relaxed line-clamp-3 mb-4" style={{ color: 'var(--text-2)' }}>{spotlight.synopsis}</p>}
            <button
              onClick={() => onOpen(spotlight)}
              disabled={!!openingId}
              className="inline-flex items-center gap-2 px-4 py-2 min-h-[40px] rounded-xl text-white text-sm font-bold transition-all hover:brightness-110 disabled:opacity-60"
              style={{ background: 'var(--accent-gradient)' }}
            >
              {openingId === spotlight.malId ? <Loader2 size={15} className="animate-spin" /> : <BookOpen size={15} />}
              Read now
            </button>
          </div>
        </div>
      </section>

      <div className="flex items-center justify-between gap-2 mt-5 mb-2.5">
        <h4 className="flex items-center gap-2 text-sm font-bold min-w-0" style={{ color: 'var(--text-1)' }}>
          <Flame size={14} style={{ color: 'var(--accent-hover)' }} /> Trending now
          <span className="hidden sm:inline text-[11px] font-medium" style={{ color: 'var(--text-3)' }}>what readers are into</span>
        </h4>
        <div className="flex gap-1 shrink-0">
          <button className="wf-icon-btn" onClick={() => scrollRail(-1)} aria-label="Scroll left"><ChevronLeft size={15} /></button>
          <button className="wf-icon-btn" onClick={() => scrollRail(1)} aria-label="Scroll right"><ChevronRight size={15} /></button>
        </div>
      </div>
      <div ref={railRef} className="flex gap-3 overflow-x-auto pb-2 scroll-smooth snap-x" style={{ scrollbarWidth: 'none' }}>
        {items.map((m, i) => (
          <button
            key={m.malId}
            onClick={() => onOpen(m)}
            disabled={!!openingId}
            className="group shrink-0 w-[120px] sm:w-[132px] snap-start text-left transition-transform hover:-translate-y-1 disabled:opacity-70"
          >
            <div className="relative aspect-[2/3] rounded-xl overflow-hidden" style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
              {m.coverUrl && <img src={coverUrl(m.coverUrl) || ''} alt="" loading="lazy" className="w-full h-full object-cover" />}
              <span className="absolute top-1.5 left-1.5 min-w-[22px] h-[22px] px-1 rounded-md flex items-center justify-center text-[11px] font-black text-white" style={{ background: i < 3 ? 'var(--accent-gradient)' : 'rgba(0,0,0,0.65)' }}>{i + 1}</span>
              {m.score ? <span className="absolute top-1.5 right-1.5 flex items-center gap-0.5 px-1.5 py-0.5 rounded-md text-[10px] font-bold bg-black/65" style={{ color: 'var(--warning)' }}><Star size={9} fill="currentColor" />{m.score.toFixed(1)}</span> : null}
              <div className="absolute inset-x-0 bottom-0 px-2 py-1.5 text-[10px] font-semibold flex items-center gap-1" style={{ background: 'linear-gradient(0deg, rgba(0,0,0,0.85), transparent)', color: '#fff' }}>
                <Users size={10} /> {formatCount(m.members)} readers
              </div>
              {openingId === m.malId && <div className="absolute inset-0 flex items-center justify-center bg-black/60"><Loader2 size={18} className="animate-spin text-white" /></div>}
            </div>
            <div className="mt-1.5 text-xs font-semibold leading-snug line-clamp-2 group-hover:text-[var(--accent-hover)] transition-colors" style={{ color: 'var(--text-1)' }}>{m.title}</div>
            {m.genres[0] && <div className="text-[10px] mt-0.5" style={{ color: 'var(--text-3)' }}>{m.genres.slice(0, 2).join(' · ')}</div>}
          </button>
        ))}
      </div>
    </div>
  )
}
