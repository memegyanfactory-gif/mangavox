import { useState, useEffect, useCallback, useRef } from 'react'
import { motion, AnimatePresence } from 'framer-motion'
import { Play, ChevronLeft, ChevronRight, Info, Plus, Check, Star } from 'lucide-react'
import { invoke } from '../hooks/useIpc'
import { coverUrl } from '../utils/cover-url'
import { IPC } from '../../shared/ipc-channels'

export interface HeroSlide {
  id: string
  title: string
  subtitle?: string
  description?: string
  imageUrl?: string
  badge?: string
  meta?: string
  kind?: 'anime' | 'movie' | 'series' | 'manga'
  /** Cinematic variant: genre chips under the title. */
  genres?: string[]
  score?: number
  /** Image is a portrait cover (not a banner): rendered blurred + poster card on the right. */
  imageIsPoster?: boolean
}

interface HeroCarouselProps {
  slides: HeroSlide[]
  accent?: string
  onPlay: (slide: HeroSlide) => void
  onInfo?: (slide: HeroSlide) => void
  onAdd?: (slide: HeroSlide) => void
  intervalMs?: number
  /** Full-bleed cinematic layout (taller, labelled buttons, dot indicators bottom-centre). */
  cinematic?: boolean
  playLabel?: string
  addLabel?: string
  /** Slide ids already in the user's list (switches the add button to a confirmed state). */
  addedIds?: string[]
}

/** Split a packed meta string ("TV-MA · 2024 · 2h 04m") into chips. */
function metaChips(meta?: string): string[] {
  if (!meta) return []
  return meta.split(/[·•|]/).map((s) => s.trim()).filter(Boolean)
}

