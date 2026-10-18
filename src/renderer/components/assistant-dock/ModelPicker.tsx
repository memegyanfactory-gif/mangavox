// Provider rail + searchable model list (reference: screenshot 3 in the plan).
import { useEffect, useMemo, useRef, useState } from 'react'
import { Loader2, Plus, RefreshCw, Search, Settings2, Star } from 'lucide-react'
import type { AiModel, AiProviderId, AiProviderStatus } from '../../../shared/ai-types'
import { useAiCatalog, modelKey } from '../../assistant/catalog'
import { useAssistantStore } from '../../stores/assistant.store'
import { useUiStore } from '../../stores/ui.store'
import { ProviderBadge } from './ui'

const STATE_LABEL: Record<AiProviderStatus['state'], string> = {
  ready: 'Ready',
  'needs-key': 'Needs key',
  'not-installed': 'Not installed',
  offline: 'Not running',
  disabled: 'Off',
  error: 'Error',
}

export function ModelPicker({ onClose }: { onClose: () => void }) {
  const { providers, models, loadingModels, loadModels, refresh } = useAiCatalog()
  const { providerId, model, favorites, setModel, toggleFavorite } = useAssistantStore()
  const [tab, setTab] = useState<AiProviderId | 'favorites'>(providerId)
  const [query, setQuery] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => { inputRef.current?.focus() }, [])
  useEffect(() => { if (tab !== 'favorites') void loadModels(tab).catch(() => {}) }, [tab, loadModels])

  const ordered = useMemo(
    () => [...providers].filter((p) => p.state !== 'disabled').sort((a, b) => Number(b.state === 'ready') - Number(a.state === 'ready')),
    [providers],
  )
  const current = providers.find((p) => p.id === tab)

  const list: AiModel[] = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (tab === 'favorites') {
      return favorites.map((f) => {
        const [pid, ...rest] = f.split('::')
        const id = rest.join('::')
        return models[pid as AiProviderId]?.find((m) => m.id === id) ?? { id, label: id, providerId: pid as AiProviderId }
      }).filter((m) => !q || m.label.toLowerCase().includes(q))
    }
    return (models[tab] ?? []).filter((m) => !q || m.id.toLowerCase().includes(q) || m.label.toLowerCase().includes(q))
  }, [tab, models, favorites, query])

  const choose = (pid: AiProviderId, id: string) => {
    setModel(pid, id)
    onClose()
  }

  const canUseCustom = tab !== 'favorites' && query.trim() && !list.some((m) => m.id === query.trim())

  return (
    <div className="mv-ai-picker">
      <div className="mv-ai-picker-rail">
        <button type="button" title="Favourites" className={`mv-ai-rail-btn ${tab === 'favorites' ? 'is-active' : ''}`} onClick={() => setTab('favorites')}>
          <Star size={14} />
        </button>
        {ordered.map((p) => (
          <button
            key={p.id}
            type="button"
            title={`${p.label} · ${STATE_LABEL[p.state]}`}
            className={`mv-ai-rail-btn ${tab === p.id ? 'is-active' : ''}`}
            onClick={() => setTab(p.id)}
          >
            <ProviderBadge id={p.id} size={22} dim={p.state !== 'ready'} />
            {p.state === 'ready' && <span className="mv-ai-rail-dot" />}
          </button>
        ))}
      </div>

      <div className="flex-1 min-w-0 flex flex-col">
        <div className="mv-ai-picker-search">
          <Search size={13} className="mv-ai-muted" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={tab === 'favorites' ? 'Search favourites…' : `Search ${current?.label ?? ''} models…`}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                if (list[0]) choose(list[0].providerId, list[0].id)
                else if (canUseCustom) choose(tab as AiProviderId, query.trim())
              }
            }}
          />
          {tab !== 'favorites' && (
            <button type="button" title="Reload models" className="mv-ai-icon-btn" onClick={() => { void refresh(true); void loadModels(tab, true).catch(() => {}) }}>
              {loadingModels[tab] ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
            </button>
          )}
        </div>

        {current && tab !== 'favorites' && (
          <div className="mv-ai-picker-head">
            <span className="font-medium" style={{ color: 'var(--text-1)' }}>{current.label}</span>
            <span className={`mv-ai-state mv-ai-state-${current.state}`}>{STATE_LABEL[current.state]}</span>
            {current.version && <span className="mv-ai-muted truncate">{current.version}</span>}
          </div>
        )}

        <div className="mv-ai-picker-list">
          {current && current.state !== 'ready' && tab !== 'favorites' && (
            <p className="mv-ai-picker-note">
              {current.detail || STATE_LABEL[current.state]}. {current.installHint && current.state !== 'needs-key' ? current.installHint : ''}
            </p>
          )}
          {list.map((m) => {
            const key = modelKey(m.providerId, m.id)
            const fav = favorites.includes(key)
            const selected = m.providerId === providerId && m.id === model
            return (
              <div key={key} className={`mv-ai-model-row ${selected ? 'is-active' : ''}`}>
                <button type="button" className="flex-1 min-w-0 flex items-center gap-2 text-left" onClick={() => choose(m.providerId, m.id)}>
                  {tab === 'favorites' && <ProviderBadge id={m.providerId} size={18} />}
                  <span className="truncate">{m.label}</span>
                  {m.label !== m.id && <span className="mv-ai-muted truncate text-[11px]">{m.id}</span>}
                  {m.tags?.filter(Boolean).slice(0, 2).map((t) => <span key={t} className="mv-ai-tag">{t}</span>)}
                </button>
                <button type="button" title={fav ? 'Remove favourite' : 'Add favourite'} className={`mv-ai-star ${fav ? 'is-on' : ''}`} onClick={() => toggleFavorite(key)}>
                  <Star size={12} fill={fav ? 'currentColor' : 'none'} />
                </button>
              </div>
            )
          })}
          {!list.length && !loadingModels[tab as AiProviderId] && !canUseCustom && (
            <p className="mv-ai-picker-note">
              {tab === 'favorites' ? 'Star models to keep them here.' : current?.state === 'needs-key' ? 'Add an API key to load models.' : 'No models found.'}
            </p>
          )}
          {canUseCustom && (
            <button type="button" className="mv-ai-model-row" onClick={() => choose(tab as AiProviderId, query.trim())}>
              <Plus size={13} /> <span className="truncate">Use “{query.trim()}”</span>
            </button>
          )}
        </div>

        <button
          type="button"
          className="mv-ai-picker-foot"
          onClick={() => { useUiStore.getState().navigate('settings', { tab: 'providers' }); onClose() }}
        >
          <Settings2 size={13} /> Manage providers & keys
        </button>
      </div>
    </div>
  )
}
