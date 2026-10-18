import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import * as categoryRepo from '../db/category.repo'

export function registerCategoryHandlers(): void {
  ipcMain.handle(IPC.CATEGORY_LIST, () => categoryRepo.listCategories())
  ipcMain.handle(IPC.CATEGORY_CREATE, (_e, name: string) => categoryRepo.createCategory(name))
  ipcMain.handle(IPC.CATEGORY_RENAME, (_e, id: string, name: string) => {
    categoryRepo.renameCategory(id, name)
    return true
  })
  ipcMain.handle(IPC.CATEGORY_DELETE, (_e, id: string) => {
    categoryRepo.deleteCategory(id)
    return true
  })
  ipcMain.handle(IPC.CATEGORY_SET_FOR_MANGA, (_e, mangaId: string, categoryIds: string[]) => {
    categoryRepo.setMangaCategories(mangaId, categoryIds)
    return true
  })
  ipcMain.handle(IPC.CATEGORY_MANGA_MAP, () => categoryRepo.getMangaCategoryMap())
}
