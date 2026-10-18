import { useEffect, useState, useMemo, useCallback, useRef } from 'react'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { useUiStore } from '../stores/ui.store'
import { coverUrl } from '../utils/cover-url'
import { EmptyState } from '../components/EmptyState'
import { toast } from '../components/Toaster'
import {
  Film, Play, Square, Loader2, Download as DownloadIcon, Volume2,
  Wand2, CheckCircle2,
} from 'lucide-react'
import type { Manga, Chapter } from '../../shared/types'

interface ChapterStageStatus {
  chapterId: string
  number: number | null
  title: string | null
  downloaded: boolean
  cropped: boolean
  extracted: boolean
  scripted: boolean
  combined: boolean
  audio: boolean
  video: boolean
}

interface AudioData {
  syncMap?: any
  audioFiles?: { page: number; url: string }[]
  hasVideo?: boolean
  videoPath?: string
}

const RESOLUTIONS = [
  { id: '720p',  label: '720p · faster render' },
  { id: '1080p', label: '1080p · default' },
  { id: '1440p', label: '1440p · slower' },
  { id: '4k',    label: '4K · very slow' },
]
const TRANSITIONS = [
  { id: 'fade',       label: 'Fade'       },
  { id: 'crossfade',  label: 'Crossfade'  },
  { id: 'cut',        label: 'Cut'        },
  { id: 'zoom-punch', label: 'Zoom Punch' },
]
const EFFECTS = [
  { id: 'none',      label: 'None — static' },
  { id: 'ken_burns', label: 'Ken Burns — slow zoom' },
  { id: 'pan',       label: 'Pan'           },
]

