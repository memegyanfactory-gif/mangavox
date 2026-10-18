import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { buildSkill, deleteSkill, getSkill, listSkills } from '../services/custom-skills'

export function registerSkillHandlers(): void {
  ipcMain.handle(IPC.SKILL_LIST, () => listSkills())
  ipcMain.handle(IPC.SKILL_GET, (_e, id: string) => getSkill(id))
  ipcMain.handle(IPC.SKILL_DELETE, (_e, id: string) => { deleteSkill(id); return true })
  ipcMain.handle(IPC.SKILL_BUILD, async (event, payload: { topic: string; model: string; requestId?: string }) => {
    return buildSkill(payload.topic, payload.model, (progress) => {
      if (payload.requestId) {
        event.sender.send(IPC.MEDIA_CHAT_STATUS, { requestId: payload.requestId, skillBuild: progress })
      }
    })
  })
}
