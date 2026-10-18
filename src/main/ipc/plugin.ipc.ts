import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { listPluginSources, getPlugin, installPluginFromUrl, uninstallPlugin } from '../plugins/plugin-loader'
import { getRepoPlugin, listRepoPlugins } from '../plugins/repo-manager'
import type { MangaSourcePlugin } from '../../shared/plugin-interface'

function resolvePlugin(sourceId: string): MangaSourcePlugin {
  const plugin = getPlugin(sourceId) ?? getRepoPlugin(sourceId)
  if (!plugin) throw new Error(`Source not found: ${sourceId}`)
  return plugin
}

export function registerPluginHandlers(): void {
  ipcMain.handle(IPC.PLUGIN_LIST_SOURCES, () => {
    const main = listPluginSources()
    const repo = listRepoPlugins().map((p) => ({
      id: p.id,
      name: p.name,
      version: p.version,
      lang: p.lang,
      bundled: false,
      icon: p.icon,
    }))
    return [...main, ...repo]
  })

  ipcMain.handle(IPC.PLUGIN_SEARCH, async (_e, sourceId: string, query: string, page?: number) => {
    try {
      return await resolvePlugin(sourceId).searchManga(query, page)
    } catch (err: any) {
      console.error(`[plugin] SEARCH FAILED plugin=${sourceId} query="${query}" page=${page || 1}:`, err?.message || err)
      throw err
    }
  })

  ipcMain.handle(IPC.PLUGIN_GET_DETAILS, async (_e, sourceId: string, mangaId: string) => {
    try {
      return await resolvePlugin(sourceId).getMangaDetails(mangaId)
    } catch (err: any) {
      console.error(`[plugin] GET_DETAILS FAILED plugin=${sourceId} manga=${mangaId}:`, err?.message || err)
      return { source_id: mangaId, title: mangaId, status: 'unknown' }
    }
  })

  ipcMain.handle(IPC.PLUGIN_GET_CHAPTERS, async (_e, sourceId: string, mangaId: string) => {
    try {
      return await resolvePlugin(sourceId).getChapterList(mangaId)
    } catch (err: any) {
      console.error(`[plugin] GET_CHAPTERS FAILED plugin=${sourceId} manga=${mangaId}:`, err?.message || err)
      return []
    }
  })

  ipcMain.handle(IPC.PLUGIN_GET_PAGES, async (_e, sourceId: string, chapterId: string) => {
    try {
      return await resolvePlugin(sourceId).getPageUrls(chapterId)
    } catch (err: any) {
      console.error(`[plugin] GET_PAGES FAILED plugin=${sourceId} chapter=${chapterId}:`, err?.message || err)
      throw err // pages are critical — let the error surface to the user
    }
  })

  // Popular manga
  ipcMain.handle(IPC.PLUGIN_GET_POPULAR, async (_e, sourceId: string, page?: number) => {
    const plugin = resolvePlugin(sourceId)
    if (!plugin.getPopularManga) return []
    try {
      return await plugin.getPopularManga(page)
    } catch (err: any) {
      console.error(`[plugin] GET_POPULAR FAILED plugin=${sourceId} page=${page || 1}:`, err?.message || err)
      throw err
    }
  })

  // Latest manga
  ipcMain.handle(IPC.PLUGIN_GET_LATEST, async (_e, sourceId: string, page?: number) => {
    const plugin = resolvePlugin(sourceId)
    if (!plugin.getLatestManga) return []
    try {
      return await plugin.getLatestManga(page)
    } catch (err: any) {
      console.error(`[plugin] GET_LATEST FAILED plugin=${sourceId} page=${page || 1}:`, err?.message || err)
      throw err
    }
  })

  // Tag list
  ipcMain.handle(IPC.PLUGIN_GET_TAGS, async (_e, sourceId: string) => {
    try {
      const plugin = resolvePlugin(sourceId)
      if (!plugin.getTagList) return []
      return await plugin.getTagList()
    } catch (err: any) {
      console.error(`[plugin] GET_TAGS FAILED plugin=${sourceId}:`, err?.message || err)
      return []
    }
  })

  // Manga by tag
  ipcMain.handle(IPC.PLUGIN_GET_BY_TAG, async (_e, sourceId: string, tagId: string, page?: number) => {
    try {
      const plugin = resolvePlugin(sourceId)
      if (!plugin.getMangaByTag) return []
      return await plugin.getMangaByTag(tagId, page)
    } catch (err: any) {
      console.error(`[plugin] GET_BY_TAG FAILED plugin=${sourceId} tag=${tagId} page=${page || 1}:`, err?.message || err)
      return []
    }
  })

  // Plugin management
  ipcMain.handle(IPC.PLUGIN_INSTALL, async (_e, url: string) => {
    return installPluginFromUrl(url)
  })

  ipcMain.handle(IPC.PLUGIN_UNINSTALL, async (_e, pluginId: string) => {
    return uninstallPlugin(pluginId)
  })

  // Find manga across all sources
  ipcMain.handle(IPC.PLUGIN_FIND_ACROSS_SOURCES, async (_e, title: string, currentSourceId: string, currentMangaSourceId: string) => {
    const allSources = [...listPluginSources(), ...listRepoPlugins().map(p => ({ id: p.id, name: p.name, version: p.version, lang: p.lang, bundled: false }))]
    const results: { sourceId: string; sourceName: string; mangaSourceId: string; chapterCount: number }[] = []

    // Add current source first (we already know it has this manga)
    const currentPlugin = getPlugin(currentSourceId) ?? getRepoPlugin(currentSourceId)
    if (currentPlugin) {
      try {
        const chapters = await currentPlugin.getChapterList(currentMangaSourceId)
        const currentSource = allSources.find(s => s.id === currentSourceId)
        results.push({
          sourceId: currentSourceId,
          sourceName: currentSource?.name || currentSourceId,
          mangaSourceId: currentMangaSourceId,
          chapterCount: chapters.length,
        })
      } catch {}
    }

    // Search other sources in parallel (with timeout)
    const otherSources = allSources.filter(s => s.id !== currentSourceId)
    const searchPromises = otherSources.map(async (source) => {
      try {
        const plugin = getPlugin(source.id) ?? getRepoPlugin(source.id)
        if (!plugin) return null
        const searchResults = await Promise.race([
          plugin.searchManga(title, 1),
          new Promise<never>((_, reject) => setTimeout(() => reject(new Error('timeout')), 8000))
        ])
        if (searchResults && searchResults.length > 0) {
          // Find the best match by title
          const match = searchResults.find(r => r.title.toLowerCase() === title.toLowerCase()) || searchResults[0]
          if (match) {
            try {
              const chapters = await Promise.race([
                plugin.getChapterList(match.source_id),
                new Promise<never>((_, reject) => setTimeout(() => reject(new Error('timeout')), 8000))
              ])
              return {
                sourceId: source.id,
                sourceName: source.name,
                mangaSourceId: match.source_id,
                chapterCount: chapters.length,
              }
            } catch { return null }
          }
        }
        return null
      } catch { return null }
    })

    const otherResults = await Promise.allSettled(searchPromises)
    for (const r of otherResults) {
      if (r.status === 'fulfilled' && r.value) {
        results.push(r.value)
      }
    }

    // Sort by chapter count descending
    results.sort((a, b) => b.chapterCount - a.chapterCount)
    return results
  })
}
