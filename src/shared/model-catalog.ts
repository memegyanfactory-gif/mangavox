import type {
  AIHardwareTier,
  AIModelPreferences,
  CompatibleModel,
  ModelCatalogEntry,
  SystemProfile,
} from './types'

export const DEFAULT_AI_MODEL_PREFERENCES: AIModelPreferences = {
  visionModel: '',
  textModel: '',
  autoSelect: true,
  migratedLegacy: false,
}

export const OLLAMA_MODEL_CATALOG: ModelCatalogEntry[] = [
  {
    name: 'qwen3-vl:2b', label: 'Qwen3-VL 2B', roles: ['vision'], downloadGb: 1.9,
    minMemoryGb: 6, recommendedMemoryGb: 8, vision: true, multilingual: true, hindi: true, quality: 72,
    description: 'Light multilingual vision model for manga OCR and page understanding.',
    sourceUrl: 'https://ollama.com/library/qwen3-vl',
  },
  {
    name: 'qwen3-vl:4b', label: 'Qwen3-VL 4B', roles: ['vision'], downloadGb: 3.3,
    minMemoryGb: 8, recommendedMemoryGb: 12, vision: true, multilingual: true, hindi: true, quality: 84,
    description: 'Balanced multilingual manga vision model with strong OCR.',
    sourceUrl: 'https://ollama.com/library/qwen3-vl',
  },
  {
    name: 'qwen3-vl:8b', label: 'Qwen3-VL 8B', roles: ['vision'], downloadGb: 6.1,
    minMemoryGb: 12, recommendedMemoryGb: 16, vision: true, multilingual: true, hindi: true, quality: 94,
    description: 'High-quality multilingual vision and detailed page understanding.',
    sourceUrl: 'https://ollama.com/library/qwen3-vl',
  },
  {
    name: 'minicpm-v:8b', label: 'MiniCPM-V 8B', roles: ['vision'], downloadGb: 5.5,
    minMemoryGb: 12, recommendedMemoryGb: 16, vision: true, multilingual: true, hindi: false, quality: 86,
    description: 'Accurate compact vision model, especially effective at dense text.',
    sourceUrl: 'https://ollama.com/library/minicpm-v',
  },
  {
    name: 'gemma3:4b', label: 'Gemma 3 4B', roles: ['vision', 'text'], downloadGb: 3.3,
    minMemoryGb: 8, recommendedMemoryGb: 12, vision: true, multilingual: true, hindi: true, quality: 80,
    description: 'Efficient multimodal model that can cover both vision and writing.',
    sourceUrl: 'https://ollama.com/library/gemma3',
  },
  {
    name: 'gemma3:12b', label: 'Gemma 3 12B', roles: ['vision', 'text'], downloadGb: 8.1,
    minMemoryGb: 16, recommendedMemoryGb: 24, vision: true, multilingual: true, hindi: true, quality: 92,
    description: 'Premium multimodal quality for high-memory systems.',
    sourceUrl: 'https://ollama.com/library/gemma3',
  },
  {
    name: 'moondream:1.8b', label: 'Moondream 1.8B', roles: ['vision'], downloadGb: 1.7,
    minMemoryGb: 6, recommendedMemoryGb: 8, vision: true, multilingual: false, hindi: false, quality: 56,
    description: 'Fast preview model for systems with limited memory.',
    sourceUrl: 'https://ollama.com/library/moondream',
  },
  {
    name: 'qwen3:1.7b', label: 'Qwen3 1.7B', roles: ['text'], downloadGb: 1.4,
    minMemoryGb: 5, recommendedMemoryGb: 8, vision: false, multilingual: true, hindi: true, quality: 66,
    description: 'Small multilingual writer for basic chat and narration.',
    sourceUrl: 'https://ollama.com/library/qwen3',
  },
  {
    name: 'llama3.2:3b', label: 'Llama 3.2 3B', roles: ['text'], downloadGb: 2.0,
    minMemoryGb: 6, recommendedMemoryGb: 8, vision: false, multilingual: true, hindi: false, quality: 70,
    description: 'Compact general assistant for English chat and writing.',
    sourceUrl: 'https://ollama.com/library/llama3.2',
  },
  {
    name: 'qwen3:4b', label: 'Qwen3 4B', roles: ['text'], downloadGb: 2.6,
    minMemoryGb: 7, recommendedMemoryGb: 10, vision: false, multilingual: true, hindi: true, quality: 80,
    description: 'Balanced multilingual narration and assistant model.',
    sourceUrl: 'https://ollama.com/library/qwen3',
  },
  {
    name: 'qwen3:8b', label: 'Qwen3 8B', roles: ['text'], downloadGb: 5.2,
    minMemoryGb: 12, recommendedMemoryGb: 16, vision: false, multilingual: true, hindi: true, quality: 91,
    description: 'Strong multilingual writer for polished Hindi and English narration.',
    sourceUrl: 'https://ollama.com/library/qwen3',
  },
  {
    name: 'dolphin3:8b', label: 'Dolphin 3 8B', roles: ['text'], downloadGb: 4.9,
    minMemoryGb: 12, recommendedMemoryGb: 16, vision: false, multilingual: false, hindi: false, quality: 78,
    description: 'General local assistant with broad conversational coverage.',
    sourceUrl: 'https://ollama.com/library/dolphin3',
  },
  {
    name: 'hf.co/bartowski/L3-8B-Stheno-v3.2-GGUF:Q4_K_M',
    label: 'Stheno 3.2 8B Roleplay', roles: ['text'], downloadGb: 4.9,
    minMemoryGb: 12, recommendedMemoryGb: 16, vision: false, multilingual: false, hindi: false, quality: 88,
    description: 'Llama 3 roleplay fine-tune for expressive one-on-one character and adult chat.',
    sourceUrl: 'https://huggingface.co/bartowski/L3-8B-Stheno-v3.2-GGUF',
  },
]

