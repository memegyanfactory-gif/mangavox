import { useEffect, useState, useRef } from 'react'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import {
  Video, Download, X, Loader2, CheckCircle2, AlertCircle,
  Music, Film, Subtitles, Link, RefreshCw,
} from 'lucide-react'

interface Format {
  format_id: string
  ext: string
  resolution: string
  fps?: number
  vcodec?: string
  acodec?: string
  filesize?: number
  format_note?: string
}

interface VideoInfo {
  title: string
  thumbnail?: string
  duration?: number
  uploader?: string
  formats: Format[]
}

interface ProgressData {
  downloadId: string
  percent?: number
  size?: string
  speed?: string
  eta?: string
  status: 'downloading' | 'merging' | 'complete' | 'error'
  outputDir?: string
  title?: string
  error?: string
}

function formatDuration(secs?: number): string {
  if (!secs) return ''
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  const s = Math.floor(secs % 60)
  if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
  return `${m}:${String(s).padStart(2, '0')}`
}

function formatBytes(bytes?: number): string {
  if (!bytes) return ''
  if (bytes > 1e9) return `${(bytes / 1e9).toFixed(1)} GB`
  if (bytes > 1e6) return `${(bytes / 1e6).toFixed(1)} MB`
  return `${(bytes / 1e3).toFixed(0)} KB`
}

