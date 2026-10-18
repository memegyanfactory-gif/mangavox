import type { MediaEventType } from './types'

export function mediaEventWeight(type: MediaEventType): number {
  switch (type) {
    case 'complete':
    case 'like':
      return 6
    case 'progress':
      return 3
    case 'play':
      return 2
    case 'open':
    case 'search':
      return 1
    case 'skip':
      return -2
    case 'dislike':
      return -8
  }
}

export function normalizedMediaKey(kind: 'anime' | 'movie' | 'series', sourceId: string): string {
  const source = kind === 'anime' ? 'mal' : 'imdb'
  return `${kind}:${source}:${sourceId}`
}
