/**
 * Live state of bulk (multi-episode) download batches, fed by
 * BULK_DOWNLOAD_STATUS events from the main-process batch queue.
 */
import { create } from 'zustand'
import { IPC } from '../../shared/ipc-channels'
import { invoke, onEvent } from '../hooks/useIpc'

export type BulkItemStatus = 'queued' | 'resolving' | 'downloading' | 'complete' | 'failed' | 'cancelled'

export interface BulkItem {
  index: number
  number: number
  season?: number
  label: string
  status: BulkItemStatus
  percent: number
  quality?: string
  attempts: number
  error?: string
}

export interface BulkBatch {
  batchId: string
  title: string
  kind: 'anime' | 'movie' | 'series'
  coverUrl?: string
  total: number
  completed: number
  failed: number
  percent: number
  status: 'running' | 'complete' | 'cancelled'
  items: BulkItem[]
}

interface BulkState {
  batches: Record<string, BulkBatch>
  cancel: (batchId: string) => Promise<void>
  dismiss: (batchId: string) => void
}

export const useBulkDownloadStore = create<BulkState>((set) => ({
  batches: {},
  cancel: async (batchId) => { await invoke(IPC.BULK_DOWNLOAD_CANCEL, batchId).catch(() => {}) },
  dismiss: (batchId) => set((s) => {
    const next = { ...s.batches }
    delete next[batchId]
    return { batches: next }
  }),
}))

onEvent(IPC.BULK_DOWNLOAD_STATUS, (data: any) => {
  if (!data?.batchId) return
  useBulkDownloadStore.setState((s) => ({ batches: { ...s.batches, [data.batchId]: data as BulkBatch } }))
})
