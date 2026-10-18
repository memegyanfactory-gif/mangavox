import { useState, useEffect, useRef } from 'react'
import { useLibraryStore } from '../stores/library.store'
import { useUiStore } from '../stores/ui.store'
import { useBrowseStore } from '../stores/browse.store'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { coverUrl } from '../utils/cover-url'
import { Search, AlertCircle, Loader2, ChevronLeft, Shield, ShieldCheck, ShieldOff, Download, RefreshCw, ShieldAlert, WifiOff, ArrowRight } from 'lucide-react'
import type { Chapter, Manga, MangaSearchResult, MangaDetails } from '../../shared/types'
import { toast } from '../components/Toaster'
import { TrendingSpotlight, type TrendingManga } from '../components/manga/TrendingSpotlight'
import { findAndPrepareManga } from '../utils/open-manga'
import { classifySourceError, cleanErrorMessage, type SourceHealth } from '../../shared/source-health'
import { HEALTH_STYLE, healthTooltip, useSourceHealth, baseUrlForSource, openSourceVerification } from '../utils/source-health'

interface SourceInfo {
  id: string
  name: string
  version: string
  lang: string
  icon?: string
}

type BrowseTab = 'popular' | 'latest' | 'genres' | 'search'

// Brand colour per source. Keys match plugin IDs from plugin-loader.ts so the
// card badge auto-themes to whichever plugin a result came from. New plugins
// fall through to a neutral default — no breakage.
const SOURCE_THEME: Record<string, { label: string; bg: string; fg: string; ring: string }> = {
  mangadex:   { label: 'MD',   bg: 'rgba(255,103,30,0.92)',  fg: '#fff', ring: 'rgba(255,103,30,0.35)' },
  comick:     { label: 'CK',   bg: 'rgba(74,144,226,0.92)',  fg: '#fff', ring: 'rgba(74,144,226,0.35)' },
  mangafire:  { label: 'MF',   bg: 'rgba(220,38,38,0.92)',   fg: '#fff', ring: 'rgba(220,38,38,0.35)' },
  webtoons:   { label: 'WT',   bg: 'rgba(0,210,98,0.92)',    fg: '#04231a', ring: 'rgba(0,210,98,0.35)' },
  nhentai:    { label: 'NH',   bg: 'rgba(236,72,153,0.92)',  fg: '#fff', ring: 'rgba(236,72,153,0.35)' },
  ehentai:    { label: 'EH',   bg: 'rgba(120,53,15,0.92)',   fg: '#fff', ring: 'rgba(180,83,9,0.35)' },
  hentai2read:{ label: 'H2R',  bg: 'rgba(168,85,247,0.92)',  fg: '#fff', ring: 'rgba(168,85,247,0.35)' },
}
function themeFor(id: string | undefined, name: string | undefined) {
  if (id && SOURCE_THEME[id]) return SOURCE_THEME[id]
  const label = (name || id || '?').slice(0, 3).toUpperCase()
  return { label, bg: 'rgba(82,82,91,0.88)', fg: '#fff', ring: 'rgba(161,161,170,0.3)' }
}

