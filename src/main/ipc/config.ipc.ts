import { ipcMain, dialog, BrowserWindow } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { readFileSync, writeFileSync, existsSync, mkdirSync, copyFileSync } from 'fs'
import { join } from 'path'
import { AppPaths } from '../services/app-paths'

function readConfig(): Record<string, any> {
  try {
    if (existsSync(AppPaths.config)) {
      return JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
    }
  } catch { /* corrupted config — return defaults */ }
  return {}
}

function writeConfig(config: Record<string, any>): void {
  writeFileSync(AppPaths.config, JSON.stringify(config, null, 2))
}

function redactAssistant(ai: any): any {
  if (!ai || typeof ai !== 'object') return ai ?? null
  const providers: Record<string, any> = {}
  for (const [id, p] of Object.entries<any>(ai.providers ?? {})) {
    const { apiKey, ...rest } = p ?? {}
    providers[id] = { ...rest, hasKey: Boolean(apiKey) }
  }
  const { hfToken, ...hub } = ai.hub ?? {}
  return { ...ai, providers, hub: { ...hub, hasToken: Boolean(hfToken) } }
}

export function registerConfigHandlers(): void {
  ipcMain.handle(IPC.CONFIG_GET, (_e, key?: string) => {
    const config = readConfig()
    if (key === 'trakt' && config.trakt) {
      const { token: _token, ...publicTrakt } = config.trakt
      return publicTrakt
    }
    // Assistant API keys and the Hugging Face token never leave the main process.
    if (key === 'aiAssistant') return redactAssistant(config.aiAssistant)
    if (key) return config[key] ?? null
    return { ...config, aiAssistant: redactAssistant(config.aiAssistant) }
  })

  ipcMain.handle(IPC.CONFIG_SET, (_e, key: string, value: any) => {
    if (key === 'aiAssistant') throw new Error('Use the ai:* channels to change assistant settings')
    const config = readConfig()
    config[key] = key === 'trakt' ? { ...(config[key] || {}), ...(value || {}) } : value
    writeConfig(config)

    for (const win of BrowserWindow.getAllWindows()) {
      if (!win.isDestroyed()) {
        win.webContents.send('config:changed', { key, value })
      }
    }
    return true
  })

  // Upload user avatar photo — copies chosen file to ~/.mangavox/cache/avatars/user.png
  ipcMain.handle(IPC.USER_UPLOAD_AVATAR, async () => {
    const { canceled, filePaths } = await dialog.showOpenDialog({
      title: 'Choose Profile Photo',
      filters: [{ name: 'Images', extensions: ['jpg', 'jpeg', 'png', 'webp'] }],
      properties: ['openFile'],
    })
    if (canceled || !filePaths[0]) return null

    const destDir = join(AppPaths.root, 'cache', 'avatars')
    mkdirSync(destDir, { recursive: true })
    const destPath = join(destDir, 'user.png')
    copyFileSync(filePaths[0], destPath)
    return destPath
  })

  // Pick background music file
  ipcMain.handle(IPC.CONFIG_PICK_AUDIO, async () => {
    const { canceled, filePaths } = await dialog.showOpenDialog({
      title: 'Choose Background Music',
      filters: [{ name: 'Audio Files', extensions: ['mp3', 'wav', 'flac', 'm4a', 'aac'] }],
      properties: ['openFile'],
    })
    if (canceled || !filePaths[0]) return null
    return filePaths[0]
  })

  // Return true immediately when queried by renderer to indicate startup is finished.
  ipcMain.handle(IPC.APP_READY, () => {
    return true
  })
}
