// Must stay first: log capture, crash handlers, IPC tracing, dev CDP port (docs/DEBUGGING.md)
import './services/debug-bootstrap'
import { app, BrowserWindow, Tray, Menu, nativeImage, ipcMain, shell } from 'electron'
import { join } from 'path'
import { ensureAppDirs, AppPaths } from './services/app-paths'
import { existsSync, readFileSync, writeFileSync } from 'fs'
import { initDatabase, closeDatabase } from './db/database'
import { registerAllHandlers } from './ipc/index'
import { loadAllPlugins } from './plugins/plugin-loader'
import { setMainWindow, resetStaleDownloads } from './download/download-manager'
import { setMainWindowForPipeline } from './pipeline/orchestrator'
import { setMainWindowForLogger, interceptConsole } from './services/logger'
import { setMainWindowForText } from './ipc/text.ipc'
import { setMainWindowForTts } from './ipc/tts.ipc'
import { setMainWindowForCropImport } from './ipc/crop-import.ipc'
import { loadAllAddons } from './addons/addon-loader'
import { setMainWindowForTorrent, destroyClient as destroyTorrentClient } from './services/torrent-engine'
import { setMainWindowForAgent } from './services/terminal-agent'
import { patchGlobalFetch } from './services/dns-fetch'
import { registerCoverProtocol } from './services/cover-proxy'
import { registerLocalFileProtocol } from './services/local-file-protocol'
import { registerStreamProtocol } from './services/stream-protocol'
import { startDirectStreamServer, destroyDirectStreamServer } from './services/direct-stream-engine'
import { protocol } from 'electron'
import { ensureBundledChatModel } from './services/bundled-chat-model'
import { stopPlaybackSession } from './services/playback-session'
import { IPC } from '../shared/ipc-channels'
import { startAutoUpdates } from './services/app-updater'

// Must register custom scheme privileges BEFORE app.ready
protocol.registerSchemesAsPrivileged([
  {
    scheme: 'mangavox-cover',
    privileges: {
      standard: true,
      secure: true,
      supportFetchAPI: true,
      corsEnabled: true,
      stream: true,
    },
  },
  {
    scheme: 'mangavox-local',
    privileges: {
      standard: true,
      secure: true,
      supportFetchAPI: true,
      corsEnabled: true,
      stream: true,
    },
  },
  {
    // Torrent video stream — must be secure+stream to bypass Chromium's
    // "Media load rejected by URL safety check" error when the renderer is
    // loaded from a file:// origin and the <video src> points to http://127.0.0.1
    scheme: 'mvstream',
    privileges: {
      standard: true,
      secure: true,
      supportFetchAPI: true,
      corsEnabled: true,
      stream: true,
      bypassCSP: true,
    },
  },
])

// Also allow http://127.0.0.1 to be treated as a trustworthy origin as a belt-and-braces
// backup in case the renderer tries to hit the raw HTTP URL directly (e.g. for download).
app.commandLine.appendSwitch('unsafely-treat-insecure-origin-as-secure', 'http://127.0.0.1:9876,http://localhost:9876')
app.commandLine.appendSwitch('autoplay-policy', 'no-user-gesture-required')
// Enable hardware HEVC/H.265 decoding where the platform supports it.
// macOS (VideoToolbox), Windows (D3D11), and Linux (VA-API) can decode HEVC
// natively when this feature flag is on. Without it, many anime/movie
// torrents (which are frequently HEVC to save bandwidth) fail with a
// PIPELINE_ERROR_DECODE. Paired with the ffmpeg transcode fallback in
// torrent-engine.ts, this covers virtually every codec in the wild.
app.commandLine.appendSwitch('enable-features', 'PlatformHEVCDecoderSupport,PlatformEncryptedHevcSupport')
// Also let Chromium use more codecs from system libraries when available.
app.commandLine.appendSwitch('enable-accelerated-video-decode')

let mainWindow: BrowserWindow | null = null
let tray: Tray | null = null

function getResourcePath(filename: string): string {
  // In production, extraResources copies resources/ into process.resourcesPath/resources/
  return app.isPackaged
    ? join(process.resourcesPath, 'resources', filename)
    : join(app.getAppPath(), 'resources', filename)
}

function getIconPath(): string {
  return getResourcePath('icon.png')
}

function getTrayIconPath(): string {
  return getResourcePath('tray-iconTemplate.png')
}

