import { useState, type KeyboardEvent } from 'react'
import { Play, Plus, Check, Star, MonitorPlay, Download, Loader2 } from 'lucide-react'
import type { AnimeSearchResult } from '../../../shared/anime-addon-interface'
import { coverUrl } from '../../utils/cover-url'

const TYPE_COLORS: Record<string, string> = {
  TV: '#3B82F6', Movie: '#c8501a', OVA: '#F59E0B', ONA: '#c8501a', Special: '#a63f12',
}

/**
 * Poster card with a streaming-site hover preview: poster scales up and an overlay
 * reveals score, synopsis, a play button and an add-to-list button.
 * `fluid` makes it fill its grid cell; otherwise it is a fixed-width shelf card.
 */
export function AnimeCard({
  anime, onOpen, onAdd, onDownload, downloading, listed, badge, fluid,
}: {
  onDownload?: (a: AnimeSearchResult) => void
  downloading?: boolean
  anime: AnimeSearchResult
  onOpen: (a: AnimeSearchResult) => void
  onAdd?: (a: AnimeSearchResult) => void
  listed?: boolean
  /** Overrides the type badge (e.g. broadcast time on the schedule). */
  badge?: string
  fluid?: boolean
}) {
  const [loaded, setLoaded] = useState(false)
  const typeLabel = badge ?? anime.animeType
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onOpen(anime) }
  }
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => onOpen(anime)}
      onKeyDown={onKey}
      className={`group text-left cursor-pointer outline-none ${fluid ? 'w-full min-w-0' : 'shrink-0 snap-start w-[124px] sm:w-[148px]'}`}
    >
      <div
        className="aspect-[2/3] rounded-xl overflow-hidden mb-1.5 relative transition-all duration-300 ease-out group-hover:scale-[1.06] group-hover:z-10 group-hover:shadow-2xl group-focus-visible:ring-2 ring-[color:var(--accent)]"
        style={{ background: 'var(--surface-2)' }}
      >
        {anime.coverUrl ? (
          <>
            <img
              src={coverUrl(anime.coverUrl) || ''}
              alt={anime.title}
              loading="lazy"
              onLoad={() => setLoaded(true)}
              className={`w-full h-full object-cover transition-opacity duration-500 ${loaded ? 'opacity-100' : 'opacity-0'}`}
            />
            {!loaded && <div className="absolute inset-0 animate-pulse" style={{ background: 'var(--surface-3)' }} />}
          </>
        ) : (
          <div className="w-full h-full flex items-center justify-center">
            <MonitorPlay size={24} className="text-[color:var(--text-3)]" />
          </div>
        )}
        {typeLabel && (
          <span
            className="absolute top-1.5 left-1.5 px-1.5 py-0.5 rounded text-[8px] font-bold uppercase tracking-wide text-white"
            style={{ background: badge ? 'rgba(0,0,0,0.75)' : TYPE_COLORS[typeLabel] || '#6B7280' }}
          >
            {typeLabel}
          </span>
        )}
        {/* Hover preview */}
        <div
          className="absolute inset-0 flex flex-col justify-end p-2 opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100 transition-opacity duration-200"
          style={{ background: 'linear-gradient(0deg, rgba(8,8,9,0.96) 0%, rgba(8,8,9,0.82) 55%, rgba(8,8,9,0.25) 100%)' }}
        >
          <p className="text-[11px] font-bold text-white leading-tight line-clamp-2 mb-1">{anime.title}</p>
          <div className="flex items-center gap-1.5 text-[10px] mb-1">
            {anime.score != null && (
              <span className="flex items-center gap-0.5 text-amber-400 font-semibold"><Star size={9} fill="currentColor" /> {anime.score}</span>
            )}
            {anime.year && <span className="text-[color:var(--text-2)]">{anime.year}</span>}
            {anime.episodeCount ? (
              <span className="text-[color:var(--text-3)] ml-auto">{anime.animeType === 'Movie' ? 'Movie' : `${anime.episodeCount} eps`}</span>
            ) : null}
          </div>
          {anime.description && (
            <p className="text-[9.5px] leading-snug text-[color:var(--text-2)] line-clamp-4 mb-1.5">{anime.description}</p>
          )}
          <div className="flex items-center gap-1.5">
            <span
              className="flex-1 min-w-0 h-10 sm:h-7 rounded-md flex items-center justify-center gap-1 text-[10px] font-bold text-white"
              style={{ background: 'var(--accent)' }}
            >
              <Play size={10} fill="currentColor" /> Watch
            </span>
            {onDownload && (
              <button
                type="button"
                title="Download"
                onClick={(e) => { e.stopPropagation(); onDownload(anime) }}
                className="hidden sm:flex w-7 h-7 rounded-md items-center justify-center text-white hover:bg-white/25 transition-colors"
                style={{ background: 'rgba(255,255,255,0.14)', border: '1px solid rgba(255,255,255,0.25)' }}
              >
                {downloading ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />}
              </button>
            )}
            {onAdd && (
              <button
                type="button"
                title={listed ? 'In My List' : 'Add to My List'}
                onClick={(e) => { e.stopPropagation(); onAdd(anime) }}
                className="shrink-0 w-10 h-10 sm:w-7 sm:h-7 rounded-md flex items-center justify-center text-white hover:bg-white/25 transition-colors"
                style={{ background: 'rgba(255,255,255,0.14)', border: '1px solid rgba(255,255,255,0.25)' }}
              >
                {listed ? <Check size={13} /> : <Plus size={13} />}
              </button>
            )}
          </div>
        </div>
      </div>
      <p className="text-[11px] leading-tight text-[color:var(--text-1)] font-medium line-clamp-2 group-hover:text-white transition-colors">
        {anime.title}
      </p>
      {anime.genres && anime.genres.length > 0 && (
        <p className="text-[10px] text-[color:var(--text-3)] truncate mt-0.5">{anime.genres.slice(0, 2).join(' · ')}</p>
      )}
    </div>
  )
}
