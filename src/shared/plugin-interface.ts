import type { MangaSearchResult, MangaDetails, ChapterInfo } from './types'

export interface MangaSourcePlugin {
  id: string
  name: string
  version: string
  lang: string
  icon?: string // URL or base64 data URI for plugin icon

  searchManga(query: string, page?: number): Promise<MangaSearchResult[]>
  getMangaDetails(mangaId: string): Promise<MangaDetails>
  getChapterList(mangaId: string): Promise<ChapterInfo[]>
  getPageUrls(chapterId: string): Promise<string[]>

  // Optional: popular / latest / tag browsing
  getPopularManga?(page?: number): Promise<MangaSearchResult[]>
  getLatestManga?(page?: number): Promise<MangaSearchResult[]>
  getTagList?(): Promise<{ id: string; name: string }[]>
  getMangaByTag?(tagId: string, page?: number): Promise<MangaSearchResult[]>

  // Optional: HTTP headers to use when downloading page images (Referer, User-Agent, etc.)
  getDownloadHeaders?(): Record<string, string>

  // Optional: download an image via plugin's own transport (e.g. BrowserWindow with CF cookies).
  // If provided, download-manager uses this instead of nodeFetchBuffer.
  downloadImage?(url: string): Promise<Buffer>
}