function createTray(): void {
  let trayImg = nativeImage.createFromPath(getTrayIconPath())
  if (trayImg.isEmpty()) {
    console.warn('[tray] Template icon not found, falling back to main icon')
    trayImg = nativeImage.createFromPath(getIconPath()).resize({ width: 22, height: 22 })
  }
  tray = new Tray(trayImg)
  tray.setToolTip('MangaVox')

  const contextMenu = Menu.buildFromTemplate([
    {
      label: 'Open MangaVox',
      click: () => {
        if (mainWindow) {
          mainWindow.show()
          mainWindow.focus()
        } else {
          createWindow()
        }
      },
    },
    { type: 'separator' },
    {
      label: 'Quit',
      click: () => {
        app.quit()
      },
    },
  ])

  tray.setContextMenu(contextMenu)

  tray.on('click', () => {
    if (mainWindow) {
      mainWindow.isVisible() ? mainWindow.focus() : mainWindow.show()
    }
  })
}

ipcMain.handle(IPC.WINDOW_SET_OVERLAY, (_e, opts: { color: string; symbolColor: string }) => {
  if (process.platform === 'darwin' || !mainWindow || mainWindow.isDestroyed()) return
  try { mainWindow.setTitleBarOverlay({ color: opts.color, symbolColor: opts.symbolColor, height: 48 }) } catch { /* overlay unsupported */ }
})

function createWindow(): void {
  const appIcon = nativeImage.createFromPath(getIconPath())

  mainWindow = new BrowserWindow({
    title: 'MangaVox',
    width: 1280,
    height: 800,
    // Responsive UI: the renderer adapts down to phone widths (compact layout below 640 CSS px)
    minWidth: 360,
    minHeight: 560,
    ...(process.platform === 'darwin'
      ? { titleBarStyle: 'hiddenInset' as const }
      : {
          // Single unified top bar: native min/max/close are drawn over the app's own nav
          titleBarStyle: 'hidden' as const,
          titleBarOverlay: { color: '#0e0907', symbolColor: '#f0b27a', height: 48 },
        }),
    backgroundColor: '#0e0907', // --base
    icon: appIcon,
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      sandbox: false,
      contextIsolation: true,
      nodeIntegration: false,
    },
  })

  // Compact UI: Windows DPI scaling made the layout look zoomed-in; 0.85 gives a denser, more professional density
  mainWindow.webContents.on('did-finish-load', () => mainWindow?.webContents.setZoomFactor(0.85))

  // Load renderer
  if (process.env.ELECTRON_RENDERER_URL) {
    mainWindow.loadURL(process.env.ELECTRON_RENDERER_URL)
  } else {
    mainWindow.loadFile(join(__dirname, '../renderer/index.html'))
  }

  mainWindow.on('closed', () => {
    setMainWindow(null)
    mainWindow = null
  })

  mainWindow.webContents.on('render-process-gone', (_event, details) => {
    console.error(`[window] Renderer process gone: ${details.reason} (${details.exitCode})`)
    void stopPlaybackSession(undefined, false).finally(() => {
      if (mainWindow && !mainWindow.isDestroyed() && details.reason !== 'clean-exit') {
        setTimeout(() => mainWindow?.reload(), 500)
      }
    })
  })

  mainWindow.on('unresponsive', () => {
    console.error('[window] Renderer became unresponsive; preserving recovery state and restarting the renderer')
    void stopPlaybackSession(undefined, false).finally(() => {
      if (mainWindow && !mainWindow.isDestroyed()) mainWindow.reload()
    })
  })

  // Allow popup windows for trusted OAuth flows (Puter sign-in, etc.).
  // Default Electron behaviour is `deny` for window.open(); without this the
  // Puter SDK can't show its sign-in dialog.
  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    try {
      const u = new URL(url)
      // Puter authentication flow — allow the popup as its own native window
      if (u.hostname === 'puter.com' || u.hostname.endsWith('.puter.com')) {
        return {
          action: 'allow',
          overrideBrowserWindowOptions: {
            width: 480,
            height: 640,
            minWidth: 360,
            minHeight: 480,
            title: 'Sign in to Puter',
            backgroundColor: '#0e0907',
            webPreferences: {
              // Puter pages need full Chromium — no preload, default sandbox
              contextIsolation: true,
              sandbox: true,
              nodeIntegration: false,
            },
          },
        }
      }
      // Links in assistant replies and docs open in the user's browser.
      if (u.protocol === 'https:' || u.protocol === 'http:') void shell.openExternal(url)
    } catch { /* malformed URL — fall through */ }
    return { action: 'deny' }
  })
}

