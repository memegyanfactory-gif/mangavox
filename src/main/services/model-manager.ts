import { BrowserWindow } from 'electron'
import { execFileSync } from 'child_process'
import { cpus, arch, platform, totalmem } from 'os'
import { existsSync, readFileSync, writeFileSync } from 'fs'
import { AppPaths } from './app-paths'
import {
  DEFAULT_AI_MODEL_PREFERENCES,
  bestModelForRole,
  compatibleModels,
  hardwareTier,
} from '../../shared/model-catalog'
import type { AIModelPreferences, SystemProfile } from '../../shared/types'
import { IPC } from '../../shared/ipc-channels'

function command(command: string, args: string[]): string {
  try {
    return execFileSync(command, args, { encoding: 'utf8', timeout: 5000 }).trim()
  } catch {
    return ''
  }
}

function detectGpu(): { gpu: string; vendor: SystemProfile['gpuVendor']; vramGb: number | null; unified: boolean } {
  if (platform() === 'darwin') {
    const output = command('system_profiler', ['SPDisplaysDataType', '-json'])
    const names = Array.from(output.matchAll(/"sppci_model"\s*:\s*"([^"]+)"/g)).map((match) => match[1])
    const gpu = names.join(', ') || (arch() === 'arm64' ? 'Apple integrated GPU' : 'Unknown GPU')
    return { gpu, vendor: gpu.toLowerCase().includes('apple') ? 'apple' : 'unknown', vramGb: null, unified: arch() === 'arm64' }
  }

  const nvidia = command('nvidia-smi', ['--query-gpu=name,memory.total', '--format=csv,noheader,nounits'])
  if (nvidia) {
    const [first] = nvidia.split('\n')
    const parts = first.split(',').map((value) => value.trim())
    return { gpu: parts[0], vendor: 'nvidia', vramGb: Number(parts[1]) / 1024, unified: false }
  }

  if (platform() === 'win32') {
    const output = command('powershell.exe', ['-NoProfile', '-Command', 'Get-CimInstance Win32_VideoController | Select-Object -First 1 Name,AdapterRAM | ConvertTo-Json -Compress'])
    try {
      const parsed = JSON.parse(output)
      const gpu = String(parsed.Name || 'Unknown GPU')
      const lower = gpu.toLowerCase()
      const vendor = lower.includes('amd') || lower.includes('radeon') ? 'amd' : lower.includes('intel') ? 'intel' : 'unknown'
      return { gpu, vendor, vramGb: parsed.AdapterRAM ? Number(parsed.AdapterRAM) / 1024 ** 3 : null, unified: vendor === 'intel' }
    } catch { /* fall through */ }
  }

  const linuxGpu = command('sh', ['-c', "lspci 2>/dev/null | grep -Ei 'vga|3d|display' | head -1"])
  const lower = linuxGpu.toLowerCase()
  const vendor = lower.includes('amd') ? 'amd' : lower.includes('intel') ? 'intel' : 'unknown'
  return { gpu: linuxGpu || 'Unknown GPU', vendor, vramGb: null, unified: vendor === 'intel' }
}

let cachedProfile: SystemProfile | null = null

export function getSystemProfile(): SystemProfile {
  if (cachedProfile) return cachedProfile
  const ramGb = Math.round((totalmem() / 1024 ** 3) * 10) / 10
  const gpu = detectGpu()
  const sharedBudget = Math.max(3, ramGb - (ramGb <= 8 ? 3 : ramGb <= 16 ? 5 : 7))
  const dedicatedBudget = gpu.vramGb ? Math.max(2, gpu.vramGb - 1.5) : sharedBudget
  const availableModelMemoryGb = Math.round((gpu.unified ? sharedBudget : Math.min(sharedBudget, dedicatedBudget)) * 10) / 10
  cachedProfile = {
    platform: platform(),
    arch: arch(),
    cpu: cpus()[0]?.model || 'Unknown CPU',
    cpuCores: cpus().length,
    ramGb,
    gpu: gpu.gpu,
    gpuVendor: gpu.vendor,
    vramGb: gpu.vramGb ? Math.round(gpu.vramGb * 10) / 10 : null,
    unifiedMemory: gpu.unified,
    availableModelMemoryGb,
    tier: hardwareTier(ramGb, availableModelMemoryGb),
  }
  return cachedProfile
}

function readConfig(): Record<string, any> {
  try {
    return existsSync(AppPaths.config) ? JSON.parse(readFileSync(AppPaths.config, 'utf8')) : {}
  } catch {
    return {}
  }
}

function writeConfig(config: Record<string, any>): void {
  writeFileSync(AppPaths.config, JSON.stringify(config, null, 2))
}

export function getAIModelPreferences(installed: string[] = []): AIModelPreferences {
  const config = readConfig()
  const saved = { ...DEFAULT_AI_MODEL_PREFERENCES, ...(config.aiModels || {}) }
  const profile = getSystemProfile()
  if (saved.autoSelect) {
    if (!saved.visionModel || !installed.some((name) => name === saved.visionModel)) {
      saved.visionModel = bestModelForRole('vision', profile, installed)
    }
    if (!saved.textModel || !installed.some((name) => name === saved.textModel)) {
      saved.textModel = bestModelForRole('text', profile, installed)
    }
  }
  return saved
}

export function setAIModelPreferences(next: Partial<AIModelPreferences>): AIModelPreferences {
  const config = readConfig()
  const current = { ...DEFAULT_AI_MODEL_PREFERENCES, ...(config.aiModels || {}) }
  const preferences = { ...current, ...next }
  config.aiModels = preferences
  config.ollama = {
    ...(config.ollama || {}),
    ...(preferences.visionModel ? { vision_model: preferences.visionModel } : {}),
    ...(preferences.textModel ? { text_model: preferences.textModel } : {}),
  }
  writeConfig(config)
  broadcastModelChange(preferences)
  return preferences
}

export function getCompatibleCatalog(installed: string[] = []) {
  return compatibleModels(getSystemProfile(), installed)
}

export function broadcastModelChange(payload: unknown = null): void {
  for (const window of BrowserWindow.getAllWindows()) {
    if (!window.isDestroyed()) window.webContents.send(IPC.AI_MODELS_CHANGED, payload)
  }
}
