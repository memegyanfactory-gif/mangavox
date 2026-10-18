import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { ArrowUp, ChevronDown, Gauge, Lock, LockOpen, ShieldCheck, Square } from 'lucide-react'
import type { AiAccessMode, AiEffort } from '../../../shared/ai-types'
import { useAssistantStore } from '../../stores/assistant.store'
import { useAiCatalog } from '../../assistant/catalog'
import { CLI_PROVIDERS, sendToAssistant, stopAssistant } from '../../assistant/runtime'
import { ModelPicker } from './ModelPicker'
import { MenuItem, Popover, ProviderBadge } from './ui'

const EFFORTS: { id: AiEffort; label: string; hint: string }[] = [
  { id: 'low', label: 'Low', hint: 'Fastest' },
  { id: 'medium', label: 'Medium', hint: 'Balanced' },
  { id: 'high', label: 'High', hint: 'Thinks longer' },
]

const ACCESS: { id: AiAccessMode; label: string; hint: string; icon: typeof Lock }[] = [
  { id: 'ask', label: 'Ask first', hint: 'Approve every action', icon: Lock },
  { id: 'auto', label: 'Auto', hint: 'Asks only for risky actions', icon: ShieldCheck },
  { id: 'full', label: 'Full access', hint: 'Never asks', icon: LockOpen },
]

export function Composer({ autoFocusKey }: { autoFocusKey?: string | null }) {
  const { providerId, model, effort, access, busyThreadId, activeId, setEffort, setAccess } = useAssistantStore()
  const providerLabel = useAiCatalog((s) => s.providers.find((p) => p.id === providerId)?.label)
  const [text, setText] = useState('')
  const [menu, setMenu] = useState<'model' | 'effort' | 'access' | null>(null)
  const ref = useRef<HTMLTextAreaElement>(null)
  const busy = Boolean(busyThreadId)
  const isCli = CLI_PROVIDERS.includes(providerId)

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, 200)}px`
  }, [text])

  useEffect(() => { ref.current?.focus() }, [autoFocusKey, activeId])

  // Ctrl+K opens the model picker while the dock is focused.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k' && ref.current?.closest('.mv-ai-dock')?.contains(document.activeElement)) {
        e.preventDefault()
        setMenu('model')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const submit = () => {
    if (busy || !text.trim()) return
    const value = text
    setText('')
    void sendToAssistant(value)
  }

  const AccessIcon = ACCESS.find((a) => a.id === access)?.icon ?? ShieldCheck
  const close = () => setMenu(null)

  return (
    <div className="mv-ai-composer">
      <textarea
        ref={ref}
        value={text}
        rows={1}
        placeholder={busy ? 'Working… press Esc to stop' : 'Ask anything, or tell me what to do in the app…'}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); submit() }
          if (e.key === 'Escape' && busy) { e.preventDefault(); stopAssistant() }
        }}
      />
      <div className="mv-ai-composer-bar">
        <div className="min-w-0">
          <button type="button" className="mv-ai-chip max-w-[190px]" onClick={() => setMenu(menu === 'model' ? null : 'model')} title={`${providerLabel ?? providerId} · ${model || 'no model'} (Ctrl+K)`}>
            <ProviderBadge id={providerId} size={16} />
            <span className="truncate">{model ? (model === 'default' ? `${providerLabel ?? ''} default` : model) : 'Choose model'}</span>
            <ChevronDown size={12} className="shrink-0 opacity-60" />
          </button>
          {/* Anchored to the whole composer so it never spills past the panel edge. */}
          <Popover open={menu === 'model'} onClose={close} className="bottom-[calc(100%+8px)] right-0">
            <ModelPicker onClose={close} />
          </Popover>
        </div>

        <div className="relative">
          <button type="button" className="mv-ai-chip" onClick={() => setMenu(menu === 'effort' ? null : 'effort')} title="Reasoning effort">
            <Gauge size={13} />
            <span className="hidden min-[420px]:inline">{EFFORTS.find((e) => e.id === effort)?.label}</span>
          </button>
          <Popover open={menu === 'effort'} onClose={close} className="bottom-[calc(100%+8px)] w-52 p-1">
            {EFFORTS.map((e) => <MenuItem key={e.id} active={e.id === effort} hint={e.hint} onClick={() => { setEffort(e.id); close() }}>{e.label}</MenuItem>)}
          </Popover>
        </div>

        <div className="relative">
          <button
            type="button"
            className={`mv-ai-chip ${access === 'full' ? 'is-warn' : ''}`}
            onClick={() => setMenu(menu === 'access' ? null : 'access')}
            title={isCli ? 'Access for the CLI agent' : 'What the assistant may do without asking'}
          >
            <AccessIcon size={13} />
            <span className="hidden min-[420px]:inline">{ACCESS.find((a) => a.id === access)?.label}</span>
          </button>
          <Popover open={menu === 'access'} onClose={close} className="bottom-[calc(100%+8px)] w-60 p-1">
            {ACCESS.map((a) => <MenuItem key={a.id} active={a.id === access} hint={a.hint} onClick={() => { setAccess(a.id); close() }}>{a.label}</MenuItem>)}
          </Popover>
        </div>

        <div className="flex-1" />
        {busy ? (
          <button type="button" className="mv-ai-send is-stop" onClick={stopAssistant} title="Stop (Esc)" aria-label="Stop">
            <Square size={12} fill="currentColor" />
          </button>
        ) : (
          <button type="button" className="mv-ai-send" onClick={submit} disabled={!text.trim()} title="Send (Enter)" aria-label="Send">
            <ArrowUp size={15} />
          </button>
        )}
      </div>
    </div>
  )
}
