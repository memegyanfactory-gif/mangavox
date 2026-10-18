import { useEffect, useState, useRef, useCallback } from 'react'
import { useUiStore } from '../stores/ui.store'
import { useLibraryStore } from '../stores/library.store'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { coverUrl } from '../utils/cover-url'
import {
  ArrowLeft, Play, Pause, SkipBack, SkipForward,
  Volume2, VolumeX, Loader2, Headphones, BookOpen,
  Download, FileText, Mic, Film, ChevronRight, AlertTriangle,
  RotateCcw,
} from 'lucide-react'

interface Segment {
  page: number
  text: string
  audio_file: string
  audio_url: string | null
  page_url: string | null
  start_ms: number
  end_ms: number
}

interface AudioData {
  syncMap: { segments: Segment[]; total_duration_ms: number } | null
  pageFiles: string[]
  audioFiles: string[]
  hasAudio: boolean
  hasVideo: boolean
  videoPath: string | null
}

interface ChapterWithAudio {
  id: string
  mangaId: string
  mangaTitle: string
  mangaCover: string | null
  number: number | null
  title: string | null
  tts_status: string
  ocr_status: string
  download_status: string
  video_status: string
}

// ── Custom Slider ──
function CustomSlider({ value, max, onChange, className = '', thin }: {
  value: number; max: number; onChange: (v: number) => void; className?: string; thin?: boolean
}) {
  const trackRef = useRef<HTMLDivElement>(null)
  const dragging = useRef(false)

  const percent = max > 0 ? Math.min(100, (value / max) * 100) : 0

  const applyFromEvent = (e: MouseEvent | React.MouseEvent) => {
    if (!trackRef.current) return
    const rect = trackRef.current.getBoundingClientRect()
    const ratio = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width))
    onChange(ratio * max)
  }

  const onMouseDown = (e: React.MouseEvent) => {
    dragging.current = true
    applyFromEvent(e)
    const onMove = (ev: MouseEvent) => { if (dragging.current) applyFromEvent(ev) }
    const onUp = () => { dragging.current = false; window.removeEventListener('mousemove', onMove); window.removeEventListener('mouseup', onUp) }
    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp)
  }

  const h = thin ? 'h-1 hover:h-1.5' : 'h-1.5 hover:h-2'

  return (
    <div
      ref={trackRef}
      className={`relative group cursor-pointer rounded-full transition-all duration-150 ${h} ${className}`}
      style={{ background: 'rgba(255,255,255,0.08)' }}
      onMouseDown={onMouseDown}
    >
      <div
        className="absolute inset-y-0 left-0 rounded-full transition-all duration-75"
        style={{ width: `${percent}%`, background: 'var(--accent-gradient)' }}
      />
      <div
        className="absolute top-1/2 w-3 h-3 rounded-full bg-white shadow-lg opacity-0 group-hover:opacity-100 transition-opacity duration-150 pointer-events-none"
        style={{ left: `${percent}%`, transform: 'translate(-50%, -50%)' }}
      />
    </div>
  )
}

