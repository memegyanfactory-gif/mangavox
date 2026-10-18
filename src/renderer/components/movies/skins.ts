import type { StreamingProvider } from '../../../shared/types'
import type { AnimeSearchResult } from '../../../shared/anime-addon-interface'

/**
 * Layout/colour "skin" per streaming service. Only layout language, palette and
 * typography patterns are replicated - no logos or brand imagery. Services are
 * labelled with their plain provider name ("MangaVox • Netflix catalog").
 */
export type SkinFamily = 'default' | 'netflix' | 'prime' | 'disney' | 'max' | 'hulu' | 'apple' | 'poster' | 'generic'
export type CardStyle = 'expand' | 'badge' | 'glow' | 'caption'
export type RankStyle = 'outline' | 'solid' | 'corner'

export interface Skin {
  family: SkinFamily
  label: string
  bg: string
  bgGradient?: string
  surface: string
  accent: string
  accentFg: string
  text: string
  muted: string
  border: string
  radius: number
  card: CardStyle
  cardW: number
  aspect: string
  rank: RankStyle
  heroH: string
  heroOverlay: string
  playLabel: string
  infoLabel: string
  featured: string
  top10Title: (kind: 'movie' | 'series') => string
  trendingTitle: string
  wordmarkStyle: { fontWeight: number; letterSpacing: string; fontStyle?: string; textTransform?: 'uppercase' }
  pill: boolean
  badge: (item: AnimeSearchResult, index: number) => string | null
}

const base = {
  border: 'rgba(255,255,255,0.08)',
  text: '#ffffff',
  muted: 'rgba(255,255,255,0.6)',
}

export const DEFAULT_SKIN: Skin = {
  family: 'default',
  label: 'MangaVox Cinema',
  bg: 'var(--base)',
  surface: 'var(--surface-2)',
  accent: 'var(--accent)',
  accentFg: '#ffffff',
  text: 'var(--text-1)',
  muted: 'var(--text-3)',
  border: 'var(--panel-border)',
  radius: 12,
  card: 'caption',
  cardW: 150,
  aspect: 'aspect-[2/3]',
  rank: 'outline',
  heroH: 'h-[clamp(260px,44vh,400px)]',
  heroOverlay: 'linear-gradient(90deg, var(--base) 0%, color-mix(in srgb, var(--base) 78%, transparent) 36%, transparent 72%), linear-gradient(0deg, var(--base) 0%, transparent 46%)',
  playLabel: 'Play',
  infoLabel: 'More info',
  featured: 'Featured tonight',
  top10Title: (k) => `Top 10 ${k === 'movie' ? 'movies' : 'series'} this week`,
  trendingTitle: 'Trending now',
  wordmarkStyle: { fontWeight: 800, letterSpacing: '-0.02em' },
  pill: false,
  badge: (_i, idx) => (idx === 0 ? 'Hot' : null),
}

function dark(bg: string, extra: Partial<Skin>): Skin {
  return { ...DEFAULT_SKIN, ...base, bg, ...extra }
}

