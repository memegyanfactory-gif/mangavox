import { ipcMain } from 'electron'
import { existsSync, readFileSync } from 'fs'
import { join } from 'path'
import { IPC } from '../../shared/ipc-channels'
import { chapterDir } from '../services/app-paths'

/**
 * Reader Overlay — provides per-page bounding-box text data so the Reader can
 * render translated text *on top of* the manga images, in the spirit of the
 * AI Manga Reader project. Reads from the existing pipeline OCR output
 * (ocr_results.json) which already contains 4-corner box geometry.
 *
 * Returns null if no OCR data is available — caller is expected to surface a
 * "run OCR first" prompt rather than failing.
 */
export interface OverlayBox {
  /** Image-natural-pixel coordinates as 4 corner points: [tl, tr, br, bl]. */
  points: number[][]
  text: string
  /** 0–1 confidence if the engine reported one. */
  score?: number
}

export interface OverlayPage {
  page: number
  file: string
  boxes: OverlayBox[]
}

export function registerReaderOverlayHandlers(): void {
  ipcMain.handle(IPC.READER_GET_OCR_OVERLAY, (_e, mangaId: string, chapterId: string) => {
    const ocrPath = join(chapterDir(mangaId, chapterId), 'pipeline', 'ocr_results.json')
    if (!existsSync(ocrPath)) {
      return { available: false, pages: [] }
    }

    try {
      const raw = JSON.parse(readFileSync(ocrPath, 'utf-8')) as Array<{
        page: number
        file: string
        texts: string[]
        boxes: Array<{ points: number[][]; score?: number }>
      }>

      const pages: OverlayPage[] = raw.map((p) => ({
        page: p.page,
        file: p.file,
        boxes: (p.boxes || []).map((b, i) => ({
          points: b.points || [],
          text: p.texts?.[i] ?? '',
          score: b.score,
        })).filter((b) => b.points.length === 4 && b.text.trim()),
      }))

      const totalBoxes = pages.reduce((sum, p) => sum + p.boxes.length, 0)
      return { available: totalBoxes > 0, pages, totalBoxes }
    } catch (err: any) {
      console.error('[reader-overlay] Failed to read ocr_results.json:', err)
      return { available: false, pages: [], error: err.message }
    }
  })
}
