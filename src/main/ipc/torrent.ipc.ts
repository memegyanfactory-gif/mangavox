import { ipcMain, shell } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import {
  startStream, stopStream, getStatus, preDownloadNext, stopPreDownload, seekToPiece,
  cancelMediaDownload,
} from '../services/torrent-engine'
import { launchMediaDownload, type MediaDownloadRequest } from '../services/media-download'
import { startBulkDownload, cancelBulkDownload, getBulkStatus, type BulkStartRequest } from '../services/bulk-download'

export function registerTorrentHandlers(): void {
  ipcMain.handle(IPC.TORRENT_START, async (_e, infoHash: string, fileIdx?: number) => {
    try {
      return await startStream(infoHash, fileIdx)
    } catch (err: any) {
      // Return an error object instead of throwing. Thrown errors produce
      // ugly "Error occurred in handler for 'torrent:start'" spam in the
      // Electron console that we can't suppress. Returning { error } lets
      // the renderer handle it cleanly without any framework noise.
      const msg = err?.message || String(err)
      console.warn(`[torrent:start] ${msg}`)
      return { error: msg }
    }
  })

  ipcMain.handle(IPC.TORRENT_STOP, async () => {
    console.log('[torrent:stop] Stopping active stream')
    await stopStream()
    stopPreDownload()
    return true
  })

  ipcMain.handle(IPC.TORRENT_STATUS, () => {
    return getStatus()
  })

  ipcMain.handle(IPC.TORRENT_SEEK, (_e, ratio: number) => {
    seekToPiece(ratio)
    return true
  })

  ipcMain.handle(IPC.MEDIA_TORRENT_DOWNLOAD, async (event, payload: MediaDownloadRequest) => {
    try {
      const { outputDir } = launchMediaDownload(payload, (progress) => {
        if (!event.sender.isDestroyed()) event.sender.send(IPC.MEDIA_DOWNLOAD_PROGRESS, progress)
      })
      return { started: true, outputDir }
    } catch (err: any) {
      return { started: false, error: err?.message || String(err) }
    }
  })

  ipcMain.handle(IPC.BULK_DOWNLOAD_START, async (event, req: BulkStartRequest) => {
    const sender = event.sender
    return startBulkDownload(
      req,
      (state) => { if (!sender.isDestroyed()) sender.send(IPC.BULK_DOWNLOAD_STATUS, state) },
      (progress) => { if (!sender.isDestroyed()) sender.send(IPC.MEDIA_DOWNLOAD_PROGRESS, progress) },
    )
  })

  ipcMain.handle(IPC.BULK_DOWNLOAD_STATUS, (_e, batchId?: string) => getBulkStatus(batchId))
  ipcMain.handle(IPC.BULK_DOWNLOAD_CANCEL, (_e, batchId: string) => cancelBulkDownload(batchId))

  ipcMain.handle(IPC.MEDIA_DOWNLOAD_CANCEL, async (_e, downloadId: string) => {
    cancelMediaDownload(downloadId)
    return true
  })

  ipcMain.handle(IPC.MEDIA_DOWNLOAD_REVEAL, (_e, path: string) => {
    if (path) shell.showItemInFolder(path)
    return true
  })

  ipcMain.handle(IPC.TORRENT_PRE_NEXT, async (_e, infoHash: string, fileIdx?: number) => {
    try {
      console.log(`[torrent:pre-next] Pre-downloading next episode: hash=${infoHash?.substring(0, 20)}…`)
      preDownloadNext(infoHash, fileIdx)
      return true
    } catch (err: any) {
      console.error(`[torrent:pre-next] Failed: ${err?.message || err}`)
      return false
    }
  })
}
