import { useState, useEffect } from 'react'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { Subtitles as SubtitlesIcon, Upload, Trash2, Eye, FileText, Loader2 } from 'lucide-react'

interface SubtitleFile {
  name: string
  path: string
  size: number
  animeTitle?: string
}

export function Subtitles() {
  const [subtitles, setSubtitles] = useState<SubtitleFile[]>([])
  const [loading, setLoading] = useState(false)
  const [viewing, setViewing] = useState<string | null>(null)
  const [viewContent, setViewContent] = useState('')

  // Placeholder — will connect to backend when subtitle management IPC is ready
  const importSubtitle = async () => {
    // Will use dialog.showOpenDialog via IPC
    console.log('[subtitles] Import not yet connected')
  }

  return (
    <div className="flex flex-col h-full page-enter">
      {/* Header */}
      <div className="px-5 pt-3 pb-2">
        <h2
          className="text-xl font-bold tracking-tight"
          style={{ color: 'var(--text-1)' }}
        >
          Subtitles
        </h2>
        <p className="text-sm text-[var(--text-3)] mt-1">
          Manage subtitle files for anime playback
        </p>
      </div>

      {/* Actions */}
      <div className="px-5 mb-4 flex gap-3">
        <button
          onClick={importSubtitle}
          className="flex items-center gap-2 px-4 py-2 rounded-xl text-sm font-medium text-white transition-all hover:brightness-110"
          style={{ background: 'linear-gradient(135deg, #7C3AED, #2563EB)' }}
        >
          <Upload size={14} /> Import SRT / VTT
        </button>
      </div>

      {/* Subtitle list */}
      <div className="flex-1 overflow-y-auto px-5 pb-8">
        {subtitles.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-4 py-20">
            <div
              className="w-16 h-16 rounded-2xl flex items-center justify-center"
              style={{ background: 'var(--surface-2)' }}
            >
              <SubtitlesIcon size={28} className="text-[var(--text-3)]" />
            </div>
            <div className="text-center">
              <p className="text-sm text-[var(--text-2)]">No subtitles imported yet</p>
              <p className="text-xs text-[var(--text-3)] mt-1">
                Import .srt or .vtt files to use during anime playback
              </p>
            </div>
          </div>
        ) : (
          <div className="space-y-1">
            {subtitles.map((sub) => (
              <div
                key={sub.path}
                className="flex items-center gap-3 px-4 py-3 rounded-xl text-sm hover:bg-[var(--surface-3)] transition-colors"
              >
                <FileText size={16} className="text-[var(--text-3)] shrink-0" />
                <div className="flex-1 min-w-0">
                  <p className="text-[var(--text-2)] truncate">{sub.name}</p>
                  {sub.animeTitle && (
                    <p className="text-xs text-[var(--text-3)]">{sub.animeTitle}</p>
                  )}
                </div>
                <button
                  onClick={() => setViewing(sub.path)}
                  className="text-[var(--text-3)] hover:text-[var(--text-2)] transition-colors"
                >
                  <Eye size={14} />
                </button>
                <button className="text-[var(--text-3)] hover:text-emerald-400 transition-colors">
                  <Trash2 size={14} />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
