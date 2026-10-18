// Always-available assistant: a collapsible, resizable side panel on every page.
import { useEffect, useMemo, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { MessageSquarePlus, PanelLeft, PanelLeftClose, Pencil, Search, Sparkles, Trash2 } from 'lucide-react'
import { useAssistantStore, MIN_WIDTH, MAX_WIDTH } from '../../stores/assistant.store'
import { useUiStore } from '../../stores/ui.store'
import { useAiCatalog } from '../../assistant/catalog'
import { sendToAssistant } from '../../assistant/runtime'
import { MessageList } from './MessageList'
import { Composer } from './Composer'
import './assistant.css'

const SUGGESTIONS: Record<string, string[]> = {
  anime: ['Find me a cozy slice-of-life anime', 'What was I watching last?', 'Play Frieren'],
  'anime-detail': ['Is this worth watching?', 'Find similar anime', 'Play the first episode'],
  movies: ['Recommend a clever sci-fi movie', 'Show me Interstellar', 'What did I watch recently?'],
  library: ["What's in my library?", 'Open One Piece manga', 'Recommend a manga like Berserk'],
  browse: ['Open Chainsaw Man manga', 'Recommend a short completed manga', "What's in my library?"],
  settings: ['Set up a local AI model', 'Switch to a darker theme', 'Which AI providers can I use?'],
}
const DEFAULT_SUGGESTIONS = ['What can you do in this app?', 'Switch to a darker theme', 'Set up a local AI model', 'Find me something to watch tonight']

function timeAgo(ts: number): string {
  const m = Math.round((Date.now() - ts) / 60000)
  if (m < 1) return 'now'
  if (m < 60) return `${m}m`
  const h = Math.round(m / 60)
  if (h < 24) return `${h}h`
  return `${Math.round(h / 24)}d`
}

/** Pick a sensible model on first run: first ready local model, else a signed-in CLI. */
function useAutoModel() {
  const { model, setModel } = useAssistantStore()
  const { providers, refresh, loadModels } = useAiCatalog()
  useEffect(() => { void refresh().catch(() => {}) }, [refresh])
  useEffect(() => {
    if (model || !providers.length) return
    let cancelled = false
    void (async () => {
      const order = ['ollama', 'lmstudio', 'claude-code', 'codex', 'opencode', 'anthropic', 'openai', 'gemini', 'openrouter'] as const
      for (const id of order) {
        const p = providers.find((x) => x.id === id && x.state === 'ready')
        if (!p) continue
        const list = await loadModels(id).catch(() => [])
        const pick = p.defaultModel || list[0]?.id
        if (pick && !cancelled) { setModel(id, pick); return }
      }
    })()
    return () => { cancelled = true }
  }, [model, providers, setModel, loadModels])
}

function ThreadRail({ onPick }: { onPick: () => void }) {
  const { threads, activeId, selectThread, deleteThread, renameThread, newThread } = useAssistantStore()
  const [q, setQ] = useState('')
  const [editing, setEditing] = useState<string | null>(null)
  const shown = useMemo(() => {
    const s = q.trim().toLowerCase()
    return threads.filter((t) => !s || t.title.toLowerCase().includes(s) || t.items.some((i) => i.text.toLowerCase().includes(s)))
  }, [threads, q])

  return (
    <motion.div
      className="mv-ai-rail"
      initial={{ x: -16, opacity: 0 }}
      animate={{ x: 0, opacity: 1 }}
      exit={{ x: -16, opacity: 0 }}
      transition={{ duration: 0.14 }}
    >
      <button type="button" className="mv-ai-rail-new" onClick={() => { newThread(); onPick() }}>
        <MessageSquarePlus size={14} /> New chat
      </button>
      <div className="mv-ai-rail-search">
        <Search size={12} className="mv-ai-muted" />
        <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search chats" />
      </div>
      <div className="mv-ai-rail-list">
        {shown.length === 0 && <p className="mv-ai-muted text-[12px] px-3 py-2">{threads.length ? 'No matches' : 'No chats yet'}</p>}
        {shown.map((t) => (
          <div key={t.id} className={`mv-ai-rail-item group ${t.id === activeId ? 'is-active' : ''}`}>
            {editing === t.id ? (
              <input
                autoFocus
                defaultValue={t.title}
                className="mv-ai-rail-rename"
                onBlur={(e) => { renameThread(t.id, e.target.value); setEditing(null) }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
                  if (e.key === 'Escape') setEditing(null)
                }}
              />
            ) : (
              <button type="button" className="flex-1 min-w-0 text-left" onClick={() => { selectThread(t.id); onPick() }}>
                <div className="truncate">{t.title}</div>
                <div className="mv-ai-muted text-[11px]">{timeAgo(t.updatedAt)} · {t.items.filter((i) => i.role === 'user').length} messages</div>
              </button>
            )}
            <div className="mv-ai-rail-actions">
              <button type="button" title="Rename" onClick={() => setEditing(t.id)}><Pencil size={12} /></button>
              <button type="button" title="Delete" onClick={() => deleteThread(t.id)}><Trash2 size={12} /></button>
            </div>
          </div>
        ))}
      </div>
    </motion.div>
  )
}