export function hardwareTier(ramGb: number, availableModelMemoryGb: number): AIHardwareTier {
  const usable = Math.min(ramGb, availableModelMemoryGb)
  if (usable >= 24) return 'workstation'
  if (usable >= 14) return 'performance'
  if (usable >= 8) return 'standard'
  return 'basic'
}

export function modelBaseName(name: string): string {
  return name.trim().toLowerCase().split(':')[0]
}

export function isInstalledModel(name: string, installed: string[]): boolean {
  const exact = name.toLowerCase()
  const base = modelBaseName(name)
  return installed.some((candidate) => {
    const lower = candidate.toLowerCase()
    if (lower === exact || lower.startsWith(`${exact}-`)) return true
    // Catalog entries without an explicit tag represent the family default.
    return !exact.includes(':') && modelBaseName(lower) === base
  })
}

export function compatibleModels(profile: SystemProfile, installed: string[] = []): CompatibleModel[] {
  return OLLAMA_MODEL_CATALOG
    .map((model) => {
      const compatible = profile.availableModelMemoryGb >= model.minMemoryGb
      const comfortable = profile.availableModelMemoryGb >= model.recommendedMemoryGb
      return {
        ...model,
        compatible,
        recommended: compatible && comfortable,
        installed: isInstalledModel(model.name, installed),
        reason: !compatible
          ? `Needs at least ${model.minMemoryGb} GB usable memory`
          : comfortable
            ? `Recommended for this ${profile.ramGb} GB system`
            : `Compatible, but ${model.recommendedMemoryGb} GB is recommended`,
      }
    })
    .sort((a, b) => {
      if (a.compatible !== b.compatible) return a.compatible ? -1 : 1
      if (a.installed !== b.installed) return a.installed ? -1 : 1
      return b.quality - a.quality
    })
}

export function bestModelForRole(
  role: 'vision' | 'text',
  profile: SystemProfile,
  installed: string[],
  requireHindi = false,
): string {
  const candidates = compatibleModels(profile, installed)
    .filter((model) => model.roles.includes(role) && model.installed && (!requireHindi || model.hindi))
    .sort((a, b) => b.quality - a.quality)
  return candidates[0]?.name || ''
}
