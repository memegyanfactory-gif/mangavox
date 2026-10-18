import { useEffect, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { TopNav } from './components/layout/TopNav'
import { FocusBar } from './components/layout/FocusBar'
import { LogPanel } from './components/layout/LogPanel'
import { BottomTabBar } from './components/layout/BottomTabBar'
import { useBreakpoint } from './hooks/useBreakpoint'
import { useUiStore } from './stores/ui.store'
import { useLibraryStore } from './stores/library.store'
import { useLoggingStore } from './stores/logging.store'
import { useWatchHistoryStore } from './stores/watch-history.store'
import { onEvent, invoke } from './hooks/useIpc'
import { IPC } from '../shared/ipc-channels'
import { Browse } from './pages/Browse'
import { Library } from './pages/Library'
import { History } from './pages/History'
import { MangaDetail } from './pages/MangaDetail'
import { Reader } from './pages/Reader'
import { Queue } from './pages/Queue'
import { Player } from './pages/Player'
import { Dashboard } from './pages/Dashboard'
import { TextPage } from './pages/TextPage'
import { Settings } from './pages/Settings'
import { YouTube } from './pages/YouTube'
import { Anime } from './pages/Anime'
import { AnimeDetail } from './pages/AnimeDetail'
import { AnimePlayer } from './pages/AnimePlayer'
import { Movies } from './pages/Movies'
import { Subtitles } from './pages/Subtitles'
import { Discover } from './pages/Discover'
import { PipelineAutomation } from './pages/PipelineAutomation'
import { AudioPage } from './pages/AudioPage'
import { VideoPage } from './pages/VideoPage'
import { Chat } from './pages/Chat'
import { Toaster } from './components/Toaster'
import { UpdateReadyBanner } from './components/UpdateCard'
import { SourceVerifyPrompt } from './components/SourceVerifyPrompt'
import { MiniPlayer } from './components/MiniPlayer'
import { DownloadWidget } from './components/DownloadWidget'
import { useAIModelsStore } from './stores/ai-models.store'
import { SplashScreen } from './components/SplashScreen'
import { FirstRunSetup } from './components/FirstRunSetup'
import logoUrl from './assets/logo.png'
import { Lock } from 'lucide-react'
import { useAppearanceStore } from './stores/appearance.store'
import { useAppModeStore } from './stores/app-mode.store'
import { ErrorBoundary } from './components/ErrorBoundary'
import { AssistantDock, AssistantLauncher } from './components/assistant-dock/AssistantDock'
import { AppSidebar, TitleBar } from './components/layout/AppShell'
import { PlaybackRecovery } from './components/PlaybackRecovery'
import { AppReadyProvider, LoadingOverlay, useAppReady } from './components/common/LoadingOverlay'

const STUDIO_ONLY_PAGES = new Set(['text', 'audio', 'video', 'pipeline', 'dashboard', 'queue'])

// TextPage mounts once and stays mounted for the lifetime of the app — toggled
// only by CSS visibility. This preserves in-flight extraction, generated
// segments, scroll position, and all the page's local useState across
// navigations to/from other pages.
function KeepAliveText({ visible }: { visible: boolean }) {
  return (
    <div
      className="absolute inset-0 h-full"
      style={{
        visibility: visible ? 'visible' : 'hidden',
        // `display: none` would tear down the component tree. Use opacity +
        // pointer-events instead so React keeps the subtree alive.
        opacity: visible ? 1 : 0,
        pointerEvents: visible ? 'auto' : 'none',
        zIndex: visible ? 1 : 0,
      }}
      aria-hidden={!visible}
    >
      <TextPage />
    </div>
  )
}

function PageContent() {
  const { currentPage } = useUiStore()
  const appMode = useAppModeStore((state) => state.mode)

  if (appMode !== 'studio' && STUDIO_ONLY_PAGES.has(currentPage)) return <Library />

  switch (currentPage) {
    case 'chat':
      return <Chat />
    case 'browse':
      return <Browse />
    case 'library':
      return <Library />
    case 'history':
      return <History />
    case 'manga-detail':
      return <MangaDetail />
    case 'reader':
      return <Reader />
    case 'queue':
      return <Queue />
    case 'anime':
      return <Anime />
    case 'anime-detail':
      return <AnimeDetail />
    case 'anime-player':
      return <ErrorBoundary playback><AnimePlayer /></ErrorBoundary>
    case 'movies':
      return <Movies />
    case 'subtitles':
      return <Subtitles />
    case 'discover':
      return <Discover />
    case 'player':
      return <Player />
    case 'text':
      // Rendered as a persistent keep-alive sibling so navigating away and
      // back preserves extraction progress, segments, and pipeline state.
      // The animation layer renders a placeholder; the real TextPage lives
      // in <KeepAliveText/> below.
      return null
    case 'dashboard':
      return <Dashboard />
    case 'settings':
      return <Settings />
    case 'youtube':
      return <YouTube />
    case 'pipeline':
      return <PipelineAutomation />
    case 'audio':
      return <AudioPage />
    case 'video':
      return <VideoPage />
    default:
      return <Library />
  }
}

function LockScreen({ onUnlock }: { onUnlock: () => void }) {
  const [password, setPassword] = useState('')
  const [error, setError] = useState('')
  const [checking, setChecking] = useState(false)

  const handleUnlock = async () => {
    if (!password) return
    setChecking(true)
    setError('')
    try {
      const user = await invoke<{ password_hash?: string | null } | null>(IPC.CONFIG_GET, 'user')
      if (!user?.password_hash) { onUnlock(); return }
      const msgBuffer = new TextEncoder().encode(password)
      const hashBuffer = await crypto.subtle.digest('SHA-256', msgBuffer)
      const hashHex = Array.from(new Uint8Array(hashBuffer)).map((b) => b.toString(16).padStart(2, '0')).join('')
      if (hashHex === user.password_hash) {
        onUnlock()
      } else {
        setError('Incorrect password')
        setPassword('')
      }
    } finally {
      setChecking(false)
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col items-center justify-center gap-6"
      style={{ background: 'var(--base)', backdropFilter: 'blur(20px)' }}
    >
      <img src={logoUrl} alt="MangaVox" className="w-16 h-16 rounded-2xl object-contain" />
      <div className="text-center">
        <h1 className="text-2xl font-bold text-zinc-100">MangaVox</h1>
        <p className="text-sm text-zinc-500 mt-1">Enter your password to continue</p>
      </div>
      <div className="w-72 space-y-3">
        <div className="relative">
          <Lock size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-zinc-500" />
          <input
            type="password"
            value={password}
            autoFocus
            onChange={(e) => { setPassword(e.target.value); setError('') }}
            onKeyDown={(e) => e.key === 'Enter' && handleUnlock()}
            placeholder="Password"
            className="w-full h-11 pl-9 pr-4 rounded-xl text-sm text-zinc-200 placeholder-zinc-600 outline-none"
            style={{ background: 'var(--surface-2)', border: error ? '1px solid rgba(239,68,68,0.4)' : '1px solid rgba(255,255,255,0.06)' }}
          />
        </div>
        {error && <p className="text-xs text-rose-400 text-center">{error}</p>}
        <button
          onClick={handleUnlock}
          disabled={!password || checking}
          className="w-full h-11 rounded-xl text-sm font-semibold text-white transition-all disabled:opacity-40"
          style={{ background: 'linear-gradient(135deg, #7C3AED, #2563EB)' }}
        >
          {checking ? 'Checking…' : 'Unlock'}
        </button>
      </div>
    </div>
  )
}

function AppContent() {
  const { ready, setReady } = useAppReady()
  const { currentPage, navigate } = useUiStore()
  const appMode = useAppModeStore((state) => state.mode)
  const appModeHydrated = useAppModeStore((state) => state.hydrated)
  const hydrateAppMode = useAppModeStore((state) => state.hydrate)
  const textLayout = useUiStore((state) => state.textLayout)
  const isFullscreen = currentPage === 'reader' || currentPage === 'player' || currentPage === 'anime-player'
  const isFocusMode = isFullscreen || (currentPage === 'text' && textLayout === 'focus')
  const [locked, setLocked] = useState<boolean | null>(null) // null = loading
  const [showSplash, setShowSplash] = useState(true)
  const [showFirstRun, setShowFirstRun] = useState(false)
  const hydrateAppearance = useAppearanceStore((state) => state.hydrate)
  const hydrateAIModels = useAIModelsStore((state) => state.hydrate)
  const bp = useBreakpoint()
  const isCompact = bp === 'sm'
  const showBottomNav = isCompact && !isFocusMode
  // The assistant docks beside the page on wide windows and floats over it otherwise.
  const dockOverlay = isCompact || isFocusMode

  // Expose layout state to CSS: html[data-bp="sm|md|lg"], html[data-bottom-nav] (→ --mv-bottom-nav-h)
  useEffect(() => {
    const root = document.documentElement
    root.dataset.bp = bp
    if (showBottomNav) root.dataset.bottomNav = ''
    else delete root.dataset.bottomNav
  }, [bp, showBottomNav])

  useEffect(() => {
    invoke<boolean>(IPC.APP_READY)
      .then((isReady) => {
        if (isReady) setReady(true)
      })
      .catch(() => {})

    return onEvent(IPC.APP_READY, () => {
      setReady(true)
    })
  }, [setReady])

  useEffect(() => {
    hydrateAppearance()
    hydrateAppMode()
    hydrateAIModels()
  }, [hydrateAppearance, hydrateAppMode, hydrateAIModels])

  useEffect(() => {
    const migrationKey = 'mangavox.media.sqliteMigration.v1'
    if (localStorage.getItem(migrationKey)) return
    try {
      const persisted = JSON.parse(localStorage.getItem('mangavox-watch-history') || '{}')
      const entries = persisted?.state?.entries || []
      Promise.all(entries.map((entry: any) => invoke(IPC.MEDIA_EVENT_RECORD, {
        kind: entry.kind,
        type: 'progress',
        positionSecs: entry.episodeNumber ? 1 : 0,
        durationSecs: 0,
        episodeNumber: entry.episodeNumber,
        createdAt: entry.lastOpenedAt,
        title: {
          id: entry.kind === 'anime' ? `anime:mal:${entry.id}` : `movie:imdb:${entry.id}`,
          kind: entry.kind,
          title: entry.title,
          imdbId: entry.kind === 'movie' ? entry.id : undefined,
          malId: entry.kind === 'anime' ? Number(entry.id) : undefined,
          coverUrl: entry.coverUrl,
          year: entry.year,
          genres: entry.genres || [],
          languages: entry.kind === 'anime' ? ['Japanese'] : [],
          description: entry.description,
          score: entry.score,
        },
      }))).then(() => {
        localStorage.setItem(migrationKey, 'done')
        localStorage.removeItem('mangavox-watch-history')
        localStorage.removeItem('mangavox:discover:seen')
        localStorage.removeItem('mangavox:discover:liked')
      }).catch(() => {})
    } catch {
      localStorage.setItem(migrationKey, 'done')
    }
  }, [])

  useEffect(() => {
    if (appMode !== 'studio' && STUDIO_ONLY_PAGES.has(currentPage)) navigate('library')
  }, [appMode, currentPage, navigate])

  // Check if password lock is enabled and if first run on startup
  useEffect(() => {
    invoke<{ password_hash?: string | null } | null>(IPC.CONFIG_GET, 'user').then((user) => {
      setLocked(!!user?.password_hash)
    }).catch(() => setLocked(false))

    Promise.all([
      invoke<boolean | null>(IPC.CONFIG_GET, 'app.firstRunComplete'),
      invoke<'entertainment' | 'studio' | null>(IPC.CONFIG_GET, 'app.mode'),
    ]).then(([done, savedMode]) => {
      if (!done || !savedMode) setShowFirstRun(true)
    }).catch(() => {})
  }, [])

  // Listen for full app reset from main process
  useEffect(() => {
    return onEvent('app:reset', () => {
      // Clear ALL Zustand stores and persisted state
      useLibraryStore.setState({ manga: [], chapters: [], selectedManga: null, searchResults: [] })
      useLoggingStore.getState().clearLogs()
      // Clear watch history (anime + movies) and wipe its persisted localStorage key
      useWatchHistoryStore.getState().clear()
      void invoke(IPC.MEDIA_PERSONALIZATION_RESET)
      try { localStorage.removeItem('mangavox-watch-history') } catch { /* ok */ }
      // Clear any other persisted stores (zustand/persist keys)
      try {
        for (let i = localStorage.length - 1; i >= 0; i--) {
          const k = localStorage.key(i)
          if (k && (k.startsWith('mangavox-') || k.startsWith('mangavox:'))) localStorage.removeItem(k)
        }
      } catch { /* ok */ }
      // Reset UI state (category, history stack, PIP)
      useUiStore.setState({
        category: 'manga',
        currentPage: 'library',
        pageParams: {},
        history: [],
        pip: null,
      })
      navigate('library')
    })
  }, [])

  // Splash screen
  if (showSplash) return <SplashScreen onDone={() => setShowSplash(false)} />

  if (!appModeHydrated || locked === null) {
    return (
      <div className="flex h-screen items-center justify-center" style={{ background: 'var(--base)', color: 'var(--text-3)' }}>
        <div className="flex flex-col items-center gap-3">
          <img src={logoUrl} alt="" className="w-12 h-12 rounded-xl opacity-80" />
          <p className="text-sm">Loading MangaVox…</p>
        </div>
      </div>
    )
  }

  // First-run setup wizard
  if (showFirstRun) return <FirstRunSetup onComplete={() => setShowFirstRun(false)} />

  // Show lock screen
  if (locked) return <LockScreen onUnlock={() => setLocked(false)} />

  return (
    <>
      <LoadingOverlay show={!ready} />
      <div className="flex flex-col h-screen text-[var(--text-1)] relative overflow-hidden" style={{ background: 'var(--base)' }}>
        {isFocusMode ? <FocusBar /> : isCompact ? <TopNav /> : <TitleBar />}
        <div className="flex-1 flex min-h-0 relative z-[1]">
        {!isFocusMode && !isCompact && <AppSidebar />}
        <ErrorBoundary><AssistantDock overlay={dockOverlay} /></ErrorBoundary>
        <div
          className="flex-1 flex flex-col overflow-hidden relative min-w-0"
          style={{ paddingBottom: showBottomNav ? 'var(--mv-bottom-nav-h)' : undefined }}
        >
          <main className="flex-1 overflow-hidden relative">
            <AnimatePresence mode="wait">
              <motion.div
                key={currentPage}
                className="h-full"
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                // Instant exit so the next page mounts right away; short enter keeps a hint of motion.
                exit={{ opacity: 0, transition: { duration: 0 } }}
                transition={{ duration: 0.12, ease: [0.4, 0, 0.2, 1] }}
              >
                <ErrorBoundary><PageContent /></ErrorBoundary>
              </motion.div>
            </AnimatePresence>
            {appMode === 'studio' && (
              <ErrorBoundary>
                <KeepAliveText visible={currentPage === 'text'} />
              </ErrorBoundary>
            )}
          </main>
          {appMode === 'studio' && currentPage !== 'chat' && <LogPanel />}
          {!isFullscreen && <DownloadWidget />}
        </div>
        </div>
        <AssistantLauncher hidden={isFocusMode || !isCompact} />
        {showBottomNav && <BottomTabBar />}
        <Toaster />
        <UpdateReadyBanner />
        <SourceVerifyPrompt />
        <PlaybackRecovery />
      </div>
    </>
  )
}

export default function App() {
  return (
    <AppReadyProvider>
      <ErrorBoundary>
        <AppContent />
      </ErrorBoundary>
    </AppReadyProvider>
  )
}