export function getSkin(provider: StreamingProvider | undefined): Skin {
  if (!provider) return DEFAULT_SKIN
  const id = provider.id
  const label = `MangaVox • ${provider.name} catalog`
  const accent = provider.theme.accent

  if (id === 'netflix') {
    return dark('#080808', {
      family: 'netflix', label, surface: '#181818', accent: '#e50914', accentFg: '#fff', radius: 4,
      card: 'expand', cardW: 232, aspect: 'aspect-video', rank: 'outline',
      heroH: 'h-[clamp(300px,52vh,460px)]',
      heroOverlay: 'linear-gradient(77deg, rgba(0,0,0,.85) 0%, rgba(0,0,0,.45) 40%, transparent 70%), linear-gradient(0deg, #080808 0%, transparent 38%)',
      playLabel: 'Play', infoLabel: 'More Info', featured: 'Featured',
      top10Title: (k) => `Top 10 ${k === 'movie' ? 'Movies' : 'TV Shows'} This Week`,
      trendingTitle: 'Trending Now',
      wordmarkStyle: { fontWeight: 900, letterSpacing: '-0.06em', textTransform: 'uppercase' },
      badge: (_i, idx) => (idx < 10 ? 'TOP 10' : null),
    })
  }
  if (id === 'prime-video') {
    return dark('#0f171e', {
      family: 'prime', label, surface: '#1a242f', accent: '#00a8e1', accentFg: '#fff', radius: 8,
      card: 'badge', cardW: 250, aspect: 'aspect-video', rank: 'corner',
      heroH: 'h-[clamp(300px,50vh,440px)]',
      heroOverlay: 'linear-gradient(90deg, #0f171e 2%, rgba(15,23,30,.78) 38%, transparent 74%), linear-gradient(0deg, #0f171e 0%, transparent 44%)',
      playLabel: 'Watch now', infoLabel: 'More details', featured: 'Featured',
      top10Title: (k) => `Top 10 ${k === 'movie' ? 'movies' : 'TV'} in your region`,
      trendingTitle: 'Picks for you',
      wordmarkStyle: { fontWeight: 800, letterSpacing: '-0.02em', fontStyle: 'italic' },
      badge: () => 'Included with Prime',
    })
  }
  if (id === 'jiohotstar' || /disney/.test(id)) {
    return dark('#0b0d1a', {
      family: 'disney', label, surface: '#171a30', accent: '#1f80e0', accentFg: '#fff', radius: 14,
      bgGradient: 'radial-gradient(ellipse at 50% -10%, #1b2c6b 0%, #0b0d1a 58%)',
      card: 'glow', cardW: 232, aspect: 'aspect-video', rank: 'outline', pill: true,
      heroH: 'h-[clamp(300px,50vh,440px)]',
      heroOverlay: 'linear-gradient(90deg, #0b0d1a 0%, rgba(11,13,26,.75) 36%, transparent 72%), linear-gradient(0deg, #0b0d1a 0%, transparent 42%)',
      playLabel: 'Watch now', infoLabel: 'Details', featured: 'Spotlight',
      top10Title: (k) => `Top 10 ${k === 'movie' ? 'movies' : 'shows'} today`,
      trendingTitle: 'Trending today',
      wordmarkStyle: { fontWeight: 700, letterSpacing: '0.01em' },
      badge: (_i, idx) => (idx < 3 ? 'New' : null),
    })
  }
  if (id === 'max' || id === 'hbo-max') {
    return dark('#0a0a14', {
      family: 'max', label, surface: '#17172a', accent: '#7b3ff2', accentFg: '#fff', radius: 8,
      bgGradient: 'linear-gradient(180deg, #14103a 0%, #0a0a14 380px)',
      card: 'caption', cardW: 240, aspect: 'aspect-video', rank: 'solid',
      heroH: 'h-[clamp(300px,50vh,440px)]',
      heroOverlay: 'linear-gradient(90deg, rgba(10,10,20,.92) 0%, rgba(10,10,20,.5) 40%, transparent 72%), linear-gradient(0deg, #0a0a14 0%, transparent 40%)',
      playLabel: 'Watch', infoLabel: 'Details', featured: 'Now streaming',
      top10Title: (k) => `Top ${k === 'movie' ? 'movies' : 'series'} this week`,
      trendingTitle: 'Popular now',
      wordmarkStyle: { fontWeight: 800, letterSpacing: '-0.03em' },
      badge: () => null,
    })
  }
  if (id === 'hulu') {
    return dark('#0b0c0f', {
      family: 'hulu', label, surface: '#1a1d23', accent: '#1ce783', accentFg: '#04140b', radius: 6,
      card: 'caption', cardW: 232, aspect: 'aspect-video', rank: 'solid',
      heroH: 'h-[clamp(300px,50vh,440px)]',
      heroOverlay: 'linear-gradient(90deg, rgba(11,12,15,.95) 0%, rgba(11,12,15,.55) 40%, transparent 70%), linear-gradient(0deg, #0b0c0f 0%, transparent 40%)',
      playLabel: 'Start watching', infoLabel: 'Details', featured: 'Featured',
      top10Title: (k) => `Top 10 ${k === 'movie' ? 'movies' : 'shows'}`,
      trendingTitle: 'Popular right now',
      wordmarkStyle: { fontWeight: 800, letterSpacing: '-0.02em' },
      badge: (_i, idx) => (idx < 4 ? 'New' : null),
    })
  }
  if (id === 'apple-tv') {
    return dark('#000000', {
      family: 'apple', label, surface: '#1c1c1e', accent: '#f5f5f7', accentFg: '#000', radius: 14,
      card: 'caption', cardW: 280, aspect: 'aspect-[16/10]', rank: 'corner', pill: true,
      heroH: 'h-[clamp(320px,54vh,480px)]',
      heroOverlay: 'linear-gradient(0deg, #000 0%, rgba(0,0,0,.35) 42%, transparent 72%)',
      playLabel: 'Play', infoLabel: 'Details', featured: 'Featured',
      top10Title: () => 'Top picks this week',
      trendingTitle: 'Just added',
      wordmarkStyle: { fontWeight: 600, letterSpacing: '-0.01em' },
      badge: () => null,
    })
  }
  if (id === 'mubi') {
    return dark('#151515', {
      family: 'poster', label, surface: '#262626', accent: '#ffffff', accentFg: '#000', radius: 0,
      card: 'caption', cardW: 160, aspect: 'aspect-[2/3]', rank: 'solid',
      heroOverlay: 'linear-gradient(90deg, #151515 0%, rgba(21,21,21,.7) 40%, transparent 72%), linear-gradient(0deg, #151515 0%, transparent 44%)',
      playLabel: 'Watch', infoLabel: 'Details', featured: 'Film of the day',
      top10Title: () => 'Top 10 this week', trendingTitle: 'Now showing',
      wordmarkStyle: { fontWeight: 700, letterSpacing: '0.04em', textTransform: 'uppercase' },
      badge: () => null,
    })
  }
  // Every other service: generic dark layout tinted with its own palette.
  return dark(provider.theme.background, {
    family: 'generic', label, surface: provider.theme.surface, accent, accentFg: '#fff',
    radius: provider.theme.cardRadius, card: 'glow', cardW: 232, aspect: 'aspect-video',
    heroOverlay: `linear-gradient(90deg, ${provider.theme.background} 0%, ${provider.theme.background}cc 38%, transparent 72%), linear-gradient(0deg, ${provider.theme.background} 0%, transparent 44%)`,
    playLabel: 'Watch now', infoLabel: 'Details', featured: 'Featured',
    top10Title: (k) => `Top 10 ${k === 'movie' ? 'movies' : 'shows'}`,
    trendingTitle: 'Popular now',
    badge: () => null,
  })
}

export const titleKey = (kind: 'movie' | 'series', item: AnimeSearchResult): string =>
  item.imdbId ? `${kind}:imdb:${item.imdbId}` : `${kind}:tmdb:${item.tmdbId ?? item.id}`
