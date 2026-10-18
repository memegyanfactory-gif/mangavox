import { motion } from 'framer-motion'
import { FolderOpen, Check } from 'lucide-react'
import { useEffect, useState } from 'react'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'

export interface DownloadOptions {
  format: string
  resolution: string
  outputPath: string
}

interface DownloadOptionsProps {
  onConfirm: (options: DownloadOptions) => void
  onCancel: () => void
  defaultPath?: string
  title?: string
}

const FORMATS = ['MP4', 'MP3', 'WebM', 'MKV', 'AAC', 'OPUS']
const RESOLUTIONS = ['4K', '1080p', '720p', '480p', 'Audio-only']

export function DownloadOptionsPanel({ onConfirm, onCancel, defaultPath, title }: DownloadOptionsProps) {
  const [format, setFormat] = useState('MP4')
  const [resolution, setResolution] = useState('1080p')
  const [outputPath, setOutputPath] = useState(defaultPath || '')
  const [isSelectingPath, setIsSelectingPath] = useState(false)

  useEffect(() => {
    if (defaultPath) setOutputPath(defaultPath)
  }, [defaultPath])

  const handleSelectPath = async () => {
    setIsSelectingPath(true)
    try {
      const path = await invoke<string | null>(IPC.AGENT_PICK_FOLDER)
      if (path) setOutputPath(path)
    } finally {
      setIsSelectingPath(false)
    }
  }

  const handleConfirm = () => {
    if (!outputPath) return
    onConfirm({
      format,
      resolution: resolution === 'Audio-only' ? 'audio' : resolution,
      outputPath,
    })
  }

  return (
    <motion.div
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -12 }}
      className="rounded-2xl border p-4"
      style={{ background: 'var(--surface-1)', borderColor: 'var(--panel-border)' }}
    >
      <h3 className="font-semibold text-sm mb-1" style={{ color: 'var(--text-1)' }}>
        Download Options
      </h3>
      {title && <p className="text-xs mb-4 truncate" style={{ color: 'var(--text-3)' }}>{title}</p>}

      <div className="mb-4">
        <label className="text-xs font-medium block mb-2" style={{ color: 'var(--text-2)' }}>Format</label>
        <div className="grid grid-cols-3 gap-2">
          {FORMATS.map((fmt) => (
            <button key={fmt} onClick={() => setFormat(fmt)}
              className="px-3 py-2 rounded-lg text-xs font-medium transition-all"
              style={{
                background: format === fmt ? 'var(--accent-gradient)' : 'var(--surface-2)',
                color: format === fmt ? 'white' : 'var(--text-2)',
                border: `1px solid ${format === fmt ? 'var(--accent)' : 'var(--panel-border)'}`,
              }}>
              {fmt}
            </button>
          ))}
        </div>
      </div>

      <div className="mb-4">
        <label className="text-xs font-medium block mb-2" style={{ color: 'var(--text-2)' }}>Resolution</label>
        <div className="grid grid-cols-3 gap-2">
          {RESOLUTIONS.map((res) => (
            <button key={res} onClick={() => setResolution(res)}
              className="px-3 py-2 rounded-lg text-xs font-medium transition-all"
              style={{
                background: resolution === res ? 'var(--accent-gradient)' : 'var(--surface-2)',
                color: resolution === res ? 'white' : 'var(--text-2)',
                border: `1px solid ${resolution === res ? 'var(--accent)' : 'var(--panel-border)'}`,
              }}>
              {res}
            </button>
          ))}
        </div>
      </div>

      <div className="mb-4">
        <label className="text-xs font-medium block mb-2" style={{ color: 'var(--text-2)' }}>Save Location</label>
        <div className="flex gap-2">
          <div className="flex-1 rounded-lg px-3 py-2 text-xs truncate"
            style={{ background: 'var(--surface-2)', color: outputPath ? 'var(--text-2)' : 'var(--text-3)' }}>
            {outputPath || 'Choose a folder…'}
          </div>
          <button onClick={handleSelectPath} disabled={isSelectingPath}
            className="px-3 py-2 rounded-lg text-xs font-medium flex items-center gap-1"
            style={{ background: 'var(--surface-2)', color: 'var(--accent)', opacity: isSelectingPath ? 0.5 : 1 }}>
            <FolderOpen size={14} /> Browse
          </button>
        </div>
      </div>

      <div className="flex gap-2">
        <button onClick={onCancel} className="flex-1 px-3 py-2 rounded-lg text-xs font-medium"
          style={{ background: 'var(--surface-2)', color: 'var(--text-2)', border: '1px solid var(--panel-border)' }}>
          Cancel
        </button>
        <button onClick={handleConfirm} disabled={!outputPath}
          className="flex-1 px-3 py-2 rounded-lg text-xs font-medium flex items-center justify-center gap-1 disabled:opacity-40"
          style={{ background: 'var(--accent-gradient)', color: 'white' }}>
          <Check size={14} /> Download
        </button>
      </div>
    </motion.div>
  )
}
