import { useEffect, useRef, useState } from 'react'
import { motion, AnimatePresence } from 'framer-motion'
import { Sparkles, Plus, Trash2, X, Loader2, ChevronDown, Globe, Wand2 } from 'lucide-react'
import { v4 as uuidv4 } from 'uuid'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import type { CustomSkill, SkillBuildProgress } from '../../shared/assistant-types'
import { toast } from './Toaster'

// One-click "install" presets — building these researches the topic live, the
// same way Claude's skill marketplace gives you ready-made capabilities.
const SUGGESTED_SKILLS = [
  'Resume & cover letter writing',
  'YouTube SEO & thumbnails',
  'Personal finance budgeting',
  'Anime recommendations expert',
  'Python debugging',
  'Meal planning & nutrition',
]

interface SkillsPanelProps {
  model: string
  onClose: () => void
  /** Insert a prompt that uses the skill into the composer. */
  onUse?: (skill: CustomSkill) => void
}

export function SkillsPanel({ model, onClose, onUse }: SkillsPanelProps) {
  const [skills, setSkills] = useState<CustomSkill[]>([])
  const [loading, setLoading] = useState(true)
  const [expanded, setExpanded] = useState<string | null>(null)
  const [topic, setTopic] = useState('')
  const [building, setBuilding] = useState<string | null>(null)
  const [progress, setProgress] = useState<SkillBuildProgress | null>(null)
  const buildReqRef = useRef<string | null>(null)

  const refresh = async () => {
    try { setSkills(await invoke<CustomSkill[]>(IPC.SKILL_LIST)) } catch { /* ignore */ }
    setLoading(false)
  }

  useEffect(() => { void refresh() }, [])

  // Live build progress streams over MEDIA_CHAT_STATUS tagged with our requestId.
  useEffect(() => onEvent(IPC.MEDIA_CHAT_STATUS, (data: any) => {
    if (!data?.skillBuild || data.requestId !== buildReqRef.current) return
    setProgress(data.skillBuild as SkillBuildProgress)
  }), [])

  const build = async (t: string) => {
    const clean = t.trim()
    if (!clean || building) return
    const requestId = uuidv4()
    buildReqRef.current = requestId
    setBuilding(clean)
    setProgress({ phase: 'research', label: 'Starting…', status: 'running', percent: 2 })
    try {
      await invoke<CustomSkill>(IPC.SKILL_BUILD, { topic: clean, model, requestId })
      toast.success('Skill built', clean)
      setTopic('')
      await refresh()
    } catch (error: any) {
      toast.error('Could not build skill', error?.message || 'Try again')
    } finally {
      setBuilding(null)
      setProgress(null)
      buildReqRef.current = null
    }
  }

  const remove = async (id: string) => {
    await invoke(IPC.SKILL_DELETE, id)
    await refresh()
  }

  return (
    <div className="rounded-2xl overflow-hidden shadow-2xl" style={{ background: 'var(--surface-1)', border: '1px solid var(--panel-border-strong)' }}>
      <div className="flex items-center gap-2 px-4 py-3 border-b" style={{ borderColor: 'var(--panel-border)' }}>
        <Sparkles size={15} style={{ color: 'var(--accent-hover)' }} />
        <span className="text-sm font-semibold" style={{ color: 'var(--text-1)' }}>Skills</span>
        <span className="text-[11px]" style={{ color: 'var(--text-3)' }}>{skills.length} built</span>
        <button onClick={onClose} className="ml-auto p-1 rounded-lg hover:bg-[var(--surface-3)]" style={{ color: 'var(--text-3)' }}>
          <X size={15} />
        </button>
      </div>

      {/* Create new skill */}
      <div className="px-4 py-3 border-b" style={{ borderColor: 'var(--panel-border)' }}>
        <div className="flex items-center gap-2">
          <div className="relative flex-1">
            <Wand2 size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-3)' }} />
            <input
              value={topic}
              onChange={(e) => setTopic(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') void build(topic) }}
              placeholder="Build a skill — e.g. 'video editing in DaVinci Resolve'"
              className="allow-select w-full h-9 pl-8 pr-3 rounded-lg text-xs outline-none"
              style={{ background: 'var(--panel-border)', border: '1px solid var(--panel-border)', color: 'var(--text-1)' }}
            />
          </div>
          <button
            onClick={() => void build(topic)}
            disabled={!topic.trim() || !!building}
            className="h-9 px-3 rounded-lg text-xs font-semibold text-white flex items-center gap-1.5 disabled:opacity-40"
            style={{ background: 'var(--accent-gradient)' }}
          >
            {building === topic.trim() ? <Loader2 size={13} className="animate-spin" /> : <Plus size={13} />}
            Build
          </button>
        </div>
        <p className="text-[10px] mt-1.5 flex items-center gap-1" style={{ color: 'var(--text-3)' }}>
          <Globe size={9} /> The assistant researches the topic online, then saves a reusable skill.
        </p>

        {/* Live build progress */}
        <AnimatePresence>
          {building && progress && (
            <motion.div
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              className="overflow-hidden mt-2.5 rounded-lg p-2.5"
              style={{ background: 'rgba(0,0,0,0.2)', border: '1px solid var(--panel-border)' }}
            >
              <div className="flex items-center gap-2 mb-1.5">
                <Loader2 size={12} className="animate-spin shrink-0" style={{ color: 'var(--accent-hover)' }} />
                <span className="text-[11px] font-medium truncate" style={{ color: 'var(--text-1)' }}>
                  Building “{building}” — {progress.label}
                </span>
                <span className="ml-auto text-[10px] tabular-nums shrink-0" style={{ color: 'var(--text-3)' }}>
                  {Math.round(progress.percent ?? 0)}%
                </span>
              </div>
              <div className="h-1.5 rounded-full overflow-hidden" style={{ background: 'var(--panel-border)' }}>
                <motion.div
                  className="h-full rounded-full"
                  style={{ background: 'var(--accent-gradient)' }}
                  animate={{ width: `${Math.max(2, Math.min(100, progress.percent ?? 0))}%` }}
                  transition={{ duration: 0.4 }}
                />
              </div>
              {progress.detail && (
                <p className="text-[10px] mt-1.5 truncate" style={{ color: 'var(--text-3)' }}>{progress.detail}</p>
              )}
              {progress.params && Object.keys(progress.params).length > 0 && (
                <div className="flex flex-wrap gap-1 mt-1.5">
                  {Object.entries(progress.params).map(([k, v]) => (
                    <span key={k} className="px-1.5 py-0.5 rounded text-[9px] tabular-nums" style={{ background: 'var(--panel-border)', color: 'var(--text-3)' }}>
                      {k}: {String(v)}
                    </span>
                  ))}
                </div>
              )}
            </motion.div>
          )}
        </AnimatePresence>
        {/* Suggested / installable */}
        <div className="flex flex-wrap gap-1.5 mt-2.5">
          {SUGGESTED_SKILLS.filter((s) => !skills.some((k) => k.name.toLowerCase() === s.toLowerCase())).map((s) => (
            <button
              key={s}
              onClick={() => void build(s)}
              disabled={!!building}
              className="px-2 py-1 rounded-full text-[10px] disabled:opacity-40 flex items-center gap-1"
              style={{ background: 'var(--panel-border)', border: '1px solid var(--panel-border)', color: 'var(--text-2)' }}
            >
              {building === s ? <Loader2 size={9} className="animate-spin" /> : <Plus size={9} />} {s}
            </button>
          ))}
        </div>
      </div>

      {/* Skills list */}
      <div className="max-h-[300px] overflow-y-auto p-2">
        {loading ? (
          <div className="flex items-center justify-center py-8"><Loader2 size={18} className="animate-spin" style={{ color: 'var(--text-3)' }} /></div>
        ) : skills.length === 0 ? (
          <p className="text-center text-xs py-8" style={{ color: 'var(--text-3)' }}>No skills yet. Build one above — or try a suggestion.</p>
        ) : skills.map((skill) => {
          const open = expanded === skill.id
          return (
            <div key={skill.id} className="rounded-xl mb-1.5" style={{ background: 'var(--panel-border)', border: '1px solid var(--panel-border)' }}>
              <div className="flex items-center gap-2.5 px-3 py-2.5">
                <span className="text-base shrink-0">{skill.icon}</span>
                <button onClick={() => setExpanded(open ? null : skill.id)} className="min-w-0 flex-1 text-left">
                  <div className="text-xs font-medium truncate" style={{ color: 'var(--text-1)' }}>{skill.name}</div>
                  <div className="text-[10px] truncate" style={{ color: 'var(--text-3)' }}>{skill.description}</div>
                </button>
                {skill.usageCount > 0 && <span className="text-[9px] shrink-0" style={{ color: 'var(--text-3)' }}>used {skill.usageCount}×</span>}
                {onUse && (
                  <button onClick={() => onUse(skill)} className="text-[10px] px-2 py-1 rounded-md shrink-0" style={{ color: 'var(--accent-hover)', background: 'var(--accent-gradient-dim)' }}>Use</button>
                )}
                <button onClick={() => void remove(skill.id)} className="p-1 rounded-md shrink-0 hover:bg-[var(--surface-3)]" style={{ color: 'var(--text-3)' }} title="Delete skill">
                  <Trash2 size={12} />
                </button>
                <button onClick={() => setExpanded(open ? null : skill.id)} className="shrink-0" style={{ color: 'var(--text-3)' }}>
                  <ChevronDown size={13} className={`transition-transform ${open ? 'rotate-180' : ''}`} />
                </button>
              </div>
              <AnimatePresence initial={false}>
                {open && (
                  <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} className="overflow-hidden">
                    <div className="allow-select px-3 pb-3 text-[11px] space-y-2" style={{ color: 'var(--text-2)' }}>
                      {skill.triggers.length > 0 && (
                        <div className="flex flex-wrap gap-1">
                          {skill.triggers.slice(0, 12).map((t) => (
                            <span key={t} className="px-1.5 py-0.5 rounded text-[9px]" style={{ background: 'var(--panel-border)', color: 'var(--text-3)' }}>{t}</span>
                          ))}
                        </div>
                      )}
                      {skill.knowledge && (
                        <div className="whitespace-pre-wrap leading-relaxed max-h-40 overflow-y-auto rounded-lg p-2" style={{ background: 'rgba(0,0,0,0.18)' }}>
                          {skill.knowledge}
                        </div>
                      )}
                      {skill.sources.length > 0 && (
                        <div>
                          <p className="text-[9px] uppercase tracking-wide mb-1" style={{ color: 'var(--text-3)' }}>Sources</p>
                          {skill.sources.slice(0, 5).map((s, i) => (
                            <a key={i} href={s.url} target="_blank" rel="noreferrer" className="block truncate text-[10px]" style={{ color: 'var(--accent-hover)' }} onClick={(e) => e.stopPropagation()}>
                              {i + 1}. {s.title || s.url}
                            </a>
                          ))}
                        </div>
                      )}
                    </div>
                  </motion.div>
                )}
              </AnimatePresence>
            </div>
          )
        })}
      </div>
    </div>
  )
}
