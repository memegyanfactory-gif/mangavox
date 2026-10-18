import { useEffect, useCallback, useRef, useState, useMemo } from 'react'
import { useReaderStore } from '../stores/reader.store'
import { toBcp47, pickVoice, whenVoicesReady } from '../services/tts'
import { useUiStore } from '../stores/ui.store'
import { useLibraryStore } from '../stores/library.store'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { ArrowLeft, SkipBack, SkipForward, BookOpen, Monitor, Smartphone, Loader2, RotateCcw, Timer, X, Languages, Volume2, Pause, ChevronDown } from 'lucide-react'
import type { ReadingMode, ImageFit } from '../stores/reader.store'
import type { Chapter } from '../../shared/types'
import { useAppModeStore } from '../stores/app-mode.store'

interface OverlayBox {
  points: number[][]
  text: string
  score?: number
}

interface OverlayData {
  available: boolean
  pages: { page: number; file: string; boxes: OverlayBox[] }[]
}

// Languages we offer in the overlay translate menu — same shortlist as TextPage.
const OVERLAY_LANGS: { code: string; label: string }[] = [
  { code: 'en', label: 'English' },
  { code: 'hi', label: 'Hindi' },
  { code: 'es', label: 'Spanish' },
  { code: 'fr', label: 'French' },
  { code: 'de', label: 'German' },
  { code: 'ja', label: 'Japanese' },
  { code: 'ko', label: 'Korean' },
  { code: 'zh-CN', label: 'Chinese' },
  { code: 'pt', label: 'Portuguese' },
  { code: 'ru', label: 'Russian' },
  { code: 'id', label: 'Indonesian' },
  { code: 'ar', label: 'Arabic' },
]

/**
 * Renders an image with optional OCR-bbox overlays. Overlays auto-position
 * themselves over the image using its naturalWidth/naturalHeight to scale
 * coordinates from the original OCR pass to the rendered display rect.
 *
 * Supplying an empty `boxes` array (or `enabled=false`) means the bare image
 * is rendered with no extra DOM.
 */
