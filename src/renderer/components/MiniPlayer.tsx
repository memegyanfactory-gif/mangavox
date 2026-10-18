import { useState, useEffect, useRef, useCallback } from 'react'
import { Play, Pause, Maximize2, X, Volume2, VolumeX } from 'lucide-react'
import { useUiStore } from '../stores/ui.store'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'

function formatTime(seconds: number): string {
  if (!isFinite(seconds) || seconds < 0) return '0:00'
  const m = Math.floor(seconds / 60)
  const s = Math.floor(seconds % 60)
  return `${m}:${s.toString().padStart(2, '0')}`
}

export function MiniPlayer() {
  const pip = useUiStore((s) => s.pip)
  const videoRef = useRef<HTMLVideoElement>(null)

  // Dragging state
  const [position, setPosition] = useState({
    x: window.innerWidth - 340,
    y: window.innerHeight - 240,
  })
  const [dragging, setDragging] = useState(false)
  const [dragOffset, setDragOffset] = useState({ x: 0, y: 0 })

  // Playback state
  const [playing, setPlaying] = useState(true)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [muted, setMuted] = useState(false)

  // --- Dragging ---

  const onMouseDown = useCallback(
    (e: React.MouseEvent) => {
      setDragging(true)
      setDragOffset({
        x: e.clientX - position.x,
        y: e.clientY - position.y,
      })
    },
    [position],
  )

  useEffect(() => {
    if (!dragging) return

    const onMouseMove = (e: MouseEvent) => {
      const width = 320
      const height = 220 // video + title bar approx
      const nx = Math.min(Math.max(0, e.clientX - dragOffset.x), window.innerWidth - width)
      const ny = Math.min(Math.max(0, e.clientY - dragOffset.y), window.innerHeight - height)
      setPosition({ x: nx, y: ny })
    }

    const onMouseUp = () => setDragging(false)

    window.addEventListener('mousemove', onMouseMove)
    window.addEventListener('mouseup', onMouseUp)
    return () => {
      window.removeEventListener('mousemove', onMouseMove)
      window.removeEventListener('mouseup', onMouseUp)
    }
  }, [dragging, dragOffset])

  // --- Video time tracking ---

  useEffect(() => {
    const v = videoRef.current
    if (!v) return

    const onTimeUpdate = () => {
      setCurrentTime(v.currentTime)
      setPlaying(!v.paused)
    }
    const onDurationChange = () => setDuration(v.duration)
    const onPlay = () => setPlaying(true)
    const onPause = () => setPlaying(false)

    v.addEventListener('timeupdate', onTimeUpdate)
    v.addEventListener('durationchange', onDurationChange)
    v.addEventListener('play', onPlay)
    v.addEventListener('pause', onPause)

    return () => {
      v.removeEventListener('timeupdate', onTimeUpdate)
      v.removeEventListener('durationchange', onDurationChange)
      v.removeEventListener('play', onPlay)
      v.removeEventListener('pause', onPause)
    }
  }, [pip?.videoSrc])

  // --- Handlers ---

  const handleLoadedMetadata = useCallback(() => {
    const v = videoRef.current
    if (v && pip) {
      v.currentTime = pip.currentTime
    }
  }, [pip])

  const togglePlay = useCallback(() => {
    const v = videoRef.current
    if (!v) return
    if (v.paused) {
      v.play()
    } else {
      v.pause()
    }
  }, [])

  const toggleMute = useCallback(() => {
    const v = videoRef.current
    if (!v) return
    v.muted = !v.muted
    setMuted(v.muted)
  }, [])

  const handleExpand = useCallback(() => {
    useUiStore.getState().exitPip()
  }, [])

  const handleClose = useCallback(() => {
    useUiStore.getState().dismissPip()
    invoke(IPC.TORRENT_STOP)
  }, [])

  if (!pip) return null

  const titleLabel = pip.episodeNumber
    ? `${pip.animeTitle} — Ep ${pip.episodeNumber}`
    : pip.animeTitle

  return (
    <div
      className="fixed z-50 rounded-2xl shadow-2xl overflow-hidden select-none"
      style={{
        left: position.x,
        top: position.y,
        width: 320,
        background: 'var(--surface-1)',
        border: '1px solid var(--panel-border)',
      }}
    >
      {/* Title bar */}
      <div
        className="flex items-center gap-2 px-3 py-1.5 cursor-grab active:cursor-grabbing"
        onMouseDown={onMouseDown}
        style={{ background: 'rgba(200, 80, 26, 0.15)' }}
      >
        <span
          className="flex-1 truncate text-white/80 font-medium"
          style={{ fontSize: 11 }}
        >
          {titleLabel}
        </span>

        <button
          onClick={handleExpand}
          className="p-1 rounded text-white/60 hover:text-white hover:bg-white/10 transition-all hover:scale-110"
          title="Expand"
        >
          <Maximize2 size={13} />
        </button>

        <button
          onClick={handleClose}
          className="p-1 rounded text-white/60 hover:text-[color:var(--accent-hover)] hover:bg-white/10 transition-all hover:scale-110"
          title="Close"
        >
          <X size={13} />
        </button>
      </div>

      {/* Video area */}
      <div className="bg-black" style={{ width: 320, height: 180 }}>
        <video
          ref={videoRef}
          src={pip.videoSrc}
          autoPlay
          onLoadedMetadata={handleLoadedMetadata}
          className="w-full h-full object-contain"
        />
      </div>

      {/* Bottom controls */}
      <div
        className="flex items-center gap-2 px-3 py-1.5"
        style={{ background: 'var(--surface-1)' }}
      >
        <button
          onClick={togglePlay}
          className="p-1 rounded text-white/70 hover:text-[color:var(--accent-hover)] transition-all hover:scale-110"
          title={playing ? 'Pause' : 'Play'}
        >
          {playing ? <Pause size={14} /> : <Play size={14} />}
        </button>

        <button
          onClick={toggleMute}
          className="p-1 rounded text-white/70 hover:text-[color:var(--accent-hover)] transition-all hover:scale-110"
          title={muted ? 'Unmute' : 'Mute'}
        >
          {muted ? <VolumeX size={14} /> : <Volume2 size={14} />}
        </button>

        <span className="flex-1" />

        <span className="text-white/50 font-mono" style={{ fontSize: 10 }}>
          {formatTime(currentTime)} / {formatTime(duration)}
        </span>
      </div>
    </div>
  )
}
