import { ipcMain, dialog } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import {
  diagnoseErrors, agentChat, execCommand, killActiveProcess,
  autoFix, resolveApproval, cancelFix, setPermissionMode, getPermissionMode,
  clearChatHistory,
} from '../services/terminal-agent'
import { parseAgentInput, getSuggestions, getContextualTags } from '../services/agent-command-parser'
import { searchTorrents, findSimilarTitles, matchTitlesFast, playTitle, prefetchPlayResults } from '../services/agent-torrent-search'
import type { TitleMatch } from '../services/agent-torrent-search'
import { downloadMedia, cancelDownload, getDownloadStatus, getActiveDownloads } from '../services/agent-media-downloader'
import {
  getMemory,
  recordSearch,
  recordDownload,
  recordPlay,
  clearMemory,
  updatePreferences,
} from '../services/agent-memory'
import type { DownloadProgress, DownloadRequest } from '../services/agent-media-downloader'

export function registerAgentHandlers(): void {
  // Diagnose recent error logs using AI
  ipcMain.handle(IPC.AGENT_DIAGNOSE, async (_e, errorLogs: string[]) => {
    try {
      console.log(`[agent:diagnose] Analyzing ${errorLogs.length} error log(s)`)
      return await diagnoseErrors(errorLogs)
    } catch (err: any) {
      console.error(`[agent:diagnose] AI diagnosis failed: ${err?.message}`)
      return `**Diagnosis failed**: ${err?.message}`
    }
  })

  // Execute a shell command (streams output via AGENT_EXEC_OUTPUT)
  ipcMain.handle(IPC.AGENT_EXEC, async (_e, command: string) => {
    try {
      console.log(`[agent:exec] Running: ${command}`)
      return execCommand(command)
    } catch (err: any) {
      console.error(`[agent:exec] Failed to execute "${command}": ${err?.message}`)
      throw err
    }
  })

  // Free-form chat with the terminal agent
  ipcMain.handle(IPC.AGENT_CHAT, async (_e, message: string, recentLogs: string[]) => {
    try {
      console.log(`[agent:chat] User: ${message.substring(0, 80)}…`)
      return await agentChat(message, recentLogs)
    } catch (err: any) {
      console.error(`[agent:chat] Chat failed: ${err?.message}`)
      return `**Error**: ${err?.message}`
    }
  })

  // Start autonomous auto-fix loop
  ipcMain.handle(IPC.AGENT_AUTO_FIX, async (_e, errorLogs: string[], mode?: string) => {
    try {
      if (mode) setPermissionMode(mode as any)
      console.log(`[agent:auto-fix] Starting auto-fix for ${errorLogs.length} error(s), mode=${getPermissionMode()}`)
      // Run in background — status updates come via AGENT_FIX_STATUS push events
      autoFix(errorLogs).catch((err) => {
        console.error(`[agent:auto-fix] Unhandled error: ${err?.message}`)
      })
      return { started: true }
    } catch (err: any) {
      console.error(`[agent:auto-fix] Failed to start: ${err?.message}`)
      throw err
    }
  })

  // Approve or reject a pending action (manual mode)
  ipcMain.handle(IPC.AGENT_APPROVE_ACTION, async (_e, approved: boolean) => {
    resolveApproval(approved)
    return true
  })

  // Cancel an ongoing auto-fix
  ipcMain.handle(IPC.AGENT_CANCEL_FIX, async () => {
    cancelFix()
    killActiveProcess()
    return true
  })

  // Clear chat conversation history
  ipcMain.handle(IPC.AGENT_CHAT_CLEAR, async () => {
    clearChatHistory()
    return true
  })

  // ===== MEDIA AGENT HANDLERS =====

  // Parse user input for commands
  ipcMain.handle(IPC.AGENT_PARSE_INPUT, async (_event, input: string) => parseAgentInput(input))
  ipcMain.handle(IPC.AGENT_GET_SUGGESTIONS, async (_event, input: string) => getSuggestions(input))
  ipcMain.handle(IPC.AGENT_GET_CONTEXTUAL_TAGS, async (_event, command: string, text: string) => getContextualTags(command, text))

  ipcMain.handle(IPC.AGENT_SEARCH_TORRENTS, async (_event, query: string, category?: string) => {
    recordSearch('default', query)
    return searchTorrents({ query, category: (category as any) || 'all', limit: 20 })
  })

  ipcMain.handle(IPC.AGENT_FIND_SIMILAR, async (_event, query: string) => findSimilarTitles(query, 6))

  ipcMain.handle(IPC.AGENT_MATCH_TITLES, async (_event, query: string, category?: string) => {
    recordSearch('default', query)
    return matchTitlesFast(query, (category as any) || 'all')
  })

  ipcMain.handle(IPC.AGENT_PLAY_TITLE, async (_event, match: TitleMatch, query?: string) => {
    const result = await playTitle(match, query)
    if (result) recordPlay('default', match.title, match.category)
    return result
  })

  ipcMain.handle(IPC.AGENT_PREFETCH_PLAY, async (_event, matches: TitleMatch[]) => {
    return prefetchPlayResults(matches)
  })

  ipcMain.handle(IPC.AGENT_START_DOWNLOAD, async (event, request: DownloadRequest) => {
    const { sender } = event
    void downloadMedia(
      request,
      (progress: DownloadProgress) => { sender.send(IPC.AGENT_DOWNLOAD_PROGRESS, progress) },
      (error: { id: string; message: string }) => { sender.send('AGENT_DOWNLOAD_ERROR', error) },
    )
    recordDownload('default', request.outputFilename, request.format)
    return { id: request.id, status: 'started' }
  })

  ipcMain.handle(IPC.AGENT_CANCEL_DOWNLOAD, async (_event, downloadId: string) => {
    cancelDownload(downloadId)
    return { id: downloadId, status: 'cancelled' }
  })

  ipcMain.handle(IPC.AGENT_GET_MEMORY, async () => getMemory('default'))
  ipcMain.handle(IPC.AGENT_CLEAR_MEMORY, async () => { clearMemory('default'); return { status: 'cleared' } })

  ipcMain.handle(IPC.AGENT_PICK_FOLDER, async () => {
    const result = await dialog.showOpenDialog({ properties: ['openDirectory', 'createDirectory'], title: 'Choose save folder' })
    return result.canceled ? null : result.filePaths[0]
  })

  // Update preferences
  ipcMain.handle('AGENT_UPDATE_PREFERENCES', async (_event, updates: any) => {
    updatePreferences('default', updates)
    return getMemory('default').preferences
  })

  // Record play/watch
  ipcMain.handle(IPC.AGENT_RECORD_PLAY, async (_event, title: string, type: string) => {
    recordPlay('default', title, type)
    return { status: 'recorded' }
  })
}