// Set the app name for dock/taskbar display
app.setName('MangaVox')

app.whenReady().then(async () => {
  // Chromium's built-in async resolver fails on some networks where the OS resolver works (ERR_NAME_NOT_RESOLVED for scraper windows)
  try { app.configureHostResolver({ enableBuiltInResolver: false }) } catch { /* older Electron */ }
  resetStaleDownloads()
  // Intercept console for log broadcasting (before anything else logs)
  interceptConsole()

  // Patch global fetch with DNS-aware implementation (before loading plugins)
  patchGlobalFetch()

  // Initialize data directory and database
  ensureAppDirs()

  // ── One-shot startup migration: scrub dead bundled g0i.ai sample key ────
  // Earlier MangaVox builds auto-persisted a shared g0i.ai trial key to
  // ~/.mangavox/config.json. g0i has since killed that trial, so any leftover
  // config keeps narration failing with "Trial expired". Clean it here at
  // boot — before any IPC handler or window — so the first launch with this
  // build silently fixes itself even if the user never opens Settings.
  try {
    const DEAD_G0I_KEYS = new Set<string>([
      'sk-43484e549f578382daf4453b70887abff1b98ba0b1e9295d196c4871c217240b',
    ])
    if (existsSync(AppPaths.config)) {
      const cfgRaw = readFileSync(AppPaths.config, 'utf-8')
      const cfg = JSON.parse(cfgRaw)
      const k = cfg?.g0i?.api_key
      if (typeof k === 'string' && DEAD_G0I_KEYS.has(k.trim())) {
        cfg.g0i.api_key = ''
        writeFileSync(AppPaths.config, JSON.stringify(cfg, null, 2))
        console.log('[boot] Auto-cleared dead bundled g0i.ai sample key from config')
      }
      // Also drop the known-bad gemini-2.0-flash-live ID at boot for the same reason.
      if (cfg?.gemini?.model === 'gemini-2.0-flash-live') {
        cfg.gemini.model = 'gemini-2.5-flash'
        writeFileSync(AppPaths.config, JSON.stringify(cfg, null, 2))
        console.log('[boot] Migrated retired gemini-2.0-flash-live -> gemini-2.5-flash')
      }
    }
  } catch (err) {
    console.warn('[boot] Config migration skipped:', err)
  }

  try {
    initDatabase()
    console.log('Database initialized successfully')
  } catch (err) {
    console.error('Failed to initialize database:', err)
  }

  // Register custom protocols
  registerCoverProtocol()
  registerLocalFileProtocol()
  registerStreamProtocol()

  // Start standalone direct HTTP/HLS streaming server
  startDirectStreamServer().catch((err) => {
    console.error('Failed to start direct stream server:', err)
  })

  // Register IPC handlers
  registerAllHandlers()

  // Register the packaged Studio chat model with the user's Ollama store.
  // This is a local file copy; users never need to download the model.
  ensureBundledChatModel().catch((err) => {
    console.warn('[boot] Bundled chat model setup failed:', err)
  })

  // Load manga source plugins + anime addons
  await loadAllPlugins()
  loadAllAddons()

  // Create tray icon
  createTray()

  // Check GitHub Releases for new versions (packaged builds only)
  startAutoUpdates()

  // Create the main window
  createWindow()

  // Pass window ref to download manager, pipeline, and logger for progress events
  if (mainWindow) {
    setMainWindow(mainWindow)
    setMainWindowForPipeline(mainWindow)
    setMainWindowForLogger(mainWindow)
    setMainWindowForText(mainWindow)
    setMainWindowForTts(mainWindow)
    setMainWindowForCropImport(mainWindow)
    setMainWindowForTorrent(mainWindow)
    setMainWindowForAgent(mainWindow)
  }

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow()
    }
  })

  // Emit ready signal after all init steps and when content finishes loading
  if (mainWindow) {
    mainWindow.webContents.on('did-finish-load', () => {
      mainWindow?.webContents.send(IPC.APP_READY)
    })
  }
})

app.on('window-all-closed', () => {
  destroyTorrentClient()
  destroyDirectStreamServer()
  closeDatabase()
  if (process.platform !== 'darwin') {
    app.quit()
  }
})
