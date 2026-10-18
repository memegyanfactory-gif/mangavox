export type ThemePresetId =
  | 'rust'
  | 'watchfiwn'
  | 'mangavox-emerald'
  | 'chatgpt-green'
  | 'claude-warm'
  | 'gemini-prism'
  | 'amber-terminal'
  | 'cyberpunk'
  | 'sakura'
  | 'ocean'
  | 'monochrome'
  | 'aurora'

export type ParticleStyle =
  | 'line-rain'
  | 'linked-dots'
  | 'dust'
  | 'orbits'
  | 'data-rain'
  | 'streaks'
  | 'petals'
  | 'bubbles'
  | 'sparse-lines'
  | 'circular-flow'

export type ParticleIntensity = 'off' | 'low' | 'medium' | 'high'

export interface AppearanceConfig {
  preset: ThemePresetId
  particleIntensity: ParticleIntensity
  reduceMotion: boolean
}

export interface ThemePreset {
  id: ThemePresetId
  name: string
  description: string
  particleStyle: ParticleStyle
  colors: {
    base: string
    surface1: string
    surface2: string
    surface3: string
    surface4: string
    accent: string
    accentHover: string
    accentStrong: string
    accentDeep: string
    accentRgb: string
    secondary: string
    secondaryRgb: string
    text1: string
    text2: string
    text3: string
    particle: string[]
  }
}

export const DEFAULT_APPEARANCE: AppearanceConfig = {
  preset: 'rust',
  particleIntensity: 'low',
  reduceMotion: false,
}

