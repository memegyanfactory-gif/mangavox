import { useState, useEffect } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { invoke, onEvent } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import {
  Settings2, Puzzle, Cpu, Info, Search, Plus, Trash2, Download, Check,
  ExternalLink, Loader2, AlertTriangle, HardDrive, FolderOpen, Image,
  BookOpen, RefreshCw, Zap, Database, Shield, Volume2, Film, Eye, Save,
  Brain, CloudDownload, X, UserCircle, Lock, Camera, Palette, Sparkles,
  Play, Timer, Gauge, Activity, Square,
  MessageSquare,
  Bot, DownloadCloud,
} from 'lucide-react'
import { AiProvidersTab } from '../components/settings-ai/AiProvidersTab'
import { ModelHubTab } from '../components/settings-ai/ModelHubTab'
import './settings.css'
import type { RepoExtension } from '../../shared/types'
import logoUrl from '../assets/logo.png'
import {
  loadPuterSDK, isPuterSignedIn, signInToPuter, signOutOfPuter, getPuterUser,
  PUTER_DEFAULT_VISION_MODEL, PUTER_VISION_MODELS,
} from '../services/puter-client'
import { toast } from '../components/Toaster'
import { THEME_PRESETS } from '../../shared/appearance'
import { useAppearanceStore } from '../stores/appearance.store'
import { useUiStore } from '../stores/ui.store'
import { useAppModeStore, type AppMode } from '../stores/app-mode.store'
import { useAIModelsStore } from '../stores/ai-models.store'
import { AvatarPickerModal } from '../components/common/AvatarPickerModal'
import { SourceHealthPanel } from '../components/manga/SourceHealthPanel'
import { DiagnosticsCard } from '../components/DiagnosticsCard'
import { UpdateCard } from '../components/UpdateCard'

interface SourceInfo {
  id: string
  name: string
  version: string
  lang: string
  bundled: boolean
  icon?: string
}

interface CacheStats {
  covers: { count: number; sizeBytes: number }
  library: { count: number; sizeBytes: number }
  total: { sizeBytes: number }
  dataDir: string
}

type SettingsTab = 'providers' | 'models' | 'sources' | 'extensions' | 'storage' | 'pipeline' | 'ai' | 'about' | 'profile' | 'addons' | 'playback' | 'themes' | 'mode' | 'chat' | 'streaming'

const TABS: { id: SettingsTab; label: string; icon: React.ReactNode }[] = [
  { id: 'profile', label: 'Profile', icon: <UserCircle size={14} /> },
  { id: 'playback', label: 'Playback', icon: <Play size={14} /> },
  { id: 'sources', label: 'Sources', icon: <Settings2 size={14} /> },
  { id: 'extensions', label: 'Extensions', icon: <Puzzle size={14} /> },
  { id: 'addons', label: 'Addons', icon: <Film size={14} /> },
  { id: 'streaming', label: 'Streaming', icon: <Play size={14} /> },
  { id: 'storage', label: 'Storage', icon: <HardDrive size={14} /> },
  { id: 'pipeline', label: 'Pipeline', icon: <Cpu size={14} /> },
  { id: 'providers', label: 'AI Providers', icon: <Bot size={14} /> },
  { id: 'models', label: 'Model Hub', icon: <DownloadCloud size={14} /> },
  { id: 'ai', label: 'Studio models', icon: <Brain size={14} /> },
  { id: 'about', label: 'About', icon: <Info size={14} /> },
  { id: 'chat', label: 'Assistant', icon: <MessageSquare size={14} /> },
  { id: 'themes', label: 'Themes', icon: <Palette size={14} /> },
  { id: 'mode', label: 'App Mode', icon: <Sparkles size={14} /> },
]

const STUDIO_SETTINGS = new Set<SettingsTab>(['pipeline', 'ai', 'chat'])

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
}

const TAB_GROUPS: { title: string; ids: SettingsTab[] }[] = [
  { title: 'General', ids: ['profile', 'playback', 'themes', 'mode'] },
  { title: 'Content', ids: ['sources', 'extensions', 'addons', 'streaming', 'storage'] },
  { title: 'AI', ids: ['providers', 'models', 'chat', 'ai', 'pipeline'] },
  { title: 'App', ids: ['about'] },
]

export function Settings() {
  const appMode = useAppModeStore((state) => state.mode)
  const requestedTab = useUiStore((state) => state.pageParams.tab)
  const availableTabs = TABS.filter(({ id }) => appMode === 'studio' || !STUDIO_SETTINGS.has(id))
  const initialTab = availableTabs.some(({ id }) => id === requestedTab) ? requestedTab as SettingsTab : 'profile'
  const [tab, setTab] = useState<SettingsTab>(initialTab)

  // Apply a requested tab only when the request changes, so clicking other tabs doesn't snap back
  useEffect(() => {
    if (availableTabs.some(({ id }) => id === requestedTab)) setTab(requestedTab as SettingsTab)
  }, [requestedTab])

  useEffect(() => {
    if (!availableTabs.some(({ id }) => id === tab)) setTab('mode')
  }, [appMode, tab])

  useEffect(() => {
    document.querySelector<HTMLElement>(`[data-settings-tab="${tab}"]`)?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
  }, [tab])

  const current = TABS.find((t) => t.id === tab)
  const wide = tab === 'models'

  return (
    <div className="mv-settings">
      <nav className="mv-settings-nav" aria-label="Settings sections">
        {TAB_GROUPS.map((g) => {
          const items = availableTabs.filter((t) => g.ids.includes(t.id)).sort((a, b) => g.ids.indexOf(a.id) - g.ids.indexOf(b.id))
          if (!items.length) return null
          return (
            <div key={g.title} className="mv-settings-group">
              <div className="mv-settings-group-title">{g.title}</div>
              {items.map(({ id, label, icon }) => (
                <button
                  key={id}
                  type="button"
                  data-settings-tab={id}
                  onClick={() => setTab(id)}
                  className={`mv-settings-tab ${tab === id ? 'is-active' : ''}`}
                  aria-current={tab === id ? 'page' : undefined}
                >
                  {icon}
                  <span>{label}</span>
                </button>
              ))}
            </div>
          )
        })}
      </nav>

      <div className="mv-settings-body">
        <div className={`mv-settings-content ${wide ? 'is-wide' : ''}`}>
          <h1 className="mv-settings-title">{current?.label}</h1>
          {tab === 'providers' && <AiProvidersTab />}
          {tab === 'models' && <ModelHubTab />}
          {tab === 'profile' && <ProfileTab />}
          {tab === 'playback' && <PlaybackTab />}
          {tab === 'sources' && <SourcesTab />}
          {tab === 'extensions' && <ExtensionsTab />}
          {tab === 'storage' && <StorageTab />}
          {appMode === 'studio' && tab === 'pipeline' && <PipelineTab />}
          {appMode === 'studio' && tab === 'ai' && <AIModelsTab />}
          {tab === 'addons' && <AddonsTab />}
          {tab === 'streaming' && <StreamingSettingsTab />}
          {tab === 'about' && <AboutTab />}
          {tab === 'chat' && <ChatSettingsTab />}
          {tab === 'themes' && <ThemesTab />}
          {tab === 'mode' && <AppModeTab />}
        </div>
      </div>
    </div>
  )
}

function ChatSettingsTab() {
  const [clearing, setClearing] = useState(false)
  const [count, setCount] = useState(0)

  const refresh = async () => {
    const conversations = await invoke<any[]>(IPC.MEDIA_CHAT_LIST)
    setCount(conversations.length)
  }

  useEffect(() => {
    refresh().catch(() => {})
  }, [])

  const clearChatHistory = async () => {
    if (!window.confirm('Delete every saved assistant conversation? This cannot be undone.')) return
    setClearing(true)
    try {
      await invoke(IPC.MEDIA_CHAT_CLEAR)
      setCount(0)
      toast.success('Assistant history deleted')
    } catch (error: any) {
      toast.error('Could not delete assistant history', error?.message)
    } finally {
      setClearing(false)
    }
  }

  return (
    <div className="w-full space-y-5 fade-up">
      <div>
        <h3 className="text-base font-semibold" style={{ color: 'var(--text-1)' }}>Assistant privacy</h3>
        <p className="text-xs mt-1 max-w-2xl" style={{ color: 'var(--text-3)' }}>
          Normal conversations are stored only in MangaVox's local database. Private chats are kept in memory for the active session and are never saved.
        </p>
      </div>

      <div className="mv-card-2 p-5 flex items-center justify-between gap-5">
        <div>
          <div className="text-sm font-medium" style={{ color: 'var(--text-1)' }}>Saved conversation history</div>
          <div className="text-xs mt-1" style={{ color: 'var(--text-3)' }}>
            {count} saved conversation{count === 1 ? '' : 's'}. Deleting chat history does not remove manga, watch history, library notes, or story memory.
          </div>
        </div>
        <button
          type="button"
          onClick={clearChatHistory}
          disabled={clearing || count === 0}
          className="h-10 px-4 rounded-xl text-xs font-semibold flex items-center gap-2 disabled:opacity-40"
          style={{ background: 'rgba(var(--error-rgb),0.1)', color: 'var(--error)', border: '1px solid rgba(var(--error-rgb),0.2)' }}
        >
          {clearing ? <Loader2 size={14} className="animate-spin" /> : <Trash2 size={14} />}
          Delete chat history
        </button>
      </div>

      <div className="mv-card-2 p-5">
        <div className="flex items-center gap-2 text-sm font-medium" style={{ color: 'var(--text-1)' }}>
          <Shield size={15} style={{ color: 'var(--accent-hover)' }} /> What the assistant can read
        </div>
        <p className="text-xs leading-relaxed mt-2" style={{ color: 'var(--text-3)' }}>
          MangaVox supplies a limited snapshot of your library, favorites, notes, reading position, recent anime and movie history, and generated manga story memory. Online research is off by default and is used only for the message where you enable it.
        </p>
      </div>
    </div>
  )
}

function AppModeTab() {
  const mode = useAppModeStore((state) => state.mode)
  const setMode = useAppModeStore((state) => state.setMode)
  const [changing, setChanging] = useState<AppMode | null>(null)

  const chooseMode = async (nextMode: AppMode) => {
    if (nextMode === mode) return
    setChanging(nextMode)
    await setMode(nextMode)
    setChanging(null)
  }

  return (
    <div className="w-full space-y-5 fade-up">
      <div>
        <h3 className="text-base font-semibold" style={{ color: 'var(--text-1)' }}>Choose your MangaVox experience</h3>
        <p className="text-xs mt-1 max-w-2xl" style={{ color: 'var(--text-3)' }}>
          You can switch at any time. Changing modes never removes your library, downloads, projects, or settings.
        </p>
      </div>

      <div className="grid md:grid-cols-2 gap-4">
        <ModeSettingCard
          active={mode === 'entertainment'}
          title="MangaVox"
          badge="Entertainment"
          description="Manga, anime, movies, reader, library, favorites, history, downloads, subtitles, and themes. Assistant and production controls stay hidden."
          icon={<BookOpen size={22} />}
          busy={changing === 'entertainment'}
          onClick={() => chooseMode('entertainment')}
        />
        <ModeSettingCard
          active={mode === 'studio'}
          title="Manga Studio"
          badge="Full workspace"
          description="Unlock Assistant, Text, Audio, Video, Dashboard, Pipeline, AI Models, Ollama, cloud models, and production controls."
          icon={<Sparkles size={22} />}
          busy={changing === 'studio'}
          onClick={() => chooseMode('studio')}
        />
      </div>

      <AnimatePresence mode="wait">
        <motion.div
          key={mode}
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -8 }}
          className="mv-card-2 px-4 py-3 flex items-center gap-3"
        >
          <span className="w-8 h-8 rounded-full flex items-center justify-center" style={{ background: 'var(--accent-gradient-dim)', color: 'var(--accent-hover)' }}>
            <Check size={15} />
          </span>
          <div>
            <div className="text-xs font-semibold" style={{ color: 'var(--text-2)' }}>
              {mode === 'studio' ? 'Manga Studio is unlocked' : 'Entertainment mode is active'}
            </div>
            <div className="text-[11px] mt-0.5" style={{ color: 'var(--text-3)' }}>
              {mode === 'studio'
                ? 'Creator tools are now visible in the Manga sidebar.'
                : 'Assistant, Studio AI, and creation tools are hidden for a focused entertainment app.'}
            </div>
          </div>
        </motion.div>
      </AnimatePresence>
    </div>
  )
}

function ModeSettingCard({
  active, title, badge, description, icon, busy, onClick,
}: {
  active: boolean
  title: string
  badge: string
  description: string
  icon: React.ReactNode
  busy: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={busy}
      className="text-left p-5 rounded-xl transition-all disabled:opacity-60"
      style={{
        background: active ? 'var(--accent-gradient-dim)' : 'var(--surface-2)',
        border: active ? '1px solid var(--panel-border-strong)' : '1px solid var(--panel-border)',
        boxShadow: active ? '0 18px 45px var(--accent-glow-soft)' : undefined,
      }}
    >
      <div className="flex items-start justify-between">
        <span className="w-10 h-10 rounded-xl flex items-center justify-center" style={{ background: active ? 'var(--accent-gradient)' : 'var(--surface-4)', color: active ? '#fff' : 'var(--text-3)' }}>
          {busy ? <Loader2 size={18} className="animate-spin" /> : icon}
        </span>
        {active && <Check size={17} style={{ color: 'var(--accent-hover)' }} />}
      </div>
      <div className="mt-4 text-sm font-semibold" style={{ color: 'var(--text-1)' }}>{title}</div>
      <div className="text-[10px] uppercase tracking-wider mt-1" style={{ color: active ? 'var(--accent-hover)' : 'var(--text-3)' }}>{badge}</div>
      <p className="text-xs leading-relaxed mt-3" style={{ color: 'var(--text-3)' }}>{description}</p>
    </button>
  )
}

function ThemesTab() {
  const preset = useAppearanceStore((state) => state.preset)
  const reduceMotion = useAppearanceStore((state) => state.reduceMotion)
  const setPreset = useAppearanceStore((state) => state.setPreset)
  const setReduceMotion = useAppearanceStore((state) => state.setReduceMotion)

  return (
    <div className="w-full space-y-5 fade-up">
      <div className="flex items-start justify-between gap-5">
        <div>
          <h3 className="text-base font-semibold" style={{ color: 'var(--text-1)' }}>Studio themes</h3>
          <p className="text-xs mt-1 max-w-2xl" style={{ color: 'var(--text-3)' }}>
            Each preset changes the complete workspace palette, panels and controls immediately.
          </p>
        </div>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
        {THEME_PRESETS.map((theme) => {
          const active = preset === theme.id
          const c = theme.colors
          return (
            <button
              key={theme.id}
              onClick={() => setPreset(theme.id)}
              className="text-left overflow-hidden transition-all hover:-translate-y-0.5"
              style={{
                borderRadius: 12,
                border: active ? `1px solid ${c.accent}` : '1px solid var(--panel-border)',
                boxShadow: active ? `0 0 0 2px rgba(${c.accentRgb}, .12), 0 18px 40px rgba(0,0,0,.28)` : '0 12px 30px rgba(0,0,0,.18)',
                background: c.surface1,
              }}
            >
              <div
                className="h-28 relative overflow-hidden"
                style={{
                  background: `radial-gradient(circle at 78% 20%, rgba(${c.accentRgb}, .34), transparent 34%), radial-gradient(circle at 18% 88%, rgba(${c.secondaryRgb}, .22), transparent 38%), ${c.base}`,
                }}
              >
                <div className="absolute inset-x-4 top-4 flex gap-2">
                  <div className="w-9 h-20 rounded-md" style={{ background: c.surface2, border: `1px solid rgba(${c.accentRgb}, .22)` }} />
                  <div className="flex-1 space-y-2">
                    <div className="h-3 w-1/2 rounded-sm" style={{ background: c.text1, opacity: 0.86 }} />
                    <div className="h-12 rounded-md p-2" style={{ background: c.surface2, border: `1px solid rgba(${c.accentRgb}, .18)` }}>
                      <div className="h-1.5 w-4/5 rounded-full" style={{ background: c.accent, opacity: 0.75 }} />
                      <div className="h-1.5 w-2/3 rounded-full mt-2" style={{ background: c.secondary, opacity: 0.45 }} />
                    </div>
                  </div>
                </div>
                <div className="absolute bottom-3 right-3 flex gap-1">
                  {[c.accent, c.accentHover, c.secondary].map((color) => (
                    <span key={color} className="w-3 h-3 rounded-full" style={{ background: color }} />
                  ))}
                </div>
              </div>
              <div className="p-3 flex items-start gap-3">
                <div className="flex-1 min-w-0">
                  <div className="text-sm font-semibold" style={{ color: c.text1 }}>{theme.name}{theme.id === "rust" && <span className="ml-2 text-[9px] font-bold uppercase tracking-wider px-1.5 py-0.5 rounded-full align-middle" style={{ background: c.accent, color: "#fff" }}>Default</span>}</div>
                  <div className="text-[11px] mt-0.5 truncate" style={{ color: c.text3 }}>{theme.description}</div>
                </div>
                {active && (
                  <span className="w-6 h-6 rounded-full flex items-center justify-center shrink-0" style={{ background: c.accent }}>
                    <Check size={13} color="#fff" />
                  </span>
                )}
              </div>
            </button>
          )
        })}
      </div>

      <div className="mv-card-2 px-4 py-3 flex items-center justify-between gap-4">
        <div className="flex items-center gap-3">
          <Sparkles size={16} style={{ color: 'var(--accent-hover)' }} />
          <div>
            <div className="text-xs font-semibold" style={{ color: 'var(--text-2)' }}>Reduce ambient motion</div>
            <div className="text-[11px] mt-0.5" style={{ color: 'var(--text-3)' }}>Slows particles further; system reduced-motion is always respected.</div>
          </div>
        </div>
        <button
          onClick={() => setReduceMotion(!reduceMotion)}
          className="w-11 h-6 rounded-full relative transition-colors shrink-0"
          style={{ background: reduceMotion ? 'var(--accent)' : 'var(--surface-4)' }}
          aria-pressed={reduceMotion}
        >
          <span
            className="absolute top-1 w-4 h-4 rounded-full bg-white transition-all"
            style={{ left: reduceMotion ? 24 : 4 }}
          />
        </button>
      </div>
    </div>
  )
}

// ── Profile Tab ───────────────────────────────────────────

interface UserProfile {
  name: string
  avatar_path: string | null
  theme?: 'dark' | 'light' | 'system'
  password_hash: string | null
}

const DEFAULT_PROFILE: UserProfile = { name: 'User', avatar_path: null, theme: 'dark', password_hash: null }

function ProfileTab() {
  const [profile, setProfile] = useState<UserProfile>(DEFAULT_PROFILE)
  const [nameInput, setNameInput] = useState('')
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const [uploadingAvatar, setUploadingAvatar] = useState(false)
  const [avatarCacheKey, setAvatarCacheKey] = useState(Date.now())
  const [avatarModalOpen, setAvatarModalOpen] = useState(false)

  // Password lock state
  const [passwordEnabled, setPasswordEnabled] = useState(false)
  const [newPassword, setNewPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [passwordError, setPasswordError] = useState('')
  const [passwordSaved, setPasswordSaved] = useState(false)

  useEffect(() => {
    invoke<UserProfile | null>(IPC.CONFIG_GET, 'user').then((p) => {
      const prof = p || DEFAULT_PROFILE
      setProfile(prof)
      setNameInput(prof.name)
      setPasswordEnabled(!!prof.password_hash)
    })
  }, [])

  const saveName = async () => {
    setSaving(true)
    try {
      const updated = { ...profile, name: nameInput.trim() || 'User' }
      await invoke(IPC.CONFIG_SET, 'user', updated)
      setProfile(updated)
      setSaved(true)
      setTimeout(() => setSaved(false), 2000)
    } catch (err: any) {
      console.error('Failed to save profile:', err)
    }
    setSaving(false)
  }

  const handleAvatarUpload = () => {
    setAvatarModalOpen(true)
  }

  const handleSelectPredefinedAvatar = async (path: string) => {
    try {
      const updated = { ...profile, avatar_path: path }
      await invoke(IPC.CONFIG_SET, 'user', updated)
      setProfile(updated)
      setAvatarCacheKey(Date.now())
      setAvatarModalOpen(false)
    } catch (err) {
      console.error('Failed to set predefined avatar:', err)
    }
  }

  const handleCustomUploadFromModal = async () => {
    setUploadingAvatar(true)
    try {
      const path = await invoke<string | null>(IPC.USER_UPLOAD_AVATAR)
      if (path) {
        const updated = { ...profile, avatar_path: path }
        await invoke(IPC.CONFIG_SET, 'user', updated)
        setProfile(updated)
        setAvatarCacheKey(Date.now())
        setAvatarModalOpen(false)
      }
    } catch (err) {
      console.error('Failed to upload custom avatar:', err)
    } finally {
      setUploadingAvatar(false)
    }
  }

  const handleSetPassword = async () => {
    setPasswordError('')
    try {
      if (!newPassword) { setPasswordError('Enter a password'); return }
      if (newPassword !== confirmPassword) { setPasswordError('Passwords do not match'); return }
      // SHA-256 via SubtleCrypto (available in renderer)
      const msgBuffer = new TextEncoder().encode(newPassword)
      const hashBuffer = await crypto.subtle.digest('SHA-256', msgBuffer)
      const hashArray = Array.from(new Uint8Array(hashBuffer))
      const hashHex = hashArray.map((b) => b.toString(16).padStart(2, '0')).join('')
      const updated = { ...profile, password_hash: hashHex }
      await invoke(IPC.CONFIG_SET, 'user', updated)
      setProfile(updated)
      setNewPassword('')
      setConfirmPassword('')
      setPasswordSaved(true)
      setTimeout(() => setPasswordSaved(false), 2000)
    } catch (err: any) {
      setPasswordError(`Failed to set password: ${err.message}`)
    }
  }

  const handleDisablePassword = async () => {
    try {
      const updated = { ...profile, password_hash: null }
      await invoke(IPC.CONFIG_SET, 'user', updated)
      setProfile(updated)
      setPasswordEnabled(false)
      setNewPassword('')
      setConfirmPassword('')
    } catch (err: any) {
      console.error('Failed to disable password:', err)
    }
  }

  const avatarSrc = profile.avatar_path
    ? (profile.avatar_path.startsWith('/avatars/') || profile.avatar_path.startsWith('avatars/')
      ? (profile.avatar_path.startsWith('/') ? profile.avatar_path : `/${profile.avatar_path}`)
      : `mangavox-local://${profile.avatar_path}?t=${avatarCacheKey}`)
    : null

  return (
    <div className="w-full space-y-6 fade-up">
      {/* Avatar + Name */}
      <div className="rounded-xl px-6 py-6" style={{ background: 'var(--surface-2)' }}>
        <h3 className="text-sm font-semibold text-[var(--text-2)] mb-5">Your Profile</h3>

        {/* Avatar */}
        <div className="flex items-center gap-5 mb-6">
          <div className="relative">
            {avatarSrc ? (
              <img
                src={avatarSrc}
                className="w-20 h-20 rounded-full object-cover ring-2 ring-emerald-500/40/30"
                alt="avatar"
              />
            ) : (
              <div
                className="w-20 h-20 rounded-full flex items-center justify-center text-2xl font-bold"
                style={{ background: 'var(--accent-gradient)', color: 'white' }}
              >
                {(nameInput[0] || 'U').toUpperCase()}
              </div>
            )}
            <button
              onClick={handleAvatarUpload}
              disabled={uploadingAvatar}
              className="absolute -bottom-1 -right-1 w-7 h-7 rounded-full flex items-center justify-center transition-colors"
              style={{ background: 'var(--surface-4)', border: '2px solid var(--surface-2)' }}
              title="Change photo"
            >
              {uploadingAvatar ? <Loader2 size={12} className="animate-spin text-[var(--text-2)]" /> : <Camera size={12} className="text-[var(--text-2)]" />}
            </button>
          </div>
          <div>
            <p className="text-sm text-[var(--text-2)] font-medium">{profile.name}</p>
            <button
              onClick={handleAvatarUpload}
              disabled={uploadingAvatar}
              className="mt-1 text-xs text-emerald-400 hover:text-emerald-300 transition-colors"
            >
              {uploadingAvatar ? 'Uploading…' : 'Change photo'}
            </button>
          </div>
        </div>

        {/* Name field */}
        <div>
          <label className="text-xs font-medium text-[var(--text-3)] block mb-1.5">Display Name</label>
          <div className="flex gap-2">
            <input
              value={nameInput}
              onChange={(e) => setNameInput(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && saveName()}
              className="flex-1 h-9 px-3 rounded-lg text-sm text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
              placeholder="Your name"
            />
            <button
              onClick={saveName}
              disabled={saving || nameInput.trim() === profile.name}
              className="flex items-center gap-1.5 px-4 py-2 rounded-lg text-xs font-medium transition-all disabled:opacity-40"
              style={{ background: saved ? 'rgba(var(--success-rgb),0.15)' : 'var(--surface-4)', color: saved ? 'var(--success)' : 'var(--info)' }}
            >
              {saving ? <Loader2 size={12} className="animate-spin" /> : saved ? <Check size={12} /> : <Save size={12} />}
              {saved ? 'Saved' : 'Save'}
            </button>
          </div>
        </div>
      </div>

      {/* Password Lock */}
      <div className="rounded-xl px-6 py-5" style={{ background: 'var(--surface-2)' }}>
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-2">
            <Lock size={14} className="text-[var(--text-2)]" />
            <h3 className="text-sm font-semibold text-[var(--text-2)]">Password Lock</h3>
          </div>
          {profile.password_hash && (
            <span className="text-[10px] font-semibold text-[var(--success)] px-2 py-0.5 rounded-full" style={{ background: 'rgba(var(--success-rgb),0.1)' }}>
              Enabled
            </span>
          )}
        </div>
        <p className="text-xs text-[var(--text-3)] mb-4">
          Locks the app on startup — you'll need to enter the password to open MangaVox.
        </p>

        {!passwordEnabled ? (
          <div className="space-y-3">
            <div>
              <label className="text-xs text-[var(--text-3)] block mb-1">New password</label>
              <input
                type="password"
                value={newPassword}
                onChange={(e) => { setNewPassword(e.target.value); setPasswordError('') }}
                className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                placeholder="Enter password"
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-3)] block mb-1">Confirm password</label>
              <input
                type="password"
                value={confirmPassword}
                onChange={(e) => { setConfirmPassword(e.target.value); setPasswordError('') }}
                className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                placeholder="Confirm password"
              />
            </div>
            {passwordError && <p className="text-xs text-[var(--error)]">{passwordError}</p>}
            <button
              onClick={() => { setPasswordEnabled(true); handleSetPassword() }}
              disabled={!newPassword}
              className="flex items-center gap-1.5 px-4 py-2 rounded-lg text-xs font-medium transition-all disabled:opacity-40"
              style={{ background: 'rgba(200, 80, 26,0.15)', color: 'var(--info)' }}
            >
              {passwordSaved ? <Check size={12} /> : <Lock size={12} />}
              {passwordSaved ? 'Password set!' : 'Enable Password Lock'}
            </button>
          </div>
        ) : (
          <div className="space-y-3">
            <div>
              <label className="text-xs text-[var(--text-3)] block mb-1">New password (leave blank to keep current)</label>
              <input
                type="password"
                value={newPassword}
                onChange={(e) => { setNewPassword(e.target.value); setPasswordError('') }}
                className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                placeholder="New password"
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-3)] block mb-1">Confirm</label>
              <input
                type="password"
                value={confirmPassword}
                onChange={(e) => { setConfirmPassword(e.target.value); setPasswordError('') }}
                className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                placeholder="Confirm password"
              />
            </div>
            {passwordError && <p className="text-xs text-[var(--error)]">{passwordError}</p>}
            <div className="flex gap-2">
              <button
                onClick={handleSetPassword}
                disabled={!newPassword}
                className="flex items-center gap-1.5 px-4 py-2 rounded-lg text-xs font-medium transition-all disabled:opacity-40"
                style={{ background: 'rgba(200, 80, 26,0.15)', color: 'var(--info)' }}
              >
                {passwordSaved ? <Check size={12} /> : <Save size={12} />}
                {passwordSaved ? 'Updated!' : 'Update Password'}
              </button>
              <button
                onClick={handleDisablePassword}
                className="flex items-center gap-1.5 px-4 py-2 rounded-lg text-xs font-medium text-emerald-400 hover:text-emerald-300 transition-all"
                style={{ background: 'rgba(239,68,68,0.08)' }}
              >
                <X size={12} />
                Disable Lock
              </button>
            </div>
          </div>
        )}
      </div>

      <AvatarPickerModal
        isOpen={avatarModalOpen}
        currentAvatarPath={profile.avatar_path}
        onSelect={handleSelectPredefinedAvatar}
        onCancel={() => setAvatarModalOpen(false)}
        onCustomUpload={handleCustomUploadFromModal}
        uploading={uploadingAvatar}
      />
    </div>
  )
}

