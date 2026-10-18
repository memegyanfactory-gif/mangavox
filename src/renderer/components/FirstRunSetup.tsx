import { useEffect, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import {
  BookOpen, Check, ChevronLeft, ChevronRight, Clapperboard, Cpu,
  Film, Loader2, MonitorPlay, Sparkles, WandSparkles,
} from 'lucide-react'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { useAppModeStore, type AppMode } from '../stores/app-mode.store'
import logoUrl from '../assets/logo.png'

interface StudioStatus {
  online: boolean
  modelCount: number
}

export function FirstRunSetup({ onComplete }: { onComplete: () => void }) {
  const setAppMode = useAppModeStore((state) => state.setMode)
  const [step, setStep] = useState<'choose' | 'studio'>('choose')
  const [selected, setSelected] = useState<AppMode>('entertainment')
  const [studioStatus, setStudioStatus] = useState<StudioStatus | null>(null)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (step !== 'studio') return
    Promise.all([
      invoke<{ online: boolean }>(IPC.OLLAMA_STATUS).catch(() => ({ online: false })),
      invoke<{ vision: unknown[]; text: unknown[] }>(IPC.OLLAMA_LIST_MODELS)
        .catch(() => ({ vision: [], text: [] })),
    ]).then(([status, models]) => {
      setStudioStatus({
        online: status.online,
        modelCount: models.vision.length + models.text.length,
      })
    })
  }, [step])

  const finish = async (mode: AppMode) => {
    setSaving(true)
    try {
      await setAppMode(mode)
      await invoke(IPC.CONFIG_SET, 'app.firstRunComplete', true)
      onComplete()
    } finally {
      setSaving(false)
    }
  }

  const continueChoice = () => {
    if (selected === 'studio') setStep('studio')
    else finish('entertainment')
  }

  return (
    <div className="fixed inset-0 z-[90] flex items-center justify-center px-6 overflow-hidden" style={{ background: 'var(--base)' }}>
      <div className="mv-app-bg" aria-hidden />
      <div className="absolute inset-0 opacity-40" style={{ background: 'radial-gradient(circle at 50% 25%, var(--theme-bg-a), transparent 38%)' }} />

      <AnimatePresence mode="wait">
        {step === 'choose' ? (
          <motion.div
            key="choose"
            className="relative z-10 w-full max-w-3xl"
            initial={{ opacity: 0, y: 18 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, x: -24 }}
          >
            <div className="text-center mb-7">
              <img src={logoUrl} alt="MangaVox" className="w-14 h-14 rounded-2xl object-contain mx-auto mb-4" />
              <h1 className="text-2xl font-bold" style={{ color: 'var(--text-1)' }}>What do you want MangaVox to be?</h1>
              <p className="text-sm mt-2" style={{ color: 'var(--text-3)' }}>
                Choose a focused entertainment app or unlock the complete manga creation workspace.
              </p>
            </div>

            <div className="grid md:grid-cols-2 gap-4">
              <ModeCard
                active={selected === 'entertainment'}
                icon={<Clapperboard size={24} />}
                title="MangaVox"
                subtitle="Read and watch"
                description="A clean app for manga, anime, and movies. Assistant, AI setup, and production tools stay inside Manga Studio."
                features={[
                  <><BookOpen size={13} /> Manga library and reader</>,
                  <><MonitorPlay size={13} /> Anime and movie streaming</>,
                  <><Film size={13} /> History, favorites, downloads, and subtitles</>,
                ]}
                onClick={() => setSelected('entertainment')}
              />
              <ModeCard
                active={selected === 'studio'}
                icon={<WandSparkles size={24} />}
                title="Manga Studio"
                subtitle="Read, create, and publish"
                description="Everything in MangaVox plus the personal Assistant, OCR, script generation, voices, video, and automated chapter pipelines."
                features={[
                  <><Sparkles size={13} /> Text extraction and AI narration</>,
                  <><Cpu size={13} /> Private media-aware Assistant</>,
                  <><Film size={13} /> Audio, video, and pipeline tools</>,
                ]}
                onClick={() => setSelected('studio')}
              />
            </div>

            <div className="mt-6 flex justify-center">
              <button
                type="button"
                onClick={continueChoice}
                disabled={saving}
                className="h-11 px-6 rounded-xl flex items-center gap-2 text-sm font-semibold text-white disabled:opacity-50"
                style={{ background: 'var(--accent-gradient)', boxShadow: '0 10px 30px var(--accent-glow-soft)' }}
              >
                {saving ? <Loader2 size={15} className="animate-spin" /> : null}
                Continue with {selected === 'studio' ? 'Manga Studio' : 'MangaVox'}
                {!saving && <ChevronRight size={15} />}
              </button>
            </div>
          </motion.div>
        ) : (
          <motion.div
            key="studio"
            className="relative z-10 w-full max-w-lg mv-card p-6"
            initial={{ opacity: 0, x: 28 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, y: -16 }}
          >
            <div className="w-11 h-11 rounded-xl flex items-center justify-center mb-4" style={{ background: 'var(--accent-gradient-dim)', color: 'var(--accent-hover)' }}>
              <Sparkles size={21} />
            </div>
            <h2 className="text-xl font-bold" style={{ color: 'var(--text-1)' }}>Manga Studio is ready</h2>
            <p className="text-sm mt-2 leading-relaxed" style={{ color: 'var(--text-3)' }}>
              Studio unlocks Text, Audio, Video, Dashboard, and Pipeline tools. Cloud AI can be configured later. Ollama is optional and only needed for private local AI processing.
            </p>

            <div className="mt-5 rounded-xl p-4 flex items-center gap-3" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
              {!studioStatus ? (
                <Loader2 size={18} className="animate-spin" style={{ color: 'var(--accent-hover)' }} />
              ) : (
                <span
                  className="w-8 h-8 rounded-full flex items-center justify-center"
                  style={{ background: studioStatus.online ? 'rgba(var(--success-rgb),.12)' : 'var(--surface-4)', color: studioStatus.online ? 'var(--success)' : 'var(--text-3)' }}
                >
                  {studioStatus.online ? <Check size={16} /> : <Cpu size={16} />}
                </span>
              )}
              <div>
                <div className="text-xs font-semibold" style={{ color: 'var(--text-2)' }}>
                  {!studioStatus ? 'Checking local AI...' : studioStatus.online ? 'Ollama detected' : 'Local AI not configured'}
                </div>
                <div className="text-[11px] mt-0.5" style={{ color: 'var(--text-3)' }}>
                  {studioStatus?.online
                    ? `${studioStatus.modelCount} local model${studioStatus.modelCount === 1 ? '' : 's'} found`
                    : 'That is okay. Studio opens normally and you can configure AI Models later.'}
                </div>
              </div>
            </div>

            <div className="mt-6 flex items-center justify-between">
              <button type="button" onClick={() => setStep('choose')} className="flex items-center gap-1.5 text-xs" style={{ color: 'var(--text-3)' }}>
                <ChevronLeft size={14} /> Back
              </button>
              <button
                type="button"
                onClick={() => finish('studio')}
                disabled={saving}
                className="h-10 px-5 rounded-xl flex items-center gap-2 text-sm font-semibold text-white disabled:opacity-50"
                style={{ background: 'var(--accent-gradient)' }}
              >
                {saving && <Loader2 size={14} className="animate-spin" />}
                Open Studio <Sparkles size={14} />
              </button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}

function ModeCard({
  active, icon, title, subtitle, description, features, onClick,
}: {
  active: boolean
  icon: React.ReactNode
  title: string
  subtitle: string
  description: string
  features: React.ReactNode[]
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="text-left p-5 rounded-2xl transition-all hover:-translate-y-1"
      style={{
        background: active ? 'var(--accent-gradient-dim)' : 'var(--surface-1)',
        border: active ? '1px solid var(--panel-border-strong)' : '1px solid var(--panel-border)',
        boxShadow: active ? '0 18px 50px var(--accent-glow-soft)' : undefined,
      }}
    >
      <div className="flex items-start justify-between">
        <span className="w-11 h-11 rounded-xl flex items-center justify-center" style={{ background: active ? 'var(--accent-gradient)' : 'var(--surface-3)', color: active ? '#fff' : 'var(--text-3)' }}>
          {icon}
        </span>
        <span className="w-5 h-5 rounded-full flex items-center justify-center" style={{ border: active ? 'none' : '1px solid var(--panel-border-strong)', background: active ? 'var(--accent)' : 'transparent' }}>
          {active && <Check size={13} color="#fff" />}
        </span>
      </div>
      <h2 className="text-lg font-bold mt-4" style={{ color: 'var(--text-1)' }}>{title}</h2>
      <div className="text-xs font-semibold mt-0.5" style={{ color: active ? 'var(--accent-hover)' : 'var(--text-3)' }}>{subtitle}</div>
      <p className="text-xs leading-relaxed mt-3 min-h-14" style={{ color: 'var(--text-3)' }}>{description}</p>
      <div className="mt-4 space-y-2">
        {features.map((feature, index) => (
          <div key={index} className="flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-2)' }}>{feature}</div>
        ))}
      </div>
    </button>
  )
}
