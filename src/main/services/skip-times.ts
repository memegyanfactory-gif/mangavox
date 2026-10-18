import { ipcMain, net } from 'electron'
import { IPC } from '../../shared/ipc-channels'

export type SkipType = 'op' | 'ed' | 'recap' | 'mixed-op' | 'mixed-ed'

export interface SkipSegment {
  type: SkipType
  start: number
  end: number
}

export interface SkipTimesResult {
  found: boolean
  segments: SkipSegment[]
}

const EMPTY: SkipTimesResult = { found: false, segments: [] }
const TTL_OK = 24 * 60 * 60 * 1000
const TTL_EMPTY = 10 * 60 * 1000
const cache = new Map<string, { at: number; ttl: number; value: SkipTimesResult }>()
const VALID: SkipType[] = ['op', 'ed', 'recap', 'mixed-op', 'mixed-ed']

/** Fetch intro/outro/recap ranges from the public AniSkip API. Never throws. */
export async function getSkipTimes(malId: number, episode: number, episodeLength = 0): Promise<SkipTimesResult> {
  if (!Number.isFinite(malId) || malId <= 0 || !Number.isFinite(episode) || episode <= 0) return EMPTY
  const len = Math.max(0, Math.round(episodeLength || 0))
  const key = `${malId}:${episode}:${Math.round(len / 5)}`
  const hit = cache.get(key)
  if (hit && Date.now() - hit.at < hit.ttl) return hit.value

  try {
    const qs = [...VALID.map((t) => `types=${t}`), `episodeLength=${len}`].join('&')
    const res = await net.fetch(`https://api.aniskip.com/v2/skip-times/${malId}/${Math.floor(episode)}?${qs}`, {
      headers: { Accept: 'application/json' },
      signal: AbortSignal.timeout(8000),
    })
    if (res.status === 404) {
      cache.set(key, { at: Date.now(), ttl: TTL_EMPTY, value: EMPTY })
      return EMPTY
    }
    if (!res.ok) return EMPTY
    const body: any = await res.json()
    const segments: SkipSegment[] = (Array.isArray(body?.results) ? body.results : [])
      .map((r: any): SkipSegment => ({
        type: r?.skipType,
        start: Number(r?.interval?.startTime),
        end: Number(r?.interval?.endTime),
      }))
      .filter((s: SkipSegment) => VALID.includes(s.type) && Number.isFinite(s.start) && Number.isFinite(s.end) && s.end > s.start)
      .sort((a: SkipSegment, b: SkipSegment) => a.start - b.start)
    const value: SkipTimesResult = { found: segments.length > 0, segments }
    cache.set(key, { at: Date.now(), ttl: value.found ? TTL_OK : TTL_EMPTY, value })
    return value
  } catch (err: any) {
    console.warn(`[skip-times] ${err?.message || err}`)
    return EMPTY
  }
}

export function registerSkipTimesHandlers(): void {
  ipcMain.handle(IPC.SKIP_TIMES, async (_e, malId: number, episode: number, episodeLength?: number) => {
    return getSkipTimes(Number(malId), Number(episode), Number(episodeLength) || 0)
  })
}
