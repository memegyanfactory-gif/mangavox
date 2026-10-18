import { randomUUID } from 'crypto'
import { getDb } from '../db/database'
import {
  getStreamServerBaseUrl,
  probeActiveMedia,
  seekToPiece,
  startStream,
  stopPreDownload,
  stopStream,
} from './torrent-engine'
import {
  probeUrl,
  stopDirectStream,
} from './direct-stream-engine'
import type {
  PlaybackFailure,
  PlaybackPreferences,
  PlaybackSession,
  PlaybackSource,
  RecoveryState,
} from '../../shared/types'
import { selectPlaybackMode, type DecodeCapabilities } from '../../shared/playback-utils'

const DEFAULT_PREFERENCES: PlaybackPreferences = {
  playbackSpeed: 1,
  volume: 1,
  quality: 'auto',
  audioLanguage: 'eng',
  subtitleLanguage: 'eng',
  subtitlesEnabled: true,
  subtitleSize: 100,
  subtitleVerticalPosition: 10,
  subtitleOffsetSecs: 0,
}

type CreatePlaybackSession = Pick<
  PlaybackSession,
  'source' | 'sources' | 'title' | 'returnRoute'
> & Partial<Pick<
  PlaybackSession,
  'mediaId' | 'mediaKind' | 'episodeNumber' | 'positionSecs' | 'selectedAudioIndex'
>> & { capabilities?: DecodeCapabilities }

// Renderer-reported decoder support; kept for audio-track switches mid-session.
let decodeCapabilities: DecodeCapabilities = {}

let activeSession: PlaybackSession | null = null
let generation = 0
const recentLogs: string[] = []

function addLog(message: string): void {
  recentLogs.push(`${new Date().toISOString()} ${message}`)
  if (recentLogs.length > 80) recentLogs.splice(0, recentLogs.length - 80)
}

function persistSession(session: PlaybackSession | null, recoverable: boolean): void {
  const db = getDb()
  if (!session) {
    db.prepare('DELETE FROM playback_session_state WHERE id = 1').run()
    return
  }
  db.prepare(`
    INSERT INTO playback_session_state (id, payload_json, recoverable, updated_at)
    VALUES (1, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET
      payload_json = excluded.payload_json,
      recoverable = excluded.recoverable,
      updated_at = excluded.updated_at
  `).run(JSON.stringify(session), recoverable ? 1 : 0, Date.now())
}

export async function createPlaybackSession(input: CreatePlaybackSession): Promise<PlaybackSession> {
  await stopPlaybackSession(undefined, false)
  if (input.capabilities) decodeCapabilities = input.capabilities
  const token = ++generation
  const now = Date.now()
  const session: PlaybackSession = {
    id: randomUUID(),
    state: 'connecting',
    source: input.source,
    sources: input.sources?.length ? input.sources : [input.source],
    mediaId: input.mediaId,
    mediaKind: input.mediaKind,
    title: input.title,
    episodeNumber: input.episodeNumber,
    mode: 'direct',
    selectedAudioIndex: input.selectedAudioIndex || 0,
    positionSecs: Math.max(0, input.positionSecs || 0),
    transcodeOffsetSecs: 0,
    returnRoute: input.returnRoute,
    createdAt: now,
    updatedAt: now,
  }
  activeSession = session
  persistSession(session, true)
  addLog(`session ${session.id} connecting to ${session.source.title}`)

  try {
    if (session.source.infoHash.startsWith('http')) {
      session.streamUrl = `mvstream://stream?url=${encodeURIComponent(session.source.infoHash)}`
      try {
        session.probe = await probeUrl(session.source.infoHash)
      } catch (err) {
        console.warn('[playback-session] Failed to probe remote URL:', err)
        session.probe = { durationSecs: null, tracks: [] }
      }
    } else {
      const result = await startStream(session.source.infoHash, session.source.fileIdx, session.id)
      if (token !== generation || activeSession?.id !== session.id) throw new Error('Playback session was superseded')
      session.streamUrl = result.streamUrl
      session.probe = await probeActiveMedia()
    }
    if (token !== generation || activeSession?.id !== session.id) throw new Error('Playback session was superseded')
    session.mode = selectPlaybackMode(session.probe, session.selectedAudioIndex, decodeCapabilities)
    session.state = 'ready'
    session.updatedAt = Date.now()
    activeSession = { ...session }
    persistSession(activeSession, true)
    addLog(`session ${session.id} ready in ${session.mode} mode`)
    return activeSession
  } catch (error: any) {
    if (token !== generation) throw error
    const failure: PlaybackFailure = {
      code: 'SESSION_START_FAILED',
      message: error?.message || String(error),
      recoverable: true,
      occurredAt: Date.now(),
    }
    session.state = 'failed'
    session.failure = failure
    session.updatedAt = Date.now()
    activeSession = session
    persistSession(session, true)
    addLog(`session ${session.id} failed: ${failure.message}`)
    throw error
  }
}

export function getPlaybackSession(): PlaybackSession | null {
  return activeSession ? { ...activeSession } : null
}