function PageWithOverlay({
  src, boxes, enabled, translations, alt, className, draggable,
}: {
  src: string
  boxes: OverlayBox[]
  enabled: boolean
  translations: Record<string, string>
  alt: string
  className?: string
  draggable?: boolean
}) {
  const imgRef = useRef<HTMLImageElement | null>(null)
  const [, forceUpdate] = useState(0)

  // Recompute overlay positions on image load + window resize
  useEffect(() => {
    if (!enabled) return
    const handler = () => forceUpdate((n) => n + 1)
    window.addEventListener('resize', handler)
    return () => window.removeEventListener('resize', handler)
  }, [enabled])

  const transform = useMemo(() => {
    const img = imgRef.current
    if (!img || !enabled) return null
    const nW = img.naturalWidth
    const nH = img.naturalHeight
    const rect = img.getBoundingClientRect()
    if (!nW || !nH || !rect.width || !rect.height) return null

    // object-contain math: figure out where the actual pixels land
    const naturalAR = nW / nH
    const displayAR = rect.width / rect.height
    let renderedW: number, renderedH: number, offsetX = 0, offsetY = 0
    if (naturalAR > displayAR) {
      renderedW = rect.width
      renderedH = rect.width / naturalAR
      offsetY = (rect.height - renderedH) / 2
    } else {
      renderedH = rect.height
      renderedW = rect.height * naturalAR
      offsetX = (rect.width - renderedW) / 2
    }
    return { scaleX: renderedW / nW, scaleY: renderedH / nH, offsetX, offsetY }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, imgRef.current?.naturalWidth, imgRef.current?.naturalHeight, imgRef.current?.clientWidth, imgRef.current?.clientHeight])

  return (
    <div className="relative inline-block max-w-full max-h-full">
      <img
        ref={imgRef}
        src={src}
        alt={alt}
        className={className}
        draggable={draggable}
        referrerPolicy="no-referrer"
        onLoad={() => forceUpdate((n) => n + 1)}
      />
      {enabled && transform && boxes.length > 0 && (
        <div className="absolute inset-0 pointer-events-none">
          {boxes.map((b, i) => {
            const xs = b.points.map((p) => p[0])
            const ys = b.points.map((p) => p[1])
            const x = Math.min(...xs)
            const y = Math.min(...ys)
            const w = Math.max(...xs) - x
            const h = Math.max(...ys) - y
            const dx = x * transform.scaleX + transform.offsetX
            const dy = y * transform.scaleY + transform.offsetY
            const dw = w * transform.scaleX
            const dh = h * transform.scaleY
            const fontSize = Math.max(9, Math.min(14, dh * 0.55))
            const display = translations[b.text] ?? b.text
            return (
              <div
                key={i}
                className="absolute font-semibold leading-tight rounded shadow-md flex items-center justify-center text-center bg-white/95 text-black pointer-events-auto"
                style={{
                  left: `${dx}px`,
                  top: `${dy}px`,
                  width: `${dw}px`,
                  minHeight: `${dh}px`,
                  fontSize: `${fontSize}px`,
                  padding: '2px 4px',
                  lineHeight: 1.15,
                }}
                title={b.text}
              >
                {display}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}

const MODE_PRESETS: { mode: ReadingMode; label: string; desc: string; icon: React.ReactNode }[] = [
  { mode: 'rtl', label: 'Manga', desc: 'RTL', icon: <BookOpen size={14} /> },
  { mode: 'ltr', label: 'Comic', desc: 'LTR', icon: <Monitor size={14} /> },
  { mode: 'vertical', label: 'Webtoon', desc: 'Scroll', icon: <Smartphone size={14} /> },
]

export function Reader() {
  const isStudio = useAppModeStore((state) => state.mode === 'studio')
  const {
    pages, currentPage, loading, error, readingMode, imageFit, showControls,
    loadChapter, setCurrentPage, nextPage, prevPage,
    setReadingMode, setImageFit, toggleControls, mangaId, chapterId,
  } = useReaderStore()
  const { pageParams, goBack } = useUiStore()
  const { chapters, fetchChapters } = useLibraryStore()
  const containerRef = useRef<HTMLDivElement>(null)
  const verticalRef = useRef<HTMLDivElement>(null)

  const pMangaId = pageParams.mangaId as string
  const pChapterId = pageParams.chapterId as string
  const pSourceId = pageParams.sourceId as string
  const pSourceChapterId = pageParams.sourceChapterId as string

  const [dlProgress, setDlProgress] = useState<{ current: number; total: number; status: string } | null>(null)

  // Auto-scroll
  const [autoScroll, setAutoScroll] = useState(false)
  const [autoScrollSpeed, setAutoScrollSpeed] = useState(5) // seconds per page
  const [showSpeedPicker, setShowSpeedPicker] = useState(false)
  const [customSpeed, setCustomSpeed] = useState('')
  const autoScrollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  // Translate overlay (AI Manga Reader-style on-image translations)
  const [overlayData, setOverlayData] = useState<OverlayData | null>(null)
  const [overlayEnabled, setOverlayEnabled] = useState(false)
  const [overlayLang, setOverlayLang] = useState('en')
  const [overlayLangMenuOpen, setOverlayLangMenuOpen] = useState(false)
  const [overlayTranslations, setOverlayTranslations] = useState<Record<string, string>>({})
  const [translatingOverlay, setTranslatingOverlay] = useState(false)

  // Read Aloud (Web Speech API — current page text)
  const [reading, setReading] = useState(false)

  useEffect(() => {
    if (!pMangaId || !pChapterId || !pSourceId || !pSourceChapterId) return
    const ch = chapters.find((c) => c.id === pChapterId)
    if (ch?.download_status === 'downloading' || ch?.download_status === 'queued') {
      setDlProgress({ current: 0, total: 0, status: ch.download_status })
    } else {
      setDlProgress(null)
    }
    loadChapter(pMangaId, pChapterId, pSourceId, pSourceChapterId)
    fetchChapters(pMangaId)
  }, [pChapterId])

  // Subscribe to download progress — refresh pages as each one completes
  useEffect(() => {
    let lastPage = 0
    const unsub = onEvent(IPC.DOWNLOAD_PROGRESS, (data: any) => {
      if (data.chapterId !== pChapterId) return
      if (data.status === 'complete') {
        setDlProgress(null)
        if (pMangaId && pChapterId && pSourceId && pSourceChapterId) {
          loadChapter(pMangaId, pChapterId, pSourceId, pSourceChapterId)
        }
      } else if (data.status === 'failed' || data.status === 'none') {
        setDlProgress(null)
        if (pMangaId) fetchChapters(pMangaId)
      } else {
        setDlProgress({ current: data.current, total: data.total, status: data.status })
        // Reload pages every 5 new pages to show progressive content without thrashing
        if (data.current > 0 && data.current - lastPage >= 5 && pMangaId && pChapterId && pSourceId && pSourceChapterId) {
          lastPage = data.current
          loadChapter(pMangaId, pChapterId, pSourceId, pSourceChapterId)
        }
      }
    })
    return unsub
  }, [pChapterId])

  // Chapter-end auto-advance: when already on the last page/spread, "next" opens the next chapter.
  const nextChapterRef = useRef<Chapter | null>(null)
  const goToChapterRef = useRef<((ch: Chapter) => void) | null>(null)
  const smartNext = useCallback(() => {
    const s = useReaderStore.getState()
    const atEnd = s.readingMode === 'rtl'
      ? s.currentPage >= s.pages.length - 2
      : s.currentPage >= s.pages.length - 1
    const nc = nextChapterRef.current
    if (atEnd && s.pages.length > 0 && nc && s.readingMode !== 'vertical') {
      if (s.chapterId) invoke(IPC.CHAPTER_MARK_READ, s.chapterId).catch(() => {})
      goToChapterRef.current?.(nc)
      return
    }
    s.nextPage()
  }, [])

  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    const t = e.target as HTMLElement | null
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
    switch (e.key) {
      case 'f':
      case 'F':
        if (e.ctrlKey || e.metaKey || e.altKey) return
        e.preventDefault()
        if (document.fullscreenElement) document.exitFullscreen().catch(() => {})
        else document.documentElement.requestFullscreen().catch(() => {})
        break
      case 'ArrowRight':
      case ' ':
        e.preventDefault()
        smartNext()
        break
      case 'ArrowLeft':
        e.preventDefault()
        prevPage()
        break
      case 'Escape':
        if (document.fullscreenElement) return
        goBack()
        break
    }
  }, [smartNext, prevPage, goBack])

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [handleKeyDown])

  // Fetch OCR overlay data once per chapter — cheap (just reads JSON from disk)
  useEffect(() => {
    if (!isStudio) {
      setOverlayData(null)
      setOverlayEnabled(false)
      return
    }
    if (!pMangaId || !pChapterId) { setOverlayData(null); return }
    invoke<OverlayData>(IPC.READER_GET_OCR_OVERLAY, pMangaId, pChapterId)
      .then(setOverlayData)
      .catch(() => setOverlayData(null))
    setOverlayEnabled(false)
    setOverlayTranslations({})
  }, [isStudio, pMangaId, pChapterId])

  // Stop Web Speech if user navigates pages or toggles overlay
  useEffect(() => {
    return () => { try { window.speechSynthesis.cancel() } catch { /* ok */ } }
  }, [])
  useEffect(() => {
    if (reading) {
      try { window.speechSynthesis.cancel() } catch { /* ok */ }
      setReading(false)
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentPage, pChapterId])

  // Translate all unique strings on the current overlay page (cached per string).
  const translateOverlay = useCallback(async () => {
    if (!overlayData?.available) return
    setTranslatingOverlay(true)
    try {
      // Collect every unique source string across all pages so flipping pages
      // is instant after the initial pass. Skip strings already translated.
      const allTexts = new Set<string>()
      for (const p of overlayData.pages) for (const b of p.boxes) {
        if (b.text.trim() && !overlayTranslations[b.text]) allTexts.add(b.text)
      }
      const list = Array.from(allTexts)
      if (list.length === 0) { setTranslatingOverlay(false); return }
      // Batch into a joined blob — one network call instead of N. Pipe-delimit,
      // then split. Simple and works for short OCR strings.
      const SEP = '\n«»\n'
      const joined = list.join(SEP)
      const result = await invoke<string>(IPC.TEXT_TRANSLATE, joined, overlayLang)
      const parts = result.split(SEP)
      const next: Record<string, string> = { ...overlayTranslations }
      list.forEach((src, i) => {
        next[src] = parts[i] ?? src
      })
      setOverlayTranslations(next)
    } catch (err) {
      console.error('[reader] overlay translate failed:', err)
    }
    setTranslatingOverlay(false)
  }, [overlayData, overlayLang, overlayTranslations])

  const handleToggleOverlay = async () => {
    if (!overlayData?.available) return
    const next = !overlayEnabled
    setOverlayEnabled(next)
    if (next && Object.keys(overlayTranslations).length === 0) {
      await translateOverlay()
    }
  }

  const currentPageBoxes = useMemo<OverlayBox[]>(() => {
    if (!isStudio) return []
    if (!overlayData?.available) return []
    return overlayData.pages.find((p) => p.page === currentPage)?.boxes ?? []
  }, [isStudio, overlayData, currentPage])

  const handleReadAloud = async () => {
    const boxes = currentPageBoxes
    const text = boxes.length > 0
      ? boxes.map((b) => overlayTranslations[b.text] ?? b.text).join('. ')
      : ''
    if (!text.trim()) return
    try { window.speechSynthesis.cancel() } catch { /* ok */ }
    await whenVoicesReady()
    const langCode = overlayEnabled ? overlayLang : 'en'
    const u = new SpeechSynthesisUtterance(text)
    u.rate = 1
    u.pitch = 1
    u.lang = toBcp47(langCode)
    const voice = pickVoice(langCode)
    if (voice) u.voice = voice
    u.onend = () => setReading(false)
    u.onerror = () => setReading(false)
    window.speechSynthesis.speak(u)
    setReading(true)
  }
  const handleStopReading = () => {
    try { window.speechSynthesis.cancel() } catch { /* ok */ }
    setReading(false)
  }

  useEffect(() => {
    if (pages.length > 0 && currentPage >= pages.length - 1 && chapterId) {
      invoke(IPC.CHAPTER_MARK_READ, chapterId)
    }
  }, [currentPage, pages.length, chapterId])

  // Auto-scroll effect
  useEffect(() => {
    if (autoScrollRef.current) clearInterval(autoScrollRef.current)
    if (!autoScroll || pages.length === 0) return

    if (readingMode === 'vertical') {
      // Smooth scroll for vertical/webtoon mode
      const pixelsPerTick = 2
      const tickMs = 50
      autoScrollRef.current = setInterval(() => {
        verticalRef.current?.scrollBy({ top: pixelsPerTick, behavior: 'auto' })
      }, tickMs * (autoScrollSpeed / 5))
    } else {
      // Page mode: advance page every N seconds
      autoScrollRef.current = setInterval(() => {
        const store = useReaderStore.getState()
        if (store.currentPage < store.pages.length - 1) {
          store.nextPage()
        } else {
          setAutoScroll(false) // stop at last page
        }
      }, autoScrollSpeed * 1000)
    }

    return () => { if (autoScrollRef.current) clearInterval(autoScrollRef.current) }
  }, [autoScroll, autoScrollSpeed, readingMode, pages.length])

  const handlePageClick = (e: React.MouseEvent) => {
    if (readingMode === 'vertical') {
      toggleControls()
      return
    }
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
    const x = e.clientX - rect.left
    const zone = x / rect.width
    if (zone < 0.33) prevPage()
    else if (zone > 0.66) smartNext()
    else toggleControls()
  }

  const sortedChapters = [...chapters].sort((a, b) => (a.number ?? 0) - (b.number ?? 0))
  const currentChapterIdx = sortedChapters.findIndex((c) => c.id === pChapterId)
  const prevChapter = currentChapterIdx > 0 ? sortedChapters[currentChapterIdx - 1] : null
  const nextChapter = currentChapterIdx < sortedChapters.length - 1 ? sortedChapters[currentChapterIdx + 1] : null

  const goToChapter = (ch: typeof sortedChapters[0]) => {
    const { navigate } = useUiStore.getState()
    navigate('reader', {
      mangaId: pMangaId,
      chapterId: ch.id,
      sourceId: ch.source_plugin || pSourceId,
      sourceChapterId: ch.source_id,
    })
  }

  nextChapterRef.current = nextChapter
  goToChapterRef.current = goToChapter

  // Preload upcoming pages (decode into the HTTP/image cache) — <link rel=prefetch> is unreliable in Electron
  const preloadRef = useRef<HTMLImageElement[]>([])
  const preloadedNextRef = useRef<string>('')
  const preloadUrls = (urls: string[]) => {
    const imgs = urls.filter(Boolean).map((u) => {
      const img = new Image()
      img.referrerPolicy = 'no-referrer'
      img.src = u
      return img
    })
    preloadRef.current = [...preloadRef.current.slice(-16), ...imgs]
  }
  useEffect(() => {
    if (pages.length === 0) return
    const ahead = readingMode === 'rtl' ? [2, 3, 4] : readingMode === 'vertical' ? [] : [1, 2, 3]
    preloadUrls(ahead.map((o) => pages[currentPage + o]))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentPage, pages, readingMode])

  // Near the end of the chapter (or in webtoon mode): warm the first pages of the next chapter
  useEffect(() => {
    if (pages.length === 0 || !nextChapter || !pMangaId) return
    const nearEnd = readingMode === 'vertical' || currentPage >= pages.length - 4
    if (!nearEnd || preloadedNextRef.current === nextChapter.id) return
    preloadedNextRef.current = nextChapter.id
    invoke<string[]>(IPC.READER_GET_PAGES, pMangaId, nextChapter.id, nextChapter.source_plugin || pSourceId, nextChapter.source_id)
      .then((urls) => { if (Array.isArray(urls)) preloadUrls(urls.slice(0, 3)) })
      .catch(() => { preloadedNextRef.current = '' })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentPage, pages.length, nextChapter?.id, readingMode, pMangaId])

  // Loading state
  if (loading) {
    return (
      <div className="flex items-center justify-center h-full bg-[var(--base)]">
        <div className="flex flex-col items-center gap-3 fade-up">
          <Loader2 size={28} className="text-[var(--accent)] animate-spin" />
          <span className="text-[var(--text-3)] text-sm">Loading chapter...</span>
        </div>
      </div>
    )
  }

  // Error state
  if (error || pages.length === 0) {
    return (
      <div className="flex items-center justify-center h-full bg-[var(--base)]">
        <div className="text-center max-w-md px-6 fade-up">
          <div className="w-16 h-16 rounded-2xl mx-auto mb-4 flex items-center justify-center" style={{ background: 'var(--surface-2)' }}>
            <BookOpen size={24} className="text-[var(--text-3)]" />
          </div>
          <p className="text-[var(--text-2)] font-medium mb-1">No pages available</p>
          {error && <p className="text-[var(--accent)] text-sm mb-5">{error}</p>}
          <div className="flex items-center justify-center gap-3">
            <button
              onClick={() => {
                if (pMangaId && pChapterId && pSourceId && pSourceChapterId) {
                  loadChapter(pMangaId, pChapterId, pSourceId, pSourceChapterId)
                }
              }}
              className="flex items-center gap-2 px-4 py-2 rounded-xl text-sm font-medium text-white transition-all hover:opacity-90"
              style={{ background: 'var(--accent-gradient)' }}
            >
              <RotateCcw size={14} /> Retry
            </button>
            <button
              onClick={goBack}
              className="px-4 py-2 rounded-xl text-sm text-[var(--text-3)] hover:text-[var(--text-1)] transition-colors"
              style={{ background: 'var(--surface-3)' }}
            >
              Go Back
            </button>
          </div>
        </div>
      </div>
    )
  }

  const currentChapter = sortedChapters[currentChapterIdx]
  const progress = pages.length > 0 ? ((currentPage + 1) / pages.length) * 100 : 0

  return (
    <div className="flex flex-col h-full bg-[var(--base)] relative" ref={containerRef}>
      {/* Vignette */}
      <div className="absolute inset-0 pointer-events-none z-[1]" style={{
        background: 'radial-gradient(ellipse at center, transparent 60%, rgba(0,0,0,0.25) 100%)',
      }} />

      {/* Download progress banner */}
      {dlProgress && (
        <div className="absolute top-9 left-0 right-0 z-30 flex items-center gap-3 px-4 py-1.5 text-xs"
          style={{ background: 'rgba(var(--accent-rgb),0.9)', backdropFilter: 'blur(8px)' }}>
          <Loader2 size={12} className="animate-spin text-white shrink-0" />
          <span className="text-white font-medium">
            {dlProgress.status === 'queued' ? 'Queued…' : `Downloading ${dlProgress.current > 0 ? `${dlProgress.current}/${dlProgress.total} pages` : 'chapter…'}`}
          </span>
          {dlProgress.total > 0 && (
            <div className="flex-1 h-1 rounded-full bg-white/20">
              <div className="h-full rounded-full bg-white transition-all duration-300"
                style={{ width: `${Math.round((dlProgress.current / dlProgress.total) * 100)}%` }} />
            </div>
          )}
        </div>
      )}

      {/* Top bar */}
      {showControls && (
        <div
          className="absolute top-0 left-0 right-0 z-20 px-2 sm:px-4 py-1 sm:py-1.5 flex items-center gap-2 sm:gap-3 border-b"
          style={{ animation: 'page-enter 0.2s var(--ease-snap) both', background: 'var(--surface-1)', borderColor: 'var(--panel-border)' }}
        >
          <button onClick={goBack} aria-label="Back" className="w-10 h-10 sm:w-auto sm:h-auto shrink-0 flex items-center justify-center text-[var(--text-2)] hover:text-[var(--text-1)] transition-colors">
            <ArrowLeft size={18} />
          </button>
          <span className="text-[var(--text-2)] text-sm font-medium flex-1 truncate">
            {currentChapter ? `Ch. ${currentChapter.number ?? '?'}` : 'Reader'}
            {currentChapter?.title ? ` — ${currentChapter.title}` : ''}
          </span>
          <span
            className="text-xs font-medium px-2.5 py-1 rounded-full shrink-0 whitespace-nowrap"
            style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}
          >
            {currentPage + 1} / {pages.length}
          </span>
        </div>
      )}

      {/* Main reading area */}
      {readingMode === 'vertical' ? (
        <div ref={verticalRef} className="flex-1 overflow-y-auto z-[2]" onClick={handlePageClick}>
          <div className="max-w-3xl mx-auto">
            {pages.map((url, i) => {
              const boxes = overlayData?.pages.find((p) => p.page === i)?.boxes ?? []
              return (
                <PageWithOverlay
                  key={i}
                  src={url}
                  boxes={boxes}
                  enabled={overlayEnabled}
                  translations={overlayTranslations}
                  alt={`Page ${i + 1}`}
                  className="w-full block"
                />
              )
            })}
            {nextChapter && (
              <div className="py-8 flex justify-center">
                <button
                  onClick={(e) => { e.stopPropagation(); invoke(IPC.CHAPTER_MARK_READ, pChapterId).catch(() => {}); goToChapter(nextChapter) }}
                  className="px-5 py-2 rounded-xl text-sm font-medium text-white"
                  style={{ background: 'var(--accent-gradient)' }}
                >
                  Next chapter{nextChapter.number !== null ? ` (Ch. ${nextChapter.number})` : ''}
                </button>
              </div>
            )}
          </div>
        </div>
      ) : readingMode === 'rtl' ? (
        /* RTL two-page spread: right page is current, left page is next (manga reading order) */
        <div
          className="flex-1 flex items-center justify-center overflow-hidden cursor-pointer select-none z-[2]"
          onClick={handlePageClick}
        >
          {/* Preload ahead */}
          {currentPage + 2 < pages.length && <link rel="prefetch" href={pages[currentPage + 2]} />}
          {currentPage + 3 < pages.length && <link rel="prefetch" href={pages[currentPage + 3]} />}

          <div className="flex items-center justify-center h-full gap-1" style={{ direction: 'rtl' }}>
            <PageWithOverlay
              src={pages[currentPage]}
              boxes={overlayData?.pages.find((p) => p.page === currentPage)?.boxes ?? []}
              enabled={overlayEnabled}
              translations={overlayTranslations}
              alt={`Page ${currentPage + 1}`}
              className="max-h-full max-w-[50%] object-contain"
              draggable={false}
            />
            {currentPage + 1 < pages.length && (
              <PageWithOverlay
                src={pages[currentPage + 1]}
                boxes={overlayData?.pages.find((p) => p.page === currentPage + 1)?.boxes ?? []}
                enabled={overlayEnabled}
                translations={overlayTranslations}
                alt={`Page ${currentPage + 2}`}
                className="max-h-full max-w-[50%] object-contain"
                draggable={false}
              />
            )}
          </div>
        </div>
      ) : (
        /* LTR single page mode */
        <div
          className="flex-1 flex items-center justify-center overflow-hidden cursor-pointer select-none z-[2]"
          onClick={handlePageClick}
        >
          {/* Preload adjacent pages */}
          {currentPage > 0 && <link rel="prefetch" href={pages[currentPage - 1]} />}
          {currentPage < pages.length - 1 && <link rel="prefetch" href={pages[currentPage + 1]} />}
          {currentPage < pages.length - 2 && <link rel="prefetch" href={pages[currentPage + 2]} />}

          <PageWithOverlay
            src={pages[currentPage]}
            boxes={currentPageBoxes}
            enabled={overlayEnabled}
            translations={overlayTranslations}
            alt={`Page ${currentPage + 1}`}
            className={`max-h-full transition-opacity duration-150 ${
              imageFit === 'width' ? 'w-full object-contain' :
              imageFit === 'height' ? 'h-full object-contain' :
              'object-none'
            }`}
            draggable={false}
          />
        </div>
      )}

      {/* No-OCR-yet hint when overlay button is missing but user might expect it */}
      {isStudio && showControls && overlayData && !overlayData.available && (
        <div className="absolute bottom-32 sm:bottom-24 right-3 sm:right-4 z-30 max-w-[min(280px,calc(100%-24px))] px-3 py-2 rounded-lg text-[11px] text-[var(--text-2)] shadow-lg"
          style={{ background: 'rgba(0,0,0,0.7)', border: '1px solid var(--panel-border)', backdropFilter: 'blur(8px)' }}>
          Translation overlay needs OCR data — run the pipeline OCR stage on this chapter first (Queue page).
        </div>
      )}

      {/* Auto-scroll floating pill */}
      {autoScroll && (
        <div className="absolute top-12 right-4 z-30 flex items-center gap-2 px-3 py-1.5 rounded-full text-xs font-medium text-white shadow-lg"
          style={{ background: 'rgba(var(--accent-rgb),0.9)', backdropFilter: 'blur(8px)' }}>
          <Timer size={12} />
          <span className="tabular-nums">{autoScrollSpeed}s</span>
          <button onClick={() => setAutoScroll(false)} className="text-white/70 hover:text-[var(--text-1)] ml-1"><X size={12} /></button>
        </div>
      )}

      {/* Auto-scroll speed picker */}
      {showSpeedPicker && (
        <div className="absolute top-12 left-1/2 -translate-x-1/2 z-30 rounded-xl p-3 shadow-xl w-max max-w-[calc(100%-24px)]"
          style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)', backdropFilter: 'blur(12px)' }}>
          <p className="text-[10px] font-semibold text-[var(--text-3)] uppercase tracking-wider mb-2">Auto-scroll speed</p>
          <div className="flex flex-wrap gap-1.5 mb-2">
            {[{ label: 'Slow', val: 8 }, { label: 'Medium', val: 5 }, { label: 'Fast', val: 3 }].map((preset) => (
              <button
                key={preset.val}
                onClick={() => { setAutoScrollSpeed(preset.val); setAutoScroll(true); setShowSpeedPicker(false) }}
                className={`px-3 py-1.5 min-h-[40px] sm:min-h-0 rounded-lg text-xs font-medium transition-all ${autoScrollSpeed === preset.val ? 'text-white' : 'text-[var(--text-3)] hover:text-[var(--text-1)]'}`}
                style={autoScrollSpeed === preset.val ? { background: 'var(--accent-gradient)' } : { background: 'var(--surface-3)' }}
              >
                {preset.label} ({preset.val}s)
              </button>
            ))}
          </div>
          <div className="flex items-center gap-2">
            <input
              type="number"
              min={1}
              max={60}
              value={customSpeed}
              onChange={(e) => setCustomSpeed(e.target.value)}
              placeholder="Custom (s)"
              className="w-24 h-10 sm:h-7 px-2 rounded-lg text-xs text-[var(--text-1)] placeholder-zinc-600 outline-none"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
            />
            <button
              onClick={() => {
                const val = parseInt(customSpeed)
                if (val > 0) { setAutoScrollSpeed(val); setAutoScroll(true); setShowSpeedPicker(false) }
              }}
              className="px-3 py-1.5 min-h-[40px] sm:min-h-0 rounded-lg text-xs font-medium text-white"
              style={{ background: 'var(--accent-gradient)' }}
            >
              Start
            </button>
            <button onClick={() => setShowSpeedPicker(false)} className="text-[var(--text-3)] hover:text-[var(--text-2)] text-xs px-2 min-h-[40px] sm:min-h-0">Cancel</button>
          </div>
        </div>
      )}

      {/* Bottom bar */}
      {showControls && (
        <div
          className="absolute bottom-0 left-0 right-0 z-20 px-2 sm:px-4 py-2 sm:py-2.5 border-t"
          style={{ animation: 'page-enter 0.2s var(--ease-snap) both', background: 'var(--surface-1)', borderColor: 'var(--panel-border)' }}
        >
          {/* Progress bar */}
          <div className="flex items-center gap-2 sm:gap-3 mb-2 sm:mb-2.5">
            <span className="text-xs text-[var(--text-3)] w-8 text-right font-mono">{currentPage + 1}</span>
            <div className="flex-1 relative py-2 sm:py-0">
              <div className="h-1 rounded-full" style={{ background: 'var(--surface-3)' }}>
                <div
                  className="h-full rounded-full transition-all duration-100"
                  style={{ width: `${progress}%`, background: 'var(--accent-gradient)' }}
                />
              </div>
              <input
                type="range"
                min={0}
                max={pages.length - 1}
                value={currentPage}
                onChange={(e) => setCurrentPage(Number(e.target.value))}
                className="absolute inset-0 w-full opacity-0 cursor-pointer"
              />
            </div>
            <span className="text-xs text-[var(--text-3)] w-8 font-mono">{pages.length}</span>
          </div>

          {/* Controls row */}
          <div className="flex flex-wrap items-center justify-center sm:justify-between gap-1.5 sm:gap-2">
            {/* Chapter nav */}
            <div className="flex gap-1.5">
              <button
                onClick={() => prevChapter && goToChapter(prevChapter)}
                disabled={!prevChapter}
                className="flex items-center justify-center gap-1.5 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 text-xs text-[var(--text-3)] hover:text-[var(--text-1)] disabled:opacity-20 px-2.5 sm:px-3 py-1.5 rounded-lg transition-colors"
                style={{ background: 'var(--surface-3)' }}
                title="Previous chapter"
              >
                <SkipBack size={12} /> <span className="hidden sm:inline">Prev</span>
              </button>
              <button
                onClick={() => nextChapter && goToChapter(nextChapter)}
                disabled={!nextChapter}
                className="flex items-center justify-center gap-1.5 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 text-xs text-[var(--text-3)] hover:text-[var(--text-1)] disabled:opacity-20 px-2.5 sm:px-3 py-1.5 rounded-lg transition-colors"
                style={{ background: 'var(--surface-3)' }}
                title="Next chapter"
              >
                <span className="hidden sm:inline">Next</span> <SkipForward size={12} />
              </button>
            </div>

            {/* Auto-scroll toggle */}
            <button
              onClick={() => {
                if (autoScroll) { setAutoScroll(false) }
                else { setShowSpeedPicker(!showSpeedPicker) }
              }}
              title="Auto-scroll"
              className={`flex items-center justify-center gap-1.5 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 text-xs px-2.5 sm:px-3 py-1.5 rounded-lg transition-all ${autoScroll ? 'text-[var(--accent)]' : 'text-[var(--text-3)] hover:text-[var(--text-1)]'}`}
              style={autoScroll ? { background: 'rgba(var(--accent-rgb),0.18)', border: '1px solid rgba(var(--accent-rgb),0.35)' } : { background: 'var(--surface-3)' }}
            >
              <Timer size={12} /> <span className={autoScroll ? '' : 'hidden sm:inline'}>{autoScroll ? `Auto ${autoScrollSpeed}s` : 'Auto-scroll'}</span>
            </button>

            {/* Translate overlay toggle + language picker */}
            {isStudio && overlayData?.available && (
              <div className="relative flex items-center rounded-lg overflow-hidden" style={{ background: 'var(--surface-3)' }}>
                <button
                  onClick={handleToggleOverlay}
                  disabled={translatingOverlay}
                  className={`flex items-center gap-1.5 min-h-[40px] sm:min-h-0 text-xs px-3 py-1.5 transition-all disabled:opacity-50 ${
                    overlayEnabled ? 'text-[var(--accent)]' : 'text-[var(--text-3)] hover:text-[var(--text-1)]'
                  }`}
                  style={overlayEnabled ? { background: 'rgba(var(--accent-rgb),0.18)' } : {}}
                  title="Toggle translated overlay"
                >
                  {translatingOverlay ? <Loader2 size={12} className="animate-spin" /> : <Languages size={12} />}
                  {overlayEnabled ? 'Overlay on' : 'Translate'}
                </button>
                <button
                  onClick={() => setOverlayLangMenuOpen((o) => !o)}
                  className="px-2 py-1.5 min-h-[40px] sm:min-h-0 text-[11px] font-semibold text-[var(--accent)] border-l border-[var(--panel-border)] hover:bg-[var(--surface-4)]"
                  title="Target language"
                >
                  {overlayLang.toUpperCase()} <ChevronDown size={9} className="inline ml-0.5" />
                </button>
                {overlayLangMenuOpen && (
                  <div className="absolute right-0 bottom-full mb-1 rounded-xl overflow-hidden shadow-xl z-30 py-1 w-44 max-h-72 overflow-y-auto"
                    style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
                    {OVERLAY_LANGS.map((l) => (
                      <button
                        key={l.code}
                        onClick={() => {
                          setOverlayLang(l.code)
                          setOverlayLangMenuOpen(false)
                          setOverlayTranslations({}) // re-translate on next toggle
                        }}
                        className={`w-full text-left px-3 py-1.5 text-xs hover:bg-[var(--surface-4)] transition-colors ${
                          overlayLang === l.code ? 'text-[var(--accent)]' : 'text-[var(--text-2)]'
                        }`}
                      >
                        {l.label} <span className="text-[var(--text-3)] ml-1">({l.code})</span>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}

            {/* Read Aloud (Web Speech API) */}
            {isStudio && currentPageBoxes.length > 0 && (
              !reading ? (
                <button
                  onClick={handleReadAloud}
                  className="flex items-center gap-1.5 min-h-[40px] sm:min-h-0 text-xs px-3 py-1.5 rounded-lg text-[var(--text-3)] hover:text-[var(--text-1)] transition-all"
                  style={{ background: 'var(--surface-3)' }}
                  title="Read this page aloud"
                >
                  <Volume2 size={12} /> Listen
                </button>
              ) : (
                <button
                  onClick={handleStopReading}
                  className="flex items-center gap-1.5 min-h-[40px] sm:min-h-0 text-xs px-3 py-1.5 rounded-lg text-[var(--accent)] transition-all"
                  style={{ background: 'rgba(var(--accent-rgb),0.15)' }}
                >
                  <Pause size={12} /> Stop
                </button>
              )
            )}

            {/* Mode presets */}
            <div className="flex rounded-lg p-0.5" style={{ background: 'var(--surface-3)' }}>
              {MODE_PRESETS.map((p) => (
                <button
                  key={p.mode}
                  onClick={() => setReadingMode(p.mode)}
                  title={p.label}
                  className={`flex items-center justify-center gap-1.5 min-w-[40px] min-h-[36px] sm:min-w-0 sm:min-h-0 text-xs px-2.5 sm:px-3 py-1.5 rounded-md transition-all duration-200 ${
                    readingMode === p.mode ? 'text-white' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
                  }`}
                  style={readingMode === p.mode ? { background: 'var(--accent-gradient)' } : {}}
                >
                  {p.icon} <span className="hidden sm:inline">{p.label}</span>
                </button>
              ))}
            </div>

            {/* Image fit */}
            <div className="flex rounded-lg p-0.5" style={{ background: 'var(--surface-3)' }}>
              {(['width', 'height', 'original'] as ImageFit[]).map((fit) => (
                <button
                  key={fit}
                  onClick={() => setImageFit(fit)}
                  className={`text-xs px-2.5 sm:px-3 py-1.5 min-h-[36px] sm:min-h-0 rounded-md transition-all duration-200 ${
                    imageFit === fit ? 'text-white' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
                  }`}
                  style={imageFit === fit ? { background: 'var(--accent-gradient)' } : {}}
                >
                  {fit === 'width' ? 'Fit W' : fit === 'height' ? 'Fit H' : 'Orig'}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
