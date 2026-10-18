import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import {
  enableProxy, disableProxy, isProxyEnabled,
  getActiveProxy, getPublicIp,
} from '../services/proxy-service'

export function registerPrivacyHandlers(): void {
  ipcMain.handle(IPC.PRIVACY_TOGGLE, async (_e, enable: boolean) => {
    if (enable) {
      return await enableProxy()
    } else {
      await disableProxy()
      return { ok: true, enabled: false }
    }
  })

  ipcMain.handle(IPC.PRIVACY_STATUS, () => {
    const proxy = getActiveProxy()
    return {
      enabled: isProxyEnabled(),
      proxyAddr: proxy ? `${proxy.host}:${proxy.port}` : null,
    }
  })

  ipcMain.handle(IPC.PRIVACY_CHECK_IP, async () => {
    const ip = await getPublicIp()
    return { ip }
  })
}
