import { useState, useEffect, useRef, useCallback, useMemo } from 'react'
import { motion, AnimatePresence } from 'framer-motion'
import { useUiStore } from '../stores/ui.store'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import {
  ArrowLeft, Play, Pause, Volume2, Volume1, VolumeX, Maximize, Minimize,
  SkipForward, Users, Download, Subtitles, PictureInPicture2,
  Check, X, Settings, ListVideo, AlertCircle, FastForward, Keyboard, ListOrdered,
} from 'lucide-react'
import { SkipButton } from '../components/player/SkipButton'
import { NextEpisodeCard } from '../components/player/NextEpisodeCard'
import { SeekIcon } from '../components/player/SeekIcon'
import { EpisodeDrawer } from '../components/player/EpisodeDrawer'
import { kindOf, SKIP_LABEL, SKIP_COLOR, type SkipSegment, type SkipTimesResult } from '../components/player/skip'
import { parseSubtitleText, sidecarCandidates, streamSubtitleCues, type SubCue } from '../components/player/subtitles'
import { SubtitleOverlay, type SubBackground } from '../components/player/SubtitleOverlay'
import { TrackPanel, type SubtitleOption } from '../components/player/TrackPanel'
import { LangFlag } from '../components/player/LangFlag'
import { PixelLoader } from '../components/player/PixelLoader'
import { resolveLanguage, sameLanguage } from '../components/player/languages'
import { useMediaStore } from '../stores/media.store'
import type {
  MediaKind,
  MediaTrack,
  PlaybackPreferences,
  PlaybackSession,
  PlaybackSource,
} from '../../shared/types'
import type { AnimeTorrentStream } from '../../shared/anime-addon-interface'
import { hasPlayableLink } from '../../shared/stream-utils'
import { filterMatchingStreams, streamAudioLanguages } from '../../shared/stream-match'

/** Probed once: with hardware HEVC the main process copies video instead of re-encoding it. */
const DECODE_CAPABILITIES = (() => {
  try {
    return { hevc: typeof MediaSource !== 'undefined' && MediaSource.isTypeSupported('video/mp4; codecs="hvc1.1.6.L120.90"') }
  } catch {
    return { hevc: false }
  }
})()

/** Sources a single play attempt may roll through before showing an error. */
const MAX_AUTO_SOURCE_SWITCHES = 4

const sourceKey = (stream: Pick<AnimeTorrentStream, 'infoHash' | 'fileIdx'>) => `${stream.infoHash}|${stream.fileIdx ?? ''}`

// Framer Motion shared variants for dropdown menus
const dropdownMotion = {
  initial: { opacity: 0, y: 8, scale: 0.96 },
  animate: { opacity: 1, y: 0, scale: 1 },
  exit: { opacity: 0, y: 6, scale: 0.96 },
  transition: { duration: 0.18, ease: [0.22, 1, 0.36, 1] as [number, number, number, number] },
}

interface TorrentStatus {
  downloadSpeed: number
  uploadSpeed: number
  peers: number
  progress: number
  streamUrl?: string
  ready: boolean
}

function formatBytes(b: number): string {
  if (b < 1024) return `${b} B/s`
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB/s`
  return `${(b / 1024 / 1024).toFixed(1)} MB/s`
}

function formatTime(secs: number): string {
  if (isNaN(secs) || !isFinite(secs)) return '0:00'
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  const s = Math.floor(secs % 60)
  if (h > 0) return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`
  return `${m}:${s.toString().padStart(2, '0')}`
}

const SPEED_OPTIONS = [0.5, 0.75, 1, 1.25, 1.5, 2]

