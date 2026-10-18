import { useEffect, useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { X, Play, Plus, Check, Download, Loader2, Star, Layers, ExternalLink, Film } from 'lucide-react'
import { invoke } from '../../hooks/useIpc'
import { IPC } from '../../../shared/ipc-channels'
import { coverUrl, tmdbImageSize } from '../../utils/cover-url'
import type { AnimeSearchResult, AnimeEpisode } from '../../../shared/anime-addon-interface'
import type { ProviderAvailability, StreamingProvider } from '../../../shared/types'
import { BulkDownloadModal } from '../BulkDownloadModal'
import type { Skin } from './skins'

type Extras = AnimeSearchResult & { runtime?: string | number; cast?: Array<string | { name: string }>; rating?: string }

const AVAIL_LABEL: Record<ProviderAvailability['type'], string> = {
  subscription: 'Stream', free: 'Free', ads: 'Free with ads', rent: 'Rent', buy: 'Buy',
}

export function TitleDetailModal({
  item, kind, skin, providers, region, activeProvider, inList, downloading,
  onClose, onPlay, onToggleList, onDownload, onOfficial,
}: {
  item: AnimeSearchResult | null
  kind: 'movie' | 'series'
  skin: Skin
  providers: StreamingProvider[]
  region: string
  activeProvider?: StreamingProvider
  inList: boolean
  downloading: boolean
  onClose: () => void
  onPlay: (item: AnimeSearchResult) => void
  onToggleList: (item: AnimeSearchResult) => void
  onDownload: (item: AnimeSearchResult) => void
  onOfficial: (providerId: string, title: string) => void
}) {
  const [detail, setDetail] = useState<Extras | null>(null)
  const [availability, setAvailability] = useState<ProviderAvailability[]>([])
  const [episodes, setEpisodes] = useState<AnimeEpisode[]>([])
  const [season, setSeason] = useState<number | 'all'>('all')
  const [bulkOpen, setBulkOpen] = useState(false)

  useEffect(() => {
    setDetail(null); setAvailability([]); setEpisodes([]); setSeason('all'); setBulkOpen(false)
    if (!item) return
    let alive = true
    ;(async () => {
      try {
        let full: AnimeSearchResult | null = null
        if (item.tmdbId) full = await invoke<AnimeSearchResult | null>(IPC.MEDIA_TMDB_DETAILS, item.tmdbId, kind)
        else if (item.imdbId) full = await invoke<AnimeSearchResult | null>(kind === 'movie' ? IPC.MOVIE_DETAILS : IPC.SERIES_DETAILS, item.imdbId)
        if (alive && full) setDetail({ ...item, ...Object.fromEntries(Object.entries(full).filter(([, v]) => v != null && v !== '')) } as Extras)
      } catch { /* base data is enough */ }
    })()
    if (item.tmdbId) {
      invoke<ProviderAvailability[]>(IPC.MEDIA_AVAILABILITY, item.tmdbId, kind, region)
        .then((rows) => { if (alive) setAvailability(rows || []) }).catch(() => {})
    }
    if (kind === 'series' && item.imdbId) {
      invoke<AnimeEpisode[]>(IPC.SERIES_EPISODES, item.imdbId)
        .then((rows) => { if (alive) setEpisodes(rows || []) }).catch(() => {})
    }
    return () => { alive = false }
  }, [item?.id, item?.imdbId, item?.tmdbId, kind, region])

  useEffect(() => {
    if (!item) return
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape' && !bulkOpen) onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [item, onClose, bulkOpen])

  const d: Extras | null = item ? (detail ?? item) : null
  const seasonOf = (ep: AnimeEpisode) => Number(ep.id.split(':')[1]) || 1
  const seasons = useMemo(() => [...new Set(episodes.map(seasonOf))].sort((a, b) => a - b), [episodes])
  const shown = season === 'all' ? episodes : episodes.filter((ep) => seasonOf(ep) === season)
  const cast = (d?.cast || []).map((c) => (typeof c === 'string' ? c : c.name)).filter(Boolean)
  const pillR = skin.pill ? 999 : Math.min(skin.radius, 8)
  const primary = skin.family === 'netflix' || skin.family === 'apple' || skin.family === 'poster'
    ? { background: '#fff', color: '#000' } : { background: skin.accent, color: skin.accentFg }

  const rows = availability.map((a) => ({ a, p: providers.find((p) => p.id === a.providerId) })).filter((r) => r.p)
  const bg = skin.bg.startsWith('var') ? 'var(--surface-1)' : skin.surface
  // Same TMDB renditions before and after details load, so the art isn't fetched twice.
  const bannerSrc = d ? coverUrl(tmdbImageSize(d.bannerUrl, 'w1280') || d.coverUrl) : null
  const posterSrc = d ? coverUrl(tmdbImageSize(d.coverUrl, 'w342') || d.bannerUrl) : null

  return (
    <>
      <AnimatePresence>
        {item && d && !bulkOpen && (
          <motion.div className="fixed inset-0 z-[80] flex items-start justify-center overflow-y-auto overflow-x-hidden p-0 sm:py-8 sm:px-4"
            style={{ background: 'rgba(0,0,0,.72)', backdropFilter: 'blur(6px)' }}
            initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}
            onMouseDown={(e) => { if (e.target === e.currentTarget) onClose() }}>
            <motion.div role="dialog" aria-label={d.title}
              className="relative w-full max-w-[860px] min-h-full sm:min-h-0 overflow-hidden shadow-2xl max-sm:!rounded-none max-sm:!border-0"
              style={{ background: bg, borderRadius: Math.max(skin.radius, 10) + 2, border: `1px solid ${skin.border}`, color: skin.text }}
              initial={{ y: 28, scale: 0.97, opacity: 0 }} animate={{ y: 0, scale: 1, opacity: 1 }} exit={{ y: 16, opacity: 0 }}
              transition={{ type: 'spring', stiffness: 320, damping: 30 }}>
              <button onClick={onClose} aria-label="Close" className="absolute top-3 right-3 z-20 w-10 h-10 sm:w-8 sm:h-8 rounded-full bg-black/70 text-white flex items-center justify-center hover:bg-black">
                <X size={16} />
              </button>

              {/* Backdrop */}
              <div className="relative h-[230px] sm:h-[300px]">
                {bannerSrc ? <img src={bannerSrc} alt="" className="absolute inset-0 w-full h-full object-cover" />
                  : <div className="absolute inset-0 flex items-center justify-center" style={{ background: skin.surface }}><Film size={40} style={{ color: skin.muted }} /></div>}
                <div className="absolute inset-0" style={{ background: `linear-gradient(0deg, ${bg} 0%, transparent 62%), linear-gradient(90deg, rgba(0,0,0,.55), transparent 60%)` }} />
                <div className="absolute left-4 right-4 sm:left-7 sm:right-7 bottom-4 sm:bottom-5 flex items-end gap-5">
                  {posterSrc && <img src={posterSrc} alt="" className="hidden sm:block w-[110px] aspect-[2/3] object-cover rounded-lg shadow-2xl ring-1 ring-white/15 shrink-0" />}
                  <div className="min-w-0 flex-1">
                    <p className="text-[10px] uppercase tracking-[0.2em] font-bold mb-1" style={{ color: skin.accent === '#ffffff' || skin.accent === '#f5f5f7' ? '#fff' : skin.accent }}>
                      {skin.label}
                    </p>
                    <h2 className="text-[22px] sm:text-[30px] font-black leading-tight text-white drop-shadow-lg line-clamp-3 break-words">{d.title}</h2>
                    <div className="flex flex-wrap items-center gap-2.5 mt-1.5 text-[11.5px] text-white/80">
                      {d.score ? <span className="flex items-center gap-1 font-bold text-emerald-400"><Star size={12} fill="currentColor" />{d.score.toFixed(1)} rating</span> : null}
                      {d.year ? <span>{d.year}</span> : null}
                      {d.runtime ? <span>{typeof d.runtime === 'number' ? `${d.runtime} min` : d.runtime}</span> : null}
                      <span className="px-1.5 py-px rounded border border-white/30 text-[9.5px] uppercase tracking-wider">{kind === 'movie' ? 'Film' : 'Series'}</span>
                      {kind === 'series' && episodes.length > 0 && <span>{seasons.length} season{seasons.length === 1 ? '' : 's'} · {episodes.length} episodes</span>}
                    </div>
                  </div>
                </div>
              </div>

              <div className="px-4 sm:px-7 pb-7 pt-1">
                {/* Actions */}
                <div className="flex flex-wrap items-center gap-2 mb-5">
                  <button onClick={() => onPlay(d)} className="flex items-center gap-2 px-5 sm:px-6 py-2.5 min-h-[40px] text-[13px] font-bold hover:scale-[1.03] transition-transform" style={{ ...primary, borderRadius: pillR }}>
                    <Play size={16} fill="currentColor" /> {skin.playLabel}
                  </button>
                  <button onClick={() => onToggleList(d)} className="flex items-center gap-2 px-4 py-2.5 text-[12.5px] font-semibold"
                    style={{ border: `1.5px solid ${skin.border}`, borderRadius: pillR, background: 'rgba(255,255,255,.06)' }}>
                    {inList ? <Check size={15} /> : <Plus size={15} />} {inList ? 'In My List' : 'Add to My List'}
                  </button>
                  <button onClick={() => onDownload(d)} disabled={downloading} className="flex items-center gap-2 px-4 py-2.5 text-[12.5px] font-semibold disabled:opacity-60"
                    style={{ border: `1.5px solid ${skin.border}`, borderRadius: pillR, background: 'rgba(255,255,255,.06)' }}>
                    {downloading ? <Loader2 size={15} className="animate-spin" /> : <Download size={15} />} Download
                  </button>
                  {kind === 'series' && (
                    <button onClick={() => setBulkOpen(true)} className="flex items-center gap-2 px-4 py-2.5 text-[12.5px] font-semibold"
                      style={{ border: `1.5px solid ${skin.border}`, borderRadius: pillR, background: 'rgba(255,255,255,.06)' }}>
                      <Layers size={15} /> Bulk download
                    </button>
                  )}
                  {activeProvider && (
                    <button onClick={() => onOfficial(activeProvider.id, d.title)} className="sm:ml-auto min-h-[40px] flex items-center gap-1.5 text-[11.5px] hover:underline" style={{ color: skin.muted }}>
                      <ExternalLink size={13} /> Open on {activeProvider.name}
                    </button>
                  )}
                </div>

                <div className="grid gap-6 md:grid-cols-[1fr_230px]">
                  <div>
                    <p className="text-[13px] leading-relaxed break-words" style={{ color: skin.text, opacity: 0.88 }}>
                      {d.description || 'No synopsis is available for this title yet.'}
                    </p>
                    {cast.length > 0 && (
                      <p className="text-[12px] mt-3"><span style={{ color: skin.muted }}>Cast: </span>{cast.slice(0, 8).join(', ')}</p>
                    )}
                  </div>
                  <div className="text-[12px] space-y-2 min-w-0 break-words">
                    {d.genres && d.genres.length > 0 && (
                      <div><span style={{ color: skin.muted }}>Genres: </span>{d.genres.join(', ')}</div>
                    )}
                    {d.status && <div><span style={{ color: skin.muted }}>Release: </span>{d.status}</div>}
                    {rows.length > 0 && (
                      <div>
                        <p className="mb-1.5" style={{ color: skin.muted }}>Available on ({region})</p>
                        <div className="flex flex-wrap gap-1.5">
                          {rows.map(({ a, p }, i) => (
                            <button key={`${a.providerId}-${a.type}-${i}`} onClick={() => onOfficial(p!.id, d.title)}
                              className="px-2 py-1 rounded-md text-[10.5px] font-semibold"
                              style={{ background: p!.theme.accent + '26', color: p!.theme.accent === '#ffffff' ? '#fff' : p!.theme.accent, border: `1px solid ${p!.theme.accent}55` }}>
                              {p!.name} · {AVAIL_LABEL[a.type]}
                            </button>
                          ))}
                        </div>
                      </div>
                    )}
                  </div>
                </div>

                {/* Episodes */}
                {kind === 'series' && episodes.length > 0 && (
                  <div className="mt-6">
                    <div className="flex items-center gap-3 mb-2">
                      <h3 className="text-[15px] font-bold">Episodes</h3>
                      {seasons.length > 1 && (
                        <select value={season} onChange={(e) => setSeason(e.target.value === 'all' ? 'all' : Number(e.target.value))}
                          className="text-[11.5px] px-2 py-1 min-h-[36px] sm:min-h-0 rounded-md outline-none" style={{ background: skin.surface, border: `1px solid ${skin.border}`, color: skin.text }}>
                          <option value="all">All seasons</option>
                          {seasons.map((s) => <option key={s} value={s}>{s === 0 ? 'Specials' : `Season ${s}`}</option>)}
                        </select>
                      )}
                    </div>
                    <div className="max-h-[220px] overflow-y-auto rounded-lg divide-y" style={{ background: 'rgba(255,255,255,.03)', borderColor: skin.border }}>
                      {shown.slice(0, 200).map((ep, i) => (
                        <div key={`${ep.id}-${i}`} className="flex items-center gap-3 px-3 py-2 text-[12px]" style={{ borderColor: skin.border }}>
                          <span className="w-8 shrink-0 text-right font-bold" style={{ color: skin.muted }}>{ep.number}</span>
                          <span className="flex-1 min-w-0 truncate">{ep.title || `Episode ${ep.number}`}</span>
                          {ep.airDate && <span className="text-[10px]" style={{ color: skin.muted }}>{ep.airDate.slice(0, 10)}</span>}
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
      {item && d && kind === 'series' && (
        <BulkDownloadModal
          open={bulkOpen}
          onClose={() => setBulkOpen(false)}
          kind="series"
          title={d.title}
          imdbId={d.imdbId}
          year={d.year ? String(d.year) : undefined}
          coverUrl={d.coverUrl}
          episodes={episodes.map((ep) => ({ number: ep.number, title: ep.title, season: seasonOf(ep) }))}
        />
      )}
    </>
  )
}