// ── Playback Tab ─────────────────────────────────────────

const TIMER_PRESETS = [10, 15, 20, 30, 45, 60]
const QUALITY_OPTIONS = ['Any', '720p', '1080p', '4K']

function PlaybackTab() {
  const [autoPlay, setAutoPlay] = useState(true)
  const [autoPlayTimer, setAutoPlayTimer] = useState(20)
  const [customTimer, setCustomTimer] = useState('')
  const [preDownloadNext, setPreDownloadNext] = useState(true)
  const [preferredQuality, setPreferredQuality] = useState('1080p')
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)

  useEffect(() => {
    invoke<any>(IPC.CONFIG_GET, 'playback').then((cfg) => {
      if (!cfg) return
      if (cfg.autoPlay != null) setAutoPlay(cfg.autoPlay)
      if (cfg.autoPlayTimer) setAutoPlayTimer(cfg.autoPlayTimer)
      if (cfg.preDownloadNext != null) setPreDownloadNext(cfg.preDownloadNext)
      if (cfg.preferredQuality) setPreferredQuality(cfg.preferredQuality)
    }).catch(() => {})
  }, [])

  const saveSettings = async () => {
    setSaving(true)
    try {
      await invoke(IPC.CONFIG_SET, 'playback', {
        autoPlay,
        autoPlayTimer,
        preDownloadNext,
        preferredQuality,
      })
      setSaved(true)
      setTimeout(() => setSaved(false), 2000)
    } catch { /* ok */ }
    setSaving(false)
  }

  const handleCustomTimer = () => {
    const val = parseInt(customTimer)
    if (val >= 5 && val <= 120) {
      setAutoPlayTimer(val)
      setCustomTimer('')
    }
  }

  return (
    <div className="w-full space-y-6">
      {/* Auto-play toggle */}
      <div className="rounded-xl p-5" style={{ background: 'var(--surface-2)' }}>
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-3">
            <Play size={16} className="text-emerald-400" />
            <div>
              <h3 className="text-sm font-semibold text-[var(--text-2)]">Auto-Play Next Episode</h3>
              <p className="text-[11px] text-[var(--text-3)] mt-0.5">Automatically play the next episode when the current one ends</p>
            </div>
          </div>
          <button
            onClick={() => setAutoPlay(!autoPlay)}
            className={`w-11 h-6 rounded-full transition-all duration-200 relative ${autoPlay ? 'bg-emerald-600' : 'bg-[var(--surface-4)]'}`}
          >
            <div className={`absolute top-0.5 w-5 h-5 rounded-full bg-white shadow transition-all duration-200 ${autoPlay ? 'left-[22px]' : 'left-0.5'}`} />
          </button>
        </div>

        {/* Countdown timer */}
        {autoPlay && (
          <div className="mt-4">
            <div className="flex items-center gap-2 mb-3">
              <Timer size={13} className="text-[var(--text-3)]" />
              <span className="text-xs text-[var(--text-2)]">Countdown before auto-play (seconds)</span>
            </div>
            <div className="flex flex-wrap gap-2 mb-3">
              {TIMER_PRESETS.map((t) => (
                <button
                  key={t}
                  onClick={() => setAutoPlayTimer(t)}
                  className={`px-3 py-1.5 rounded-lg text-xs font-medium transition-all ${
                    autoPlayTimer === t ? 'text-white' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
                  }`}
                  style={autoPlayTimer === t ? { background: 'rgba(200, 80, 26,0.2)', border: '1px solid rgba(200, 80, 26,0.3)' } : { background: 'var(--surface-3)', border: '1px solid transparent' }}
                >
                  {t}s
                </button>
              ))}
            </div>
            <div className="flex items-center gap-2">
              <input
                type="number"
                value={customTimer}
                onChange={(e) => setCustomTimer(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && handleCustomTimer()}
                placeholder="Custom (5-120)"
                min={5}
                max={120}
                className="w-36 h-8 px-3 rounded-lg text-xs text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
              />
              <button onClick={handleCustomTimer} className="px-3 py-1.5 rounded-lg text-xs text-emerald-400 hover:text-emerald-300 transition-colors" style={{ background: 'rgba(200, 80, 26,0.1)' }}>
                Set
              </button>
              {autoPlayTimer && !TIMER_PRESETS.includes(autoPlayTimer) && (
                <span className="text-xs text-emerald-400 font-medium">Custom: {autoPlayTimer}s</span>
              )}
            </div>
          </div>
        )}
      </div>

      {/* Pre-download next episode */}
      <div className="rounded-xl p-5" style={{ background: 'var(--surface-2)' }}>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <Download size={16} className="text-emerald-400" />
            <div>
              <h3 className="text-sm font-semibold text-[var(--text-2)]">Pre-Download Next Episode</h3>
              <p className="text-[11px] text-[var(--text-3)] mt-0.5">Start downloading the next episode during countdown for instant playback</p>
            </div>
          </div>
          <button
            onClick={() => setPreDownloadNext(!preDownloadNext)}
            className={`w-11 h-6 rounded-full transition-all duration-200 relative ${preDownloadNext ? 'bg-emerald-500' : 'bg-[var(--surface-4)]'}`}
          >
            <div className={`absolute top-0.5 w-5 h-5 rounded-full bg-white shadow transition-all duration-200 ${preDownloadNext ? 'left-[22px]' : 'left-0.5'}`} />
          </button>
        </div>
      </div>

      {/* Preferred quality */}
      <div className="rounded-xl p-5" style={{ background: 'var(--surface-2)' }}>
        <div className="flex items-center gap-3 mb-3">
          <Gauge size={16} className="text-[var(--info)]" />
          <div>
            <h3 className="text-sm font-semibold text-[var(--text-2)]">Preferred Stream Quality</h3>
            <p className="text-[11px] text-[var(--text-3)] mt-0.5">Auto-select streams matching this quality when available</p>
          </div>
        </div>
        <div className="flex gap-2">
          {QUALITY_OPTIONS.map((q) => (
            <button
              key={q}
              onClick={() => setPreferredQuality(q)}
              className={`px-4 py-2 rounded-lg text-xs font-medium transition-all ${
                preferredQuality === q ? 'text-white' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
              }`}
              style={preferredQuality === q ? {
                background: q === '4K' ? 'rgba(245,158,11,0.2)' : q === '1080p' ? 'rgba(59,130,246,0.2)' : q === '720p' ? 'rgba(200, 80, 26,0.2)' : 'rgba(200, 80, 26,0.2)',
                border: `1px solid ${q === '4K' ? 'rgba(245,158,11,0.3)' : q === '1080p' ? 'rgba(59,130,246,0.3)' : q === '720p' ? 'rgba(200, 80, 26,0.3)' : 'rgba(200, 80, 26,0.3)'}`,
              } : { background: 'var(--surface-3)', border: '1px solid transparent' }}
            >
              {q}
            </button>
          ))}
        </div>
      </div>

      {/* Save button */}
      <button
        onClick={saveSettings}
        disabled={saving}
        className="flex items-center gap-2 px-5 py-2.5 rounded-xl text-sm font-semibold text-white transition-all hover:opacity-90 disabled:opacity-50"
        style={{ background: saved ? 'var(--accent-gradient)' : 'var(--accent-gradient)' }}
      >
        {saved ? <><Check size={14} /> Saved</> : saving ? <><Loader2 size={14} className="animate-spin" /> Saving...</> : <><Save size={14} /> Save Playback Settings</>}
      </button>
    </div>
  )
}

// ── Sources Tab ──────────────────────────────────────────

