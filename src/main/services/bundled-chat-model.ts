import { app } from 'electron'
import { constants, copyFileSync, existsSync, mkdirSync, readFileSync } from 'fs'
import { homedir } from 'os'
import { dirname, join } from 'path'
import { AppPaths } from './app-paths'

export const DEFAULT_BUNDLED_CHAT_MODEL = 'dolphin3:8b'

let installation: Promise<boolean> | null = null

export function ensureBundledChatModel(): Promise<boolean> {
  if (!installation) installation = installBundledChatModel()
  return installation
}

async function installBundledChatModel(): Promise<boolean> {
  const bundleRoot = bundledModelsRoot()
  const metadataPath = join(bundleRoot, 'bundle.json')
  if (!existsSync(metadataPath)) return false

  try {
    const metadata = JSON.parse(readFileSync(metadataPath, 'utf-8')) as {
      model: string
      manifest: string
    }
    if (metadata.model !== DEFAULT_BUNDLED_CHAT_MODEL || !metadata.manifest) return false

    const sourceManifest = join(bundleRoot, metadata.manifest)
    if (!existsSync(sourceManifest)) return false

    const destinationRoot = ollamaModelsRoot()
    const destinationManifest = join(destinationRoot, metadata.manifest)
    const manifest = JSON.parse(readFileSync(sourceManifest, 'utf-8'))
    const entries = [manifest.config, ...(manifest.layers || [])]

    for (const entry of entries) {
      const fileName = String(entry.digest).replace(':', '-')
      const source = join(bundleRoot, 'blobs', fileName)
      const destination = join(destinationRoot, 'blobs', fileName)
      if (!existsSync(source)) throw new Error(`Bundled model blob is missing: ${fileName}`)
      if (!existsSync(destination)) {
        mkdirSync(dirname(destination), { recursive: true })
        copyFileSync(source, destination, constants.COPYFILE_FICLONE)
      }
    }

    if (!existsSync(destinationManifest)) {
      mkdirSync(dirname(destinationManifest), { recursive: true })
      copyFileSync(sourceManifest, destinationManifest)
    }

    console.log(`[bundled-model] ${DEFAULT_BUNDLED_CHAT_MODEL} registered with Ollama`)
    return true
  } catch (error) {
    console.warn('[bundled-model] Could not register bundled chat model:', error)
    return false
  }
}

function bundledModelsRoot(): string {
  return app.isPackaged
    ? join(process.resourcesPath, 'resources', 'models', 'ollama')
    : join(app.getAppPath(), 'resources', 'models', 'ollama')
}

function ollamaModelsRoot(): string {
  if (process.env.OLLAMA_MODELS) return process.env.OLLAMA_MODELS

  try {
    if (existsSync(AppPaths.config)) {
      const config = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
      if (typeof config?.ollama?.models_dir === 'string' && config.ollama.models_dir.trim()) {
        return config.ollama.models_dir.trim()
      }
    }
  } catch { /* use Ollama's default location */ }

  return join(homedir(), '.ollama', 'models')
}
