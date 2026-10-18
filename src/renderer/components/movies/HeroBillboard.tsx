import { useEffect, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { Play, Info, Plus, Check, Star, ExternalLink } from 'lucide-react'
import type { AnimeSearchResult } from '../../../shared/anime-addon-interface'
import { coverUrl, tmdbImageSize } from '../../utils/cover-url'
import type { Skin } from './skins'

/** Cinematic billboard that rotates through up to 5 titles. Layout follows the skin. */
export function HeroBillboard({
  items, skin, kind, onPlay, onInfo, onToggleList, inList, onOfficial, officialName,
}: {
  items: AnimeSearchResult[]
  skin: Skin
  kind: 'movie' | 'series'
  onPlay: (item: AnimeSearchResult) => void
  onInfo: (item: AnimeSearchResult) => void
  onToggleList: (item: AnimeSearchResult) => void
  inList: (item: AnimeSearchResult) => boolean
  onOfficial?: (item: AnimeSearchResult) => void
  officialName?: string
}) {
  const [index, setIndex] = useState(0)
  const [paused, setPaused] = useState(false)
  const slides = items.slice(0, 5)
  useEffect(() => { setIndex(0) }, [items[0]?.id])
  useEffect(() => {
    if (slides.length < 2 || paused) return
    const t = window.setInterval(() => setIndex((i) => (i + 1) % slides.length), 9000)
    return () => window.clearInterval(t)
  }, [slides.length, paused])
  if (slides.length === 0) return null
  const item = slides[index % slides.length]
  // Catalog backdrops are card-sized (w780); the billboard spans the page, so ask TMDB for w1280.
  const src = coverUrl(tmdbImageSize(item.bannerUrl, 'w1280') || item.coverUrl)
  const centered = skin.family === 'apple'
  const r = skin.pill ? 999 : Math.min(skin.radius, 8)
  const primary = skin.family === 'netflix' || skin.family === 'apple' || skin.family === 'poster'
    ? { background: '#fff', color: '#000' }
    : { background: skin.accent, color: skin.accentFg }
  const mine = inList(item)

  return (
    <section
      className={`relative overflow-hidden mb-6 ${skin.heroH}`}
      style={{ borderRadius: skin.family === 'netflix' ? 0 : Math.max(skin.radius, 10) + 2, background: skin.surface }}
      onMouseEnter={() => setPaused(true)} onMouseLeave={() => setPaused(false)}
    >
      <AnimatePresence mode="wait">
        <motion.div key={item.id} className="absolute inset-0"
          initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.6 }}>
          {src && <img src={src} alt="" className="absolute inset-0 w-full h-full object-cover" style={{ objectPosition: 'center 22%' }} />}
          <div className="absolute inset-0" style={{ background: skin.heroOverlay }} />
          <div className={`absolute bottom-0 left-0 right-0 p-4 pb-7 sm:p-7 sm:pb-8 flex ${centered ? 'justify-center text-center' : ''}`}>
            <div className={`min-w-0 ${centered ? 'max-w-[560px]' : 'max-w-[520px]'}`}>
              <p className="flex items-center gap-2 text-[10px] uppercase tracking-[0.22em] font-bold mb-2.5" style={{ color: skin.accent === '#ffffff' || skin.accent === '#f5f5f7' ? '#fff' : skin.accent }}>
                {skin.family === 'netflix' && <span className="w-1 h-3.5 rounded-sm" style={{ background: skin.accent }} />}
                {skin.family === 'prime' || skin.family === 'disney' || skin.family === 'max' || skin.family === 'hulu' ? skin.featured : kind === 'movie' ? 'Film' : 'Series'}
                {skin.family === 'netflix' && <span className="text-white/60 tracking-[0.18em]">{kind === 'movie' ? 'Film' : 'Series'}</span>}
              </p>
              <h1 className={`font-black text-white leading-[1.02] mb-2.5 drop-shadow-lg line-clamp-2 break-words ${skin.family === 'netflix' ? 'text-[26px] sm:text-[40px] tracking-tight uppercase' : 'text-[24px] sm:text-[34px] tracking-tight'}`}>
                {item.title}
              </h1>
              <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[11px] text-white/75 mb-2.5" style={{ justifyContent: centered ? 'center' : undefined }}>
                {item.score ? <span className="flex items-center gap-1 font-bold text-emerald-400"><Star size={11} fill="currentColor" />{item.score.toFixed(1)}</span> : null}
                {item.year ? <span>{item.year}</span> : null}
                {item.genres?.length ? <span>{item.genres.slice(0, 3).join(' · ')}</span> : null}
                {skin.family === 'prime' && <span className="px-1.5 py-0.5 rounded text-[9px] font-semibold bg-black/50 border border-white/25">Included with Prime</span>}
              </div>
              {item.description && <p className="text-[12px] text-white/75 line-clamp-2 sm:line-clamp-3 mb-3 sm:mb-4 leading-relaxed">{item.description}</p>}
              <div className="flex flex-wrap items-center gap-2" style={{ justifyContent: centered ? 'center' : undefined }}>
                <button onClick={() => onPlay(item)} className="flex items-center gap-2 px-4 sm:px-5 py-2 min-h-[40px] text-[13px] font-bold transition-transform hover:scale-[1.03]"
                  style={{ ...primary, borderRadius: r }}>
                  <Play size={16} fill="currentColor" /> {skin.playLabel}
                </button>
                <button onClick={() => onInfo(item)} className="flex items-center gap-2 px-4 sm:px-5 py-2 min-h-[40px] text-[13px] font-semibold text-white backdrop-blur-md transition-colors hover:bg-white/25"
                  style={{ background: 'rgba(120,120,120,.45)', borderRadius: r }}>
                  <Info size={16} /> {skin.infoLabel}
                </button>
                <button onClick={() => onToggleList(item)} title={mine ? 'Remove from My List' : 'Add to My List'}
                  className="w-10 h-10 sm:w-9 sm:h-9 rounded-full flex items-center justify-center text-white border-2 border-white/50 hover:border-white bg-black/30 backdrop-blur-md">
                  {mine ? <Check size={16} /> : <Plus size={16} />}
                </button>
                {onOfficial && officialName && (
                  <button onClick={() => onOfficial(item)} className="hidden md:flex items-center gap-1.5 px-3 py-2 text-[11px] text-white/70 hover:text-white">
                    <ExternalLink size={12} /> Open on {officialName}
                  </button>
                )}
              </div>
            </div>
          </div>
        </motion.div>
      </AnimatePresence>
      {slides.length > 1 && (
        <div className="absolute right-3 bottom-3 sm:right-5 sm:bottom-5 flex items-center gap-1.5">
          {slides.map((s, i) => (
            <button key={s.id} onClick={() => setIndex(i)} aria-label={`Show ${s.title}`}
              className="h-1 rounded-full transition-all"
              style={{ width: i === index ? 22 : 8, background: i === index ? skin.accent === '#ffffff' ? '#fff' : skin.accent : 'rgba(255,255,255,.4)' }} />
          ))}
        </div>
      )}
    </section>
  )
}
