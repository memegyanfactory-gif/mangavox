import { useEffect, useRef, useState, useCallback } from 'react'
import { useLoggingStore, type LogLevel, type AgentMessage, type PermissionMode } from '../../stores/logging.store'
import { invoke, onEvent } from '../../hooks/useIpc'
import { IPC } from '../../../shared/ipc-channels'
import { useIsCompact } from '../../hooks/useBreakpoint'
import {
  Trash2, Pause, Play, Clock, ArrowDownToLine, Bot, Terminal,
  Send, Zap, Loader2, Square, TriangleAlert, Shield, ShieldCheck,
  ShieldX, Check, X, RotateCcw, FileEdit, FolderOpen, Copy,
} from 'lucide-react'

// ── Copy helper ──

function CopyButton({ text, className = '' }: { text: string; className?: string }) {
  const [copied, setCopied] = useState(false)
  const copy = () => {
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    }).catch(() => {})
  }
  return (
    <button
      onClick={copy}
      title="Copy"
      className={`flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded transition-all ${
        copied ? 'text-[var(--success)]' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
      } ${className}`}
    >
      {copied ? <Check size={10} /> : <Copy size={10} />}
      {copied ? 'Copied' : ''}
    </button>
  )
}

// ── Log view helpers ──

const SOURCE_TAG_COLORS: Record<string, string> = {
  ocr: 'text-emerald-300', tts: 'text-emerald-400', video: 'text-blue-300',
  sync_map: 'text-blue-400', text: 'text-emerald-300', vision: 'text-blue-300',
  enhance: 'text-emerald-400', download: 'text-[var(--text-2)]', pipeline: 'text-emerald-300',
  python: 'text-blue-400', main: 'text-[var(--text-3)]', renderer: 'text-[var(--text-3)]',
  ollama: 'text-emerald-300', agent: 'text-blue-400', torrent: 'text-[var(--text-2)]',
  anime: 'text-blue-300', addon: 'text-emerald-400',
}

function sourceTagClass(source: string): string {
  const lower = source.toLowerCase()
  for (const key of Object.keys(SOURCE_TAG_COLORS)) {
    if (lower.includes(key)) return SOURCE_TAG_COLORS[key]
  }
  return 'text-[var(--text-3)]'
}

const LEVEL_COLORS: Record<LogLevel, string> = {
  debug: 'text-[var(--text-3)]', info: 'text-[var(--info)]', warn: 'text-[var(--warning)]', error: 'text-[var(--error)]',
}

const LEVEL_LABELS: Record<LogLevel, string> = {
  debug: 'DBG', info: 'INF', warn: 'WRN', error: 'ERR',
}

function formatEta(ms: number): string {
  if (ms <= 0) return ''
  const secs = Math.round(ms / 1000)
  if (secs < 60) return `~${secs}s`
  const mins = Math.floor(secs / 60)
  const remainSecs = secs % 60
  if (mins < 60) return `~${mins}m ${remainSecs}s`
  const hrs = Math.floor(mins / 60)
  return `~${hrs}h ${mins % 60}m`
}

// ── Permission mode selector ──

function PermissionModeSelector() {
  const { permissionMode, setPermissionMode } = useLoggingStore()

  const modes: { id: PermissionMode; label: string; icon: React.ReactNode; color: string; desc: string }[] = [
    { id: 'auto', label: 'Auto', icon: <ShieldCheck size={11} />, color: 'text-[var(--success)]', desc: 'Execute all actions automatically' },
    { id: 'manual', label: 'Manual', icon: <Shield size={11} />, color: 'text-[var(--warning)]', desc: 'Ask before each action' },
    { id: 'decline', label: 'Decline', icon: <ShieldX size={11} />, color: 'text-[var(--error)]', desc: 'Block all actions' },
  ]

  return (
    <div className="flex rounded-lg p-0.5" style={{ background: 'var(--surface-2)' }}>
      {modes.map(({ id, label, icon, color }) => {
        const active = permissionMode === id
        return (
          <button
            key={id}
            onClick={() => setPermissionMode(id)}
            className={`flex items-center gap-1 text-[10px] px-2 py-1 rounded-md transition-all ${
              active ? `${color} font-semibold` : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
            }`}
            style={active ? { background: 'var(--surface-4)' } : {}}
            title={modes.find((m) => m.id === id)?.desc}
          >
            {icon} {label}
          </button>
        )
      })}
    </div>
  )
}

// ── Action approval card ──