export const THEME_PRESETS: ThemePreset[] = [
  {
    id: 'rust',
    name: 'Rust',
    description: 'Oxidized orange on warm charcoal — forged metal',
    particleStyle: 'dust',
    colors: {
      base: '#0e0907', surface1: '#16100d', surface2: '#1e1612', surface3: '#291d17', surface4: '#372820',
      accent: '#c8501a', accentHover: '#e8793f', accentStrong: '#a63f12', accentDeep: '#451a08', accentRgb: '200, 80, 26',
      secondary: '#f0b27a', secondaryRgb: '240, 178, 122',
      text1: '#fff6ee', text2: '#e0cdbf', text3: '#9a8173',
      particle: ['#c8501a', '#e8793f', '#f0b27a', '#a63f12'],
    },
  },
  {
    id: 'watchfiwn',
    name: 'Watchfiwn',
    description: 'Cinematic red on true black — streaming-grade',
    particleStyle: 'sparse-lines',
    colors: {
      base: '#0b0b0c', surface1: '#121214', surface2: '#1a1a1d', surface3: '#202024', surface4: '#2a2a2f',
      accent: '#e8392e', accentHover: '#ff4b3e', accentStrong: '#c01a12', accentDeep: '#7a0f09', accentRgb: '232, 57, 46',
      secondary: '#ff6b5f', secondaryRgb: '255, 107, 95',
      text1: '#f4f4f5', text2: '#c9c9cf', text3: '#8a8a90',
      particle: ['#e8392e', '#ff4b3e', '#ff6b5f', '#c01a12'],
    },
  },
  {
    id: 'mangavox-emerald',
    name: 'MangaVox Emerald',
    description: 'Emerald and cyan studio lighting',
    particleStyle: 'line-rain',
    colors: {
      base: '#07090d', surface1: '#0b0e12', surface2: '#10141b', surface3: '#181d26', surface4: '#232a36',
      accent: '#10b981', accentHover: '#34d399', accentStrong: '#059669', accentDeep: '#064e3b', accentRgb: '16, 185, 129',
      secondary: '#60a5fa', secondaryRgb: '96, 165, 250',
      text1: '#f8fafc', text2: '#cbd5e1', text3: '#7f8794',
      particle: ['#10b981', '#34d399', '#60a5fa', '#7dd3fc'],
    },
  },
  {
    id: 'chatgpt-green',
    name: 'ChatGPT Green',
    description: 'Charcoal, mint, and connected signals',
    particleStyle: 'linked-dots',
    colors: {
      base: '#080b0a', surface1: '#0d1210', surface2: '#121916', surface3: '#19221e', surface4: '#223029',
      accent: '#10a37f', accentHover: '#3ecfaf', accentStrong: '#0b8065', accentDeep: '#064b3d', accentRgb: '16, 163, 127',
      secondary: '#9ce6d1', secondaryRgb: '156, 230, 209',
      text1: '#f4f8f6', text2: '#c4d2cd', text3: '#788a83',
      particle: ['#10a37f', '#3ecfaf', '#9ce6d1'],
    },
  },
  {
    id: 'claude-warm',
    name: 'Claude Warm',
    description: 'Warm terracotta on deep charcoal',
    particleStyle: 'dust',
    colors: {
      base: '#0d0b0a', surface1: '#15110f', surface2: '#1c1714', surface3: '#27201c', surface4: '#332922',
      accent: '#d97757', accentHover: '#ee9a78', accentStrong: '#b85f43', accentDeep: '#663528', accentRgb: '217, 119, 87',
      secondary: '#e8c7a8', secondaryRgb: '232, 199, 168',
      text1: '#fff8f1', text2: '#dccdc1', text3: '#927f72',
      particle: ['#d97757', '#ee9a78', '#e8c7a8'],
    },
  },
  {
    id: 'gemini-prism',
    name: 'Gemini Prism',
    description: 'Blue, violet, and cyan refractions',
    particleStyle: 'orbits',
    colors: {
      base: '#080a12', surface1: '#0d1020', surface2: '#12172a', surface3: '#1a2138', surface4: '#252d49',
      accent: '#5b8def', accentHover: '#7db7ff', accentStrong: '#466bd6', accentDeep: '#263a7d', accentRgb: '91, 141, 239',
      secondary: '#a78bfa', secondaryRgb: '167, 139, 250',
      text1: '#f7f8ff', text2: '#c8d0e8', text3: '#7d88aa',
      particle: ['#5b8def', '#7dd3fc', '#a78bfa', '#f0abfc'],
    },
  },
  {
    id: 'amber-terminal',
    name: 'Amber Terminal',
    description: 'Black glass and restrained amber data',
    particleStyle: 'data-rain',
    colors: {
      base: '#090806', surface1: '#100e09', surface2: '#18140c', surface3: '#231d10', surface4: '#302716',
      accent: '#f59e0b', accentHover: '#fbbf24', accentStrong: '#d97706', accentDeep: '#78350f', accentRgb: '245, 158, 11',
      secondary: '#fde68a', secondaryRgb: '253, 230, 138',
      text1: '#fffbed', text2: '#e5d8b3', text3: '#918363',
      particle: ['#f59e0b', '#fbbf24', '#fde68a'],
    },
  },
  {
    id: 'cyberpunk',
    name: 'Cyberpunk',
    description: 'Cyan and magenta over graphite',
    particleStyle: 'streaks',
    colors: {
      base: '#07090c', surface1: '#0c1015', surface2: '#111820', surface3: '#18232d', surface4: '#22313d',
      accent: '#22d3ee', accentHover: '#67e8f9', accentStrong: '#0891b2', accentDeep: '#164e63', accentRgb: '34, 211, 238',
      secondary: '#f472b6', secondaryRgb: '244, 114, 182',
      text1: '#f5fcff', text2: '#c6d9df', text3: '#718a93',
      particle: ['#22d3ee', '#67e8f9', '#f472b6'],
    },
  },
  {
    id: 'sakura',
    name: 'Sakura',
    description: 'Rose highlights and drifting petals',
    particleStyle: 'petals',
    colors: {
      base: '#0d090c', surface1: '#150f14', surface2: '#1d151c', surface3: '#291d27', surface4: '#372633',
      accent: '#fb7185', accentHover: '#fda4af', accentStrong: '#e11d48', accentDeep: '#881337', accentRgb: '251, 113, 133',
      secondary: '#f9a8d4', secondaryRgb: '249, 168, 212',
      text1: '#fff7fa', text2: '#decbd2', text3: '#967984',
      particle: ['#fb7185', '#fda4af', '#f9a8d4'],
    },
  },
  {
    id: 'ocean',
    name: 'Ocean',
    description: 'Deep neutral water and clear cyan',
    particleStyle: 'bubbles',
    colors: {
      base: '#060b0e', surface1: '#0a1217', surface2: '#0e1a21', surface3: '#14252e', surface4: '#1d323d',
      accent: '#06b6d4', accentHover: '#67e8f9', accentStrong: '#0891b2', accentDeep: '#164e63', accentRgb: '6, 182, 212',
      secondary: '#38bdf8', secondaryRgb: '56, 189, 248',
      text1: '#f3fbfd', text2: '#c1d8de', text3: '#6e8c95',
      particle: ['#06b6d4', '#38bdf8', '#67e8f9'],
    },
  },
  {
    id: 'monochrome',
    name: 'Monochrome',
    description: 'Graphite, silver, and quiet motion',
    particleStyle: 'sparse-lines',
    colors: {
      base: '#090a0b', surface1: '#101214', surface2: '#17191c', surface3: '#202328', surface4: '#2b2f35',
      accent: '#b8c0cc', accentHover: '#e2e8f0', accentStrong: '#8993a1', accentDeep: '#3f4752', accentRgb: '184, 192, 204',
      secondary: '#7f8ea3', secondaryRgb: '127, 142, 163',
      text1: '#f8fafc', text2: '#cbd0d7', text3: '#7d838c',
      particle: ['#94a3b8', '#cbd5e1', '#ffffff'],
    },
  },
  {
    id: 'aurora',
    name: 'Aurora',
    description: 'Green, blue, and violet circular flow',
    particleStyle: 'circular-flow',
    colors: {
      base: '#070a0c', surface1: '#0b1114', surface2: '#10191d', surface3: '#172329', surface4: '#213038',
      accent: '#2dd4bf', accentHover: '#5eead4', accentStrong: '#0f9f91', accentDeep: '#134e4a', accentRgb: '45, 212, 191',
      secondary: '#818cf8', secondaryRgb: '129, 140, 248',
      text1: '#f4fbfa', text2: '#c6d9d6', text3: '#748b89',
      particle: ['#2dd4bf', '#38bdf8', '#818cf8', '#c084fc'],
    },
  },
]

export function getThemePreset(id: ThemePresetId): ThemePreset {
  return THEME_PRESETS.find((preset) => preset.id === id) ?? THEME_PRESETS[0]
}