export function Browse() {
  const [sources, setSources] = useState<SourceInfo[]>([])
  const [loading, setLoading] = useState(false)
  const [addingIds, setAddingIds] = useState<Set<string>>(new Set())
  const [downloadingIds, setDownloadingIds] = useState<Set<string>>(new Set())

  // Persistent browse state — survives navigation
  const {
    selectedSource, setSelectedSource,
    activeTab, setActiveTab,
    query, setQuery,
    results, setResults,
    tags, setTags,
    selectedTag, setSelectedTag,
    page, setPage,
    errorMsg, setErrorMsg,
  } = useBrowseStore()

  const { addToLibrary } = useLibraryStore()
  const { navigate } = useUiStore()
  const [openingTrendingId, setOpeningTrendingId] = useState<string | null>(null)

  // Source health (status dots + error panel) and "retry last action" support
  const { health, recheck, setOne: setHealthOne } = useSourceHealth()
  const lastActionRef = useRef<(() => void) | null>(null)
  const [reloadKey, setReloadKey] = useState(0)
  const [verifying, setVerifying] = useState(false)

  /** Record a failure: keep the raw message (classified at render) and update the dot. */
  const reportError = (sourceId: string, err: unknown, prefix = '') => {
    const msg = cleanErrorMessage(err)
    setErrorMsg(prefix + msg)
    const c = classifySourceError(msg)
    const prev = health[sourceId]
    setHealthOne({
      id: sourceId,
      name: prev?.name || sources.find((s) => s.id === sourceId)?.name || sourceId,
      status: c.status,
      latencyMs: prev?.latencyMs ?? null,
      checkedAt: Date.now(),
      message: c.message,
      baseUrl: prev?.baseUrl,
    })
  }

  const openTrending = async (m: TrendingManga) => {
    if (openingTrendingId) return
    setOpeningTrendingId(m.malId)
    try {
      const found = await findAndPrepareManga(m.title, addToLibrary, { preferredSource: selectedSource, coverFallback: m.coverUrl })
      if (!found) { toast.error('Not available', `${m.title} wasn't found on your installed sources.`); return }
      navigate('manga-detail', { mangaId: found.manga.id, sourceId: found.sourceId })
    } catch (err: any) {
      toast.error('Could not open', err?.message || m.title)
    } finally {
      setOpeningTrendingId(null)
    }
  }

  // Privacy mode — free proxy
  const [privacyEnabled, setPrivacyEnabled] = useState(false)
  const [privacyLoading, setPrivacyLoading] = useState(false)
  const [privacyError, setPrivacyError] = useState<string | null>(null)
  const [proxyAddr, setProxyAddr] = useState<string | null>(null)
  const [myIp, setMyIp] = useState<string | null>(null)
  const [showPrivacyPanel, setShowPrivacyPanel] = useState(false)
  const privacyRef = useRef<HTMLDivElement>(null)

  // Load initial status once
  useEffect(() => {
    invoke<{ enabled: boolean; proxyAddr: string | null }>(IPC.PRIVACY_STATUS)
      .then((s) => { setPrivacyEnabled(s.enabled); setProxyAddr(s.proxyAddr) })
      .catch(() => {})
  }, [])

  // Close panel on outside click
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (privacyRef.current && !privacyRef.current.contains(e.target as Node))
        setShowPrivacyPanel(false)
    }
    if (showPrivacyPanel) document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [showPrivacyPanel])

  const togglePrivacy = async () => {
    setPrivacyLoading(true)
    setPrivacyError(null)
    setMyIp(null)
    try {
      const res = await invoke<{ ok: boolean; proxy?: string; error?: string }>(
        IPC.PRIVACY_TOGGLE, !privacyEnabled
      )
      if (res.ok) {
        setPrivacyEnabled(!privacyEnabled)
        setProxyAddr(res.proxy || null)
      } else {
        setPrivacyError(res.error || 'Failed to find a working proxy')
      }
    } catch (e: any) {
      setPrivacyError(e?.message || 'Failed')
    } finally {
      setPrivacyLoading(false)
    }
  }

  const checkIp = async () => {
    setMyIp('checking...')
    try {
      const { ip } = await invoke<{ ip: string }>(IPC.PRIVACY_CHECK_IP)
      setMyIp(ip)
    } catch {
      setMyIp('unknown')
    }
  }

  // Load source list once; restore previously selected source if it still exists
  useEffect(() => {
    invoke<SourceInfo[]>(IPC.PLUGIN_LIST_SOURCES).then((s) => {
      setSources(s)
      if (s.length > 0 && (!selectedSource || !s.find((src) => src.id === selectedSource))) {
        setSelectedSource(s[0].id)
      }
    })
  }, [])

  // Reload content when source or tab changes — but NOT if we already have cached results
  useEffect(() => {
    if (!selectedSource) return
    // Already have results for this source+tab combo — don't re-fetch
    if (results.length > 0 || (activeTab === 'genres' && tags.length > 0)) return

    let cancelled = false
    lastActionRef.current = () => {
      setResults([])
      setTags([])
      setReloadKey((k) => k + 1)
    }
    const load = async () => {
      setLoading(true)
      setErrorMsg(null)
      try {
        if (activeTab === 'popular') {
          const data = await invoke<MangaSearchResult[]>(IPC.PLUGIN_GET_POPULAR, selectedSource, 1)
          if (!cancelled) setResults(data)
        } else if (activeTab === 'latest') {
          const data = await invoke<MangaSearchResult[]>(IPC.PLUGIN_GET_LATEST, selectedSource, 1)
          if (!cancelled) setResults(data)
        } else if (activeTab === 'genres') {
          const data = await invoke<{ id: string; name: string }[]>(IPC.PLUGIN_GET_TAGS, selectedSource)
          if (!cancelled) setTags(data)
        }
      } catch (err: any) {
        console.warn('Browse load error:', err)
        if (!cancelled) reportError(selectedSource, err)
      } finally {
        if (!cancelled) setLoading(false)
      }
    }
    load()
    return () => { cancelled = true }
  }, [selectedSource, activeTab, reloadKey])

  // When user explicitly switches source — clear cache and refetch
  const handleSourceChange = (sourceId: string) => {
    setSelectedSource(sourceId)
    setResults([])
    setTags([])
    setSelectedTag(null)
    setPage(1)
    setErrorMsg(null)
  }

  // When user explicitly switches tab — clear results so useEffect refetches
  const handleTabChange = (tab: BrowseTab) => {
    if (tab === activeTab) return
    setActiveTab(tab)
    setResults([])
    setTags([])
    setSelectedTag(null)
    setPage(1)
    setErrorMsg(null)
  }

  const fetchPage = async (source: string, tab: BrowseTab, p: number, tagId?: string) => {
    if (tab === 'popular') return invoke<MangaSearchResult[]>(IPC.PLUGIN_GET_POPULAR, source, p)
    if (tab === 'latest') return invoke<MangaSearchResult[]>(IPC.PLUGIN_GET_LATEST, source, p)
    if (tab === 'genres' && tagId) return invoke<MangaSearchResult[]>(IPC.PLUGIN_GET_BY_TAG, source, tagId, p)
    if (tab === 'search') return invoke<MangaSearchResult[]>(IPC.PLUGIN_SEARCH, source, query.trim(), p)
    return []
  }

  const runSearch = async (q: string, sourceId: string) => {
    if (!q || !sourceId) return
    lastActionRef.current = () => { runSearch(q, sourceId) }
    setActiveTab('search')
    setLoading(true)
    setPage(1)
    setResults([])
    setErrorMsg(null)
    try {
      const data = await invoke<MangaSearchResult[]>(IPC.PLUGIN_SEARCH, sourceId, q, 1)
      setResults(data)
    } catch (err: any) {
      console.warn('Search error:', err)
      reportError(sourceId, err)
    } finally {
      setLoading(false)
    }
  }

  const handleSearch = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!query.trim() || !selectedSource) return
    await runSearch(query.trim(), selectedSource)
  }

  // "Try another source": switch and repeat what the user was doing there.
  const trySource = (sourceId: string) => {
    if (activeTab === 'search' && query.trim()) {
      setSelectedSource(sourceId)
      setTags([])
      setSelectedTag(null)
      runSearch(query.trim(), sourceId)
    } else {
      handleSourceChange(sourceId)
    }
  }

  const retryLast = () => {
    setErrorMsg(null)
    const fn = lastActionRef.current
    if (fn) fn()
    else { setResults([]); setTags([]); setReloadKey((k) => k + 1) }
  }

  const verifyAccess = async () => {
    const src = sources.find((s) => s.id === selectedSource)
    setVerifying(true)
    try {
      const ok = await openSourceVerification(baseUrlForSource(selectedSource, health[selectedSource]), src?.name)
      if (ok) {
        recheck(selectedSource)
        retryLast()
      }
    } finally {
      setVerifying(false)
    }
  }

  const handleLoadMore = async () => {
    const nextPage = page + 1
    setPage(nextPage)
    setLoading(true)
    try {
      const data = await fetchPage(selectedSource, activeTab, nextPage, selectedTag?.id)
      setResults([...results, ...(data || [])])
    } catch (err: any) {
      console.error('Load more failed:', err)
    } finally {
      setLoading(false)
    }
  }

  const handleSelectTag = async (tag: { id: string; name: string }) => {
    lastActionRef.current = () => { handleSelectTag(tag) }
    setSelectedTag(tag)
    setResults([])
    setPage(1)
    setErrorMsg(null)
    setLoading(true)
    try {
      const data = await invoke<MangaSearchResult[]>(IPC.PLUGIN_GET_BY_TAG, selectedSource, tag.id, 1)
      setResults(data)
    } catch (err) {
      console.warn('Failed to load by tag:', err)
      reportError(selectedSource, err)
    } finally {
      setLoading(false)
    }
  }

  const prepareManga = async (result: MangaSearchResult): Promise<Manga> => {
    const details = await invoke<MangaDetails>(IPC.PLUGIN_GET_DETAILS, selectedSource, result.source_id)
    const manga = await addToLibrary(selectedSource, {
      ...result,
      author: details.author ?? result.author,
      cover_url: details.cover_url ?? result.cover_url,
    })
    await invoke(IPC.CHAPTER_SYNC, manga.id, selectedSource, result.source_id)
    await invoke(IPC.LIBRARY_UPDATE_DETAILS, {
      id: manga.id, author: details.author, artist: details.artist,
      description: details.description, cover_path: details.cover_url,
      status: details.status, genres: details.genres,
    })
    return manga
  }

  const handleMangaClick = async (result: MangaSearchResult) => {
    setAddingIds((prev) => new Set(prev).add(result.source_id))
    try {
      const manga = await prepareManga(result)
      navigate('manga-detail', { mangaId: manga.id, sourceId: selectedSource })
    } catch (err: any) {
      console.warn('Failed to open manga:', err)
      lastActionRef.current = () => { handleMangaClick(result) }
      reportError(selectedSource, err, `Opening "${result.title}" failed: `)
    } finally {
      setAddingIds((prev) => { const next = new Set(prev); next.delete(result.source_id); return next })
    }
  }

  const handleQuickDownload = async (result: MangaSearchResult, event: React.MouseEvent) => {
    event.stopPropagation()
    if (downloadingIds.has(result.source_id)) return
    setDownloadingIds((prev) => new Set(prev).add(result.source_id))
    try {
      const manga = await prepareManga(result)
      const chapters = await invoke<Chapter[]>(IPC.CHAPTER_LIST_BY_SOURCE, manga.id, selectedSource)
      const downloadable = [...chapters]
        .filter((item) => item.download_status !== 'complete' && item.download_status !== 'queued' && item.download_status !== 'downloading')
        .sort((a, b) => (a.number ?? Number.MAX_SAFE_INTEGER) - (b.number ?? Number.MAX_SAFE_INTEGER))
      if (!chapters.length) {
        toast.error('No chapters found', `${result.title} returned no chapters from this source. Try another source.`)
        return
      }
      if (!downloadable.length) {
        toast.info('Already downloaded', `${result.title} has no new downloadable chapters.`)
        return
      }
      for (const chapter of downloadable) {
        await invoke(IPC.DOWNLOAD_CHAPTER, manga.id, chapter.id, chapter.source_plugin || selectedSource, chapter.source_id)
      }
      toast.success('Series download queued', `${downloadable.length} chapters of ${result.title}`)
    } catch (error: any) {
      toast.error('Download failed', error?.message || `Could not download ${result.title}`)
    } finally {
      setDownloadingIds((prev) => {
        const next = new Set(prev)
        next.delete(result.source_id)
        return next
      })
    }
  }

  const tabs: { id: BrowseTab; label: string }[] = [
    { id: 'popular', label: 'Popular' },
    { id: 'latest', label: 'Latest' },
    { id: 'genres', label: 'Genres' },
  ]

  return (
    <div className="flex flex-col h-full page-enter" style={{ background: 'var(--base)' }}>
      {/* Header */}
      <div className="px-3 sm:px-5 pt-3 pb-1 flex items-end justify-between gap-2">
        <h2
          className="text-xl font-bold tracking-tight"
          style={{ background: 'linear-gradient(180deg, var(--text-1) 0%, var(--text-2) 100%)', WebkitBackgroundClip: 'text', WebkitTextFillColor: 'transparent' }}
        >
          Manga
        </h2>

        {/* Privacy button */}
        <div className="relative pb-1" ref={privacyRef} style={{ WebkitAppRegion: 'no-drag' } as any}>
          <button
            onClick={() => setShowPrivacyPanel((v) => !v)}
            className="flex items-center gap-1.5 px-3 min-h-[40px] sm:min-h-0 sm:py-1.5 rounded-lg text-xs font-medium transition-all"
            style={privacyEnabled
              ? { background: 'rgba(34,197,94,0.12)', border: '1px solid rgba(34,197,94,0.25)', color: '#4ade80' }
              : { background: 'var(--surface-2)', border: '1px solid var(--panel-border)', color: '#71717a' }
            }
            title="Privacy mode"
          >
            {privacyEnabled
              ? <ShieldCheck size={13} />
              : <Shield size={13} />
            }
            {privacyEnabled ? 'Protected' : 'Privacy'}
          </button>

          {/* Dropdown panel */}
          {showPrivacyPanel && (
            <div
              className="absolute right-0 top-full mt-2 rounded-xl p-4 shadow-2xl z-50 w-72 max-w-[calc(100vw-24px)]"
              style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}
            >
              <div className="flex items-center gap-2 mb-3">
                {privacyEnabled
                  ? <ShieldCheck size={16} className="text-green-400" />
                  : <ShieldOff size={16} className="text-[var(--text-3)]" />
                }
                <span className="text-sm font-semibold text-[var(--text-1)]">Privacy Mode</span>
              </div>

              <p className="text-[11px] text-[var(--text-3)] mb-3 leading-relaxed">
                Routes all manga requests through a <span className="text-[var(--accent)] font-medium">free anonymous proxy</span>,
                hiding your real IP. No installation needed — finds a working one automatically.
              </p>

              {privacyEnabled && proxyAddr && (
                <div className="mb-3 px-3 py-2 rounded-lg text-[11px] text-green-300 flex items-center gap-2" style={{ background: 'rgba(34,197,94,0.08)', border: '1px solid rgba(34,197,94,0.15)' }}>
                  <ShieldCheck size={11} className="shrink-0" />
                  <span>Connected via <span className="font-mono">{proxyAddr}</span></span>
                </div>
              )}

              {privacyError && (
                <div className="mb-3 px-3 py-2 rounded-lg text-[11px] text-[var(--accent)]" style={{ background: 'rgba(239,68,68,0.08)', border: '1px solid rgba(239,68,68,0.15)' }}>
                  {privacyError}
                </div>
              )}

              <button
                onClick={togglePrivacy}
                disabled={privacyLoading}
                className="w-full flex items-center justify-center gap-2 py-2 rounded-lg text-sm font-medium transition-all disabled:opacity-50"
                style={privacyEnabled
                  ? { background: 'rgba(239,68,68,0.1)', border: '1px solid rgba(239,68,68,0.2)', color: '#f87171' }
                  : { background: 'rgba(34,197,94,0.1)', border: '1px solid rgba(34,197,94,0.2)', color: '#4ade80' }
                }
              >
                {privacyLoading
                  ? <Loader2 size={13} className="animate-spin" />
                  : privacyEnabled ? <ShieldOff size={13} /> : <ShieldCheck size={13} />
                }
                {privacyLoading ? 'Finding proxy...' : privacyEnabled ? 'Disable' : 'Enable Privacy'}
              </button>

              {/* IP checker */}
              <div className="mt-3 pt-3" style={{ borderTop: '1px solid var(--panel-border)' }}>
                <div className="flex items-center justify-between">
                  <span className="text-[11px] text-[var(--text-3)]">Your IP</span>
                  <button
                    onClick={checkIp}
                    className="text-[11px] text-[var(--accent)] hover:text-[var(--accent)] transition-colors"
                  >
                    Check
                  </button>
                </div>
                {myIp && (
                  <p className="mt-1 text-xs font-mono text-[var(--text-2)]">{myIp}</p>
                )}
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Source chips + Search */}
      <div className="px-3 sm:px-5 pt-2 pb-2 flex flex-col gap-2 min-w-0">
        {/* Source chip bar */}
        <div className="flex gap-2 overflow-x-auto pb-1 scrollbar-none">
          {sources.map((s) => {
            const h = health[s.id]
            const hs = HEALTH_STYLE[h?.status ?? 'unknown']
            return (
            <button
              key={s.id}
              onClick={() => handleSourceChange(s.id)}
              title={`${s.name} — ${healthTooltip(h)}`}
              className={`flex items-center gap-2 px-4 py-2 rounded-full text-sm font-medium whitespace-nowrap transition-all duration-150 shrink-0 ${
                selectedSource === s.id
                  ? 'text-white shadow-lg'
                  : 'text-[var(--text-3)] hover:text-[var(--text-1)]'
              }`}
              style={selectedSource === s.id
                ? { background: 'var(--accent-gradient)', boxShadow: '0 4px 12px rgba(200, 80, 26,0.25)' }
                : { background: 'var(--surface-2)' }
              }
            >
              {s.icon ? (
                <img src={s.icon} alt="" className="w-4 h-4 rounded-sm object-contain" referrerPolicy="no-referrer" />
              ) : (
                <span className="w-4 h-4 rounded-sm flex items-center justify-center text-[10px] font-bold" style={{ background: 'var(--surface-3)' }}>
                  {s.name.charAt(0)}
                </span>
              )}
              {s.name}
              <span
                aria-label={hs.label}
                className={`w-2 h-2 rounded-full shrink-0 ${h?.checking ? 'animate-pulse' : ''}`}
                style={{
                  background: hs.color,
                  opacity: h && h.status !== 'unknown' ? 1 : 0.45,
                  boxShadow: `0 0 0 2px ${selectedSource === s.id ? 'rgba(255,255,255,0.35)' : 'var(--surface-3)'}`,
                }}
              />
            </button>
            )
          })}
          {sources.length === 0 && (
            <span className="text-[var(--text-3)] text-sm py-2">
              No sources installed.{' '}
              <button type="button" onClick={() => navigate('settings', { tab: 'sources' })} className="underline" style={{ color: 'var(--accent)' }}>
                Add a source
              </button>
            </span>
          )}
        </div>

        {/* Search bar */}
        <form onSubmit={handleSearch} className="flex gap-2">
          <div className="relative flex-1 min-w-0">
            <Search size={16} className="absolute left-3.5 top-1/2 -translate-y-1/2 text-[var(--text-3)]" />
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search manga..."
              className="w-full h-11 pl-10 pr-4 rounded-xl text-sm text-[var(--text-1)] placeholder-[var(--text-3)] outline-none transition-all duration-150"
              style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
              onFocus={(e) => {
                e.currentTarget.style.borderColor = 'rgba(200, 80, 26,0.3)'
                e.currentTarget.style.boxShadow = '0 0 0 3px rgba(200, 80, 26,0.08)'
              }}
              onBlur={(e) => {
                e.currentTarget.style.borderColor = 'var(--panel-border)'
                e.currentTarget.style.boxShadow = 'none'
              }}
            />
          </div>
          <button
            type="submit"
            disabled={loading || !query.trim()}
            className="h-11 px-4 sm:px-5 shrink-0 rounded-xl text-sm font-medium text-white disabled:opacity-40 transition-all duration-150 hover:opacity-90"
            style={{ background: 'var(--accent-gradient)' }}
          >
            Search
          </button>
        </form>
      </div>

      {/* Tab bar - segmented control */}
      <div className="px-3 sm:px-5 pb-3">
        <div className="inline-flex max-w-full overflow-x-auto scrollbar-none rounded-xl p-1" style={{ background: 'var(--surface-2)' }}>
          {tabs.map((t) => (
            <button
              key={t.id}
              onClick={() => handleTabChange(t.id)}
              className={`px-3.5 sm:px-5 py-2 min-h-[40px] sm:min-h-0 shrink-0 rounded-lg text-sm font-medium transition-all duration-200 ${
                activeTab === t.id
                  ? 'text-white shadow-sm'
                  : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
              }`}
              style={activeTab === t.id ? { background: 'var(--accent-gradient)' } : {}}
            >
              {t.label}
            </button>
          ))}
          {activeTab === 'search' && (
            <span className="px-3.5 sm:px-5 py-2 shrink-0 rounded-lg text-sm font-medium text-white" style={{ background: 'var(--accent-gradient)' }}>
              Results
            </span>
          )}
        </div>
      </div>

      {/* Genre tag selector */}
      {activeTab === 'genres' && !selectedTag && (
        <div className="px-3 sm:px-5 pb-3">
          {loading ? (
            <div className="flex items-center gap-2 py-8 justify-center text-[var(--text-3)]">
              <Loader2 size={16} className="animate-spin" /> Loading genres...
            </div>
          ) : (
            <div className="flex flex-wrap gap-2">
              {tags.map((tag) => (
                <button
                  key={tag.id}
                  onClick={() => handleSelectTag(tag)}
                  className="px-3.5 py-2 min-h-[40px] rounded-full text-sm text-[var(--text-3)] hover:text-[var(--accent)] transition-all duration-150"
                  style={{ background: 'var(--surface-3)' }}
                >
                  {tag.name}
                </button>
              ))}
              {tags.length === 0 && (
                <p className="text-[var(--text-3)] text-sm">This source doesn't support tag browsing.</p>
              )}
            </div>
          )}
        </div>
      )}

      {/* Selected tag breadcrumb */}
      {activeTab === 'genres' && selectedTag && (
        <div className="px-3 sm:px-5 pb-2 flex items-center gap-2 min-w-0">
          <button
            onClick={() => { setSelectedTag(null); setResults([]) }}
            className="flex items-center gap-1 text-[var(--text-3)] hover:text-[var(--text-2)] text-sm transition-colors"
          >
            <ChevronLeft size={14} /> Genres
          </button>
          <span className="text-[var(--text-3)]">/</span>
          <span className="text-[var(--text-1)] text-sm font-medium truncate">{selectedTag.name}</span>
        </div>
      )}

      {/* Results */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden px-3 sm:px-5 pb-6">
        {activeTab !== 'search' && !selectedTag && !['nhentai', 'ehentai', 'hentai2read'].includes(selectedSource) && <TrendingSpotlight onOpen={openTrending} openingId={openingTrendingId} />}

        {/* Loading skeletons */}
        {loading && results.length === 0 && activeTab !== 'genres' && (
          <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 2xl:grid-cols-7 gap-3 sm:gap-5">
            {Array.from({ length: 12 }).map((_, i) => (
              <div key={i} className="fade-up" style={{ animationDelay: `${i * 30}ms` }}>
                <div className="aspect-[2/3] skeleton rounded-xl" />
                <div className="mt-2.5 skeleton h-4 w-3/4 rounded-md" />
              </div>
            ))}
          </div>
        )}

        {/* Error */}
        {!loading && errorMsg && (
          <SourceErrorPanel
            sourceName={sources.find((s) => s.id === selectedSource)?.name || selectedSource}
            error={errorMsg}
            health={health[selectedSource]}
            verifying={verifying}
            onRetry={retryLast}
            onVerify={verifyAccess}
            alternatives={sources
              .filter((s) => s.id !== selectedSource && (health[s.id]?.status === 'ok' || health[s.id]?.status === 'degraded'))
              .sort((a, b) => (health[a.id]?.status === 'ok' ? 0 : 1) - (health[b.id]?.status === 'ok' ? 0 : 1))
              .slice(0, 4)
              .map((s) => ({ id: s.id, name: s.name, health: health[s.id] }))}
            onTrySource={trySource}
            searching={activeTab === 'search' && !!query.trim()}
          />
        )}

        {/* Empty states */}
        {!loading && !errorMsg && results.length === 0 && activeTab === 'search' && (
          <div className="text-center py-12 text-[var(--text-3)] fade-up">No results found. Try a different search term.</div>
        )}
        {!loading && !errorMsg && results.length === 0 && activeTab !== 'genres' && activeTab !== 'search' && (
          <div className="flex flex-col items-center py-12 fade-up">
            <div className="w-16 h-16 rounded-xl flex items-center justify-center mb-4" style={{ background: 'var(--surface-2)' }}>
              <Search size={24} className="text-[var(--text-3)]" />
            </div>
            <p className="text-[var(--text-3)] text-sm font-medium mb-1">This source doesn't support browsing</p>
            <p className="text-[var(--text-3)] text-xs">Use the search bar above to find manga</p>
          </div>
        )}

        {/* Results grid */}
        {(results.length > 0 || (activeTab === 'genres' && selectedTag)) && (
          <>
            <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 2xl:grid-cols-7 gap-3 sm:gap-5">
              {results.map((result, i) => {
                const currentSource = sources.find((s) => s.id === selectedSource)
                const theme = themeFor(currentSource?.id, currentSource?.name)
                return (
                <div
                  key={`${result.source_id}-${i}`}
                  className="group cursor-pointer fade-up"
                  style={{ animationDelay: `${Math.min(i, 20) * 30}ms` }}
                  onClick={() => handleMangaClick(result)}
                >
                  <div
                    className="aspect-[2/3] rounded-xl overflow-hidden relative transition-all duration-300 group-hover:-translate-y-1 group-hover:ring-1 group-hover:ring-[rgba(var(--accent-rgb),0.5)] group-hover:shadow-2xl group-hover:shadow-black/50"
                    style={{ background: 'var(--surface-2)' }}
                  >
                    {result.cover_url ? (
                      <img
                        src={coverUrl(result.cover_url)!}
                        alt={result.title}
                        className="w-full h-full object-cover"
                        loading="lazy"
                        referrerPolicy="no-referrer"
                      />
                    ) : (
                      <div className="w-full h-full flex items-center justify-center text-[var(--text-3)] text-4xl font-bold">
                        {result.title.charAt(0)}
                      </div>
                    )}

                    <div className="pointer-events-none absolute inset-0 bg-gradient-to-t from-black/85 via-black/5 to-black/20 opacity-30 group-hover:opacity-100 transition-opacity duration-300" />

                    {/* Status (top-left). */}
                    {result.status && (
                      <div className="absolute top-2 left-2 z-10">
                        <span className={`inline-flex items-center gap-1 text-[10px] font-semibold px-1.5 py-0.5 rounded-md backdrop-blur-md uppercase tracking-wide ${
                          result.status === 'ongoing' ? 'bg-[rgba(var(--accent-rgb),0.15)] text-[var(--accent)] ring-1 ring-[rgba(var(--accent-rgb),0.3)]'
                            : result.status === 'completed' ? 'bg-[rgba(var(--secondary-rgb),0.15)] text-[var(--secondary)] ring-1 ring-[rgba(var(--secondary-rgb),0.3)]'
                              : result.status === 'hiatus' ? 'bg-amber-500/25 text-amber-200 ring-1 ring-amber-400/30'
                              : 'bg-[var(--surface-3)] text-[var(--text-1)] ring-1 ring-white/10'
                        }`}>
                          {result.status === 'ongoing' && <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)] status-pulse" />}
                          {result.status}
                        </span>
                      </div>
                    )}

                    {/* Source badge (top-right) — themed per plugin. */}
                    {currentSource && (
                      <div className="absolute top-2 right-2 z-10">
                        <span
                          className="inline-flex items-center gap-1 text-[10px] font-bold px-1.5 py-0.5 rounded-md backdrop-blur-md ring-1 uppercase tracking-wider shadow-sm"
                          style={{ background: theme.bg, color: theme.fg, boxShadow: `0 0 0 1px ${theme.ring}` }}
                          title={currentSource.name}
                        >
                          {currentSource.icon ? (
                            <img src={currentSource.icon} alt="" className="w-3 h-3 rounded-sm object-contain" referrerPolicy="no-referrer" />
                          ) : null}
                          {theme.label}
                        </span>
                      </div>
                    )}

                    <div className="absolute inset-x-0 bottom-0 p-3 z-10 translate-y-3 opacity-0 group-hover:translate-y-0 group-hover:opacity-100 transition-all duration-300">
                      <button
                        type="button"
                        onClick={(event) => handleQuickDownload(result, event)}
                        className="w-full h-9 rounded-xl flex items-center justify-center gap-1.5 text-[11px] font-bold text-white bg-[var(--accent)] hover:bg-[var(--accent-hover)] transition-colors shadow-lg"
                        title={`Download the latest available chapter of ${result.title}`}
                      >
                        {downloadingIds.has(result.source_id)
                          ? <Loader2 size={12} className="animate-spin" />
                          : <Download size={12} />}
                        Download all
                      </button>
                    </div>

                    {/* Adding overlay (sits above everything). */}
                    {addingIds.has(result.source_id) && (
                      <div className="absolute inset-0 glass flex flex-col items-center justify-center gap-2 z-20">
                        <Loader2 size={20} className="text-[var(--accent)] animate-spin" />
                        <span className="text-white text-xs font-medium">Adding...</span>
                      </div>
                    )}
                  </div>
                  <div className="pt-2.5 px-0.5">
                    <h3 className="text-[var(--text-1)] text-[13px] font-semibold leading-snug line-clamp-2 group-hover:text-[var(--accent)] transition-colors">
                      {result.title}
                    </h3>
                    <p className="text-[var(--text-3)] text-[11px] mt-1 truncate">
                      {result.author || currentSource?.name || 'Manga'}
                    </p>
                  </div>
                </div>
                )
              })}
            </div>

            {/* Load more */}
            {results.length >= 20 && (
              <div className="flex justify-center mt-8">
                <button
                  onClick={handleLoadMore}
                  disabled={loading}
                  className="px-8 py-2.5 rounded-xl text-sm font-medium text-[var(--text-2)] transition-all duration-150 hover:text-[var(--text-1)] disabled:opacity-40"
                  style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                >
                  {loading ? (
                    <span className="flex items-center gap-2"><Loader2 size={14} className="animate-spin" /> Loading...</span>
                  ) : 'Load More'}
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  )
}

// ── Error panel ─────────────────────────────────────────────

const ERROR_TITLE: Record<string, string> = {
  verify: 'needs a quick browser check',
  down: "isn't responding",
  degraded: 'is having trouble',
}

function SourceErrorPanel({
  sourceName, error, health, verifying, onRetry, onVerify, alternatives, onTrySource, searching,
}: {
  sourceName: string
  error: string
  health?: SourceHealth
  verifying: boolean
  onRetry: () => void
  onVerify: () => void
  alternatives: { id: string; name: string; health?: SourceHealth }[]
  onTrySource: (id: string) => void
  searching: boolean
}) {
  const c = classifySourceError(error)
  const needsVerify = c.status === 'verify' || health?.status === 'verify'
  const tone = needsVerify ? '#3b82f6' : c.status === 'degraded' ? 'var(--warning, #f59e0b)' : 'var(--error, #ef4444)'
  const Icon = needsVerify ? ShieldAlert : c.status === 'down' ? WifiOff : AlertCircle

  return (
    <div className="flex justify-center py-10 fade-up">
      <div
        className="w-full max-w-xl rounded-2xl p-4 sm:p-5"
        style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)', boxShadow: '0 10px 30px rgba(0,0,0,0.25)' }}
        role="alert"
      >
        <div className="flex items-start gap-3">
          <div
            className="w-10 h-10 rounded-xl flex items-center justify-center shrink-0"
            style={{ background: `color-mix(in srgb, ${tone} 14%, transparent)`, color: tone }}
          >
            <Icon size={18} />
          </div>
          <div className="min-w-0 flex-1">
            <p className="text-sm font-semibold text-[var(--text-1)]">
              {sourceName} {ERROR_TITLE[needsVerify ? 'verify' : c.status] || 'failed to load'}
            </p>
            <p className="text-[13px] text-[var(--text-2)] mt-1 leading-relaxed">
              {needsVerify && c.status !== 'verify' ? health?.message || c.message : c.message}
            </p>
            {c.detail && (
              <details className="mt-2">
                <summary className="text-[11px] text-[var(--text-3)] cursor-pointer select-none">Technical details</summary>
                <p className="text-[11px] font-mono text-[var(--text-3)] mt-1 break-words">{c.detail.slice(0, 400)}</p>
              </details>
            )}
          </div>
        </div>

        <div className="flex flex-wrap gap-2 mt-4">
          <button
            type="button"
            onClick={onRetry}
            className="flex items-center gap-1.5 h-9 px-4 rounded-xl text-xs font-semibold text-white hover:opacity-90 transition-opacity"
            style={{ background: 'var(--accent-gradient, var(--accent))' }}
          >
            <RefreshCw size={13} /> Retry
          </button>
          {needsVerify && (
            <button
              type="button"
              onClick={onVerify}
              disabled={verifying}
              className="flex items-center gap-1.5 h-9 px-4 rounded-xl text-xs font-semibold transition-colors disabled:opacity-50"
              style={{ background: 'rgba(59,130,246,0.12)', border: '1px solid rgba(59,130,246,0.35)', color: '#93c5fd' }}
              title="Opens the site in a window so you can pass the check, then retries"
            >
              {verifying ? <Loader2 size={13} className="animate-spin" /> : <ShieldCheck size={13} />}
              {verifying ? 'Waiting for verification…' : 'Verify access'}
            </button>
          )}
        </div>

        {alternatives.length > 0 && (
          <div className="mt-4 pt-4" style={{ borderTop: '1px solid var(--panel-border)' }}>
            <p className="text-[11px] uppercase tracking-wider font-semibold text-[var(--text-3)] mb-2">
              {searching ? 'Search another source' : 'Try another source'}
            </p>
            <div className="flex flex-wrap gap-2">
              {alternatives.map((a) => (
                <button
                  key={a.id}
                  type="button"
                  onClick={() => onTrySource(a.id)}
                  title={healthTooltip(a.health)}
                  className="flex items-center gap-2 h-8 px-3 rounded-full text-xs font-medium text-[var(--text-2)] hover:text-[var(--text-1)] transition-colors"
                  style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                >
                  <span className="w-2 h-2 rounded-full" style={{ background: HEALTH_STYLE[a.health?.status ?? 'unknown'].color }} />
                  {a.name}
                  <ArrowRight size={12} className="opacity-60" />
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
