import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'

/** Horizontally scrolling shelf with hover arrow buttons (streaming-site style). */
export function AnimeShelf({
  title, icon, count, actionLabel, onAction, children, itemCount,
}: {
  title: string
  icon?: ReactNode
  count?: number
  actionLabel?: string
  onAction?: () => void
  children: ReactNode
  /** Changes when content changes so arrow state is recomputed. */
  itemCount?: number
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [canLeft, setCanLeft] = useState(false)
  const [canRight, setCanRight] = useState(false)

  const update = useCallback(() => {
    const el = ref.current
    if (!el) return
    setCanLeft(el.scrollLeft > 4)
    setCanRight(el.scrollLeft + el.clientWidth < el.scrollWidth - 4)
  }, [])

  useEffect(() => {
    update()
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(update)
    ro.observe(el)
    return () => ro.disconnect()
  }, [update, itemCount])

  const scroll = (dir: -1 | 1) => {
    const el = ref.current
    if (el) el.scrollBy({ left: dir * el.clientWidth * 0.85, behavior: 'smooth' })
  }

  return (
    <section className="mb-5 group/shelf">
      <div className="flex items-center gap-2 mb-1.5 px-1">
        {icon}
        <h3 className="text-[14px] font-bold tracking-tight text-[color:var(--text-1)]">{title}</h3>
        {count != null && <span className="text-[10px] text-[color:var(--text-3)]">{count}</span>}
        {onAction && (
          <button
            onClick={onAction}
            className="ml-auto min-h-10 sm:min-h-0 flex items-center gap-0.5 text-[11px] font-semibold uppercase tracking-wide transition-colors hover:text-white"
            style={{ color: 'var(--accent-hover)' }}
          >
            {actionLabel || 'See all'} <ChevronRight size={13} />
          </button>
        )}
      </div>
      <div className="relative">
        <div
          ref={ref}
          onScroll={update}
          className="flex gap-2.5 sm:gap-3 overflow-x-auto scroll-smooth snap-x snap-mandatory sm:snap-none overscroll-x-contain px-1 py-3 -my-1 scroll-px-1"
          style={{ scrollbarWidth: 'none', WebkitOverflowScrolling: 'touch' }}
        >
          {children}
        </div>
        {canLeft && (
          <button
            onClick={() => scroll(-1)}
            aria-label="Scroll left"
            className="absolute left-0 top-0 bottom-0 w-10 z-20 hidden sm:flex items-center justify-center text-white opacity-0 group-hover/shelf:opacity-100 transition-opacity"
            style={{ background: 'linear-gradient(90deg, var(--base) 10%, transparent)' }}
          >
            <span className="w-7 h-7 rounded-full flex items-center justify-center" style={{ background: 'rgba(0,0,0,0.6)' }}><ChevronLeft size={16} /></span>
          </button>
        )}
        {canRight && (
          <button
            onClick={() => scroll(1)}
            aria-label="Scroll right"
            className="absolute right-0 top-0 bottom-0 w-10 z-20 hidden sm:flex items-center justify-center text-white opacity-0 group-hover/shelf:opacity-100 transition-opacity"
            style={{ background: 'linear-gradient(270deg, var(--base) 10%, transparent)' }}
          >
            <span className="w-7 h-7 rounded-full flex items-center justify-center" style={{ background: 'rgba(0,0,0,0.6)' }}><ChevronRight size={16} /></span>
          </button>
        )}
      </div>
    </section>
  )
}

export function ShelfSkeleton({ title }: { title: string }) {
  return (
    <section className="mb-5">
      <h3 className="text-[14px] font-bold tracking-tight text-[color:var(--text-1)] mb-1.5 px-1">{title}</h3>
      <div className="flex gap-3 px-1 py-2 overflow-hidden">
        {Array.from({ length: 9 }).map((_, i) => (
          <div key={i} className="shrink-0 w-[124px] sm:w-[148px] aspect-[2/3] rounded-xl animate-pulse" style={{ background: 'var(--surface-2)' }} />
        ))}
      </div>
    </section>
  )
}
