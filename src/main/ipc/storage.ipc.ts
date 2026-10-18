import { ipcMain, dialog, BrowserWindow } from 'electron'
import { existsSync, readFileSync, writeFileSync, mkdirSync, accessSync, constants, statfsSync } from 'fs'
import { IPC } from '../../shared/ipc-channels'
import { AppPaths, STORAGE_KINDS, getStorageDefaults, getStorageDir, type StorageKind } from '../services/app-paths'

function readConfig(): any {
  try { return existsSync(AppPaths.config) ? JSON.parse(readFileSync(AppPaths.config, 'utf-8')) : {} } catch { return {} }
}

function freeBytes(dir: string): number | null {
  try { const s = statfsSync(dir); return Number(s.bavail) * Number(s.bsize) } catch { return null }
}

function snapshot() {
  const cfg = readConfig().storage ?? {}
  const defaults = getStorageDefaults()
  return STORAGE_KINDS.map((kind) => ({
    kind,
    custom: (cfg[kind] as string | undefined) || '',
    default: defaults[kind],
    effective: getStorageDir(kind),
    freeBytes: freeBytes(getStorageDir(kind)),
  }))
}

export function registerStorageHandlers(): void {
  ipcMain.handle(IPC.STORAGE_GET, () => snapshot())

  // path === '' resets to the default folder
  ipcMain.handle(IPC.STORAGE_SET, (_e, kind: StorageKind, path: string) => {
    if (!STORAGE_KINDS.includes(kind)) throw new Error('Unknown storage kind: ' + kind)
    const value = (path || '').trim()
    if (value) {
      mkdirSync(value, { recursive: true })
      accessSync(value, constants.W_OK) // throws if the folder is not writable
    }
    const cfg = readConfig()
    cfg.storage = { ...(cfg.storage ?? {}), [kind]: value }
    writeFileSync(AppPaths.config, JSON.stringify(cfg, null, 2))
    return snapshot()
  })

  ipcMain.handle(IPC.STORAGE_PICK, async (e, kind: StorageKind) => {
    const win = BrowserWindow.fromWebContents(e.sender) ?? undefined
    const res = await dialog.showOpenDialog(win!, {
      title: 'Choose ' + kind + ' folder',
      defaultPath: getStorageDir(kind),
      properties: ['openDirectory', 'createDirectory'],
    })
    return res.canceled ? null : res.filePaths[0]
  })
}
