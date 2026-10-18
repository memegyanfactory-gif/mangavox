import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import {
  clearRecoveryState,
  createPlaybackSession,
  getEmbeddedSubtitleUrl,
  getPlaybackPreferences,
  getPlaybackSession,
  getRecoveryState,
  recordPlaybackFailure,
  seekPlaybackSession,
  setPlaybackPreferences,
  stopPlaybackSession,
  switchPlaybackSource,
  updatePlaybackSession,
} from '../services/playback-session'

export function registerPlaybackSessionHandlers(): void {
  ipcMain.handle(IPC.PLAYBACK_SESSION_CREATE, async (_event, input) => {
    try {
      return { session: await createPlaybackSession(input) }
    } catch (error: any) {
      return { error: error?.message || String(error) }
    }
  })
  ipcMain.handle(IPC.PLAYBACK_SESSION_GET, () => getPlaybackSession())
  ipcMain.handle(IPC.PLAYBACK_SESSION_STOP, (_event, sessionId?: string) => stopPlaybackSession(sessionId, true))
  ipcMain.handle(IPC.PLAYBACK_SESSION_UPDATE, (_event, sessionId: string, patch) => updatePlaybackSession(sessionId, patch))
  ipcMain.handle(IPC.PLAYBACK_SESSION_SWITCH_SOURCE, async (_event, sessionId: string, source) => {
    try {
      return { session: await switchPlaybackSource(sessionId, source) }
    } catch (error: any) {
      return { error: error?.message || String(error) }
    }
  })
  ipcMain.handle(IPC.PLAYBACK_SESSION_SEEK, (_event, sessionId: string, positionSecs: number) =>
    seekPlaybackSession(sessionId, positionSecs))
  ipcMain.handle(IPC.PLAYBACK_SESSION_FAILURE, (_event, sessionId: string, failure) =>
    recordPlaybackFailure(sessionId, failure))
  ipcMain.handle(IPC.PLAYBACK_RECOVERY_GET, () => getRecoveryState())
  ipcMain.handle(IPC.PLAYBACK_RECOVERY_CLEAR, () => clearRecoveryState())
  ipcMain.handle(IPC.PLAYBACK_PREFERENCES_GET, () => getPlaybackPreferences())
  ipcMain.handle(IPC.PLAYBACK_PREFERENCES_SET, (_event, patch) => setPlaybackPreferences(patch))
  ipcMain.handle(IPC.PLAYBACK_SUBTITLE_EXTRACT, (_event, index: number) => getEmbeddedSubtitleUrl(index))
}