export function updatePlaybackSession(
  sessionId: string,
  patch: Partial<Pick<PlaybackSession, 'positionSecs' | 'state' | 'selectedAudioIndex' | 'selectedSubtitleIndex' | 'mode'>>,
): PlaybackSession | null {
  if (!activeSession || activeSession.id !== sessionId) return null
  const canonicalDuration = activeSession.probe?.durationSecs
  const nextPosition = patch.positionSecs == null
    ? activeSession.positionSecs
    : Math.max(0, canonicalDuration ? Math.min(patch.positionSecs, canonicalDuration) : patch.positionSecs)
  activeSession = {
    ...activeSession,
    ...patch,
    positionSecs: nextPosition,
    updatedAt: Date.now(),
  }
  if (patch.selectedAudioIndex != null) {
    activeSession.mode = selectPlaybackMode(activeSession.probe, patch.selectedAudioIndex, decodeCapabilities)
  }
  persistSession(activeSession, true)
  return { ...activeSession }
}

export async function stopPlaybackSession(sessionId?: string, normalExit = true): Promise<boolean> {
  if (sessionId && activeSession && activeSession.id !== sessionId) return false
  generation++
  const stopped = activeSession
  activeSession = null
  await stopStream()
  stopDirectStream()
  stopPreDownload()
  if (stopped) addLog(`session ${stopped.id} stopped`)
  if (normalExit) persistSession(null, false)
  return true
}

export async function switchPlaybackSource(
  sessionId: string,
  source: PlaybackSource,
): Promise<PlaybackSession> {
  if (!activeSession || activeSession.id !== sessionId) throw new Error('Playback session is no longer active')
  const previous = activeSession
  return createPlaybackSession({
    source,
    sources: previous.sources,
    title: previous.title,
    mediaId: previous.mediaId,
    mediaKind: previous.mediaKind,
    episodeNumber: previous.episodeNumber,
    positionSecs: previous.positionSecs,
    selectedAudioIndex: previous.selectedAudioIndex,
    returnRoute: previous.returnRoute,
  })
}

export function seekPlaybackSession(sessionId: string, positionSecs: number): PlaybackSession | null {
  if (!activeSession || activeSession.id !== sessionId) return null
  const duration = activeSession.probe?.durationSecs || 0
  if (duration > 0) seekToPiece(Math.max(0, Math.min(1, positionSecs / duration)))
  activeSession.positionSecs = Math.max(0, duration ? Math.min(positionSecs, duration) : positionSecs)
  activeSession.transcodeOffsetSecs = activeSession.mode === 'direct' ? 0 : activeSession.positionSecs
  activeSession.updatedAt = Date.now()
  persistSession(activeSession, true)
  return { ...activeSession }
}

export function recordPlaybackFailure(
  sessionId: string,
  failure: PlaybackFailure,
): RecoveryState | null {
  if (!activeSession || activeSession.id !== sessionId) return null
  activeSession = { ...activeSession, state: 'failed', failure, updatedAt: Date.now() }
  persistSession(activeSession, true)
  getDb().prepare(`
    INSERT INTO playback_source_failure (id, media_id, source_name, failure_code, failed_at)
    VALUES (?, ?, ?, ?, ?)
  `).run(randomUUID(), activeSession.mediaId || null, activeSession.source.title, failure.code, Date.now())
  addLog(`session ${sessionId} failure: ${failure.code} ${failure.message}`)
  return { session: { ...activeSession }, failure, recentLogs: [...recentLogs] }
}

export function getRecoveryState(): RecoveryState | null {
  if (activeSession?.failure) {
    return { session: { ...activeSession }, failure: activeSession.failure, recentLogs: [...recentLogs] }
  }
  const row = getDb().prepare(
    'SELECT payload_json, recoverable FROM playback_session_state WHERE id = 1',
  ).get() as { payload_json: string; recoverable: number } | undefined
  if (!row?.recoverable) return null
  try {
    const session = JSON.parse(row.payload_json) as PlaybackSession
    const failure = session.failure || {
      code: 'INTERRUPTED',
      message: 'Playback was interrupted before the session closed normally.',
      recoverable: true,
      occurredAt: session.updatedAt,
    }
    return { session, failure, recentLogs: [...recentLogs] }
  } catch {
    return null
  }
}

export function clearRecoveryState(): void {
  persistSession(null, false)
  if (activeSession?.state === 'failed') activeSession = null
}

export function getPlaybackPreferences(): PlaybackPreferences {
  const row = getDb().prepare('SELECT payload_json FROM player_preferences WHERE id = 1').get() as
    | { payload_json: string }
    | undefined
  if (!row) return { ...DEFAULT_PREFERENCES }
  try {
    return { ...DEFAULT_PREFERENCES, ...JSON.parse(row.payload_json) }
  } catch {
    return { ...DEFAULT_PREFERENCES }
  }
}

export function setPlaybackPreferences(patch: Partial<PlaybackPreferences>): PlaybackPreferences {
  const preferences = { ...getPlaybackPreferences(), ...patch }
  getDb().prepare(`
    INSERT INTO player_preferences (id, payload_json, updated_at)
    VALUES (1, ?, ?)
    ON CONFLICT(id) DO UPDATE SET payload_json = excluded.payload_json, updated_at = excluded.updated_at
  `).run(JSON.stringify(preferences), Date.now())
  return preferences
}

export function getEmbeddedSubtitleUrl(index: number): string {
  return `${getStreamServerBaseUrl()}/subtitle?index=${Math.max(0, index)}`
}
