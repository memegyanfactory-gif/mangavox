import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import type { AiProviderId, AiProviderSettings, AiStepRequest, HubModelFile } from '../../shared/ai-types'
import { listModels, listProviders, runStep, stopStep, testProvider, updateProviderSettings } from '../ai/registry'
import {
  cancelDownload, deleteLocalModel, downloadHubFile, downloadUrl, hubConfigPublic, hubDetail, importIntoOllama,
  listDownloads, listLocalModels, revealModelsFolder, searchHub, setHubConfig, systemFit,
} from '../ai/model-hub'

export function registerAiHandlers(): void {
  ipcMain.handle(IPC.AI_PROVIDERS, (_e, force?: boolean) => listProviders(Boolean(force)))
  ipcMain.handle(IPC.AI_MODELS, (_e, id: AiProviderId, force?: boolean) => listModels(id, Boolean(force)))

  ipcMain.handle(IPC.AI_STEP, (e, req: AiStepRequest) =>
    runStep(req, (ev) => {
      if (!e.sender.isDestroyed()) e.sender.send(IPC.AI_EVENT, { ...ev, requestId: req.requestId })
    }),
  )
  ipcMain.handle(IPC.AI_STOP, (_e, requestId: string) => stopStep(requestId))

  ipcMain.handle(IPC.AI_PROVIDER_UPDATE, (_e, id: AiProviderId, patch: AiProviderSettings) => {
    updateProviderSettings(id, patch)
    return true
  })
  ipcMain.handle(IPC.AI_PROVIDER_TEST, (_e, id: AiProviderId, model?: string) => testProvider(id, model))

  ipcMain.handle(IPC.AI_HUB_SEARCH, (_e, query: string, opts?: Parameters<typeof searchHub>[1]) => searchHub(query ?? '', opts))
  ipcMain.handle(IPC.AI_HUB_DETAIL, (_e, repo: string) => hubDetail(repo))
  ipcMain.handle(IPC.AI_HUB_FIT, () => systemFit())
  ipcMain.handle(IPC.AI_HUB_DOWNLOAD, (_e, repo: string, file: HubModelFile) => downloadHubFile(repo, file))
  ipcMain.handle(IPC.AI_HUB_DOWNLOAD_URL, (_e, url: string) => downloadUrl(url))
  ipcMain.handle(IPC.AI_HUB_CANCEL, (_e, id: string) => cancelDownload(id))
  ipcMain.handle(IPC.AI_HUB_DOWNLOADS, () => listDownloads())
  ipcMain.handle(IPC.AI_HUB_LOCAL, () => listLocalModels())
  ipcMain.handle(IPC.AI_HUB_DELETE, (_e, path: string) => deleteLocalModel(path))
  ipcMain.handle(IPC.AI_HUB_REVEAL, (_e, path?: string) => revealModelsFolder(path))
  ipcMain.handle(IPC.AI_HUB_IMPORT_OLLAMA, (_e, path: string, name?: string) => importIntoOllama(path, name))
  ipcMain.handle(IPC.AI_HUB_CONFIG, () => hubConfigPublic())
  ipcMain.handle(IPC.AI_HUB_SET_CONFIG, (_e, patch: { hfToken?: string; modelsDir?: string }) => {
    setHubConfig(patch)
    return hubConfigPublic()
  })
}