export function YouTube() {
  const [ytdlpAvailable, setYtdlpAvailable] = useState<boolean | null>(null)
  const [url, setUrl] = useState('')
  const [fetching, setFetching] = useState(false)
  const [videoInfo, setVideoInfo] = useState<VideoInfo | null>(null)
  const [fetchError, setFetchError] = useState<string | null>(null)

  const [selectedFormatId, setSelectedFormatId] = useState<string>('')
  const [audioOnly, setAudioOnly] = useState(false)
  const [subtitles, setSubtitles] = useState(false)
  const [subLang, setSubLang] = useState('en')

  const [outputFormat, setOutputFormat] = useState<'mp4' | 'mkv' | 'avi'>('mp4')
  const [outputDir, setOutputDir] = useState<string | null>(null)
  const [downloading, setDownloading] = useState(false)
  const [progress, setProgress] = useState<ProgressData | null>(null)
  const downloadIdRef = useRef(`dl-${Date.now()}`)

  // Check yt-dlp availability on mount
  useEffect(() => {
    invoke<{ available: boolean }>(IPC.YTDL_CHECK)
      .then((r) => setYtdlpAvailable(r.available))
      .catch(() => setYtdlpAvailable(false))
  }, [])

  // Listen for progress events
  useEffect(() => {
    const unsub = onEvent(IPC.YTDL_PROGRESS, (...args: unknown[]) => {
      const data = args[0] as ProgressData
      if (data.downloadId !== downloadIdRef.current) return
      setProgress(data)
      if (data.status === 'complete' || data.status === 'error') {
        setDownloading(false)
      }
    })
    return unsub
  }, [])

  const handleFetchInfo = async () => {
    if (!url.trim()) return
    setFetching(true)
    setFetchError(null)
    setVideoInfo(null)
    setProgress(null)
    try {
      const info = await invoke<VideoInfo>(IPC.YTDL_LIST_FORMATS, url.trim())
      setVideoInfo(info)
      // Pre-select best video format
      const videoFormats = info.formats.filter((f) => f.vcodec && f.vcodec !== 'none' && f.resolution !== '0x0')
      if (videoFormats.length > 0) {
        const best = videoFormats.reduce((a, b) =>
          (parseInt(b.resolution) || 0) > (parseInt(a.resolution) || 0) ? b : a
        )
        setSelectedFormatId(best.format_id)
      }
    } catch (err: any) {
      setFetchError(err?.message || 'Failed to fetch video info')
    } finally {
      setFetching(false)
    }
  }

  const handlePickFolder = async () => {
    const dir = await invoke<string | null>(IPC.YTDL_PICK_FOLDER)
    if (dir) setOutputDir(dir)
  }

  const handleDownload = async () => {
    if (!url.trim()) return
    downloadIdRef.current = `dl-${Date.now()}`
    setDownloading(true)
    setProgress({ downloadId: downloadIdRef.current, status: 'downloading', percent: 0 })
    try {
      await invoke(IPC.YTDL_START, {
        url: url.trim(),
        formatId: audioOnly ? undefined : selectedFormatId || undefined,
        audioOnly,
        subtitles,
        subLang: subtitles ? subLang : undefined,
        downloadId: downloadIdRef.current,
        outputDir: outputDir || undefined,
        outputFormat: audioOnly ? undefined : outputFormat,
      })
    } catch (err: any) {
      setDownloading(false)
      setProgress({ downloadId: downloadIdRef.current, status: 'error', error: err?.message })
    }
  }

  const handleCancel = async () => {
    await invoke(IPC.YTDL_CANCEL, downloadIdRef.current)
    setDownloading(false)
    setProgress(null)
  }

  const videoFormats = (videoInfo?.formats || []).filter(
    (f) => f.vcodec && f.vcodec !== 'none' && f.resolution && f.resolution !== '0x0'
  ).sort((a, b) => {
    const ha = parseInt(a.resolution?.split('x')[1] || '0')
    const hb = parseInt(b.resolution?.split('x')[1] || '0')
    return hb - ha
  })

  return (
    <div className="flex flex-col h-full overflow-hidden" style={{ paddingTop: '40px' }}>
      {/* Header */}
      <div className="px-3 sm:px-8 pt-4 sm:pt-6 pb-4 shrink-0">
        <div className="flex items-center gap-3 mb-1 min-w-0">
          <div className="w-9 h-9 shrink-0 rounded-xl flex items-center justify-center" style={{ background: 'var(--accent-gradient)' }}>
            <Video size={18} className="text-white" />
          </div>
          <div className="min-w-0">
            <h1 className="text-lg font-bold text-[color:var(--text-1)]">YouTube Downloader</h1>
            <p className="text-xs text-[color:var(--text-3)]">Download videos, audio, and subtitles from YouTube and 1000+ sites</p>
          </div>
        </div>
      </div>

      {/* yt-dlp not found banner */}
      {ytdlpAvailable === false && (
        <div className="mx-3 sm:mx-8 mb-4 rounded-xl p-4 flex items-start gap-3 min-w-0" style={{ background: 'rgba(245,158,11,0.08)', border: '1px solid rgba(245,158,11,0.2)' }}>
          <AlertCircle size={16} className="text-amber-400 shrink-0 mt-0.5" />
          <div>
            <p className="text-sm font-semibold text-amber-300">yt-dlp not installed</p>
            <p className="text-xs text-amber-400/80 mt-1">Install it to enable downloading:</p>
            <div className="mt-2 space-y-1">
              <code className="block text-xs bg-[var(--surface-3)] rounded px-2 py-1 text-[color:var(--text-1)] break-all">pip3 install yt-dlp</code>
              <code className="block text-xs bg-[var(--surface-3)] rounded px-2 py-1 text-[color:var(--text-1)]">brew install yt-dlp</code>
            </div>
            <button
              onClick={() => invoke<{ available: boolean }>(IPC.YTDL_CHECK).then((r) => setYtdlpAvailable(r.available))}
              className="mt-3 flex items-center gap-1.5 min-h-[40px] sm:min-h-0 text-xs text-amber-300 hover:text-amber-100"
            >
              <RefreshCw size={11} /> Check again
            </button>
          </div>
        </div>
      )}

      <div className="flex-1 overflow-y-auto overflow-x-hidden px-3 sm:px-8 pb-8 space-y-5">
        {/* URL input */}
        <div className="rounded-xl p-4 sm:p-5" style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}>
          <label className="text-xs font-semibold text-[color:var(--text-2)] uppercase tracking-wider block mb-3">Video URL</label>
          <div className="flex flex-col min-[400px]:flex-row gap-2">
            <div className="flex-1 min-w-0 relative">
              <Link size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-[color:var(--text-3)]" />
              <input
                type="url"
                value={url}
                onChange={(e) => { setUrl(e.target.value); setVideoInfo(null); setFetchError(null) }}
                onKeyDown={(e) => e.key === 'Enter' && handleFetchInfo()}
                placeholder="https://youtube.com/watch?v=..."
                className="w-full h-10 pl-9 pr-4 rounded-xl text-sm text-[color:var(--text-1)] placeholder-zinc-600 outline-none"
                style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
              />
            </div>
            <button
              onClick={handleFetchInfo}
              disabled={!url.trim() || fetching || ytdlpAvailable === false}
              className="flex items-center justify-center gap-2 px-4 h-10 shrink-0 rounded-xl text-sm font-medium text-white transition-all disabled:opacity-40"
              style={{ background: 'var(--accent-gradient)' }}
            >
              {fetching ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
              {fetching ? 'Fetching…' : 'Fetch Info'}
            </button>
          </div>
          {fetchError && (
            <p className="mt-2 text-xs text-[color:var(--accent-hover)] flex items-center gap-1.5 break-words min-w-0">
              <AlertCircle size={12} className="shrink-0" /> {fetchError}
            </p>
          )}
        </div>

        {/* Video info */}
        {videoInfo && (
          <div className="rounded-xl p-4 sm:p-5 flex flex-col min-[400px]:flex-row gap-3 sm:gap-4" style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}>
            {videoInfo.thumbnail && (
              <img src={videoInfo.thumbnail} alt="" className="w-full min-[400px]:w-32 h-auto aspect-video min-[400px]:h-20 object-cover rounded-xl shrink-0" />
            )}
            <div className="min-w-0">
              <p className="font-semibold text-[color:var(--text-1)] truncate">{videoInfo.title}</p>
              <p className="text-xs text-[color:var(--text-3)] mt-1">
                {videoInfo.uploader && <span>{videoInfo.uploader} · </span>}
                {formatDuration(videoInfo.duration)}
              </p>
              <p className="text-xs text-[color:var(--text-3)] mt-1">{videoInfo.formats.length} formats available</p>
            </div>
          </div>
        )}

        {/* Download options */}
        {(videoInfo || true) && (
          <div className="rounded-xl p-4 sm:p-5 space-y-4" style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}>
            <p className="text-xs font-semibold text-[color:var(--text-2)] uppercase tracking-wider">Download Options</p>

            {/* Mode toggle */}
            <div className="flex flex-wrap gap-2">
              <button
                onClick={() => setAudioOnly(false)}
                className={`flex items-center gap-2 px-4 py-2 min-h-[40px] rounded-xl text-sm font-medium transition-all ${!audioOnly ? 'text-white' : 'text-[color:var(--text-3)] hover:text-[color:var(--text-1)]'}`}
                style={!audioOnly ? { background: 'var(--accent-gradient)' } : { background: 'var(--surface-2)' }}
              >
                <Film size={14} /> Video
              </button>
              <button
                onClick={() => setAudioOnly(true)}
                className={`flex items-center gap-2 px-4 py-2 min-h-[40px] rounded-xl text-sm font-medium transition-all ${audioOnly ? 'text-white' : 'text-[color:var(--text-3)] hover:text-[color:var(--text-1)]'}`}
                style={audioOnly ? { background: 'var(--accent-gradient)' } : { background: 'var(--surface-2)' }}
              >
                <Music size={14} /> Audio Only (MP3)
              </button>
            </div>

            {/* Format selector */}
            {!audioOnly && videoFormats.length > 0 && (
              <div>
                <label className="text-xs text-[color:var(--text-3)] block mb-2">Quality</label>
                <div className="flex flex-wrap gap-2">
                  {videoFormats.slice(0, 8).map((f) => (
                    <button
                      key={f.format_id}
                      onClick={() => setSelectedFormatId(f.format_id)}
                      className={`px-3 py-1.5 min-h-[40px] sm:min-h-0 rounded-lg text-xs font-medium transition-all ${
                        selectedFormatId === f.format_id ? 'text-white' : 'text-[color:var(--text-2)] hover:text-[color:var(--text-1)]'
                      }`}
                      style={selectedFormatId === f.format_id ? { background: 'var(--accent-gradient)' } : { background: 'var(--surface-2)' }}
                    >
                      {f.resolution || f.format_note || f.format_id}
                      {f.filesize ? ` · ${formatBytes(f.filesize)}` : ''}
                    </button>
                  ))}
                  <button
                    onClick={() => setSelectedFormatId('')}
                    className={`px-3 py-1.5 min-h-[40px] sm:min-h-0 rounded-lg text-xs font-medium transition-all ${
                      selectedFormatId === '' ? 'text-white' : 'text-[color:var(--text-2)] hover:text-[color:var(--text-1)]'
                    }`}
                    style={selectedFormatId === '' ? { background: 'var(--accent-gradient)' } : { background: 'var(--surface-2)' }}
                  >
                    Best (auto)
                  </button>
                </div>
              </div>
            )}

            {/* Output format */}
            {!audioOnly && (
              <div>
                <label className="text-xs text-[color:var(--text-3)] block mb-2">Output Format</label>
                <div className="flex gap-2">
                  {(['mp4', 'mkv', 'avi'] as const).map((fmt) => (
                    <button
                      key={fmt}
                      onClick={() => setOutputFormat(fmt)}
                      className={`px-3 py-1.5 min-h-[40px] sm:min-h-0 rounded-lg text-xs font-medium uppercase transition-all ${
                        outputFormat === fmt ? 'text-white' : 'text-[color:var(--text-2)] hover:text-[color:var(--text-1)]'
                      }`}
                      style={outputFormat === fmt ? { background: 'var(--accent-gradient)' } : { background: 'var(--surface-2)' }}
                    >
                      .{fmt}
                    </button>
                  ))}
                </div>
              </div>
            )}

            {/* Subtitles */}
            <div className="flex flex-wrap items-center gap-3">
              <button
                onClick={() => setSubtitles((v) => !v)}
                className={`flex items-center gap-2 px-3 py-1.5 min-h-[40px] sm:min-h-0 rounded-lg text-xs font-medium transition-all ${subtitles ? 'text-white' : 'text-[color:var(--text-3)]'}`}
                style={subtitles ? { background: 'rgba(var(--accent-rgb),0.25)', border: '1px solid rgba(var(--accent-rgb),0.5)' } : { background: 'var(--surface-2)' }}
              >
                <Subtitles size={13} /> Download Subtitles
              </button>
              {subtitles && (
                <input
                  value={subLang}
                  onChange={(e) => setSubLang(e.target.value)}
                  placeholder="en"
                  className="w-20 px-2 py-1.5 rounded-lg text-xs text-[color:var(--text-1)] outline-none"
                  style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                />
              )}
            </div>

            {/* Output folder */}
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2 min-w-0">
              <button
                onClick={handlePickFolder}
                className="flex items-center gap-2 px-3 py-1.5 min-h-[40px] sm:min-h-0 shrink-0 rounded-lg text-xs font-medium text-[color:var(--text-2)] hover:text-[color:var(--text-1)] transition-all"
                style={{ background: 'var(--surface-2)' }}
              >
                <Download size={13} /> Save to folder
              </button>
              <span className="text-xs text-[color:var(--text-3)] truncate min-w-0 max-w-full sm:max-w-xs" title={outputDir || ''}>
                {outputDir ? outputDir : <span className="text-[color:var(--text-3)]">~/.mangavox/youtube/ (default)</span>}
              </span>
              {outputDir && (
                <button onClick={() => setOutputDir(null)} aria-label="Reset folder" className="min-w-[40px] min-h-[40px] sm:min-w-0 sm:min-h-0 flex items-center justify-center text-[color:var(--text-3)] hover:text-[color:var(--text-2)]">
                  <X size={12} />
                </button>
              )}
            </div>

            {/* Download button */}
            <div className="flex flex-wrap items-center gap-3 pt-1">
              <button
                onClick={handleDownload}
                disabled={downloading || !url.trim() || ytdlpAvailable === false}
                className="flex items-center gap-2 px-5 py-2.5 rounded-xl text-sm font-semibold text-white transition-all hover:brightness-110 disabled:opacity-40"
                style={{ background: 'linear-gradient(135deg, var(--accent), var(--accent-strong))' }}
              >
                {downloading ? <Loader2 size={15} className="animate-spin" /> : <Download size={15} />}
                {downloading ? 'Downloading…' : audioOnly ? 'Download MP3' : 'Download Video'}
              </button>
              {downloading && (
                <button onClick={handleCancel} className="flex items-center gap-1.5 px-3 py-2 rounded-xl text-xs text-[color:var(--text-2)] hover:text-[color:var(--accent-hover)] transition-colors"
                  style={{ background: 'var(--surface-2)' }}>
                  <X size={12} /> Cancel
                </button>
              )}
            </div>
          </div>
        )}

        {/* Progress */}
        {progress && (
          <div className="rounded-xl p-4 sm:p-5 min-w-0" style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}>
            {progress.status === 'complete' ? (
              <div className="flex items-center gap-3 text-[color:var(--accent-hover)]">
                <CheckCircle2 size={20} />
                <div>
                  <p className="font-semibold">Download complete!</p>
                  <p className="text-xs text-[color:var(--text-3)] mt-0.5 break-all">Saved to: {outputDir || progress.outputDir || '~/.mangavox/youtube/'}</p>
                </div>
              </div>
            ) : progress.status === 'error' ? (
              <div className="flex items-start gap-3 text-[color:var(--accent-hover)]">
                <AlertCircle size={20} className="shrink-0 mt-0.5" />
                <div>
                  <p className="font-semibold">Download failed</p>
                  <p className="text-xs text-[color:var(--text-3)] mt-1 font-mono break-words">{progress.error}</p>
                </div>
              </div>
            ) : (
              <div className="space-y-3">
                <div className="flex items-center justify-between text-sm">
                  <span className="text-[color:var(--text-2)] font-medium">
                    {progress.status === 'merging' ? 'Merging video + audio…' : 'Downloading…'}
                  </span>
                  <span className="text-[color:var(--text-1)] font-mono tabular-nums">
                    {progress.percent != null ? `${progress.percent.toFixed(1)}%` : ''}
                  </span>
                </div>
                <div className="h-2 rounded-full" style={{ background: 'var(--surface-3)' }}>
                  <div
                    className="h-full rounded-full transition-all duration-300"
                    style={{ width: `${progress.percent || 0}%`, background: 'var(--accent-gradient)' }}
                  />
                </div>
                <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-[color:var(--text-3)]">
                  {progress.size && <span>{progress.size}</span>}
                  {progress.speed && <span>@ {progress.speed}</span>}
                  {progress.eta && <span>ETA {progress.eta}</span>}
                </div>
              </div>
            )}
          </div>
        )}

        {/* Info note */}
        <div className="rounded-xl px-4 py-3 flex items-start gap-2.5" style={{ background: 'rgba(255,255,255,0.02)', border: '1px solid var(--panel-border)' }}>
          <AlertCircle size={13} className="text-[color:var(--text-3)] shrink-0 mt-0.5" />
          <p className="text-[11px] text-[color:var(--text-3)]">
            Downloads are saved to <code className="text-[color:var(--text-3)]">~/.mangavox/youtube/</code>.
            Powered by <strong className="text-[color:var(--text-3)]">yt-dlp</strong> — works with YouTube, Vimeo, Twitter, and 1000+ other sites.
          </p>
        </div>
      </div>
    </div>
  )
}