function ActionApprovalCard({ action, onApprove, onDecline }: {
  action: any
  onApprove: () => void
  onDecline: () => void
}) {
  const typeIcon = action.type === 'bash'
    ? <Terminal size={13} className="text-emerald-400" />
    : action.type === 'file_edit'
      ? <FileEdit size={13} className="text-emerald-400" />
      : <FolderOpen size={13} className="text-blue-400" />

  const typeColor = action.type === 'bash' ? 'border-emerald-500/20' : action.type === 'file_edit' ? 'border-emerald-500/25' : 'border-[var(--panel-border)]'

  return (
    <div className={`rounded-xl border ${typeColor} p-3 mb-2`}
      style={{ background: 'var(--panel-border)' }}>
      <div className="flex items-center gap-2 mb-2">
        {typeIcon}
        <span className="text-xs font-semibold text-[var(--text-2)]">{action.description}</span>
      </div>

      {action.type === 'bash' && action.command && (
        <div className="font-mono text-[11px] text-emerald-400 px-2 py-1.5 rounded-lg mb-2"
          style={{ background: 'rgba(200, 80, 26,0.06)', border: '1px solid rgba(200, 80, 26,0.1)' }}>
          $ {action.command}
        </div>
      )}

      {action.type === 'file_edit' && (
        <div className="text-[10px] text-[var(--text-3)] mb-2">
          <div>File: <span className="text-[var(--text-2)]">{action.filePath}</span></div>
          {action.oldText && (
            <div className="mt-1 font-mono px-2 py-1 rounded" style={{ background: 'rgba(239,68,68,0.06)' }}>
              <span className="text-[var(--error)]">- </span>
              <span className="text-[var(--error)]">{action.oldText.substring(0, 100)}{action.oldText.length > 100 ? '...' : ''}</span>
            </div>
          )}
          {action.newText && (
            <div className="mt-0.5 font-mono px-2 py-1 rounded" style={{ background: 'rgba(200, 80, 26,0.06)' }}>
              <span className="text-[var(--success)]">+ </span>
              <span className="text-[var(--success)]">{action.newText.substring(0, 100)}{action.newText.length > 100 ? '...' : ''}</span>
            </div>
          )}
        </div>
      )}

      {action.type === 'file_write' && (
        <div className="text-[10px] text-[var(--text-3)] mb-2">
          <div>Create: <span className="text-[var(--text-2)]">{action.filePath}</span></div>
        </div>
      )}

      <div className="flex items-center gap-2">
        <button
          onClick={onApprove}
          className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-[11px] font-medium text-[var(--success)] hover:brightness-125 transition-all"
          style={{ background: 'rgba(var(--success-rgb),0.1)', border: '1px solid rgba(var(--success-rgb),0.25)' }}
        >
          <Check size={11} /> Approve
        </button>
        <button
          onClick={onDecline}
          className="flex items-center gap-1 px-3 py-1.5 rounded-lg text-[11px] font-medium text-[var(--error)] hover:brightness-125 transition-all"
          style={{ background: 'rgba(var(--error-rgb),0.08)', border: '1px solid rgba(var(--error-rgb),0.2)' }}
        >
          <X size={11} /> Decline
        </button>
      </div>
    </div>
  )
}

// ── Fix status message rendering ──

function FixStatusBubble({ msg }: { msg: AgentMessage }) {
  const phase = msg.fixPhase

  const phaseColors: Record<string, string> = {
    diagnosing: 'text-emerald-400',
    planning: 'text-[var(--info)]',
    executing: 'text-[var(--warning)]',
    verifying: 'text-cyan-400',
    retrying: 'text-[var(--accent-hover)]',
    done: 'text-[var(--success)]',
    failed: 'text-[var(--error)]',
    cancelled: 'text-[var(--text-2)]',
  }

  const phaseIcons: Record<string, React.ReactNode> = {
    diagnosing: <Loader2 size={12} className="animate-spin text-emerald-400" />,
    planning: <Bot size={12} className="text-blue-400" />,
    executing: <Terminal size={12} className="text-amber-400" />,
    verifying: <RotateCcw size={12} className="animate-spin text-cyan-400" />,
    retrying: <RotateCcw size={12} className="text-emerald-400" />,
    done: <Check size={12} className="text-[var(--success)]" />,
    failed: <X size={12} className="text-[var(--error)]" />,
    cancelled: <Square size={12} className="text-[var(--text-2)]" />,
  }

  return (
    <div className="flex items-start gap-2 mb-2">
      <div className="w-5 h-5 rounded-md flex items-center justify-center shrink-0 mt-0.5"
        style={{ background: 'rgba(200, 80, 26,0.1)' }}>
        {phaseIcons[phase || ''] || <Bot size={11} className="text-emerald-400" />}
      </div>
      <div className="flex-1">
        <div className={`text-[10px] font-semibold uppercase tracking-wide mb-0.5 ${phaseColors[phase || ''] || 'text-[var(--text-2)]'}`}>
          {phase}
        </div>
        <div className="text-xs text-[var(--text-2)] leading-relaxed">
          {msg.text.split('\n').map((line, i) => {
            const boldMatch = line.match(/^\*\*(.+?)\*\*:?\s*(.*)/)
            if (boldMatch) {
              return (
                <div key={i} className="mb-0.5">
                  <span className="text-white font-semibold">{boldMatch[1]}</span>
                  {boldMatch[2] && <span className="text-[var(--text-2)]">: {boldMatch[2]}</span>}
                </div>
              )
            }
            if (line.trim() === '') return <div key={i} className="h-1" />
            return <div key={i} className="mb-0.5">{line}</div>
          })}
        </div>
      </div>
    </div>
  )
}

// ── Agent chat message rendering ──