export function Player() {
  const { pageParams, goBack, navigate } = useUiStore()
  const { chapters, fetchChapters } = useLibraryStore()
  const mangaId = pageParams.mangaId as string | undefined
  const chapterId = pageParams.chapterId as string | undefined
  const chapterTitle = (pageParams.chapterTitle as string) || 'Chapter'
  const mangaCover = pageParams.mangaCover as string | undefined

  const [data, setData] = useState<AudioData | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Playback state
  const [currentSegIdx, setCurrentSegIdx] = useState(0)
  const [isPlaying, setIsPlaying] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [volume, setVolume] = useState(1)
  const [muted, setMuted] = useState(false)
  const [speed, setSpeed] = useState(1)
  const [controlsVisible, setControlsVisible] = useState(true)

  // For page-only navigation when no audio segments
  const [pageIdx, setPageIdx] = useState(0)

  const audioRef = useRef<HTMLAudioElement | null>(null)
  const hideControlsTimer = useRef<number | null>(null)

  // ── No chapter selected: show chapter picker ──
  const noChapterSelected = !mangaId || !chapterId

  // Load available chapters with audio when showing picker
  const [audioChapters, setAudioChapters] = useState<ChapterWithAudio[]>([])
  useEffect(() => {
    if (!noChapterSelected) return
    invoke<any[]>(IPC.LIBRARY_LIST).then((mangas) => {
      const allChapters: ChapterWithAudio[] = []
      const promises = (mangas || []).map(async (m: any) => {
        const chs = await invoke<any[]>(IPC.CHAPTER_LIST, m.id)
        for (const ch of (chs || [])) {
          if (ch.tts_status === 'complete' || ch.ocr_status === 'complete') {
            allChapters.push({
              id: ch.id, mangaId: m.id, mangaTitle: m.title,
              mangaCover: m.cover_url || m.cover_path || null,
              number: ch.number, title: ch.title || null,
              tts_status: ch.tts_status, ocr_status: ch.ocr_status,
              download_status: ch.download_status, video_status: ch.video_status,
            })
          }
        }
      })
      Promise.all(promises).then(() => setAudioChapters(allChapters))
    }).catch(() => {})
  }, [noChapterSelected])

  // Load player data when chapter is selected
  const loadData = useCallback(() => {
    if (!mangaId || !chapterId) return
    setLoading(true)
    setError(null)
    setData(null)
    setCurrentSegIdx(0)
    setPageIdx(0)

    invoke<AudioData>(IPC.PLAYER_GET_AUDIO_DATA, mangaId, chapterId)
      .then((result) => {
        if (!result || (!result.hasAudio && !result.pageFiles?.length)) {
          setError('No audio or pages found. Download and process the chapter first.')
          return
        }
        setData(result)
      })
      .catch((err) => setError(err?.message || 'Failed to load player data'))
      .finally(() => setLoading(false))
  }, [mangaId, chapterId])

  useEffect(() => { loadData() }, [loadData])

  const segments = data?.syncMap?.segments || []
  const hasSegments = segments.length > 0
  const currentSeg = segments[currentSegIdx]
  const totalPages = data?.pageFiles?.length || 0

  const currentPageUrl = hasSegments
    ? (currentSeg?.page_url || data?.pageFiles[currentSeg?.page ?? currentSegIdx] || null)
    : (data?.pageFiles[pageIdx] || null)

  // Load audio for current segment
  useEffect(() => {
    if (!currentSeg?.audio_url) return
    if (!audioRef.current) audioRef.current = new Audio()
    const audio = audioRef.current
    audio.src = currentSeg.audio_url
    audio.volume = muted ? 0 : volume
    audio.playbackRate = speed

    const onLoaded = () => setDuration(audio.duration)
    const onTimeUpdate = () => setCurrentTime(audio.currentTime)
    const onEnded = () => {
      if (currentSegIdx < segments.length - 1) {
        setCurrentSegIdx((i) => i + 1)
        setIsPlaying(true)
      } else {
        setIsPlaying(false)
      }
    }

    audio.addEventListener('loadedmetadata', onLoaded)
    audio.addEventListener('timeupdate', onTimeUpdate)
    audio.addEventListener('ended', onEnded)
    if (isPlaying) audio.play().catch(() => {})

    return () => {
      audio.removeEventListener('loadedmetadata', onLoaded)
      audio.removeEventListener('timeupdate', onTimeUpdate)
      audio.removeEventListener('ended', onEnded)
    }
  }, [currentSeg?.audio_url, currentSegIdx])

  useEffect(() => {
    if (!audioRef.current) return
    if (isPlaying) audioRef.current.play().catch(() => {})
    else audioRef.current.pause()
  }, [isPlaying])

  useEffect(() => {
    if (!audioRef.current) return
    audioRef.current.volume = muted ? 0 : volume
    audioRef.current.playbackRate = speed
  }, [volume, muted, speed])

  useEffect(() => {
    return () => { if (audioRef.current) { audioRef.current.pause(); audioRef.current.src = '' } }
  }, [])

  const togglePlay = useCallback(() => {
    if (!hasSegments) return
    setIsPlaying((p) => !p)
  }, [hasSegments])

  const goToSegment = useCallback((idx: number) => {
    if (idx < 0 || idx >= segments.length) return
    setCurrentSegIdx(idx)
    setCurrentTime(0)
    if (audioRef.current) audioRef.current.currentTime = 0
  }, [segments.length])

  const prevItem = useCallback(() => {
    if (hasSegments) goToSegment(currentSegIdx - 1)
    else setPageIdx((i) => Math.max(0, i - 1))
  }, [hasSegments, currentSegIdx, goToSegment])

  const nextItem = useCallback(() => {
    if (hasSegments) goToSegment(currentSegIdx + 1)
    else setPageIdx((i) => Math.min(totalPages - 1, i + 1))
  }, [hasSegments, currentSegIdx, goToSegment, totalPages])

  const seekTo = useCallback((time: number) => {
    if (audioRef.current) { audioRef.current.currentTime = time; setCurrentTime(time) }
  }, [])

  const showControls = useCallback(() => {
    setControlsVisible(true)
    if (hideControlsTimer.current) clearTimeout(hideControlsTimer.current)
    if (isPlaying) {
      hideControlsTimer.current = window.setTimeout(() => setControlsVisible(false), 4000)
    }
  }, [isPlaying])

  // Keyboard
  useEffect(() => {
    if (noChapterSelected) return
    const handleKey = (e: KeyboardEvent) => {
      switch (e.key) {
        case ' ': case 'k': e.preventDefault(); togglePlay(); break
        case 'ArrowLeft': e.preventDefault(); prevItem(); break
        case 'ArrowRight': e.preventDefault(); nextItem(); break
        case 'ArrowUp': e.preventDefault(); setVolume((v) => Math.min(1, v + 0.1)); break
        case 'ArrowDown': e.preventDefault(); setVolume((v) => Math.max(0, v - 0.1)); break
        case 'm': setMuted((m) => !m); break
        case 'Escape': goBack(); break
      }
      showControls()
    }
    window.addEventListener('keydown', handleKey)
    return () => window.removeEventListener('keydown', handleKey)
  }, [noChapterSelected, togglePlay, prevItem, nextItem, goBack, showControls])

  const formatTime = (sec: number) => {
    const m = Math.floor(sec / 60)
    const s = Math.floor(sec % 60)
    return `${m}:${s.toString().padStart(2, '0')}`
  }

  // ══════════ Chapter picker (no chapter selected) ══════════
  if (noChapterSelected) {
    return (
      <div className="flex flex-col h-full" style={{ background: 'var(--base)' }}>
        <div className="flex items-center gap-3 px-4 py-2.5" style={{ borderBottom: '1px solid var(--panel-border)' }}>
          <button onClick={goBack} className="titlebar-no-drag text-[color:var(--text-2)] hover:text-white transition-colors">
            <ArrowLeft size={20} />
          </button>
          <div className="titlebar-no-drag flex items-center gap-2">
            <Headphones size={18} className="text-[var(--accent)]" />
            <h1 className="text-lg font-semibold text-white">Player</h1>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto px-6 py-6">
          {audioChapters.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full gap-4 text-center fade-up">
              <div className="w-16 h-16 rounded-2xl flex items-center justify-center" style={{ background: 'var(--accent-glow)' }}>
                <Headphones size={28} className="text-[var(--accent)]" />
              </div>
              <p className="text-[color:var(--text-2)] text-sm max-w-sm">
                No chapters with audio yet. Download a manga, extract text, then generate audio to listen here.
              </p>
            </div>
          ) : (
            <div className="page-enter">
              <p className="text-xs text-[color:var(--text-3)] mb-4 uppercase tracking-wider font-semibold">{audioChapters.length} chapter{audioChapters.length !== 1 ? 's' : ''} ready</p>
              <div className="space-y-2">
                {audioChapters.map((ch) => {
                  const chLabel = ch.number != null ? `Chapter ${ch.number}` : 'Chapter'
                  const subtitle = ch.title ? `${chLabel} — ${ch.title}` : chLabel
                  return (
                    <button
                      key={ch.id}
                      onClick={() => navigate('player', { mangaId: ch.mangaId, chapterId: ch.id, chapterTitle: subtitle, mangaCover: ch.mangaCover })}
                      className="w-full flex items-center gap-3 px-4 py-3 rounded-xl text-left transition-all hover:scale-[1.005] group"
                      style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                    >
                      {ch.mangaCover ? (
                        <img
                          src={coverUrl(ch.mangaCover) || ''}
                          className="w-12 h-16 rounded-lg object-cover shrink-0 ring-1 ring-white/5"
                          alt=""
                        />
                      ) : (
                        <div className="w-12 h-16 rounded-lg shrink-0 flex items-center justify-center" style={{ background: 'var(--surface-3)' }}>
                          <BookOpen size={14} className="text-[color:var(--text-3)]" />
                        </div>
                      )}

                      <div className="flex-1 min-w-0">
                        <div className="text-[11px] text-[color:var(--text-3)] truncate mb-0.5">{ch.mangaTitle}</div>
                        <div className="text-sm font-medium text-[color:var(--text-1)] truncate">{subtitle}</div>
                        <div className="flex items-center gap-1.5 mt-1.5">
                          <StageBadge icon={<Download size={9} />} done={ch.download_status === 'complete'} label="DL" />
                          <StageBadge icon={<FileText size={9} />} done={ch.ocr_status === 'complete'} label="Text" />
                          <StageBadge icon={<Mic size={9} />} done={ch.tts_status === 'complete'} label="Audio" highlight />
                          <StageBadge icon={<Film size={9} />} done={ch.video_status === 'complete'} label="Video" />
                        </div>
                      </div>

                      <ChevronRight size={14} className="text-[color:var(--text-3)] group-hover:text-[color:var(--text-2)] transition-colors shrink-0" />
                    </button>
                  )
                })}
              </div>
            </div>
          )}
        </div>
      </div>
    )
  }

  // ══════════ Loading state ══════════
  if (loading) {
    return (
      <div className="flex flex-col h-full" style={{ background: 'var(--base)' }}>
        <div className="flex items-center gap-3 px-4 py-2.5" style={{ borderBottom: '1px solid var(--panel-border)' }}>
          <button onClick={goBack} className="text-[color:var(--text-2)] hover:text-white transition-colors">
            <ArrowLeft size={20} />
          </button>
          <p className="text-sm text-[color:var(--text-2)] truncate">{chapterTitle}</p>
        </div>
        <div className="flex-1 flex flex-col items-center justify-center gap-5 fade-up">
          <div className="w-48 aspect-[2/3] skeleton rounded-xl" />
          <div className="spinner-ring" />
          <p className="text-xs text-[color:var(--text-3)]">Loading chapter audio...</p>
        </div>
        <div className="h-24 mx-6 mb-6 skeleton rounded-xl" />
      </div>
    )
  }

  // ══════════ Error state ══════════
  if (error || !data) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-5 fade-up" style={{ background: 'var(--base)' }}>
        <div className="rounded-2xl px-8 py-6 flex flex-col items-center gap-4 max-w-sm text-center"
          style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
          <div className="w-12 h-12 rounded-xl flex items-center justify-center" style={{ background: 'rgba(var(--accent-rgb),0.12)' }}>
            <AlertTriangle size={22} className="text-[color:var(--accent-hover)]" />
          </div>
          <div>
            <p className="text-sm font-medium text-[color:var(--text-1)]">Failed to load</p>
            <p className="text-xs text-[color:var(--text-3)] mt-1">{error || 'No data available'}</p>
          </div>
          <div className="flex items-center gap-3">
            <button onClick={loadData} className="flex items-center gap-1.5 text-xs font-medium px-4 py-2 rounded-lg text-white transition-all hover:brightness-110"
              style={{ background: 'var(--accent-gradient)' }}>
              <RotateCcw size={12} /> Try again
            </button>
            <button onClick={goBack} className="text-xs text-[color:var(--text-3)] hover:text-[color:var(--text-1)] px-3 py-2 rounded-lg transition-colors"
              style={{ background: 'var(--surface-3)' }}>
              Go back
            </button>
          </div>
        </div>
      </div>
    )
  }

  const displayPage = hasSegments ? (currentSeg?.page ?? 0) + 1 : pageIdx + 1

  // ══════════ Main player UI ══════════
  return (
    <div
      className="relative flex flex-col h-full select-none"
      style={{ background: 'var(--base)' }}
      onMouseMove={showControls}
      onClick={showControls}
    >
      {/* Page display */}
      <div className="flex-1 relative overflow-hidden flex items-center justify-center">
        {currentPageUrl ? (
          <img src={currentPageUrl} className="max-h-full max-w-full object-contain transition-opacity duration-300" draggable={false} />
        ) : (
          <div className="text-[color:var(--text-3)] text-lg">No page image</div>
        )}

        {/* Click zones for navigation */}
        <div className="absolute inset-0 flex">
          <div className="w-1/3 cursor-pointer" onClick={(e) => { e.stopPropagation(); prevItem() }} />
          <div className="w-1/3" />
          <div className="w-1/3 cursor-pointer" onClick={(e) => { e.stopPropagation(); nextItem() }} />
        </div>
      </div>

      {/* Text overlay */}
      {hasSegments && currentSeg?.text && (
        <div
          className={`absolute bottom-36 left-1/2 -translate-x-1/2 max-w-2xl px-6 py-3 rounded-2xl transition-opacity duration-300 ${controlsVisible ? 'opacity-100' : 'opacity-0'}`}
          style={{ background: 'var(--surface-1)', backdropFilter: 'blur(20px)', border: '1px solid var(--panel-border)' }}
        >
          <p className="text-sm text-[color:var(--text-1)] text-center leading-relaxed">{currentSeg.text}</p>
        </div>
      )}

      {/* Top bar */}
      <div
        className={`absolute top-0 left-0 right-0 z-20 transition-opacity duration-300 ${controlsVisible ? 'opacity-100' : 'opacity-0 pointer-events-none'}`}
        style={{ background: 'linear-gradient(to bottom, rgba(0,0,0,0.55) 0%, transparent 100%)' }}
      >
        <div className="flex items-center gap-3 px-4 py-2.5">
          <button onClick={goBack} className="titlebar-no-drag text-[color:var(--text-2)] hover:text-white transition-colors">
            <ArrowLeft size={20} />
          </button>
          <div className="titlebar-no-drag flex-1 min-w-0">
            <p className="text-sm font-medium text-white truncate">{chapterTitle}</p>
            <p className="text-xs text-[color:var(--text-3)]">
              Page {displayPage} of {totalPages}
              {hasSegments && ` · Segment ${currentSegIdx + 1}/${segments.length}`}
            </p>
          </div>
        </div>
      </div>

      {/* Bottom controls */}
      <div
        className={`absolute bottom-0 left-0 right-0 z-20 transition-opacity duration-300 ${controlsVisible ? 'opacity-100' : 'opacity-0 pointer-events-none'}`}
        style={{ background: 'linear-gradient(to top, rgba(0,0,0,0.85) 0%, rgba(0,0,0,0.5) 60%, transparent 100%)', backdropFilter: 'blur(12px)' }}
      >
        <div className="px-6 pt-8 pb-5 space-y-3">

          {/* Audio progress bar */}
          {hasSegments && (
            <div className="space-y-1">
              <CustomSlider value={currentTime} max={duration || 1} onChange={seekTo} />
              <div className="flex justify-between">
                <span className="text-[10px] text-[color:var(--text-3)] font-mono">{formatTime(currentTime)}</span>
                <span className="text-[10px] text-[color:var(--text-3)] font-mono">{formatTime(duration)}</span>
              </div>
            </div>
          )}

          {/* Segment strip */}
          {hasSegments && segments.length > 1 && (
            <div className="flex gap-0.5">
              {segments.map((_, i) => (
                <button
                  key={i}
                  onClick={() => goToSegment(i)}
                  className="flex-1 rounded-full transition-all"
                  style={{
                    height: '3px',
                    minWidth: '4px',
                    background: i === currentSegIdx ? 'var(--accent)' : i < currentSegIdx ? 'rgba(200,80,26,0.35)' : 'rgba(255,255,255,0.06)',
                    boxShadow: i === currentSegIdx ? '0 0 6px rgba(200,80,26,0.4)' : 'none',
                  }}
                />
              ))}
            </div>
          )}

          {/* Page progress when no audio */}
          {!hasSegments && totalPages > 1 && (
            <div className="space-y-1">
              <CustomSlider value={pageIdx} max={totalPages - 1} onChange={(v) => setPageIdx(Math.round(v))} />
              <p className="text-[10px] text-[color:var(--text-3)] text-center">Page {pageIdx + 1} of {totalPages}</p>
            </div>
          )}

          {/* Controls row: 3-column layout */}
          <div className="flex items-center justify-between gap-4">

            {/* LEFT: cover + track info */}
            <div className="flex items-center gap-3 w-48 min-w-0">
              {mangaCover ? (
                <img src={coverUrl(mangaCover) || ''} className="w-10 h-10 rounded-lg object-cover shrink-0 ring-1 ring-white/5" alt="" />
              ) : (
                <div className="w-10 h-10 rounded-lg shrink-0 flex items-center justify-center" style={{ background: 'var(--surface-3)' }}>
                  <Headphones size={14} className="text-[color:var(--text-3)]" />
                </div>
              )}
              <div className="min-w-0">
                <p className="text-xs text-[color:var(--text-1)] truncate font-medium">{chapterTitle}</p>
                <p className="text-[10px] text-[color:var(--text-3)]">
                  {hasSegments ? `Segment ${currentSegIdx + 1} of ${segments.length}` : `Page ${pageIdx + 1} of ${totalPages}`}
                </p>
              </div>
            </div>

            {/* CENTER: playback controls */}
            <div className="flex items-center gap-5">
              <button
                onClick={prevItem}
                disabled={hasSegments ? currentSegIdx <= 0 : pageIdx <= 0}
                className="text-[color:var(--text-2)] hover:text-white disabled:opacity-20 transition-colors"
              >
                <SkipBack size={22} />
              </button>

              {hasSegments ? (
                <button
                  onClick={togglePlay}
                  className="w-[52px] h-[52px] rounded-full flex items-center justify-center text-white transition-all hover:scale-105 active:scale-95"
                  style={{
                    background: 'var(--accent-gradient)',
                    boxShadow: isPlaying ? '0 0 24px rgba(200,80,26,0.35)' : '0 0 12px rgba(200,80,26,0.15)',
                  }}
                >
                  {isPlaying ? <Pause size={24} /> : <Play size={24} className="ml-0.5" />}
                </button>
              ) : (
                <div
                  className="w-[52px] h-[52px] rounded-full flex items-center justify-center text-[color:var(--text-3)]"
                  style={{ background: 'var(--surface-3)' }}
                >
                  <Play size={24} className="ml-0.5" />
                </div>
              )}

              <button
                onClick={nextItem}
                disabled={hasSegments ? currentSegIdx >= segments.length - 1 : pageIdx >= totalPages - 1}
                className="text-[color:var(--text-2)] hover:text-white disabled:opacity-20 transition-colors"
              >
                <SkipForward size={22} />
              </button>
            </div>

            {/* RIGHT: volume + speed */}
            <div className="flex items-center gap-3 w-48 justify-end">
              {hasSegments ? (
                <>
                  <button onClick={() => setMuted(!muted)} className="text-[color:var(--text-2)] hover:text-white transition-colors shrink-0">
                    {muted || volume === 0 ? <VolumeX size={16} /> : <Volume2 size={16} />}
                  </button>
                  <CustomSlider
                    value={muted ? 0 : volume}
                    max={1}
                    onChange={(v) => { setVolume(v); setMuted(false) }}
                    className="w-20"
                    thin
                  />
                  <button
                    onClick={() => setSpeed((s) => {
                      const speeds = [0.5, 0.75, 1, 1.25, 1.5, 2]
                      return speeds[(speeds.indexOf(s) + 1) % speeds.length]
                    })}
                    className="text-[11px] font-semibold text-[color:var(--text-1)] hover:text-white px-2.5 py-1.5 rounded-lg transition-colors shrink-0"
                    style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                  >
                    {speed}x
                  </button>
                </>
              ) : (
                <span className="text-[10px] text-[color:var(--text-3)] px-3 py-1.5 rounded-lg" style={{ background: 'var(--surface-3)' }}>
                  Pages only
                </span>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

function StageBadge({ icon, done, label, highlight }: { icon: React.ReactNode; done: boolean; label: string; highlight?: boolean }) {
  return (
    <span
      className={`inline-flex items-center gap-0.5 text-[9px] font-semibold px-1.5 py-0.5 rounded-full ${
        done
          ? highlight
            ? 'text-[color:var(--accent-hover)] bg-[rgba(var(--accent-rgb),0.16)]'
            : 'text-[color:var(--accent-hover)] bg-[rgba(var(--accent-rgb),0.12)]'
          : 'text-[color:var(--text-3)] bg-white/[0.03]'
      }`}
    >
      {icon}{label}
    </span>
  )
}
