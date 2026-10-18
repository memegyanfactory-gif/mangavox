import { create } from 'zustand'
import {
  DEFAULT_APPEARANCE,
  getThemePreset,
  type AppearanceConfig,
  type ParticleIntensity,
  type ThemePresetId,
} from '../../shared/appearance'
import { IPC } from '../../shared/ipc-channels'
import { invoke } from '../hooks/useIpc'

interface AppearanceState extends AppearanceConfig {
  hydrated: boolean
  hydrate: () => Promise<void>
  setPreset: (preset: ThemePresetId) => Promise<void>
  setParticleIntensity: (particleIntensity: ParticleIntensity) => Promise<void>
  setReduceMotion: (reduceMotion: boolean) => Promise<void>
}

function applyAppearance(config: AppearanceConfig): void {
  const preset = getThemePreset(config.preset)
  const root = document.documentElement
  root.dataset.theme = preset.id
  root.dataset.particles = config.particleIntensity
  root.dataset.reduceMotion = config.reduceMotion ? 'true' : 'false'

  const c = preset.colors
  const vars: Record<string, string> = {
    '--base': c.base,
    '--surface-1': `color-mix(in srgb, ${c.surface1} 90%, transparent)`,
    '--surface-2': `color-mix(in srgb, ${c.surface2} 82%, transparent)`,
    '--surface-3': `color-mix(in srgb, ${c.surface3} 78%, transparent)`,
    '--surface-4': `color-mix(in srgb, ${c.surface4} 82%, transparent)`,
    '--accent': c.accent,
    '--accent-hover': c.accentHover,
    '--accent-strong': c.accentStrong,
    '--accent-deep': c.accentDeep,
    '--accent-rgb': c.accentRgb,
    '--secondary': c.secondary,
    '--secondary-rgb': c.secondaryRgb,
    '--accent-glow': `rgba(${c.accentRgb}, 0.30)`,
    '--accent-glow-soft': `rgba(${c.accentRgb}, 0.12)`,
    '--accent-gradient': `linear-gradient(135deg, ${c.accent} 0%, ${c.accentHover} 55%, ${c.secondary} 100%)`,
    '--accent-gradient-dim': `linear-gradient(135deg, rgba(${c.accentRgb}, 0.18), rgba(${c.secondaryRgb}, 0.08))`,
    '--accent-gradient-hot': `linear-gradient(90deg, ${c.accent}, ${c.secondary}, ${c.accentHover})`,
    '--text-1': c.text1,
    '--text-2': c.text2,
    '--text-3': c.text3,
    '--panel-border': `rgba(${c.accentRgb}, 0.12)`,
    '--panel-border-strong': `rgba(${c.accentRgb}, 0.28)`,
    '--theme-bg-a': `rgba(${c.accentRgb}, 0.16)`,
    '--theme-bg-b': `rgba(${c.secondaryRgb}, 0.10)`,
  }
  Object.entries(vars).forEach(([key, value]) => root.style.setProperty(key, value))
  // Keep the native window controls on the same colour as the top bar
  void invoke(IPC.WINDOW_SET_OVERLAY, { color: c.base, symbolColor: c.accentHover }).catch(() => {})
}

async function persist(config: AppearanceConfig): Promise<void> {
  await invoke(IPC.CONFIG_SET, 'appearance', config)
}

export const useAppearanceStore = create<AppearanceState>((set, get) => ({
  ...DEFAULT_APPEARANCE,
  hydrated: false,
  hydrate: async () => {
    let next = DEFAULT_APPEARANCE
    try {
      const saved = await invoke<Partial<AppearanceConfig> | null>(IPC.CONFIG_GET, 'appearance')
      next = {
        preset: saved?.preset && getThemePreset(saved.preset).id === saved.preset ? saved.preset : DEFAULT_APPEARANCE.preset,
        particleIntensity: saved?.particleIntensity === 'off' || saved?.particleIntensity === 'medium' || saved?.particleIntensity === 'high'
          ? saved.particleIntensity
          : 'low',
        reduceMotion: !!saved?.reduceMotion,
      }
    } catch { /* defaults remain usable */ }
    applyAppearance(next)
    set({ ...next, hydrated: true })
  },
  setPreset: async (preset) => {
    const next = { preset, particleIntensity: get().particleIntensity, reduceMotion: get().reduceMotion }
    applyAppearance(next)
    set({ preset })
    await persist(next)
  },
  setParticleIntensity: async (particleIntensity) => {
    const next = { preset: get().preset, particleIntensity, reduceMotion: get().reduceMotion }
    applyAppearance(next)
    set({ particleIntensity })
    await persist(next)
  },
  setReduceMotion: async (reduceMotion) => {
    const next = { preset: get().preset, particleIntensity: get().particleIntensity, reduceMotion }
    applyAppearance(next)
    set({ reduceMotion })
    await persist(next)
  },
}))
