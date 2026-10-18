import { useEffect, useState } from 'react'
import { useLibraryStore } from '../stores/library.store'
import { useUiStore } from '../stores/ui.store'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { coverUrl } from '../utils/cover-url'
import {
  ArrowLeft, BookOpen, Star, RefreshCw, Download, Check, X,
  ChevronDown, Play, Loader2, Search, Zap, Headphones,
  Eye, Volume2, Film, FileText, Edit3, Save, DownloadCloud, Sparkles, Users,
} from 'lucide-react'
import type { MangaDetails, MangaSearchResult, Chapter } from '../../shared/types'
import { toast } from '../components/Toaster'
import { prepareFromSource } from '../utils/open-manga'
import { onEvent } from '../hooks/useIpc'
import { useAppModeStore } from '../stores/app-mode.store'

interface SourceTab {
  sourceId: string
  sourceName: string
  mangaSourceId: string
  chapterCount: number
}

interface MangaRec {
  malId: string // source_id on the same plugin
  result: MangaSearchResult
  title: string
  coverUrl?: string
  genres: string[]
  score?: number
  type?: string
  members?: number
}

interface ScriptSegment {
  page: number
  text: string
}

export function MangaDetail() {
  const isStudio = useAppModeStore((state) => state.mode === 'studio')
  const { selectedManga, chapters, chaptersLoading, addToLibrary, selectManga, fetchChapters, fetchChaptersBySource, setActiveSourcePlugin, toggleFavorite } = useLibraryStore()
  const { pageParams, goBack, navigate } = useUiStore()
  const [syncing, setSyncing] = useState(false)
  const [descExpanded, setDescExpanded] = useState(false)
  const [sortDesc, setSortDesc] = useState(false)
  const [sourceTabs, setSourceTabs] = useState<SourceTab[]>([])
  const [activeSource, setActiveSource] = useState<SourceTab | null>(null)
  const [findingSources, setFindingSources] = useState(false)
  const [downloadProgress, setDownloadProgress] = useState<Record<string, { current: number; total: number }>>({})
  const [failedIds, setFailedIds] = useState<Set<string>>(new Set())
  const [chapterSearch, setChapterSearch] = useState('')
  const [pipelineRunning, setPipelineRunning] = useState(false)
  const [pipelineStatus, setPipelineStatus] = useState<Record<string, { stage: string; percent: number; message?: string }>>({})
  const [selectedVoice, setSelectedVoice] = useState('af_heart')
  const [selectedEngine, setSelectedEngine] = useState('kokoro')
  const [recs, setRecs] = useState<MangaRec[]>([])
  const [recsLoading, setRecsLoading] = useState(false)
  const [openingRec, setOpeningRec] = useState<string | null>(null)

  // Script editor state
  const [scriptChapter, setScriptChapter] = useState<Chapter | null>(null)
  const [scriptSegments, setScriptSegments] = useState<ScriptSegment[]>([])
  const [scriptLoading, setScriptLoading] = useState(false)
  const [scriptDirty, setScriptDirty] = useState(false)
  const [scriptSaving, setScriptSaving] = useState(false)
  const [pipelineCollapsed, setPipelineCollapsed] = useState(false)

  const mangaId = pageParams.mangaId as string
  const sourceId = pageParams.sourceId as string | undefined

  useEffect(() => {
    if (!mangaId) return
    let cancelled = false

    const loadAndSync = async () => {
      try { await selectManga(mangaId) } catch (err) { console.error('Failed to load manga:', err); return }
      const manga = await invoke<any>(IPC.LIBRARY_GET, mangaId)
      if (cancelled || !manga) return

      setActiveSourcePlugin(manga.source_plugin)
      try { await fetchChaptersBySource(mangaId, manga.source_plugin) } catch { await fetchChapters(mangaId) }

      setSyncing(true)
      try {
        const details = await invoke<MangaDetails>(IPC.PLUGIN_GET_DETAILS, manga.source_plugin, manga.source_id)
        if (cancelled) return
        await invoke(IPC.LIBRARY_UPDATE_DETAILS, {
          id: manga.id, author: details.author, artist: details.artist,
          description: details.description, cover_path: details.cover_url,
          status: details.status, genres: details.genres,
        })
        await invoke(IPC.CHAPTER_SYNC, manga.id, manga.source_plugin, manga.source_id)
        if (cancelled) return
        await selectManga(manga.id)
        await fetchChaptersBySource(manga.id, manga.source_plugin)
      } catch (err) {
        console.error('Auto-sync failed:', err)
        await fetchChapters(manga.id)
      } finally {
        if (!cancelled) setSyncing(false)
      }

      if (!cancelled) {
        invoke(IPC.HISTORY_TOUCH, manga.id).catch(() => {})
      }

      if (!cancelled) {
        setFindingSources(true)
        try {
          const sources = await invoke<SourceTab[]>(IPC.PLUGIN_FIND_ACROSS_SOURCES, manga.title, manga.source_plugin, manga.source_id)
          if (!cancelled && sources) {
            setSourceTabs(sources)
            if (sources.length > 0) setActiveSource(sources[0])
          }
        } catch (err) { console.error('Source search failed:', err) }
        finally { if (!cancelled) setFindingSources(false) }
      }
    }

    loadAndSync()
    return () => { cancelled = true }
  }, [mangaId])

  // Recommendations come from the SAME source plugin the manga is on: titles sharing its genres/tags
  useEffect(() => {
    const manga = selectedManga
    if (!manga || manga.id !== mangaId) return
    const plugin = manga.source_plugin
    let cancelled = false
    setRecs([])
    setRecsLoading(true)

    const norm = (t: string) => t.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim()
    ;(async () => {
      const tags = await invoke<{ id: string; name: string }[]>(IPC.PLUGIN_GET_TAGS, plugin).catch(() => [] as { id: string; name: string }[])
      const wanted = new Set((manga.genres || []).map(norm))
      const matched = (tags || []).filter((t) => wanted.has(norm(t.name))).slice(0, 4)

      const bucket = new Map<string, { result: MangaSearchResult; shared: string[] }>()
      const add = (r: MangaSearchResult, tag?: string) => {
        if (!r?.source_id || r.source_id === manga.source_id || !r.title) return
        const cur = bucket.get(r.source_id) ?? { result: r, shared: [] }
        if (tag && !cur.shared.includes(tag)) cur.shared.push(tag)
        bucket.set(r.source_id, cur)
      }

      await Promise.all(matched.map(async (t) => {
        const list = await invoke<MangaSearchResult[]>(IPC.PLUGIN_GET_BY_TAG, plugin, t.id, 1).catch(() => [] as MangaSearchResult[])
        for (const r of list || []) add(r, t.name)
      }))
      if (bucket.size < 10) {
        const popular = await invoke<MangaSearchResult[]>(IPC.PLUGIN_GET_POPULAR, plugin, 1).catch(() => [] as MangaSearchResult[])
        for (const r of popular || []) add(r)
      }

      // Titles matching more of this manga's genres rank first; keep source order within a tier
      const ranked = [...bucket.values()]
        .map((v, i) => ({ v, i }))
        .sort((x, y) => y.v.shared.length - x.v.shared.length || x.i - y.i)
        .slice(0, 10)
        .map(({ v }): MangaRec => ({
          malId: v.result.source_id,
          result: v.result,
          title: v.result.title,
          coverUrl: v.result.cover_url,
          genres: v.shared,
        }))
      if (!cancelled) setRecs(ranked)
    })()
      .catch(() => { if (!cancelled) setRecs([]) })
      .finally(() => { if (!cancelled) setRecsLoading(false) })
    return () => { cancelled = true }
  }, [mangaId, selectedManga?.id, selectedManga?.source_plugin])

  // Open a recommendation straight from the same source: add to library, sync chapters, navigate
  const openRecommendation = async (rec: MangaRec) => {
    if (openingRec || !selectedManga) return
    setOpeningRec(rec.malId)
    try {
      const plugin = selectedManga.source_plugin
      const manga = await prepareFromSource(plugin, rec.result, addToLibrary)
      navigate('manga-detail', { mangaId: manga.id, sourceId: plugin })
    } catch (err: any) {
      toast.error('Could not open', err?.message || rec.title)
    } finally {
      setOpeningRec(null)
    }
  }

  // Reset all local state when mangaId changes (e.g. when clicking a recommendation)
  useEffect(() => {
    setSourceTabs([])
    setActiveSource(null)
    setChapterSearch('')
  }, [mangaId])

  const handleSwitchSource = async (source: SourceTab) => {
    if (!selectedManga || source.sourceId === activeSource?.sourceId) return
    setActiveSource(source)
    setActiveSourcePlugin(source.sourceId)
    setSyncing(true)
    try {
      await invoke(IPC.CHAPTER_SYNC, selectedManga.id, source.sourceId, source.mangaSourceId)
      await fetchChaptersBySource(selectedManga.id, source.sourceId)
    } catch (err) { console.error('Source switch failed:', err); await fetchChapters(selectedManga.id) }
    finally { setSyncing(false) }
  }

  const handleRefresh = async () => {
    if (!selectedManga) return
    const sp = activeSource?.sourceId || selectedManga.source_plugin
    const si = activeSource?.mangaSourceId || selectedManga.source_id
    setSyncing(true)
    try {
      const details = await invoke<MangaDetails>(IPC.PLUGIN_GET_DETAILS, sp, si)
      await invoke(IPC.LIBRARY_UPDATE_DETAILS, {
        id: selectedManga.id, author: details.author, artist: details.artist,
        description: details.description, cover_path: details.cover_url,
        status: details.status, genres: details.genres,
      })
      await invoke(IPC.CHAPTER_SYNC, selectedManga.id, sp, si)
      await selectManga(selectedManga.id)
      await fetchChaptersBySource(selectedManga.id, sp)
    } catch (err) { console.error('Refresh failed:', err); await fetchChapters(selectedManga.id) }
    finally { setSyncing(false) }
  }

  const getActivePlugin = () => activeSource?.sourceId || selectedManga?.source_plugin || ''

  useEffect(() => {
    if (!selectedManga) return
    const unsub = onEvent(IPC.DOWNLOAD_PROGRESS, (data: any) => {
      if (!data?.chapterId) return
      const uiStatus = data.status === 'failed' || data.status === 'none' ? 'none' : data.status
      setFailedIds((prev) => {
        const had = prev.has(data.chapterId)
        if (data.status === 'failed') { const n = new Set(prev); n.add(data.chapterId); return n }
        if (had && data.status !== 'none') { const n = new Set(prev); n.delete(data.chapterId); return n }
        return prev
      })
      useLibraryStore.setState((state) => ({
        chapters: state.chapters.map((ch) =>
          ch.id === data.chapterId ? { ...ch, download_status: uiStatus } : ch
        ),
      }))
      if (uiStatus === 'downloading' || uiStatus === 'queued') {
        setDownloadProgress((prev) => ({
          ...prev,
          [data.chapterId]: { current: data.current || 0, total: data.total || 0 },
        }))
      } else {
        setDownloadProgress((prev) => {
          const next = { ...prev }
          delete next[data.chapterId]
          return next
        })
      }
    })
    return unsub
  }, [selectedManga?.id])

  const handleStartReading = () => {
    if (!selectedManga || chapters.length === 0) return
    const sorted = [...chapters].sort((a, b) => (a.number ?? 0) - (b.number ?? 0))
    const firstUnread = sorted.find((ch) => !ch.is_read)
    const target = firstUnread ?? sorted[0]
    navigate('reader', {
      mangaId: selectedManga.id, chapterId: target.id,
      sourceId: target.source_plugin || selectedManga.source_plugin,
      sourceChapterId: target.source_id,
    })
  }

  const handleChapterClick = (ch: Chapter) => {
    if (!selectedManga) return
    // Auto-queue download if not already downloaded/queued
    if (ch.download_status !== 'complete' && ch.download_status !== 'downloading' && ch.download_status !== 'queued') {
      handleDownload(ch)
    }
    navigate('reader', {
      mangaId: selectedManga.id, chapterId: ch.id,
      sourceId: ch.source_plugin || selectedManga.source_plugin,
      sourceChapterId: ch.source_id,
    })
  }

  const handleDownload = async (ch: Chapter, e?: React.MouseEvent) => {
    if (e) e.stopPropagation()
    if (!selectedManga) return
    if (ch.download_status === 'complete' || ch.download_status === 'downloading' || ch.download_status === 'queued') return
    setFailedIds((prev) => { if (!prev.has(ch.id)) return prev; const n = new Set(prev); n.delete(ch.id); return n })
    useLibraryStore.setState((state) => ({
      chapters: state.chapters.map((item) =>
        item.id === ch.id ? { ...item, download_status: 'queued' } : item
      ),
    }))
    try {
      await invoke(IPC.DOWNLOAD_CHAPTER, selectedManga.id, ch.id, ch.source_plugin || getActivePlugin(), ch.source_id)
    } catch (err) {
      useLibraryStore.setState((state) => ({
        chapters: state.chapters.map((item) =>
          item.id === ch.id ? { ...item, download_status: 'none' } : item
        ),
      }))
      console.error('Download failed:', err)
    }
  }

  const handleCancelDownload = async (ch: Chapter, e?: React.MouseEvent) => {
    if (e) e.stopPropagation()
    try { await invoke(IPC.DOWNLOAD_CANCEL, ch.id) } catch (err) { console.error('Cancel failed:', err) }
    useLibraryStore.setState((state) => ({
      chapters: state.chapters.map((item) => item.id === ch.id ? { ...item, download_status: 'none' } : item),
    }))
  }

  const queueChapters = async (list: Chapter[]) => {
    if (!selectedManga) return
    const plugin = getActivePlugin()
    const ids = new Set(list.map((c) => c.id))
    setFailedIds((prev) => { const n = new Set(prev); ids.forEach((i) => n.delete(i)); return n })
    useLibraryStore.setState((state) => ({
      chapters: state.chapters.map((item) => ids.has(item.id) ? { ...item, download_status: 'queued' } : item),
    }))
    for (const ch of list) {
      try { await invoke(IPC.DOWNLOAD_CHAPTER, selectedManga.id, ch.id, ch.source_plugin || plugin, ch.source_id) }
      catch (err) {
        console.error(`Download failed for ch ${ch.number}:`, err)
        useLibraryStore.setState((state) => ({
          chapters: state.chapters.map((item) => item.id === ch.id ? { ...item, download_status: 'none' } : item),
        }))
        setFailedIds((prev) => new Set(prev).add(ch.id))
      }
    }
  }

  const isDownloadable = (ch: Chapter) =>
    ch.download_status !== 'complete' && ch.download_status !== 'downloading' && ch.download_status !== 'queued'

  // Unread chapters (ascending) that are not yet downloaded
  const handleDownloadUnread = () => {
    const list = [...chapters].sort((a, b) => (a.number ?? 0) - (b.number ?? 0)).filter((ch) => !ch.is_read && isDownloadable(ch))
    return queueChapters(list)
  }
  // Next N undownloaded chapters starting from the first unread one
  const handleDownloadNext = (n: number) => {
    const sorted = [...chapters].sort((a, b) => (a.number ?? 0) - (b.number ?? 0))
    const start = sorted.findIndex((ch) => !ch.is_read)
    if (start < 0) return Promise.resolve()
    const list = sorted.slice(start).filter(isDownloadable).slice(0, n)
    return queueChapters(list)
  }

  // ── BIG BUTTON: Download All ──
  const handleDownloadAll = async () => {
    if (!selectedManga) return
    const plugin = getActivePlugin()
    const sorted = [...chapters].sort((a, b) => (a.number ?? 0) - (b.number ?? 0))
    const downloadable = sorted.filter((ch) => ch.download_status !== 'complete' && ch.download_status !== 'downloading' && ch.download_status !== 'queued')
    for (const ch of downloadable) {
      try { await invoke(IPC.DOWNLOAD_CHAPTER, selectedManga.id, ch.id, ch.source_plugin || plugin, ch.source_id) }
      catch (err) { console.error(`Download failed for ch ${ch.number}:`, err) }
    }
  }

  // ── BIG BUTTON: OCR All downloaded chapters ──
  const handleOcrAll = async () => {
    if (!selectedManga) return
    setPipelineRunning(true)
    const ready = chapters.filter((ch) => ch.download_status === 'complete' && ch.ocr_status !== 'complete' && ch.ocr_status !== 'running')
    for (const ch of ready) {
      try { await invoke(IPC.PIPELINE_START, selectedManga.id, ch.id, ['ocr']) }
      catch (err) { console.error(`OCR failed for ch ${ch.number}:`, err) }
    }
    setPipelineRunning(false)
  }

  // ── BIG BUTTON: Generate Audio for all OCR'd chapters ──
  const handleGenerateAll = async () => {
    if (!selectedManga) return
    setPipelineRunning(true)
    const ready = chapters.filter((ch) => ch.ocr_status === 'complete' && ch.tts_status !== 'complete' && ch.tts_status !== 'running')
    for (const ch of ready) {
      try {
        // Reset TTS status first in case of previous failure
        await invoke(IPC.PIPELINE_RESET_CHAPTER, ch.id)
        await invoke(IPC.PIPELINE_START, selectedManga.id, ch.id, ['tts', 'sync_map'],
          { engine: selectedEngine, voice: selectedVoice, speed: 1.0 })
      }
      catch (err) { console.error(`TTS failed for ch ${ch.number}:`, err) }
    }
    setPipelineRunning(false)
  }

  // ── Script editor ──
  const handleOpenScript = async (ch: Chapter, e: React.MouseEvent) => {
    e.stopPropagation()
    if (!selectedManga) return
    setScriptChapter(ch)
    setScriptLoading(true)
    setScriptDirty(false)
    try {
      const segments = await invoke<ScriptSegment[] | null>(IPC.PIPELINE_GET_SCRIPT, selectedManga.id, ch.id)
      setScriptSegments(segments || [])
    } catch (err) {
      console.error('Failed to load script:', err)
      setScriptSegments([])
    } finally {
      setScriptLoading(false)
    }
  }

  const handleScriptEdit = (idx: number, newText: string) => {
    setScriptSegments((prev) => prev.map((s, i) => i === idx ? { ...s, text: newText } : s))
    setScriptDirty(true)
  }

  const handleSaveScript = async () => {
    if (!selectedManga || !scriptChapter) return
    setScriptSaving(true)
    try {
      await invoke(IPC.PIPELINE_SAVE_SCRIPT, selectedManga.id, scriptChapter.id, scriptSegments)
      setScriptDirty(false)
    } catch (err) { console.error('Failed to save script:', err) }
    finally { setScriptSaving(false) }
  }

  const handleExportText = async (ch: Chapter, e: React.MouseEvent) => {
    e.stopPropagation()
    if (!selectedManga) return
    try {
      await invoke(IPC.PIPELINE_EXPORT_TEXT, selectedManga.id, ch.id)
    } catch (err) { console.error('Export failed:', err) }
  }

  const handleGenerateChapter = async (ch: Chapter, e: React.MouseEvent) => {
    e.stopPropagation()
    if (!selectedManga) return
    try {
      await invoke(IPC.PIPELINE_RESET_CHAPTER, ch.id)
      await invoke(IPC.PIPELINE_START, selectedManga.id, ch.id, ['tts', 'sync_map'],
        { engine: selectedEngine, voice: selectedVoice, speed: 1.0 })
    } catch (err) { console.error('Generate failed:', err) }
  }

  // Voice config
  const handleVoiceChange = async (voice: string) => {
    setSelectedVoice(voice)
    try {
      const cfg = await invoke<any>(IPC.CONFIG_GET, 'pipeline') || {}
      await invoke(IPC.CONFIG_SET, 'pipeline', { ...cfg, tts_voice: voice })
    } catch {}
  }
  const handleEngineChange = async (engine: string) => {
    setSelectedEngine(engine)
    const defaultVoices: Record<string, string> = { kokoro: 'af_heart', chatterbox: 'default', vctk: 'p225', xtts: 'default' }
    const newVoice = defaultVoices[engine] || 'default'
    setSelectedVoice(newVoice)
    try {
      const cfg = await invoke<any>(IPC.CONFIG_GET, 'pipeline') || {}
      await invoke(IPC.CONFIG_SET, 'pipeline', { ...cfg, tts_engine: engine, tts_voice: newVoice })
    } catch {}
  }

  const handlePlayChapter = (ch: Chapter, e: React.MouseEvent) => {
    e.stopPropagation()
    if (!selectedManga) return
    navigate('player', {
      mangaId: selectedManga.id, chapterId: ch.id,
      chapterTitle: ch.title || `Chapter ${ch.number ?? '?'}`,
    })
  }

  const handleBookmark = async () => {
    if (!selectedManga) return
    await toggleFavorite(selectedManga.id)
    await selectManga(selectedManga.id)
  }

  // ── Event listeners ──
  useEffect(() => {
    const mid = selectedManga?.id
    if (!mid) return
    const unsub = onEvent('download:progress', (data: any) => {
      if (!data) return
      if (data.chapterId && data.current !== undefined && data.total !== undefined) {
        setDownloadProgress((prev) => ({ ...prev, [data.chapterId]: { current: data.current, total: data.total } }))
      }
      if (data.status === 'complete' || data.status === 'failed') {
        setDownloadProgress((prev) => { const next = { ...prev }; delete next[data.chapterId]; return next })
        fetchChapters(mid)
      }
    })
    return unsub
  }, [selectedManga?.id])

  useEffect(() => {
    const mid = selectedManga?.id
    if (!mid) return
    const unsub = onEvent('pipeline:progress', (data: any) => {
      if (!data) return
      const cid = data.chapterId

      if (data.type === 'progress' && cid) {
        setPipelineStatus((prev) => ({ ...prev, [cid]: { stage: data.stage || '', percent: data.percent || 0 } }))
      }
      if (data.type === 'info' && cid) {
        setPipelineStatus((prev) => ({ ...prev, [cid]: { stage: prev[cid]?.stage || '', percent: prev[cid]?.percent || 0, message: data.message } }))
      }
      if (data.type === 'stage_complete' && cid) {
        setPipelineStatus((prev) => ({ ...prev, [cid]: { stage: data.stage + ' done', percent: 100 } }))
        fetchChapters(mid)
      }
      if (data.type === 'done' || data.type === 'error') {
        if (cid) setPipelineStatus((prev) => { const n = { ...prev }; delete n[cid]; return n })
        fetchChapters(mid)
      }
      if (data.status === 'completed' || data.status === 'failed') fetchChapters(mid)
    })
    return unsub
  }, [selectedManga?.id])

  // Load saved voice config
  useEffect(() => {
    invoke<any>(IPC.CONFIG_GET, 'pipeline').then((cfg) => {
      if (cfg?.tts_voice) setSelectedVoice(cfg.tts_voice)
      if (cfg?.tts_engine) setSelectedEngine(cfg.tts_engine)
    }).catch(() => {})
  }, [])

  if (!selectedManga) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-4">
        <div className="spinner-ring" />
        <div className="loader-dots"><span /><span /><span /></div>
      </div>
    )
  }

  const cover = coverUrl(selectedManga.cover_path)
  const filteredChapters = chapters.filter((ch) => {
    if (!chapterSearch) return true
    const q = chapterSearch.toLowerCase()
    return (ch.number !== null && String(ch.number).includes(q)) || (ch.title?.toLowerCase().includes(q) ?? false)
  })
  const sortedChapters = sortDesc
    ? [...filteredChapters].sort((a, b) => (b.number ?? 0) - (a.number ?? 0))
    : [...filteredChapters].sort((a, b) => (a.number ?? 0) - (b.number ?? 0))

  const readCount = chapters.filter((ch) => ch.is_read).length
  const downloadedCount = chapters.filter((ch) => ch.download_status === 'complete').length
  const ocrCount = chapters.filter((ch) => ch.ocr_status === 'complete').length
  const ttsCount = chapters.filter((ch) => ch.tts_status === 'complete').length
  const downloadingCount = chapters.filter((ch) => ch.download_status === 'downloading' || ch.download_status === 'queued').length
  const ocrRunning = chapters.filter((ch) => ch.ocr_status === 'running').length
  const ttsRunning = chapters.filter((ch) => ch.tts_status === 'running').length

  return (
    <div className="flex flex-col h-full overflow-y-auto overflow-x-hidden lg:overflow-y-hidden">
      {/* Hero section */}
      <div className="relative overflow-hidden shrink-0">
        {cover && (
          <div className="absolute inset-0 -m-8">
            <img src={cover} className="w-full h-full object-cover scale-125 blur-[60px] opacity-15" referrerPolicy="no-referrer" />
            <div className="absolute inset-0" style={{ background: 'linear-gradient(to bottom, transparent 0%, var(--base) 100%)' }} />
          </div>
        )}

        <div className="relative z-10 px-3 sm:px-5 pt-3 pb-4">
          <button onClick={goBack} className="flex items-center gap-2 min-h-[40px] sm:min-h-0 text-[var(--text-3)] hover:text-[var(--text-1)] text-xs mb-1 sm:mb-3 transition-colors">
            <ArrowLeft size={16} /> Back
            {syncing && <span className="text-xs text-[var(--accent)] animate-pulse ml-2">Syncing...</span>}
          </button>

          <div className="flex flex-col lg:flex-row gap-4 lg:gap-5">
            <div className="flex flex-col sm:flex-row gap-4 sm:gap-5 flex-1 min-w-0">
            <div className="w-32 h-[182px] sm:w-36 sm:h-[204px] mx-auto sm:mx-0 rounded-xl overflow-hidden shrink-0 shadow-2xl shadow-black/40" style={{ border: '1px solid var(--panel-border)' }}>
              {cover ? (
                <img src={cover} alt={selectedManga.title} className="w-full h-full object-cover" referrerPolicy="no-referrer" />
              ) : (
                <div className="w-full h-full flex items-center justify-center text-[var(--text-3)] text-5xl font-bold" style={{ background: 'var(--surface-2)' }}>
                  {selectedManga.title.charAt(0)}
                </div>
              )}
            </div>

            <div className="flex-1 min-w-0 pt-1">
              <h1 className="text-lg sm:text-xl font-bold text-white leading-tight mb-1.5 break-words">{selectedManga.title}</h1>
              <div className="flex flex-wrap items-center gap-2 mb-3 text-sm">
                {selectedManga.author && <span className="text-[var(--text-3)]">By <span className="text-[var(--text-1)]">{selectedManga.author}</span></span>}
                {selectedManga.status && (
                  <span className={`inline-flex items-center gap-1.5 text-xs font-medium px-2 py-0.5 rounded-full ${
                    selectedManga.status === 'ongoing' ? 'bg-[rgba(var(--accent-rgb),0.15)] text-[var(--accent)]'
                      : selectedManga.status === 'completed' ? 'bg-[rgba(var(--secondary-rgb),0.15)] text-[var(--secondary)]'
                        : 'bg-[var(--surface-3)] text-[var(--text-3)]'
                  }`}>
                    {selectedManga.status === 'ongoing' && <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)] status-pulse" />}
                    {selectedManga.status}
                  </span>
                )}
              </div>

              {selectedManga.genres.length > 0 && (
                <div className="flex flex-wrap gap-1.5 mb-3">
                  {selectedManga.genres.slice(0, 8).map((g) => (
                    <span key={g} className="px-2.5 py-1 rounded-full text-xs text-[var(--text-2)] backdrop-blur-sm" style={{ background: 'var(--panel-border)' }}>{g}</span>
                  ))}
                </div>
              )}

              {selectedManga.description && (
                <div className="mb-3">
                  <p className={`text-sm text-[var(--text-3)] leading-relaxed ${!descExpanded ? 'line-clamp-2' : ''}`}>{selectedManga.description}</p>
                  {selectedManga.description.length > 150 && (
                    <button onClick={() => setDescExpanded(!descExpanded)} className="text-xs text-[var(--accent)] hover:text-[var(--accent-hover)] mt-0.5 transition-colors">
                      {descExpanded ? 'Show less' : 'Show more'}
                    </button>
                  )}
                </div>
              )}

              <div className="flex gap-2 mt-2">
                <button onClick={handleStartReading} disabled={chapters.length === 0}
                  className="flex items-center justify-center gap-2 px-4 py-2 min-h-[40px] flex-1 sm:flex-none rounded-xl text-sm font-medium text-white disabled:opacity-40 transition-all hover:opacity-90"
                  style={{ background: 'var(--accent-gradient)' }}>
                  <Play size={13} fill="white" /> {readCount > 0 ? 'Continue' : 'Read'}
                </button>
                <button onClick={handleBookmark}
                  className={`px-3 py-2 min-h-[40px] min-w-[40px] flex items-center justify-center rounded-xl text-sm transition-all ${selectedManga.is_favorite ? 'text-amber-400' : 'text-[var(--text-3)] hover:text-amber-400'}`}
                  style={{ background: selectedManga.is_favorite ? 'rgba(251,191,36,0.08)' : 'var(--surface-3)', border: selectedManga.is_favorite ? '1px solid rgba(251,191,36,0.15)' : '1px solid var(--panel-border)' }}>
                  <Star size={13} fill={selectedManga.is_favorite ? 'currentColor' : 'none'} />
                </button>
                <button onClick={handleRefresh} disabled={syncing}
                  className="px-3 py-2 min-h-[40px] min-w-[40px] flex items-center justify-center rounded-xl text-sm text-[var(--text-3)] hover:text-[var(--text-1)] disabled:opacity-40 transition-all"
                  style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
                  <RefreshCw size={13} className={syncing ? 'animate-spin' : ''} />
                </button>
              </div>

              <div className="flex flex-wrap gap-x-5 gap-y-1 mt-3 text-xs text-[var(--text-3)]">
                <span>{chapters.length} chapters</span>
                <span>{readCount} read</span>
                <span>{downloadedCount} downloaded</span>
                {isStudio && ocrCount > 0 && <span>{ocrCount} OCR'd</span>}
                {isStudio && ttsCount > 0 && <span>{ttsCount} with audio</span>}
              </div>
            </div>
            </div>

            {(recsLoading || recs.length > 0) && (
              <div className="w-full lg:w-72 shrink-0 rounded-xl flex flex-col lg:h-[272px] overflow-hidden"
                style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}>
                <div className="flex items-center gap-1.5 px-3 pt-3 pb-2 shrink-0">
                  <Sparkles size={13} style={{ color: 'var(--accent-hover)' }} />
                  <h3 className="text-[11px] font-bold uppercase tracking-wider text-[var(--text-2)]">More like this</h3>
                  {recs.length > 0 && <span className="ml-auto text-[10px] text-[var(--text-3)]">{recs.length}</span>}
                </div>
                <div className="flex-1 flex lg:block gap-2 lg:gap-0 overflow-x-auto lg:overflow-x-hidden lg:overflow-y-auto snap-x snap-mandatory lg:snap-none scrollbar-none px-2 pb-2 lg:space-y-1">
                  {recsLoading && recs.length === 0 && Array.from({ length: 4 }).map((_, i) => (
                    <div key={i} className="flex gap-2.5 p-1.5 animate-pulse w-56 shrink-0 lg:w-auto">
                      <div className="w-10 h-14 rounded-md shrink-0" style={{ background: 'var(--surface-3)' }} />
                      <div className="flex-1 space-y-2 py-1.5">
                        <div className="h-2.5 rounded" style={{ background: 'var(--surface-3)', width: '80%' }} />
                        <div className="h-2 rounded" style={{ background: 'var(--surface-3)', width: '50%' }} />
                      </div>
                    </div>
                  ))}
                  {recs.map((item) => (
                    <button
                      key={item.malId}
                      onClick={() => openRecommendation(item)}
                      disabled={!!openingRec}
                      className="w-56 shrink-0 snap-start lg:w-full flex gap-2.5 p-1.5 rounded-lg text-left group transition-colors hover:bg-[rgba(var(--accent-rgb),0.10)] disabled:opacity-60"
                    >
                      <div className="w-10 h-14 rounded-md overflow-hidden shrink-0 relative" style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
                        {item.coverUrl ? (
                          <img src={coverUrl(item.coverUrl) || ''} alt="" loading="lazy" className="w-full h-full object-cover"
                            onError={(e) => { (e.currentTarget as HTMLImageElement).style.display = 'none' }} />
                        ) : null}
                        <div className="absolute inset-0 -z-0 flex items-center justify-center text-[11px] font-bold text-[var(--text-3)]">{item.title.charAt(0)}</div>
                        {openingRec === item.malId && (
                          <div className="absolute inset-0 flex items-center justify-center bg-black/60"><Loader2 size={14} className="animate-spin text-white" /></div>
                        )}
                      </div>
                      <div className="min-w-0 flex-1 py-0.5">
                        <div className="text-xs font-semibold leading-snug line-clamp-2 text-[var(--text-1)] group-hover:text-[var(--accent-hover)] transition-colors">{item.title}</div>
                        <div className="flex items-center gap-1.5 mt-1 text-[10px] text-[var(--text-3)]">
                          {item.genres.length > 0 && <span className="truncate">{item.genres.slice(0, 2).join(' · ')}</span>}
                        </div>
                      </div>
                    </button>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* ═══ COMPACT ACTION BAR ═══ */}
      <div className="px-3 sm:px-5 py-2 space-y-2 shrink-0">
        <div className="flex flex-wrap items-center gap-2">
          {/* Download All */}
          <button onClick={handleDownloadAll}
            disabled={downloadedCount === chapters.length || downloadingCount > 0 || chapters.length === 0}
            className="relative overflow-hidden flex items-center gap-2 px-3 py-2 min-h-[40px] sm:min-h-0 rounded-xl text-xs font-medium transition-all hover:brightness-110 disabled:opacity-40"
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
            {downloadingCount > 0 ? <Loader2 size={13} className="text-amber-400 animate-spin" /> :
             downloadedCount === chapters.length && chapters.length > 0 ? <Check size={13} className="text-[var(--accent)]" /> :
             <Download size={13} className="text-[var(--accent)]" />}
            <span className="text-[var(--text-2)]">
              {downloadedCount === chapters.length && chapters.length > 0 ? 'Downloaded' : downloadingCount > 0 ? 'Downloading…' : 'Download All'}
            </span>
            <span className="text-[var(--text-3)]">{downloadedCount}/{chapters.length}</span>
            {(downloadingCount > 0 || downloadedCount > 0) && (
              <div className="absolute bottom-0 left-0 h-0.5 rounded-full transition-all duration-500" style={{ width: `${chapters.length > 0 ? (downloadedCount / chapters.length) * 100 : 0}%`, background: downloadedCount === chapters.length ? '#e8793f' : '#60A5FA' }} />
            )}
          </button>

          {/* Download next 5 / unread */}
          <button onClick={() => handleDownloadNext(5)} disabled={chapters.every((c) => c.is_read || !isDownloadable(c))}
            title="Download the next 5 undownloaded chapters starting from your first unread"
            className="flex items-center gap-1.5 px-3 py-2 min-h-[40px] sm:min-h-0 rounded-xl text-xs font-medium transition-all hover:brightness-110 disabled:opacity-40"
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
            <Download size={13} className="text-[var(--accent)]" />
            <span className="text-[var(--text-2)]">Next 5</span>
          </button>
          <button onClick={handleDownloadUnread} disabled={chapters.every((c) => c.is_read || !isDownloadable(c))}
            title="Download all unread chapters"
            className="flex items-center gap-1.5 px-3 py-2 min-h-[40px] sm:min-h-0 rounded-xl text-xs font-medium transition-all hover:brightness-110 disabled:opacity-40"
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
            <DownloadCloud size={13} className="text-[var(--accent)]" />
            <span className="text-[var(--text-2)]">All unread</span>
            <span className="text-[var(--text-3)]">{chapters.filter((c) => !c.is_read && isDownloadable(c)).length}</span>
          </button>

          {/* Studio creation actions */}
          {isStudio && <>
          <button onClick={handleOcrAll}
            disabled={downloadedCount === 0 || (ocrCount === downloadedCount && downloadedCount > 0) || ocrRunning > 0}
            className="relative overflow-hidden flex items-center gap-2 px-3 py-2 min-h-[40px] sm:min-h-0 rounded-xl text-xs font-medium transition-all hover:brightness-110 disabled:opacity-40"
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
            {ocrRunning > 0 ? <Loader2 size={13} className="text-[var(--secondary)] animate-spin" /> :
             ocrCount === downloadedCount && downloadedCount > 0 ? <Check size={13} className="text-[var(--accent)]" /> :
             <FileText size={13} className="text-[var(--secondary)]" />}
            <span className="text-[var(--text-2)]">
              {ocrCount === downloadedCount && downloadedCount > 0 ? 'Text Ready' : ocrRunning > 0 ? 'Extracting…' : 'Extract Text'}
            </span>
            <span className="text-[var(--text-3)]">{ocrCount}/{downloadedCount}</span>
            {(ocrRunning > 0 || ocrCount > 0) && downloadedCount > 0 && (
              <div className="absolute bottom-0 left-0 h-0.5 rounded-full transition-all duration-500" style={{ width: `${(ocrCount / downloadedCount) * 100}%`, background: ocrCount === downloadedCount ? '#e8793f' : '#3b82f6' }} />
            )}
          </button>

          {/* Generate Audio */}
          <button onClick={handleGenerateAll}
            disabled={ocrCount === 0 || (ttsCount === ocrCount && ocrCount > 0) || ttsRunning > 0}
            className="relative overflow-hidden flex items-center gap-2 px-3 py-2 min-h-[40px] sm:min-h-0 rounded-xl text-xs font-medium transition-all hover:brightness-110 disabled:opacity-40"
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
            {ttsRunning > 0 ? <Loader2 size={13} className="text-[var(--accent)] animate-spin" /> :
             ttsCount === ocrCount && ocrCount > 0 ? <Check size={13} className="text-[var(--accent)]" /> :
             <Volume2 size={13} className="text-[var(--accent)]" />}
            <span className="text-[var(--text-2)]">
              {ttsCount === ocrCount && ocrCount > 0 ? 'Audio Ready' : ttsRunning > 0 ? 'Generating…' : 'Generate Audio'}
            </span>
            <span className="text-[var(--text-3)]">{ttsCount}/{ocrCount}</span>
            {(ttsRunning > 0 || ttsCount > 0) && ocrCount > 0 && (
              <div className="absolute bottom-0 left-0 h-0.5 rounded-full transition-all duration-500" style={{ width: `${(ttsCount / ocrCount) * 100}%`, background: ttsCount === ocrCount ? '#e8793f' : '#60A5FA' }} />
            )}
          </button>

          {/* Voice selector - inline */}
          <div className="ml-auto flex items-center gap-1.5">
            <select value={selectedEngine} onChange={(e) => handleEngineChange(e.target.value)}
              className="h-7 px-2 rounded-lg text-[11px] text-[var(--text-3)] outline-none"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
              <option value="kokoro">Kokoro</option>
              <option value="chatterbox">Chatterbox</option>
              <option value="xtts">XTTS v2</option>
              <option value="vctk">VCTK</option>
            </select>
            <select value={selectedVoice} onChange={(e) => handleVoiceChange(e.target.value)}
              className="h-7 px-2 rounded-lg text-[11px] text-[var(--text-3)] outline-none"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>
              {selectedEngine === 'kokoro' && <>
                <option value="af_heart">Warm F</option><option value="af_nova">Bright F</option>
                <option value="am_echo">Clear M</option><option value="am_onyx">Deep M</option>
                <option value="bf_emma">UK F</option><option value="bm_george">UK M</option>
              </>}
              {selectedEngine === 'chatterbox' && <option value="default">Default</option>}
              {selectedEngine === 'xtts' && <option value="default">English</option>}
              {selectedEngine === 'vctk' && <>
                <option value="p225">225 F</option><option value="p226">226 M</option>
                <option value="p228">228 F</option><option value="p232">232 M</option>
              </>}
            </select>
          </div>
          </>}
        </div>

        {/* Pipeline status moved to floating widget */}
      </div>

      {/* ═══ SCRIPT EDITOR MODAL ═══ */}
      {isStudio && scriptChapter && (
        <div className="fixed inset-0 z-50 flex items-center justify-center" style={{ background: 'rgba(0,0,0,0.7)', backdropFilter: 'blur(8px)' }}>
          <div className="w-full h-full sm:h-auto sm:w-[700px] sm:max-w-[calc(100vw-32px)] sm:max-h-[85vh] flex flex-col rounded-none sm:rounded-xl overflow-hidden shadow-2xl" style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}>
            {/* Header */}
            <div className="flex flex-wrap items-center justify-between gap-2 px-4 sm:px-6 py-3 sm:py-4 shrink-0" style={{ borderBottom: '1px solid var(--panel-border)' }}>
              <div className="min-w-0">
                <h3 className="text-sm font-semibold text-white">Edit Script</h3>
                <p className="text-xs text-[var(--text-3)]">Chapter {scriptChapter.number ?? '?'} — {scriptSegments.length} pages with text</p>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <button onClick={() => handleExportText(scriptChapter, { stopPropagation: () => {} } as any)}
                  className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs text-[var(--text-3)] hover:text-[var(--text-1)] transition-colors"
                  style={{ background: 'var(--surface-3)' }}>
                  <DownloadCloud size={13} /> Export .txt
                </button>
                {scriptDirty && (
                  <button onClick={handleSaveScript} disabled={scriptSaving}
                    className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs text-white transition-all hover:opacity-90"
                    style={{ background: 'var(--accent-gradient)' }}>
                    {scriptSaving ? <Loader2 size={13} className="animate-spin" /> : <Save size={13} />} Save
                  </button>
                )}
                <button onClick={() => handleGenerateChapter(scriptChapter, { stopPropagation: () => {} } as any)}
                  disabled={scriptSegments.length === 0 || scriptChapter.tts_status === 'running'}
                  className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs text-white transition-all hover:opacity-90 disabled:opacity-40"
                  style={{ background: 'var(--accent-gradient)' }}>
                  <Volume2 size={13} /> Generate Audio
                </button>
                <button onClick={() => { setScriptChapter(null); setScriptSegments([]) }}
                  className="text-[var(--text-3)] hover:text-[var(--text-1)] transition-colors p-1 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center">
                  <X size={16} />
                </button>
              </div>
            </div>

            {/* Script content */}
            <div className="flex-1 overflow-y-auto px-3 sm:px-6 py-4 space-y-3">
              {scriptLoading ? (
                <div className="flex items-center justify-center py-12">
                  <Loader2 size={24} className="text-[var(--accent)] animate-spin" />
                </div>
              ) : scriptSegments.length === 0 ? (
                <div className="text-center py-12 text-[var(--text-3)] text-sm">No text extracted for this chapter.</div>
              ) : (
                scriptSegments.map((seg, idx) => (
                  <div key={idx} className="rounded-xl overflow-hidden" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
                    <div className="flex items-center gap-2 px-4 py-2" style={{ borderBottom: '1px solid var(--panel-border)' }}>
                      <span className="text-[10px] font-mono text-[var(--text-3)]">Page {seg.page + 1}</span>
                    </div>
                    <textarea
                      value={seg.text}
                      onChange={(e) => handleScriptEdit(idx, e.target.value)}
                      rows={Math.max(2, Math.ceil(seg.text.length / 80))}
                      className="w-full px-4 py-3 text-sm text-[var(--text-1)] leading-relaxed resize-none outline-none"
                      style={{ background: 'transparent' }}
                    />
                  </div>
                ))
              )}
            </div>
          </div>
        </div>
      )}

      {/* Source tabs */}
      {sourceTabs.length > 1 && (
        <div className="px-3 sm:px-5 pb-2 shrink-0">
          <div className="flex items-center gap-2 mb-2">
            <span className="text-xs text-[var(--text-3)] uppercase tracking-wider font-medium">Sources</span>
            {findingSources && <Loader2 size={12} className="text-[var(--accent)] animate-spin" />}
          </div>
          <div className="flex flex-wrap gap-2">
            {sourceTabs.map((source) => (
              <button key={source.sourceId} onClick={() => handleSwitchSource(source)}
                className={`px-3.5 py-1.5 min-h-[40px] sm:min-h-0 max-w-full truncate rounded-lg text-xs font-medium transition-all duration-150 ${
                  activeSource?.sourceId === source.sourceId ? 'text-white shadow-sm' : 'text-[var(--text-3)] hover:text-[var(--text-1)]'
                }`}
                style={activeSource?.sourceId === source.sourceId
                  ? { background: 'var(--accent-gradient)' }
                  : { background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }
                }>
                {source.sourceName} <span className="opacity-60">({source.chapterCount})</span>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Chapter list */}
      <div className="lg:flex-1 lg:overflow-y-auto px-3 sm:px-5 pb-6">
        <div className="flex flex-wrap items-center justify-between mb-3 gap-2 sm:gap-3">
          <h3 className="text-lg font-semibold text-[var(--text-1)] shrink-0">Chapters</h3>
          <div className="relative flex-1 min-w-[8rem] sm:max-w-xs">
            <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-3)]" />
            <input type="text" placeholder="Search chapters..." value={chapterSearch} onChange={(e) => setChapterSearch(e.target.value)}
              className="w-full h-10 sm:h-8 pl-8 pr-3 rounded-lg text-xs text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
              style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }} />
          </div>
          <button onClick={() => setSortDesc(!sortDesc)}
            className="text-xs text-[var(--text-3)] hover:text-[var(--text-2)] px-3 py-1.5 min-h-[40px] sm:min-h-0 shrink-0 rounded-lg transition-colors"
            style={{ background: 'var(--surface-3)' }}>
            {sortDesc ? 'Newest first' : 'Oldest first'}
          </button>
        </div>

        {chaptersLoading && chapters.length === 0 && (
          <div className="space-y-2">
            {Array.from({ length: 8 }).map((_, i) => (
              <div key={i} className="skeleton h-14 rounded-xl" style={{ animationDelay: `${i * 50}ms` }} />
            ))}
          </div>
        )}

        {!chaptersLoading && chapters.length === 0 && !syncing && (
          <div className="text-center py-12 text-[var(--text-3)] fade-up">No chapters found.</div>
        )}

        {/* Chapter rows */}
        <div className="space-y-1">
          {sortedChapters.map((ch) => {
            const hasAudio = ch.tts_status === 'complete'
            const hasText = ch.ocr_status === 'complete'

            return (
              <div key={ch.id} onClick={() => handleChapterClick(ch)}
                className={`flex items-center gap-2 sm:gap-3 px-3 sm:px-4 py-2 sm:py-3 min-w-0 rounded-xl transition-all duration-150 cursor-pointer group ${ch.is_read ? 'opacity-50' : ''}`}
                style={{ background: ch.is_read ? 'transparent' : 'var(--surface-2)', border: '1px solid transparent' }}
                onMouseEnter={(e) => { if (!ch.is_read) e.currentTarget.style.borderColor = 'rgba(200, 80, 26,0.15)' }}
                onMouseLeave={(e) => { e.currentTarget.style.borderColor = 'transparent' }}>

                {ch.is_read ? <Check size={14} className="text-[var(--accent)] shrink-0" /> :
                  <div className="w-1.5 h-1.5 rounded-full shrink-0" style={{ background: 'var(--accent)' }} />}

                <span className="text-sm font-mono w-14 sm:w-16 shrink-0 truncate text-[var(--text-3)]">
                  {ch.number !== null ? `Ch. ${ch.number}` : 'Oneshot'}
                </span>
                <span className="flex-1 min-w-0 text-sm truncate text-[var(--text-2)] group-hover:text-[var(--text-1)] transition-colors">
                  {ch.title || `Chapter ${ch.number ?? '?'}`}
                </span>

                {ch.read_progress > 0 && !ch.is_read && (
                  <span className="text-[11px] text-[var(--accent)] font-medium shrink-0">p.{ch.read_progress}</span>
                )}

                {/* Status badges */}
                <div className="flex items-center gap-1.5 shrink-0">
                  {ch.download_status === 'complete' && <span className="text-[10px] text-[var(--accent)]" title="Downloaded"><Download size={11} /></span>}
                  {ch.download_status === 'downloading' && (
                    <span className="text-[10px] text-amber-400 animate-pulse tabular-nums">
                      {downloadProgress[ch.id] && downloadProgress[ch.id].total > 1
                        ? `${Math.round((downloadProgress[ch.id].current / downloadProgress[ch.id].total) * 100)}%`
                        : <Loader2 size={11} className="animate-spin" />}
                    </span>
                  )}
                  {ch.download_status === 'queued' && <span className="text-[10px] text-[var(--secondary)]">Queued</span>}
                  {failedIds.has(ch.id) && isDownloadable(ch) && <span className="text-[10px] text-red-400">Failed</span>}
                  {isStudio && ch.ocr_status === 'complete' && <span className="text-[10px] text-[var(--secondary)]" title="Text extracted"><FileText size={11} /></span>}
                  {isStudio && ch.ocr_status === 'running' && <span className="text-[10px] text-[var(--secondary)] animate-pulse"><Loader2 size={11} className="animate-spin" /></span>}
                  {isStudio && ch.tts_status === 'complete' && <span className="text-[10px] text-[var(--accent)]" title="Audio ready"><Volume2 size={11} /></span>}
                  {isStudio && ch.tts_status === 'running' && <span className="text-[10px] text-[var(--accent)] animate-pulse"><Loader2 size={11} className="animate-spin" /></span>}
                </div>

                {/* Action buttons */}
                {isDownloadable(ch) && (
                  <button onClick={(e) => handleDownload(ch, e)}
                    title={failedIds.has(ch.id) ? 'Retry download' : 'Download chapter'}
                    className={`text-xs px-2 py-1 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center shrink-0 rounded-lg transition-all ${failedIds.has(ch.id) ? 'text-red-400' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'}`}
                    style={{ background: 'var(--surface-3)' }}>
                    {failedIds.has(ch.id) ? <RefreshCw size={14} /> : <Download size={14} />}
                  </button>
                )}
                {(ch.download_status === 'downloading' || ch.download_status === 'queued') && (
                  <button onClick={(e) => handleCancelDownload(ch, e)} title="Cancel download"
                    className="text-xs px-2 py-1 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center shrink-0 rounded-lg text-[var(--text-3)] hover:text-red-400 transition-all"
                    style={{ background: 'var(--surface-3)' }}>
                    <X size={14} />
                  </button>
                )}

                {/* Edit Script button (when OCR done) */}
                {isStudio && hasText && (
                  <button onClick={(e) => handleOpenScript(ch, e)} title="Edit script"
                    className="text-xs px-2 py-1 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center shrink-0 rounded-lg text-[var(--secondary)] hover:text-[var(--text-1)] transition-all"
                    style={{ background: 'rgba(59,130,246,0.08)' }}>
                    <Edit3 size={14} />
                  </button>
                )}

                {/* Export text button */}
                {isStudio && hasText && (
                  <button onClick={(e) => handleExportText(ch, e)} title="Export text"
                    className="text-xs px-2 py-1 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center shrink-0 rounded-lg text-[var(--text-3)] hover:text-[var(--text-2)] transition-all"
                    style={{ background: 'var(--surface-3)' }}>
                    <DownloadCloud size={13} />
                  </button>
                )}

                {/* Generate Audio button (when text ready but no audio) */}
                {isStudio && hasText && !hasAudio && ch.tts_status !== 'running' && (
                  <button onClick={(e) => handleGenerateChapter(ch, e)} title="Generate audio"
                    className="text-xs px-2 py-1 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center shrink-0 rounded-lg text-[var(--accent)] hover:text-[var(--text-1)] transition-all"
                    style={{ background: 'rgba(200, 80, 26,0.08)' }}>
                    <Zap size={14} />
                  </button>
                )}

                {/* Listen button */}
                {isStudio && hasAudio && (
                  <button onClick={(e) => handlePlayChapter(ch, e)} title="Listen"
                    className="text-xs px-2.5 py-1 min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center shrink-0 rounded-lg text-[var(--accent)] hover:text-[var(--text-1)] transition-all"
                    style={{ background: 'rgba(200, 80, 26,0.12)', border: '1px solid rgba(200, 80, 26,0.15)' }}>
                    <Headphones size={14} />
                  </button>
                )}

                {ch.page_count && <span className="hidden sm:inline text-[11px] text-[var(--text-3)] w-10 text-right shrink-0">{ch.page_count}p</span>}
              </div>
            )
          })}
        </div>
      </div>

      {/* ═══ FLOATING PIPELINE STATUS WIDGET ═══ */}
      {isStudio && Object.keys(pipelineStatus).length > 0 && (
        <div className="fixed bottom-4 right-4 z-40 rounded-xl shadow-2xl overflow-hidden max-w-[calc(100vw-32px)]"
          style={{ background: 'var(--surface-1)', border: '1px solid rgba(200, 80, 26,0.15)', width: pipelineCollapsed ? 'auto' : '280px', backdropFilter: 'blur(12px)' }}>
          {/* Header — always visible, click to toggle */}
          <button onClick={() => setPipelineCollapsed(!pipelineCollapsed)}
            className="w-full flex items-center gap-2 px-3 py-2 text-[11px] font-medium text-[var(--text-2)] hover:text-[var(--text-1)] transition-colors"
            style={{ borderBottom: pipelineCollapsed ? 'none' : '1px solid var(--panel-border)' }}>
            <Loader2 size={11} className="text-[var(--accent)] animate-spin shrink-0" />
            <span>Processing {Object.keys(pipelineStatus).length} chapter{Object.keys(pipelineStatus).length > 1 ? 's' : ''}</span>
            <ChevronDown size={12} className={`ml-auto text-[var(--text-3)] transition-transform ${pipelineCollapsed ? '-rotate-90' : ''}`} />
          </button>

          {/* Expandable list */}
          {!pipelineCollapsed && (
            <div className="max-h-32 overflow-y-auto px-3 py-1.5 space-y-1.5">
              {Object.entries(pipelineStatus).map(([chId, status]) => {
                const ch = chapters.find((c) => c.id === chId)
                return (
                  <div key={chId} className="flex items-center gap-2 text-[10px]">
                    <span className="text-[var(--text-3)] w-10 shrink-0 truncate">Ch.{ch?.number ?? '?'}</span>
                    <span className="text-[var(--text-3)] w-12 shrink-0 truncate">{status.stage}</span>
                    <div className="flex-1 h-1 rounded-full overflow-hidden" style={{ background: 'var(--panel-border)' }}>
                      <div className="h-full rounded-full transition-all duration-300" style={{ width: `${status.percent}%`, background: 'var(--accent-gradient)' }} />
                    </div>
                    <span className="text-[var(--text-3)] w-7 text-right">{Math.round(status.percent)}%</span>
                  </div>
                )
              })}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