export function AnimePlayer() {
  const { pageParams, replace, navigate, enterPip } = useUiStore()
  const {
    infoHash: initialInfoHash, fileIdx: initialFileIdx, animeTitle, episodeNumber, episodeTitle, coverUrl,
    httpUrl: initialHttpUrl, streamQuality: initialStreamQuality, availableStreams = [],
    episodeList, currentEpIndex, animeDetailParams, imdbId, seasonNumber,
    mediaKind = 'anime', providerId, genres, year, description, score, resumePosition,
  } = pageParams
  const recordMedia = useMediaStore((state) => state.record)
  const lastProgressRef = useRef(0)
  const lastSessionPersistRef = useRef(0)
  const sessionRef = useRef<PlaybackSession | null>(null)
  const [session, setSession] = useState<PlaybackSession | null>(null)
  const [transcodeOffset, setTranscodeOffset] = useState(0)
  const leavingRef = useRef(false)
  const preferencesRef = useRef<PlaybackPreferences | null>(null)

  const videoRef = useRef<HTMLVideoElement>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const seekTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const [activeStream, setActiveStream] = useState<AnimeTorrentStream>({
    infoHash: initialInfoHash, fileIdx: initialFileIdx, title: pageParams.streamTitle || 'Current stream',
    quality: initialStreamQuality,
  })
  const [streamUrl, setStreamUrl] = useState<string | null>(initialHttpUrl || null)
  const [loading, setLoading] = useState(!initialHttpUrl)
  const [error, setError] = useState<string | null>(null)
  const [retryNonce, setRetryNonce] = useState(0)
  const [torrentStatus, setTorrentStatus] = useState<TorrentStatus | null>(null)
  const [statusText, setStatusText] = useState('Connecting to peers...')

  // Player state
  const [playing, setPlaying] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)

  const trackPlayback = useCallback((force = false) => {
    const video = videoRef.current
    const canonicalDuration = sessionRef.current?.probe?.durationSecs || duration
    if (!video || !animeTitle || !canonicalDuration) return
    const now = Date.now()
    if (!force && now - lastProgressRef.current < 15000) return
    lastProgressRef.current = now
    const kind = mediaKind as MediaKind
    const position = transcodeOffset + video.currentTime
    const complete = position / canonicalDuration >= 0.9
    void recordMedia({
      kind, type: complete ? 'complete' : 'progress', providerId, episodeNumber,
      positionSecs: Math.min(position, canonicalDuration), durationSecs: canonicalDuration,
      title: {
        id: kind === 'anime' ? `anime:mal:${animeDetailParams?.animeId || imdbId || animeTitle}` : `${kind}:imdb:${imdbId || animeTitle}`,
        kind, title: animeTitle, imdbId: kind === 'anime' ? undefined : imdbId,
        malId: kind === 'anime' ? Number(animeDetailParams?.animeId) || undefined : undefined,
        coverUrl, year, genres: genres || [], languages: kind === 'anime' ? ['Japanese'] : [],
        description, score,
      },
    })
  }, [animeTitle, animeDetailParams?.animeId, coverUrl, description, duration, episodeNumber, genres, imdbId, mediaKind, providerId, recordMedia, score, transcodeOffset, year])

  useEffect(() => () => trackPlayback(true), [trackPlayback])
  // Volume 0–2: 0–1 = normal, 1–2 = boost (via Web Audio GainNode)
  const [volume, setVolume] = useState(1)
  const [muted, setMuted] = useState(false)
  const gainNodeRef = useRef<GainNode | null>(null)
  const audioCtxRef = useRef<AudioContext | null>(null)
  const audioSourceRef = useRef<MediaElementAudioSourceNode | null>(null)
  const [isFullscreen, setIsFullscreen] = useState(false)
  const [showControls, setShowControls] = useState(true)
  const hideTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const [buffered, setBuffered] = useState(0)

  // Panels
  const [showTracks, setShowTracks] = useState(false)
  const [showSettings, setShowSettings] = useState(false)
  const [showSources, setShowSources] = useState(false)
  const [showShortcuts, setShowShortcuts] = useState(false)
  const [playbackSpeed, setPlaybackSpeed] = useState(1)
  const [qualityPreference, setQualityPreference] = useState<PlaybackPreferences['quality']>('auto')
  const [prefsReady, setPrefsReady] = useState(false)
  // Embedded subtitle streams reported by the session probe.
  const [subtitleTracks, setSubtitleTracks] = useState<{
    idx: number; label: string; lang: string; codec?: string; forced?: boolean; imageBased?: boolean
  }[]>([])
  const [audioTracks, setAudioTracks] = useState<{
    label: string; lang: string; enabled: boolean; idx: number; codec?: string; channels?: number
  }[]>([])
  const [selectedAudioIndex, setSelectedAudioIndex] = useState(0)
  // Audio language chosen from another source; applied when that source's session starts.
  const pendingAudioLangRef = useRef<string | null>(null)
  const [switchingAudioLang, setSwitchingAudioLang] = useState<string | null>(null)
  // Subtitle sync offset in seconds — positive = delay subtitles, negative = advance.
  const [subtitleOffset, setSubtitleOffset] = useState(0)
  const subtitleOffsetRef = useRef(0)

  // External subtitles from the online aggregator
  interface ExtSubEntry { id: string; lang: string; langName: string; title: string; downloads: number; fileId: number; url?: string }
  const [extSubs, setExtSubs] = useState<ExtSubEntry[]>([])
  const [extSubLoading, setExtSubLoading] = useState(false)

  // The subtitle being drawn by <SubtitleOverlay>. Keys: emb-<i>, online-<lang>, or a local id.
  const [activeSub, setActiveSub] = useState<{ key: string; label: string; lang: string; cues: SubCue[] } | null>(null)
  const activeSubRef = useRef(activeSub)
  activeSubRef.current = activeSub
  const [subLoadingKey, setSubLoadingKey] = useState<string | null>(null)
  const subAbortRef = useRef<AbortController | null>(null)
  const embeddedRangeRef = useRef<{ key: string; from: number } | null>(null)
  const onlineVersionRef = useRef(0)
  // The user picked a subtitle (or Off) themselves — automatic selection stays out of the way.
  const userSubChoiceRef = useRef(false)
  const autoSubDoneRef = useRef(false)

  // Subtitle appearance
  const [subtitleSize, setSubtitleSize] = useState(100)
  const [subtitleVPos, setSubtitleVPos] = useState(10)

  // Next episode auto-play
  const [showNextEp, setShowNextEp] = useState(false)
  const [nextEpCountdown, setNextEpCountdown] = useState(0)
  const [autoPlayTimer, setAutoPlayTimer] = useState(20)
  const [autoPlayEnabled, setAutoPlayEnabled] = useState(true)
  const [preDownloadEnabled, setPreDownloadEnabled] = useState(true)
  const countdownRef = useRef<ReturnType<typeof setInterval> | undefined>(undefined)
  const nextEpPreloadedRef = useRef(false)
  const nextEpCancelledRef = useRef(false)

  // Smart skip (AniSkip) + persisted player toggles
  const [skipSegments, setSkipSegments] = useState<SkipSegment[]>([])
  const [autoSkip, setAutoSkip] = useState(false)
  const autoSkippedRef = useRef<Set<string>>(new Set())
  const [showEpisodes, setShowEpisodes] = useState(false)
  const [pulse, setPulse] = useState<{ n: number; kind: 'play' | 'pause' } | null>(null)
  const [toastMsg, setToastMsg] = useState<string | null>(null)
  const toastTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const clickTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const playerCfgRef = useRef<Record<string, any>>({})
  const [subColor, setSubColor] = useState('#ffffff')
  const [subBg, setSubBg] = useState<SubBackground>('soft')
  const [localSubs, setLocalSubs] = useState<Array<{ id: string; label: string; lang: string; cues: SubCue[] }>>([])
  const malId = mediaKind === 'anime' && /^\d+$/.test(String(animeDetailParams?.animeId ?? '')) ? Number(animeDetailParams?.animeId) : 0
  const showToast = useCallback((msg: string) => {
    setToastMsg(msg)
    if (toastTimer.current) clearTimeout(toastTimer.current)
    toastTimer.current = setTimeout(() => setToastMsg(null), 1400)
  }, [])
  const savePlayerCfg = useCallback((patch: Record<string, any>) => {
    playerCfgRef.current = { ...playerCfgRef.current, ...patch }
    void invoke(IPC.CONFIG_SET, 'player', playerCfgRef.current).catch(() => {})
  }, [])

  // Seek hover preview + drag scrubbing
  const [seekHover, setSeekHover] = useState<number | null>(null)
  const [scrubbing, setScrubbing] = useState(false)
  const scrubbingRef = useRef(false)
  const seekBarRef = useRef<HTMLDivElement>(null)
  const transcodeOffsetRef = useRef(0)
  transcodeOffsetRef.current = transcodeOffset

  // Pre-buffer state: wait until N seconds are buffered before auto-playing
  // Netflix-style fast start: a few seconds is enough because the torrent
  // engine keeps prioritising pieces just ahead of the playhead.
  const BUFFER_TARGET_SECONDS = 3
  // Slightly deeper cushion when re-buffering mid-stream, so it doesn't stutter
  const REBUFFER_TARGET_SECONDS = 4
  const [prebuffering, setPrebuffering] = useState(true)
  const [bufferFillPct, setBufferFillPct] = useState(0)
  const hasStartedRef = useRef(false)
  // True when mid-stream stall triggered the overlay (vs initial load)
  const midStreamStallRef = useRef(false)
  // Mirrors midStreamStallRef for rendering (refs don't trigger re-renders)
  const [isMidStreamStall, setIsMidStreamStall] = useState(false)

  // Cycling phrases shown during mid-stream buffering
  const BUFFER_PHRASES = [
    'Buffering...',
    'Wait a moment...',
    'Just a little while...',
    'Hang tight...',
    'Almost there...',
    'Loading your stream...',
    'Just a sec...',
    'Getting more data...',
  ]
  const [bufferPhraseIdx, setBufferPhraseIdx] = useState(0)
  // True when the user deliberately clicked pause (not a stall-induced pause)
  const userPausedRef = useRef(false)
  // True when WE paused the video due to a stall (so onPause doesn't set userPausedRef)
  const stallPausedRef = useRef(false)

  // Transcode fallback — if the browser can't decode the native stream
  // (HEVC without HW decode, exotic containers, VP9-in-MKV, etc), we
  // transparently swap the <video> src to `mvstream://stream/transcode`
  // which pipes the file through system ffmpeg → H.264/AAC fragmented MP4.
  // `transcodeMode` is set once per session to prevent infinite retry loops.
  const [transcodeMode, setTranscodeMode] = useState(false)
  // A cheap copy/remux that still failed to decode escalates to a full re-encode
  // before we give up on the source.
  const [forceFullTranscode, setForceFullTranscode] = useState(false)
  // Only the audio needs converting (track switch / AC-3 / silent audio): copy the
  // H.264 video untouched instead of re-encoding every frame.
  const [copyVideo, setCopyVideo] = useState(false)
  const transcodeAttemptedRef = useRef(false)
  const automaticSourceSwitchRef = useRef(false)
  const triedSourcesRef = useRef(new Set<string>())
  const videoSessionKey = useMemo(() => {
    const mode = transcodeMode ? (forceFullTranscode ? 'full' : 'transcode') : 'direct'
    return `${activeStream.infoHash}:${activeStream.fileIdx ?? 'auto'}:${mode}`
  }, [activeStream.fileIdx, activeStream.infoHash, transcodeMode, forceFullTranscode])

  const transcodeKind = useMemo(() => {
    if (forceFullTranscode) return 'full-transcode'
    if (session?.mode && session.mode !== 'direct') return session.mode
    return copyVideo ? 'audio-transcode' : 'full-transcode'
  }, [copyVideo, forceFullTranscode, session?.mode])
  /** Switch to the ffmpeg path for an audio-only reason, copying video when it is safe. */
  const transcodeForAudio = useCallback(() => {
    const videoCodec = sessionRef.current?.probe?.tracks.find((track) => track.kind === 'video')?.codec || ''
    if (/^(h264|avc)/.test(videoCodec)) setCopyVideo(true)
    transcodeAttemptedRef.current = true
    setTranscodeMode(true)
  }, [])

  const returnRoute = useMemo(() => pageParams.returnRoute || (
    animeDetailParams
      ? { page: 'anime-detail', params: animeDetailParams }
      : { page: mediaKind === 'movie' || mediaKind === 'series' ? 'movies' : 'anime', params: {} }
  ), [animeDetailParams, mediaKind, pageParams.returnRoute])

  // Load playback settings
  useEffect(() => {
    Promise.all([
      invoke<any>(IPC.CONFIG_GET, 'playback'),
      invoke<PlaybackPreferences>(IPC.PLAYBACK_PREFERENCES_GET),
      invoke<any>(IPC.CONFIG_GET, 'player').catch(() => null),
    ]).then(([cfg, preferences, playerCfg]) => {
      if (cfg?.autoPlay != null) setAutoPlayEnabled(cfg.autoPlay)
      if (playerCfg && typeof playerCfg === 'object') {
        playerCfgRef.current = playerCfg
        if (typeof playerCfg.autoNext === 'boolean') setAutoPlayEnabled(playerCfg.autoNext)
        if (typeof playerCfg.autoSkip === 'boolean') setAutoSkip(playerCfg.autoSkip)
        if (typeof playerCfg.subColor === 'string') setSubColor(playerCfg.subColor)
        if (playerCfg.subBg === 'none' || playerCfg.subBg === 'soft' || playerCfg.subBg === 'solid') setSubBg(playerCfg.subBg)
      }
      if (cfg?.autoPlayTimer) setAutoPlayTimer(cfg.autoPlayTimer)
      if (cfg?.preDownloadNext != null) setPreDownloadEnabled(cfg.preDownloadNext)
      preferencesRef.current = preferences
      setPlaybackSpeed(preferences.playbackSpeed)
      setQualityPreference(preferences.quality)
      setVolume(preferences.volume)
      setSubtitleSize(preferences.subtitleSize)
      setSubtitleVPos(preferences.subtitleVerticalPosition)
      setSubtitleOffset(preferences.subtitleOffsetSecs)
    }).catch(() => {}).finally(() => setPrefsReady(true))
  }, [])

  // Start or replace the supervised playback session.
  useEffect(() => {
    const infoHash = activeStream.infoHash
    const fileIdx = activeStream.fileIdx
    const httpUrl = infoHash?.startsWith('http') ? infoHash : undefined
    // Reset pre-buffer state for every new stream
    hasStartedRef.current = false
    setPrebuffering(true)
    setBufferFillPct(0)
    setBuffered(0)
    setPlaying(false)
    setCurrentTime(Math.max(0, Number(resumePosition) || 0))
    // Start loading direct links immediately through the same proxy URL the
    // session will return, so the <video> doesn't reload when it arrives.
    setStreamUrl(httpUrl ? `mvstream://stream?url=${encodeURIComponent(httpUrl)}` : null)
    // Drop the previous session so its id doesn't filter out this one's
    // connect/peer progress events.
    sessionRef.current = null
    setForceFullTranscode(false)
    setCopyVideo(false)
    setError(null)
    const isTorrent = infoHash && !infoHash.startsWith('http')
    setStatusText(isTorrent ? 'Connecting to peers...' : 'Connecting to stream...')
    setSelectedAudioIndex(0)
    setAudioTracks([])
    setSubtitleTracks([])
    // Embedded tracks belong to the old file; online/local subtitles carry over.
    if (activeSubRef.current?.key.startsWith('emb-')) {
      subAbortRef.current?.abort()
      setActiveSub(null)
    }
    autoSubDoneRef.current = false

    if (!infoHash) return
    triedSourcesRef.current.add(sourceKey({ infoHash, fileIdx }))

    setLoading(true)
    setError(null)
    setStatusText(isTorrent ? 'Connecting to peers...' : 'Connecting to stream...')

    let cancelled = false
    const startTimer = window.setTimeout(() => {
      if (cancelled) return
      const source: PlaybackSource = {
        infoHash,
        fileIdx,
        title: activeStream.title || 'Current stream',
        quality: activeStream.quality,
      }
      const sources: PlaybackSource[] = (availableStreams as AnimeTorrentStream[]).map((stream) => ({
        infoHash: stream.infoHash,
        fileIdx: stream.fileIdx,
        title: stream.title,
        quality: stream.quality,
        seeders: stream.seeders,
        size: stream.size,
      }))
      invoke<{ session?: PlaybackSession; error?: string }>(IPC.PLAYBACK_SESSION_CREATE, {
        source,
        sources,
        capabilities: DECODE_CAPABILITIES,
        title: animeTitle || activeStream.title || 'Playing',
        mediaId: mediaKind === 'anime'
          ? `anime:mal:${animeDetailParams?.animeId || imdbId || animeTitle}`
          : `${mediaKind}:imdb:${imdbId || animeTitle}`,
        mediaKind,
        episodeNumber,
        positionSecs: Math.max(Number(resumePosition) || 0, currentTime || 0),
        returnRoute,
      })
        .then((result) => {
          if (cancelled) return
          if (result?.error) {
            // Soft error — returned as object, no console spam.
            // If playback is already live via another path, silently ignore.
            const v = videoRef.current
            const playbackLive =
              hasStartedRef.current || !!streamUrl ||
              (v && !v.paused && v.currentTime > 0 && v.readyState >= 2)
            if (playbackLive) {
              console.debug('[player] torrent:start soft error but playback is live — ignoring')
              return
            }
            if (result.error.includes('superseded')) return
            // Dead swarm / unreachable link: roll straight over to the next
            // best source instead of parking the user on an error screen.
            const next = nextUntriedSource()
            if (next) {
              console.warn(`[player] source failed (${result.error}) — trying next: ${next.title}`)
              setStatusText('That source didn’t respond. Trying the next best one…')
              switchStream(next, true)
              return
            }
            setError(result.error)
            setLoading(false)
            return
          }
          if (result?.session?.streamUrl) {
            sessionRef.current = result.session
            setSession(result.session)
            setStreamUrl(result.session.streamUrl)
            setDuration(result.session.probe?.durationSecs || 0)
            const resumeAt = Math.max(0, Number(resumePosition) || result.session.positionSecs || 0)
            // A source that already fell back to conversion keeps its resume offset.
            setTranscodeOffset(result.session.mode === 'direct' && !transcodeAttemptedRef.current ? 0 : resumeAt)
            setCurrentTime(resumeAt)
            const probedAudio = result.session.probe?.tracks.filter((track) => track.kind === 'audio') || []
            // A language picked from another source wins over the saved preference.
            const pendingAudio = pendingAudioLangRef.current
            pendingAudioLangRef.current = null
            const wantAudio = resolveLanguage(pendingAudio || preferencesRef.current?.audioLanguage).code
            // Untagged tracks often say their language in the title ("Hindi 5.1").
            const preferredTrack = (wantAudio && probedAudio.find((track) => resolveLanguage(track.language, track.title).code === wantAudio))
              || probedAudio.find((track) => track.default) || probedAudio[0]
            if (pendingAudio && preferredTrack && resolveLanguage(preferredTrack.language, preferredTrack.title).code !== wantAudio) {
              showToast(`This source doesn't label a ${resolveLanguage(pendingAudio).name} track — pick it in Audio & Subtitles`)
            }
            setSwitchingAudioLang(null)
            const selectedIndex = preferredTrack?.index ?? result.session.selectedAudioIndex
            setSelectedAudioIndex(selectedIndex)
            setAudioTracks(probedAudio.map((track) => ({
              idx: track.index,
              label: track.title || '',
              lang: track.language || '',
              codec: track.codec,
              channels: track.channels,
              enabled: track.index === selectedIndex,
            })))
            const probedSubs = result.session.probe?.tracks.filter((track) => track.kind === 'subtitle') || []
            setSubtitleTracks(probedSubs.map((track) => ({
              idx: track.index,
              label: track.title || '',
              lang: track.language || '',
              codec: track.codec,
              forced: track.forced || /\bforced\b/i.test(track.title || ''),
              imageBased: track.imageBased,
            })))
            if (result.session.mode !== 'direct') {
              transcodeAttemptedRef.current = true
              setTranscodeMode(true)
            } else if (probedAudio.length > 1) {
              // The browser only ever plays the file's default track; any other
              // language goes through ffmpeg (video copied, audio converted).
              const browserTrack = probedAudio.find((track) => track.default) || probedAudio[0]
              if (selectedIndex !== browserTrack.index) {
                setTranscodeOffset(resumeAt)
                setStatusText(`Switching to ${resolveLanguage(preferredTrack?.language, preferredTrack?.title).name} audio…`)
                transcodeForAudio()
              }
            }
            setLoading(false)
            setStatusText('')
            if (selectedIndex !== result.session.selectedAudioIndex) {
              void invoke(IPC.PLAYBACK_SESSION_UPDATE, result.session.id, { selectedAudioIndex: selectedIndex })
            }
          }
        })
        .catch(() => {
          if (!cancelled && !hasStartedRef.current) {
            setError('Connection lost')
            setLoading(false)
          }
        })
    }, 0)

    return () => {
      cancelled = true
      window.clearTimeout(startTimer)
    }
  }, [activeStream, retryNonce])

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void invoke(IPC.PLAYBACK_PREFERENCES_SET, {
        volume,
        subtitleSize,
        subtitleVerticalPosition: subtitleVPos,
        subtitleOffsetSecs: subtitleOffset,
      })
    }, 250)
    return () => window.clearTimeout(timer)
  }, [subtitleOffset, subtitleSize, subtitleVPos, volume])

  useEffect(() => () => {
    if (seekTimeoutRef.current) {
      clearTimeout(seekTimeoutRef.current)
    }
    if (!leavingRef.current && sessionRef.current) {
      void invoke(IPC.PLAYBACK_SESSION_STOP, sessionRef.current.id)
    }
  }, [])

  const handleBack = useCallback(async () => {
    if (leavingRef.current) return
    leavingRef.current = true
    const video = videoRef.current
    video?.pause()
    trackPlayback(true)
    const active = sessionRef.current
    if (active) {
      await invoke(IPC.PLAYBACK_SESSION_UPDATE, active.id, {
        positionSecs: currentTime,
        state: 'stopped',
      }).catch(() => null)
      await invoke(IPC.PLAYBACK_SESSION_STOP, active.id).catch(() => null)
    }
    replace(returnRoute.page, returnRoute.params)
  }, [currentTime, replace, returnRoute, trackPlayback])

  const handleRetry = useCallback(() => {
    transcodeAttemptedRef.current = false
    automaticSourceSwitchRef.current = false
    const stale = sessionRef.current
    if (stale) void invoke(IPC.PLAYBACK_SESSION_STOP, stale.id).catch(() => null)
    sessionRef.current = null
    setSession(null)
    setTranscodeMode(false)
    setError(null)
    setRetryNonce((n) => n + 1)
  }, [])

  const switchStream = useCallback((stream: AnimeTorrentStream, automatic = false) => {
    if (stream.infoHash === activeStream.infoHash && stream.fileIdx === activeStream.fileIdx) {
      setShowSources(false)
      return
    }
    trackPlayback(true)
    transcodeAttemptedRef.current = false
    automaticSourceSwitchRef.current = automatic
    // A source the user picked by hand gets a fresh auto-fallback budget.
    if (!automatic) triedSourcesRef.current.clear()
    setTranscodeMode(false)
    setSelectedAudioIndex(0)
    setAudioTracks([])
    setError(null)
    setBuffered(0)
    // The session effect sets the (proxied) URL for the new source.
    setStreamUrl(null)
    setActiveStream(stream)
    setShowSources(false)
  }, [activeStream, trackPlayback])

  /** Best-ranked source not yet attempted in this play, if the budget allows. */
  const nextUntriedSource = useCallback((): AnimeTorrentStream | null => {
    if (triedSourcesRef.current.size > MAX_AUTO_SOURCE_SWITCHES) return null
    return (availableStreams as AnimeTorrentStream[]).find((candidate) =>
      hasPlayableLink(candidate) && !triedSourcesRef.current.has(sourceKey(candidate))) || null
  }, [availableStreams])

  // Torrent progress events
  useEffect(() => {
    return onEvent(IPC.TORRENT_PROGRESS, ((status: TorrentStatus & { sessionId?: string }) => {
      // While connecting there's no session yet — those peer/metadata updates
      // are exactly what the loading screen should show.
      if (status.sessionId && sessionRef.current && status.sessionId !== sessionRef.current.id) return
      setTorrentStatus(status)
      if (status.ready && status.streamUrl && !streamUrl) {
        setStreamUrl(status.streamUrl); setLoading(false)
      }
      if (!status.ready) {
        setStatusText(status.peers > 0 ? `Found ${status.peers} peer${status.peers > 1 ? 's' : ''}, fetching metadata...` : 'Searching for peers...')
      } else if (status.progress < 0.02) {
        setStatusText('Buffering...')
      }
    }) as (...args: unknown[]) => void)
  }, [streamUrl])

  // Detect subtitle & audio tracks after video loads
  const handleLoadedMetadata = useCallback(() => {
    const video = videoRef.current
    if (!video) return
    const canonical = sessionRef.current?.probe?.durationSecs
    if (!canonical && Number.isFinite(video.duration) && video.duration > 0) setDuration(video.duration)
    const resumeAt = sessionRef.current?.positionSecs || 0
    if (sessionRef.current?.mode === 'direct' && resumeAt > 5 && video.currentTime < 1) {
      video.currentTime = canonical ? Math.min(resumeAt, canonical) : resumeAt
    }

    // Audio tracks (Chromium-only API)
    const audios: typeof audioTracks = []
    const at = (video as any).audioTracks
    if (at?.length > 1) {
      for (let i = 0; i < at.length; i++) {
        audios.push({ label: at[i].label || `Audio ${i + 1}`, lang: at[i].language || '', enabled: at[i].enabled, idx: i })
      }
      setAudioTracks(audios)
    }
  }, [])

  useEffect(() => {
    if (!streamUrl || activeStream.infoHash.startsWith('http')) return
    let cancelled = false
    fetch('mvstream://tracks')
      .then((response) => response.ok ? response.json() : { audio: [] })
      .then((payload) => {
        if (cancelled || !Array.isArray(payload.audio) || payload.audio.length === 0) return
        const tracks = payload.audio.map((track: any) => ({
          idx: Number(track.idx) || 0,
          label: track.label || `Audio ${(Number(track.idx) || 0) + 1}`,
          lang: track.lang || '',
          codec: track.codec || '',
          channels: Number(track.channels) || 0,
          enabled: (Number(track.idx) || 0) === selectedAudioIndex,
        }))
        setAudioTracks(tracks)
        const selectedCodec = String(tracks.find((track: any) => track.idx === selectedAudioIndex)?.codec || tracks[0]?.codec || '').toLowerCase()
        if (!transcodeMode && /^(ac-?3|eac3|dts|dca|truehd|mlp)$/.test(selectedCodec)) {
          setStatusText('Preparing compatible audio…')
          transcodeForAudio()
        }
      })
      .catch(() => {})
    return () => { cancelled = true }
  }, [streamUrl, activeStream.infoHash])

  useEffect(() => {
    if (!playing || transcodeMode) return
    const video = videoRef.current as any
    if (!video || typeof video.webkitAudioDecodedByteCount !== 'number') return
    const before = video.webkitAudioDecodedByteCount
    const timer = window.setTimeout(() => {
      const current = videoRef.current as any
      if (!current || current.paused || current.muted || current.volume === 0) return
      if (current.webkitAudioDecodedByteCount === before && before === 0) {
        setStatusText('Repairing audio…')
        transcodeForAudio()
      }
    }, 3500)
    return () => window.clearTimeout(timer)
  }, [playing, transcodeMode, activeStream.infoHash])

  // Buffered progress
  const handleProgress = useCallback(() => {
    const video = videoRef.current
    if (!video || !video.buffered.length) return
    setBuffered(video.buffered.end(video.buffered.length - 1))
  }, [])

  // Episode switched via replace('anime-player', ...) — same component instance, so reload the stream.
  const lastEpRef = useRef(episodeNumber)
  useEffect(() => {
    if (lastEpRef.current === episodeNumber) return
    lastEpRef.current = episodeNumber
    nextEpCancelledRef.current = false
    nextEpPreloadedRef.current = false
    setShowNextEp(false)
    setSkipSegments([])
    setLocalSubs([])
    subAbortRef.current?.abort()
    setActiveSub(null)
    userSubChoiceRef.current = false
    autoSubDoneRef.current = false
    setDuration(0)
    transcodeAttemptedRef.current = false
    setTranscodeMode(false)
    const stale = sessionRef.current
    if (stale) void invoke(IPC.PLAYBACK_SESSION_STOP, stale.id).catch(() => null)
    sessionRef.current = null
    setSession(null)
    setActiveStream({
      infoHash: initialInfoHash, fileIdx: initialFileIdx, title: pageParams.streamTitle || 'Current stream',
      quality: initialStreamQuality,
    })
  }, [episodeNumber])

  // Next episode logic
  const hasNextEp = !!episodeList && currentEpIndex != null && currentEpIndex < episodeList.length - 1
  const nextEp = hasNextEp ? episodeList[currentEpIndex + 1] : null

  // Start of the credits/outro (only when AniSkip gave one in the back half of the episode)
  const outroStart = useMemo(() => {
    if (!duration) return null
    const ed = skipSegments.find((s) => kindOf(s.type) === 'credits' && s.start > duration * 0.5)
    return ed ? ed.start : null
  }, [skipSegments, duration])

  const playEpisodeAt = useCallback(async (index: number) => {
    const ep = episodeList?.[index]
    if (!ep) return
    setShowNextEp(false)
    setShowEpisodes(false)
    // Only the current and next episode arrive with a resolved source; the rest carry a
    // streamKey (e.g. "tt…:1:5" / "kitsu:…") that must be looked up before playing.
    let pick: AnimeTorrentStream | undefined = ep.infoHash
      ? { infoHash: ep.infoHash, fileIdx: ep.fileIdx, title: ep.streamTitle || ep.title || '', quality: ep.streamQuality }
      : undefined
    let sources: AnimeTorrentStream[] = []
    if (ep.streamKey) {
      if (!pick) showToast(`Finding a source for episode ${ep.number}…`)
      try {
        const lookup = invoke<AnimeTorrentStream[]>(IPC.ADDON_FIND_STREAMS, ep.streamKey, 'series')
        // A pre-resolved source shouldn't wait long on the alternatives list.
        const found = pick
          ? await Promise.race([lookup, new Promise<null>((resolve) => setTimeout(() => resolve(null), 2500))])
          : await lookup
        const checked = filterMatchingStreams((found || []).filter(hasPlayableLink), {
          titles: [animeTitle].filter(Boolean),
          year,
          kind: mediaKind === 'movie' ? 'movie' : mediaKind === 'series' ? 'series' : 'anime',
          season: Number(ep.season ?? seasonNumber) || undefined,
          episode: Number(ep.number) || undefined,
        })
        sources = checked.allMismatched ? [] : checked.streams
      } catch { /* keep whatever we have */ }
      if (!pick) {
        pick = sources.find((s) => s.quality && s.quality === activeStream.quality && s.source === activeStream.source)
          || sources.find((s) => s.quality && s.quality === activeStream.quality)
          || sources[0]
      }
    }
    if (!pick?.infoHash) {
      showToast(`No source found for episode ${ep.number}`)
      return
    }
    replace('anime-player', {
      ...pageParams,
      infoHash: pick.infoHash,
      fileIdx: pick.fileIdx,
      httpUrl: pick.infoHash.startsWith('http') ? pick.infoHash : undefined,
      streamTitle: pick.title || ep.streamTitle || ep.title,
      streamQuality: pick.quality,
      // Fallback sources must belong to THIS episode, never the previous one.
      availableStreams: sources.length ? sources : [pick],
      episodeNumber: ep.number,
      episodeTitle: ep.title,
      currentEpIndex: index,
      resumePosition: 0,
    })
  }, [episodeList, pageParams, replace, showToast, activeStream.quality, activeStream.source])

  const playNextEpisode = useCallback(() => {
    if (currentEpIndex == null) return
    playEpisodeAt(currentEpIndex + 1)
  }, [currentEpIndex, playEpisodeAt])

  // Show the "Next episode" card when the outro starts (or in the last 25s), pre-fetch earlier.
  useEffect(() => {
    if (!hasNextEp || !duration || duration <= 0) return
    const tick = () => {
      const video = videoRef.current
      if (!video || video.paused) return
      const pos = transcodeOffset + video.currentTime
      const remaining = duration - pos
      if (remaining <= 0) return
      if (preDownloadEnabled && !nextEpPreloadedRef.current && (remaining <= 120 || (outroStart != null && pos >= outroStart - 30))) {
        const ep = episodeList[currentEpIndex + 1]
        if (ep?.infoHash) {
          invoke(IPC.TORRENT_PRE_NEXT, ep.infoHash, ep.fileIdx).catch(() => {})
          nextEpPreloadedRef.current = true
        }
      }
      const atOutro = outroStart != null && pos >= outroStart
      if ((atOutro || remaining <= 25) && !nextEpCancelledRef.current) setShowNextEp(true)
    }
    const interval = setInterval(tick, 500)
    return () => clearInterval(interval)
  }, [hasNextEp, duration, outroStart, preDownloadEnabled, episodeList, currentEpIndex, transcodeOffset])

  const cancelNextEp = () => {
    nextEpCancelledRef.current = true
    setShowNextEp(false)
  }

  // Active AniSkip segment at the current position
  const activeSkip = useMemo(() => {
    return skipSegments.find((s) => currentTime >= s.start - 0.25 && currentTime < s.end - 0.5) || null
  }, [skipSegments, currentTime])
  // Heuristic fallback only when there is no AniSkip data at all
  const heuristicSkip = skipSegments.length === 0 && currentTime > 5 && currentTime < 240 && duration > 600

  const doSkip = useCallback(() => {
    if (activeSkip) {
      const credits = kindOf(activeSkip.type) === 'credits'
      if (credits && hasNextEp && activeSkip.end >= duration - 3) { playNextEpisode(); return }
      seekTo(Math.min(duration || activeSkip.end, activeSkip.end))
    } else {
      seekTo(Math.min(duration || currentTime + 85, currentTime + 85))
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSkip, duration, currentTime, hasNextEp, playNextEpisode])

  // Auto-skip (once per segment)
  useEffect(() => {
    if (!autoSkip || !activeSkip) return
    const key = `${activeSkip.type}:${activeSkip.start}`
    if (autoSkippedRef.current.has(key)) return
    autoSkippedRef.current.add(key)
    showToast(SKIP_LABEL[kindOf(activeSkip.type)].replace('Skip ', 'Skipped '))
    doSkip()
  }, [autoSkip, activeSkip, doSkip, showToast])

  // Fetch AniSkip data once the duration is known; empty on failure -> heuristic pill
  useEffect(() => {
    setSkipSegments([])
    autoSkippedRef.current = new Set()
    if (!malId || episodeNumber == null || !duration) return
    let cancelled = false
    invoke<SkipTimesResult>(IPC.SKIP_TIMES, malId, Number(episodeNumber), Math.round(duration))
      .then((res) => { if (!cancelled && res?.segments) setSkipSegments(res.segments) })
      .catch(() => {})
    return () => { cancelled = true }
  }, [malId, episodeNumber, Math.round(duration || 0)])

  // Keyboard shortcuts — one listener for the page's lifetime that always
  // calls the latest handler, instead of re-binding on every time update.
  const keyHandlerRef = useRef<(e: KeyboardEvent) => void>(() => {})
  useEffect(() => {
    const listener = (e: KeyboardEvent) => keyHandlerRef.current(e)
    window.addEventListener('keydown', listener)
    return () => window.removeEventListener('keydown', listener)
  }, [])
  {
    keyHandlerRef.current = (e: KeyboardEvent) => {
      const video = videoRef.current
      if (!video) return
      const tag = (e.target as HTMLElement | null)?.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return
      if (e.ctrlKey || e.metaKey || e.altKey) return
      if (showShortcuts && e.key === 'Escape') { setShowShortcuts(false); return }
      switch (e.key) {
        case '?':
          e.preventDefault(); setShowShortcuts((v) => !v); break
        case 's': case 'S':
          e.preventDefault(); doSkip(); break
        case ' ': case 'k': case 'K':
          e.preventDefault(); togglePlay(); break
        case 'j': case 'J':
          e.preventDefault(); seekTo(Math.max(0, currentTime - 10)); break
        case 'l': case 'L':
          e.preventDefault(); seekTo(Math.min(duration || currentTime + 10, currentTime + 10)); break
        case 'ArrowLeft':
          e.preventDefault(); seekTo(Math.max(0, currentTime - 5)); break
        case 'ArrowRight':
          e.preventDefault(); seekTo(Math.min(duration || currentTime + 5, currentTime + 5)); break
        case 'g': case 'G':
          e.preventDefault(); adjustSubtitleOffset(-0.1); break
        case 'h': case 'H':
          e.preventDefault(); adjustSubtitleOffset(0.1); break
        case 'e': case 'E':
          if (episodeList?.length) setShowEpisodes((v) => !v); break
        case 'ArrowUp':
          e.preventDefault(); setVolumeSmooth(Math.min(2, volume + 0.1)); break
        case 'ArrowDown':
          e.preventDefault(); setVolumeSmooth(Math.max(0, volume - 0.1)); break
        case 'f': toggleFullscreen(); break
        case 'Escape':
          if (showEpisodes) { setShowEpisodes(false); break }
          if (document.fullscreenElement) document.exitFullscreen()
          else void handleBack()
          break
        case 'm': video.muted = !video.muted; setMuted(video.muted); break
        case 'c': case 'C': cycleSubtitles(); break
        case '>': case '.':
          e.preventDefault(); changeSpeed(1); break
        case '<': case ',':
          e.preventDefault(); changeSpeed(-1); break
        case 'n': case 'N':
          if (hasNextEp) playNextEpisode(); break
      }
    }
  }

  /** C: Off → each selectable subtitle → Off. */
  const cycleSubtitles = () => {
    const keys = subtitleOptions.filter((o) => !o.disabledReason).map((o) => o.key)
    if (!keys.length) { showToast('No subtitles available'); return }
    const at = activeSub ? keys.indexOf(activeSub.key) : -1
    const next = at + 1 < keys.length ? keys[at + 1] : null
    void selectSubtitleKey(next)
    const option = subtitleOptions.find((o) => o.key === next)
    showToast(option ? `Subtitles: ${option.label}` : 'Subtitles off')
  }

  const changeSpeed = (direction: number) => {
    const video = videoRef.current
    if (!video) return
    const currentIdx = SPEED_OPTIONS.indexOf(playbackSpeed)
    const newIdx = Math.max(0, Math.min(SPEED_OPTIONS.length - 1, currentIdx + direction))
    const newSpeed = SPEED_OPTIONS[newIdx]
    video.playbackRate = newSpeed
    setPlaybackSpeed(newSpeed)
  }

  const selectAudio = (idx: number) => {
    const video = videoRef.current
    if (!video) return
    const at = (video as any).audioTracks
    setSelectedAudioIndex(idx)
    setAudioTracks((tracks) => tracks.map((track) => ({ ...track, enabled: track.idx === idx })))
    if (at?.length) {
      for (let i = 0; i < at.length; i++) at[i].enabled = i === idx
      handleLoadedMetadata()
    } else {
      // Re-mux from where we are now, not from the start / last seek.
      const position = transcodeOffsetRef.current + (video.currentTime || 0)
      setTranscodeOffset(Math.max(0, position))
      setStatusText('Switching audio language…')
      setLoading(true)
      setPrebuffering(true)
      setBufferFillPct(0)
      hasStartedRef.current = false
      transcodeForAudio()
    }
    const active = sessionRef.current
    if (active) void invoke(IPC.PLAYBACK_SESSION_UPDATE, active.id, { selectedAudioIndex: idx })
    void invoke(IPC.PLAYBACK_PREFERENCES_SET, {
      audioLanguage: audioTracks.find((track) => track.idx === idx)?.lang || '',
    })
  }

  const selectSpeed = (speed: number) => {
    const video = videoRef.current
    if (!video) return
    video.playbackRate = speed
    setPlaybackSpeed(speed)
    void invoke(IPC.PLAYBACK_PREFERENCES_SET, { playbackSpeed: speed })
  }

  const selectQuality = (quality: PlaybackPreferences['quality']) => {
    setQualityPreference(quality)
    void invoke(IPC.PLAYBACK_PREFERENCES_SET, { quality })
    if (quality === 'auto' || quality === 'best') {
      const best = (availableStreams as AnimeTorrentStream[])[0]
      if (best) switchStream(best)
      return
    }
    const needle = quality === 'data-saver' ? /(360|480)/ : new RegExp(quality.replace('p', ''), 'i')
    const match = (availableStreams as AnimeTorrentStream[]).find((source) =>
      needle.test(`${source.quality || ''} ${source.title || ''}`),
    )
    if (match) switchStream(match)
  }

  // ── Audio boost via Web Audio API GainNode ──
  // Normal volume (0–1) uses video.volume. Boost (1–2) keeps video.volume=1
  // and raises gainNode.gain.value above 1.0 for amplification beyond 100%.
  const ensureAudioBoost = useCallback(() => {
    const v = videoRef.current
    if (!v || audioSourceRef.current) return // already set up
    try {
      const ctx = new AudioContext()
      const source = ctx.createMediaElementSource(v)
      const gain = ctx.createGain()
      gain.gain.value = 1.0
      source.connect(gain)
      gain.connect(ctx.destination)
      audioCtxRef.current = ctx
      audioSourceRef.current = source
      gainNodeRef.current = gain
    } catch (err) {
      console.warn('[player] Web Audio boost unavailable:', err)
    }
  }, [])

  const setVolumeSmooth = useCallback((val: number) => {
    const v = videoRef.current
    if (!v) return
    const clamped = Math.max(0, Math.min(2, val))
    setVolume(clamped)
    if (clamped > 1) {
      // Boost range: video stays at 1.0, GainNode amplifies
      v.volume = 1
      ensureAudioBoost()
      if (gainNodeRef.current) gainNodeRef.current.gain.value = clamped
    } else {
      // Normal range: video volume, gain at 1.0
      v.volume = clamped
      if (gainNodeRef.current) gainNodeRef.current.gain.value = 1.0
    }
    if (clamped > 0 && v.muted) { v.muted = false; setMuted(false) }
  }, [ensureAudioBoost])

  // ── Stall recovery ──
  // Returns seconds buffered ahead of `currentTime` by finding the range that
  // actually contains the current position. Using the last range's end is wrong
  // after a seek — the old range might be far behind the new playback position.
  const getBufferedAhead = useCallback((video: HTMLVideoElement): number => {
    const ct = video.currentTime
    for (let i = 0; i < video.buffered.length; i++) {
      if (video.buffered.start(i) <= ct + 0.5 && video.buffered.end(i) > ct) {
        return video.buffered.end(i) - ct
      }
    }
    return 0
  }, [])

  const stallTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const resumeFromBuffered = useCallback((video: HTMLVideoElement, reason: string, force = false) => {
    const bufferedSecs = getBufferedAhead(video)
    const target = midStreamStallRef.current ? REBUFFER_TARGET_SECONDS : BUFFER_TARGET_SECONDS
    const fill = Math.min(1, bufferedSecs / target)
    setBufferFillPct(fill)
    if (!prebuffering || hasStartedRef.current || (!force && bufferedSecs < target)) return false

    hasStartedRef.current = true
    const wasMidStream = midStreamStallRef.current
    midStreamStallRef.current = false
    stallPausedRef.current = false
    setIsMidStreamStall(false)
    setPrebuffering(false)
    setLoading(false)
    setError(null)
    if (!wasMidStream) {
      video.muted = false
      setMuted(false)
    }
    console.debug(`[player] resuming after buffer (${reason}, ${bufferedSecs.toFixed(1)}s ahead)`)
    video.play().catch((err) => {
      console.error('[player] play() rejected:', err?.message)
      if (!wasMidStream) {
        setError(`Click play to start. (${err?.message || 'autoplay blocked'})`)
      }
    })
    return true
  }, [BUFFER_TARGET_SECONDS, REBUFFER_TARGET_SECONDS, getBufferedAhead, prebuffering])

  useEffect(() => {
    if (!prebuffering || error) return
    const id = window.setInterval(() => {
      const video = videoRef.current
      if (!video || video.ended) return
      resumeFromBuffered(video, 'watchdog')
    }, 700)
    return () => window.clearInterval(id)
  }, [error, prebuffering, resumeFromBuffered])

  const handleStalled = useCallback(() => {
    const v = videoRef.current
    // Skip if: already prebuffering, user deliberately paused, ended
    if (!v || v.ended || prebuffering || userPausedRef.current) return

    if (stallTimerRef.current) clearTimeout(stallTimerRef.current)
    stallTimerRef.current = setTimeout(() => {
      const video = videoRef.current
      if (!video || video.ended || userPausedRef.current) return
      const bufferedAhead = getBufferedAhead(video)
      if (bufferedAhead < 3) {
        console.warn(`[player] stalled mid-stream — only ${bufferedAhead.toFixed(1)}s ahead, pausing to rebuffer`)
        stallPausedRef.current = true
        video.pause()
        midStreamStallRef.current = true
        hasStartedRef.current = false
        setIsMidStreamStall(true)
        setBufferPhraseIdx(0)
        setPrebuffering(true)
        setBufferFillPct(Math.min(1, bufferedAhead / REBUFFER_TARGET_SECONDS))
        setStatusText('Buffering…')
      }
    }, 2500)
  }, [getBufferedAhead, prebuffering, REBUFFER_TARGET_SECONDS])

  // Sync isFullscreen with the real browser state (handles ESC key exit too)
  useEffect(() => {
    const handler = () => setIsFullscreen(!!document.fullscreenElement)
    document.addEventListener('fullscreenchange', handler)
    return () => document.removeEventListener('fullscreenchange', handler)
  }, [])

  const toggleFullscreen = useCallback(() => {
    const el = containerRef.current
    if (!el) return
    if (document.fullscreenElement) document.exitFullscreen()
    else el.requestFullscreen()
  }, [])

  const resetHideTimer = useCallback(() => {
    setShowControls(true)
    if (hideTimer.current) clearTimeout(hideTimer.current)
    // Always hide after 3s of inactivity — no condition on playing state.
    // Controls show again on any mouse/key event.
    hideTimer.current = setTimeout(() => setShowControls(false), 3000)
  }, [])

  function togglePlay() {
    const v = videoRef.current
    if (!v) return
    if (v.paused || (prebuffering && midStreamStallRef.current)) {
      if (midStreamStallRef.current) {
        midStreamStallRef.current = false
        hasStartedRef.current = true
        setPrebuffering(false)
      }
      userPausedRef.current = false
      v.play().catch(() => {})
      setPulse((p) => ({ n: (p?.n || 0) + 1, kind: 'play' }))
    } else {
      userPausedRef.current = true
      v.pause()
      setPulse((p) => ({ n: (p?.n || 0) + 1, kind: 'pause' }))
    }
  }

  /** Seconds buffered contiguously from local media time `t` (0 when not buffered). */
  const bufferedFrom = (video: HTMLVideoElement, t: number) => {
    for (let i = 0; i < video.buffered.length; i++) {
      if (video.buffered.start(i) <= t + 0.25 && video.buffered.end(i) > t) return video.buffered.end(i) - t
    }
    return 0
  }

  function seekTo(position: number) {
    const video = videoRef.current
    if (!video) return
    const target = Math.max(0, duration ? Math.min(position, duration) : position)
    const wasPlaying = !video.paused && !userPausedRef.current

    // Instantly update the display/local time to keep UI responsive
    setCurrentTime(target)
    followEmbeddedSeek(target)
    if (seekTimeoutRef.current) {
      clearTimeout(seekTimeoutRef.current)
      seekTimeoutRef.current = null
    }

    if (transcodeMode) {
      // Inside what ffmpeg has already delivered: seek locally, no restart.
      const local = target - transcodeOffset
      if (local >= 0 && bufferedFrom(video, local) > 0.5) {
        video.currentTime = local
        return
      }
    } else {
      if (duration > 0 && !activeStream.infoHash.startsWith('http')) {
        void invoke(IPC.TORRENT_SEEK, Math.max(0, Math.min(1, target / duration)))
      }
      video.currentTime = target
      // Only show the buffering state when the new position isn't buffered yet.
      if (wasPlaying && bufferedFrom(video, target) < 2) {
        if (stallTimerRef.current) clearTimeout(stallTimerRef.current)
        stallPausedRef.current = true
        midStreamStallRef.current = true
        hasStartedRef.current = false
        setIsMidStreamStall(true)
        setPrebuffering(true)
        setBufferFillPct(0)
        setStatusText('Buffering…')
      }
    }

    // Debounce the backend seek notification and transcode restarts
    seekTimeoutRef.current = setTimeout(() => {
      const active = sessionRef.current
      if (active) {
        void invoke(IPC.PLAYBACK_SESSION_SEEK, active.id, target)
      }
      if (transcodeMode) {
        setTranscodeOffset(target)
        setPrebuffering(true)
        hasStartedRef.current = false
      }
      seekTimeoutRef.current = null
    }, transcodeMode ? 350 : 250)
  }

  const timeFromPointer = (clientX: number) => {
    const rect = seekBarRef.current?.getBoundingClientRect()
    if (!rect || !duration) return null
    return Math.max(0, Math.min(1, (clientX - rect.left) / rect.width)) * duration
  }

  const handlePip = () => {
    const video = videoRef.current
    enterPip({
      videoSrc: streamUrl || '',
      animeTitle: animeTitle || 'Playing',
      episodeNumber,
      episodeTitle,
      currentTime: video?.currentTime || 0,
      playerParams: pageParams,
    })
  }

  // Close panels when clicking outside
  useEffect(() => {
    const close = () => { setShowTracks(false); setShowSettings(false); setShowSources(false) }
    if (showTracks || showSettings || showSources) {
      const timer = setTimeout(() => window.addEventListener('click', close, { once: true }), 100)
      return () => { clearTimeout(timer); window.removeEventListener('click', close) }
    }
  }, [showTracks, showSettings, showSources])

  const adjustSubtitleOffset = useCallback((delta: number) => {
    const next = Math.round((subtitleOffsetRef.current + delta) * 10) / 10 // snap to 0.1s
    subtitleOffsetRef.current = next
    setSubtitleOffset(next)
    showToast('Subtitle delay ' + (next > 0 ? '+' : '') + next.toFixed(1) + 's')
  }, [showToast])

  useEffect(() => { subtitleOffsetRef.current = subtitleOffset }, [subtitleOffset])

  const resetSubtitleOffset = useCallback(() => {
    subtitleOffsetRef.current = 0
    setSubtitleOffset(0)
  }, [])

  // Cycle phrases while mid-stream buffering
  useEffect(() => {
    if (!isMidStreamStall || !prebuffering) return
    const id = setInterval(() => {
      setBufferPhraseIdx((i) => (i + 1) % BUFFER_PHRASES.length)
    }, 2200)
    return () => clearInterval(id)
  }, [isMidStreamStall, prebuffering])

  // Search online subtitles as soon as we know what is playing — in parallel
  // with peer discovery, so they're ready by the time the video starts.
  useEffect(() => {
    if (!animeTitle) return
    let cancelled = false
    setExtSubLoading(true)
    setExtSubs([])
    invoke<{ subs: ExtSubEntry[]; error?: string }>(IPC.SUBTITLE_SEARCH, {
      query: animeTitle,
      episodeNumber,
      seasonNumber,
      imdbId: imdbId || undefined,
      titleEn: animeTitle,
    }).then((res) => {
      if (!cancelled) setExtSubs(res?.subs || [])
    }).catch((err) => { console.warn('[subs] search failed:', err) })
      .finally(() => { if (!cancelled) setExtSubLoading(false) })
    return () => { cancelled = true }
  }, [animeTitle, episodeNumber, seasonNumber, imdbId])

  // Sidecar .srt/.vtt/.ass next to local videos
  useEffect(() => {
    setLocalSubs([])
    const candidates = [
      ...sidecarCandidates(activeStream.infoHash),
      ...sidecarCandidates(initialHttpUrl),
    ]
    if (!candidates.length) return
    let cancelled = false
    ;(async () => {
      const found: typeof localSubs = []
      for (const c of candidates) {
        if (cancelled) return
        try {
          const res = await fetch(c.url)
          if (!res.ok) continue
          const cues = parseSubtitleText(await res.text())
          if (!cues.length) continue
          const file = decodeURIComponent(c.url.split('/').pop() || 'subtitle')
          found.push({ id: 'sidecar-' + c.url, label: file, lang: c.lang, cues })
        } catch { /* no sidecar */ }
      }
      if (!cancelled && found.length) setLocalSubs(found)
    })()
    return () => { cancelled = true }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeStream.infoHash, initialHttpUrl, episodeNumber])

  // Online results grouped per language, most-downloaded first — one row per language.
  const onlineByLang = useMemo(() => {
    const groups = new Map<string, { code: string; name: string; entries: ExtSubEntry[] }>()
    for (const entry of extSubs) {
      const info = resolveLanguage(entry.lang, entry.langName)
      const code = info.code || entry.lang || 'und'
      const group = groups.get(code) || { code, name: info.code ? info.name : (entry.langName || code.toUpperCase()), entries: [] }
      group.entries.push(entry)
      groups.set(code, group)
    }
    for (const group of groups.values()) group.entries.sort((a, b) => (b.downloads || 0) - (a.downloads || 0))
    const preferred = resolveLanguage(preferencesRef.current?.subtitleLanguage || 'en').code
    return [...groups.values()].sort((a, b) =>
      Number(b.code === preferred) - Number(a.code === preferred)
      || Number(b.code === 'en') - Number(a.code === 'en')
      || a.name.localeCompare(b.name))
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [extSubs, prefsReady])

  const subtitleOptions = useMemo<SubtitleOption[]>(() => {
    const options: SubtitleOption[] = []
    for (const track of subtitleTracks) {
      const info = resolveLanguage(track.lang, track.label)
      const name = info.code ? info.name : (track.label || `Track ${track.idx + 1}`)
      const detail = [
        track.label && track.label.toLowerCase() !== name.toLowerCase() ? track.label : '',
        track.forced && !/forced/i.test(track.label) ? 'Forced' : '',
      ].filter(Boolean).join(' · ')
      options.push({
        key: `emb-${track.idx}`, lang: track.lang, label: name, group: 'embedded',
        detail: detail || undefined,
        disabledReason: track.imageBased ? 'Picture-based track — not supported' : undefined,
      })
    }
    for (const sub of localSubs) {
      const info = resolveLanguage(sub.lang)
      options.push({ key: sub.id, lang: sub.lang, label: info.code ? info.name : sub.label, detail: info.code ? sub.label : undefined, group: 'file' })
    }
    for (const group of onlineByLang) {
      options.push({
        key: `online-${group.code}`, lang: group.code, label: group.name, group: 'online',
        detail: group.entries.length > 1 ? `${group.entries.length} versions` : undefined,
      })
    }
    return options
  }, [subtitleTracks, localSubs, onlineByLang])

  /**
   * Stream an embedded track's cues from media time `from`, merged into `keep`
   * (cues already read from other parts of the file).
   */
  const extractEmbedded = useCallback(async (key: string, label: string, lang: string, from: number, keep: SubCue[]) => {
    subAbortRef.current?.abort()
    const controller = new AbortController()
    subAbortRef.current = controller
    embeddedRangeRef.current = { key, from }
    const index = Number(key.slice(4))
    if (!keep.length) setSubLoadingKey(key)
    setActiveSub({ key, label, lang, cues: keep })
    const source = activeStream.infoHash
    const startParam = from > 0 ? `&start=${Math.floor(from)}` : ''
    const url = source.startsWith('http')
      ? `mvstream://subtitle?url=${encodeURIComponent(source)}&index=${index}${startParam}`
      : `mvstream://subtitle?index=${index}${startParam}`
    const cueId = (c: SubCue) => `${c.start.toFixed(2)}|${c.text}`
    const merge = (fresh: SubCue[]) => {
      if (!keep.length) return fresh
      const seen = new Set(fresh.map(cueId))
      return [...keep.filter((c) => !seen.has(cueId(c))), ...fresh].sort((a, b) => a.start - b.start)
    }
    try {
      const cues = await streamSubtitleCues(url, (partial) => {
        if (controller.signal.aborted) return
        setSubLoadingKey((k) => (k === key ? null : k))
        setActiveSub((current) => (current?.key === key ? { ...current, cues: merge(partial) } : current))
      }, controller.signal)
      if (!controller.signal.aborted && cues.length === 0 && from === 0) throw new Error('no cues')
    } catch (err: any) {
      if (controller.signal.aborted) return
      console.warn('[subs] embedded extraction failed:', err?.message)
      if (!keep.length) {
        setActiveSub((current) => (current?.key === key ? null : current))
        showToast(`Couldn't read the ${label} track from this video`)
      }
    } finally {
      if (subAbortRef.current === controller) setSubLoadingKey(null)
    }
  }, [activeStream.infoHash, showToast])

  /** After a seek outside what has been extracted, restart extraction there. */
  const followEmbeddedSeek = (target: number) => {
    const range = embeddedRangeRef.current
    const sub = activeSubRef.current
    if (!range || sub?.key !== range.key) return
    const lastEnd = sub.cues.length ? sub.cues[sub.cues.length - 1].end : range.from
    if (target < range.from - 1 || target > lastEnd + 120) {
      void extractEmbedded(sub.key, sub.label, sub.lang, Math.max(0, target - 5), sub.cues)
    }
  }

  /** Activate a subtitle by option key (null = off). `auto` choices don't touch preferences. */
  const selectSubtitleKey = useCallback(async (key: string | null, auto = false, version = 0) => {
    subAbortRef.current?.abort()
    subAbortRef.current = null
    embeddedRangeRef.current = null
    setSubLoadingKey(null)
    if (!auto) userSubChoiceRef.current = true
    if (!key) {
      setActiveSub(null)
      if (!auto) void invoke(IPC.PLAYBACK_PREFERENCES_SET, { subtitlesEnabled: false })
      return
    }
    const option = subtitleOptions.find((o) => o.key === key)
    if (!option || option.disabledReason) return
    const remember = () => {
      if (auto) return
      const lang = resolveLanguage(option.lang, option.label).code
      void invoke(IPC.PLAYBACK_PREFERENCES_SET, { subtitlesEnabled: true, ...(lang ? { subtitleLanguage: lang } : {}) })
    }

    if (key.startsWith('emb-')) {
      remember()
      const active = sessionRef.current
      if (active) void invoke(IPC.PLAYBACK_SESSION_UPDATE, active.id, { selectedSubtitleIndex: Number(key.slice(4)) })
      // Start extracting at the playhead (or resume point) rather than byte 0.
      const live = transcodeOffsetRef.current + (videoRef.current?.currentTime || 0)
      const at = live > 1 ? live : (Number(resumePosition) || sessionRef.current?.positionSecs || 0)
      await extractEmbedded(key, option.label, option.lang, at > 30 ? at - 5 : 0, [])
      return
    }

    const local = localSubs.find((sub) => sub.id === key)
    if (local) {
      setActiveSub({ key, label: option.label, lang: local.lang, cues: local.cues })
      remember()
      return
    }

    if (key.startsWith('online-')) {
      const group = onlineByLang.find((g) => `online-${g.code}` === key)
      if (!group) return
      const controller = new AbortController()
      subAbortRef.current = controller
      setSubLoadingKey(key)
      // Try releases in order until one downloads and parses.
      const start = version % group.entries.length
      const ordered = [...group.entries.slice(start), ...group.entries.slice(0, start)]
      for (let i = 0; i < ordered.length; i++) {
        if (controller.signal.aborted) return
        const res = await invoke<{ vtt: string; error?: string }>(IPC.SUBTITLE_DOWNLOAD, ordered[i]).catch(() => null)
        if (controller.signal.aborted) return
        if (!res?.vtt || res.error || /^\s*</.test(res.vtt)) continue
        const cues = parseSubtitleText(res.vtt)
        if (!cues.length) continue
        onlineVersionRef.current = (start + i) % group.entries.length
        setActiveSub({ key, label: option.label, lang: group.code, cues })
        setSubLoadingKey(null)
        remember()
        if (version > 0) showToast(`${group.name} · version ${onlineVersionRef.current + 1} of ${group.entries.length}`)
        return
      }
      setSubLoadingKey(null)
      showToast(`No working ${group.name} subtitles found`)
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [subtitleOptions, localSubs, onlineByLang, showToast, extractEmbedded])

  const nextOnlineVersion = useCallback(() => {
    const key = activeSubRef.current?.key
    if (!key?.startsWith('online-')) return
    void selectSubtitleKey(key, false, onlineVersionRef.current + 1)
  }, [selectSubtitleKey])

  // Automatic subtitles: preferred language, embedded first (best sync), then
  // sidecar files, then the best online release. Audio already in the
  // preferred language only gets forced (signs / foreign dialogue) tracks.
  useEffect(() => {
    if (!prefsReady || !session || userSubChoiceRef.current || autoSubDoneRef.current || activeSubRef.current) return
    const prefs = preferencesRef.current
    if (prefs?.subtitlesEnabled === false) { autoSubDoneRef.current = true; return }
    const want = resolveLanguage(prefs?.subtitleLanguage || 'en').code || 'en'
    const textTracks = subtitleTracks.filter((t) => !t.imageBased)
    const audioLang = audioTracks.find((t) => t.idx === selectedAudioIndex)?.lang
      || session.probe?.tracks.find((t) => t.kind === 'audio' && t.index === selectedAudioIndex)?.language
    const pick = (key: string | undefined) => {
      autoSubDoneRef.current = true
      if (key) void selectSubtitleKey(key, true)
    }

    if (audioLang && sameLanguage(audioLang, want)) {
      const forced = textTracks.find((t) => t.forced && sameLanguage(t.lang, want))
      pick(forced ? `emb-${forced.idx}` : undefined)
      return
    }
    const matching = textTracks.filter((t) => sameLanguage(t.lang, want) || (!t.lang && resolveLanguage('', t.label).code === want))
    // Full dialogue beats "Signs & Songs" / forced-only tracks.
    const full = matching.find((t) => !t.forced && !/sign|song/i.test(t.label)) || matching.find((t) => !t.forced) || matching[0]
    if (full) { pick(`emb-${full.idx}`); return }
    const local = localSubs.find((sub) => !sub.lang || sameLanguage(sub.lang, want))
    if (local) { pick(local.id); return }
    if (extSubLoading) return // wait for the online search
    const online = onlineByLang.find((g) => g.code === want) || onlineByLang.find((g) => g.code === 'en')
    pick(online ? `online-${online.code}` : undefined)
  }, [prefsReady, session, subtitleTracks, audioTracks, selectedAudioIndex, localSubs, extSubLoading, onlineByLang, selectSubtitleKey])

  useEffect(() => () => subAbortRef.current?.abort(), [])

  const importSubtitleFile = useCallback(async () => {
    try {
      const res = await invoke<{ name: string; vtt: string } | null>(IPC.SUBTITLE_PICK_FILE)
      if (!res?.vtt) return
      const cues = parseSubtitleText(res.vtt)
      if (!cues.length) { showToast('That file had no readable subtitles'); return }
      const lang = resolveLanguage(res.name.match(/\.([a-z]{2,3}(?:-[a-z]{2})?)\.(?:srt|vtt|ass|ssa)$/i)?.[1] || '').code
      const sub = { id: `file-${res.name}`, label: res.name, lang, cues }
      setLocalSubs((list) => [...list.filter((x) => x.id !== sub.id), sub])
      subAbortRef.current?.abort()
      userSubChoiceRef.current = true
      setActiveSub({ key: sub.id, label: res.name, lang, cues })
      setShowTracks(false)
    } catch (err) {
      console.error('[subs] import failed:', err)
    }
  }, [showToast])

  const videoSrc = useMemo(() => {
    if (!streamUrl) return ''
    if (transcodeMode) {
      const mode = transcodeKind
      const videoCodec = session?.probe?.tracks.find((track) => track.kind === 'video')?.codec || ''
      const copiesVideo = mode === 'remux' || mode === 'audio-transcode'
      const vtag = copiesVideo && /^(hevc|h265)$/.test(videoCodec) ? '&vtag=hvc1' : ''
      const query = `audio=${selectedAudioIndex}&mode=${mode}&start=${transcodeOffset}${vtag}`
      if (streamUrl.startsWith('mvstream://')) {
        try {
          const u = new URL(streamUrl)
          const nestedUrl = u.searchParams.get('url')
          if (nestedUrl) {
            return `mvstream://transcode?url=${encodeURIComponent(nestedUrl)}&${query}`
          }
        } catch { /* fallback */ }
        return `mvstream://transcode?${query}`
      }
      return `mvstream://transcode?url=${encodeURIComponent(streamUrl)}&${query}`
    }
    return streamUrl
  }, [streamUrl, transcodeMode, transcodeKind, session?.probe, selectedAudioIndex, transcodeOffset])

  // Any reload of the <video> (conversion fallback, audio switch, session info
  // arriving, restart after a seek) must buffer and start playing again —
  // otherwise it sits paused on a black frame with no indicator.
  const lastVideoSrcRef = useRef('')
  useEffect(() => {
    const previous = lastVideoSrcRef.current
    lastVideoSrcRef.current = videoSrc
    if (!previous || !videoSrc || previous === videoSrc) return
    const wasPlaying = hasStartedRef.current || midStreamStallRef.current
    hasStartedRef.current = false
    setBufferFillPct(0)
    setPrebuffering(true)
    if (wasPlaying && !loading) {
      midStreamStallRef.current = true
      setIsMidStreamStall(true)
      setBufferPhraseIdx(0)
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [videoSrc])

  // Audio languages other sources carry that this file doesn't.
  const otherSourceAudio = useMemo(() => {
    const have = new Set(audioTracks.map((track) => resolveLanguage(track.lang, track.label).code).filter(Boolean))
    if (!have.size) for (const lang of streamAudioLanguages(activeStream)) have.add(lang)
    const picks = new Map<string, AnimeTorrentStream>()
    const candidates = (availableStreams as AnimeTorrentStream[]).filter((stream) =>
      hasPlayableLink(stream) && !(stream.infoHash === activeStream.infoHash && stream.fileIdx === activeStream.fileIdx))
    // Prefer a source at the current quality, then the best-ranked one.
    for (const sameQuality of [true, false]) {
      for (const stream of candidates) {
        if (sameQuality && stream.quality !== activeStream.quality) continue
        for (const lang of streamAudioLanguages(stream)) if (!have.has(lang) && !picks.has(lang)) picks.set(lang, stream)
      }
    }
    return [...picks].map(([lang, stream]) => ({ lang, stream }))
  }, [audioTracks, availableStreams, activeStream])

  const switchToAudioSource = (lang: string) => {
    const pick = otherSourceAudio.find((option) => option.lang === lang)
    if (!pick) return
    pendingAudioLangRef.current = lang
    setSwitchingAudioLang(lang)
    if (preferencesRef.current) preferencesRef.current = { ...preferencesRef.current, audioLanguage: lang }
    void invoke(IPC.PLAYBACK_PREFERENCES_SET, { audioLanguage: lang })
    showToast(`Switching to ${resolveLanguage(lang).name} audio${pick.stream.quality ? ` · ${pick.stream.quality}` : ''}…`)
    setShowTracks(false)
    switchStream(pick.stream)
  }

  // Loading stages → one overall progress value for the pixel loader.
  const isTorrentSource = !activeStream.infoHash.startsWith('http')
  const loadProgress = (() => {
    const peers = torrentStatus?.peers || 0
    const stage = prebuffering && !loading ? 3
      : session || torrentStatus?.ready ? 2
        : isTorrentSource && peers > 0 ? 1 : 0
    const progress = stage === 3 ? 0.35 + bufferFillPct * 0.65
      : stage === 2 ? 0.3
        : stage === 1 ? 0.12 + Math.min(peers, 40) / 40 * 0.13 : 0.04
    const labels = isTorrentSource ? ['Connecting', 'Peers', 'Preparing', 'Buffering'] : ['Connecting', 'Preparing', 'Buffering']
    const at = isTorrentSource ? stage : Math.max(0, stage - 1)
    return {
      progress,
      steps: labels.map((label, i) => ({ label, state: (i < at ? 'done' : i === at ? 'active' : 'todo') as 'done' | 'active' | 'todo' })),
      title: stage === 3 ? 'Buffering' : statusText || 'Connecting…',
    }
  })()

  const closePanels = () => { setShowTracks(false); setShowSettings(false); setShowSources(false) }
  const videoTrack = session?.probe?.tracks.find((track: MediaTrack) => track.kind === 'video')
  const qualityLabel = (() => {
    // Wide (cinemascope) frames are short: rate by the 16:9-equivalent height.
    const h = videoTrack?.height ? Math.max(videoTrack.height, Math.round((videoTrack.width || 0) * 9 / 16)) : 0
    if (h) return h >= 2000 ? '4K' : h >= 1400 ? '1440p' : h >= 1000 ? '1080p' : h >= 700 ? '720p' : `${h}p`
    return activeStream.quality || ''
  })()

  const displayTitle = episodeNumber != null
    ? `${animeTitle} — Episode ${episodeNumber}${episodeTitle ? `: ${episodeTitle}` : ''}`
    : animeTitle || 'Playing'

  return (
    <div
      ref={containerRef}
      className="flex flex-col h-full bg-black relative select-none overflow-hidden"
      onMouseMove={resetHideTimer}
      onClick={resetHideTimer}
      style={{ cursor: showControls ? 'default' : 'none' }}
    >
      {/* ── Mid-stream buffering overlay — minimal, transparent, text only ── */}
      <AnimatePresence>
      {isMidStreamStall && prebuffering && (
        <motion.div
          key="midstream-overlay"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.35 }}
          className="absolute inset-0 z-20 flex flex-col items-center justify-end pb-28 pointer-events-none"
          style={{ background: 'rgba(0,0,0,0.55)' }}
        >
          <div className="absolute left-1/2 top-1/2 -ml-7 -mt-7 w-14 h-14 rounded-full animate-spin"
            style={{ border: '3px solid rgba(255,255,255,0.12)', borderTopColor: 'var(--accent-hover)' }} />
          <AnimatePresence mode="wait">
            <motion.p
              key={bufferPhraseIdx}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.4, ease: [0.22, 1, 0.36, 1] }}
              className="text-sm font-medium text-white/75 tracking-widest uppercase"
            >
              {BUFFER_PHRASES[bufferPhraseIdx]}
            </motion.p>
          </AnimatePresence>
          {/* tiny progress bar at the very bottom */}
          <div className="absolute bottom-0 left-0 right-0 h-0.5" style={{ background: 'rgba(255,255,255,0.08)' }}>
            <motion.div
              className="h-full"
              animate={{ width: `${Math.round(bufferFillPct * 100)}%` }}
              transition={{ duration: 0.3, ease: 'easeOut' }}
              style={{ background: 'rgba(200, 80, 26,0.7)' }}
            />
          </div>
        </motion.div>
      )}
      </AnimatePresence>

      {/* ── Initial loading / Error overlay ── */}
      <AnimatePresence>
      {!isMidStreamStall && (loading || error || prebuffering) && (
        <motion.div
          key="loading-overlay"
          initial={{ opacity: 0, scale: 1.02 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 1.05 }}
          transition={{ duration: 0.4, ease: [0.22, 1, 0.36, 1] }}
          className="absolute inset-0 z-20 overflow-hidden">
          {/* Full-bleed backdrop — no blur, just dark overlay */}
          <div className="absolute inset-0 bg-black" />
          {coverUrl && (
            <>
              <motion.img
                src={coverUrl}
                alt=""
                initial={{ scale: 1.08, opacity: 0 }}
                animate={{ scale: 1.0, opacity: 0.18 }}
                transition={{ duration: 1.2, ease: [0.22, 1, 0.36, 1] }}
                className="absolute inset-0 w-full h-full object-cover"
              />
              {/* Dark gradient so title pops */}
              <div
                className="absolute inset-0"
                style={{
                  background:
                    'linear-gradient(180deg, rgba(0,0,0,0.7) 0%, rgba(0,0,0,0.5) 40%, rgba(0,0,0,0.85) 100%)',
                }}
              />
            </>
          )}

          {/* Center content */}
          <div className="relative z-10 flex flex-col items-center justify-center h-full px-8">
            {/* Poster + hero title (Stremio layout) */}
            <motion.div
              initial={{ opacity: 0, y: 24 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.8, delay: 0.1, ease: [0.22, 1, 0.36, 1] }}
              className="flex flex-col items-center gap-6 max-w-2xl text-center"
            >
              {coverUrl && (
                <motion.div
                  initial={{ scale: 0.9, opacity: 0 }}
                  animate={{ scale: 1, opacity: 1 }}
                  transition={{ duration: 0.7, delay: 0.15, ease: [0.22, 1, 0.36, 1] }}
                  className="relative"
                >
                  <img
                    src={coverUrl}
                    alt={animeTitle}
                    className="w-40 h-56 rounded-2xl object-cover"
                    style={{
                      boxShadow:
                        '0 30px 80px rgba(0,0,0,0.7), 0 0 0 1px rgba(255,255,255,0.08), 0 0 60px rgba(200,80,26,0.25)',
                    }}
                  />
                  <div className="absolute -inset-0.5 rounded-2xl border border-white/10 pointer-events-none" />
                </motion.div>
              )}

              {/* Hero title — big display text */}
              <div className="space-y-2">
                <motion.h1
                  initial={{ opacity: 0, y: 12 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ duration: 0.6, delay: 0.2, ease: [0.22, 1, 0.36, 1] }}
                  className="text-4xl md:text-5xl font-black text-white tracking-tight leading-tight"
                  style={{
                    textShadow: '0 4px 24px rgba(0,0,0,0.8), 0 0 40px rgba(200,80,26,0.3)',
                    fontFamily: 'system-ui, -apple-system, sans-serif',
                  }}
                >
                  {animeTitle || displayTitle}
                </motion.h1>
                {episodeNumber != null && (
                  <motion.p
                    initial={{ opacity: 0, y: 8 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ duration: 0.6, delay: 0.5 }}
                    className="text-sm md:text-base font-medium text-[color:var(--accent-hover)] uppercase tracking-[0.2em]"
                  >
                    Episode {episodeNumber}{episodeTitle ? ` — ${episodeTitle}` : ''}
                  </motion.p>
                )}
              </div>

              {/* Status / buffer — minimal, centered */}
              {error ? (
                <motion.div
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  transition={{ delay: 0.3 }}
                  className="flex flex-col items-center gap-3 mt-2"
                >
                  <div className="flex flex-col items-center gap-3 px-6 py-5 rounded-xl max-w-md" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
                    <AlertCircle size={22} style={{ color: 'var(--accent-hover)' }} />
                    <p className="text-sm font-semibold text-[color:var(--text-1)]">Playback failed</p>
                    <p className="text-xs text-[color:var(--text-2)] text-center leading-relaxed">{error}</p>
                    <div className="flex items-center gap-2 mt-1">
                      <button
                        onClick={handleRetry}
                        className="px-4 py-2 rounded-[10px] text-xs font-semibold text-white transition-colors hover:brightness-110"
                        style={{ background: 'var(--accent)' }}
                      >
                        Retry
                      </button>
                      <button
                        onClick={() => void handleBack()}
                        className="px-4 py-2 rounded-[10px] text-xs text-[color:var(--text-1)] transition-colors hover:bg-white/10"
                        style={{ background: 'var(--surface-4)' }}
                      >
                        Go back
                      </button>
                    </div>
                  </div>
                </motion.div>
              ) : (
                <motion.div
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  transition={{ duration: 0.6, delay: 0.55 }}
                  className="flex flex-col items-center w-full max-w-xl mt-2"
                >
                  <PixelLoader
                    progress={loadProgress.progress}
                    steps={loadProgress.steps}
                    title={loadProgress.title}
                    detail={torrentStatus && isTorrentSource ? (
                      <div className="flex items-center justify-center gap-5 text-[11px] text-white/50 tabular-nums">
                        <span className="flex items-center gap-1.5"><Users size={11} /> {torrentStatus.peers} peers</span>
                        <span className="flex items-center gap-1.5"><Download size={11} /> {formatBytes(torrentStatus.downloadSpeed || 0)}</span>
                      </div>
                    ) : undefined}
                  />
                </motion.div>
              )}
            </motion.div>
          </div>

          {/* Back button — floating top-left */}
          <button
            onClick={() => void handleBack()}
            className="absolute top-3 left-4 z-20 p-2 rounded-[10px] text-[color:var(--text-1)] hover:text-white transition-colors"
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
            title="Back"
          >
            <ArrowLeft size={16} />
          </button>
        </motion.div>
      )}
      </AnimatePresence>

      {/* ── Video ── */}
      {streamUrl && !error && (
      <video
        key={videoSessionKey}
        ref={videoRef}
        src={videoSrc}
          className="flex-1 w-full h-full object-contain bg-black"
          playsInline
          preload="auto"
          controls={false}
          onPlay={() => { setPlaying(true); userPausedRef.current = false; stallPausedRef.current = false; if (stallTimerRef.current) clearTimeout(stallTimerRef.current) }}
          onPause={() => { setPlaying(false); if (!stallPausedRef.current) userPausedRef.current = true }}
          onVolumeChange={() => {
            const v = videoRef.current
            if (!v) return
            setMuted(v.muted)
            setVolume(v.volume)
          }}
          onTimeUpdate={() => {
            const localTime = videoRef.current?.currentTime || 0
            const displayedTime = Math.min(duration || Number.MAX_SAFE_INTEGER, transcodeOffset + localTime)
            setCurrentTime(displayedTime)
            const active = sessionRef.current
            const now = Date.now()
            if (active && now - lastSessionPersistRef.current >= 5000) {
              lastSessionPersistRef.current = now
              void invoke(IPC.PLAYBACK_SESSION_UPDATE, active.id, {
                positionSecs: displayedTime,
                state: 'playing',
              })
            }
            trackPlayback()
          }}
          onDurationChange={() => {
            const browserDuration = videoRef.current?.duration || 0
            const canonical = sessionRef.current?.probe?.durationSecs
            if (!canonical && Number.isFinite(browserDuration) && browserDuration > 0) {
              setDuration(browserDuration)
            }
          }}
          onLoadedMetadata={handleLoadedMetadata}
          onProgress={() => {
            const v = videoRef.current
            if (!v) return
            handleProgress()
            resumeFromBuffered(v, 'progress')
          }}
          onCanPlayThrough={() => {
            const v = videoRef.current
            if (v) resumeFromBuffered(v, 'canplaythrough', true)
          }}
          onError={(e) => {
            if (error) return
            const v = e.currentTarget
            const code = v.error?.code
            const msg = v.error?.message || ''
            const codeStr = code === 1 ? 'ABORTED' : code === 2 ? 'NETWORK' : code === 3 ? 'DECODE' : code === 4 ? 'SRC_NOT_SUPPORTED' : 'UNKNOWN'
            console.error(`[player] video error: ${codeStr} — ${msg}`)

            // ── Auto-fallback to ffmpeg transcoding ──
            // If this is a decode / unsupported-format failure AND we haven't
            // already tried the transcode path, transparently switch to it.
            // ffmpeg converts HEVC/VP9/exotic containers to H.264+AAC fragmented
            // MP4 which every browser can play. The user never sees an error
            // screen — they just see a brief "Preparing compatible stream…"
            // while ffmpeg spins up, then playback resumes.
            const isDecodeFailure = code === 3 || code === 4
            if (isDecodeFailure && !transcodeAttemptedRef.current) {
              transcodeAttemptedRef.current = true
              console.warn('[player] native decode failed — switching to ffmpeg transcode fallback')
              // Resume the converted stream where the native one broke off.
              if (!transcodeMode) {
                const resumeAt = Math.max(0, Number(resumePosition) || sessionRef.current?.positionSecs || 0)
                setTranscodeOffset(v.currentTime > 1 ? v.currentTime : resumeAt)
              }
              // H.264 the browser rejected only because of its container (MKV/AVI…):
              // repackage it instead of re-encoding. A failure escalates to full below.
              const videoCodec = sessionRef.current?.probe?.tracks.find((track) => track.kind === 'video')?.codec || ''
              if (code === 4 && /^(h264|avc)/.test(videoCodec)) setCopyVideo(true)
              setStatusText('Preparing compatible stream…')
              setError(null)
              setLoading(true)
              setPrebuffering(true)
              setBufferFillPct(0)
              hasStartedRef.current = false
              setTranscodeMode(true)
              return
            }

            // If we already tried transcode and it still failed, show error
            if (transcodeAttemptedRef.current && isDecodeFailure) {
              // A copy-based path (remux / audio-only conversion) can fail when
              // the GPU decoder rejects the stream — re-encode before giving up.
              const copiedVideo = transcodeKind === 'remux' || transcodeKind === 'audio-transcode'
              if (transcodeMode && copiedVideo && !forceFullTranscode) {
                console.warn('[player] copied video failed to decode — escalating to full transcode')
                setStatusText('Preparing compatible stream…')
                setLoading(true)
                setPrebuffering(true)
                setBufferFillPct(0)
                hasStartedRef.current = false
                setForceFullTranscode(true)
                return
              }
              const nextSource = nextUntriedSource()
              if (nextSource) {
                setStatusText('Current source failed. Trying the next best source…')
                switchStream(nextSource, true)
                return
              }
              const message = 'This source could not be decoded after compatibility conversion. Choose another source or return to the catalog.'
              setError(message)
              const active = sessionRef.current
              if (active) void invoke(IPC.PLAYBACK_SESSION_FAILURE, active.id, {
                code: codeStr,
                message,
                recoverable: true,
                occurredAt: Date.now(),
              })
              return
            }

            if (code === 4) {
              setError('Video format not supported. Try a different stream.')
            } else {
              const message = `Playback error (${codeStr}): ${msg || 'unknown error'}`
              setError(message)
              const active = sessionRef.current
              if (active) void invoke(IPC.PLAYBACK_SESSION_FAILURE, active.id, {
                code: codeStr,
                message,
                recoverable: true,
                occurredAt: Date.now(),
              })
            }
          }}
          onStalled={handleStalled}
          onWaiting={handleStalled}
          onEnded={() => { trackPlayback(true); if (hasNextEp && autoPlayEnabled && !nextEpCancelledRef.current) playNextEpisode() }}
        />
      )}

      {/* ── Subtitles (drawn by the player, follows the absolute timeline) ── */}
      {streamUrl && !error && (
        <SubtitleOverlay
          cues={activeSub?.cues || null}
          getTime={() => transcodeOffsetRef.current + (videoRef.current?.currentTime || 0)}
          offset={subtitleOffset}
          size={subtitleSize}
          color={subColor}
          background={subBg}
          bottomPct={subtitleVPos}
          lifted={showControls}
        />
      )}

      {/* ── Audio & subtitles panel ── */}
      <AnimatePresence>
        {showTracks && (
          <TrackPanel
            audio={audioTracks}
            selectedAudio={selectedAudioIndex}
            onSelectAudio={(idx) => { if (idx !== selectedAudioIndex) { selectAudio(idx); setShowTracks(false) } }}
            otherAudio={otherSourceAudio.map(({ lang, stream }) => ({
              lang,
              detail: [stream.quality, streamAudioLanguages(stream).length > 1 ? 'multi-audio source' : 'other source'].filter(Boolean).join(' · '),
            }))}
            switchingAudio={switchingAudioLang}
            onSelectOtherAudio={switchToAudioSource}
            subtitles={subtitleOptions}
            activeSubtitle={activeSub?.key || null}
            loadingSubtitle={subLoadingKey}
            onlineLoading={extSubLoading}
            onSelectSubtitle={(key) => { void selectSubtitleKey(key) }}
            onNextVersion={nextOnlineVersion}
            onImport={() => { void importSubtitleFile() }}
            style={{
              offset: subtitleOffset, size: subtitleSize, color: subColor, background: subBg, position: subtitleVPos,
              onOffset: adjustSubtitleOffset, onResetOffset: resetSubtitleOffset,
              onSize: setSubtitleSize, onPosition: setSubtitleVPos,
              onColor: (c) => { setSubColor(c); savePlayerCfg({ subColor: c }) },
              onBackground: (b) => { setSubBg(b); savePlayerCfg({ subBg: b }) },
            }}
            onClose={() => setShowTracks(false)}
          />
        )}
      </AnimatePresence>

      {/* ── Click surface: single click = play/pause, double click = fullscreen ── */}
      {streamUrl && !error && !loading && !(prebuffering && !isMidStreamStall) && (
        <div
          className="absolute inset-0 z-[25]"
          onClick={() => {
            if (clickTimer.current) clearTimeout(clickTimer.current)
            clickTimer.current = setTimeout(() => {
              if (showTracks || showSettings || showSources) { closePanels(); return }
              togglePlay()
            }, 220)
          }}
          onDoubleClick={() => {
            if (clickTimer.current) clearTimeout(clickTimer.current)
            toggleFullscreen()
          }}
        />
      )}

      {/* ── Big centered play/pause pulse ── */}
      <AnimatePresence>
        {pulse && (
          <motion.div
            key={pulse.n}
            initial={{ opacity: 0.85, scale: 0.8 }}
            animate={{ opacity: 0, scale: 1.35 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.5, ease: 'easeOut' }}
            className="absolute left-1/2 top-1/2 z-[28] -ml-9 -mt-9 w-[72px] h-[72px] rounded-full flex items-center justify-center pointer-events-none"
            style={{ background: 'rgba(0,0,0,0.45)', backdropFilter: 'blur(6px)', border: '1px solid rgba(255,255,255,0.12)' }}
          >
            {pulse.kind === 'play' ? <Play size={30} fill="white" color="white" className="ml-1" /> : <Pause size={30} fill="white" color="white" />}
          </motion.div>
        )}
      </AnimatePresence>

      {/* ── Paused: title / episode overlay ── */}
      <AnimatePresence>
        {!playing && streamUrl && !error && !loading && !prebuffering && !showEpisodes && !showTracks && (
          <motion.div
            key="pause-card"
            initial={{ opacity: 0, x: -16 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -16 }}
            transition={{ duration: 0.35, delay: 0.5 }}
            className="absolute left-12 top-1/2 -translate-y-1/2 z-[26] max-w-md pointer-events-none"
          >
            <p className="text-[10px] uppercase tracking-[0.25em] text-white/55">You're watching</p>
            <h2 className="text-3xl font-black text-white leading-tight mt-1" style={{ textShadow: '0 4px 24px rgba(0,0,0,0.85)' }}>
              {animeTitle || 'Playing'}
            </h2>
            {episodeNumber != null && (
              <p className="mt-1 text-sm font-semibold text-white/80">
                Episode {episodeNumber}{episodeTitle ? ' · ' + episodeTitle : ''}
              </p>
            )}
            {description && <p className="mt-3 text-xs text-white/60 leading-relaxed line-clamp-3">{description}</p>}
          </motion.div>
        )}
      </AnimatePresence>

      {/* ── Transient toast (subtitle delay, auto-skip) ── */}
      <AnimatePresence>
        {toastMsg && (
          <motion.div
            key="toast"
            initial={{ opacity: 0, y: -8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8 }}
            className="absolute top-20 left-1/2 -translate-x-1/2 z-[48] px-3.5 py-2 rounded-full text-xs font-medium text-white pointer-events-none"
            style={{ background: 'rgba(14,14,16,0.85)', border: '1px solid rgba(255,255,255,0.1)', backdropFilter: 'blur(12px)' }}
          >
            {toastMsg}
          </motion.div>
        )}
      </AnimatePresence>

      {/* ── Controls overlay — z-30 so it's always above the loading overlay (z-20) ── */}
      <div className="absolute inset-0 z-30 flex flex-col justify-between pointer-events-none">
        <motion.div
          animate={{ opacity: showControls ? 1 : 0 }}
          transition={{ duration: 0.3, ease: [0.22, 1, 0.36, 1] }}
          className="absolute top-0 left-0 right-0 h-28 pointer-events-none"
          style={{ background: 'linear-gradient(180deg, rgba(0,0,0,0.7) 0%, rgba(0,0,0,0.3) 55%, transparent 100%)' }}
        />
        <motion.div
          animate={{ opacity: showControls ? 1 : 0 }}
          transition={{ duration: 0.3, ease: [0.22, 1, 0.36, 1] }}
          className="absolute bottom-0 left-0 right-0 h-44 pointer-events-none"
          style={{ background: 'linear-gradient(0deg, rgba(0,0,0,0.85) 0%, rgba(0,0,0,0.45) 50%, transparent 100%)' }}
        />

        {/* Top bar: back, title, stream status */}
        <motion.div
          animate={{ y: showControls ? 0 : -16, opacity: showControls ? 1 : 0 }}
          transition={{ duration: 0.3, ease: [0.22, 1, 0.36, 1] }}
          className={`relative flex items-center gap-3 px-5 pt-4 ${showControls ? 'pointer-events-auto' : 'pointer-events-none'}`}
        >
          <CtrlButton onClick={() => void handleBack()} title="Back (Esc)">
            <ArrowLeft size={20} />
          </CtrlButton>
          <div className="flex-1 min-w-0">
            <h3 className="text-[15px] font-bold text-white truncate leading-tight" style={{ textShadow: '0 2px 12px rgba(0,0,0,0.7)' }}>
              {animeTitle || 'Playing'}
            </h3>
            {episodeNumber != null && (
              <p className="text-[12px] text-white/65 font-medium truncate mt-0.5">
                Episode {episodeNumber}{episodeTitle ? ` · ${episodeTitle}` : ''}
              </p>
            )}
          </div>
          <div className="flex items-center gap-2 shrink-0">
            {torrentStatus && !activeStream.infoHash.startsWith('http') && (
              <span className="hidden md:flex items-center gap-2.5 h-7 px-3 rounded-full text-[11px] text-white/70 font-medium tabular-nums"
                style={{ background: 'rgba(255,255,255,0.08)' }} title="Peers / download speed">
                <span className="flex items-center gap-1"><Users size={11} /> {torrentStatus.peers}</span>
                <span className="flex items-center gap-1"><Download size={11} /> {formatBytes(torrentStatus.downloadSpeed || 0)}</span>
              </span>
            )}
            {qualityLabel && (
              <span className="h-7 px-2.5 rounded-full flex items-center text-[11px] font-bold tracking-wide text-white"
                style={{ background: 'rgba(255,255,255,0.12)', border: '1px solid rgba(255,255,255,0.14)' }}>
                {qualityLabel}
              </span>
            )}
          </div>
        </motion.div>

        {/* Bottom controls */}
        <motion.div
          animate={{ y: showControls ? 0 : 16, opacity: showControls ? 1 : 0 }}
          transition={{ duration: 0.3, ease: [0.22, 1, 0.36, 1] }}
          className={`relative px-5 pb-4 ${showControls ? 'pointer-events-auto' : 'pointer-events-none'}`}
        >
          {/* Seek bar: click or drag to scrub */}
          <div className="flex items-center gap-3 mb-1.5">
            <span className="text-[12px] text-white/90 font-medium tabular-nums w-14 text-right shrink-0">{formatTime(scrubbing && seekHover != null ? seekHover : currentTime)}</span>
            <div
              ref={seekBarRef}
              className="group relative flex-1 h-5 flex items-center cursor-pointer touch-none"
              onPointerDown={(e) => {
                const t = timeFromPointer(e.clientX)
                if (t == null) return
                try { e.currentTarget.setPointerCapture(e.pointerId) } catch { /* synthetic pointer */ }
                scrubbingRef.current = true
                setScrubbing(true)
                setSeekHover(t)
              }}
              onPointerMove={(e) => { const t = timeFromPointer(e.clientX); if (t != null) setSeekHover(t) }}
              onPointerUp={(e) => {
                if (!scrubbingRef.current) return
                const t = timeFromPointer(e.clientX)
                scrubbingRef.current = false
                setScrubbing(false)
                if (t != null) seekTo(t)
              }}
              onPointerCancel={() => { scrubbingRef.current = false; setScrubbing(false); setSeekHover(null) }}
              onPointerLeave={() => { if (!scrubbingRef.current) setSeekHover(null) }}
            >
              <div className={`relative w-full rounded-full overflow-hidden transition-[height] duration-150 ${scrubbing ? 'h-[6px]' : 'h-[4px] group-hover:h-[6px]'}`}
                style={{ background: 'rgba(255,255,255,0.18)' }}>
                <div className="absolute inset-y-0 left-0 rounded-full" style={{ width: duration ? `${Math.min(100, ((transcodeOffset + buffered) / duration) * 100)}%` : '0%', background: 'rgba(255,255,255,0.3)' }} />
                {seekHover != null && !scrubbing && (
                  <div className="absolute inset-y-0 left-0" style={{ width: `${(seekHover / (duration || 1)) * 100}%`, background: 'rgba(255,255,255,0.18)' }} />
                )}
                <div className="absolute inset-y-0 left-0 rounded-full" style={{ width: duration ? `${Math.min(100, ((scrubbing && seekHover != null ? seekHover : currentTime) / duration) * 100)}%` : '0%', background: 'var(--accent)' }} />
                {duration > 0 && skipSegments.map((seg) => {
                  const kind = kindOf(seg.type)
                  return (
                    <div
                      key={`${seg.type}-${seg.start}`}
                      className="absolute inset-y-0 pointer-events-none"
                      style={{
                        left: `${(seg.start / duration) * 100}%`,
                        width: `${((Math.min(seg.end, duration) - seg.start) / duration) * 100}%`,
                        background: SKIP_COLOR[kind], opacity: 0.55,
                      }}
                    />
                  )
                })}
              </div>
              {/* Thumb */}
              <div
                className={`absolute top-1/2 w-3.5 h-3.5 -ml-[7px] -mt-[7px] rounded-full bg-white pointer-events-none transition-transform duration-150 ${scrubbing ? 'scale-110' : 'scale-0 group-hover:scale-100'}`}
                style={{ left: duration ? `${Math.min(100, ((scrubbing && seekHover != null ? seekHover : currentTime) / duration) * 100)}%` : '0%', boxShadow: '0 0 0 3px rgba(var(--accent-rgb),0.35), 0 2px 6px rgba(0,0,0,0.5)' }}
              />
              {/* Hover preview: time + chapter label */}
              {seekHover != null && (() => {
                const seg = skipSegments.find((x) => seekHover >= x.start && seekHover <= x.end)
                const pct = Math.max(3, Math.min(97, (seekHover / (duration || 1)) * 100))
                return (
                  <div
                    className="absolute bottom-6 px-2.5 py-1 rounded-md text-center pointer-events-none whitespace-nowrap"
                    style={{ left: `${pct}%`, transform: 'translateX(-50%)', background: 'rgba(14,14,16,0.92)', border: '1px solid rgba(255,255,255,0.1)' }}
                  >
                    <div className="text-[12px] text-white font-semibold tabular-nums">{formatTime(seekHover)}</div>
                    {seg && <div className="text-[9px] font-bold uppercase tracking-wider" style={{ color: SKIP_COLOR[kindOf(seg.type)] }}>{SKIP_LABEL[kindOf(seg.type)].replace('Skip ', '')}</div>}
                  </div>
                )
              })()}
            </div>
            <span className="text-[12px] text-white/60 font-medium tabular-nums w-14 shrink-0">{formatTime(duration)}</span>
          </div>

          <div className="flex items-center gap-1">
            <CtrlButton onClick={togglePlay} title={playing ? 'Pause (Space)' : 'Play (Space)'} size="lg">
              {playing ? <Pause size={26} fill="currentColor" /> : <Play size={26} fill="currentColor" className="ml-0.5" />}
            </CtrlButton>
            <SeekIcon dir="back" size={24} onClick={() => seekTo(currentTime - 10)} title="Back 10s (J)" />
            <div className="w-2" />
            <SeekIcon dir="forward" size={24} onClick={() => seekTo(currentTime + 10)} title="Forward 10s (L)" />

            {/* Volume — hover to reveal the slider, boost to 200% */}
            <div className="group/vol flex items-center ml-2">
              <CtrlButton
                onClick={() => { const v = videoRef.current; if (!v) return; v.muted = !v.muted; setMuted(v.muted) }}
                title={muted ? 'Unmute (M)' : 'Mute (M)'}
              >
                {muted || volume === 0 ? <VolumeX size={20} /> : volume < 0.5 ? <Volume1 size={20} /> : <Volume2 size={20} />}
              </CtrlButton>
              <div className="flex items-center w-0 opacity-0 group-hover/vol:w-[132px] group-hover/vol:opacity-100 focus-within:w-[132px] focus-within:opacity-100 overflow-hidden transition-all duration-200">
                <input
                  type="range" min="0" max="2" step="0.05"
                  value={muted ? 0 : volume}
                  onChange={(e) => setVolumeSmooth(parseFloat(e.target.value))}
                  className="w-24 ml-1 cursor-pointer appearance-none h-1 rounded-full outline-none"
                  style={{
                    background: `linear-gradient(90deg, ${volume > 1 ? '#f59e0b' : 'var(--accent)'} ${((muted ? 0 : volume) / 2) * 100}%, rgba(255,255,255,0.2) ${((muted ? 0 : volume) / 2) * 100}%)`,
                  }}
                />
                <span className={`ml-2 text-[10px] tabular-nums w-8 shrink-0 ${volume > 1 ? 'text-amber-400 font-bold' : 'text-white/60'}`}>
                  {Math.round((muted ? 0 : volume) * 100)}%
                </span>
              </div>
            </div>

            <div className="flex-1" />

            {hasNextEp && (
              <CtrlButton onClick={playNextEpisode} title="Next episode (N)">
                <SkipForward size={20} />
              </CtrlButton>
            )}

            {episodeList && episodeList.length > 1 && (
              <CtrlButton
                active={showEpisodes}
                onClick={() => { setShowEpisodes((v) => !v); closePanels() }}
                title="Episodes (E)"
              >
                <ListOrdered size={20} />
              </CtrlButton>
            )}

            {/* Audio & subtitles */}
            <CtrlButton
              active={showTracks || !!activeSub}
              onClick={() => { const open = !showTracks; closePanels(); setShowTracks(open) }}
              title="Audio & subtitles (C cycles subtitles)"
            >
              <span className="relative inline-flex">
                <Subtitles size={20} />
                {activeSub && (
                  <span className="absolute -right-2 -bottom-1.5"><LangFlag lang={activeSub.lang} hint={activeSub.label} size={12} /></span>
                )}
              </span>
            </CtrlButton>

            {availableStreams.length > 1 && (
              <div className="relative">
                <CtrlButton
                  active={showSources}
                  onClick={() => { const open = !showSources; closePanels(); setShowSources(open) }}
                  title="Source / quality"
                >
                  <ListVideo size={20} />
                </CtrlButton>
                <AnimatePresence>
                  {showSources && (
                    <motion.div {...dropdownMotion}
                      className="absolute bottom-full right-0 mb-3 p-1.5 rounded-2xl w-[380px] max-h-[360px] overflow-y-auto"
                      style={PANEL_STYLE}
                      onClick={(e) => e.stopPropagation()}>
                      <div className="px-2.5 pt-1.5 pb-2 text-[10px] text-white/40 uppercase tracking-[0.16em] font-bold">Sources</div>
                      {availableStreams.map((stream: AnimeTorrentStream, index: number) => {
                        const selected = stream.infoHash === activeStream.infoHash && stream.fileIdx === activeStream.fileIdx
                        return (
                          <button key={`${stream.infoHash}-${stream.fileIdx ?? index}`}
                            onClick={() => switchStream(stream)}
                            className={`w-full text-left px-2.5 h-11 rounded-lg flex items-center gap-2.5 transition-colors ${selected ? 'bg-white/[0.09] text-white' : 'text-white/75 hover:bg-white/[0.05] hover:text-white'}`}>
                            <span className="w-4 shrink-0">{selected && <Check size={14} strokeWidth={3} style={{ color: 'var(--accent-hover)' }} />}</span>
                            <span className="w-12 shrink-0 text-[11px] font-bold">{stream.quality || 'Auto'}</span>
                            <span className="min-w-0 flex-1 truncate text-[12px]">{stream.title.split('\n')[0]}</span>
                            <span className="flex items-center gap-0.5 shrink-0">
                              {streamAudioLanguages(stream).slice(0, 4).map((lang) => <LangFlag key={lang} lang={lang} size={14} />)}
                            </span>
                            <span className="text-[10.5px] text-white/40 shrink-0 tabular-nums">{stream.seeders ? `${stream.seeders} peers` : stream.source}</span>
                          </button>
                        )
                      })}
                    </motion.div>
                  )}
                </AnimatePresence>
              </div>
            )}

            {/* Settings: speed, quality, playback options, stream info */}
            <div className="relative">
              <CtrlButton
                active={showSettings}
                onClick={() => { const open = !showSettings; closePanels(); setShowSettings(open) }}
                title="Settings"
              >
                <Settings size={20} />
                {playbackSpeed !== 1 && (
                  <span className="absolute -top-0.5 -right-0.5 px-1 rounded text-[8px] font-bold text-white" style={{ background: 'var(--accent)' }}>{playbackSpeed}x</span>
                )}
              </CtrlButton>
              <AnimatePresence>
                {showSettings && (
                  <motion.div
                    {...dropdownMotion}
                    className="absolute bottom-full right-0 mb-3 rounded-2xl w-[320px] overflow-hidden"
                    style={PANEL_STYLE}
                    onClick={(e) => e.stopPropagation()}
                  >
                    <div className="px-4 pt-3 pb-3">
                      <div className="text-[10px] text-white/40 uppercase tracking-[0.16em] font-bold mb-2">Speed</div>
                      <div className="grid grid-cols-6 gap-1 p-0.5 rounded-lg bg-white/[0.05]">
                        {SPEED_OPTIONS.map((s) => (
                          <button key={s} onClick={() => selectSpeed(s)}
                            className={`h-7 rounded-md text-[11px] tabular-nums transition-colors ${playbackSpeed === s ? 'bg-white/[0.14] text-white font-semibold' : 'text-white/55 hover:text-white'}`}>
                            {s === 1 ? '1x' : `${s}`}
                          </button>
                        ))}
                      </div>
                    </div>

                    <div className="px-4 py-3 border-t border-white/[0.06]">
                      <div className="text-[10px] text-white/40 uppercase tracking-[0.16em] font-bold mb-2">Quality</div>
                      <div className="grid grid-cols-5 gap-1 p-0.5 rounded-lg bg-white/[0.05]">
                        {(['auto', 'data-saver', '720p', '1080p', 'best'] as const).map((quality) => (
                          <button key={quality} onClick={() => selectQuality(quality)}
                            className={`h-7 rounded-md text-[10.5px] transition-colors ${qualityPreference === quality ? 'bg-white/[0.14] text-white font-semibold' : 'text-white/55 hover:text-white'}`}>
                            {quality === 'data-saver' ? 'Saver' : quality === 'auto' ? 'Auto' : quality === 'best' ? 'Best' : quality}
                          </button>
                        ))}
                      </div>
                    </div>

                    <div className="px-4 py-2 border-t border-white/[0.06]">
                      {([
                        ['Autoplay next episode', autoPlayEnabled, () => { const v = !autoPlayEnabled; setAutoPlayEnabled(v); savePlayerCfg({ autoNext: v }) }],
                        ['Auto-skip intro, recap & credits', autoSkip, () => { const v = !autoSkip; setAutoSkip(v); savePlayerCfg({ autoSkip: v }) }],
                      ] as const).map(([label, on, toggle]) => (
                        <button key={label} onClick={toggle} className="w-full h-9 flex items-center justify-between text-[12px] text-white/80 hover:text-white">
                          <span>{label}</span>
                          <span className="relative w-9 h-5 rounded-full transition-colors" style={{ background: on ? 'var(--accent)' : 'rgba(255,255,255,0.18)' }}>
                            <span className="absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all" style={{ left: on ? 18 : 2 }} />
                          </span>
                        </button>
                      ))}
                    </div>

                    <div className="px-4 py-3 border-t border-white/[0.06]">
                      <div className="text-[10px] text-white/40 uppercase tracking-[0.16em] font-bold mb-2">Stream</div>
                      <div className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-[11px]">
                        <span className="text-white/45">Mode</span>
                        <span className="text-white/85 text-right">{!session ? 'Connecting' : transcodeMode ? (transcodeKind === 'full-transcode' ? 'Converted' : transcodeKind === 'audio-transcode' ? 'Audio converted' : 'Repackaged') : 'Direct'}</span>
                        <span className="text-white/45">Resolution</span>
                        <span className="text-white/85 text-right">{videoTrack?.height ? `${videoTrack.width}×${videoTrack.height}` : activeStream.quality || '—'}</span>
                        <span className="text-white/45">Codecs</span>
                        <span className="text-white/85 text-right uppercase">{videoTrack?.codec || '—'} / {audioTracks.find((track) => track.idx === selectedAudioIndex)?.codec || '—'}</span>
                        <span className="text-white/45">Buffer ahead</span>
                        <span className="text-white/85 text-right tabular-nums">{Math.max(0, buffered - (currentTime - transcodeOffset)).toFixed(0)}s</span>
                        <span className="text-white/45">Source</span>
                        <span className="text-white/85 text-right truncate" title={activeStream.title}>{activeStream.title}</span>
                      </div>
                    </div>

                    <button
                      onClick={() => { setShowSettings(false); setShowShortcuts(true) }}
                      className="w-full h-10 flex items-center justify-center gap-2 border-t border-white/[0.06] text-[11px] text-white/60 hover:text-white hover:bg-white/[0.04] transition-colors"
                    >
                      <Keyboard size={13} /> Keyboard shortcuts <kbd className="px-1 rounded text-[10px] bg-white/10">?</kbd>
                    </button>
                  </motion.div>
                )}
              </AnimatePresence>
            </div>

            <CtrlButton onClick={handlePip} title="Mini player">
              <PictureInPicture2 size={20} />
            </CtrlButton>
            <CtrlButton onClick={toggleFullscreen} title={isFullscreen ? 'Exit fullscreen (F)' : 'Fullscreen (F)'}>
              {isFullscreen ? <Minimize size={20} /> : <Maximize size={20} />}
            </CtrlButton>
          </div>
        </motion.div>
      </div>

      {/* ── Smart skip button (AniSkip ranges, 85s heuristic fallback) ── */}
      <SkipButton
        label={activeSkip ? SKIP_LABEL[kindOf(activeSkip.type)] : heuristicSkip ? 'Skip 85s' : null}
        fallback={!activeSkip}
        visible={!showNextEp && !showEpisodes && (showControls || (activeSkip ? currentTime < activeSkip.start + 8 : true))}
        onSkip={doSkip}
      />

      {/* ── Keyboard shortcuts overlay ── */}
      {showShortcuts && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/60" onClick={() => setShowShortcuts(false)}>
          <div className="rounded-xl p-4 w-[380px] shadow-2xl" style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }} onClick={(e) => e.stopPropagation()}>
            <div className="flex items-center mb-3">
              <h3 className="text-sm font-bold text-white">Keyboard shortcuts</h3>
              <button className="ml-auto text-[color:var(--text-3)] hover:text-white" onClick={() => setShowShortcuts(false)}><X size={14} /></button>
            </div>
            <div className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs">
              {([
                ['Space / K', 'Play / pause'], ['\u2190 / \u2192', 'Seek 5s'], ['\u2191 / \u2193', 'Volume'],
                ['S', 'Skip intro / credits'], ['J / L', 'Back / forward 10s'], ['N', 'Next episode'], ['E', 'Episodes'], ['F', 'Fullscreen'], ['M', 'Mute'],
                ['C', 'Cycle subtitles'], ['G / H', 'Subtitle delay -/+ 0.1s'], ['< / >', 'Playback speed'], ['?', 'This overlay'], ['Esc', 'Close / back'],
              ] as const).map(([k, d]) => (
                <div key={k} className="contents">
                  <kbd className="px-1.5 py-0.5 rounded text-[10px] font-mono text-white/90 justify-self-start" style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}>{k}</kbd>
                  <span className="text-[color:var(--text-2)]">{d}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* ── Next Episode card ── */}
      <AnimatePresence>
        {showNextEp && hasNextEp && nextEp && (
          <NextEpisodeCard
            key="next-ep"
            episodeNumber={nextEp.number ?? currentEpIndex + 2}
            title={nextEp.title}
            thumbnail={nextEp.thumbnail || coverUrl}
            autoplay={autoPlayEnabled}
            onPlay={playNextEpisode}
            onCancel={cancelNextEp}
          />
        )}
      </AnimatePresence>

      {/* ── Episodes drawer ── */}
      <AnimatePresence>
        {showEpisodes && episodeList && (
          <EpisodeDrawer
            key="episodes"
            episodes={episodeList}
            currentIndex={currentEpIndex ?? -1}
            fallbackThumb={coverUrl}
            onSelect={playEpisodeAt}
            onClose={() => setShowEpisodes(false)}
          />
        )}
      </AnimatePresence>
    </div>
  )
}

const PANEL_STYLE: React.CSSProperties = {
  background: 'rgba(14,14,16,0.94)',
  border: '1px solid rgba(255,255,255,0.09)',
  backdropFilter: 'blur(24px)',
  boxShadow: '0 24px 60px rgba(0,0,0,0.55)',
}

/** Round, consistent hit target for every player control. */
function CtrlButton({ children, onClick, title, active, size = 'md' }: {
  children: React.ReactNode
  onClick: () => void
  title: string
  active?: boolean
  size?: 'md' | 'lg'
}) {
  return (
    <button
      type="button"
      onClick={(e) => { e.stopPropagation(); onClick() }}
      title={title}
      aria-label={title}
      className={`relative shrink-0 flex items-center justify-center rounded-full transition-[background-color,color,transform] duration-150 active:scale-90 hover:bg-white/[0.12] ${
        size === 'lg' ? 'w-12 h-12' : 'w-10 h-10'
      } ${active ? 'text-[color:var(--accent-hover)]' : 'text-white/85 hover:text-white'}`}
    >
      {children}
    </button>
  )
}
