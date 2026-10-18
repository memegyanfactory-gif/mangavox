import { app, BrowserWindow, ipcMain, shell } from 'electron'
import { autoUpdater } from 'electron-updater'
import { IPC } from '../../shared/ipc-channels'

// In-app updates come from GitHub Releases (see "publish" in package.json and
// .github/workflows/release.yml). Every push to main bumps the version and
// publishes a release; installed builds pick it up here.

export type UpdateState = 'idle' | 'checking' | 'available' | 'not-available' | 'downloading' | 'downloaded' | 'error' | 'unsupported'

export interface UpdateStatus {
  state: UpdateState
  currentVersion: string
  latestVersion?: string
  releaseNotes?: string
  percent?: number
  bytesPerSecond?: number
  error?: string
  checkedAt?: number
}

const RELEASES_URL = 'https://github.com/memegyanfactory-gif/mangavox/releases/latest'
const CHECK_INTERVAL_MS = 4 * 60 * 60 * 1000

let status: UpdateStatus = { state: 'idle', currentVersion: app.getVersion() }
let started = false

function setStatus(patch: Partial<UpdateStatus>): void {
  status = { ...status, ...patch }
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) win.webContents.send(IPC.UPDATE_STATUS, status)
  }
}

function notesToText(notes: unknown): string | undefined {
  if (!notes) return undefined
  if (typeof notes === 'string') return notes.replace(/<[^>]+>/g, '').trim()
  if (Array.isArray(notes)) return notes.map((n: any) => n?.note ?? '').join('\n').replace(/<[^>]+>/g, '').trim()
  return undefined
}

// Unpackaged dev runs and the portable exe can't self-update.
function isUpdatable(): boolean {
  return app.isPackaged && !process.env.PORTABLE_EXECUTABLE_DIR
}

async function check(): Promise<UpdateStatus> {
  if (!isUpdatable()) {
    setStatus({ state: 'unsupported', error: app.isPackaged ? 'Portable builds cannot self-update.' : 'Updates are disabled in development builds.' })
    return status
  }
  try {
    await autoUpdater.checkForUpdates()
  } catch (err) {
    setStatus({ state: 'error', error: err instanceof Error ? err.message : String(err) })
  }
  return status
}

export function registerUpdateHandlers(): void {
  ipcMain.handle(IPC.UPDATE_GET_STATUS, () => status)
  ipcMain.handle(IPC.UPDATE_CHECK, () => check())
  ipcMain.handle(IPC.UPDATE_DOWNLOAD, async () => {
    if (status.state !== 'available') return status
    try {
      await autoUpdater.downloadUpdate()
    } catch (err) {
      setStatus({ state: 'error', error: err instanceof Error ? err.message : String(err) })
    }
    return status
  })
  ipcMain.handle(IPC.UPDATE_INSTALL, () => {
    if (status.state !== 'downloaded') return false
    // isSilent=false shows the installer progress, forceRunAfter relaunches the app.
    setImmediate(() => autoUpdater.quitAndInstall(false, true))
    return true
  })
  ipcMain.handle(IPC.UPDATE_OPEN_RELEASES, () => shell.openExternal(RELEASES_URL))
}

export function startAutoUpdates(): void {
  if (started) return
  started = true

  autoUpdater.autoDownload = true
  autoUpdater.autoInstallOnAppQuit = true
  autoUpdater.logger = {
    info: (m: unknown) => console.log('[updater]', m),
    warn: (m: unknown) => console.warn('[updater]', m),
    error: (m: unknown) => console.error('[updater]', m),
    debug: () => {},
  }

  autoUpdater.on('checking-for-update', () => setStatus({ state: 'checking', error: undefined }))
  autoUpdater.on('update-available', (info) => setStatus({
    state: 'available',
    latestVersion: info.version,
    releaseNotes: notesToText(info.releaseNotes),
    checkedAt: Date.now(),
  }))
  autoUpdater.on('update-not-available', (info) => setStatus({ state: 'not-available', latestVersion: info.version, checkedAt: Date.now() }))
  autoUpdater.on('download-progress', (p) => setStatus({ state: 'downloading', percent: p.percent, bytesPerSecond: p.bytesPerSecond }))
  autoUpdater.on('update-downloaded', (info) => setStatus({ state: 'downloaded', latestVersion: info.version, percent: 100 }))
  autoUpdater.on('error', (err) => setStatus({ state: 'error', error: err?.message ?? String(err) }))

  if (!isUpdatable()) {
    setStatus({ state: 'unsupported' })
    return
  }
  // Give the window a moment to load before the first network check.
  setTimeout(() => { void check() }, 15_000)
  setInterval(() => {
    if (status.state !== 'downloading' && status.state !== 'downloaded') void check()
  }, CHECK_INTERVAL_MS)
}