export function VideoPage() {
  const { pageParams } = useUiStore()
  const [mangaList, setMangaList] = useState<Manga[]>([])
  const [selectedMangaId, setSelectedMangaId] = useState<string | null>(
    (pageParams.mangaId as string) ?? null,
  )
  const selectedManga = useMemo(
    () => mangaList.find((m) => m.id === selectedMangaId) ?? null,
    [mangaList, selectedMangaId],
  )
  const [chapters, setChapters] = useState<Chapter[]>([])
  const [stageStatus, setStageStatus] = useState<ChapterStageStatus[]>([])
  const [selectedChapterId, setSelectedChapterId] = useState<string | null>(
    (pageParams.chapterId as string) ?? null,
  )

  // Video render config
  const [resolution, setResolution] = useState<string>(() => {
    try { return localStorage.getItem('mangavox.videoRes') || '1080p' } catch { return '1080p' }
  })
  const [transition, setTransition] = useState<string>(() => {
    try { return localStorage.getItem('mangavox.videoTrans') || 'fade' } catch { return 'fade' }
  })
  const [effect, setEffect] = useState<string>(() => {
    try { return localStorage.getItem('mangavox.videoEffect') || 'ken_burns' } catch { return 'ken_burns' }
  })
  const [captions, setCaptions] = useState<boolean>(() => {
    try { return localStorage.getItem('mangavox.videoCaptions') !== '0' } catch { return true }
  })
  useEffect(() => {
    try {
      localStorage.setItem('mangavox.videoRes', resolution)
      localStorage.setItem('mangavox.videoTrans', transition)
      localStorage.setItem('mangavox.videoEffect', effect)
      localStorage.setItem('mangavox.videoCaptions', captions ? '1' : '0')
    } catch { /* ok */ }
  }, [resolution, transition, effect, captions])

  const [audioData, setAudioData] = useState<AudioData | null>(null)
  const [running, setRunning] = useState(false)
  const [progress, setProgress] = useState<{ current: number; total: number; percent: number } | null>(null)
  const [stageMsg, setStageMsg] = useState<string>('')
  const [discarding, setDiscarding] = useState(false)

  const handleDiscardVideo = async () => {
    if (!selectedMangaId || !selectedChapterId || discarding) return
    if (!confirm('Discard the generated video and temporary parts? The audio files will remain intact.')) return
    setDiscarding(true)
    try {
      const res = await invoke<{ ok: boolean; deleted?: number }>(IPC.VIDEO_DISCARD, selectedMangaId, selectedChapterId)
      if (res?.ok) {
        toast.success('Video discarded')
        refreshArtifacts()
        if (selectedMangaId) {
          invoke<ChapterStageStatus[]>(IPC.CHAPTER_STAGE_STATUS, selectedMangaId)
            .then(setStageStatus).catch(() => {})
        }
      } else {
        toast.error('Failed to discard video')
      }
    } catch (err: any) {
      toast.error('Discard failed', err?.message)
    } finally {
      setDiscarding(false)
    }
  }

  const videoRef = useRef<HTMLVideoElement | null>(null)

  // Load library
  useEffect(() => {
    invoke<Manga[]>(IPC.LIBRARY_LIST).then(setMangaList).catch(console.error)
  }, [])

  // Load chapters + status when manga changes
  useEffect(() => {
    if (!selectedMangaId) { setChapters([]); setStageStatus([]); return }
    invoke<Chapter[]>(IPC.CHAPTER_LIST, selectedMangaId)
      .then((chs) => setChapters(chs.filter((c) => c.download_status === 'complete')))
      .catch(() => setChapters([]))
    invoke<ChapterStageStatus[]>(IPC.CHAPTER_STAGE_STATUS, selectedMangaId)
      .then(setStageStatus)
      .catch(() => setStageStatus([]))
  }, [selectedMangaId])

  // Pull audio + video data on chapter change
  const refreshArtifacts = useCallback(async () => {
    if (!selectedMangaId || !selectedChapterId) { setAudioData(null); return }
    try {
      const data = await invoke<AudioData | null>(IPC.PLAYER_GET_AUDIO_DATA, selectedMangaId, selectedChapterId)
      setAudioData(data)
    } catch {
      setAudioData(null)
    }
  }, [selectedMangaId, selectedChapterId])
  useEffect(() => { refreshArtifacts() }, [refreshArtifacts])

  // Listen for pipeline progress
  useEffect(() => {
    const unsub = onEvent(IPC.PIPELINE_PROGRESS, (data: any) => {
      if (data?.chapterId && data.chapterId !== selectedChapterId) return
      if (data.type === 'progress') {
        setProgress({
          current: data.current ?? 0,
          total: data.total ?? 0,
          percent: data.percent ?? 0,
        })
        if (data.stage === 'sync_map') {
          setStageMsg('Syncing page images to audio timing…')
        } else if (data.stage === 'video' && (data.total ?? 0) > 0) {
          const pages = Math.ceil((data.total ?? 0) / 3)
          const cur = Math.min(pages, Math.ceil((data.current ?? 0) / 3) || 1)
          setStageMsg(`Rendering video · page ${cur}/${pages} (${resolution})`)
        } else if (data.stage) {
          setStageMsg(data.stage)
        }
      } else if (data.type === 'info' && data.message) {
        setStageMsg(data.message)
      } else if (data.type === 'stage_complete') {
        if (data.stage === 'sync_map') setStageMsg('Sync map ready — starting ffmpeg render…')
        if (data.stage === 'video') refreshArtifacts()
      } else if (data.type === 'done') {
        setRunning(false)
        setProgress(null)
        refreshArtifacts()
        if (selectedMangaId) {
          invoke<ChapterStageStatus[]>(IPC.CHAPTER_STAGE_STATUS, selectedMangaId)
            .then(setStageStatus).catch(() => {})
        }
        toast.success('Video render complete', 'Preview below or save to your Mac')
      } else if (data.type === 'error') {
        setRunning(false)
        setProgress(null)
        toast.error('Video render failed', data.message?.slice(0, 120))
      }
    })
    return unsub
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedChapterId, selectedMangaId, resolution])

  const status = stageStatus.find((s) => s.chapterId === selectedChapterId)
  const hasAudio = (audioData?.audioFiles?.length || 0) > 0
  const hasVideo = !!audioData?.hasVideo
  const videoUrl = audioData?.videoPath || ''

  const handleRender = async () => {
    if (!selectedMangaId || !selectedChapterId || !hasAudio) return
    setRunning(true)
    setProgress({ current: 0, total: 0, percent: 0 })
    setStageMsg('Loading audio + page images…')
    try {
      await invoke(
        IPC.PIPELINE_START,
        selectedMangaId, selectedChapterId,
        ['sync_map', 'video'],
        undefined,
        {
          resolution,
          transition,
          transition_duration: 0.5,
          effect,
          captions,
          force_rerender: true,
        },
      )
      toast.info(`Rendering ${resolution} video…`, 'Syncing pages → composing with ffmpeg')
    } catch (err: any) {
      setRunning(false)
      setProgress(null)
      toast.error('Failed to start render', err?.message)
    }
  }

  const handleStop = async () => {
    if (!selectedChapterId) return
    try {
      await invoke(IPC.PIPELINE_CANCEL, selectedChapterId)
      setRunning(false)
      setProgress(null)
      toast.info('Render cancelled')
    } catch { /* ok */ }
  }

  const handleSaveVideo = async () => {
    if (!selectedMangaId || !selectedChapterId || !hasVideo) return
    const ch = chapters.find((c) => c.id === selectedChapterId)
    const name = `${(selectedManga?.title || 'chapter').replace(/[^\w\- ]+/g, '').slice(0, 60)}${ch?.number != null ? ' - Ch ' + ch.number : ''}`
    const res: { ok: boolean; path?: string; canceled?: boolean; error?: string } = await invoke<{ ok: boolean; path?: string; canceled?: boolean; error?: string }>(
      IPC.VIDEO_EXPORT, selectedMangaId, selectedChapterId, name,
    ).catch((e) => ({ ok: false, error: e.message }))
    if (res?.canceled) return
    if (res?.ok) toast.success('Video saved', res.path)
    else toast.error('Save failed', res?.error?.slice(0, 140))
  }

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-5 py-3 mv-page-wash">
        <h2 className="mv-title !text-xl">Video Studio</h2>
        <p className="text-sm text-[var(--text-3)] mt-1 max-w-xl">
          Preview the narrated chapter, tweak transitions, and export MP4. Audio pipeline can render video automatically when enabled in Settings.
        </p>
      </div>

      <div className="flex-1 overflow-y-auto px-5 py-3 pb-6 grid grid-cols-1 xl:grid-cols-[1fr_2fr] gap-6">
        {/* LEFT: pickers + render config */}
        <div className="space-y-4">
          <div className="mv-card p-4">
            <label className="mv-label">Manga</label>
            <select
              value={selectedMangaId || ''}
              onChange={(e) => { setSelectedMangaId(e.target.value || null); setSelectedChapterId(null) }}
              className="mv-input w-full mt-2"
            >
              <option value="">— Pick a manga —</option>
              {mangaList.map((m) => <option key={m.id} value={m.id}>{m.title}</option>)}
            </select>
          </div>

          {selectedManga && (
            <div className="mv-card p-4 flex items-center gap-3">
              {(() => {
                const c = coverUrl(selectedManga.cover_url || selectedManga.cover_path || null)
                return c ? (
                  <img src={c} className="w-12 h-16 rounded-lg object-cover shadow" alt="" />
                ) : (
                  <div className="w-12 h-16 rounded-lg flex items-center justify-center text-base font-bold"
                    style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}>
                    {selectedManga.title.charAt(0)}
                  </div>
                )
              })()}
              <div className="min-w-0 flex-1">
                <div className="text-sm font-semibold text-[var(--text-1)] truncate">{selectedManga.title}</div>
                <div className="text-[11px] text-[var(--text-3)]">{chapters.length} chapter{chapters.length !== 1 ? 's' : ''}</div>
              </div>
            </div>
          )}

          {selectedManga && (
            <div className="mv-card p-4">
              <label className="mv-label">Chapter</label>
              <select
                value={selectedChapterId || ''}
                onChange={(e) => setSelectedChapterId(e.target.value || null)}
                className="mv-input w-full mt-2"
              >
                <option value="">— Pick a chapter —</option>
                {chapters.map((ch) => {
                  const s = stageStatus.find((x) => x.chapterId === ch.id)
                  const tag = s?.video ? '✓ video' : s?.audio ? '✓ audio' : 'no audio'
                  const label = ch.number != null ? `Ch. ${ch.number}` : (ch.title || ch.id.slice(0, 8))
                  return <option key={ch.id} value={ch.id}>{label} — {tag}</option>
                })}
              </select>
            </div>
          )}

          {/* Render config */}
          <div className="mv-card p-4 space-y-3">
            <label className="mv-label">Render</label>
            <div className="space-y-2">
              <select value={resolution} onChange={(e) => setResolution(e.target.value)} className="mv-input w-full">
                {RESOLUTIONS.map((r) => <option key={r.id} value={r.id}>{r.label}</option>)}
              </select>
              <select value={transition} onChange={(e) => setTransition(e.target.value)} className="mv-input w-full">
                {TRANSITIONS.map((t) => <option key={t.id} value={t.id}>Transition: {t.label}</option>)}
              </select>
              <select value={effect} onChange={(e) => setEffect(e.target.value)} className="mv-input w-full">
                {EFFECTS.map((eff) => <option key={eff.id} value={eff.id}>Effect: {eff.label}</option>)}
              </select>
              <button
                type="button"
                onClick={() => setCaptions((v) => !v)}
                className="w-full flex items-center justify-between mv-input"
                title="Burn synced narration captions into the video"
              >
                <span>Synced captions</span>
                <span className={`text-[11px] font-semibold ${captions ? 'text-[var(--success)]' : 'text-[var(--text-3)]'}`}>
                  {captions ? 'On' : 'Off'}
                </span>
              </button>
            </div>
            <p className="text-[10px] text-[var(--text-3)] leading-relaxed">
              Panel-level beats sync each panel to its narration; captions (when on) are burned in word-by-word in time with the audio.
            </p>
          </div>

          {/* Run / stop */}
          <div className="mv-card p-4">
            {!running ? (
              <button
                onClick={handleRender}
                disabled={!selectedChapterId || !hasAudio}
                className="w-full mv-btn-primary justify-center !py-3"
              >
                <Wand2 size={14} />
                {hasVideo ? 'Re-render Video' : 'Render Video'}
              </button>
            ) : (
              <button
                onClick={handleStop}
                className="w-full mv-btn-ghost justify-center !py-3 !text-emerald-300"
              >
                <Square size={14} /> Stop render
              </button>
            )}
            {running && progress && (
              <div className="mt-3 space-y-1.5">
                <div className="flex items-center justify-between text-[11px] text-[var(--text-2)]">
                  <span>{stageMsg || 'Rendering…'}</span>
                  <span className="font-mono">
                    {progress.total > 0 ? `${progress.current}/${progress.total}` : `${progress.percent}%`}
                  </span>
                </div>
                <div className="h-1.5 rounded-full overflow-hidden" style={{ background: 'var(--surface-3)' }}>
                  <div className="h-full rounded-full transition-all duration-300"
                    style={{ width: `${progress.percent}%`, background: 'var(--accent-gradient)' }} />
                </div>
              </div>
            )}
            {!hasAudio && selectedChapterId && (
              <p className="text-[11px] text-amber-300 mt-3 leading-snug">
                No audio yet. Generate audio in the Audio tab first.
              </p>
            )}
          </div>

          {hasVideo && (
            <div className="flex gap-2">
              <button
                onClick={handleSaveVideo}
                className="flex-1 justify-center !py-3 flex items-center gap-2 rounded-xl text-sm font-semibold text-white"
                style={{ background: 'var(--accent-gradient)', boxShadow: '0 6px 20px var(--accent-glow)' }}
              >
                <DownloadIcon size={14} /> Save Video As…
              </button>
              <button
                onClick={handleDiscardVideo}
                disabled={discarding}
                className="px-4 py-3 justify-center flex items-center gap-2 rounded-xl text-sm font-semibold text-white bg-red-600/20 border border-red-500/30 hover:bg-red-600/30 disabled:opacity-40 transition-colors"
                title="Discard generated video and temporary parts"
              >
                {discarding ? <Loader2 size={14} className="animate-spin" /> : 'Discard'}
              </button>
            </div>
          )}
        </div>

        {/* RIGHT: video preview */}
        <div className="space-y-4">
          {!selectedManga && (
            <div className="mv-card-2">
              <EmptyState
                icon={<Film size={28} strokeWidth={1.5} />}
                title="Pick a manga + chapter"
                description="The Video tab needs an audio-generated chapter. Convert audio in the Audio tab, then render here."
              />
            </div>
          )}

          {selectedManga && !hasAudio && selectedChapterId && (
            <div className="mv-card-2">
              <EmptyState
                tone="warn"
                icon={<Volume2 size={28} strokeWidth={1.5} />}
                title="Generate audio first"
                description="Open the Audio tab, run Generate Audio, then come back here to render the video."
              />
            </div>
          )}

          {/* Status row */}
          {selectedChapterId && hasAudio && (
            <div className="mv-card p-4">
              <div className="grid grid-cols-3 gap-3 text-center">
                <Stage label="Audio" ok={hasAudio} tint="#c8501a" />
                <Stage label="Sync Map" ok={!!audioData?.syncMap} tint="#60A5FA" />
                <Stage label="Video" ok={hasVideo} tint="#fb7185" />
              </div>
            </div>
          )}

          {/* Video player */}
          {hasVideo && videoUrl && (
            <div className="mv-card overflow-hidden">
              <div className="aspect-video bg-[var(--base)]">
                <video
                  ref={videoRef}
                  src={videoUrl}
                  controls
                  className="w-full h-full"
                />
              </div>
              <div className="px-5 py-3 flex items-center justify-between border-t" style={{ borderColor: 'var(--panel-border)' }}>
                <div className="text-[11px] text-[var(--text-3)]">
                  Saved to <span className="font-mono">pipeline/video_final.mp4</span>
                </div>
                <div className="flex items-center gap-2">
                  <button onClick={() => videoRef.current?.play()} className="mv-btn-ghost"><Play size={11} /> Play</button>
                  <button onClick={handleSaveVideo} className="mv-btn-primary !py-1.5">
                    <DownloadIcon size={11} /> Save as…
                  </button>
                </div>
              </div>
            </div>
          )}

          {selectedChapterId && hasAudio && !hasVideo && !running && (
            <div className="mv-card-2">
              <EmptyState
                tone="accent"
                icon={<Film size={28} strokeWidth={1.5} />}
                title="Ready to render"
                description="Audio is ready. Click Render Video to compose pages + audio into an MP4 with the chosen transitions and effect."
              />
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

function Stage({ label, ok, tint }: { label: string; ok: boolean; tint: string }) {
  return (
    <div className="flex flex-col items-center gap-1.5 py-2 rounded-lg"
      style={{
        background: ok ? `${tint}15` : 'var(--panel-border)',
        border: `1px solid ${ok ? `${tint}30` : 'var(--panel-border)'}`,
      }}>
      {ok
        ? <CheckCircle2 size={16} style={{ color: 'var(--success)' }} />
        : <Loader2 size={16} className="text-[var(--text-3)]" />}
      <span className="text-[10px] font-semibold uppercase tracking-wider"
        style={{ color: ok ? tint : 'var(--text-3)' }}>{label}</span>
    </div>
  )
}