export function HeroCarousel({
  slides,
  accent = '#e8392e',
  onPlay,
  onInfo,
  onAdd,
  intervalMs = 10_000,
  cinematic = false,
  playLabel = 'Play',
  addLabel,
  addedIds,
}: HeroCarouselProps) {
  const [index, setIndex] = useState(0)
  const [clips, setClips] = useState<Record<string, string>>({})
  const count = Math.min(5, slides.length)
  const visible = slides.slice(0, count)
  const current = visible[index % Math.max(1, count)]

  useEffect(() => { setIndex(0) }, [slides.length])

  useEffect(() => {
    if (count < 2) return
    const timer = window.setInterval(() => setIndex((i) => (i + 1) % count), intervalMs)
    return () => window.clearInterval(timer)
  }, [count, intervalMs])

  useEffect(() => {
    if (!current?.title) return
    const key = current.id
    if (clips[key]) return
    void invoke<string | null>(IPC.HERO_GET_CLIP, current.title, current.kind || 'movie').then((url) => {
      if (url) setClips((prev) => ({ ...prev, [key]: url }))
    })
  }, [current?.id, current?.title, current?.kind, clips])

  const go = useCallback((dir: -1 | 1) => {
    setIndex((i) => (i + dir + count) % count)
  }, [count])

  const touchX = useRef<number | null>(null)

  if (!current) return null
  const clip = clips[current.id]
  const chips = metaChips(current.meta)

  return (
    <section
      className={cinematic
        ? 'relative mb-2 overflow-hidden sm:h-[clamp(320px,52vh,460px)]'
        : 'relative mb-4 overflow-hidden rounded-xl sm:h-[clamp(240px,40vh,340px)]'}
      onTouchStart={(e) => { touchX.current = e.touches[0]?.clientX ?? null }}
      onTouchEnd={(e) => {
        const start = touchX.current
        touchX.current = null
        const end = e.changedTouches[0]?.clientX
        if (start == null || end == null || count < 2) return
        if (Math.abs(end - start) > 50) go(end < start ? 1 : -1)
      }}
    >
      {/* Media: a short block on phones (text sits below it), full-bleed backdrop from sm up */}
      <div className="relative h-[200px] min-[420px]:h-[240px] sm:absolute sm:inset-0 sm:h-auto">
      <AnimatePresence mode="wait">
        <motion.div
          key={current.id}
          className="absolute inset-0"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.7, ease: [0.22, 1, 0.36, 1] }}
        >
          {clip ? (
            <video
              key={clip}
              src={clip}
              className="absolute inset-0 w-full h-full object-cover"
              autoPlay
              muted
              loop
              playsInline
            />
          ) : current.imageUrl && current.imageIsPoster ? (
            <div
              className="absolute inset-0 bg-cover bg-center"
              style={{ backgroundImage: `url(${coverUrl(current.imageUrl) || ''})`, filter: 'blur(28px) saturate(1.2)', transform: 'scale(1.25)', opacity: 0.7 }}
            />
          ) : current.imageUrl ? (
            <div
              className="absolute inset-0 bg-cover bg-center"
              style={{ backgroundImage: `url(${coverUrl(current.imageUrl) || ''})`, animation: 'wf-pan 18s ease-out forwards' }}
            />
          ) : (
            <div className="absolute inset-0" style={{ background: 'var(--surface-2)' }} />
          )}
          {/* Left-to-right cinematic scrim */}
          <div
            className="absolute inset-0 hidden sm:block"
            style={{ background: 'linear-gradient(90deg, rgba(8,8,9,.92) 0%, rgba(8,8,9,.62) 38%, rgba(8,8,9,0) 70%)' }}
          />
          {/* Bottom fade into page */}
          <div
            className="absolute inset-x-0 bottom-0 h-[46%]"
            style={{ background: 'linear-gradient(0deg, var(--base) 4%, rgba(11,11,12,.4) 56%, transparent 100%)' }}
          />
        </motion.div>
      </AnimatePresence>
      </div>

      {cinematic && current.imageIsPoster && current.imageUrl && (
        <div className="absolute right-16 top-1/2 -translate-y-1/2 z-10 hidden md:block wf-fade" key={`p-${current.id}`}>
          <img src={coverUrl(current.imageUrl) || ''} alt="" className="h-[clamp(220px,38vh,330px)] aspect-[2/3] object-cover rounded-xl shadow-2xl" style={{ border: '1px solid rgba(255,255,255,.12)' }} />
        </div>
      )}

      <div className={`relative z-10 -mt-12 px-4 pb-2 sm:mt-0 sm:p-0 sm:absolute ${cinematic ? 'sm:left-10 sm:bottom-12' : 'sm:left-6 sm:bottom-8'} sm:max-w-xl wf-fade`} key={`c-${current.id}`}>
        <div className="flex items-center gap-2 mb-2">
          <span className="text-white font-extrabold text-[10px] tracking-wider px-1.5 py-[2px] rounded" style={{ background: accent }}>
            {current.kind === 'manga' ? 'MV' : 'WF'}
          </span>
          {current.badge && (
            <span className="text-[11px] font-semibold tracking-wide" style={{ color: 'var(--text-2)' }}>{current.badge}</span>
          )}
        </div>

        <h1 className="font-extrabold text-white leading-tight mb-2 line-clamp-2 drop-shadow-[0_4px_30px_rgba(0,0,0,0.5)]"
          style={{ fontSize: 'clamp(22px,2.6vw,32px)', letterSpacing: '-0.5px' }}>
          {current.title}
        </h1>

        {chips.length > 0 && (
          <div className="flex items-center gap-2.5 mb-2 text-[11px] font-semibold flex-wrap" style={{ color: 'var(--text-2)' }}>
            {chips.map((c, i) => {
              const isRating = /^(TV-|PG|R$|NC|G$|\d{1,2}\+$|U\/A)/i.test(c)
              return isRating ? (
                <span key={i} className="border rounded px-1.5 py-px text-[10px]" style={{ borderColor: 'rgba(255,255,255,.35)' }}>{c}</span>
              ) : (
                <span key={i}>{c}</span>
              )
            })}
            <span className="border rounded px-1.5 py-px text-[9px]" style={{ borderColor: 'rgba(255,255,255,.25)', color: 'var(--text-3)' }}>HD</span>
          </div>
        )}

        {cinematic && ((current.genres && current.genres.length > 0) || current.score != null) && (
          <div className="flex flex-wrap items-center gap-1.5 mb-2">
            {current.score != null && (
              <span className="flex items-center gap-1 text-[11px] font-bold text-amber-400 mr-1"><Star size={11} fill="currentColor" /> {current.score}</span>
            )}
            {(current.genres || []).slice(0, 4).map((g) => (
              <span key={g} className="px-2 py-px rounded-full text-[10px] font-medium text-white" style={{ background: 'rgba(255,255,255,.14)' }}>{g}</span>
            ))}
          </div>
        )}

        {current.description && (
          <p className="text-[12px] leading-snug mb-3 line-clamp-3 sm:line-clamp-2 max-w-lg drop-shadow-[0_2px_12px_rgba(0,0,0,0.6)]" style={{ color: 'var(--text-2)', textWrap: 'pretty' as any }}>
            {current.description}
          </p>
        )}

        <div className="flex items-center gap-3 flex-wrap">
          <button onClick={() => onPlay(current)} className="wf-btn-play" style={{ padding: '7px 16px', fontSize: 12, gap: 6, minHeight: 40 }}>
            <Play size={14} fill="currentColor" /> {playLabel}
          </button>
          {onInfo && (
            <button onClick={() => onInfo(current)} className="wf-btn-glass" style={{ padding: '7px 14px', fontSize: 12, gap: 6, minHeight: 40 }}>
              <Info size={14} /> More Info
            </button>
          )}
          {onAdd && addLabel ? (
            <button
              onClick={() => onAdd(current)}
              className="wf-btn-glass"
              style={{ padding: '7px 14px', fontSize: 12, gap: 6, minHeight: 40 }}
            >
              {addedIds?.includes(current.id) ? <Check size={14} /> : <Plus size={14} />}
              {addedIds?.includes(current.id) ? 'In My List' : addLabel}
            </button>
          ) : onAdd && (
            <button
              onClick={() => onAdd(current)}
              title="Add to My List"
              className="w-10 h-10 rounded-full flex items-center justify-center text-white transition-all"
              style={{ border: '1.5px solid rgba(255,255,255,.45)' }}
              onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,.12)')}
              onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
            >
              <Plus size={18} strokeWidth={2} />
            </button>
          )}
        </div>
      </div>

      {count > 1 && (
        <>
          <div className={`relative flex justify-center gap-0.5 z-10 sm:absolute sm:bottom-1 ${cinematic ? 'sm:left-1/2 sm:-translate-x-1/2' : 'sm:right-6'}`}>
            {visible.map((slide, i) => (
              <button key={slide.id} onClick={() => setIndex(i)} aria-label={`Slide ${i + 1}`}
                className="h-10 sm:h-7 px-1 flex items-center">
                <span className="block h-1 rounded-full transition-all"
                  style={{ width: i === index % count ? 26 : 8, background: i === index % count ? accent : 'rgba(255,255,255,.32)' }} />
              </button>
            ))}
          </div>
          <button onClick={() => go(-1)} aria-label="Previous" className="absolute left-2 top-1/2 -translate-y-1/2 z-10 w-10 h-10 rounded-full bg-black/45 text-white hidden sm:flex items-center justify-center hover:bg-black/70 transition-colors backdrop-blur-sm">
            <ChevronLeft size={16} />
          </button>
          <button onClick={() => go(1)} aria-label="Next" className="absolute right-2 top-1/2 -translate-y-1/2 z-10 w-10 h-10 rounded-full bg-black/45 text-white hidden sm:flex items-center justify-center hover:bg-black/70 transition-colors backdrop-blur-sm">
            <ChevronRight size={16} />
          </button>
        </>
      )}
    </section>
  )
}