function AgentMessageBubble({ msg }: { msg: AgentMessage }) {
  if (msg.role === 'fix-status') {
    return <FixStatusBubble msg={msg} />
  }

  if (msg.role === 'user') {
    return (
      <div className="flex justify-end mb-2 group">
        <div className="flex items-end gap-1 max-w-[85%]">
          <CopyButton text={msg.text} className="opacity-0 group-hover:opacity-100 mb-1" />
          <div className="px-3 py-2 rounded-xl text-xs text-[var(--text-2)] leading-relaxed"
            style={{ background: 'rgba(200, 80, 26,0.15)' }}>
            {msg.text}
          </div>
        </div>
      </div>
    )
  }

  if (msg.role === 'command') {
    return (
      <div className="mb-2 flex items-start gap-2">
        <Terminal size={12} className="text-emerald-400 mt-0.5 shrink-0" />
        <div className="flex-1">
          <div className="flex items-center justify-between font-mono text-[11px] text-emerald-400 px-2 py-1.5 rounded-lg group"
            style={{ background: 'rgba(200, 80, 26,0.08)', border: '1px solid rgba(200, 80, 26,0.15)' }}>
            <span>$ {msg.text}</span>
            <CopyButton text={msg.text} className="opacity-0 group-hover:opacity-100" />
          </div>
        </div>
      </div>
    )
  }

  if (msg.role === 'output') {
    return (
      <div className="mb-2 ml-5 group relative">
        <CopyButton text={msg.text} className="absolute top-1 right-1 opacity-0 group-hover:opacity-100 z-10" />
        <pre className="font-mono text-[10px] text-[var(--text-2)] px-2 py-1.5 rounded-lg whitespace-pre-wrap break-all max-h-40 overflow-y-auto"
          style={{ background: 'rgba(0,0,0,0.3)' }}>
          {msg.text}
          {msg.isRunning && <span className="inline-block w-1.5 h-3 bg-emerald-400 animate-pulse ml-0.5" />}
        </pre>
      </div>
    )
  }

  // agent response — full markdown-like rendering with code block support
  const renderAgentText = (text: string) => {
    const segments: React.ReactNode[] = []
    const lines = text.split('\n')
    let i = 0

    while (i < lines.length) {
      const line = lines[i]

      // Fenced code block
      if (line.startsWith('```')) {
        const lang = line.slice(3).trim()
        const blockLines: string[] = []
        i++
        while (i < lines.length && !lines[i].startsWith('```')) {
          blockLines.push(lines[i])
          i++
        }
        const code = blockLines.join('\n')
        segments.push(
          <div key={i} className="my-2 group relative rounded-lg overflow-hidden"
            style={{ background: 'rgba(0,0,0,0.35)', border: '1px solid var(--panel-border)' }}>
            <div className="flex items-center justify-between px-2 py-1"
              style={{ background: 'var(--panel-border)', borderBottom: '1px solid var(--panel-border)' }}>
              <span className="text-[9px] text-[var(--text-3)] font-mono uppercase tracking-wider">{lang || 'code'}</span>
              <CopyButton text={code} />
            </div>
            <pre className="font-mono text-[11px] text-[var(--text-2)] px-3 py-2 overflow-x-auto whitespace-pre leading-relaxed">
              {code}
            </pre>
          </div>
        )
        i++ // skip closing ```
        continue
      }

      // Bold heading: **text**: rest
      const boldMatch = line.match(/^\*\*(.+?)\*\*:?\s*(.*)/)
      if (boldMatch) {
        segments.push(
          <div key={i} className="mb-1">
            <span className="text-white font-semibold">{boldMatch[1]}</span>
            {boldMatch[2] && <span className="text-[var(--text-2)]">: {boldMatch[2]}</span>}
          </div>
        )
        i++; continue
      }

      // Hide <tool>…</tool> lines from the rendered output
      if (line.match(/<tool>.+<\/tool>/)) { i++; continue }

      // Whole-line inline code `cmd`
      if (line.match(/^`[^`]+`$/)) {
        const cmd = line.slice(1, -1)
        segments.push(
          <div key={i} className="my-1 flex items-center gap-1">
            <code className="font-mono text-[11px] text-emerald-400 px-1.5 py-0.5 rounded"
              style={{ background: 'rgba(200, 80, 26,0.08)' }}>
              {cmd}
            </code>
            <CopyButton text={cmd} />
          </div>
        )
        i++; continue
      }

      // Mixed inline code within a line
      const parts = line.split(/(`[^`]+`)/)
      if (parts.length > 1) {
        segments.push(
          <div key={i} className="mb-0.5">
            {parts.map((part, j) => {
              if (part.startsWith('`') && part.endsWith('`')) {
                return (
                  <code key={j} className="font-mono text-[10px] text-emerald-400 px-1 py-0.5 rounded"
                    style={{ background: 'rgba(200, 80, 26,0.08)' }}>
                    {part.slice(1, -1)}
                  </code>
                )
              }
              return <span key={j}>{part}</span>
            })}
          </div>
        )
        i++; continue
      }

      if (line.trim() === '') { segments.push(<div key={i} className="h-1" />); i++; continue }
      segments.push(<div key={i} className="mb-0.5">{line}</div>)
      i++
    }

    return segments
  }

  return (
    <div className="flex items-start gap-2 mb-2 group">
      <div className="w-5 h-5 rounded-md flex items-center justify-center shrink-0 mt-0.5"
        style={{ background: 'rgba(200, 80, 26,0.15)' }}>
        <Bot size={11} className="text-emerald-400" />
      </div>
      <div className="flex-1 text-xs text-[var(--text-2)] leading-relaxed min-w-0">
        {renderAgentText(msg.text)}
        {msg.text && (
          <div className="mt-1 opacity-0 group-hover:opacity-100 transition-opacity">
            <CopyButton text={msg.text} />
          </div>
        )}
      </div>
    </div>
  )
}

// ── Main LogPanel ──