function EmptyState() {
  const page = useUiStore((s) => s.currentPage)
  const ideas = SUGGESTIONS[page] ?? DEFAULT_SUGGESTIONS
  return (
    <div className="mv-ai-empty">
      <div className="mv-ai-empty-mark"><Sparkles size={18} /></div>
      <h2>What can I help with?</h2>
      <p>Ask a question, or tell me what to do — I can open pages, find and play titles, and change settings for you.</p>
      <div className="mv-ai-suggestions">
        {ideas.map((s) => (
          <button key={s} type="button" onClick={() => void sendToAssistant(s)}>{s}</button>
        ))}
      </div>
    </div>
  )
}

export function AssistantDock({ overlay }: { overlay: boolean }) {
  const { open, width, railOpen, threads, activeId, setOpen, setWidth, setRailOpen, newThread } = useAssistantStore()
  const thread = threads.find((t) => t.id === activeId)
  const dragging = useRef(false)
  useAutoModel()

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === 'j') {
        e.preventDefault()
        useAssistantStore.getState().toggle()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const startResize = (e: React.PointerEvent) => {
    e.preventDefault()
    dragging.current = true
    const startX = e.clientX
    const startW = width
    const move = (ev: PointerEvent) => setWidth(startW + (ev.clientX - startX))
    const up = () => {
      dragging.current = false
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      document.body.style.cursor = ''
    }
    document.body.style.cursor = 'col-resize'
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  return (
    <AnimatePresence initial={false}>
      {open && (
        <motion.aside
          key="dock"
          className={`mv-ai-dock ${overlay ? 'is-overlay' : ''}`}
          style={{ width: overlay ? `min(100vw, ${width}px)` : width }}
          initial={overlay ? { x: -40, opacity: 0 } : { width: 0, opacity: 0 }}
          animate={overlay ? { x: 0, opacity: 1 } : { width, opacity: 1 }}
          exit={overlay ? { x: -40, opacity: 0 } : { width: 0, opacity: 0 }}
          transition={{ duration: dragging.current ? 0 : 0.18, ease: [0.2, 0, 0, 1] }}
          aria-label="Assistant"
        >
          {!overlay && (
            <div
              className="mv-ai-resize"
              onPointerDown={startResize}
              onDoubleClick={() => setWidth(420)}
              title="Drag to resize · double-click to reset"
              role="separator"
              aria-valuemin={MIN_WIDTH}
              aria-valuemax={MAX_WIDTH}
              aria-valuenow={width}
            />
          )}
          <div className="mv-ai-inner" style={{ minWidth: MIN_WIDTH }}>
            <header className="mv-ai-header">
              <button type="button" className="mv-ai-icon-btn" title="Chats" onClick={() => setRailOpen(!railOpen)} aria-pressed={railOpen}>
                <PanelLeft size={16} />
              </button>
              <div className="flex-1 min-w-0 truncate text-[13px] font-medium" style={{ color: 'var(--text-1)' }}>
                {thread?.title ?? 'Assistant'}
              </div>
              <button type="button" className="mv-ai-icon-btn" title="New chat" onClick={() => { newThread(); setRailOpen(false) }}>
                <MessageSquarePlus size={16} />
              </button>
              <button type="button" className="mv-ai-icon-btn" title="Close (Ctrl+J)" onClick={() => setOpen(false)}>
                <PanelLeftClose size={16} />
              </button>
            </header>

            <div className="relative flex-1 min-h-0 flex flex-col">
              <AnimatePresence>
                {railOpen && (
                  <>
                    <motion.div key="scrim" className="mv-ai-rail-scrim" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onClick={() => setRailOpen(false)} />
                    <ThreadRail key="rail" onPick={() => setRailOpen(false)} />
                  </>
                )}
              </AnimatePresence>
              {thread && thread.items.length > 0 ? <MessageList items={thread.items} /> : <EmptyState />}
              <Composer autoFocusKey={open ? 'open' : null} />
            </div>
          </div>
        </motion.aside>
      )}
    </AnimatePresence>
  )
}

/** Floating button shown while the dock is closed. */
export function AssistantLauncher({ hidden }: { hidden?: boolean }) {
  const { open, setOpen, busyThreadId } = useAssistantStore()
  if (open || hidden) return null
  return (
    <button type="button" className="mv-ai-launcher" onClick={() => setOpen(true)} title="Assistant (Ctrl+J)" aria-label="Open assistant">
      <Sparkles size={18} />
      {busyThreadId && <span className="mv-ai-launcher-dot" />}
    </button>
  )
}
