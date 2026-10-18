import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ChevronLeft, ChevronRight, Play, Plus, Check, Download, Loader2, Info, Film, Star, CheckCircle2, ExternalLink } from 'lucide-react'
import type { AnimeSearchResult } from '../../../shared/anime-addon-interface'
import type { PlaybackState } from '../../../shared/types'
import { coverUrl } from '../../utils/cover-url'
import type { Skin } from './skins'

export interface CardActions {
  onDetails: (item: AnimeSearchResult) => void
  onPlay: (item: AnimeSearchResult) => void
  onToggleList: (item: AnimeSearchResult) => void
  inList: (item: AnimeSearchResult) => boolean
  onDownload: (item: AnimeSearchResult) => void
  downloading: (item: AnimeSearchResult) => boolean
  onOfficial?: (item: AnimeSearchResult) => void
}

const art = (item: AnimeSearchResult, landscape: boolean): string | null =>
  coverUrl(landscape ? item.bannerUrl || item.coverUrl : item.coverUrl || item.bannerUrl)

const Poster = memo(function Poster({ item, skin, landscape, className = '' }: { item: AnimeSearchResult; skin: Skin; landscape: boolean; className?: string }) {
  const [ok, setOk] = useState(false)
  const src = art(item, landscape)
  return (
    <div className={`absolute inset-0 ${className}`} style={{ background: skin.surface }}>
      {src ? (
        // width/height are intrinsic-ratio hints only (CSS sizes the box); async decode
        // keeps JPEG decoding off the scroll/paint path.
        <img src={src} alt={item.title} loading="lazy" decoding="async" draggable={false}
          width={landscape ? 780 : 342} height={landscape ? 439 : 513} onLoad={() => setOk(true)}
          className={`w-full h-full object-cover transition-opacity duration-500 ${ok ? 'opacity-100' : 'opacity-0'}`} />
      ) : null}
      {(!src || !ok) && (
        <div className={`absolute inset-0 flex items-center justify-center ${src ? 'animate-pulse' : ''}`}>
          {!src && <Film size={22} style={{ color: skin.muted }} />}
        </div>
      )}
    </div>
  )
})

function RoundBtn({ title, onClick, children, solid, skin }: {
  title: string; onClick: () => void; children: React.ReactNode; solid?: boolean; skin: Skin
}) {
  return (
    <button title={title} aria-label={title}
      onClick={(e) => { e.stopPropagation(); onClick() }}
      className="w-7 h-7 rounded-full flex items-center justify-center transition-transform hover:scale-110"
      style={solid
        ? { background: '#fff', color: '#000' }
        : { background: 'rgba(20,20,20,.75)', color: '#fff', border: `1.5px solid rgba(255,255,255,.55)` }}
    >{children}</button>
  )
}

