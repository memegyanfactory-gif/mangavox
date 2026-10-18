import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import {
  addRepo,
  removeRepo,
  listRepos,
  fetchRepoExtensions,
  installRepoExtension,
} from '../plugins/repo-manager'
import type { RepoExtension } from '../../shared/types'

export function registerRepoHandlers(): void {
  ipcMain.handle(IPC.REPO_ADD, async (_e, url: string) => {
    const normalized = addRepo(url)
    // Immediately fetch to validate
    const extensions = await fetchRepoExtensions(normalized)
    return { url: normalized, count: extensions.length }
  })

  ipcMain.handle(IPC.REPO_REMOVE, (_e, url: string) => {
    removeRepo(url)
    return true
  })

  ipcMain.handle(IPC.REPO_LIST, () => {
    return listRepos()
  })

  ipcMain.handle(IPC.REPO_BROWSE, async (_e, repoUrl: string, lang?: string, search?: string) => {
    const extensions = await fetchRepoExtensions(repoUrl)

    let filtered = extensions

    // Filter by language
    if (lang && lang !== 'all') {
      filtered = filtered.filter((ext) =>
        ext.lang === lang || ext.sources.some((s) => s.lang === lang)
      )
    }

    // Filter by search query
    if (search) {
      const q = search.toLowerCase()
      filtered = filtered.filter((ext) =>
        ext.name.toLowerCase().includes(q) ||
        ext.sources.some((s) => s.name.toLowerCase().includes(q))
      )
    }

    // Filter out NSFW by default (could be a setting later)
    filtered = filtered.filter((ext) => ext.nsfw === 0)

    // Sort by name
    filtered.sort((a, b) => a.name.localeCompare(b.name))

    return filtered
  })

  ipcMain.handle(IPC.REPO_INSTALL_EXT, async (_e, ext: RepoExtension, sourceIndex: number) => {
    return installRepoExtension(ext, sourceIndex)
  })
}
