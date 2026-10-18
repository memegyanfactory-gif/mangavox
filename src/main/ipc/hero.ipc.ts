import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { getHeroClip } from '../services/hero-clip-cache'

export function registerHeroHandlers(): void {
  ipcMain.handle(IPC.HERO_GET_CLIP, async (_e, title: string, kind?: string) => {
    try {
      const clip = await getHeroClip(title, (kind as any) || 'movie')
      return clip ? `mangavox-local://${clip}` : null
    } catch {
      return null
    }
  })
}
