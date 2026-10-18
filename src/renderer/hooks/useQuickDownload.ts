/**
 * One-tap download straight from a catalog thumbnail — no need to open the
 * detail page. Resolves the best torrent stream for the title and starts the download
 * into the folder chosen in Settings → Storage.
 * Progress appears in the Downloads tab and the floating widget.
 */
import { useCallback, useState } from 'react'
import { v4 as uuidv4 } from 'uuid'
import { IPC } from '../../shared/ipc-channels'
import { invoke } from './useIpc'
import { toast } from '../components/Toaster'
import { trackDownloadStart } from '../stores/downloads.store'
import type { TitleMatch, PlayTitleResult } from '../../shared/agent-types'

export interface QuickDownloadItem {
  id: string
  title: string
  coverUrl?: string
  imdbId?: string
  malId?: string
  category: 'movie' | 'series' | 'anime'
  year?: string
}

export function useQuickDownload() {
  /** ids currently resolving a stream (show a spinner on that card). */
  const [resolving, setResolving] = useState<Set<string>>(new Set())

  const quickDownload = useCallback(async (item: QuickDownloadItem) => {
    if (resolving.has(item.id)) return
    setResolving((prev) => new Set(prev).add(item.id))
    try {
      const match: TitleMatch = {
        id: item.id,
        title: item.title,
        category: item.category,
        imdbId: item.imdbId,
        malId: item.malId,
        coverUrl: item.coverUrl,
      }
      const result = await invoke<PlayTitleResult | null>(IPC.AGENT_PLAY_TITLE, match, item.title)
      if (!result?.stream?.infoHash) {
        toast.error('No download source', `Couldn't find a stream for "${item.title}". Check your addons.`)
        return
      }
      if (result.stream.infoHash.startsWith('http')) {
        toast.error('Download unavailable', 'This title only has a direct HTTP stream — use /download in chat.')
        return
      }
      // Folder is resolved by the main process from Settings → Storage.
      const downloadId = uuidv4()
      trackDownloadStart({ id: downloadId, title: item.title, coverUrl: item.coverUrl, category: item.category })
      await invoke(IPC.MEDIA_TORRENT_DOWNLOAD, {
        id: downloadId,
        infoHash: result.stream.infoHash,
        fileIdx: result.stream.fileIdx,
        year: item.year,
        imdbId: result.match?.imdbId || item.imdbId,
        malId: item.malId,
        title: item.title,
        coverUrl: item.coverUrl,
        category: item.category,
      })
      toast.success('Download started', `${item.title} (${result.stream.quality || 'best quality'})`)
    } catch (error: any) {
      toast.error('Download failed', error?.message || 'Could not start the download')
    } finally {
      setResolving((prev) => {
        const next = new Set(prev)
        next.delete(item.id)
        return next
      })
    }
  }, [resolving])

  return { quickDownload, resolving }
}
