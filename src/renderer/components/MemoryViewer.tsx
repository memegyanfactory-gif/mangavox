import { motion } from 'framer-motion'
import { Trash2, X, Brain, Search, Play, Download, Settings, Heart } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import type { AgentUserMemory } from '../../shared/agent-types'

interface MemoryViewerProps {
  memory: AgentUserMemory
  onClose: () => void
  onClearMemory: () => void
}

function formatWhen(ts: number): string {
  try {
    return new Date(ts).toLocaleString()
  } catch {
    return ''
  }
}

export function MemoryViewer({ memory, onClose, onClearMemory }: MemoryViewerProps) {
  const [activeTab, setActiveTab] = useState<'overview' | 'searches' | 'plays' | 'preferences' | 'personality'>('overview')
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false)

  const tabs = [
    { id: 'overview' as const, label: 'Overview', icon: Brain },
    { id: 'searches' as const, label: 'Searches', icon: Search },
    { id: 'plays' as const, label: 'Plays & Downloads', icon: Play },
    { id: 'preferences' as const, label: 'Preferences', icon: Settings },
    { id: 'personality' as const, label: 'Personality', icon: Heart },
  ]

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      className="rounded-2xl border overflow-hidden mb-2"
      style={{ background: 'var(--surface-1)', borderColor: 'var(--panel-border)' }}
    >
      <div className="p-3 border-b flex items-center justify-between gap-2" style={{ borderColor: 'var(--panel-border)' }}>
        <div className="min-w-0">
          <h3 className="font-semibold text-sm flex items-center gap-1.5" style={{ color: 'var(--text-1)' }}>
            <Brain size={14} /> Assistant Memory
          </h3>
          <p className="text-[10px] mt-0.5 truncate" style={{ color: 'var(--text-3)' }}>
            The AI reads this to personalize recommendations and remember your habits
          </p>
        </div>
        <button onClick={onClose} className="p-1.5 rounded-lg hover:bg-[var(--surface-3)] shrink-0" style={{ color: 'var(--text-3)' }}>
          <X size={14} />
        </button>
      </div>

      <div className="flex gap-1 p-2 border-b overflow-x-auto scrollbar-hide" style={{ borderColor: 'var(--panel-border)' }}>
        {tabs.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            onClick={() => setActiveTab(id)}
            className="px-2.5 py-1.5 rounded-lg text-[10px] font-medium transition-all shrink-0 flex items-center gap-1"
            style={{
              background: activeTab === id ? 'var(--accent-gradient-dim)' : 'transparent',
              color: activeTab === id ? 'var(--accent-hover)' : 'var(--text-3)',
              border: `1px solid ${activeTab === id ? 'var(--panel-border-strong)' : 'transparent'}`,
            }}
          >
            <Icon size={11} /> {label}
          </button>
        ))}
      </div>

      <div className="p-3 max-h-56 overflow-y-auto text-xs">
        {activeTab === 'overview' && (
          <div className="grid grid-cols-3 gap-2">
            <Stat label="Searches" value={memory.history.searches.length} />
            <Stat label="Plays" value={memory.history.plays.length} />
            <Stat label="Downloads" value={memory.history.downloads.length} />
            <div className="col-span-3 rounded-lg p-2.5 mt-1" style={{ background: 'var(--surface-2)' }}>
              <p style={{ color: 'var(--text-3)' }}>Top category</p>
              <p className="font-semibold mt-0.5 capitalize" style={{ color: 'var(--text-1)' }}>
                {memory.preferences.favoriteCategory || 'Still learning'}
              </p>
            </div>
          </div>
        )}

        {activeTab === 'searches' && (
          <ListSection
            empty="No searches saved yet — try /play or browse catalogs."
            items={memory.history.searches.slice().reverse().slice(0, 20).map((s) => ({
              primary: s.query,
              secondary: formatWhen(s.timestamp),
            }))}
          />
        )}

        {activeTab === 'plays' && (
          <div className="space-y-3">
            <ListSection
              title="Recent plays"
              icon={<Play size={11} />}
              empty="Nothing played via assistant yet."
              items={memory.history.plays.slice().reverse().slice(0, 12).map((p) => ({
                primary: p.title,
                secondary: `${p.type} · ${formatWhen(p.timestamp)}`,
              }))}
            />
            <ListSection
              title="Recent downloads"
              icon={<Download size={11} />}
              empty="No downloads recorded."
              items={memory.history.downloads.slice().reverse().slice(0, 10).map((d) => ({
                primary: d.name,
                secondary: `${d.format} · ${formatWhen(d.timestamp)}`,
              }))}
            />
          </div>
        )}

        {activeTab === 'preferences' && (
          <div className="space-y-2">
            <PrefRow label="Download format" value={memory.preferences.downloadFormat} />
            <PrefRow label="Preferred quality" value={memory.preferences.preferredQuality} />
            <PrefRow label="Favorite category" value={memory.preferences.favoriteCategory} />
            <PrefRow label="Save folder" value={memory.preferences.defaultSaveLocation || 'Not set'} />
          </div>
        )}

        {activeTab === 'personality' && (
          <div className="space-y-2">
            <PrefRow label="Communication style" value={memory.personality.communicationStyle} />
            <div className="rounded-lg p-2.5" style={{ background: 'var(--surface-2)' }}>
              <p className="text-[10px] mb-1.5" style={{ color: 'var(--text-3)' }}>Content types you watch</p>
              <TagList items={memory.personality.favoriteContentTypes} empty="Learning from your plays…" />
            </div>
            <div className="rounded-lg p-2.5" style={{ background: 'var(--surface-2)' }}>
              <p className="text-[10px] mb-1.5" style={{ color: 'var(--text-3)' }}>Genres & interests</p>
              <TagList
                items={[...memory.personality.favoriteGenres, ...memory.preferences.favoriteGenres]}
                empty="Will fill in as you explore more."
              />
            </div>
          </div>
        )}
      </div>

      <div className="p-2.5 border-t flex gap-2" style={{ borderColor: 'var(--panel-border)' }}>
        <button
          onClick={() => setShowDeleteConfirm(true)}
          className="flex-1 px-3 py-2 rounded-lg text-[11px] font-medium flex items-center justify-center gap-1"
          style={{ background: 'rgba(239,68,68,0.1)', color: '#ef4444', border: '1px solid rgba(239,68,68,0.2)' }}
        >
          <Trash2 size={12} /> Clear all memory
        </button>
      </div>

      {showDeleteConfirm && (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50 backdrop-blur-sm" onClick={() => setShowDeleteConfirm(false)}>
          <div
            className="rounded-2xl p-4 max-w-sm w-full"
            style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border)' }}
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="font-semibold text-sm mb-2" style={{ color: 'var(--text-1)' }}>Clear assistant memory?</h3>
            <p className="text-xs mb-4" style={{ color: 'var(--text-3)' }}>
              Removes search history, play habits, download prefs, and personality notes. The AI will start learning fresh.
            </p>
            <div className="flex gap-2">
              <button onClick={() => setShowDeleteConfirm(false)} className="flex-1 px-3 py-2 rounded-lg text-xs" style={{ background: 'var(--surface-2)', color: 'var(--text-2)' }}>
                Cancel
              </button>
              <button
                onClick={() => { onClearMemory(); setShowDeleteConfirm(false) }}
                className="flex-1 px-3 py-2 rounded-lg text-xs text-white"
                style={{ background: '#ef4444' }}
              >
                Clear
              </button>
            </div>
          </div>
        </div>
      )}
    </motion.div>
  )
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-lg p-2 text-center" style={{ background: 'var(--surface-2)' }}>
      <p className="text-[9px]" style={{ color: 'var(--text-3)' }}>{label}</p>
      <p className="text-base font-bold" style={{ color: 'var(--accent-hover)' }}>{value}</p>
    </div>
  )
}

function PrefRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg p-2.5 flex justify-between gap-3" style={{ background: 'var(--surface-2)' }}>
      <span style={{ color: 'var(--text-3)' }}>{label}</span>
      <span className="font-medium text-right capitalize" style={{ color: 'var(--text-1)' }}>{value}</span>
    </div>
  )
}

function TagList({ items, empty }: { items: string[]; empty: string }) {
  const unique = [...new Set(items.filter(Boolean))]
  if (!unique.length) return <p className="text-[10px]" style={{ color: 'var(--text-3)' }}>{empty}</p>
  return (
    <div className="flex flex-wrap gap-1">
      {unique.map((item) => (
        <span key={item} className="text-[10px] px-2 py-0.5 rounded-full capitalize" style={{ background: 'var(--accent-gradient-dim)', color: 'var(--accent-hover)' }}>
          {item}
        </span>
      ))}
    </div>
  )
}

function ListSection({
  title, icon, empty, items,
}: {
  title?: string
  icon?: ReactNode
  empty: string
  items: { primary: string; secondary?: string }[]
}) {
  return (
    <div>
      {title && (
        <h4 className="text-[10px] font-medium mb-1.5 flex items-center gap-1" style={{ color: 'var(--text-2)' }}>
          {icon} {title}
        </h4>
      )}
      {!items.length ? (
        <p className="text-[10px]" style={{ color: 'var(--text-3)' }}>{empty}</p>
      ) : (
        <div className="space-y-1">
          {items.map((item, idx) => (
            <div key={`${item.primary}-${idx}`} className="px-2 py-1.5 rounded-lg" style={{ background: 'var(--surface-2)' }}>
              <p className="truncate" style={{ color: 'var(--text-1)' }}>{item.primary}</p>
              {item.secondary && <p className="text-[9px] mt-0.5" style={{ color: 'var(--text-3)' }}>{item.secondary}</p>}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