function SourcesTab() {
  const [sources, setSources] = useState<SourceInfo[]>([])
  const [installUrl, setInstallUrl] = useState('')
  const [installing, setInstalling] = useState(false)
  const [message, setMessage] = useState<{ type: 'error' | 'success'; text: string } | null>(null)
  const [uninstallingId, setUninstallingId] = useState<string | null>(null)

  const loadSources = async () => {
    const list = await invoke<SourceInfo[]>(IPC.PLUGIN_LIST_SOURCES)
    setSources(list)
  }

  useEffect(() => { loadSources() }, [])

  const handleInstall = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!installUrl.trim()) return
    setInstalling(true)
    setMessage(null)
    try {
      const result = await invoke<{ id: string; name: string }>(IPC.PLUGIN_INSTALL, installUrl.trim())
      setMessage({ type: 'success', text: `Installed "${result.name}" successfully!` })
      setInstallUrl('')
      await loadSources()
    } catch (err: any) {
      setMessage({ type: 'error', text: err.message || 'Failed to install plugin' })
    } finally {
      setInstalling(false)
    }
  }

  const handleUninstall = async (id: string) => {
    setUninstallingId(id)
    try {
      await invoke(IPC.PLUGIN_UNINSTALL, id)
      await loadSources()
    } catch (err: any) {
      setMessage({ type: 'error', text: err.message })
    } finally {
      setUninstallingId(null)
    }
  }

  return (
    <div className="w-full space-y-6 fade-up">
      <SourceHealthPanel />
      <div>
        <h3 className="text-lg font-semibold text-[var(--text-2)] mb-1">Installed Sources</h3>
        <p className="text-sm text-[var(--text-3)] mb-4">
          Active manga sources. Add more from Extensions or paste a direct plugin URL.
        </p>

        {/* Direct URL install */}
        <form onSubmit={handleInstall} className="mb-4">
          <div className="flex gap-2">
            <div className="relative flex-1">
              <Plus size={14} className="absolute left-3.5 top-1/2 -translate-y-1/2 text-[var(--text-3)]" />
              <input
                type="text"
                value={installUrl}
                onChange={(e) => { setInstallUrl(e.target.value); setMessage(null) }}
                placeholder="Direct plugin URL (manifest.json or GitHub repo)..."
                className="w-full h-11 pl-9 pr-4 rounded-xl text-sm text-[var(--text-2)] placeholder-[var(--text-3)] outline-none transition-all duration-150"
                style={{
                  background: 'var(--surface-2)',
                  border: '1px solid var(--panel-border)',
                }}
                onFocus={(e) => {
                  e.currentTarget.style.borderColor = 'rgba(200, 80, 26,0.3)'
                  e.currentTarget.style.boxShadow = '0 0 0 3px rgba(200, 80, 26,0.08)'
                }}
                onBlur={(e) => {
                  e.currentTarget.style.borderColor = 'var(--panel-border)'
                  e.currentTarget.style.boxShadow = 'none'
                }}
              />
            </div>
            <button
              type="submit"
              disabled={installing || !installUrl.trim()}
              className="h-11 px-5 rounded-xl text-sm font-medium text-white transition-all duration-150 hover:opacity-90 disabled:opacity-40 flex items-center gap-2"
              style={{ background: 'var(--accent-gradient)' }}
            >
              {installing ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
              {installing ? 'Adding...' : 'Add'}
            </button>
          </div>
        </form>

        {message && (
          <div className={`mb-4 text-sm rounded-xl px-4 py-3 ${
            message.type === 'error'
              ? 'text-[var(--error)] bg-[rgba(var(--error-rgb),0.06)] border border-[rgba(var(--error-rgb),0.2)]'
              : 'text-[var(--success)] bg-[rgba(var(--success-rgb),0.06)] border border-[rgba(var(--success-rgb),0.2)]'
          }`}>
            {message.text}
          </div>
        )}

        {/* Source list */}
        <div className="space-y-1.5">
          {sources.length === 0 && (
            <div className="text-sm text-[var(--text-3)] py-6 text-center rounded-xl" style={{ background: 'var(--surface-2)' }}>
              No sources installed.
            </div>
          )}
          {sources.map((source, i) => (
            <div
              key={source.id}
              className="flex items-center justify-between px-4 py-3 rounded-xl transition-all duration-150 fade-up"
              style={{ background: 'var(--surface-2)', animationDelay: `${i * 30}ms` }}
            >
              <div className="flex items-center gap-3 min-w-0">
                {source.icon ? (
                  <img src={source.icon} alt="" className="w-10 h-10 rounded-xl object-contain shrink-0" style={{ background: 'var(--surface-3)' }} referrerPolicy="no-referrer" />
                ) : (
                  <div
                    className="w-10 h-10 rounded-xl flex items-center justify-center text-lg font-bold shrink-0"
                    style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}
                  >
                    {source.name.charAt(0).toUpperCase()}
                  </div>
                )}
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-[var(--text-2)]">{source.name}</span>
                    <span className="text-[10px] text-[var(--text-3)] font-mono">v{source.version}</span>
                    {source.bundled && (
                      <span
                        className="text-[10px] font-medium px-1.5 py-0.5 rounded-md"
                        style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}
                      >
                        built-in
                      </span>
                    )}
                  </div>
                  <p className="text-xs text-[var(--text-3)]">{source.lang.toUpperCase()}</p>
                </div>
              </div>
              {!source.bundled && (
                <button
                  onClick={() => handleUninstall(source.id)}
                  disabled={uninstallingId === source.id}
                  className="flex items-center gap-1.5 text-xs text-[var(--text-3)] hover:text-emerald-400 px-3 py-1.5 rounded-lg transition-colors disabled:opacity-40"
                  style={{ background: 'var(--surface-3)' }}
                >
                  <Trash2 size={11} />
                  {uninstallingId === source.id ? 'Removing...' : 'Remove'}
                </button>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

// ── Extensions Tab ───────────────────────────────────────

function ExtensionsTab() {
  const [repos, setRepos] = useState<string[]>([])
  const [repoUrl, setRepoUrl] = useState('')
  const [addingRepo, setAddingRepo] = useState(false)
  const [repoMessage, setRepoMessage] = useState<{ type: 'error' | 'success'; text: string } | null>(null)

  const [selectedRepo, setSelectedRepo] = useState<string | null>(null)
  const [extensions, setExtensions] = useState<RepoExtension[]>([])
  const [loadingExts, setLoadingExts] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [langFilter, setLangFilter] = useState('en')
  const [installingPkg, setInstallingPkg] = useState<string | null>(null)
  const [installedIds, setInstalledIds] = useState<Set<string>>(new Set())

  const loadRepos = async () => {
    const list = await invoke<string[]>(IPC.REPO_LIST)
    setRepos(list)
    if (list.length > 0 && !selectedRepo) {
      setSelectedRepo(list[0])
    }
  }

  useEffect(() => { loadRepos() }, [])

  useEffect(() => {
    if (!selectedRepo) {
      setExtensions([])
      return
    }
    setLoadingExts(true)
    invoke<RepoExtension[]>(IPC.REPO_BROWSE, selectedRepo, langFilter, searchQuery)
      .then(setExtensions)
      .catch(() => setExtensions([]))
      .finally(() => setLoadingExts(false))
  }, [selectedRepo, langFilter, searchQuery])

  useEffect(() => {
    invoke<SourceInfo[]>(IPC.PLUGIN_LIST_SOURCES).then((sources) => {
      setInstalledIds(new Set(sources.map((s) => s.id)))
    })
  }, [installingPkg])

  const handleAddRepo = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!repoUrl.trim()) return
    setAddingRepo(true)
    setRepoMessage(null)
    try {
      const result = await invoke<{ url: string; count: number }>(IPC.REPO_ADD, repoUrl.trim())
      setRepoMessage({ type: 'success', text: `Added repo with ${result.count} extensions` })
      setRepoUrl('')
      await loadRepos()
      setSelectedRepo(result.url)
    } catch (err: any) {
      setRepoMessage({ type: 'error', text: err.message || 'Failed to add repo' })
    } finally {
      setAddingRepo(false)
    }
  }

  const handleRemoveRepo = async (url: string) => {
    await invoke(IPC.REPO_REMOVE, url)
    if (selectedRepo === url) setSelectedRepo(null)
    await loadRepos()
  }

  const handleInstallExt = async (ext: RepoExtension, sourceIndex: number) => {
    const key = ext.pkg + '-' + sourceIndex
    setInstallingPkg(key)
    try {
      await invoke(IPC.REPO_INSTALL_EXT, ext, sourceIndex)
    } catch (err: any) {
      console.error('Install failed:', err)
    } finally {
      setInstallingPkg(null)
    }
  }

  const isInstalled = (ext: RepoExtension, sourceIndex: number): boolean => {
    const source = ext.sources[sourceIndex]
    if (!source) return false
    const pluginId = `repo-${ext.pkg.replace(/\./g, '-')}-${source.lang}`
    return installedIds.has(pluginId)
  }

  return (
    <div className="w-full space-y-6 fade-up">
      {/* Add repo */}
      <div>
        <h3 className="text-lg font-semibold text-[var(--text-2)] mb-1">Extension Repos</h3>
        <p className="text-sm text-[var(--text-3)] mb-3">
          Add a Tachiyomi-compatible extension repo to browse and install sources.
        </p>

        <form onSubmit={handleAddRepo} className="mb-3">
          <div className="flex gap-2">
            <div className="relative flex-1">
              <ExternalLink size={14} className="absolute left-3.5 top-1/2 -translate-y-1/2 text-[var(--text-3)]" />
              <input
                type="text"
                value={repoUrl}
                onChange={(e) => { setRepoUrl(e.target.value); setRepoMessage(null) }}
                placeholder="Paste repo URL (index.min.json)..."
                className="w-full h-11 pl-9 pr-4 rounded-xl text-sm text-[var(--text-2)] placeholder-[var(--text-3)] outline-none transition-all duration-150"
                style={{
                  background: 'var(--surface-2)',
                  border: '1px solid var(--panel-border)',
                }}
                onFocus={(e) => {
                  e.currentTarget.style.borderColor = 'rgba(200, 80, 26,0.3)'
                  e.currentTarget.style.boxShadow = '0 0 0 3px rgba(200, 80, 26,0.08)'
                }}
                onBlur={(e) => {
                  e.currentTarget.style.borderColor = 'var(--panel-border)'
                  e.currentTarget.style.boxShadow = 'none'
                }}
              />
            </div>
            <button
              type="submit"
              disabled={addingRepo || !repoUrl.trim()}
              className="h-11 px-5 rounded-xl text-sm font-medium text-white transition-all duration-150 hover:opacity-90 disabled:opacity-40 flex items-center gap-2"
              style={{ background: 'var(--accent-gradient)' }}
            >
              {addingRepo ? <Loader2 size={14} className="animate-spin" /> : <Plus size={14} />}
              {addingRepo ? 'Adding...' : 'Add Repo'}
            </button>
          </div>
        </form>

        {repoMessage && (
          <div className={`mb-3 text-sm rounded-xl px-4 py-3 ${
            repoMessage.type === 'error'
              ? 'text-[var(--error)] bg-[rgba(var(--error-rgb),0.06)] border border-[rgba(var(--error-rgb),0.2)]'
              : 'text-[var(--success)] bg-[rgba(var(--success-rgb),0.06)] border border-[rgba(var(--success-rgb),0.2)]'
          }`}>
            {repoMessage.text}
          </div>
        )}

        {/* Repo list */}
        {repos.length > 0 && (
          <div className="space-y-1.5 mb-4">
            {repos.map((url) => (
              <div
                key={url}
                className={`flex items-center justify-between rounded-xl px-4 py-2.5 text-sm cursor-pointer transition-all duration-150 ${
                  selectedRepo === url ? 'text-white' : 'text-[var(--text-2)] hover:text-[var(--text-2)]'
                }`}
                style={{
                  background: selectedRepo === url ? 'var(--surface-3)' : 'var(--surface-2)',
                  border: selectedRepo === url ? '1px solid rgba(200, 80, 26,0.2)' : '1px solid transparent',
                }}
                onClick={() => setSelectedRepo(url)}
              >
                <span className="truncate mr-3 text-xs font-mono">
                  {url.replace('https://raw.githubusercontent.com/', '').replace('/index.min.json', '')}
                </span>
                <button
                  onClick={(e) => { e.stopPropagation(); handleRemoveRepo(url) }}
                  className="text-[var(--text-3)] hover:text-emerald-400 shrink-0 p-1 rounded transition-colors"
                >
                  <Trash2 size={12} />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Extension browser */}
      {selectedRepo && (
        <div>
          <div className="flex items-center gap-3 mb-3">
            <h3 className="text-lg font-semibold text-[var(--text-2)]">Browse Extensions</h3>
            <span
              className="text-[10px] font-medium px-2 py-0.5 rounded-full"
              style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}
            >
              {extensions.length} found
            </span>
          </div>

          {/* Search + language filter */}
          <div className="flex gap-2 mb-4">
            <div className="relative flex-1">
              <Search size={14} className="absolute left-3.5 top-1/2 -translate-y-1/2 text-[var(--text-3)]" />
              <input
                type="text"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                placeholder="Search extensions..."
                className="w-full h-11 pl-9 pr-4 rounded-xl text-sm text-[var(--text-2)] placeholder-[var(--text-3)] outline-none transition-all duration-150"
                style={{
                  background: 'var(--surface-2)',
                  border: '1px solid var(--panel-border)',
                }}
                onFocus={(e) => {
                  e.currentTarget.style.borderColor = 'rgba(200, 80, 26,0.3)'
                  e.currentTarget.style.boxShadow = '0 0 0 3px rgba(200, 80, 26,0.08)'
                }}
                onBlur={(e) => {
                  e.currentTarget.style.borderColor = 'var(--panel-border)'
                  e.currentTarget.style.boxShadow = 'none'
                }}
              />
            </div>
            <select
              value={langFilter}
              onChange={(e) => setLangFilter(e.target.value)}
              className="h-11 px-3 rounded-xl text-sm text-[var(--text-2)] outline-none cursor-pointer"
              style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}
            >
              <option value="all">All Languages</option>
              <option value="en">English</option>
              <option value="ja">Japanese</option>
              <option value="ko">Korean</option>
              <option value="zh">Chinese</option>
              <option value="es">Spanish</option>
              <option value="fr">French</option>
              <option value="de">German</option>
              <option value="pt">Portuguese</option>
              <option value="ru">Russian</option>
              <option value="ar">Arabic</option>
              <option value="id">Indonesian</option>
              <option value="tr">Turkish</option>
              <option value="it">Italian</option>
              <option value="vi">Vietnamese</option>
              <option value="th">Thai</option>
              <option value="pl">Polish</option>
              <option value="hi">Hindi</option>
            </select>
          </div>

          {/* Loading skeleton */}
          {loadingExts && (
            <div className="space-y-1.5">
              {Array.from({ length: 6 }).map((_, i) => (
                <div key={i} className="flex items-center gap-3 px-4 py-3 rounded-xl" style={{ background: 'var(--surface-2)' }}>
                  <div className="w-10 h-10 skeleton rounded-xl shrink-0" />
                  <div className="flex-1 space-y-2">
                    <div className="skeleton h-3.5 w-1/3 rounded-md" />
                    <div className="skeleton h-3 w-1/2 rounded-md" />
                  </div>
                </div>
              ))}
            </div>
          )}

          {/* Extension list */}
          {!loadingExts && (
            <div className="space-y-1.5 max-h-[500px] overflow-y-auto pr-1">
              {extensions.length === 0 && (
                <div className="text-center py-8 text-[var(--text-3)] text-sm rounded-xl" style={{ background: 'var(--surface-2)' }}>
                  No extensions found matching your filters.
                </div>
              )}
              {extensions.map((ext) => {
                const relevantSources = langFilter === 'all'
                  ? ext.sources
                  : ext.sources.filter((s) => s.lang === langFilter || ext.lang === 'all')
                const sourcesToShow = relevantSources.length > 0 ? relevantSources : ext.sources.slice(0, 1)

                return sourcesToShow.map((source, idx) => {
                  const actualIdx = ext.sources.indexOf(source)
                  const installed = isInstalled(ext, actualIdx)
                  const installKey = ext.pkg + '-' + actualIdx

                  return (
                    <div
                      key={ext.pkg + '-' + source.lang + '-' + idx}
                      className="flex items-center justify-between px-4 py-3 rounded-xl transition-all duration-150"
                      style={{ background: 'var(--surface-2)' }}
                    >
                      <div className="flex items-center gap-3 min-w-0">
                        <div
                          className="w-10 h-10 rounded-xl flex items-center justify-center text-base font-bold shrink-0"
                          style={{ background: 'var(--surface-3)', color: 'var(--info)' }}
                        >
                          {source.name.charAt(0).toUpperCase()}
                        </div>
                        <div className="min-w-0">
                          <div className="flex items-center gap-2">
                            <span className="text-sm font-medium text-[var(--text-2)]">{source.name}</span>
                            <span className="text-[10px] text-[var(--text-3)] font-mono">v{ext.version}</span>
                            <span
                              className="text-[10px] font-medium px-1.5 py-0.5 rounded-md uppercase"
                              style={{ background: 'var(--surface-3)', color: 'var(--accent)' }}
                            >
                              {source.lang}
                            </span>
                          </div>
                          <p className="text-xs text-[var(--text-3)] truncate">{source.baseUrl}</p>
                        </div>
                      </div>

                      {installed ? (
                        <span className="flex items-center gap-1.5 text-xs text-[var(--success)] bg-[rgba(var(--success-rgb),0.1)] px-3 py-1.5 rounded-lg">
                          <Check size={12} /> Installed
                        </span>
                      ) : (
                        <button
                          onClick={() => handleInstallExt(ext, actualIdx)}
                          disabled={installingPkg === installKey}
                          className="flex items-center gap-1.5 text-xs font-medium text-white px-4 py-1.5 rounded-lg transition-all duration-150 hover:opacity-90 disabled:opacity-40"
                          style={{ background: 'var(--accent-gradient)' }}
                        >
                          {installingPkg === installKey
                            ? <><Loader2 size={12} className="animate-spin" /> Installing...</>
                            : <><Download size={12} /> Install</>
                          }
                        </button>
                      )}
                    </div>
                  )
                })
              })}
            </div>
          )}
        </div>
      )}

      {repos.length === 0 && (
        <div className="flex flex-col items-center justify-center py-12 fade-up">
          <div
            className="w-16 h-16 rounded-2xl flex items-center justify-center mb-4"
            style={{ background: 'var(--surface-2)' }}
          >
            <Puzzle size={24} className="text-[var(--text-3)]" />
          </div>
          <p className="text-[var(--text-2)] text-sm font-medium mb-1">No extension repos added yet</p>
          <p className="text-[var(--text-3)] text-xs text-center max-w-sm">
            Add a Tachiyomi-compatible repo URL above to browse and install manga sources
          </p>
        </div>
      )}
    </div>
  )
}

// ── Media folders (Settings → Storage) ───────────────────

interface StorageRow { kind: 'anime' | 'movies' | 'manga' | 'subtitles'; custom: string; default: string; effective: string; freeBytes: number | null }

const STORAGE_LABELS: Record<StorageRow['kind'], { label: string; hint: string }> = {
  anime: { label: 'Anime downloads', hint: 'Saved as <Title>/<Title - S01E05>.mkv' },
  movies: { label: 'Movies & series', hint: 'Movies: <Title (Year)>/, series: <Title>/Season 01/' },
  manga: { label: 'Manga library', hint: 'Changes apply to new downloads; existing chapters stay where they are.' },
  subtitles: { label: 'Subtitles', hint: 'A copy of every downloaded subtitle; a sidecar is also saved next to the video.' },
}

const SUBTITLE_LANG_OPTIONS: [string, string][] = [
  ['eng', 'English'], ['jpn', 'Japanese'], ['spa', 'Spanish'], ['fre', 'French'], ['ger', 'German'],
  ['ita', 'Italian'], ['por', 'Portuguese'], ['rus', 'Russian'], ['ara', 'Arabic'], ['hin', 'Hindi'],
  ['kor', 'Korean'], ['chi', 'Chinese'], ['tur', 'Turkish'], ['ind', 'Indonesian'],
]

function MediaFoldersCard() {
  const [rows, setRows] = useState<StorageRow[]>([])
  const [busy, setBusy] = useState<string | null>(null)
  const [subsOn, setSubsOn] = useState(true)
  const [langs, setLangs] = useState<string[]>(['eng'])

  useEffect(() => {
    invoke<StorageRow[]>(IPC.STORAGE_GET).then(setRows).catch(() => {})
    invoke<any>(IPC.CONFIG_GET, 'downloadSubtitles').then((v) => setSubsOn(v !== false)).catch(() => {})
    invoke<any>(IPC.CONFIG_GET, 'downloadSubtitleLangs').then((v) => { if (Array.isArray(v) && v.length) setLangs(v) }).catch(() => {})
  }, [])

  const change = async (kind: StorageRow['kind']) => {
    setBusy(kind)
    try {
      const picked = await invoke<string | null>(IPC.STORAGE_PICK, kind)
      if (!picked) return
      setRows(await invoke<StorageRow[]>(IPC.STORAGE_SET, kind, picked))
      toast.success('Folder updated', picked)
    } catch (err: any) {
      const raw = String(err?.message || err || '')
      toast.error('Cannot use that folder', /EACCES|EPERM|permission|W_OK/i.test(raw) ? 'MangaVox cannot write to that folder. Pick one you have write access to.' : raw.replace(/^Error invoking remote method '[^']+': (Error: )?/, ''))
    } finally { setBusy(null) }
  }

  const reset = async (kind: StorageRow['kind']) => {
    setBusy(kind)
    try { setRows(await invoke<StorageRow[]>(IPC.STORAGE_SET, kind, '')) }
    catch (err: any) { toast.error('Could not reset', err?.message || 'Unknown error') }
    finally { setBusy(null) }
  }

  const saveSubs = (on: boolean, nextLangs: string[]) => {
    setSubsOn(on); setLangs(nextLangs)
    void invoke(IPC.CONFIG_SET, 'downloadSubtitles', on).catch(() => {})
    void invoke(IPC.CONFIG_SET, 'downloadSubtitleLangs', nextLangs).catch(() => {})
  }

  const fmtFree = (n: number | null) => n == null ? '' : n >= 1e12 ? `${(n / 1e12).toFixed(1)} TB free` : `${(n / 1e9).toFixed(1)} GB free`

  return (
    <div>
      <h3 className="text-lg font-semibold text-[var(--text-2)] mb-1">Download Folders</h3>
      <p className="text-sm text-[var(--text-3)] mb-4">Choose where MangaVox saves anime, movies, manga and subtitles.</p>
      <div className="rounded-xl divide-y divide-[var(--panel-border)]" style={{ background: 'var(--surface-2)' }}>
        {rows.map((row) => (
          <div key={row.kind} className="px-5 py-4">
            <div className="flex items-center gap-2">
              <p className="text-sm font-medium text-[var(--text-2)]">{STORAGE_LABELS[row.kind].label}</p>
              {!row.custom && <span className="text-[10px] px-1.5 py-0.5 rounded bg-[var(--surface-3)] text-[var(--text-3)]">Default</span>}
              {row.freeBytes != null && <span className="ml-auto text-[11px] text-[var(--text-3)]">{fmtFree(row.freeBytes)}</span>}
            </div>
            <p className="text-xs text-[var(--text-3)] mt-1 break-all font-mono">{row.effective}</p>
            <p className="text-[11px] text-[var(--text-3)] mt-1">{STORAGE_LABELS[row.kind].hint}</p>
            <div className="flex gap-2 mt-3">
              <button disabled={busy === row.kind} onClick={() => void change(row.kind)} className="px-3 py-1.5 rounded-lg text-xs font-semibold text-white disabled:opacity-50" style={{ background: '#c8501a' }}>Change…</button>
              <button onClick={() => void invoke(IPC.MEDIA_DOWNLOAD_REVEAL, row.effective).catch(() => {})} className="px-3 py-1.5 rounded-lg text-xs bg-[var(--surface-3)] text-[var(--text-2)] flex items-center gap-1.5"><FolderOpen size={12} /> Open folder</button>
              {row.custom && <button disabled={busy === row.kind} onClick={() => void reset(row.kind)} className="px-3 py-1.5 rounded-lg text-xs text-[var(--text-3)] hover:text-[var(--text-2)]">Reset to default</button>}
            </div>
          </div>
        ))}
        <div className="px-5 py-4">
          <label className="flex items-center gap-2 text-sm text-[var(--text-2)] cursor-pointer">
            <input type="checkbox" checked={subsOn} onChange={(e) => saveSubs(e.target.checked, langs)} />
            Download subtitles with videos
          </label>
          <p className="text-[11px] text-[var(--text-3)] mt-1 mb-2">Fetched when a download finishes, saved as .srt and .vtt next to the video.</p>
          <div className={`flex flex-wrap gap-1.5 ${subsOn ? '' : 'opacity-40 pointer-events-none'}`}>
            {SUBTITLE_LANG_OPTIONS.map(([code, name]) => {
              const on = langs.includes(code)
              return (
                <button key={code}
                  onClick={() => { const next = on ? langs.filter((l) => l !== code) : [...langs, code]; saveSubs(subsOn, next.length ? next : ['eng']) }}
                  className="text-[11px] px-2.5 py-1 rounded-full"
                  style={{ background: on ? '#c8501a' : 'var(--surface-3)', color: on ? '#fff' : 'var(--text-2)' }}>
                  {name}
                </button>
              )
            })}
          </div>
        </div>
      </div>
    </div>
  )
}

// ── Storage & Cache Tab ──────────────────────────────────

function StorageTab() {
  const [stats, setStats] = useState<CacheStats | null>(null)
  const [loading, setLoading] = useState(true)
  const [clearing, setClearing] = useState<string | null>(null)
  const [resetConfirm, setResetConfirm] = useState(false)

  const loadStats = async () => {
    setLoading(true)
    try {
      const data = await invoke<CacheStats>(IPC.CACHE_GET_STATS)
      setStats(data)
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { loadStats() }, [])

  const handleClear = async (type: 'covers' | 'downloads' | 'all') => {
    setClearing(type)
    try {
      if (type === 'covers') await invoke(IPC.CACHE_CLEAR_COVERS)
      else if (type === 'downloads') await invoke(IPC.CACHE_CLEAR_DOWNLOADS)
      else await invoke(IPC.CACHE_CLEAR_ALL)
      await loadStats()
    } finally {
      setClearing(null)
    }
  }

  const handleReset = async () => {
    setClearing('reset')
    try {
      await invoke(IPC.CACHE_RESET_APP)
      // Wipe ALL renderer-side persistence BEFORE reload so Zustand-persist
      // can't re-hydrate from stale localStorage on next mount.
      try {
        localStorage.clear()
        sessionStorage.clear()
      } catch { /* ok */ }
      // Also clear IndexedDB databases (covers cache, etc.)
      try {
        const anyIdb: any = (window as any).indexedDB
        if (anyIdb?.databases) {
          const dbs: any[] = await anyIdb.databases()
          for (const db of dbs) { if (db?.name) anyIdb.deleteDatabase(db.name) }
        }
      } catch { /* ok */ }
      // Full reload — wipes all in-memory state so the app starts fresh
      window.location.reload()
    } catch {
      setClearing(null)
    }
  }

  const handleOpenDir = () => {
    invoke(IPC.CACHE_OPEN_DATA_DIR)
  }

  const coverPercent = stats && stats.total.sizeBytes > 0
    ? Math.round((stats.covers.sizeBytes / stats.total.sizeBytes) * 100)
    : 0
  const libraryPercent = stats && stats.total.sizeBytes > 0
    ? Math.round((stats.library.sizeBytes / stats.total.sizeBytes) * 100)
    : 0

  return (
    <div className="w-full space-y-6 fade-up">
      <MediaFoldersCard />
      {/* Overview */}
      <div>
        <h3 className="text-lg font-semibold text-[var(--text-2)] mb-1">Storage Overview</h3>
        <p className="text-sm text-[var(--text-3)] mb-4">Manage cached data and downloaded content</p>

        {loading ? (
          <div className="rounded-xl px-5 py-6" style={{ background: 'var(--surface-2)' }}>
            <div className="skeleton h-6 w-1/3 rounded-md mb-4" />
            <div className="skeleton h-3 w-full rounded-full mb-3" />
            <div className="flex gap-6">
              <div className="skeleton h-4 w-24 rounded-md" />
              <div className="skeleton h-4 w-24 rounded-md" />
            </div>
          </div>
        ) : stats && (
          <div className="rounded-xl px-5 py-5" style={{ background: 'var(--surface-2)' }}>
            {/* Total size */}
            <div className="flex items-center justify-between mb-4">
              <div className="flex items-center gap-3">
                <div className="w-10 h-10 rounded-xl flex items-center justify-center" style={{ background: 'var(--surface-3)' }}>
                  <Database size={18} className="text-[var(--text-2)]" />
                </div>
                <div>
                  <p className="text-sm text-[var(--text-2)] font-medium">Total Storage Used</p>
                  <p className="text-2xl font-bold text-[var(--text-1)]">{formatBytes(stats.total.sizeBytes)}</p>
                </div>
              </div>
              <button
                onClick={handleOpenDir}
                className="flex items-center gap-2 text-xs text-[var(--text-3)] hover:text-[var(--text-2)] px-3 py-2 rounded-xl transition-colors"
                style={{ background: 'var(--surface-3)' }}
              >
                <FolderOpen size={12} /> Open Folder
              </button>
            </div>

            {/* Bar */}
            <div className="h-2.5 rounded-full overflow-hidden flex" style={{ background: 'var(--surface-3)' }}>
              {stats.covers.sizeBytes > 0 && (
                <div
                  className="h-full rounded-l-full"
                  style={{ width: `${coverPercent}%`, background: 'var(--info)' }}
                />
              )}
              {stats.library.sizeBytes > 0 && (
                <div
                  className="h-full"
                  style={{ width: `${libraryPercent}%`, background: 'var(--info)' }}
                />
              )}
            </div>

            {/* Legend */}
            <div className="flex items-center gap-6 mt-3">
              <div className="flex items-center gap-2">
                <div className="w-2.5 h-2.5 rounded-full" style={{ background: 'var(--info)' }} />
                <span className="text-xs text-[var(--text-2)]">Covers ({formatBytes(stats.covers.sizeBytes)})</span>
              </div>
              <div className="flex items-center gap-2">
                <div className="w-2.5 h-2.5 rounded-full" style={{ background: 'var(--info)' }} />
                <span className="text-xs text-[var(--text-2)]">Downloads ({formatBytes(stats.library.sizeBytes)})</span>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* Cache categories */}
      <div>
        <h3 className="text-lg font-semibold text-[var(--text-2)] mb-3">Cache Management</h3>

        <div className="space-y-2">
          {/* Cover cache */}
          <div className="flex items-center justify-between px-5 py-4 rounded-xl" style={{ background: 'var(--surface-2)' }}>
            <div className="flex items-center gap-3">
              <div className="w-10 h-10 rounded-xl flex items-center justify-center" style={{ background: 'rgba(200, 80, 26,0.1)' }}>
                <Image size={18} className="text-[var(--info)]" />
              </div>
              <div>
                <p className="text-sm text-[var(--text-2)] font-medium">Cover Image Cache</p>
                <p className="text-xs text-[var(--text-3)]">
                  {stats ? `${stats.covers.count} images · ${formatBytes(stats.covers.sizeBytes)}` : 'Loading...'}
                </p>
              </div>
            </div>
            <button
              onClick={() => handleClear('covers')}
              disabled={clearing !== null}
              className="flex items-center gap-1.5 text-xs text-[var(--text-2)] hover:text-emerald-400 px-3 py-2 rounded-lg transition-colors disabled:opacity-40"
              style={{ background: 'var(--surface-3)' }}
            >
              {clearing === 'covers' ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
              Clear
            </button>
          </div>

          {/* Downloaded chapters */}
          <div className="flex items-center justify-between px-5 py-4 rounded-xl" style={{ background: 'var(--surface-2)' }}>
            <div className="flex items-center gap-3">
              <div className="w-10 h-10 rounded-xl flex items-center justify-center" style={{ background: 'rgba(96,165,250,0.1)' }}>
                <BookOpen size={18} className="text-[var(--info)]" />
              </div>
              <div>
                <p className="text-sm text-[var(--text-2)] font-medium">Downloaded Chapters</p>
                <p className="text-xs text-[var(--text-3)]">
                  {stats ? `${stats.library.count} files · ${formatBytes(stats.library.sizeBytes)}` : 'Loading...'}
                </p>
              </div>
            </div>
            <button
              onClick={() => handleClear('downloads')}
              disabled={clearing !== null}
              className="flex items-center gap-1.5 text-xs text-[var(--text-2)] hover:text-emerald-400 px-3 py-2 rounded-lg transition-colors disabled:opacity-40"
              style={{ background: 'var(--surface-3)' }}
            >
              {clearing === 'downloads' ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
              Clear
            </button>
          </div>
        </div>
      </div>

      {/* Danger zone */}
      <div>
        <h3 className="text-lg font-semibold text-[var(--text-2)] mb-3">Danger Zone</h3>
        <div className="space-y-2">
          <div className="rounded-xl px-5 py-4" style={{ background: 'rgba(239,68,68,0.03)', border: '1px solid rgba(239,68,68,0.1)' }}>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <div className="w-10 h-10 rounded-xl flex items-center justify-center bg-[rgba(var(--error-rgb),0.1)]">
                  <Trash2 size={18} className="text-[var(--error)]" />
                </div>
                <div>
                  <p className="text-sm text-[var(--text-2)] font-medium">Clear All Cache & Downloads</p>
                  <p className="text-xs text-[var(--text-3)]">Remove all cached covers and downloaded chapter pages</p>
                </div>
              </div>
              <button
                onClick={() => handleClear('all')}
                disabled={clearing !== null}
                className="flex items-center gap-1.5 text-xs font-medium text-[var(--error)] px-4 py-2 rounded-lg transition-colors disabled:opacity-40 bg-[rgba(var(--error-rgb),0.1)] hover:bg-[rgba(var(--error-rgb),0.18)]"
              >
                {clearing === 'all' ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
                Clear Everything
              </button>
            </div>
          </div>

          <div className="rounded-xl px-5 py-4" style={{ background: 'rgba(239,68,68,0.03)', border: '1px solid rgba(239,68,68,0.1)' }}>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <div className="w-10 h-10 rounded-xl flex items-center justify-center bg-[rgba(var(--error-rgb),0.1)]">
                  <AlertTriangle size={18} className="text-[var(--error)]" />
                </div>
                <div>
                  <p className="text-sm text-[var(--text-2)] font-medium">Factory Reset</p>
                  <p className="text-xs text-[var(--text-3)]">Wipe all data — library, chapters, history, cache. Start completely fresh.</p>
                </div>
              </div>
              {!resetConfirm ? (
                <button
                  onClick={() => setResetConfirm(true)}
                  disabled={clearing !== null}
                  className="flex items-center gap-1.5 text-xs font-medium text-[var(--error)] px-4 py-2 rounded-lg transition-colors disabled:opacity-40 bg-[rgba(var(--error-rgb),0.1)] hover:bg-[rgba(var(--error-rgb),0.18)]"
                >
                  <RefreshCw size={12} />
                  Reset App
                </button>
              ) : (
                <div className="flex items-center gap-2">
                  <span className="text-[10px] text-[var(--error)] font-medium">Are you sure?</span>
                  <button
                    onClick={() => { setResetConfirm(false); handleReset() }}
                    disabled={clearing !== null}
                    className="flex items-center gap-1.5 text-xs font-medium text-white px-3 py-1.5 rounded-lg transition-colors bg-[var(--accent-strong)] hover:bg-[var(--error)] disabled:opacity-40"
                  >
                    {clearing === 'reset' ? <Loader2 size={12} className="animate-spin" /> : <AlertTriangle size={12} />}
                    Yes, Reset Everything
                  </button>
                  <button
                    onClick={() => setResetConfirm(false)}
                    className="text-xs text-[var(--text-3)] hover:text-[var(--text-2)] px-2 py-1.5"
                  >
                    Cancel
                  </button>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* Data directory */}
      <div>
        <h3 className="text-lg font-semibold text-[var(--text-2)] mb-3">Data Locations</h3>
        <div className="rounded-xl px-5 py-4 space-y-3" style={{ background: 'var(--surface-2)' }}>
          {[
            ['Library', '~/.mangavox/library/', BookOpen],
            ['Plugins', '~/.mangavox/plugins/', Puzzle],
            ['Cache', '~/.mangavox/cache/', HardDrive],
            ['Database', '~/.mangavox/mangavox.db', Database],
          ].map(([label, path, Icon]: any) => (
            <div key={label} className="flex items-center justify-between">
              <div className="flex items-center gap-2.5">
                <Icon size={13} className="text-[var(--text-3)]" />
                <span className="text-sm text-[var(--text-2)]">{label}</span>
              </div>
              <code className="text-[11px] text-[var(--text-3)] font-mono px-2.5 py-1 rounded-lg" style={{ background: 'var(--surface-3)' }}>
                {path}
              </code>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

// ── Pipeline Tab ─────────────────────────────────────────

interface PipelineConfig {
  ocr_engine: string
  tts_engine: string
  tts_voice: string
  tts_speed: number
  video_resolution: string
  video_transition: string
  video_transition_duration: number
  video_effect: string
  stages: string[]
  auto_pipeline: boolean
  auto_continue_to_video: boolean
  video_subtitle_theme?: string
  video_subtitle_effect?: string
  bg_music?: string
  bg_music_volume?: number
  narration_volume?: number
}

const DEFAULT_PIPELINE_CONFIG: PipelineConfig = {
  ocr_engine: 'vision',
  tts_engine: 'kokoro',
  tts_voice: 'hf_alpha',
  tts_speed: 1.0,
  video_resolution: '1080p',
  video_transition: 'fade',
  video_transition_duration: 0.5,
  video_effect: 'ken_burns',
  stages: ['ocr', 'tts', 'sync_map', 'video'],
  auto_pipeline: false,
  auto_continue_to_video: true,
  video_subtitle_theme: 'Classic White',
  video_subtitle_effect: 'Karaoke Sweep',
  bg_music: '',
  bg_music_volume: 0.15,
  narration_volume: 1.0,
}

const TTS_VOICES: Record<string, { label: string; value: string }[]> = {
  kokoro: [
    { label: 'Hindi Female — hf_alpha (recommended)', value: 'hf_alpha' },
    { label: 'Hindi Male — hm_omega', value: 'hm_omega' },
    { label: 'English Female — af_heart', value: 'af_heart' },
    { label: 'English Female — af_nova', value: 'af_nova' },
    { label: 'English Male — am_echo', value: 'am_echo' },
    { label: 'English Male — am_adam', value: 'am_adam' },
    { label: 'British Female — bf_emma', value: 'bf_emma' },
    { label: 'British Male — bm_george', value: 'bm_george' },
  ],
  chatterbox: [
    { label: 'Default (or use reference audio)', value: 'default' },
  ],
  vctk: [
    { label: 'Speaker p225 (Female)', value: 'p225' },
    { label: 'Speaker p226 (Male)', value: 'p226' },
    { label: 'Speaker p227 (Male)', value: 'p227' },
    { label: 'Speaker p228 (Female)', value: 'p228' },
    { label: 'Speaker p229 (Female)', value: 'p229' },
    { label: 'Speaker p230 (Female)', value: 'p230' },
    { label: 'Speaker p231 (Female)', value: 'p231' },
    { label: 'Speaker p232 (Male)', value: 'p232' },
  ],
  xtts: [
    { label: 'Default (English)', value: 'default' },
  ],
}

const SUBTITLE_THEMES = [
  "Classic White", "Neon Cyan", "Neon Yellow", "Hot Pink", "Vibrant Orange",
  "Electric Lime", "Sleek Gold", "Cherry Red", "Sky Blue", "Mint Green",
  "Soft Lavender", "Sunset Orange", "Retro Cyberpunk", "Toxic Waste", "Ice Blue",
  "Flaming Amber", "Deep Purple", "Royal Emerald", "Dark Charcoal", "Rose Quartz"
]

const SUBTITLE_ANIMATIONS = [
  "Karaoke Sweep", "Karaoke Pop", "Fade In", "Bounce Pop", "Slide Up",
  "Slide Down", "Zoom Blur", "Elastic Bounce", "Rotate Spin", "Fade Zoom",
  "Glitch Flash", "Letter Spacing Stretch", "Shake", "Slide Left", "Slide Right",
  "Blur Fade", "Double Outline Pulse", "Slow Rise", "Fade Pulse", "Word-by-word Reveal"
]

const VISUAL_EFFECTS = [
  { value: 'ken_burns', label: 'Ken Burns (Zoom In)' },
  { value: 'zoom_out', label: 'Zoom Out' },
  { value: 'pan_right', label: 'Pan Right' },
  { value: 'pan_left', label: 'Pan Left' },
  { value: 'pan_down', label: 'Pan Down' },
  { value: 'pan_up', label: 'Pan Up' },
  { value: 'zoom_pan_right', label: 'Zoom & Pan Right' },
  { value: 'zoom_pan_left', label: 'Zoom & Pan Left' },
  { value: 'zoom_pan_down', label: 'Zoom & Pan Down' },
  { value: 'vibe', label: 'Vibe (Subtle Bounce)' },
  { value: 'none', label: 'None (Static)' },
]

function PipelineTab() {
  const [config, setConfig] = useState<PipelineConfig>(DEFAULT_PIPELINE_CONFIG)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null)

  useEffect(() => {
    invoke<PipelineConfig | null>(IPC.CONFIG_GET, 'pipeline').then((saved) => {
      if (saved) setConfig({ ...DEFAULT_PIPELINE_CONFIG, ...saved })
      setLoading(false)
    }).catch(() => setLoading(false))
  }, [])

  const saveConfig = async () => {
    setSaving(true)
    try {
      await invoke(IPC.CONFIG_SET, 'pipeline', config)
      setMessage({ type: 'success', text: 'Pipeline settings saved' })
    } catch {
      setMessage({ type: 'error', text: 'Failed to save settings' })
    }
    setSaving(false)
    setTimeout(() => setMessage(null), 3000)
  }

  const update = <K extends keyof PipelineConfig>(key: K, value: PipelineConfig[K]) => {
    setConfig((prev) => {
      const next = { ...prev, [key]: value }
      // Reset voice when engine changes
      if (key === 'tts_engine') {
        const voices = TTS_VOICES[value as string] || []
        next.tts_voice = voices[0]?.value || 'default'
      }
      return next
    })
  }

  const toggleStage = (stage: string) => {
    setConfig((prev) => {
      const has = prev.stages.includes(stage)
      return { ...prev, stages: has ? prev.stages.filter((s) => s !== stage) : [...prev.stages, stage] }
    })
  }

  const browseBgMusic = async () => {
    try {
      const path = await invoke<string | null>(IPC.CONFIG_PICK_AUDIO)
      if (path) {
        update('bg_music', path)
      }
    } catch (error: any) {
      toast.error('Failed to select background music', error?.message)
    }
  }

  if (loading) {
    return <div className="flex items-center gap-2 text-[var(--text-3)] text-sm py-8"><Loader2 size={14} className="animate-spin" /> Loading settings...</div>
  }

  const selectStyle = { background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }
  const voices = TTS_VOICES[config.tts_engine] || []

  const toggle = (on: boolean, onChange: () => void) => (
    <button
      type="button"
      onClick={onChange}
      className={`relative w-11 h-6 rounded-full transition-colors ${on ? 'bg-emerald-600' : 'bg-[var(--surface-4)]'}`}
    >
      <div className={`absolute top-0.5 w-5 h-5 rounded-full bg-white shadow transition-transform ${on ? 'translate-x-[22px]' : 'translate-x-0.5'}`} />
    </button>
  )

  return (
    <div className="w-full space-y-5 fade-up">
      {message && (
        <div className={`flex items-center gap-2 rounded-xl px-4 py-2.5 text-sm border ${
          message.type === 'success'
            ? 'text-emerald-300 border-emerald-500/25 bg-emerald-500/10'
            : 'text-rose-300 border-rose-500/25 bg-rose-500/10'
        }`}>
          {message.type === 'success' ? <Check size={14} /> : <AlertTriangle size={14} />}
          {message.text}
        </div>
      )}

      <div className="mv-card-glass p-5 flex items-center justify-between gap-4">
        <div>
          <h3 className="text-sm font-semibold text-[var(--text-1)]">Auto-process on download</h3>
          <p className="text-xs text-[var(--text-3)] mt-1">Run OCR when a chapter finishes downloading</p>
        </div>
        {toggle(config.auto_pipeline, () => update('auto_pipeline', !config.auto_pipeline))}
      </div>

      <div className="mv-card-glass p-5 flex items-center justify-between gap-4 border-blue-500/20">
        <div>
          <h3 className="text-sm font-semibold text-[var(--text-1)]">Audio → Video automatically</h3>
          <p className="text-xs text-[var(--text-3)] mt-1">After TTS + sync, render the narrated video without opening the Video tab</p>
        </div>
        {toggle(config.auto_continue_to_video, () => update('auto_continue_to_video', !config.auto_continue_to_video))}
      </div>

      <div className="mv-card p-5 space-y-4">
        <div className="flex items-center gap-2 mb-1">
          <Zap size={15} className="text-[var(--text-2)]" />
          <h3 className="text-sm font-semibold text-[var(--text-2)]">Pipeline Stages</h3>
        </div>
        <p className="text-xs text-[var(--text-3)]">Choose which stages run when you process a chapter.</p>
        <div className="grid grid-cols-2 gap-2 mt-2">
          {[
            { id: 'ocr', label: 'OCR (Text Recognition)', icon: <Eye size={13} /> },
            { id: 'tts', label: 'TTS (Text to Speech)', icon: <Volume2 size={13} /> },
            { id: 'sync_map', label: 'Sync Map (Timing)', icon: <RefreshCw size={13} /> },
            { id: 'video', label: 'Video Generation', icon: <Film size={13} /> },
          ].map(({ id, label, icon }) => (
            <button
              key={id}
              onClick={() => toggleStage(id)}
              className={`flex items-center gap-2.5 px-3.5 py-2.5 rounded-lg text-xs font-medium transition-all ${
                config.stages.includes(id) ? 'text-white' : 'text-[var(--text-3)]'
              }`}
              style={{
                background: config.stages.includes(id) ? 'var(--surface-4)' : 'var(--surface-3)',
                border: `1px solid ${config.stages.includes(id) ? 'rgba(255,255,255,0.1)' : 'var(--panel-border)'}`,
              }}
            >
              {icon}
              {label}
              {config.stages.includes(id) && <Check size={12} className="ml-auto text-emerald-400" />}
            </button>
          ))}
        </div>
      </div>

      <div className="mv-card p-5 space-y-4">
        <div className="flex items-center gap-2 mb-1">
          <Eye size={15} className="text-[var(--text-2)]" />
          <h3 className="text-sm font-semibold text-[var(--text-2)]">OCR Settings</h3>
        </div>
        <div>
          <label className="text-xs text-[var(--text-2)] block mb-1.5">Engine</label>
          <select
            value={config.ocr_engine}
            onChange={(e) => update('ocr_engine', e.target.value)}
            className="h-10 w-full px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
            style={selectStyle}
          >
            <option value="vision">macOS Vision (Built-in, Recommended)</option>
            <option value="paddleocr">PaddleOCR (Multi-language)</option>
            <option value="manga-ocr">manga-ocr (Japanese)</option>
          </select>
          <p className="text-[11px] text-[var(--text-3)] mt-1.5">
            {config.ocr_engine === 'vision' && 'Uses macOS built-in Vision framework. No installation required.'}
            {config.ocr_engine === 'paddleocr' && 'Requires: pip3 install paddleocr'}
            {config.ocr_engine === 'manga-ocr' && 'Requires: pip3 install manga-ocr'}
          </p>
        </div>
      </div>

      <div className="mv-card p-5 space-y-4">
        <div className="flex items-center gap-2 mb-1">
          <Volume2 size={15} className="text-[var(--text-2)]" />
          <h3 className="text-sm font-semibold text-[var(--text-2)]">Text-to-Speech</h3>
        </div>
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">Engine</label>
            <select
              value={config.tts_engine}
              onChange={(e) => update('tts_engine', e.target.value)}
              className="h-10 w-full px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={selectStyle}
            >
              <option value="kokoro">Kokoro-82M (Hindi + English, local AI)</option>
              <option value="chatterbox">Chatterbox (Voice Cloning)</option>
              <option value="xtts">XTTS v2 (Multilingual)</option>
              <option value="vctk">VCTK VITS (109 Speakers)</option>
            </select>
          </div>
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">Voice</label>
            <select
              value={config.tts_voice}
              onChange={(e) => update('tts_voice', e.target.value)}
              className="h-10 w-full px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={selectStyle}
            >
              {voices.map((v) => (
                <option key={v.value} value={v.value}>{v.label}</option>
              ))}
            </select>
          </div>
        </div>
        <div>
          <label className="text-xs text-[var(--text-2)] block mb-1.5">Speed: {config.tts_speed.toFixed(1)}x</label>
          <input
            type="range"
            min="0.5"
            max="2.0"
            step="0.1"
            value={config.tts_speed}
            onChange={(e) => update('tts_speed', parseFloat(e.target.value))}
            className="w-full accent-[var(--accent)]"
          />
          <div className="flex justify-between text-[10px] text-[var(--text-3)] mt-0.5">
            <span>0.5x (Slow)</span>
            <span>1.0x</span>
            <span>2.0x (Fast)</span>
          </div>
        </div>
      </div>

      <div className="mv-card p-5 space-y-4">
        <div className="flex items-center gap-2 mb-1">
          <Film size={15} className="text-[var(--info)]" />
          <h3 className="text-sm font-semibold text-[var(--text-1)]">Video output</h3>
          <span className="text-[10px] text-[var(--text-3)] ml-auto">Used when auto-continue is on</span>
        </div>
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">Resolution</label>
            <select
              value={config.video_resolution}
              onChange={(e) => update('video_resolution', e.target.value)}
              className="h-10 w-full px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={selectStyle}
            >
              <option value="720p">720p (1280x720)</option>
              <option value="1080p">1080p (1920x1080)</option>
              <option value="1440p">1440p (2560x1440)</option>
            </select>
          </div>
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">Transition</label>
            <select
              value={config.video_transition}
              onChange={(e) => update('video_transition', e.target.value)}
              className="h-10 w-full px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={selectStyle}
            >
              <option value="fade">Fade</option>
              <option value="crossfade">Crossfade</option>
              <option value="none">None (Cut)</option>
            </select>
          </div>
        </div>
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">Visual Effect</label>
            <select
              value={config.video_effect}
              onChange={(e) => update('video_effect', e.target.value)}
              className="h-10 w-full px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={selectStyle}
            >
              {VISUAL_EFFECTS.map((eff) => (
                <option key={eff.value} value={eff.value}>{eff.label}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">Transition Duration: {config.video_transition_duration.toFixed(1)}s</label>
            <input
              type="range"
              min="0.2"
              max="2.0"
              step="0.1"
              value={config.video_transition_duration}
              onChange={(e) => update('video_transition_duration', parseFloat(e.target.value))}
              className="w-full accent-[var(--accent)]"
            />
          </div>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">Subtitle Theme</label>
            <select
              value={config.video_subtitle_theme || 'Classic White'}
              onChange={(e) => update('video_subtitle_theme', e.target.value)}
              className="h-10 w-full px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={selectStyle}
            >
              {SUBTITLE_THEMES.map((theme) => (
                <option key={theme} value={theme}>{theme}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">Subtitle Animation</label>
            <select
              value={config.video_subtitle_effect || 'Karaoke Sweep'}
              onChange={(e) => update('video_subtitle_effect', e.target.value)}
              className="h-10 w-full px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={selectStyle}
            >
              {SUBTITLE_ANIMATIONS.map((anim) => (
                <option key={anim} value={anim}>{anim}</option>
              ))}
            </select>
          </div>
        </div>

        <div>
          <label className="text-xs text-[var(--text-2)] block mb-1.5">Background Music</label>
          <div className="flex gap-2">
            <input
              type="text"
              value={config.bg_music || ''}
              readOnly
              disabled
              className="flex-1 h-10 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none select-none opacity-60"
              style={selectStyle}
              placeholder="No background music selected"
            />
            <button
              type="button"
              onClick={browseBgMusic}
              className="flex items-center gap-1.5 px-4 h-10 rounded-lg text-xs font-medium transition-all"
              style={{ background: 'var(--surface-4)', color: 'var(--info)', border: '1px solid var(--panel-border)' }}
            >
              <FolderOpen size={13} />
              Browse
            </button>
            {config.bg_music && (
              <button
                type="button"
                onClick={() => update('bg_music', '')}
                className="flex items-center justify-center w-10 h-10 rounded-lg transition-all"
                style={{ background: 'rgba(var(--error-rgb),0.1)', color: 'var(--error)', border: '1px solid rgba(var(--error-rgb),0.2)' }}
                title="Clear background music"
              >
                <Trash2 size={13} />
              </button>
            )}
          </div>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">
              Narration Volume: {config.narration_volume != null ? config.narration_volume.toFixed(1) : '1.0'}x
            </label>
            <input
              type="range"
              min="0.0"
              max="2.0"
              step="0.1"
              value={config.narration_volume != null ? config.narration_volume : 1.0}
              onChange={(e) => update('narration_volume', parseFloat(e.target.value))}
              className="w-full accent-[var(--accent)]"
            />
            <div className="flex justify-between text-[10px] text-[var(--text-3)] mt-0.5">
              <span>Mute</span>
              <span>1.0x (Normal)</span>
              <span>2.0x (Loud)</span>
            </div>
          </div>
          <div>
            <label className="text-xs text-[var(--text-2)] block mb-1.5">
              Background Music Volume: {config.bg_music_volume != null ? (config.bg_music_volume * 100).toFixed(0) : '15'}%
            </label>
            <input
              type="range"
              min="0.0"
              max="1.0"
              step="0.05"
              value={config.bg_music_volume != null ? config.bg_music_volume : 0.15}
              onChange={(e) => update('bg_music_volume', parseFloat(e.target.value))}
              className="w-full accent-[var(--accent)]"
            />
            <div className="flex justify-between text-[10px] text-[var(--text-3)] mt-0.5">
              <span>Mute</span>
              <span>50%</span>
              <span>100%</span>
            </div>
          </div>
        </div>
      </div>

      <div className="flex justify-end pt-1">
        <button
          type="button"
          onClick={saveConfig}
          disabled={saving}
          className="flex items-center gap-2 px-5 py-2.5 rounded-xl text-sm font-medium text-white transition-all hover:brightness-110 disabled:opacity-50"
          style={{ background: 'var(--accent-gradient)' }}
        >
          {saving ? <Loader2 size={14} className="animate-spin" /> : <Save size={14} />}
          {saving ? 'Saving...' : 'Save Settings'}
        </button>
      </div>
    </div>
  )
}

// ── AI Models Tab ────────────────────────────────────────────

interface OllamaConfig {
  url: string
  vision_model: string
  text_model: string
  vision_temperature: number
  text_temperature: number
}

interface VoiceModelStatus {
  installed?: boolean
  engine?: string
  f5Ready?: boolean
  parlerReady?: boolean
  kokoroReady?: boolean
  xttsReady?: boolean
  piperReady?: boolean
  piperHiReady?: boolean
  piperEnReady?: boolean
  depsReady?: boolean
  modelsReady?: boolean
  sizeMb?: number
  baseDir?: string
  modelDir?: string
  piperHiDir?: string
  piperEnDir?: string
  xttsDir?: string
  f5Dir?: string
  parlerDir?: string
  activePython?: string
  venvPython?: string | null
  error?: string
}

const DEFAULT_OLLAMA_CONFIG: OllamaConfig = {
  url: 'http://127.0.0.1:11434',
  vision_model: 'minicpm-v:8b',
  text_model: 'llama3.1:8b',
  vision_temperature: 0.3,
  text_temperature: 0.7,
}

// Free vision-capable models that can be pulled from Ollama — ranked by text reading quality
const RECOMMENDED_VISION_MODELS = [
  { name: 'minicpm-v:8b', size: '5.5 GB', desc: 'Best text reader — excellent OCR, reads manga dialogue accurately', badge: 'Best' },
  { name: 'llava-llama3:8b', size: '5.5 GB', desc: 'Strong all-round vision model — good text extraction', badge: null },
  { name: 'gemma3:12b', size: '8.1 GB', desc: 'Google Gemma 3 with vision — great at reading & translating text', badge: 'Quality' },
  { name: 'llava:7b', size: '4.7 GB', desc: 'Decent text reading — lighter weight option', badge: null },
  { name: 'moondream:1.8b', size: '1.7 GB', desc: 'Tiny & fast but lower accuracy — good for quick previews', badge: 'Fastest' },
]

// NVIDIA NIM vision models — requires NVIDIA API key
const NVIDIA_NIM_MODELS = [
  { id: 'nvidia/nemotron-3-nano-omni-30b-a3b-reasoning', label: 'Nemotron 3 Nano Omni 30B', desc: 'NVIDIA Nemotron — advanced reasoning + vision, excellent for complex manga panels', badge: 'Reasoning' },
  { id: 'nvidia/neva-22b', label: 'NEVA 22B', desc: 'NVIDIA NEVA — vision-language model for image understanding', badge: null },
]

const RECOMMENDED_TEXT_MODELS = [
  { name: 'llama3.1:8b', size: '4.7 GB', desc: 'Great general text model for enhancement' },
  { name: 'qwen3.5:9b', size: '5.5 GB', desc: 'Strong multilingual text model' },
  { name: 'mistral:7b', size: '4.1 GB', desc: 'Fast, creative writing' },
  { name: 'gemma3:4b', size: '3.3 GB', desc: 'Small & efficient for quick enhancement' },
]

// Available Google Gemini models for vision/OCR
// Public Gemini v1beta models that support generateContent + vision.
// Removed `gemini-2.0-flash-live` — that ID is not exposed on the public API
// and any saved config pointing at it 404s. Map any stale value back to
// gemini-2.5-flash at read time below.
const GEMINI_MODELS = [
  { id: 'gemini-2.5-flash',   label: 'Gemini 2.5 Flash',  desc: 'Recommended — fast, accurate, best for manga' },
  { id: 'gemini-2.5-pro',     label: 'Gemini 2.5 Pro',    desc: 'Highest quality, slower, lower free quota' },
  { id: 'gemini-2.0-flash',   label: 'Gemini 2.0 Flash',  desc: 'Previous generation, still excellent' },
  { id: 'gemini-1.5-pro',     label: 'Gemini 1.5 Pro',    desc: 'Larger context window, good for detailed analysis' },
  { id: 'gemini-1.5-flash',   label: 'Gemini 1.5 Flash',  desc: 'Fast and efficient' },
]

const GEMINI_RETIRED_IDS = new Set<string>(['gemini-2.0-flash-live'])

// ── g0i.ai Universal Gateway ──────────────────────────────────────────────
// g0i.ai exposes an OpenAI-compatible /v1/chat/completions endpoint that
// proxies Gemini, Claude (Opus) and OpenAI (GPT-5 family) under one API key.
// Users get the key at https://g0i.ai/api-keys.
//
// IMPORTANT: the real API host is api.g0i.ai — the marketing site g0i.ai is
// behind Cloudflare and returns 403 on POST. The first release shipped the
// wrong default and every call failed at auth-time. We also keep this list
// in sync with `GET /v1/models` rather than guessing names.
type G0iFamily = 'gemini' | 'gpt' | 'claude' | 'kimi' | 'deepseek'

interface G0iModel {
  id: string
  label: string
  desc: string
  family: G0iFamily
  vision: boolean
  // Some tiers require a paid plan upgrade — flagged so the UI can warn ahead
  // of time instead of failing at the first narration batch.
  requiresUpgrade?: boolean
  // True when text works but vision is unstable on this specific upstream
  // (eg. gemini-3.5-flash returns 500 on image input today). We still allow
  // selecting it for non-narration use; the UI surfaces the caveat.
  visionUnstable?: boolean
  recommended?: boolean
}

// Live roster verified against POST https://api.g0i.ai/v1/chat/completions
// (see commit notes). Names that aren't here will get a 400 from the gateway.
const G0I_MODELS: G0iModel[] = [
  // Google Gemini family — verified by probing api.g0i.ai with the demo key.
  // gemini-2.5-flash-new is the only one whose vision endpoint is stable
  // right now (3.x previews 500 on image input but their text path works).
  { id: 'gemini-2.5-flash-new',   label: 'Gemini 2.5 Flash',         desc: 'Recommended — stable vision + writing on g0i', family: 'gemini', vision: true, recommended: true },
  { id: 'gemini-3.5-flash',       label: 'Gemini 3.5 Flash',         desc: 'Newest Flash — text strong, vision unstable today', family: 'gemini', vision: true, visionUnstable: true },
  { id: 'gemini-3.1-pro-preview', label: 'Gemini 3.1 Pro (preview)', desc: 'Highest Gemini quality — vision unstable today',    family: 'gemini', vision: true, visionUnstable: true },
  // OpenAI family — vision works on the demo key.
  { id: 'gpt-5.4',          label: 'GPT-5.4',          desc: 'OpenAI flagship via Azure — strong vision + writing', family: 'gpt',      vision: true  },
  { id: 'gpt-5.4-mini',     label: 'GPT-5.4 mini',     desc: 'Cheap & fast — good for high-volume OCR',             family: 'gpt',      vision: true  },
  { id: 'gpt-5.4-pro',      label: 'GPT-5.4 Pro',      desc: 'Best OpenAI quality on g0i — slowest',                family: 'gpt',      vision: true  },
  { id: 'gpt-5.5',          label: 'GPT-5.5',          desc: 'Newest GPT-5 point release',                          family: 'gpt',      vision: true  },
  { id: 'gpt-5.3-codex',    label: 'GPT-5.3 Codex',    desc: 'Code-leaning variant (text only)',                    family: 'gpt',      vision: false },
  // Claude family — Opus-tier plan required.
  { id: 'aicenter-claude-sonnet-4-6', label: 'Claude Sonnet 4.6', desc: 'Great narration prose — needs Opus plan', family: 'claude', vision: true, requiresUpgrade: true },
  { id: 'aicenter-claude-opus-4-6',   label: 'Claude Opus 4.6',   desc: 'Anthropic flagship — needs Opus plan',     family: 'claude', vision: true, requiresUpgrade: true },
  // Kimi family — Moonshot multimodal.
  { id: 'kimi-k2.6',         label: 'Kimi K2.6',         desc: 'Moonshot AI multimodal — strong long-form',         family: 'kimi',     vision: true  },
  { id: 'kimi-k2.5',         label: 'Kimi K2.5',         desc: 'Previous Kimi — stable fallback',                   family: 'kimi',     vision: true  },
  { id: 'kimi-k2-thinking',  label: 'Kimi K2 Thinking',  desc: 'Reasoning-tuned Kimi (text)',                       family: 'kimi',     vision: false },
  // DeepSeek family — text only at this tier.
  { id: 'deepseek-v3.2',          label: 'DeepSeek v3.2',           desc: 'Strong text reasoning (text only)',         family: 'deepseek', vision: false },
  { id: 'deepseek-v3.2-speciale', label: 'DeepSeek v3.2 Speciale',  desc: 'Specialised DeepSeek variant (text only)',  family: 'deepseek', vision: false },
]

const G0I_DEFAULT_BASE_URL = 'https://api.g0i.ai/v1'
// The previous release shipped a hard-coded sample key as a default. The
// gateway killed its trial, so every narration spent tokens to get
// "Trial expired". We now ship empty + force the user to paste their own —
// the dead key string is kept only so we can recognise and CLEAR it if a user
// is upgrading from a config where it was auto-persisted.
const G0I_DEAD_SAMPLE_KEY = 'sk-43484e549f578382daf4453b70887abff1b98ba0b1e9295d196c4871c217240b'

// ── Polished Test-Key result card ─────────────────────────────────────────
// Shape mirrors the IPC `g0i:test-key` reply. Lives at module scope so we can
// type it once and reuse.
interface G0iTestResultShape {
  ok: boolean
  code: string
  message: string
  detail?: string
  status?: number
  latencyMs?: number
  modelCount?: number
  visionCount?: number
  baseUrl?: string
}

const G0I_CODE_TIPS: Record<string, string> = {
  BAD_KEY: 'Open https://g0i.ai/api-keys, generate a new key, paste it above. Old keys can be revoked when a new one is created.',
  BAD_URL: 'The gateway base URL is wrong. Reset Advanced → Base URL to https://api.g0i.ai/v1.',
  RATE_LIMITED: 'You\'ve hit your plan\'s request ceiling. Wait a minute, or upgrade at https://g0i.ai/pricing.',
  QUOTA_EXCEEDED: 'The shared demo key has hit its daily 100k-token cap. Get your own free key at https://g0i.ai/api-keys (fresh daily allowance) or wait until 00:00 UTC for the cap to reset.',
  TRIAL_EXPIRED: 'The free trial on this key is over — g0i wants a paid plan to continue. Either sign up for your own free key at https://g0i.ai/api-keys (a brand-new account may grant a fresh trial), or buy a plan at https://g0i.ai/billing. Until then, switch the Text Studio engine to Google Gemini (free, direct) or local Ollama in Settings → AI Models.',
  TIMEOUT: 'Network or firewall is blocking the request. Try a VPN, or check if a corporate proxy is filtering api.g0i.ai.',
  DNS: 'Your network can\'t resolve api.g0i.ai. Check your DNS / internet — try `ping api.g0i.ai` in a terminal.',
  REFUSED: 'Something is intercepting the connection. The base URL might be wrong, or a local proxy is dropping it.',
  TLS: 'The TLS handshake failed. Your system clock might be wrong, or a corporate proxy is rewriting certificates.',
  NETWORK: 'A low-level network error happened. Re-check internet, VPN, and firewall.',
  NO_KEY: 'Paste your g0i.ai API key in the field above, then click Test key again.',
  HTTP_ERROR: 'The gateway returned an unexpected status. Try a different model family or contact g0i.ai support with the detail below.',
  IPC_ERROR: 'The renderer couldn\'t talk to the main process. Restart MangaVox.',
  OK: '',
}

function G0iTestResultCard({
  result,
  onDismiss,
  onRetry,
}: {
  result: G0iTestResultShape
  onDismiss: () => void
  onRetry: () => void
}) {
  const ok = result.ok
  const accent = ok ? 'var(--success)' : 'var(--info)'
  const bg = ok ? 'rgba(52,211,153,0.06)' : 'rgba(248,113,113,0.06)'
  const border = ok ? 'rgba(52,211,153,0.25)' : 'rgba(248,113,113,0.30)'
  const tip = G0I_CODE_TIPS[result.code] || ''
  const ms = result.latencyMs
  const latencyTone =
    ms == null ? 'text-[var(--text-3)]'
      : ms < 400 ? 'text-[var(--success)]'
      : ms < 1200 ? 'text-[var(--warning)]'
      : 'text-[var(--error)]'

  return (
    <div
      className="mt-2 rounded-xl overflow-hidden fade-up"
      style={{ background: bg, border: `1px solid ${border}` }}
    >
      <div className="flex items-start gap-3 px-3.5 py-3">
        {/* Icon */}
        <div
          className="w-8 h-8 rounded-lg flex items-center justify-center shrink-0"
          style={{ background: ok ? 'rgba(52,211,153,0.14)' : 'rgba(248,113,113,0.14)' }}
        >
          {ok
            ? <Check size={16} style={{ color: accent }} />
            : <AlertTriangle size={16} style={{ color: accent }} />}
        </div>

        {/* Body */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center justify-between gap-2">
            <p className="text-[12px] font-semibold leading-tight" style={{ color: accent }}>
              {ok ? 'Connected' : result.code.replace(/_/g, ' ').toLowerCase().replace(/^\w/, (c) => c.toUpperCase())}
            </p>
            <button
              onClick={onDismiss}
              className="text-[var(--text-3)] hover:text-[var(--text-2)] transition-colors shrink-0 rounded-md p-0.5"
              title="Dismiss"
            >
              <X size={12} />
            </button>
          </div>

          {/* Metric chips for success */}
          {ok && (
            <div className="flex flex-wrap items-center gap-1.5 mt-1.5">
              {ms != null && (
                <span
                  className={`inline-flex items-center gap-1 text-[10px] font-mono px-1.5 py-0.5 rounded-md ${latencyTone}`}
                  style={{ background: 'var(--panel-border)' }}
                  title="Round-trip latency to api.g0i.ai"
                >
                  <Activity size={9} /> {ms} ms
                </span>
              )}
              {result.modelCount != null && (
                <span
                  className="inline-flex items-center gap-1 text-[10px] font-mono text-[var(--text-2)] px-1.5 py-0.5 rounded-md"
                  style={{ background: 'var(--panel-border)' }}
                >
                  <Database size={9} /> {result.modelCount} models
                </span>
              )}
              {result.visionCount ? (
                <span
                  className="inline-flex items-center gap-1 text-[10px] font-mono text-emerald-300 px-1.5 py-0.5 rounded-md"
                  style={{ background: 'rgba(200, 80, 26,0.10)' }}
                  title="Models that should accept image input"
                >
                  <Eye size={9} /> {result.visionCount} vision
                </span>
              ) : null}
              {result.status != null && (
                <span
                  className="text-[10px] font-mono text-[var(--text-3)] px-1.5 py-0.5 rounded-md"
                  style={{ background: 'var(--panel-border)' }}
                >
                  HTTP {result.status}
                </span>
              )}
            </div>
          )}

          {/* Failure heading + status badge */}
          {!ok && (
            <p className="text-[11px] text-[var(--text-2)] leading-snug mt-1">{result.message}</p>
          )}

          {/* Success message line */}
          {ok && (
            <p className="text-[11px] text-[var(--text-2)] leading-snug mt-1">{result.message}</p>
          )}

          {/* Server detail (collapsible-feel) */}
          {!ok && result.detail && (
            <div
              className="mt-2 px-2 py-1.5 rounded-md text-[10px] font-mono text-[var(--text-2)] leading-relaxed break-words"
              style={{ background: 'rgba(0,0,0,0.25)', border: '1px solid var(--panel-border)' }}
            >
              <span className="text-[var(--text-3)] mr-1">server:</span>{result.detail}
            </div>
          )}

          {/* What to do next */}
          {!ok && tip && (
            <p className="text-[10px] text-[var(--text-3)] leading-relaxed mt-1.5">
              <span className="text-[var(--text-2)] font-semibold mr-1">Fix:</span>
              {tip}
            </p>
          )}

          {/* Action row for failures */}
          {!ok && (
            <div className="flex items-center gap-1.5 mt-2">
              <button
                onClick={onRetry}
                className="flex items-center gap-1 text-[10px] font-semibold text-white px-2 py-1 rounded-md transition-all hover:brightness-110"
                style={{ background: 'var(--accent-gradient)' }}
              >
                <RefreshCw size={10} /> Retry
              </button>
              {(result.code === 'BAD_KEY' || result.code === 'RATE_LIMITED' || result.code === 'QUOTA_EXCEEDED' || result.code === 'TRIAL_EXPIRED') && (
                <button
                  onClick={() => (window as any).electron?.openExternal?.('https://g0i.ai/api-keys')}
                  className="flex items-center gap-1 text-[10px] font-medium text-white px-2 py-1 rounded-md transition-colors hover:brightness-110"
                  style={{ background: 'var(--accent-gradient)' }}
                >
                  <ExternalLink size={10} /> Get my own key
                </button>
              )}
              {result.code === 'TRIAL_EXPIRED' && (
                <button
                  onClick={() => (window as any).electron?.openExternal?.('https://g0i.ai/billing')}
                  className="flex items-center gap-1 text-[10px] font-medium text-amber-300 hover:text-amber-200 px-2 py-1 rounded-md transition-colors"
                  style={{ background: 'rgba(245,158,11,0.10)' }}
                >
                  <ExternalLink size={10} /> Billing
                </button>
              )}
              {result.code === 'BAD_URL' && (
                <span className="text-[10px] text-[var(--text-3)] font-mono">expected https://api.g0i.ai/v1</span>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

interface TokenUsageStats {
  totalTokens: number
  privateTokens: number
  publicTokens: number
  requestCount: number
  daily: Array<{ day: string; tokens: number; privateTokens: number }>
  activities: Array<{ label: string; mode: string; tokens: number; createdAt: string; provider: string }>
  byProvider: Array<{ provider: string; tokens: number; count: number }>
}

function AIModelsTab() {
  const [config, setConfig] = useState<OllamaConfig>(DEFAULT_OLLAMA_CONFIG)
  const [geminiKey, setGeminiKey] = useState('')
  const [geminiModel, setGeminiModel] = useState<string>('gemini-2.5-flash')
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [message, setMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null)
  const [voiceStatus, setVoiceStatus] = useState<VoiceModelStatus | null>(null)
  const [voiceChecking, setVoiceChecking] = useState(false)
  const [voiceInstalling, setVoiceInstalling] = useState(false)
  const [voiceProgress, setVoiceProgress] = useState<{ percent: number; message: string } | null>(null)

  const [tokenStats, setTokenStats] = useState<TokenUsageStats | null>(null)

  const fetchTokenStats = async () => {
    try {
      const stats = await invoke<TokenUsageStats>(IPC.TOKEN_USAGE_STATS)
      if (stats) setTokenStats(stats)
    } catch (error) {
      console.error('Failed to fetch token stats:', error)
    }
  }

  const handleClearTokenUsage = async () => {
    if (!window.confirm('Are you sure you want to reset all token usage analytics? This cannot be undone.')) return
    try {
      await invoke(IPC.TOKEN_USAGE_CLEAR)
      toast.success('Token usage stats cleared successfully')
      fetchTokenStats()
    } catch (error: any) {
      toast.error('Failed to clear token usage stats', error?.message)
    }
  }

  // ── g0i.ai Universal Gateway state ──────────────────────────────────────
  // Start EMPTY now that the bundled sample key's trial has expired upstream.
  // First-run users see a clear "paste your key" placeholder instead of
  // burning tokens on a key that will always 403.
  const [g0iKey, setG0iKey] = useState<string>('')
  const [g0iFamily, setG0iFamily] = useState<G0iFamily>('gemini')
  const [g0iModel, setG0iModel] = useState<string>('gemini-2.5-flash-new')
  const [g0iBaseUrl, setG0iBaseUrl] = useState<string>(G0I_DEFAULT_BASE_URL)
  const [g0iAdvancedOpen, setG0iAdvancedOpen] = useState(false)
  const [tokenBudget, setTokenBudget] = useState<'low' | 'balanced' | 'high'>('balanced')

  // ── OpenRouter state ───────────────────────────────────────────────────
  // No bundled key: users bring their own (a shared key was previously committed here — revoke it).
  const OPENROUTER_DEFAULT_KEY = ''
  const OPENROUTER_DEFAULT_BASE_URL = 'https://openrouter.ai/api/v1'
  const [openrouterKey, setOpenrouterKey] = useState<string>(OPENROUTER_DEFAULT_KEY)
  const [openrouterModel, setOpenrouterModel] = useState<string>('google/gemini-2.5-flash')
  const [openrouterBaseUrl, setOpenrouterBaseUrl] = useState<string>(OPENROUTER_DEFAULT_BASE_URL)
  const [openrouterAdvancedOpen, setOpenrouterAdvancedOpen] = useState(false)
  const [openrouterTesting, setOpenrouterTesting] = useState(false)
  const [openrouterTestResult, setOpenrouterTestResult] = useState<any | null>(null)
  const [openrouterModels, setOpenrouterModels] = useState<Array<{ id: string; name: string }>>([
    { id: 'google/gemini-2.5-flash', name: 'Google: Gemini 2.5 Flash' },
    { id: 'google/gemini-2.5-pro', name: 'Google: Gemini 2.5 Pro' },
    { id: 'anthropic/claude-3.5-sonnet', name: 'Anthropic: Claude 3.5 Sonnet' },
    { id: 'meta-llama/llama-3.2-11b-vision-instruct', name: 'Meta: Llama 3.2 11B Vision' },
    { id: 'qwen/qwen-2-vl-7b-instruct', name: 'Qwen: Qwen 2 VL 7B' },
  ])

  // Web search providers — keys let the assistant gather better results + real
  // images everywhere it researches (chat, deep research, etc.).
  const [searchCfg, setSearchCfg] = useState<{ tavily: string; brave: string; googleKey: string; googleCx: string; bing: string; serpapi: string }>({
    tavily: '', brave: '', googleKey: '', googleCx: '', bing: '', serpapi: '',
  })
  const [searchSaved, setSearchSaved] = useState(false)
  useEffect(() => {
    invoke<any>(IPC.CONFIG_GET, 'search').then((s) => {
      if (s) setSearchCfg((prev) => ({ ...prev, ...s }))
    }).catch(() => {})
  }, [])
  const saveSearchCfg = async () => {
    const trimmed = Object.fromEntries(Object.entries(searchCfg).map(([k, v]) => [k, (v || '').trim()]))
    await invoke(IPC.CONFIG_SET, 'search', trimmed)
    setSearchSaved(true)
    setTimeout(() => setSearchSaved(false), 1800)
  }
  // Test-key UI state — tells the user instantly whether the key + base URL
  // actually work, instead of waiting for narration to fail mid-batch.
  type G0iTestResult = G0iTestResultShape
  const [g0iTesting, setG0iTesting] = useState(false)
  const [g0iTestResult, setG0iTestResult] = useState<G0iTestResult | null>(null)

  // ── Puter (free Gemini vision) ─────────────────────────────────────────
  const [puterUser, setPuterUser] = useState<{ name: string; email?: string } | null>(null)
  const [puterChecking, setPuterChecking] = useState(false)
  const [puterModel, setPuterModel] = useState<string>(() => {
    try { return localStorage.getItem('mangavox.puterModel') || PUTER_DEFAULT_VISION_MODEL }
    catch { return PUTER_DEFAULT_VISION_MODEL }
  })
  useEffect(() => {
    try { localStorage.setItem('mangavox.puterModel', puterModel) } catch { /* ok */ }
  }, [puterModel])

  // Probe Puter sign-in status on mount — silent, non-blocking
  useEffect(() => {
    let cancelled = false
    setPuterChecking(true)
    loadPuterSDK()
      .then(async () => {
        if (cancelled) return
        const signed = await isPuterSignedIn()
        if (cancelled) return
        if (signed) setPuterUser(await getPuterUser())
      })
      .catch(() => { /* silent — SDK loads on demand later if needed */ })
      .finally(() => { if (!cancelled) setPuterChecking(false) })
    return () => { cancelled = true }
  }, [])

  const handlePuterSignIn = async () => {
    setPuterChecking(true)
    try {
      await signInToPuter()
      const u = await getPuterUser()
      setPuterUser(u)
      toast.success('Signed in to Puter', u?.name ? `as ${u.name}` : 'free Gemini vision unlocked')
    } catch (err: any) {
      toast.error('Puter sign-in failed', err?.message || 'try again')
    }
    setPuterChecking(false)
  }

  const handlePuterSignOut = async () => {
    try {
      await signOutOfPuter()
      setPuterUser(null)
      toast.info('Signed out of Puter')
    } catch (err: any) {
      toast.error('Sign-out failed', err?.message)
    }
  }

  // Ollama status
  const [status, setStatus] = useState<{ online: boolean; models: number } | null>(null)
  const systemProfile = useAIModelsStore((state) => state.profile)
  const modelCatalog = useAIModelsStore((state) => state.catalog)
  const syncedVisionModels = useAIModelsStore((state) => state.visionModels)
  const syncedTextModels = useAIModelsStore((state) => state.textModels)
  const modelPreferences = useAIModelsStore((state) => state.preferences)
  const refreshAIModels = useAIModelsStore((state) => state.refresh)
  const setGlobalVisionModel = useAIModelsStore((state) => state.setVisionModel)
  const setGlobalTextModel = useAIModelsStore((state) => state.setTextModel)
  const visionModels = syncedVisionModels.map((model) => model.name)
  const textModels = syncedTextModels.map((model) => model.name)
  const allInstalledModels = [...new Set([...visionModels, ...textModels])]
  const recommendedVisionModels = modelCatalog.filter((model) => model.roles.includes('vision'))
  const recommendedTextModels = modelCatalog.filter((model) => model.roles.includes('text'))
  const [testing, setTesting] = useState(false)
  // Running LLMs (loaded in VRAM/RAM)
  const [runningModels, setRunningModels] = useState<{ name: string; size: number; expires_at?: string }[]>([])
  const [killingModel, setKillingModel] = useState<string | null>(null)

  // Model pull state
  const [pulling, setPulling] = useState<string | null>(null)
  const [pullProgress, setPullProgress] = useState<{ percent: number; status: string }>({ percent: 0, status: '' })

  useEffect(() => {
    invoke<OllamaConfig | null>(IPC.CONFIG_GET, 'ollama').then((saved) => {
      if (saved) setConfig({ ...DEFAULT_OLLAMA_CONFIG, ...saved })
      setLoading(false)
    }).catch(() => setLoading(false))
    invoke<any>(IPC.CONFIG_GET, 'gemini').then((saved) => {
      if (saved?.api_key) setGeminiKey(saved.api_key)
      if (saved?.model) {
        // Migrate retired IDs (eg. gemini-2.0-flash-live, which 404s on the
        // public v1beta API) to the current Flash default and persist the fix
        // so the narrator IPC doesn't keep reading the dead ID from disk.
        if (GEMINI_RETIRED_IDS.has(saved.model)) {
          const migrated = 'gemini-2.5-flash'
          setGeminiModel(migrated)
          invoke(IPC.CONFIG_SET, 'gemini', { api_key: saved.api_key || '', model: migrated }).catch(() => {})
        } else {
          setGeminiModel(saved.model)
        }
      }
    }).catch(() => {})
    invoke<any>(IPC.CONFIG_GET, 'g0i').then((saved) => {
      // Auto-clear the dead bundled key if a previous release auto-persisted
      // it — leaving it on disk means the narrator would 403 forever.
      if (saved?.api_key && saved.api_key !== G0I_DEAD_SAMPLE_KEY) {
        setG0iKey(saved.api_key)
      } else if (saved?.api_key === G0I_DEAD_SAMPLE_KEY) {
        setG0iKey('')
        invoke(IPC.CONFIG_SET, 'g0i', {
          api_key: '',
          family: saved.family || 'gemini',
          model: saved.model || 'gemini-2.5-flash-new',
          base_url: saved.base_url || G0I_DEFAULT_BASE_URL,
        }).catch(() => {})
      }
      // If the saved family/model points at IDs that aren't on g0i anymore,
      // snap back to the Gemini Flash default. Autosave rewrites the file.
      const validFamily: G0iFamily[] = ['gemini', 'gpt', 'claude', 'kimi', 'deepseek']
      // 'openai' was the old key for the GPT family — migrate it transparently.
      if (saved?.family === 'openai') {
        setG0iFamily('gpt')
      } else if (saved?.family && (validFamily as string[]).includes(saved.family)) {
        setG0iFamily(saved.family as G0iFamily)
      } else if (saved?.family) {
        setG0iFamily('gemini')
      }
      if (saved?.model && G0I_MODELS.some((m) => m.id === saved.model)) {
        setG0iModel(saved.model)
      } else if (saved?.model) {
        setG0iModel('gemini-2.5-flash-new')
      }
      if (saved?.base_url) setG0iBaseUrl(saved.base_url)
    }).catch(() => {})
    invoke<any>(IPC.CONFIG_GET, 'openrouter').then((saved) => {
      if (saved?.api_key) {
        setOpenrouterKey(saved.api_key)
      } else {
        setOpenrouterKey(OPENROUTER_DEFAULT_KEY)
      }
      if (saved?.model) {
        setOpenrouterModel(saved.model)
      }
      if (saved?.base_url) {
        setOpenrouterBaseUrl(saved.base_url)
      }
      const keyToUse = saved?.api_key || OPENROUTER_DEFAULT_KEY
      if (keyToUse) {
        invoke<{ ok: boolean; models?: any[]; error?: string }>(IPC.OPENROUTER_LIST_MODELS, {
          api_key: keyToUse,
          base_url: saved?.base_url || OPENROUTER_DEFAULT_BASE_URL,
        }).then((res) => {
          if (res?.ok && Array.isArray(res.models) && res.models.length > 0) {
            setOpenrouterModels(res.models)
          }
        }).catch(() => {})
      }
    }).catch(() => {})
    invoke<any>(IPC.CONFIG_GET, 'aiGeneration').then((saved) => {
      if (['low', 'balanced', 'high'].includes(saved?.token_budget)) {
        setTokenBudget(saved.token_budget)
      }
    }).catch(() => {})
    refreshStatus()
    refreshRunning()
    refreshVoiceModels()
    fetchTokenStats()
  }, [])

  useEffect(() => {
    const unsub = onEvent(IPC.TTS_KOKORO_PROGRESS, (data: any) => {
      if (data.type === 'progress') {
        setVoiceProgress({
          percent: data.percent ?? 0,
          message: data.file ? `Downloading ${data.file}...` : data.stage === 'pip' ? 'Installing Python packages...' : 'Preparing voice models...',
        })
      } else if (data.type === 'info' && data.message) {
        setVoiceProgress((p) => ({ percent: p?.percent ?? 0, message: data.message }))
      } else if (data.type === 'done') {
        setVoiceInstalling(false)
        setVoiceProgress(null)
        refreshVoiceModels()
        setMessage({ type: 'success', text: data.message || 'AI voice models are ready' })
        setTimeout(() => setMessage(null), 4000)
      } else if (data.type === 'error') {
        setVoiceInstalling(false)
        setVoiceProgress(null)
        refreshVoiceModels()
        setMessage({ type: 'error', text: data.message || 'Voice model install failed' })
        setTimeout(() => setMessage(null), 5000)
      }
    })
    return unsub
  }, [])

  // Listen for pull progress events
  useEffect(() => {
    const unsub = onEvent(IPC.OLLAMA_PULL_PROGRESS, (data: any) => {
      if (data.model === pulling) {
        setPullProgress({ percent: data.percent || 0, status: data.status || '' })
        if (data.status === 'success') {
          setPulling(null)
          setPullProgress({ percent: 0, status: '' })
          setMessage({ type: 'success', text: `${data.model} installed successfully!` })
          setTimeout(() => setMessage(null), 4000)
          refreshStatus()
        }
      }
    })
    return unsub
  }, [pulling])

  const refreshStatus = async () => {
    try {
      const s = await invoke<{ online: boolean; models: number }>(IPC.OLLAMA_STATUS)
      setStatus(s)
      if (s.online) {
        await refreshAIModels()
      }
    } catch {
      setStatus({ online: false, models: 0 })
    }
  }

  const refreshVoiceModels = async () => {
    setVoiceChecking(true)
    try {
      const st = await invoke<VoiceModelStatus>(IPC.TTS_KOKORO_STATUS)
      setVoiceStatus(st)
    } catch (err: any) {
      setVoiceStatus({ installed: false, engine: 'none', error: err?.message || 'Status check failed' })
    } finally {
      setVoiceChecking(false)
    }
  }

  const installVoiceModels = async () => {
    if (voiceInstalling) return
    if (voiceStatus?.installed && voiceStatus.modelsReady && voiceStatus.depsReady) {
      await refreshVoiceModels()
      setMessage({ type: 'success', text: 'Voice models already installed. Status refreshed.' })
      setTimeout(() => setMessage(null), 3000)
      return
    }
    setVoiceInstalling(true)
    setVoiceProgress({ percent: 0, message: 'Checking existing model files...' })
    try {
      const res = await invoke<{ started?: boolean; error?: string }>(IPC.TTS_KOKORO_INSTALL)
      if (res?.error) {
        setVoiceInstalling(false)
        setVoiceProgress(null)
        setMessage({ type: 'error', text: res.error })
        setTimeout(() => setMessage(null), 5000)
      }
    } catch (err: any) {
      setVoiceInstalling(false)
      setVoiceProgress(null)
      setMessage({ type: 'error', text: err?.message || 'Could not start voice model install' })
      setTimeout(() => setMessage(null), 5000)
    }
  }

  const refreshRunning = async () => {
    try {
      const ps = await invoke<{ models?: { name: string; size: number; expires_at?: string }[] }>(IPC.OLLAMA_LIST_RUNNING)
      setRunningModels(ps?.models || [])
    } catch {
      setRunningModels([])
    }
  }

  const killModel = async (name: string) => {
    setKillingModel(name)
    try {
      await invoke(IPC.OLLAMA_KILL_MODEL, name)
      setMessage({ type: 'success', text: `${name} unloaded from memory` })
      setTimeout(() => setMessage(null), 3000)
    } catch (err: any) {
      setMessage({ type: 'error', text: `Failed to unload ${name}: ${err.message}` })
      setTimeout(() => setMessage(null), 4000)
    }
    setKillingModel(null)
    await refreshRunning()
  }

  const killAll = async () => {
    setKillingModel('__all__')
    try {
      const result = await invoke<{ killed: number }>(IPC.OLLAMA_KILL_ALL)
      setMessage({ type: 'success', text: `${result.killed} model${result.killed !== 1 ? 's' : ''} unloaded from memory` })
      setTimeout(() => setMessage(null), 3000)
    } catch (err: any) {
      setMessage({ type: 'error', text: `Failed: ${err.message}` })
      setTimeout(() => setMessage(null), 4000)
    }
    setKillingModel(null)
    await refreshRunning()
  }

  const testConnection = async () => {
    setTesting(true)
    await refreshStatus()
    await refreshRunning()
    setTesting(false)
  }

  const pullModel = async (modelName: string) => {
    // Force clear any stale pulling state
    if (pulling) {
      console.warn('[Settings] Clearing stale pulling state:', pulling)
      setPulling(null)
    }
    console.log('[Settings] Starting pull for:', modelName)
    console.log('[Settings] Ollama online:', status?.online)
    setPulling(modelName)
    setPullProgress({ percent: 0, status: 'Starting download...' })
    try {
      await invoke(IPC.OLLAMA_PULL_MODEL, modelName)
    } catch (err: any) {
      console.error('[Settings] Pull failed:', err)
      setMessage({ type: 'error', text: `Failed to pull ${modelName}: ${err.message}` })
      setTimeout(() => setMessage(null), 5000)
      setPulling(null)
      setPullProgress({ percent: 0, status: '' })
    }
    refreshStatus()
  }

  const deleteModel = async (modelName: string) => {
    try {
      await invoke(IPC.OLLAMA_DELETE_MODEL, modelName)
      setMessage({ type: 'success', text: `${modelName} removed` })
      setTimeout(() => setMessage(null), 3000)
      refreshStatus()
    } catch (err: any) {
      setMessage({ type: 'error', text: `Failed to delete: ${err.message}` })
      setTimeout(() => setMessage(null), 5000)
    }
  }

  const saveConfig = async () => {
    setSaving(true)
    try {
      // Save all configurations
      await invoke(IPC.CONFIG_SET, 'ollama', config)
      await Promise.all([
        setGlobalVisionModel(config.vision_model),
        setGlobalTextModel(config.text_model),
      ])
      if (geminiKey.trim()) {
        await invoke(IPC.CONFIG_SET, 'gemini', { api_key: geminiKey.trim(), model: geminiModel })
      }
      if (g0iKey.trim()) {
        await invoke(IPC.CONFIG_SET, 'g0i', {
          api_key: g0iKey.trim(),
          family: g0iFamily,
          model: g0iModel,
          base_url: g0iBaseUrl.trim() || G0I_DEFAULT_BASE_URL,
        })
      }
      if (openrouterKey.trim()) {
        await invoke(IPC.CONFIG_SET, 'openrouter', {
          api_key: openrouterKey.trim(),
          model: openrouterModel,
          base_url: openrouterBaseUrl.trim() || OPENROUTER_DEFAULT_BASE_URL,
        })
      }
      await invoke(IPC.CONFIG_SET, 'aiGeneration', { token_budget: tokenBudget })
      
      setMessage({ 
        type: 'success', 
        text: 'AI settings saved ✓ All API keys and preferences have been persisted' 
      })
    } catch (err: any) {
      setMessage({ 
        type: 'error', 
        text: `Failed to save settings: ${err.message || 'Unknown error'}` 
      })
    }
    setSaving(false)
    setTimeout(() => setMessage(null), 4000)
  }

  // When the family changes, reset the model to the first one in that family
  // so the dropdown never shows a stale label from another provider.
  const handleG0iFamilyChange = (next: G0iFamily) => {
    setG0iFamily(next)
    const firstInFamily = G0I_MODELS.find((m) => m.family === next)
    if (firstInFamily) setG0iModel(firstInFamily.id)
  }

  // Autosave g0i config
  useEffect(() => {
    const t = setTimeout(() => {
      invoke(IPC.CONFIG_SET, 'g0i', {
        api_key: g0iKey.trim(),
        family: g0iFamily,
        model: g0iModel,
        base_url: g0iBaseUrl.trim() || G0I_DEFAULT_BASE_URL,
      }).catch(() => { /* ok */ })
    }, 400)
    return () => clearTimeout(t)
  }, [g0iKey, g0iFamily, g0iModel, g0iBaseUrl])

  // Autosave openrouter config
  useEffect(() => {
    const t = setTimeout(() => {
      invoke(IPC.CONFIG_SET, 'openrouter', {
        api_key: openrouterKey.trim(),
        model: openrouterModel,
        base_url: openrouterBaseUrl.trim() || OPENROUTER_DEFAULT_BASE_URL,
      }).catch(() => { /* ok */ })
    }, 400)
    return () => clearTimeout(t)
  }, [openrouterKey, openrouterModel, openrouterBaseUrl])

  useEffect(() => {
    const t = setTimeout(() => {
      invoke(IPC.CONFIG_SET, 'aiGeneration', { token_budget: tokenBudget }).catch(() => { /* ok */ })
    }, 300)
    return () => clearTimeout(t)
  }, [tokenBudget])

  const handleG0iTest = async () => {
    setG0iTesting(true)
    setG0iTestResult(null)
    try {
      const result = await invoke<G0iTestResult>(IPC.G0I_TEST_KEY, {
        api_key: g0iKey.trim(),
        base_url: (g0iBaseUrl.trim() || G0I_DEFAULT_BASE_URL),
      })
      setG0iTestResult(result)
    } catch (err: any) {
      setG0iTestResult({
        ok: false,
        code: 'IPC_ERROR',
        message: 'Could not reach the main process to run the test.',
        detail: err?.message || String(err),
      })
    }
    setG0iTesting(false)
    setTimeout(() => setG0iTestResult((cur) => (cur?.ok ? null : cur)), 9000)
  }

  const handleOpenRouterTest = async () => {
    setOpenrouterTesting(true)
    setOpenrouterTestResult(null)
    try {
      const result = await invoke<any>(IPC.OPENROUTER_TEST_KEY, {
        api_key: openrouterKey.trim(),
        base_url: openrouterBaseUrl.trim() || OPENROUTER_DEFAULT_BASE_URL,
      })
      setOpenrouterTestResult(result)
      if (result.ok) {
        const modelsRes = await invoke<{ ok: boolean; models?: any[] }>(IPC.OPENROUTER_LIST_MODELS, {
          api_key: openrouterKey.trim(),
          base_url: openrouterBaseUrl.trim() || OPENROUTER_DEFAULT_BASE_URL,
        })
        if (modelsRes?.ok && Array.isArray(modelsRes.models) && modelsRes.models.length > 0) {
          setOpenrouterModels(modelsRes.models)
        }
      }
    } catch (err: any) {
      setOpenrouterTestResult({
        ok: false,
        code: 'IPC_ERROR',
        message: 'Could not reach the main process to run the test.',
        detail: err?.message || String(err),
      })
    }
    setOpenrouterTesting(false)
    setTimeout(() => setOpenrouterTestResult((cur: any) => (cur?.ok ? null : cur)), 9000)
  }

  if (loading) {
    return <div className="flex items-center gap-2 text-[var(--text-3)] text-sm py-8"><Loader2 size={14} className="animate-spin" /> Loading settings...</div>
  }

  const selectStyle = { background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }
  const isInstalled = (name: string) => allInstalledModels.some((m) => m.startsWith(name.split(':')[0]))
  const voiceReadyRows = [
    { label: 'F5 Studio', ready: !!voiceStatus?.f5Ready, path: voiceStatus?.f5Dir },
    { label: 'Parler Studio', ready: !!voiceStatus?.parlerReady, path: voiceStatus?.parlerDir },
    { label: 'XTTS Ultra', ready: !!voiceStatus?.xttsReady, path: voiceStatus?.xttsDir },
    { label: 'Kokoro Natural', ready: !!voiceStatus?.kokoroReady, path: voiceStatus?.modelDir },
    { label: 'Piper Hindi', ready: !!voiceStatus?.piperHiReady, path: voiceStatus?.piperHiDir },
    { label: 'Piper English', ready: !!voiceStatus?.piperEnReady, path: voiceStatus?.piperEnDir },
  ]

  let onlineTokens = 0
  let localTokens = 0
  if (tokenStats && tokenStats.byProvider) {
    for (const item of tokenStats.byProvider) {
      const p = item.provider.toLowerCase()
      if (p === 'ollama' || p === 'lmstudio' || p === 'lm-studio') {
        localTokens += item.tokens
      } else {
        onlineTokens += item.tokens
      }
    }
  }

  return (
    <div className="w-full space-y-6 fade-up">
      {message && (
        <div className={`flex items-center gap-2 rounded-xl px-4 py-2.5 text-sm ${message.type === 'success' ? 'text-[var(--success)]' : 'text-[var(--error)]'}`}
          style={{ background: message.type === 'success' ? 'rgba(var(--success-rgb),0.08)' : 'rgba(var(--error-rgb),0.08)', border: `1px solid ${message.type === 'success' ? 'rgba(var(--success-rgb),0.2)' : 'rgba(var(--error-rgb),0.2)'}` }}>
          {message.type === 'success' ? <Check size={14} /> : <AlertTriangle size={14} />}
          {message.text}
        </div>
      )}

      {/* ── AI Token Analytics ── */}
      <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
        <div className="flex items-center justify-between mb-3">
          <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
            <Activity size={14} className="text-emerald-400" />
            AI Token Analytics
          </h4>
          <button
            onClick={handleClearTokenUsage}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium text-[var(--text-2)] hover:text-rose-400 hover:bg-rose-500/10 transition-all"
            style={selectStyle}
          >
            <Trash2 size={12} />
            Clear Token Usage
          </button>
        </div>

        {tokenStats ? (
          <div className="space-y-4">
            {/* Total / Breakdown */}
            <div className="grid grid-cols-2 gap-4">
              <div className="rounded-lg p-3" style={{ background: 'var(--surface-3)' }}>
                <span className="text-[10px] text-[var(--text-3)] block uppercase tracking-wider">Online Tokens</span>
                <span className="text-lg font-bold text-[var(--text-1)] mt-1 block">{(onlineTokens || 0).toLocaleString()}</span>
                <span className="text-[10px] text-[var(--text-2)]">Gemini, OpenRouter, g0i</span>
              </div>
              <div className="rounded-lg p-3" style={{ background: 'var(--surface-3)' }}>
                <span className="text-[10px] text-[var(--text-3)] block uppercase tracking-wider">Local Tokens</span>
                <span className="text-lg font-bold text-[var(--text-1)] mt-1 block">{(localTokens || 0).toLocaleString()}</span>
                <span className="text-[10px] text-[var(--text-2)]">Ollama, LM Studio</span>
              </div>
            </div>

            {/* Visual Bar Graph */}
            {tokenStats.totalTokens > 0 && (
              <div>
                <div className="flex justify-between text-[10px] text-[var(--text-2)] mb-1.5">
                  <span>Online vs Local split</span>
                  <span>Total: {tokenStats.totalTokens.toLocaleString()} tokens</span>
                </div>
                <div className="h-2 rounded-full overflow-hidden flex" style={{ background: 'var(--surface-4)' }}>
                  {onlineTokens > 0 && (
                    <div
                      className="h-full bg-blue-500"
                      style={{ width: `${(onlineTokens / tokenStats.totalTokens) * 100}%` }}
                    />
                  )}
                  {localTokens > 0 && (
                    <div
                      className="h-full bg-emerald-500"
                      style={{ width: `${(localTokens / tokenStats.totalTokens) * 100}%` }}
                    />
                  )}
                </div>
                <div className="flex items-center gap-4 mt-2 text-[10px]">
                  <div className="flex items-center gap-1.5">
                    <span className="w-2 h-2 rounded-full bg-blue-500" />
                    <span className="text-[var(--text-3)]">Online ({((onlineTokens / tokenStats.totalTokens) * 100).toFixed(0)}%)</span>
                  </div>
                  <div className="flex items-center gap-1.5">
                    <span className="w-2 h-2 rounded-full bg-emerald-500" />
                    <span className="text-[var(--text-3)]">Local ({((localTokens / tokenStats.totalTokens) * 100).toFixed(0)}%)</span>
                  </div>
                </div>
              </div>
            )}

            {/* Provider List */}
            {tokenStats.byProvider && tokenStats.byProvider.length > 0 && (
              <div className="space-y-1.5">
                <span className="text-[11px] text-[var(--text-3)] block font-medium">Usage by Provider</span>
                {tokenStats.byProvider.map((prov: any) => (
                  <div
                    key={prov.provider}
                    className="flex items-center justify-between px-3 py-2 rounded-lg text-xs"
                    style={{ background: 'var(--panel-border)', border: '1px solid var(--panel-border)' }}
                  >
                    <span className="text-[var(--text-2)] font-medium capitalize">{prov.provider}</span>
                    <div className="text-right">
                      <span className="text-[var(--text-2)] font-semibold">{prov.tokens.toLocaleString()} tokens</span>
                      <span className="text-[10px] text-[var(--text-3)] block mt-0.5">{prov.count} request{prov.count !== 1 ? 's' : ''}</span>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        ) : (
          <p className="text-xs text-[var(--text-3)]">No token stats loaded.</p>
        )}
      </div>

      {/* ── AI Voice Models ── */}
      <div
        className="rounded-xl px-5 py-4"
        style={{
          background: 'var(--surface-2)',
          border: voiceStatus?.installed ? '1px solid rgba(34,197,94,0.18)' : '1px solid var(--panel-border)',
        }}
      >
        <div className="flex items-start justify-between gap-3 mb-3">
          <div>
            <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
              <Volume2 size={14} className="text-[var(--info)]" />
              AI Voice Models
              <span
                className="text-[10px] px-1.5 py-0.5 rounded-full font-medium"
                style={{
                  background: voiceStatus?.installed ? 'rgba(34,197,94,0.12)' : 'rgba(245,158,11,0.12)',
                  color: voiceStatus?.installed ? 'var(--success)' : 'var(--warning)',
                }}
              >
                {voiceChecking ? 'Checking' : voiceStatus?.installed ? 'Installed' : 'Needs setup'}
              </span>
            </h4>
            <p className="text-[11px] text-[var(--text-3)] mt-1">
              Used by Audio Studio for Hindi/English voices. Existing files are reused from <code className="text-[var(--text-2)]">~/.mangavox</code>; downloads are skipped when files are already present.
            </p>
          </div>
          <button
            onClick={refreshVoiceModels}
            disabled={voiceChecking || voiceInstalling}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium text-[var(--text-2)] hover:text-[var(--text-1)] transition-all disabled:opacity-40"
            style={selectStyle}
          >
            {voiceChecking ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
            Refresh
          </button>
        </div>

        <div className="grid grid-cols-2 gap-2 mb-3">
          {voiceReadyRows.map((row) => (
            <div
              key={row.label}
              className="rounded-lg px-3 py-2"
              style={{ background: 'var(--surface-3)', border: `1px solid ${row.ready ? 'rgba(34,197,94,0.18)' : 'var(--panel-border)'}` }}
            >
              <div className="flex items-center justify-between gap-2">
                <span className="text-xs font-medium text-[var(--text-2)]">{row.label}</span>
                <span className={`text-[10px] font-semibold ${row.ready ? 'text-[var(--success)]' : 'text-[var(--text-3)]'}`}>
                  {row.ready ? 'Ready' : 'Missing'}
                </span>
              </div>
              {row.path && <p className="text-[9px] text-[var(--text-3)] font-mono truncate mt-1" title={row.path}>{row.path}</p>}
            </div>
          ))}
        </div>

        <div className="rounded-lg px-3 py-2 mb-3" style={{ background: 'var(--panel-border)', border: '1px solid var(--panel-border)' }}>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2 text-[10px]">
            <div className="text-[var(--text-3)]">
              Active engine: <span className="text-[var(--text-2)] font-mono">{voiceStatus?.engine || 'none'}</span>
            </div>
            <div className="text-[var(--text-3)]">
              Size: <span className="text-[var(--text-2)] font-mono">{voiceStatus?.sizeMb ? `${voiceStatus.sizeMb} MB` : 'unknown'}</span>
            </div>
            <div className="text-[var(--text-3)] sm:col-span-2 truncate" title={voiceStatus?.activePython || voiceStatus?.venvPython || undefined}>
              Python: <span className="text-[var(--text-2)] font-mono">{voiceStatus?.activePython || voiceStatus?.venvPython || 'not detected'}</span>
            </div>
          </div>
          {voiceStatus?.error && <p className="text-[10px] text-amber-300 mt-2">{voiceStatus.error}</p>}
        </div>

        {voiceInstalling && voiceProgress && (
          <div className="mb-3">
            <div className="flex items-center justify-between text-[10px] text-[var(--text-2)] mb-1">
              <span>{voiceProgress.message}</span>
              <span className="font-mono">{voiceProgress.percent}%</span>
            </div>
            <div className="h-1.5 rounded-full overflow-hidden" style={{ background: 'var(--surface-3)' }}>
              <div className="h-full rounded-full transition-all" style={{ width: `${Math.max(voiceProgress.percent, 4)}%`, background: 'var(--blue-gradient)' }} />
            </div>
          </div>
        )}

        <div className="flex flex-wrap gap-2">
          <button
            onClick={installVoiceModels}
            disabled={voiceInstalling}
            className="flex items-center gap-2 px-4 py-2 rounded-lg text-xs font-semibold text-white transition-all hover:brightness-110 disabled:opacity-50"
            style={{ background: 'var(--blue-gradient)' }}
          >
            {voiceInstalling ? <Loader2 size={13} className="animate-spin" /> : voiceStatus?.installed ? <Check size={13} /> : <CloudDownload size={13} />}
            {voiceInstalling ? 'Installing...' : voiceStatus?.installed ? 'Installed - verify paths' : 'Download missing voice models'}
          </button>
          <p className="text-[10px] text-[var(--text-3)] self-center">
            Audio Studio will use these paths automatically and only fall back when a selected premium engine is genuinely missing.
          </p>
        </div>
      </div>

      {/* ── Puter (Free Gemini, no API key) ── */}
      <div
        className="rounded-xl px-5 py-4"
        style={{
          background: 'var(--surface-2)',
          border: puterUser ? '1px solid rgba(99,102,241,0.25)' : '1px solid var(--panel-border)',
        }}
      >
        <div className="flex items-center justify-between mb-1">
          <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
            <Brain size={14} className="text-emerald-400" />
            Puter — Free Gemini Vision
            <span className="text-[10px] px-1.5 py-0.5 rounded-full font-medium"
              style={{ background: 'var(--accent-gradient)', color: 'white' }}>
              Free · No API Key
            </span>
          </h4>
          {puterUser && (
            <span className="text-[10px] text-[var(--success)] flex items-center gap-1">
              <Check size={10} /> Signed in
            </span>
          )}
        </div>
        <p className="text-[11px] text-[var(--text-3)] mb-3">
          Use Gemini 2.5 Flash via your free Puter account — unlimited within Puter's quota, no credit card.
          Sign in once, browser-side OAuth (Google / GitHub / email).
        </p>

        {puterUser ? (
          <div className="flex items-center justify-between gap-3 mb-3 px-3 py-2 rounded-lg"
            style={{ background: 'var(--surface-3)' }}>
            <div className="min-w-0">
              <div className="text-xs text-[var(--text-2)] font-medium truncate">{puterUser.name}</div>
              {puterUser.email && (
                <div className="text-[10px] text-[var(--text-3)] truncate">{puterUser.email}</div>
              )}
            </div>
            <button
              onClick={handlePuterSignOut}
              className="text-[11px] font-medium text-[var(--text-2)] hover:text-emerald-300 px-2 py-1 rounded-md transition-colors shrink-0"
              style={{ background: 'var(--panel-border)' }}
            >
              Sign out
            </button>
          </div>
        ) : (
          <button
            onClick={handlePuterSignIn}
            disabled={puterChecking}
            className="w-full flex items-center justify-center gap-2 h-10 rounded-lg text-sm font-semibold text-white transition-all hover:brightness-110 disabled:opacity-50 mb-3"
            style={{ background: 'var(--accent-gradient)' }}
          >
            {puterChecking ? <Loader2 size={14} className="animate-spin" /> : <ExternalLink size={14} />}
            Sign in to Puter
          </button>
        )}

        <div className="space-y-2">
          <label className="text-[11px] text-[var(--text-3)] block">Default vision model</label>
          <select
            value={puterModel}
            onChange={(e) => setPuterModel(e.target.value)}
            className="w-full h-9 px-3 rounded-lg text-xs text-[var(--text-2)] outline-none"
            style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
          >
            {PUTER_VISION_MODELS.map((m) => (
              <option key={m.id} value={m.id}>{m.label} — {m.desc}</option>
            ))}
          </select>
          <p className="text-[10px] text-[var(--text-3)]">
            Once signed in, pick <strong className="text-emerald-300">Puter — Free Gemini</strong> in the Text tab's engine dropdown to extract pages.
          </p>
        </div>
      </div>

      {/* ── g0i.ai Universal Gateway (one key, many models) ── */}
      <div
        className="rounded-xl px-5 py-4"
        style={{
          background: 'var(--surface-2)',
          border: g0iKey
            ? '1px solid rgba(239,68,68,0.30)'
            : '1px solid var(--panel-border)',
        }}
      >
        <div className="flex items-center justify-between mb-1">
          <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
            <Brain size={14} style={{ color: 'var(--accent)' }} />
            g0i.ai — Universal AI Gateway
            <span
              className="text-[10px] px-1.5 py-0.5 rounded-full font-medium"
              style={{ background: 'var(--accent-gradient)', color: 'white' }}
            >
              Gemini · Claude · ChatGPT
            </span>
          </h4>
          <div className="flex items-center gap-1.5">
            {g0iKey && <span className="text-[10px] text-[var(--success)]">Active</span>}
            <span
              className="text-[9px] uppercase tracking-wider font-semibold px-1.5 py-0.5 rounded-full"
              style={{ background: 'rgba(var(--success-rgb),0.1)', color: 'var(--success)' }}
              title="Changes save automatically"
            >
              Autosave
            </span>
          </div>
        </div>
        <p className="text-[11px] text-[var(--text-3)] mb-3">
          One key that unlocks Gemini, Claude (Opus) and ChatGPT (GPT-5 family).
          Get yours at{' '}
          <a
            href="#"
            onClick={(e) => { e.preventDefault(); (window as any).electron?.openExternal?.('https://g0i.ai/api-keys') }}
            className="hover:underline"
            style={{ color: 'var(--accent-hover)' }}
          >
            g0i.ai/api-keys
          </a>{' '}
          — works with the same OpenAI-compatible API everywhere.
        </p>

        {/* Empty-key state. We no longer pre-fill a sample key (its trial is
            dead upstream), so the user must paste one or fall back to one of
            the working free options below. */}
        {!g0iKey.trim() && (
          <div
            className="mb-3 rounded-lg px-3 py-2.5 flex items-start gap-2.5"
            style={{ background: 'rgba(96,165,250,0.06)', border: '1px solid rgba(96,165,250,0.20)' }}
          >
            <Info size={13} className="text-[var(--info)] shrink-0 mt-0.5" />
            <div className="min-w-0">
              <p className="text-[11px] font-semibold text-[var(--info)] leading-tight">
                No g0i.ai key set — narration via g0i is disabled
              </p>
              <p className="text-[10px] text-[var(--text-2)] leading-relaxed mt-0.5">
                Get a free trial key at{' '}
                <a
                  href="#"
                  onClick={(e) => { e.preventDefault(); (window as any).electron?.openExternal?.('https://g0i.ai/api-keys') }}
                  className="underline"
                  style={{ color: 'var(--accent-hover)' }}
                >
                  g0i.ai/api-keys
                </a>
                {' '}and paste it above. Or skip g0i entirely — the{' '}
                <strong className="text-[var(--text-2)]">Google Gemini API</strong> card below is free, fast, and has no
                trial. Local <strong className="text-[var(--text-2)]">Ollama</strong> also works for offline use.
              </p>
            </div>
          </div>
        )}

        <div className="space-y-3">
          {/* API key + Test button */}
          <div>
            <label className="text-[11px] text-[var(--text-3)] block mb-1.5">API Key</label>
            <div className="flex gap-2">
              <input
                type="password"
                value={g0iKey}
                onChange={(e) => setG0iKey(e.target.value)}
                className="flex-1 h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none font-mono"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                placeholder="sk-... (paste your g0i.ai key)"
              />
              <button
                onClick={handleG0iTest}
                disabled={g0iTesting || !g0iKey.trim()}
                className="flex items-center gap-1.5 px-3 h-9 rounded-lg text-xs font-semibold text-white transition-all disabled:opacity-40"
                style={{ background: 'var(--accent-gradient)' }}
                title="Hit GET /v1/models to verify the key without spending tokens"
              >
                {g0iTesting ? <Loader2 size={12} className="animate-spin" /> : <Zap size={12} />}
                {g0iTesting ? 'Testing…' : 'Test key'}
              </button>
            </div>
            {g0iTestResult && (
              <G0iTestResultCard
                result={g0iTestResult}
                onDismiss={() => setG0iTestResult(null)}
                onRetry={handleG0iTest}
              />
            )}
            <p className="text-[10px] text-[var(--text-3)] mt-1">
              A demo key is pre-filled so you can try it immediately — replace it with your own for production use.
            </p>
          </div>

          {/* Provider family selector */}
          <div>
            <label className="text-[11px] text-[var(--text-3)] block mb-1.5">Provider Family</label>
            <div className="grid grid-cols-2 sm:grid-cols-5 gap-2">
              {([
                { id: 'gemini'   as const, label: 'Gemini',   desc: 'Google · recommended' },
                { id: 'gpt'      as const, label: 'GPT-5',    desc: 'OpenAI · vision' },
                { id: 'claude'   as const, label: 'Claude',   desc: 'Sonnet / Opus · paid' },
                { id: 'kimi'     as const, label: 'Kimi',     desc: 'Moonshot · multimodal' },
                { id: 'deepseek' as const, label: 'DeepSeek', desc: 'Text only' },
              ]).map((f) => {
                const active = g0iFamily === f.id
                return (
                  <button
                    key={f.id}
                    onClick={() => handleG0iFamilyChange(f.id)}
                    className={`px-3 py-2 rounded-lg text-left transition-all ${active ? '' : 'hover:bg-[var(--surface-3)]'}`}
                    style={{
                      background: active ? 'rgba(239,68,68,0.12)' : 'var(--surface-3)',
                      border: active ? '1px solid rgba(239,68,68,0.35)' : '1px solid transparent',
                    }}
                  >
                    <div className={`text-xs font-semibold ${active ? 'text-white' : 'text-[var(--text-2)]'}`}>{f.label}</div>
                    <div className="text-[10px] text-[var(--text-3)] mt-0.5">{f.desc}</div>
                  </button>
                )
              })}
            </div>
          </div>

          {/* Model dropdown filtered to the chosen family */}
          <div>
            <label className="text-[11px] text-[var(--text-3)] block mb-1.5">Model</label>
            <select
              value={g0iModel}
              onChange={(e) => setG0iModel(e.target.value)}
              className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none appearance-none cursor-pointer"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
            >
              {G0I_MODELS.filter((m) => m.family === g0iFamily).map((m) => {
                const tag = !m.vision
                  ? ' [text only]'
                  : m.requiresUpgrade
                    ? ' [Opus plan]'
                    : m.visionUnstable
                      ? ' [vision unstable]'
                      : m.recommended
                        ? ' [recommended]'
                        : ''
                return (
                  <option key={m.id} value={m.id}>{m.label}{tag} — {m.desc}</option>
                )
              })}
            </select>
            {(() => {
              const current = G0I_MODELS.find((m) => m.id === g0iModel)
              if (!current) return null
              if (!current.vision) {
                return (
                  <p className="text-[10px] mt-1 flex items-center gap-1.5" style={{ color: 'var(--warning)' }}>
                    <AlertTriangle size={11} />
                    This model is <strong>text-only</strong> — narration needs vision. Pick a GPT-5.4 / Kimi K2 model instead.
                  </p>
                )
              }
              if (current.requiresUpgrade) {
                return (
                  <p className="text-[10px] mt-1 flex items-center gap-1.5" style={{ color: 'var(--warning)' }}>
                    <AlertTriangle size={11} />
                    Needs g0i's <strong>Opus-tier plan</strong>. Free keys get 403 — upgrade at g0i.ai/pricing.
                  </p>
                )
              }
              if (current.visionUnstable) {
                return (
                  <p className="text-[10px] mt-1 flex items-center gap-1.5" style={{ color: 'var(--warning)' }}>
                    <AlertTriangle size={11} />
                    Vision endpoint on g0i returns 500 right now for this model — text works, but narration may fail. Pick <strong>Gemini 2.5 Flash</strong> for reliable manga reads.
                  </p>
                )
              }
              if (current.recommended) {
                return (
                  <p className="text-[10px] mt-1 flex items-center gap-1.5" style={{ color: 'var(--error)' }}>
                    <Check size={11} />
                    Recommended for manga narration — verified vision endpoint, fast, balanced cost.
                  </p>
                )
              }
              return (
                <p className="text-[10px] text-[var(--text-3)] mt-1">
                  Used when you pick <strong style={{ color: 'var(--accent-hover)' }}>g0i.ai</strong> as the engine in Text Studio.
                </p>
              )
            })()}
          </div>

          {/* Advanced (custom base URL) */}
          <button
            onClick={() => setG0iAdvancedOpen((v) => !v)}
            className="text-[11px] text-[var(--text-3)] hover:text-[var(--text-2)] transition-colors"
          >
            {g0iAdvancedOpen ? '▾' : '▸'} Advanced
          </button>
          {g0iAdvancedOpen && (
            <div>
              <label className="text-[11px] text-[var(--text-3)] block mb-1.5">Base URL</label>
              <input
                type="text"
                value={g0iBaseUrl}
                onChange={(e) => setG0iBaseUrl(e.target.value)}
                className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none font-mono"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                placeholder={G0I_DEFAULT_BASE_URL}
              />
              <p className="text-[10px] text-[var(--text-3)] mt-1">
                Override only if g0i.ai moves their endpoint. Default: <code>{G0I_DEFAULT_BASE_URL}</code>
              </p>
            </div>
          )}

          {/* Save hint */}
          {g0iKey && (
            <p className="text-[10px] text-[var(--text-3)]">
              Tip: hit <strong className="text-[var(--text-2)]">Save AI Settings</strong> below to apply changes.
            </p>
          )}
        </div>
      </div>

      {/* ── OpenRouter Universal API (one key, many models) ── */}
      <div
        className="rounded-xl px-5 py-4"
        style={{
          background: 'var(--surface-2)',
          border: openrouterKey
            ? '1px solid rgba(59,130,246,0.30)'
            : '1px solid var(--panel-border)',
        }}
      >
        <div className="flex items-center justify-between mb-1">
          <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
            <Brain size={14} style={{ color: 'var(--accent)' }} />
            OpenRouter — Universal API
            <span
              className="text-[10px] px-1.5 py-0.5 rounded-full font-medium"
              style={{ background: 'var(--accent-gradient)', color: 'white' }}
            >
              Gemini · Claude · Llama · Qwen
            </span>
          </h4>
          <div className="flex items-center gap-1.5">
            {openrouterKey && <span className="text-[10px] text-[var(--success)]">Active</span>}
            <span
              className="text-[9px] uppercase tracking-wider font-semibold px-1.5 py-0.5 rounded-full"
              style={{ background: 'rgba(var(--success-rgb),0.1)', color: 'var(--success)' }}
              title="Changes save automatically"
            >
              Autosave
            </span>
          </div>
        </div>
        <p className="text-[11px] text-[var(--text-3)] mb-3">
          Access dozens of open-source and proprietary models under a single API key.
          Get yours at{' '}
          <a
            href="#"
            onClick={(e) => { e.preventDefault(); (window as any).electron?.openExternal?.('https://openrouter.ai/keys') }}
            className="hover:underline"
            style={{ color: 'var(--accent-hover)' }}
          >
            openrouter.ai/keys
          </a>{' '}
          — works with a pre-configured key for direct narration.
        </p>

        <div className="space-y-3">
          {/* API key + Test button */}
          <div>
            <label className="text-[11px] text-[var(--text-3)] block mb-1.5">API Key</label>
            <div className="flex gap-2">
              <input
                type="password"
                value={openrouterKey}
                onChange={(e) => setOpenrouterKey(e.target.value)}
                className="flex-1 h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none font-mono"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                placeholder="sk-or-... (paste your OpenRouter key)"
              />
              <button
                onClick={handleOpenRouterTest}
                disabled={openrouterTesting || !openrouterKey.trim()}
                className="flex items-center gap-1.5 px-3 h-9 rounded-lg text-xs font-semibold text-white transition-all disabled:opacity-40"
                style={{ background: 'var(--accent-gradient)' }}
                title="Verify key connectivity and refresh model list"
              >
                {openrouterTesting ? <Loader2 size={12} className="animate-spin" /> : <Zap size={12} />}
                {openrouterTesting ? 'Testing…' : 'Test key'}
              </button>
            </div>
            {openrouterTestResult && (
              <div
                className="mt-3 rounded-lg px-3 py-2.5 flex items-start gap-2.5 text-xs"
                style={{
                  background: openrouterTestResult.ok ? 'rgba(200, 80, 26,0.06)' : 'rgba(239,68,68,0.06)',
                  border: openrouterTestResult.ok ? '1px solid rgba(200, 80, 26,0.2)' : '1px solid rgba(239,68,68,0.2)',
                }}
              >
                <div className="flex-1">
                  <p className="font-semibold" style={{ color: openrouterTestResult.ok ? 'var(--success)' : 'var(--error)' }}>
                    {openrouterTestResult.ok ? 'Connection Successful ✓' : 'Connection Failed ✗'}
                  </p>
                  <p className="text-[var(--text-2)] mt-0.5 leading-relaxed">{openrouterTestResult.message}</p>
                  {openrouterTestResult.detail && (
                    <pre className="mt-1.5 p-1.5 rounded bg-black/30 font-mono text-[10px] text-[var(--text-3)] overflow-x-auto">
                      {openrouterTestResult.detail}
                    </pre>
                  )}
                </div>
              </div>
            )}
            <p className="text-[10px] text-[var(--text-3)] mt-1">
              A fallback key is pre-filled. You can also paste your own.
            </p>
          </div>

          {/* Model picker */}
          <div>
            <label className="text-[11px] text-[var(--text-3)] block mb-1.5">Model</label>
            <select
              value={openrouterModel}
              onChange={(e) => setOpenrouterModel(e.target.value)}
              className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none appearance-none cursor-pointer"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
            >
              {openrouterModels.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name || m.id}
                </option>
              ))}
            </select>
            <p className="text-[10px] text-[var(--text-3)] mt-1">
              Used when you pick <strong style={{ color: 'var(--accent-hover)' }}>OpenRouter</strong> as the engine in Text Studio.
            </p>
          </div>

          {/* Advanced (custom base URL) */}
          <button
            onClick={() => setOpenrouterAdvancedOpen((v) => !v)}
            className="text-[11px] text-[var(--text-3)] hover:text-[var(--text-2)] transition-colors"
          >
            {openrouterAdvancedOpen ? '▾' : '▸'} Advanced
          </button>
          {openrouterAdvancedOpen && (
            <div>
              <label className="text-[11px] text-[var(--text-3)] block mb-1.5">Base URL</label>
              <input
                type="text"
                value={openrouterBaseUrl}
                onChange={(e) => setOpenrouterBaseUrl(e.target.value)}
                className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none font-mono"
                style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
                placeholder={OPENROUTER_DEFAULT_BASE_URL}
              />
              <p className="text-[10px] text-[var(--text-3)] mt-1">
                Default OpenRouter base URL is <code>{OPENROUTER_DEFAULT_BASE_URL}</code>.
              </p>
            </div>
          )}

          {/* Save hint */}
          {openrouterKey && (
            <p className="text-[10px] text-[var(--text-3)]">
              Tip: hit <strong className="text-[var(--text-2)]">Save AI Settings</strong> below to apply changes.
            </p>
          )}
        </div>
      </div>

      {/* ── Web Search Providers (better results + real images everywhere) ── */}
      {(() => {
        const anySearch = Object.values(searchCfg).some((v) => (v || '').trim())
        const field = (k: keyof typeof searchCfg, label: string, ph: string) => (
          <div>
            <label className="text-[11px] text-[var(--text-3)] block mb-1.5">{label}</label>
            <input
              type="password"
              value={searchCfg[k]}
              onChange={(e) => setSearchCfg((p) => ({ ...p, [k]: e.target.value }))}
              className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
              placeholder={ph}
            />
          </div>
        )
        return (
          <div className="rounded-xl px-5 py-4 mb-4" style={{ background: 'var(--surface-2)', border: anySearch ? '1px solid rgba(200, 80, 26,0.15)' : '1px solid var(--panel-border)' }}>
            <div className="flex items-center justify-between mb-1">
              <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
                <Search size={14} className="text-emerald-400" />
                Web Search Providers
                <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-emerald-500/10 text-emerald-400 font-medium">Optional</span>
              </h4>
              {anySearch && <span className="text-[10px] text-[var(--success)]">Active</span>}
            </div>
            <p className="text-[11px] text-[var(--text-3)] mb-3">
              Add any of these keys to make the assistant's research (chat, <strong className="text-[var(--text-2)]">/deepresearch</strong>, recommendations) far more accurate and pull <strong className="text-[var(--text-2)]">real images</strong>. Without keys it still works using free DuckDuckGo + Wikipedia. <strong className="text-emerald-300">Tavily</strong> is recommended for AI research.
            </p>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
              {field('tavily', 'Tavily API key (recommended)', 'tvly-…  ·  tavily.com')}
              {field('brave', 'Brave Search API key', 'BSA…  ·  brave.com/search/api')}
              {field('googleKey', 'Google Programmable Search — API key', 'AIza…  ·  developers.google.com/custom-search')}
              {field('googleCx', 'Google — Search engine ID (cx)', 'e.g. 0123:abcd')}
              {field('bing', 'Bing Web Search key', 'Ocp-Apim key  ·  azure.microsoft.com')}
              {field('serpapi', 'SerpAPI key', 'serpapi.com')}
            </div>
            <div className="flex items-center gap-2 mt-3">
              <button onClick={() => void saveSearchCfg()} className="mv-btn-primary !py-2 !px-4 text-xs">
                {searchSaved ? 'Saved ✓' : 'Save search keys'}
              </button>
              <span className="text-[10px] text-[var(--text-3)]">Keys are stored locally and used automatically everywhere research runs.</span>
            </div>
          </div>
        )
      })()}

      {/* ── Gemini API (your own key) ── */}
      <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)', border: geminiKey ? '1px solid rgba(200, 80, 26,0.15)' : '1px solid var(--panel-border)' }}>
        <div className="flex items-center justify-between mb-1">
          <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
            <Zap size={14} className="text-emerald-400" />
            Google Gemini API
            <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-emerald-500/10 text-emerald-400 font-medium">Recommended</span>
          </h4>
          {geminiKey && <span className="text-[10px] text-[var(--success)]">Active</span>}
        </div>
        <p className="text-[11px] text-[var(--text-3)] mb-3">
          Free API with excellent manga reading. Get a key from{' '}
          <a href="#" onClick={(e) => { e.preventDefault(); (window as any).electron?.openExternal?.('https://aistudio.google.com/apikey') }}
            className="text-emerald-400 hover:underline">Google AI Studio</a>
          {' '}— no credit card needed, 15 requests/min free.
        </p>
        <div className="space-y-3">
          <div>
            <label className="text-[11px] text-[var(--text-3)] block mb-1.5">API Key</label>
            <input
              type="password"
              value={geminiKey}
              onChange={(e) => setGeminiKey(e.target.value)}
              className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
              placeholder="Paste your Gemini API key here"
            />
          </div>
          <div>
            <label className="text-[11px] text-[var(--text-3)] block mb-1.5">Model Version</label>
            <select
              value={geminiModel}
              onChange={(e) => setGeminiModel(e.target.value)}
              className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none appearance-none cursor-pointer"
              style={{ background: 'var(--surface-3)', border: '1px solid var(--panel-border)' }}
            >
              {GEMINI_MODELS.map((m) => (
                <option key={m.id} value={m.id}>{m.label} — {m.desc}</option>
              ))}
            </select>
            <p className="text-[10px] text-[var(--text-3)] mt-1">
              Select which Gemini model to use for text extraction. <strong className="text-emerald-300">Gemini 2.5 Flash</strong> is recommended for manga.
            </p>
          </div>
        </div>
        {!geminiKey && (
          <p className="text-[10px] text-[var(--text-3)] mt-2">
            Without an API key, OCR falls back to Ollama (local, slower, lower quality)
          </p>
        )}
      </div>

      {/* ── Ollama Connection (fallback) ── */}
      <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
        <div className="flex items-center justify-between mb-3">
          <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
            <Cpu size={14} className="text-emerald-400" />
            Ollama Server {geminiKey && <span className="text-[10px] text-[var(--text-3)]">(fallback)</span>}
          </h4>
          <div className="flex items-center gap-2">
            <div className={`w-2 h-2 rounded-full ${status?.online ? 'bg-emerald-400' : 'bg-red-400'}`} />
            <span className="text-xs text-[var(--text-3)]">
              {status?.online ? `Online · ${status.models} models` : 'Offline'}
            </span>
          </div>
        </div>

        <div className="flex items-center gap-3">
          <div className="flex-1">
            <label className="block text-xs text-[var(--text-3)] mb-1">Server URL</label>
            <input
              type="text"
              value={config.url}
              onChange={(e) => setConfig({ ...config, url: e.target.value })}
              className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none"
              style={selectStyle}
              placeholder="http://127.0.0.1:11434"
            />
          </div>
          <button
            onClick={testConnection}
            disabled={testing}
            className="mt-5 flex items-center gap-1.5 px-3 py-2 rounded-lg text-xs font-medium text-[var(--text-2)] hover:text-[var(--text-1)] transition-all"
            style={selectStyle}
          >
            {testing ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
            Test
          </button>
        </div>

        {!status?.online && (
          <p className="text-xs text-[var(--text-3)] mt-2">
            Run <code className="px-1 py-0.5 rounded bg-[var(--surface-3)] text-[var(--text-2)]">ollama serve</code> to start the Ollama server
          </p>
        )}
      </div>

      {/* ── Running LLMs (VRAM/RAM usage) ── */}
      {status?.online && (
        <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
          <div className="flex items-center justify-between mb-3">
            <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2">
              <Activity size={14} className="text-emerald-400" />
              Running Models
              {runningModels.length > 0 && (
                <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-emerald-500/15 text-emerald-400 font-semibold">
                  {runningModels.length} active
                </span>
              )}
            </h4>
            <div className="flex items-center gap-2">
              <button
                onClick={refreshRunning}
                className="flex items-center gap-1 px-2 py-1 rounded-md text-[10px] text-[var(--text-2)] hover:text-[var(--text-1)] transition-colors"
                style={{ background: 'var(--panel-border)' }}
              >
                <RefreshCw size={10} /> Refresh
              </button>
              {runningModels.length > 0 && (
                <button
                  onClick={killAll}
                  disabled={killingModel !== null}
                  className="flex items-center gap-1 px-2 py-1 rounded-md text-[10px] text-emerald-400 hover:text-emerald-300 hover:bg-emerald-500/10 transition-colors"
                  style={{ background: 'rgba(239,68,68,0.08)' }}
                >
                  {killingModel === '__all__' ? <Loader2 size={10} className="animate-spin" /> : <Square size={10} />}
                  Kill All
                </button>
              )}
            </div>
          </div>

          {runningModels.length === 0 ? (
            <p className="text-xs text-[var(--text-3)] py-2">
              No models currently loaded in memory. Models are loaded when you use the terminal agent or text extraction, and auto-unload after idle.
            </p>
          ) : (
            <div className="space-y-1.5">
              {runningModels.map((m) => (
                <div
                  key={m.name}
                  className="flex items-center gap-3 px-3 py-2 rounded-lg"
                  style={{ background: 'var(--panel-border)', border: '1px solid var(--panel-border)' }}
                >
                  <div className="w-2 h-2 rounded-full bg-emerald-400 animate-pulse shrink-0" />
                  <div className="flex-1 min-w-0">
                    <p className="text-xs text-[var(--text-2)] font-medium truncate">{m.name}</p>
                    <p className="text-[10px] text-[var(--text-3)]">
                      {m.size ? `${(m.size / 1e9).toFixed(1)} GB VRAM` : 'Unknown size'}
                    </p>
                  </div>
                  <button
                    onClick={() => killModel(m.name)}
                    disabled={killingModel !== null}
                    className="flex items-center gap-1 px-2 py-1 rounded-md text-[10px] font-medium text-emerald-400 hover:text-emerald-300 hover:bg-emerald-500/10 transition-colors shrink-0"
                    style={{ background: 'rgba(239,68,68,0.08)' }}
                    title={`Unload ${m.name} from memory`}
                  >
                    {killingModel === m.name ? <Loader2 size={10} className="animate-spin" /> : <Square size={10} />}
                    Unload
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {systemProfile && (
        <div className="mv-card-glass px-5 py-4">
          <div className="flex items-start justify-between gap-4">
            <div>
              <h4 className="text-sm font-semibold text-[var(--text-1)] flex items-center gap-2">
                <Cpu size={14} style={{ color: 'var(--accent-hover)' }} />
                Detected hardware
              </h4>
              <p className="text-[11px] text-[var(--text-3)] mt-1">
                {systemProfile.cpu} · {systemProfile.cpuCores} cores · {systemProfile.ramGb} GB RAM
              </p>
              <p className="text-[11px] text-[var(--text-3)] mt-0.5">
                {systemProfile.gpu} · {systemProfile.availableModelMemoryGb} GB conservative model budget
              </p>
            </div>
            <span className="text-[10px] uppercase tracking-wider px-2.5 py-1 rounded-full"
              style={{ color: 'var(--accent-hover)', background: 'var(--accent-glow-soft)', border: '1px solid var(--panel-border-strong)' }}>
              {systemProfile.tier}
            </span>
          </div>
        </div>
      )}

      {/* ── Vision Models (for text extraction) ── */}
      <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
        <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2 mb-1">
          <Eye size={14} className="text-emerald-400" />
          Vision Models
          <span className="text-[10px] text-emerald-400 font-normal ml-1 px-1.5 py-0.5 rounded-full" style={{ background: 'rgba(220, 38, 38,0.12)' }}>
            needed for Text Studio
          </span>
        </h4>
        <p className="text-[11px] text-[var(--text-3)] mb-4">
          These local models can see and read manga pages. All free — runs 100% on your Mac.
        </p>

        {/* Big warning banner when no vision model installed */}
        {visionModels.length === 0 && status?.online && (
          <div className="mb-4 rounded-xl px-4 py-3.5 flex items-start gap-3"
            style={{ background: 'rgba(245,158,11,0.08)', border: '1px solid rgba(245,158,11,0.2)' }}>
            <AlertTriangle size={16} className="text-amber-400 shrink-0 mt-0.5" />
            <div>
              <p className="text-sm font-medium text-amber-300">No vision model installed</p>
              <p className="text-xs text-amber-400/70 mt-0.5">
                Text extraction won't work until you download a vision model below. We recommend <strong>minicpm-v:8b</strong> (5.5 GB) — best at reading manga text.
              </p>
            </div>
          </div>
        )}

        {/* Active model selector */}
        <div className="mb-4">
          <label className="block text-xs text-[var(--text-3)] mb-1">Active Vision Model</label>
          <select
            value={modelPreferences.visionModel || config.vision_model}
            onChange={(e) => {
              setConfig({ ...config, vision_model: e.target.value })
              void setGlobalVisionModel(e.target.value)
            }}
            className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none appearance-none cursor-pointer"
            style={selectStyle}
          >
            {visionModels.length > 0 ? (
              visionModels.map((m) => <option key={m} value={m}>{m}</option>)
            ) : (
              <option value={config.vision_model}>{config.vision_model} (not installed — download below)</option>
            )}
          </select>
        </div>

        {/* Recommended vision models to download */}
        <div className="space-y-1.5">
          {recommendedVisionModels.map((model) => {
            const installed = allInstalledModels.some((m) => m === model.name || m.startsWith(model.name.split(':')[0] + ':'))
            const isPulling = pulling === model.name
            return (
              <div
                key={model.name}
                className="flex items-center justify-between px-3 py-3 rounded-lg"
                style={{
                  background: 'var(--surface-3)',
                  border: installed && config.vision_model === model.name
                    ? '1px solid rgba(220, 38, 38,0.3)'
                    : '1px solid transparent',
                }}
              >
                <div className="min-w-0 flex-1 mr-3">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="text-xs font-semibold text-[var(--text-1)]">{model.name}</span>
                    <span className="text-[10px] text-[var(--text-3)]">{model.downloadGb.toFixed(1)} GB</span>
                    {model.recommended && (
                      <span className="text-[9px] font-bold text-emerald-300 px-1.5 py-0.5 rounded-full"
                        style={{ background: 'rgba(220, 38, 38,0.18)' }}>
                        Recommended
                      </span>
                    )}
                    {installed && (
                      <span className="text-[9px] font-bold text-emerald-400 px-1.5 py-0.5 rounded-full bg-emerald-500/10">
                        INSTALLED
                      </span>
                    )}
                  </div>
                  <p className="text-[10px] text-[var(--text-3)] mt-0.5">{model.description}</p>
                  <p className={`text-[9px] mt-0.5 ${model.compatible ? 'text-emerald-400/70' : 'text-amber-400/80'}`}>{model.reason}</p>
                  {isPulling && (
                    <div className="mt-2">
                      <div className="h-1.5 rounded-full overflow-hidden" style={{ background: 'var(--surface-2)' }}>
                        <div
                          className="h-full rounded-full transition-all duration-300"
                          style={{ width: `${pullProgress.percent}%`, background: 'var(--accent-gradient)' }}
                        />
                      </div>
                      <span className="text-[9px] text-[var(--text-2)] mt-0.5 block">
                        {pullProgress.status}{pullProgress.percent > 0 ? ` · ${pullProgress.percent}%` : ''}
                      </span>
                    </div>
                  )}
                </div>
                {!installed && !isPulling && (
                  <button
                    onClick={() => pullModel(model.name)}
                    disabled={!!pulling || !status?.online || !model.compatible}
                    className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold transition-all shrink-0"
                    style={{
                      background: !!pulling || !status?.online ? 'var(--surface-4)' : 'var(--accent-gradient)',
                      color: !!pulling || !status?.online ? 'text-[var(--text-3)]' : 'text-white',
                      opacity: !!pulling || !status?.online || !model.compatible ? 0.5 : 1,
                    }}
                    title={status?.online ? (pulling ? 'Another model is being downloaded' : 'Download from Ollama') : 'Ollama must be running to download models'}
                  >
                    <CloudDownload size={12} /> {status?.online ? 'Download' : 'Ollama Offline'}
                  </button>
                )}
                {installed && (
                  <div className="flex items-center gap-1.5 shrink-0">
                    <button
                      onClick={() => {
                        setConfig({ ...config, vision_model: model.name })
                        void setGlobalVisionModel(model.name)
                      }}
                      className={`text-[10px] px-2.5 py-1 rounded-md font-medium transition-all ${
                        config.vision_model === model.name
                          ? 'text-emerald-300 bg-emerald-500/15'
                          : 'text-[var(--text-2)] hover:text-[var(--text-2)]'
                      }`}
                      style={config.vision_model !== model.name ? selectStyle : {}}
                    >
                      {config.vision_model === model.name ? '✓ Active' : 'Use'}
                    </button>
                    <button
                      onClick={() => deleteModel(model.name)}
                      className="text-[var(--text-3)] hover:text-emerald-400 p-1 rounded transition-colors"
                      title="Delete model"
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                )}
                {isPulling && (
                  <button
                    onClick={() => setPulling(null)}
                    className="text-[var(--text-3)] hover:text-emerald-400 p-1 rounded transition-colors shrink-0"
                  >
                    <X size={12} />
                  </button>
                )}
              </div>
            )
          })}
        </div>

        {!status?.online && (
          <p className="text-xs text-[var(--text-3)] mt-2 text-center">
            Start Ollama first: <code className="px-1 py-0.5 rounded bg-[var(--surface-3)] text-[var(--text-2)]">ollama serve</code>
          </p>
        )}
      </div>

      {/* ── Text Models (for enhancement) ── */}
      <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
        <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2 mb-1">
          <Zap size={14} className="text-emerald-400" />
          Text Models
        </h4>
        <p className="text-[11px] text-[var(--text-3)] mb-4">
          Used for text enhancement (Creative, Exaggerated, Detailed modes).
        </p>

        <div className="mb-4">
          <label className="block text-xs text-[var(--text-3)] mb-1">Active Text Model</label>
          <select
            value={modelPreferences.textModel || config.text_model}
            onChange={(e) => {
              setConfig({ ...config, text_model: e.target.value })
              void setGlobalTextModel(e.target.value)
            }}
            className="w-full h-9 px-3 rounded-lg text-sm text-[var(--text-2)] outline-none appearance-none cursor-pointer"
            style={selectStyle}
          >
            {textModels.length > 0 ? (
              textModels.map((m) => <option key={m} value={m}>{m}</option>)
            ) : (
              <option value={config.text_model}>{config.text_model} (not installed)</option>
            )}
          </select>
        </div>

        <div className="space-y-1.5">
          {recommendedTextModels.map((model) => {
            const installed = allInstalledModels.some((m) => m === model.name || m.startsWith(model.name.split(':')[0] + ':'))
            const isPulling = pulling === model.name
            return (
              <div
                key={model.name}
                className="flex items-center justify-between px-3 py-2.5 rounded-lg"
                style={{ background: 'var(--surface-3)' }}
              >
                <div className="min-w-0 flex-1 mr-3">
                  <div className="flex items-center gap-2">
                    <span className="text-xs font-medium text-[var(--text-2)]">{model.name}</span>
                    <span className="text-[10px] text-[var(--text-3)]">{model.downloadGb.toFixed(1)} GB</span>
                    {installed && (
                      <span className="text-[9px] font-bold text-emerald-400 px-1.5 py-0.5 rounded-full bg-emerald-500/10">
                        INSTALLED
                      </span>
                    )}
                  </div>
                  <p className="text-[10px] text-[var(--text-3)] mt-0.5">{model.description}</p>
                  <p className={`text-[9px] mt-0.5 ${model.compatible ? 'text-emerald-400/70' : 'text-amber-400/80'}`}>{model.reason}</p>
                  {isPulling && (
                    <div className="mt-1.5">
                      <div className="h-1 rounded-full overflow-hidden" style={{ background: 'var(--surface-2)' }}>
                        <div
                          className="h-full rounded-full transition-all duration-300"
                          style={{ width: `${pullProgress.percent}%`, background: 'var(--accent-gradient)' }}
                        />
                      </div>
                      <span className="text-[9px] text-[var(--text-3)] mt-0.5">{pullProgress.status} {pullProgress.percent > 0 ? `${pullProgress.percent}%` : ''}</span>
                    </div>
                  )}
                </div>
                {!installed && !isPulling && (
                  <button
                    onClick={() => pullModel(model.name)}
                    disabled={!!pulling || !status?.online || !model.compatible}
                    className="flex items-center gap-1 px-2.5 py-1.5 rounded-lg text-[11px] font-medium text-[var(--success)] hover:text-[var(--text-1)] transition-all disabled:opacity-40"
                    style={{ background: 'rgba(220, 38, 38,0.12)' }}
                  >
                    <CloudDownload size={12} /> Pull
                  </button>
                )}
                {installed && (
                  <div className="flex items-center gap-1.5">
                    <button
                      onClick={() => {
                        setConfig({ ...config, text_model: model.name })
                        void setGlobalTextModel(model.name)
                      }}
                      className={`text-[10px] px-2 py-1 rounded-md transition-all ${
                        config.text_model === model.name ? 'text-emerald-300 bg-emerald-500/15' : 'text-[var(--text-3)] hover:text-[var(--text-2)]'
                      }`}
                      style={config.text_model !== model.name ? selectStyle : {}}
                    >
                      {config.text_model === model.name ? 'Active' : 'Use'}
                    </button>
                    <button
                      onClick={() => deleteModel(model.name)}
                      className="text-[var(--text-3)] hover:text-emerald-400 p-1 rounded transition-colors"
                      title="Delete model"
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                )}
                {isPulling && (
                  <button
                    onClick={() => setPulling(null)}
                    className="text-[var(--text-3)] hover:text-emerald-400 p-1 rounded transition-colors"
                  >
                    <X size={12} />
                  </button>
                )}
              </div>
            )
          })}
        </div>
      </div>

      {/* ── Generation Budget ── */}
      <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
        <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2 mb-1">
          <Gauge size={14} className="text-[var(--info)]" />
          Generation Budget
          <span className="text-[10px] text-[var(--info)] font-normal ml-1 px-1.5 py-0.5 rounded-full" style={{ background: 'rgba(59,130,246,0.12)' }}>
            saves tokens
          </span>
        </h4>
        <p className="text-[11px] text-[var(--text-3)] mb-3">
          Controls output caps for cloud narration. Balanced is recommended; High is only for very long, detailed chapters.
        </p>
        <div className="grid grid-cols-3 gap-2">
          {([
            { id: 'low', label: 'Low', desc: 'shorter, cheapest' },
            { id: 'balanced', label: 'Balanced', desc: 'recommended' },
            { id: 'high', label: 'High', desc: 'longest output' },
          ] as const).map((opt) => {
            const active = tokenBudget === opt.id
            return (
              <button
                key={opt.id}
                onClick={() => setTokenBudget(opt.id)}
                className="rounded-lg px-3 py-2 text-left transition-all"
                style={{
                  background: active ? 'rgba(59,130,246,0.14)' : 'var(--surface-3)',
                  border: active ? '1px solid rgba(59,130,246,0.35)' : '1px solid transparent',
                }}
              >
                <div className={`text-xs font-semibold ${active ? 'text-white' : 'text-[var(--text-2)]'}`}>{opt.label}</div>
                <div className="text-[10px] text-[var(--text-3)] mt-0.5">{opt.desc}</div>
              </button>
            )
          })}
        </div>
      </div>

      {/* ── Temperature ── */}
      <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
        <h4 className="text-sm font-medium text-[var(--text-2)] flex items-center gap-2 mb-3">
          <Settings2 size={14} className="text-[var(--text-2)]" />
          Temperature
        </h4>
        <div className="grid grid-cols-2 gap-6">
          <div>
            <label className="block text-xs text-[var(--text-3)] mb-1">Vision ({config.vision_temperature})</label>
            <input
              type="range" min="0" max="1" step="0.1"
              value={config.vision_temperature}
              onChange={(e) => setConfig({ ...config, vision_temperature: parseFloat(e.target.value) })}
              className="w-full accent-emerald-500"
            />
            <div className="flex justify-between text-[10px] text-[var(--text-3)] mt-0.5">
              <span>Precise</span><span>Creative</span>
            </div>
          </div>
          <div>
            <label className="block text-xs text-[var(--text-3)] mb-1">Enhancement ({config.text_temperature})</label>
            <input
              type="range" min="0" max="1" step="0.1"
              value={config.text_temperature}
              onChange={(e) => setConfig({ ...config, text_temperature: parseFloat(e.target.value) })}
              className="w-full accent-emerald-500"
            />
            <div className="flex justify-between text-[10px] text-[var(--text-3)] mt-0.5">
              <span>Precise</span><span>Creative</span>
            </div>
          </div>
        </div>
      </div>

      {/* Save */}
      <div className="flex justify-end">
        <button
          onClick={saveConfig}
          disabled={saving}
          className="flex items-center gap-2 px-5 py-2.5 rounded-xl text-sm font-medium text-white transition-all hover:brightness-110 disabled:opacity-50"
          style={{ background: 'var(--accent-gradient)' }}
        >
          {saving ? <Loader2 size={14} className="animate-spin" /> : <Save size={14} />}
          {saving ? 'Saving...' : 'Save Settings'}
        </button>
      </div>
    </div>
  )
}

// ── About Tab ────────────────────────────────────────────

function AboutTab() {
  return (
    <div className="w-full space-y-4 fade-up">
      <div className="rounded-xl px-6 py-6" style={{ background: 'var(--surface-2)' }}>
        <div className="flex items-center gap-4 mb-4">
          <img
            src={logoUrl}
            alt="MangaVox"
            className="w-14 h-14 rounded-2xl object-contain"
          />
          <div>
            <h3 className="text-xl font-bold text-[var(--text-1)]">MangaVox</h3>
            <p className="text-sm text-[var(--text-3)]">Version {__APP_VERSION__}</p>
          </div>
        </div>
        <p className="text-sm text-[var(--text-2)] leading-relaxed">
          Manga-to-Voice Platform — browse, read, and convert manga to audio/video. All processing runs locally on your machine.
        </p>
        <div className="mt-5 pt-4" style={{ borderTop: '1px solid var(--panel-border)' }}>
          <div className="space-y-2">
            <p className="text-sm text-[var(--text-2)]">
              <span className="font-medium text-[var(--text-2)]">Owner:</span> <span style={{ color: 'var(--accent)' }} className="font-medium">Aayush Datta</span>
            </p>
            <p className="text-sm text-[var(--text-2)]">
              <span className="font-medium text-[var(--text-2)]">Built With:</span> Claude AI, Google Codex, GitHub Copilot, and Automation Tools
            </p>
            <p className="text-xs text-[var(--text-3)] italic">
              This application was created through intelligent automation and AI assistance tools, enabling rapid development and intelligent code generation.
            </p>
          </div>
        </div>
      </div>

      {/* Tech stack */}
      <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
        <h4 className="text-sm font-medium text-[var(--text-2)] mb-3">Built With</h4>
        <div className="flex flex-wrap gap-2">
          {['Electron', 'React', 'TypeScript', 'Tailwind CSS', 'SQLite', 'Zustand'].map((tech) => (
            <span
              key={tech}
              className="text-[11px] font-medium text-[var(--text-2)] px-2.5 py-1 rounded-lg"
              style={{ background: 'var(--surface-3)' }}
            >
              {tech}
            </span>
          ))}
        </div>
      </div>

      <UpdateCard />

      <DiagnosticsCard />

      {/* Keyboard shortcuts */}
      <div className="rounded-xl px-5 py-4" style={{ background: 'var(--surface-2)' }}>
        <h4 className="text-sm font-medium text-[var(--text-2)] mb-3">Keyboard Shortcuts</h4>
        <div className="space-y-2">
          {[
            ['Arrow Left / Right', 'Navigate pages'],
            ['Space', 'Next page'],
            ['Escape', 'Go back'],
          ].map(([key, desc]) => (
            <div key={key} className="flex items-center justify-between">
              <span className="text-xs text-[var(--text-3)]">{desc}</span>
              <kbd className="text-[10px] text-[var(--text-2)] font-mono px-2 py-0.5 rounded-md" style={{ background: 'var(--surface-3)' }}>
                {key}
              </kbd>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

function StreamingSettingsTab() {
  const [region, setRegion] = useState('IN')
  const [tmdbKey, setTmdbKey] = useState('')
  const [traktClientId, setTraktClientId] = useState('')
  const [traktSecret, setTraktSecret] = useState('')
  const [traktConnected, setTraktConnected] = useState(false)
  const [device, setDevice] = useState<{ device_code: string; user_code: string; verification_url: string } | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    Promise.all([
      invoke<any>(IPC.CONFIG_GET, 'media'),
      invoke<any>(IPC.CONFIG_GET, 'tmdb'),
      invoke<any>(IPC.CONFIG_GET, 'trakt'),
      invoke<{ connected: boolean }>(IPC.MEDIA_TRAKT_STATUS),
    ]).then(([media, tmdb, trakt, status]) => {
      setRegion(media?.region || localStorage.getItem('mangavox.media.region') || 'IN')
      setTmdbKey(tmdb?.apiKey || '')
      setTraktClientId(trakt?.clientId || '')
      setTraktSecret(trakt?.clientSecret || '')
      setTraktConnected(status.connected)
    }).catch(() => {})
  }, [])

  const save = async () => {
    await Promise.all([
      invoke(IPC.CONFIG_SET, 'media', { region }),
      invoke(IPC.CONFIG_SET, 'tmdb', { apiKey: tmdbKey.trim() }),
      invoke(IPC.CONFIG_SET, 'trakt', { clientId: traktClientId.trim(), clientSecret: traktSecret.trim() }),
    ])
    localStorage.setItem('mangavox.media.region', region)
    toast.success('Streaming settings saved')
  }

  const connectTrakt = async () => {
    await save()
    setBusy(true)
    try {
      const result = await invoke<any>(IPC.MEDIA_TRAKT_BEGIN)
      setDevice(result)
      window.open(result.verification_url, '_blank')
    } catch (error: any) {
      toast.error('Trakt connection failed', error?.message)
    } finally {
      setBusy(false)
    }
  }

  const finishTrakt = async () => {
    if (!device) return
    setBusy(true)
    try {
      const result = await invoke<{ connected: boolean; pending?: boolean }>(IPC.MEDIA_TRAKT_POLL, device.device_code)
      if (result.connected) {
        setTraktConnected(true)
        setDevice(null)
        toast.success('Trakt connected')
      } else toast.info('Waiting for Trakt approval', 'Complete the code in your browser, then try again.')
    } catch (error: any) {
      toast.error('Trakt connection failed', error?.message)
    } finally {
      setBusy(false)
    }
  }

  const syncTrakt = async () => {
    setBusy(true)
    try {
      const result = await invoke<{ imported: number }>(IPC.MEDIA_TRAKT_SYNC)
      toast.success('Trakt history synced', `${result.imported} watched titles imported.`)
    } catch (error: any) {
      toast.error('Trakt sync failed', error?.message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="w-full space-y-5">
      <div>
        <h3 className="text-base font-semibold text-[var(--text-2)]">Streaming Hub</h3>
        <p className="text-xs text-[var(--text-3)] mt-1">Works without API keys using installed catalog addons and keyless public metadata. Provider passwords and cookies are never requested.</p>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
        <label className="text-xs text-[var(--text-2)]">Region
          <input value={region} onChange={(e) => setRegion(e.target.value.toUpperCase().slice(0, 2))}
            className="mt-1 w-full h-10 px-3 rounded-lg bg-[var(--surface-2)] border border-[var(--panel-border)] text-[var(--text-2)]" />
        </label>
        <label className="text-xs text-[var(--text-2)]">Optional TMDB enrichment token
          <input type="password" value={tmdbKey} onChange={(e) => setTmdbKey(e.target.value)}
            className="mt-1 w-full h-10 px-3 rounded-lg bg-[var(--surface-2)] border border-[var(--panel-border)] text-[var(--text-2)]" />
        </label>
      </div>
      <div className="rounded-xl p-4 space-y-3" style={{ background: 'var(--surface-2)', border: '1px solid var(--panel-border)' }}>
        <div className="flex items-center justify-between">
          <div>
            <p className="text-sm font-semibold text-[var(--text-2)]">Trakt Sync</p>
            <p className="text-xs text-[var(--text-3)]">{traktConnected ? 'Connected securely' : 'Import watched movies and shows with device authorization.'}</p>
          </div>
          {traktConnected && <Check size={16} className="text-emerald-400" />}
        </div>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <input placeholder="Trakt client ID" value={traktClientId} onChange={(e) => setTraktClientId(e.target.value)}
            className="h-10 px-3 rounded-lg bg-[var(--surface-2)] border border-[var(--panel-border)] text-xs text-[var(--text-2)]" />
          <input type="password" placeholder="Trakt client secret" value={traktSecret} onChange={(e) => setTraktSecret(e.target.value)}
            className="h-10 px-3 rounded-lg bg-[var(--surface-2)] border border-[var(--panel-border)] text-xs text-[var(--text-2)]" />
        </div>
        {device && <p className="text-sm text-[var(--text-2)]">Enter code <strong className="text-[var(--text-1)]">{device.user_code}</strong> at Trakt, then finish authorization.</p>}
        <div className="flex flex-wrap gap-2">
          {!traktConnected && !device && <button onClick={connectTrakt} disabled={busy} className="btn-primary px-4 py-2 text-xs">Connect Trakt</button>}
          {device && <button onClick={finishTrakt} disabled={busy} className="btn-primary px-4 py-2 text-xs">Finish authorization</button>}
          {traktConnected && <button onClick={syncTrakt} disabled={busy} className="btn-primary px-4 py-2 text-xs">Sync now</button>}
        </div>
      </div>
      <button onClick={save} className="btn-primary px-5 py-2.5 text-sm"><Save size={14} /> Save streaming settings</button>
    </div>
  )
}

// ── Addons Tab (Anime Torrent Addons) ─────────────────────

interface AddonEntry {
  id: string
  name: string
  version: string
  description?: string
  icon?: string
  url: string
  catalogs: { id: string; name: string }[]
}

function AddonsTab() {
  const [addons, setAddons] = useState<AddonEntry[]>([])
  const [newUrl, setNewUrl] = useState('')
  const [installing, setInstalling] = useState(false)
  const [installError, setInstallError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  const loadAddons = () => {
    setLoading(true)
    invoke<AddonEntry[]>(IPC.ADDON_LIST)
      .then(setAddons)
      .catch(() => setAddons([]))
      .finally(() => setLoading(false))
  }

  useEffect(() => { loadAddons() }, [])

  const handleInstall = async () => {
    if (!newUrl.trim()) return
    setInstalling(true)
    setInstallError(null)
    try {
      await invoke(IPC.ADDON_INSTALL, newUrl.trim())
      setNewUrl('')
      loadAddons()
    } catch (err: any) {
      setInstallError(err?.message || 'Failed to install addon')
    } finally {
      setInstalling(false)
    }
  }

  const handleUninstall = async (id: string) => {
    try {
      await invoke(IPC.ADDON_UNINSTALL, id)
      loadAddons()
    } catch (err: any) {
      console.error('[settings:addons] Uninstall failed:', err)
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-base font-semibold text-[var(--text-2)] mb-1">Anime Addons</h3>
        <p className="text-xs text-[var(--text-3)] mb-4">
          Install Stremio-compatible addons to browse anime catalogs and stream episodes via torrent.
          Paste the addon URL below.
        </p>

        {/* Install form */}
        <div className="flex gap-2 mb-4">
          <div className="flex-1 relative">
            <ExternalLink size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-3)]" />
            <input
              type="text"
              value={newUrl}
              onChange={(e) => { setNewUrl(e.target.value); setInstallError(null) }}
              onKeyDown={(e) => e.key === 'Enter' && handleInstall()}
              placeholder="https://addon-url.com/manifest.json"
              className="w-full h-10 pl-9 pr-4 rounded-xl text-sm text-[var(--text-2)] placeholder-[var(--text-3)] outline-none"
              style={{ background: 'var(--surface-3)', border: installError ? '1px solid rgba(239,68,68,0.4)' : '1px solid var(--panel-border)' }}
            />
          </div>
          <button
            onClick={handleInstall}
            disabled={!newUrl.trim() || installing}
            className="flex items-center gap-2 px-5 h-10 rounded-xl text-sm font-semibold text-white transition-all disabled:opacity-40"
            style={{ background: 'var(--accent-gradient)' }}
          >
            {installing ? <Loader2 size={14} className="animate-spin" /> : <Plus size={14} />}
            Install
          </button>
        </div>

        {installError && (
          <p className="text-xs text-emerald-400 mb-3 flex items-center gap-1">
            <AlertTriangle size={12} /> {installError}
          </p>
        )}
      </div>

      {/* Installed addons list */}
      <div>
        <h4 className="text-sm font-medium text-[var(--text-2)] mb-3">
          Installed {addons.length > 0 && `(${addons.length})`}
        </h4>

        {loading ? (
          <div className="flex items-center gap-2 text-sm text-[var(--text-3)]">
            <Loader2 size={14} className="animate-spin" /> Loading addons...
          </div>
        ) : addons.length === 0 ? (
          <div
            className="flex flex-col items-center justify-center py-12 rounded-xl"
            style={{ background: 'var(--surface-2)' }}
          >
            <Film size={24} className="text-[var(--text-3)] mb-2" />
            <p className="text-sm text-[var(--text-3)]">No addons installed</p>
            <p className="text-xs text-[var(--text-3)] mt-1">
              Paste a Stremio addon URL above to get started
            </p>
          </div>
        ) : (
          <div className="space-y-2">
            {addons.map((addon) => (
              <div
                key={addon.id}
                className="flex items-center gap-3 sm:gap-4 px-3 sm:px-4 py-3 rounded-xl min-w-0"
                style={{ background: 'var(--surface-2)' }}
              >
                {addon.icon ? (
                  <img src={addon.icon} className="w-10 h-10 rounded-lg object-cover shrink-0" alt="" />
                ) : (
                  <div
                    className="w-10 h-10 rounded-lg flex items-center justify-center shrink-0"
                    style={{ background: 'var(--surface-3)' }}
                  >
                    <Film size={18} className="text-[var(--text-3)]" />
                  </div>
                )}

                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 min-w-0">
                    <span className="text-sm font-medium text-[var(--text-2)] truncate">{addon.name}</span>
                    <span className="text-[10px] text-[var(--text-3)] font-mono shrink-0">v{addon.version}</span>
                  </div>
                  {addon.description && (
                    <p className="text-xs text-[var(--text-3)] truncate mt-0.5">{addon.description}</p>
                  )}
                  {addon.catalogs.length > 0 && (
                    <div className="flex flex-wrap gap-1.5 mt-1">
                      {addon.catalogs.map((cat, i) => (
                        <span
                          key={`${cat.id}-${i}`}
                          className="px-1.5 py-0.5 rounded text-[10px] text-[var(--text-2)] whitespace-nowrap"
                          style={{ background: 'var(--surface-3)' }}
                        >
                          {cat.name}
                        </span>
                      ))}
                    </div>
                  )}
                </div>

                <button
                  onClick={() => handleUninstall(addon.id)}
                  className="text-[var(--text-3)] hover:text-emerald-400 transition-colors shrink-0 min-w-[40px] min-h-[40px] flex items-center justify-center"
                  title="Uninstall"
                >
                  <Trash2 size={16} />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