function ActionRow({ item, skin, actions }: { item: AnimeSearchResult; skin: Skin; actions: CardActions }) {
  const inList = actions.inList(item)
  return (
    <div className="flex items-center gap-1.5">
      <RoundBtn skin={skin} solid title={skin.playLabel} onClick={() => actions.onPlay(item)}><Play size={12} fill="currentColor" /></RoundBtn>
      <RoundBtn skin={skin} title={inList ? 'Remove from My List' : 'Add to My List'} onClick={() => actions.onToggleList(item)}>
        {inList ? <Check size={13} /> : <Plus size={13} />}
      </RoundBtn>
      <RoundBtn skin={skin} title="Download" onClick={() => actions.onDownload(item)}>
        {actions.downloading(item) ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
      </RoundBtn>
      <span className="ml-auto" />
      <RoundBtn skin={skin} title="Details" onClick={() => actions.onDetails(item)}><Info size={13} /></RoundBtn>
    </div>
  )
}

function Meta({ item, skin }: { item: AnimeSearchResult; skin: Skin }) {
  return (
    <div className="flex items-center gap-2 text-[10px]" style={{ color: skin.muted }}>
      {item.score ? <span className="flex items-center gap-0.5 font-semibold text-emerald-400"><Star size={9} fill="currentColor" />{item.score.toFixed(1)}</span> : null}
      {item.year ? <span>{item.year}</span> : null}
      {item.genres?.[0] ? <span className="truncate">{item.genres.slice(0, 2).join(' · ')}</span> : null}
    </div>
  )
}

/** One title card — layout varies by skin.card. Memoised: rails/grids re-render often. */
export const TitleCard = memo(function TitleCard({ item, index, skin, actions, width, landscape }: {
  item: AnimeSearchResult; index: number; skin: Skin; actions: CardActions; width?: number; landscape?: boolean
}) {
  const land = landscape ?? skin.aspect !== 'aspect-[2/3]'
  const w = width ?? skin.cardW
  const badge = skin.badge(item, index)
  const open = () => actions.onDetails(item)
  // Off-screen art boxes skip layout/paint (see .mv-card-art in globals.css).
  const artCls = `mv-card-art${skin.aspect === 'aspect-[2/3]' ? ' mv-card-art-portrait' : ''}`

  if (skin.card === 'expand') {
    // Hover-expand card: grows in place and reveals actions + meta (CSS transition, see .mv-card-expand).
    return (
      <div
        className="relative shrink-0 cursor-pointer group mv-card-expand"
        style={{ width: w }}
        onClick={open}
      >
        <div className={`relative ${skin.aspect} overflow-hidden ${artCls}`} style={{ borderRadius: skin.radius }}>
          <Poster item={item} skin={skin} landscape={land} />
          <div className="absolute inset-x-0 bottom-0 h-1/2 bg-gradient-to-t from-black/85 to-transparent" />
          <p className="absolute left-2 right-2 bottom-1.5 text-[11px] font-bold text-white drop-shadow truncate group-hover:opacity-0 transition-opacity">{item.title}</p>
        </div>
        <div className="absolute left-0 right-0 top-full -mt-px px-2.5 pt-2 pb-2.5 opacity-0 group-hover:opacity-100 transition-opacity duration-200 pointer-events-none group-hover:pointer-events-auto shadow-2xl"
          style={{ background: skin.surface, borderBottomLeftRadius: skin.radius, borderBottomRightRadius: skin.radius }}>
          <ActionRow item={item} skin={skin} actions={actions} />
          <p className="text-[11px] font-bold text-white truncate mt-2">{item.title}</p>
          <Meta item={item} skin={skin} />
        </div>
      </div>
    )
  }

  if (skin.card === 'badge') {
    // Carousel card with an "Included with ..." badge and a hover action strip.
    return (
      <div className="relative shrink-0 group cursor-pointer" style={{ width: w }} onClick={open}>
        <div className={`relative ${skin.aspect} overflow-hidden transition-all duration-200 group-hover:ring-2 ${artCls}`}
          style={{ borderRadius: skin.radius, ['--tw-ring-color' as string]: skin.accent }}>
          <Poster item={item} skin={skin} landscape={land} />
          <div className="absolute inset-0 bg-gradient-to-t from-black/80 via-transparent to-transparent" />
          {badge && (
            <span className="absolute left-2 bottom-2 flex items-center gap-1 text-[9.5px] font-semibold text-white px-1.5 py-0.5 rounded"
              style={{ background: 'rgba(15,23,30,.82)' }}>
              <CheckCircle2 size={10} style={{ color: skin.accent }} /> {badge}
            </span>
          )}
          <div className="absolute right-2 top-2 opacity-0 group-hover:opacity-100 transition-opacity"><RoundBtn skin={skin} title="Details" onClick={open}><Info size={13} /></RoundBtn></div>
        </div>
        <div className="pt-1.5 flex items-center gap-1.5">
          <div className="min-w-0 flex-1">
            <p className="text-[12px] font-semibold text-white truncate">{item.title}</p>
            <Meta item={item} skin={skin} />
          </div>
          <RoundBtn skin={skin} title={actions.inList(item) ? 'Remove from My List' : 'Add to My List'} onClick={() => actions.onToggleList(item)}>
            {actions.inList(item) ? <Check size={13} /> : <Plus size={13} />}
          </RoundBtn>
        </div>
      </div>
    )
  }

  if (skin.card === 'glow') {
    return (
      <div className="relative shrink-0 group cursor-pointer mv-card-lift" style={{ width: w }} onClick={open}>
        <div className={`relative ${skin.aspect} overflow-hidden transition-shadow duration-300 ${artCls}`}
          style={{ borderRadius: skin.radius, border: '2px solid transparent' }}>
          <Poster item={item} skin={skin} landscape={land} />
          <div className="absolute inset-0 bg-gradient-to-t from-black/85 via-black/10 to-transparent" />
          {badge && <span className="absolute left-2 top-2 text-[9px] font-bold px-1.5 py-0.5 rounded-full" style={{ background: skin.accent, color: skin.accentFg }}>{badge}</span>}
          <div className="absolute inset-x-2 bottom-2">
            <p className="text-[11.5px] font-bold text-white truncate">{item.title}</p>
            <div className="flex items-center gap-1.5 mt-1 opacity-0 group-hover:opacity-100 transition-opacity">
              <RoundBtn skin={skin} solid title={skin.playLabel} onClick={() => actions.onPlay(item)}><Play size={11} fill="currentColor" /></RoundBtn>
              <RoundBtn skin={skin} title={actions.inList(item) ? 'Remove from My List' : 'Add to My List'} onClick={() => actions.onToggleList(item)}>
                {actions.inList(item) ? <Check size={12} /> : <Plus size={12} />}
              </RoundBtn>
              <RoundBtn skin={skin} title="Download" onClick={() => actions.onDownload(item)}>
                {actions.downloading(item) ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
              </RoundBtn>
            </div>
          </div>
          <div className="absolute inset-0 opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none"
            style={{ borderRadius: skin.radius, boxShadow: `inset 0 0 0 2px ${skin.accent}, 0 0 22px ${skin.accent}66` }} />
        </div>
      </div>
    )
  }

  // caption: art with title/meta below (poster rails, Max/Hulu/Apple/default).
  return (
    <div className="relative shrink-0 group cursor-pointer text-left mv-card-lift" style={{ width: w }} onClick={open}>
      <div className={`relative ${skin.aspect} overflow-hidden shadow-lg ${artCls}`} style={{ borderRadius: skin.radius }}>
        <Poster item={item} skin={skin} landscape={land} className="transition-transform duration-300 group-hover:scale-105" />
        {badge && <span className="absolute left-2 top-2 text-[9px] font-bold px-1.5 py-0.5 rounded"
          style={{ background: skin.family === 'default' ? 'var(--accent)' : skin.accent, color: skin.accentFg }}>{badge}</span>}
        <div className="absolute inset-0 bg-gradient-to-t from-black/75 via-transparent to-transparent opacity-0 group-hover:opacity-100 transition-opacity" />
        <div className="absolute inset-x-2 bottom-2 flex items-center gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
          <RoundBtn skin={skin} solid title={skin.playLabel} onClick={() => actions.onPlay(item)}><Play size={11} fill="currentColor" /></RoundBtn>
          <RoundBtn skin={skin} title={actions.inList(item) ? 'Remove from My List' : 'Add to My List'} onClick={() => actions.onToggleList(item)}>
            {actions.inList(item) ? <Check size={12} /> : <Plus size={12} />}
          </RoundBtn>
          <RoundBtn skin={skin} title="Download" onClick={() => actions.onDownload(item)}>
            {actions.downloading(item) ? <Loader2 size={11} className="animate-spin" /> : <Download size={11} />}
          </RoundBtn>
          {actions.onOfficial && <span className="ml-auto"><RoundBtn skin={skin} title="Open official service" onClick={() => actions.onOfficial!(item)}><ExternalLink size={11} /></RoundBtn></span>}
        </div>
      </div>
      <div className="pt-1.5 px-0.5">
        <p className="text-[12px] font-semibold truncate" style={{ color: skin.text }}>{item.title}</p>
        <Meta item={item} skin={skin} />
      </div>
    </div>
  )
})

/** Big rank number beside a card for Top 10 rows. */
const RankedCard = memo(function RankedCard({ item, index, skin, actions }: { item: AnimeSearchResult; index: number; skin: Skin; actions: CardActions }) {
  const cardW = skin.rank === 'corner' ? skin.cardW : 128
  const portrait = skin.rank !== 'corner'
  const poster: Skin = useMemo(
    () => (portrait ? { ...skin, aspect: 'aspect-[2/3]', card: skin.card === 'expand' ? 'glow' : skin.card } : skin),
    [skin, portrait],
  )
  if (skin.rank === 'corner') {
    return (
      <div className="relative shrink-0">
        <TitleCard item={item} index={index} skin={skin} actions={actions} width={cardW} />
        <span className="absolute -left-1 -top-1 min-w-[26px] h-[26px] px-1.5 rounded-md flex items-center justify-center text-[13px] font-black z-10 shadow-lg"
          style={{ background: skin.accent, color: skin.accentFg }}>{index + 1}</span>
      </div>
    )
  }
  const outline = skin.rank === 'outline'
  return (
    <div className="relative shrink-0 flex items-end" style={{ width: cardW + 76 }}>
      <span aria-hidden className="select-none font-black leading-[0.8] -mr-5 z-0"
        style={{
          fontSize: 118, letterSpacing: '-0.08em',
          color: outline ? (skin.bg.startsWith('var') ? 'var(--base)' : skin.bg) : skin.accent,
          WebkitTextStroke: outline ? '3px rgba(255,255,255,.5)' : undefined,
          marginBottom: 4,
        }}>{index + 1}</span>
      <div className="relative z-10">
        <TitleCard item={item} index={index} skin={poster} actions={actions} width={cardW} landscape={false} />
      </div>
    </div>
  )
})

export const Rail = memo(function Rail({
  title, subtitle, items, skin, actions, ranked = false, limit = 20, icon, seeMore,
}: {
  title: string; subtitle?: string; items: AnimeSearchResult[]; skin: Skin; actions: CardActions
  ranked?: boolean; limit?: number; icon?: React.ReactNode; seeMore?: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [canL, setCanL] = useState(false)
  const [canR, setCanR] = useState(true)
  const frame = useRef(0)
  const update = useCallback(() => {
    const el = ref.current
    if (!el) return
    // Only re-render when an arrow's visibility actually flips.
    const l = el.scrollLeft > 8
    const r = el.scrollLeft + el.clientWidth < el.scrollWidth - 8
    setCanL((prev) => (prev === l ? prev : l))
    setCanR((prev) => (prev === r ? prev : r))
  }, [])
  // Coalesce scroll events into one measurement per animation frame.
  const onScroll = useCallback(() => {
    if (frame.current) return
    frame.current = requestAnimationFrame(() => { frame.current = 0; update() })
  }, [update])
  useEffect(() => () => { if (frame.current) cancelAnimationFrame(frame.current) }, [])
  useEffect(() => { update() }, [items.length, update])
  const list = useMemo(() => items.slice(0, limit), [items, limit])
  if (items.length === 0) return null
  const scroll = (dir: number) => ref.current?.scrollBy({ left: dir * (ref.current.clientWidth * 0.85), behavior: 'smooth' })
  const arrow = (side: 'l' | 'r', show: boolean) => show && (
    <button onClick={() => scroll(side === 'l' ? -1 : 1)} aria-label={side === 'l' ? 'Scroll left' : 'Scroll right'}
      className={`absolute ${side === 'l' ? 'left-0' : 'right-0'} top-6 bottom-8 w-10 z-30 hidden sm:flex items-center justify-center opacity-0 group-hover/rail:opacity-100 transition-opacity text-white`}
      style={{ background: `linear-gradient(${side === 'l' ? '90deg' : '270deg'}, rgba(0,0,0,.65), transparent)` }}>
      {side === 'l' ? <ChevronLeft size={26} /> : <ChevronRight size={26} />}
    </button>
  )
  return (
    <section className="mb-2 group/rail relative">
      <div className="flex items-baseline gap-2 mb-1 px-1 min-w-0">
        {icon}
        <h3 className="text-[15px] font-bold tracking-tight truncate min-w-0" style={{ color: skin.text }}>{title}</h3>
        {subtitle && <span className="text-[10px] truncate" style={{ color: skin.muted }}>{subtitle}</span>}
        {seeMore && (
          <button onClick={seeMore} className="ml-1 shrink-0 text-[11px] font-semibold flex items-center sm:opacity-0 group-hover/rail:opacity-100 transition-opacity" style={{ color: skin.accent }}>
            See all <ChevronRight size={12} />
          </button>
        )}
      </div>
      <div className="relative">
        {arrow('l', canL)}
        {arrow('r', canR)}
        <div ref={ref} onScroll={onScroll} className="flex gap-2.5 overflow-x-auto pt-6 pb-10 -mt-3 -mb-6 px-1 provider-rail"
          style={{ scrollbarWidth: 'none' }}>
          {list.map((item, i) => ranked
            ? <RankedCard key={`${item.id}-${i}`} item={item} index={i} skin={skin} actions={actions} />
            : <TitleCard key={`${item.id}-${i}`} item={item} index={i} skin={skin} actions={actions} />)}
        </div>
      </div>
    </section>
  )
})

/** Continue Watching - landscape cards with a resume progress bar. */
export function ContinueRail({ entries, skin, onResume, onRemove }: {
  entries: PlaybackState[]; skin: Skin; onResume: (mediaId: string) => void; onRemove: (mediaId: string) => void
}) {
  if (entries.length === 0) return null
  return (
    <section className="mb-4">
      <h3 className="text-[15px] font-bold tracking-tight mb-2 px-1" style={{ color: skin.text }}>Continue watching</h3>
      <div className="flex gap-2.5 overflow-x-auto pb-2 px-1 provider-rail" style={{ scrollbarWidth: 'none' }}>
        {entries.slice(0, 16).map((h) => {
          const pct = h.durationSecs > 0 ? Math.min(100, Math.max(3, (h.positionSecs / h.durationSecs) * 100)) : 0
          const src = coverUrl(h.coverUrl)
          return (
            <div key={h.mediaId} className="relative shrink-0 group" style={{ width: 'min(220px, 70vw)' }}>
              <button onClick={() => onResume(h.mediaId)} className="block w-full text-left">
                <div className="relative aspect-video overflow-hidden" style={{ borderRadius: skin.radius, background: skin.surface }}>
                  {src && <img src={src} alt="" loading="lazy" decoding="async" className="w-full h-full object-cover" />}
                  <div className="absolute inset-0 bg-gradient-to-t from-black/80 to-transparent" />
                  <div className="absolute inset-0 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity">
                    <span className="w-10 h-10 rounded-full flex items-center justify-center bg-black/60 border-2 border-white text-white"><Play size={16} fill="currentColor" /></span>
                  </div>
                  <div className="absolute left-0 right-0 bottom-0 h-1 bg-white/25">
                    <div className="h-full" style={{ width: `${pct}%`, background: skin.accent }} />
                  </div>
                </div>
                <p className="text-[12px] font-semibold truncate mt-1.5" style={{ color: skin.text }}>{h.title}</p>
                <p className="text-[10px]" style={{ color: skin.muted }}>
                  {h.episodeNumber ? `Episode ${h.episodeNumber} · ` : ''}{Math.round(pct)}% watched
                </p>
              </button>
              <button onClick={() => onRemove(h.mediaId)} title="Remove from Continue Watching"
                className="absolute top-1.5 right-1.5 w-8 h-8 sm:w-6 sm:h-6 rounded-full bg-black/70 text-white text-xs sm:opacity-0 group-hover:opacity-100 transition-opacity">×</button>
            </div>
          )
        })}
      </div>
    </section>
  )
}

/** Skeleton rows while the catalog loads. */
export function RailSkeleton({ skin }: { skin: Skin }) {
  return (
    <div className="space-y-6">
      {[0, 1, 2].map((r) => (
        <div key={r}>
          <div className="h-4 w-40 rounded mb-3 animate-pulse" style={{ background: skin.surface }} />
          <div className="flex gap-2.5 overflow-hidden">
            {Array.from({ length: 8 }).map((_, i) => (
              <div key={i} className={`shrink-0 ${skin.aspect} animate-pulse`} style={{ width: skin.cardW, background: skin.surface, borderRadius: skin.radius }} />
            ))}
          </div>
        </div>
      ))}
    </div>
  )
}

/** Grid layout for search / genre / non-home tabs. */
export const TitleGrid = memo(function TitleGrid({ items, skin, actions }: { items: AnimeSearchResult[]; skin: Skin; actions: CardActions }) {
  const land = skin.aspect !== 'aspect-[2/3]'
  const min = land ? 210 : 150
  // One shared skin object so memoised cells don't all re-render on every parent render.
  const s: Skin = useMemo(() => (skin.card === 'expand' ? { ...skin, card: 'caption' } : skin), [skin])
  return (
    <div className="grid gap-x-3 gap-y-5" style={{ gridTemplateColumns: `repeat(auto-fill, minmax(min(${min}px, calc(50% - 6px)), 1fr))` }}>
      {items.map((item, i) => (
        <GridCell key={`${item.id}-${i}`} item={item} index={i} skin={s} actions={actions} />
      ))}
    </div>
  )
})

const GridCell = memo(function GridCell({ item, index, skin, actions }: { item: AnimeSearchResult; index: number; skin: Skin; actions: CardActions }) {
  return (
    <div className="[&>*]:!w-full">
      <TitleCard item={item} index={index} skin={skin} actions={actions} width={undefined} />
    </div>
  )
})
