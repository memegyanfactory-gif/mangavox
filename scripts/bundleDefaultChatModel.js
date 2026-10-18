const { constants, copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } = require('fs')
const { homedir } = require('os')
const { dirname, join } = require('path')

const MODEL = {
  name: 'dolphin3:8b',
  registryPath: ['registry.ollama.ai', 'library', 'dolphin3', '8b'],
}

const ollamaModels = process.env.OLLAMA_MODELS || join(homedir(), '.ollama', 'models')
const sourceManifest = join(ollamaModels, 'manifests', ...MODEL.registryPath)
const bundleRoot = join(__dirname, '..', 'resources', 'models', 'ollama')
const destinationManifest = join(bundleRoot, 'manifests', ...MODEL.registryPath)

if (!existsSync(sourceManifest)) {
  console.error(`[bundle-model] ${MODEL.name} is not installed in Ollama.`)
  console.error(`[bundle-model] Run "ollama pull ${MODEL.name}" once on the build machine.`)
  process.exit(1)
}

const manifest = JSON.parse(readFileSync(sourceManifest, 'utf-8'))
const entries = [manifest.config, ...(manifest.layers || [])]

mkdirSync(dirname(destinationManifest), { recursive: true })
writeFileSync(destinationManifest, JSON.stringify(manifest), 'utf-8')

for (const entry of entries) {
  const fileName = entry.digest.replace(':', '-')
  const source = join(ollamaModels, 'blobs', fileName)
  const destination = join(bundleRoot, 'blobs', fileName)
  if (!existsSync(source)) {
    console.error(`[bundle-model] Missing Ollama blob: ${source}`)
    process.exit(1)
  }
  mkdirSync(dirname(destination), { recursive: true })
  if (!existsSync(destination)) {
    console.log(`[bundle-model] Adding ${fileName} (${entry.size} bytes)`)
    copyFileSync(source, destination, constants.COPYFILE_FICLONE)
  }
}

writeFileSync(join(bundleRoot, 'bundle.json'), JSON.stringify({
  model: MODEL.name,
  manifest: join('manifests', ...MODEL.registryPath),
  source: 'https://ollama.com/library/dolphin3',
  bundledAt: new Date().toISOString(),
}, null, 2))

console.log(`[bundle-model] ${MODEL.name} is ready in resources/models/ollama`)
