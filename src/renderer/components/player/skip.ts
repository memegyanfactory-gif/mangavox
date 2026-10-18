export type SkipKind = 'intro' | 'recap' | 'credits'
export interface SkipSegment { type: 'op' | 'ed' | 'recap' | 'mixed-op' | 'mixed-ed'; start: number; end: number }
export interface SkipTimesResult { found: boolean; segments: SkipSegment[] }

export function kindOf(type: SkipSegment['type']): SkipKind {
  if (type === 'recap') return 'recap'
  return type === 'ed' || type === 'mixed-ed' ? 'credits' : 'intro'
}
export const SKIP_LABEL: Record<SkipKind, string> = { intro: 'Skip Intro', recap: 'Skip Recap', credits: 'Skip Credits' }
export const SKIP_COLOR: Record<SkipKind, string> = { intro: '#f0b429', recap: '#5aa9e6', credits: '#e5484d' }
