import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import {
  getAIModelPreferences,
  getCompatibleCatalog,
  getSystemProfile,
  setAIModelPreferences,
} from '../services/model-manager'
import type { AIModelPreferences } from '../../shared/types'

export function registerModelHandlers(): void {
  ipcMain.handle(IPC.SYSTEM_PROFILE, () => getSystemProfile())
  ipcMain.handle(IPC.AI_MODEL_CATALOG, (_event, installed: string[] = []) => getCompatibleCatalog(installed))
  ipcMain.handle(IPC.AI_MODEL_PREFERENCES_GET, (_event, installed: string[] = []) => getAIModelPreferences(installed))
  ipcMain.handle(IPC.AI_MODEL_PREFERENCES_SET, (_event, preferences: Partial<AIModelPreferences>) => setAIModelPreferences(preferences))
}