export function LogPanel() {
  const {
    entries, isOpen, filter, unreadErrors, panelView,
    agentMessages, agentLoading,
    permissionMode, fixInProgress, pendingAction,
    togglePanel, setFilter, clearLogs, addEntry, addRendererLog,
    setPanelView, addAgentMessage, updateLastAgentMessage, setAgentLoading, clearAgentChat,
    setFixInProgress, setPendingAction, clearFixedErrors,
  } = useLoggingStore()

  const scrollRef = useRef<HTMLDivElement>(null)
  const agentScrollRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const [autoScroll, setAutoScroll] = useState(true)
  const [paused, setPaused] = useState(false)
  const [activeCount, setActiveCount] = useState(0)
  const [etaMs, setEtaMs] = useState<number | null>(null)
  const [currentStage, setCurrentStage] = useState<string | null>(null)
  const [currentProgress, setCurrentProgress] = useState<{ current: number; total: number } | null>(null)
  const [agentInput, setAgentInput] = useState('')
  const compact = useIsCompact()

  // Log entry listener
  useEffect(() => {
    return onEvent('log:entry', (entry: any) => {
      if (entry && entry.level && entry.message) addEntry(entry)
    })
  }, [])

  // Capture renderer console
  useEffect(() => {
    const origError = console.error
    const origWarn = console.warn
    console.error = (...args: unknown[]) => {
      origError(...args)
      const msg = args.map(a => a instanceof Error ? a.message : String(a)).join(' ')
      addRendererLog('error', msg)
    }
    console.warn = (...args: unknown[]) => {
      origWarn(...args)
      const msg = args.map(a => String(a)).join(' ')
      addRendererLog('warn', msg)
    }
    return () => { console.error = origError; console.warn = origWarn }
  }, [])

  // Auto-scroll logs
  useEffect(() => {
    if (isOpen && autoScroll && panelView === 'logs' && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [entries.length, isOpen, autoScroll, panelView])

  // Auto-scroll agent chat
  useEffect(() => {
    if (isOpen && panelView === 'agent' && agentScrollRef.current) {
      agentScrollRef.current.scrollTop = agentScrollRef.current.scrollHeight
    }
  }, [agentMessages.length, agentLoading, isOpen, panelView, pendingAction])

  // Streaming chat listener — appends tokens to the last agent message as
  // they arrive from Ollama. The final full string still comes back via
  // the AGENT_CHAT invoke reply, but rendering as we go makes the agent
  // feel alive instead of freezing while the LLM thinks.
  useEffect(() => {
    return onEvent(IPC.AGENT_CHAT_STREAM, ((data: { delta: string; done: boolean }) => {
      if (data?.delta) updateLastAgentMessage(data.delta)
    }) as (...args: unknown[]) => void)
  }, [])

  // Tool-call event listener — shows live action log for agent tool use.
  useEffect(() => {
    return onEvent(IPC.AGENT_TOOL_CALL, ((evt: { id: string; tool: string; args: string; status: string; result?: string }) => {
      const icon = evt.status === 'error' ? '✗' : evt.status === 'done' ? '✓' : '⟳'
      addAgentMessage({
        id: 0,
        role: 'output',
        timestamp: new Date().toISOString(),
        text: `${icon} ${evt.tool}(${evt.args.substring(0, 120)})${evt.result ? ` → ${evt.result.substring(0, 80)}` : ''}`,
        isRunning: evt.status === 'running',
      })
    }) as (...args: unknown[]) => void)
  }, [])

  // Command output listener
  useEffect(() => {
    return onEvent(IPC.AGENT_EXEC_OUTPUT, (data: any) => {
      if (data.type === 'stdout' || data.type === 'stderr') {
        updateLastAgentMessage(data.text)
      } else if (data.type === 'info') {
        // Info messages from auto-fix (like "Wrote file: ...")
        addAgentMessage({
          id: 0, role: 'output', timestamp: new Date().toISOString(),
          text: data.text, isRunning: false,
        })
      } else if (data.type === 'exit') {
        const store = useLoggingStore.getState()
        const msgs = [...store.agentMessages]
        if (msgs.length > 0 && msgs[msgs.length - 1].role === 'output') {
          msgs[msgs.length - 1] = { ...msgs[msgs.length - 1], isRunning: false }
          useLoggingStore.setState({ agentMessages: msgs })
        }
        if (!store.fixInProgress) {
          const exitCode = data.code ?? 0
          addAgentMessage({
            id: 0, role: 'agent', timestamp: new Date().toISOString(),
            text: exitCode === 0 ? 'Command completed successfully.' : `Command failed with exit code ${exitCode}.`,
          })
        }
      }
    })
  }, [])

  // Auto-fix status listener
  useEffect(() => {
    return onEvent(IPC.AGENT_FIX_STATUS, (status: any) => {
      const phase = status.phase

      if (phase === 'awaiting-approval') {
        setPendingAction(status.action)
        return
      }

      setPendingAction(null)

      if (phase === 'done') {
        setFixInProgress(false)
        // Clear the error entries that were fixed
        clearFixedErrors()
        addAgentMessage({
          id: 0, role: 'fix-status', timestamp: new Date().toISOString(),
          text: status.message, fixPhase: 'done',
        })
        return
      }

      if (phase === 'failed' || phase === 'cancelled') {
        setFixInProgress(false)
        addAgentMessage({
          id: 0, role: 'fix-status', timestamp: new Date().toISOString(),
          text: status.message + (status.error ? `\n${status.error}` : ''), fixPhase: phase,
        })
        return
      }

      // Ongoing status
      addAgentMessage({
        id: 0, role: 'fix-status', timestamp: new Date().toISOString(),
        text: status.message, fixPhase: phase,
      })
    })
  }, [])

  // Pipeline progress tracking
  useEffect(() => {
    const unsub1 = onEvent(IPC.PIPELINE_PROGRESS, (data: any) => {
      if (data.type === 'progress') {
        if (data.etaMs != null) setEtaMs(data.etaMs)
        if (data.stage) setCurrentStage(data.stage)
        if (data.current && data.total) setCurrentProgress({ current: data.current, total: data.total })
      } else if (data.type === 'done' || data.type === 'error') {
        setEtaMs(null); setCurrentStage(null); setCurrentProgress(null)
      }
    })
    const unsub2 = onEvent(IPC.TEXT_PROGRESS, (data: any) => {
      if (data.type === 'progress') {
        if (data.etaMs != null) setEtaMs(data.etaMs)
        setCurrentStage(data.action === 'extract' ? 'vision' : 'enhance')
        if (data.current && data.total) setCurrentProgress({ current: data.current, total: data.total })
      } else if (data.type === 'done' || data.type === 'error') {
        setEtaMs(null); setCurrentStage(null); setCurrentProgress(null)
      }
    })
    return () => { unsub1(); unsub2() }
  }, [])

  // Poll active job status
  useEffect(() => {
    const poll = () => {
      invoke<{ pipelines: any[]; textJobs: any[] }>(IPC.PIPELINE_ACTIVE_STATUS).then((s) => {
        const total = s.pipelines.length + s.textJobs.length
        setActiveCount(total)
        const anyPaused = s.pipelines.some((p) => p.paused) || s.textJobs.some((t) => t.paused)
        setPaused(anyPaused && total > 0)
      }).catch(() => {})
    }
    poll()
    const timer = setInterval(poll, 3000)
    return () => clearInterval(timer)
  }, [])

  const handlePause = async () => {
    if (paused) { await invoke(IPC.PIPELINE_RESUME); setPaused(false) }
    else { await invoke(IPC.PIPELINE_PAUSE); setPaused(true) }
  }

  // ── Agent actions ──

  const getRecentErrors = useCallback((): string[] => {
    return entries
      .filter((e) => e.level === 'error')
      .slice(-20)
      .map((e) => `[${e.source}] ${e.message}`)
  }, [entries])

  const getRecentLogs = useCallback((): string[] => {
    return entries
      .slice(-30)
      .map((e) => `[${LEVEL_LABELS[e.level]}] [${e.source}] ${e.message}`)
  }, [entries])

  // Auto-fix: autonomous error fixing
  const handleAutoFix = async () => {
    const errors = getRecentErrors()
    if (errors.length === 0) {
      addAgentMessage({ id: 0, role: 'agent', timestamp: new Date().toISOString(), text: 'No errors found in the log. Everything looks clean!' })
      setPanelView('agent')
      return
    }

    setPanelView('agent')
    setFixInProgress(true)
    addAgentMessage({
      id: 0, role: 'fix-status', timestamp: new Date().toISOString(),
      text: `Starting auto-fix for ${errors.length} error(s) in **${permissionMode}** mode...`,
      fixPhase: 'diagnosing',
    })

    try {
      await invoke(IPC.AGENT_AUTO_FIX, errors, permissionMode)
    } catch (err: any) {
      setFixInProgress(false)
      addAgentMessage({ id: 0, role: 'agent', timestamp: new Date().toISOString(), text: `**Error**: ${err.message}` })
    }
  }

  const handleCancelFix = async () => {
    try {
      await invoke(IPC.AGENT_CANCEL_FIX)
    } catch { /* ok */ }
    setFixInProgress(false)
    setPendingAction(null)
    setAgentLoading(false)
  }

  const handleApproveAction = async (approved: boolean) => {
    setPendingAction(null)
    try {
      await invoke(IPC.AGENT_APPROVE_ACTION, approved)
    } catch { /* ok */ }
  }

  // Legacy diagnose (chat-based, for when user just wants advice)
  const handleFixErrors = async () => {
    const errors = getRecentErrors()
    if (errors.length === 0) {
      addAgentMessage({ id: 0, role: 'agent', timestamp: new Date().toISOString(), text: 'No errors found in the log. Everything looks clean!' })
      setPanelView('agent')
      return
    }

    setPanelView('agent')
    addAgentMessage({ id: 0, role: 'user', timestamp: new Date().toISOString(), text: `Diagnose ${errors.length} error(s) from the terminal log` })
    setAgentLoading(true)

    try {
      const response = await invoke<string>(IPC.AGENT_DIAGNOSE, errors)
      addAgentMessage({ id: 0, role: 'agent', timestamp: new Date().toISOString(), text: response })
    } catch (err: any) {
      addAgentMessage({ id: 0, role: 'agent', timestamp: new Date().toISOString(), text: `**Error**: ${err.message}` })
    } finally {
      setAgentLoading(false)
    }
  }

  const handleAgentSend = async () => {
    const text = agentInput.trim()
    if (!text || agentLoading) return
    setAgentInput('')

    // Direct command (! prefix)
    if (text.startsWith('!')) {
      const cmd = text.slice(1).trim()
      await runCommand(cmd)
      return
    }

    // Run suggested commands
    if (text.toLowerCase() === 'run') {
      const lastAgentMsg = [...agentMessages].reverse().find((m) => m.role === 'agent' && m.text.includes('`'))
      if (lastAgentMsg) {
        const commands = extractCommands(lastAgentMsg.text)
        if (commands.length > 0) {
          for (const cmd of commands) {
            await runCommand(cmd)
          }
          return
        }
      }
    }

    // Regular chat — create an empty agent placeholder so streaming deltas
    // have somewhere to append. updateLastAgentMessage() targets the last
    // message in the list, which is this placeholder.
    addAgentMessage({ id: 0, role: 'user', timestamp: new Date().toISOString(), text })
    addAgentMessage({ id: 0, role: 'agent', timestamp: new Date().toISOString(), text: '' })
    setAgentLoading(true)

    try {
      const recentLogs = getRecentLogs()
      const response = await invoke<string>(IPC.AGENT_CHAT, text, recentLogs)
      // Fallback: if streaming didn't fire (e.g. older backend), fill in the final text
      const current = useLoggingStore.getState().agentMessages
      const last = current[current.length - 1]
      if (last && last.role === 'agent' && !last.text) {
        updateLastAgentMessage(response)
      }
    } catch (err: any) {
      updateLastAgentMessage(`**Error**: ${err.message}`)
    } finally {
      setAgentLoading(false)
    }
  }

  const runCommand = async (cmd: string) => {
    addAgentMessage({ id: 0, role: 'command', timestamp: new Date().toISOString(), text: cmd })
    addAgentMessage({ id: 0, role: 'output', timestamp: new Date().toISOString(), text: '', isRunning: true })

    try {
      await invoke(IPC.AGENT_EXEC, cmd)
    } catch (err: any) {
      addAgentMessage({ id: 0, role: 'agent', timestamp: new Date().toISOString(), text: `**Failed to run command**: ${err.message}` })
    }
  }

  function extractCommands(text: string): string[] {
    const commands: string[] = []
    const isRunnable = (cmd: string) =>
      /^(npm|pip|pip3|brew|ollama|python|python3|chmod|sudo|curl|mkdir|apt|yarn|pnpm|npx|node|ts-node|git|electron|bash|sh) /.test(cmd)

    // Fenced code blocks (```bash / ```sh / ```)
    const fenceMatches = text.matchAll(/```(?:bash|sh|shell|zsh)?\n([\s\S]*?)```/g)
    for (const m of fenceMatches) {
      for (const line of m[1].split('\n')) {
        const cmd = line.replace(/^\$\s*/, '').trim()
        if (cmd && isRunnable(cmd)) commands.push(cmd)
      }
    }

    // Inline backtick commands
    const inlineMatches = text.match(/`([^`\n]+)`/g)
    if (inlineMatches) {
      for (const m of inlineMatches) {
        const cmd = m.slice(1, -1).trim()
        if (isRunnable(cmd) && !commands.includes(cmd)) commands.push(cmd)
      }
    }

    return commands
  }

  const filtered = filter === 'all' ? entries : entries.filter((e) => e.level === filter)
  const errorCount = entries.filter((e) => e.level === 'error').length

  const formatTime = (ts: string) => {
    try {
      const d = new Date(ts)
      return d.toLocaleTimeString('en-US', { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' })
        + '.' + String(d.getMilliseconds()).padStart(3, '0')
    } catch { return '' }
  }

  const stageLabels: Record<string, string> = {
    ocr: 'OCR', text_cleanup: 'Cleanup', tts: 'TTS', sync_map: 'Sync',
    video: 'Video', vision: 'Vision Extract', enhance: 'Enhancing',
  }

  // Compact widths: the panel is hidden behind a toggle (nav drawer → "Logs & agent", or the
  // floating chip that only appears when there are errors / running jobs). When open it becomes
  // a bottom sheet above the bottom tab bar instead of eating vertical space in the layout.
  if (compact && !isOpen) {
    const busy = activeCount > 0 || !!currentStage || fixInProgress
    if (unreadErrors === 0 && !busy) return null
    return (
      <button
        onClick={togglePanel}
        className="mv-log-chip"
        title="Show logs & agent"
        aria-label={`Show logs${unreadErrors > 0 ? ` (${unreadErrors} new errors)` : ''}`}
      >
        {busy ? <Loader2 size={14} className="animate-spin" /> : <Terminal size={14} />}
        {unreadErrors > 0 && <span className="mv-log-chip-badge">{unreadErrors > 9 ? '9+' : unreadErrors}</span>}
      </button>
    )
  }

  return (
    <div
      className={compact ? 'mv-log-sheet flex flex-col' : 'flex flex-col'}
      data-compact={compact ? '' : undefined}
      style={{ borderTop: '1px solid var(--panel-border)', background: 'var(--base)' }}
    >
      {/* Toggle handle + active job bar */}
      <div className={`flex items-center py-1.5 ${compact ? 'px-3 gap-1 overflow-x-auto scrollbar-hide' : 'px-4'}`}>
        <button onClick={togglePanel} className="flex items-center gap-3 group">
          <div className="w-10 h-1 rounded-full transition-colors" style={{ background: 'var(--surface-3)' }} />
          {unreadErrors > 0 && (
            <span className="bg-[var(--error)] text-white text-[9px] w-4 h-4 rounded-full flex items-center justify-center font-bold">
              {unreadErrors > 9 ? '9+' : unreadErrors}
            </span>
          )}
          <span className="text-[10px] text-[var(--text-3)]">{entries.length}</span>
        </button>

        <div className="flex-1" />

        {/* Active job info + ETA */}
        {(activeCount > 0 || currentStage) && (
          <div className="flex items-center gap-2 mr-3">
            <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-full text-[10px]"
              style={{ background: paused ? 'rgba(251,191,36,0.1)' : 'rgba(200, 80, 26,0.1)' }}>
              <div className={`w-1.5 h-1.5 rounded-full ${paused ? 'bg-amber-400' : 'bg-red-500 animate-pulse'}`} />
              <span className={paused ? 'text-amber-400' : 'text-[var(--success)]'}>
                {paused ? 'Paused' : currentStage ? stageLabels[currentStage] || currentStage : `${activeCount} active`}
              </span>
              {currentProgress && !paused && (
                <span className="text-[var(--text-3)]">{currentProgress.current}/{currentProgress.total}</span>
              )}
            </div>
            {etaMs != null && etaMs > 0 && !paused && (
              <div className="flex items-center gap-1 text-[10px] text-[var(--text-3)]">
                <Clock size={10} /> <span>{formatEta(etaMs)}</span>
              </div>
            )}
          </div>
        )}

        {/* Auto-Fix button (with fix-in-progress indicator) */}
        {errorCount > 0 && !fixInProgress && (
          <button
            onClick={handleAutoFix}
            className="flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-[11px] font-medium text-[var(--error)] hover:brightness-125 transition-all mr-2"
            style={{ background: 'rgba(239,68,68,0.08)', border: '1px solid rgba(239,68,68,0.15)' }}
            title="Auto-fix errors (AI agent will diagnose, fix, and verify)"
          >
            <Zap size={11} /> Fix {errorCount} Error{errorCount > 1 ? 's' : ''}
          </button>
        )}

        {fixInProgress && (
          <div className="flex items-center gap-2 mr-2">
            <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-[11px] font-medium text-emerald-400"
              style={{ background: 'rgba(200, 80, 26,0.1)', border: '1px solid rgba(200, 80, 26,0.2)' }}>
              <Loader2 size={11} className="animate-spin" /> Fixing...
            </div>
            <button
              onClick={handleCancelFix}
              className="flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] text-[var(--text-3)] hover:text-emerald-400 transition-all"
              style={{ background: 'var(--surface-2)' }}
            >
              <Square size={10} /> Cancel
            </button>
          </div>
        )}

        {/* Pause/Resume */}
        {activeCount > 0 && (
          <button
            onClick={handlePause}
            className={`flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-[11px] font-medium transition-all ${
              paused ? 'text-emerald-400 hover:text-emerald-300' : 'text-amber-400 hover:text-amber-300'
            }`}
            style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
          >
            {paused ? <Play size={11} /> : <Pause size={11} />}
            {paused ? 'Resume' : 'Pause'}
          </button>
        )}
      </div>

      {/* Panel content */}
      {isOpen && (
        <div className="flex flex-col" style={{ height: compact ? 'min(55vh, 440px)' : '280px' }}>
          {/* Toolbar with view tabs */}
          <div className={`flex items-center gap-1 py-1.5 ${compact ? 'px-3 overflow-x-auto scrollbar-hide' : 'px-4'}`}
            style={{ borderTop: '1px solid var(--panel-border)', background: 'var(--surface-1)' }}>

            {/* View toggle: Logs / Agent */}
            <div className="flex rounded-lg p-0.5" style={{ background: 'var(--surface-2)' }}>
              <button
                onClick={() => setPanelView('logs')}
                className={`flex items-center gap-1 text-[11px] px-2.5 py-1 rounded-md transition-all ${
                  panelView === 'logs' ? 'text-white' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
                }`}
                style={panelView === 'logs' ? { background: 'var(--surface-4)' } : {}}
              >
                <Terminal size={11} /> Logs
              </button>
              <button
                onClick={() => setPanelView('agent')}
                className={`flex items-center gap-1 text-[11px] px-2.5 py-1 rounded-md transition-all ${
                  panelView === 'agent' ? 'text-white' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
                }`}
                style={panelView === 'agent' ? { background: 'var(--surface-4)' } : {}}
              >
                <Bot size={11} /> Agent
              </button>
            </div>

            {/* Log filter buttons (only in logs view) */}
            {panelView === 'logs' && (
              <div className="flex rounded-lg p-0.5 ml-2" style={{ background: 'var(--surface-2)' }}>
                {(['all', 'error', 'warn', 'info'] as const).map((f) => (
                  <button
                    key={f}
                    onClick={() => setFilter(f)}
                    className={`text-[11px] px-2.5 py-1 rounded-md transition-all duration-150 ${
                      filter === f ? 'text-white' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
                    }`}
                    style={filter === f ? { background: 'var(--surface-4)' } : {}}
                  >
                    {f === 'all' ? 'All' : f.charAt(0).toUpperCase() + f.slice(1)}
                  </button>
                ))}
              </div>
            )}

            {/* Permission mode selector (agent view) */}
            {panelView === 'agent' && (
              <div className="ml-2">
                <PermissionModeSelector />
              </div>
            )}

            <div className="flex-1" />

            {panelView === 'logs' && (
              <>
                <button
                  onClick={() => setAutoScroll((v) => !v)}
                  className={`p-1 rounded transition-colors ${autoScroll ? 'text-emerald-400' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'}`}
                  title={autoScroll ? 'Auto-scroll on' : 'Auto-scroll off'}
                >
                  <ArrowDownToLine size={12} />
                </button>
                <button
                  onClick={() => {
                    const all = filtered.map((e) => `[${formatTime(e.timestamp)}] [${LEVEL_LABELS[e.level]}] [${e.source}] ${e.message}`).join('\n')
                    navigator.clipboard.writeText(all).catch(() => {})
                  }}
                  className="text-[var(--text-3)] hover:text-[var(--text-2)] p-1 rounded transition-colors ml-1"
                  title="Copy all visible logs"
                >
                  <Copy size={12} />
                </button>
                <button onClick={clearLogs} className="text-[var(--text-3)] hover:text-[var(--text-2)] p-1 rounded transition-colors ml-1" title="Clear logs">
                  <Trash2 size={12} />
                </button>
              </>
            )}
            {panelView === 'agent' && (
              <button onClick={clearAgentChat} className="text-[var(--text-3)] hover:text-[var(--text-2)] p-1 rounded transition-colors">
                <Trash2 size={12} />
              </button>
            )}
          </div>

          {/* ── Logs view ── */}
          {panelView === 'logs' && (
            <div ref={scrollRef}
              className="flex-1 overflow-y-auto overflow-x-hidden font-mono text-[11px] leading-6 px-4 py-1 select-text cursor-text">
              {filtered.length === 0 && (
                <div className="text-[var(--text-3)] py-4 text-center select-none text-[11px]">No log entries</div>
              )}
              {filtered.map((entry) => (
                <div key={entry.id} className="group relative flex gap-2 rounded px-1 -mx-1 hover:bg-[var(--surface-3)]">
                  <span className="text-[var(--text-3)] shrink-0 select-none">{formatTime(entry.timestamp)}</span>
                  <span className={`shrink-0 w-7 select-none font-semibold ${LEVEL_COLORS[entry.level]}`}>
                    {LEVEL_LABELS[entry.level]}
                  </span>
                  <span className={`shrink-0 select-none font-medium ${sourceTagClass(entry.source)}`}>
                    [{entry.source}]
                  </span>
                  <span className={`break-all ${
                    entry.level === 'error' ? 'text-emerald-300' :
                    entry.level === 'warn' ? 'text-amber-300/80' : 'text-[var(--text-2)]'
                  }`}>
                    {entry.message}
                  </span>
                  <CopyButton
                    text={`[${formatTime(entry.timestamp)}] [${LEVEL_LABELS[entry.level]}] [${entry.source}] ${entry.message}`}
                    className="absolute right-1 top-0.5 opacity-0 group-hover:opacity-100 bg-[var(--surface-1)]"
                  />
                </div>
              ))}
            </div>
          )}

          {/* ── Agent view ── */}
          {panelView === 'agent' && (
            <div className="flex flex-col flex-1 min-h-0">
              {/* Chat messages */}
              <div ref={agentScrollRef} className="flex-1 overflow-y-auto px-4 py-2">
                {agentMessages.length === 0 && !agentLoading && !fixInProgress && (
                  <div className="flex flex-col items-center justify-center gap-2 py-6 text-center">
                    <Bot size={24} className="text-[var(--text-3)]" />
                    <p className="text-xs text-[var(--text-3)]">Terminal Agent</p>
                    <p className="text-[10px] text-[var(--text-3)] max-w-xs">
                      I can diagnose and <span className="text-emerald-400 font-semibold">automatically fix</span> errors.
                      Use the permission selector above to control how I execute fixes.
                    </p>
                    <div className="flex gap-2 mt-2">
                      <button
                        onClick={handleAutoFix}
                        className="flex items-center gap-1 px-2.5 py-1.5 rounded-lg text-[10px] text-emerald-400 transition-all hover:text-emerald-300"
                        style={{ background: 'rgba(200, 80, 26,0.1)', border: '1px solid rgba(200, 80, 26,0.15)' }}
                      >
                        <Zap size={10} /> Auto-Fix Errors
                      </button>
                      <button
                        onClick={handleFixErrors}
                        className="flex items-center gap-1 px-2.5 py-1.5 rounded-lg text-[10px] text-emerald-400 transition-all hover:text-emerald-300"
                        style={{ background: 'rgba(239,68,68,0.08)', border: '1px solid rgba(239,68,68,0.12)' }}
                      >
                        <TriangleAlert size={10} /> Diagnose Only
                      </button>
                      <button
                        onClick={() => { setAgentInput('!npm install'); inputRef.current?.focus() }}
                        className="flex items-center gap-1 px-2.5 py-1.5 rounded-lg text-[10px] text-emerald-400 transition-all hover:text-emerald-300"
                        style={{ background: 'rgba(200, 80, 26,0.08)', border: '1px solid rgba(200, 80, 26,0.12)' }}
                      >
                        <Terminal size={10} /> npm install
                      </button>
                    </div>
                  </div>
                )}

                {agentMessages.map((msg) => (
                  <AgentMessageBubble key={msg.id} msg={msg} />
                ))}

                {/* Pending action approval card */}
                {pendingAction && (
                  <ActionApprovalCard
                    action={pendingAction}
                    onApprove={() => handleApproveAction(true)}
                    onDecline={() => handleApproveAction(false)}
                  />
                )}

                {agentLoading && (
                  <div className="flex items-center gap-2 text-xs text-[var(--text-3)] mb-2">
                    <Loader2 size={12} className="animate-spin text-emerald-400" />
                    <span>Thinking...</span>
                  </div>
                )}
              </div>

              {/* Input bar */}
              <div className="px-3 py-2 flex items-center gap-2"
                style={{ borderTop: '1px solid var(--panel-border)' }}>
                <input
                  ref={inputRef}
                  type="text"
                  value={agentInput}
                  onChange={(e) => setAgentInput(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && handleAgentSend()}
                  placeholder={fixInProgress ? 'Fix in progress...' : 'Ask agent or prefix ! for commands...'}
                  className="flex-1 h-8 px-3 rounded-lg text-xs text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
                  style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
                  disabled={agentLoading || fixInProgress}
                />
                {agentLoading ? (
                  <button
                    onClick={handleCancelFix}
                    className="w-8 h-8 rounded-lg flex items-center justify-center transition-all hover:bg-red-500/20"
                    style={{ background: 'rgba(239, 68, 68, 0.15)' }}
                    title="Stop agent generation"
                  >
                    <Square size={11} className="text-red-400 fill-red-400" />
                  </button>
                ) : (
                  <button
                    onClick={handleAgentSend}
                    disabled={fixInProgress || !agentInput.trim()}
                    className="w-8 h-8 rounded-lg flex items-center justify-center transition-all disabled:opacity-30"
                    style={{ background: 'rgba(200, 80, 26, 0.15)' }}
                  >
                    <Send size={13} className="text-emerald-400" />
                  </button>
                )}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
