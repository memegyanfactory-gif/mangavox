import { useState, type ReactNode } from 'react'
import { motion } from 'framer-motion'
import { Check, ChevronDown, FileUp, Loader2, RefreshCw, RotateCcw, X } from 'lucide-react'
import { LangFlag } from './LangFlag'
import { audioFormat, resolveLanguage } from './languages'
import type { SubBackground } from './SubtitleOverlay'

export interface AudioOption { idx: number; label: string; lang: string; codec?: string; channels?: number }
/** An audio language this file lacks but another source has. */
export interface OtherSourceAudio { lang: string; detail: string }
export interface SubtitleOption {
  key: string
  lang: string
  label: string
  detail?: string
  group: 'embedded' | 'file' | 'online'
  /** Rendered but not selectable (image-based tracks). */
  disabledReason?: string
}

interface Props {
  audio: AudioOption[]
  selectedAudio: number
  onSelectAudio: (idx: number) => void
  otherAudio?: OtherSourceAudio[]
  switchingAudio?: string | null
  onSelectOtherAudio?: (lang: string) => void
  subtitles: SubtitleOption[]
  activeSubtitle: string | null
  loadingSubtitle: string | null
  onlineLoading: boolean
  onSelectSubtitle: (key: string | null) => void
  onNextVersion?: () => void
  onImport: () => void
  style: {
    offset: number; size: number; color: string; background: SubBackground; position: number
    onOffset: (delta: number) => void; onResetOffset: () => void
    onSize: (v: number) => void; onColor: (v: string) => void; onBackground: (v: SubBackground) => void; onPosition: (v: number) => void
  }
  onClose: () => void
}

const COLORS = ['#ffffff', '#ffe066', '#7de3ff', '#9dff8a']

function Row({ active, disabled, onClick, flag, title, detail, trailing }: {
  active: boolean; disabled?: boolean; onClick?: () => void; flag?: ReactNode; title: string; detail?: string; trailing?: ReactNode
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className={`group w-full flex items-center gap-2.5 px-3 h-10 rounded-lg text-left transition-colors ${
        active ? 'bg-white/[0.09] text-white' : disabled ? 'text-white/30 cursor-not-allowed' : 'text-white/75 hover:bg-white/[0.05] hover:text-white'
      }`}
    >
      <span className="w-4 shrink-0 flex justify-center">
        {active && <Check size={14} strokeWidth={3} style={{ color: 'var(--accent-hover)' }} />}
      </span>
      {flag}
      <span className="min-w-0 flex-1">
        <span className={`block truncate text-[13px] leading-tight ${active ? 'font-semibold' : 'font-medium'}`}>{title}</span>
        {detail && <span className="block truncate text-[10.5px] leading-tight text-white/40 mt-0.5">{detail}</span>}
      </span>
      {trailing}
    </button>
  )
}

function Heading({ children }: { children: ReactNode }) {
  return <div className="px-3 pt-3 pb-1.5 text-[10px] uppercase tracking-[0.16em] font-bold text-white/40">{children}</div>
}

function Stepper({ label, value, onMinus, onPlus, extra }: { label: string; value: string; onMinus: () => void; onPlus: () => void; extra?: ReactNode }) {
  const btn = 'w-7 h-7 rounded-md flex items-center justify-center text-white/80 text-sm font-bold transition-colors hover:bg-white/15 hover:text-white bg-white/[0.06]'
  return (
    <div className="flex items-center justify-between gap-2">
      <span className="text-[11px] text-white/55">{label}</span>
      <div className="flex items-center gap-1">
        {extra}
        <button type="button" className={btn} onClick={onMinus}>−</button>
        <span className="text-[11px] text-white font-mono tabular-nums w-12 text-center">{value}</span>
        <button type="button" className={btn} onClick={onPlus}>+</button>
      </div>
    </div>
  )
}

/** Netflix-style "Audio & Subtitles" panel: languages with flags, side by side. */
export function TrackPanel(props: Props) {
  const { audio, selectedAudio, subtitles, activeSubtitle, loadingSubtitle, onlineLoading, style } = props
  const [showStyle, setShowStyle] = useState(false)
  const groups: Array<[SubtitleOption['group'], string]> = [['embedded', 'In this video'], ['file', 'From files'], ['online', 'Online']]
  const activeOption = subtitles.find((s) => s.key === activeSubtitle)

  return (
    <motion.div
      key="track-panel"
      initial={{ opacity: 0, y: 10, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: 10, scale: 0.98 }}
      transition={{ duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
      className="absolute bottom-[104px] right-5 z-40 w-[min(620px,calc(100%-40px))] rounded-2xl overflow-hidden flex flex-col pointer-events-auto"
      style={{
        background: 'rgba(14,14,16,0.94)',
        border: '1px solid rgba(255,255,255,0.09)',
        backdropFilter: 'blur(24px)',
        boxShadow: '0 24px 60px rgba(0,0,0,0.55)',
        maxHeight: 'min(70vh, 560px)',
      }}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      <div className="flex items-center justify-between px-4 h-11 border-b border-white/[0.06] shrink-0">
        <span className="text-[13px] font-semibold text-white">Audio &amp; Subtitles</span>
        <button type="button" onClick={props.onClose} className="w-7 h-7 rounded-md flex items-center justify-center text-white/50 hover:text-white hover:bg-white/10 transition-colors">
          <X size={15} />
        </button>
      </div>

      <div className="flex-1 min-h-0 grid grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)] divide-x divide-white/[0.06]">
        {/* Audio */}
        <div className="overflow-y-auto px-1.5 pb-2">
          <Heading>Audio</Heading>
          {audio.length === 0 ? (
            <Row active title="Original audio" detail="Single track" />
          ) : audio.map((track) => {
            const info = resolveLanguage(track.lang, track.label)
            const name = info.code ? info.name : (track.label || `Audio ${track.idx + 1}`)
            const extra = track.label && track.label.toLowerCase() !== name.toLowerCase() && track.label !== `Audio ${track.idx + 1}` ? track.label : ''
            return (
              <Row
                key={track.idx}
                active={track.idx === selectedAudio}
                onClick={() => props.onSelectAudio(track.idx)}
                flag={<LangFlag lang={track.lang} hint={track.label} />}
                title={name}
                detail={[extra, audioFormat(track.codec, track.channels)].filter(Boolean).join(' · ') || undefined}
              />
            )
          })}
          {!!props.otherAudio?.length && (
            <>
              <Heading>From other sources</Heading>
              {props.otherAudio.map((option) => (
                <Row
                  key={option.lang}
                  active={false}
                  onClick={() => props.onSelectOtherAudio?.(option.lang)}
                  flag={<LangFlag lang={option.lang} />}
                  title={resolveLanguage(option.lang).name}
                  detail={option.detail}
                  trailing={props.switchingAudio === option.lang ? <Loader2 size={13} className="animate-spin text-white/50 shrink-0" /> : undefined}
                />
              ))}
            </>
          )}
        </div>

        {/* Subtitles */}
        <div className="overflow-y-auto px-1.5 pb-2">
          <Heading>Subtitles</Heading>
          <Row active={!activeSubtitle} onClick={() => props.onSelectSubtitle(null)} title="Off" />
          {groups.map(([group, title]) => {
            const items = subtitles.filter((s) => s.group === group)
            if (!items.length && !(group === 'online' && onlineLoading)) return null
            return (
              <div key={group}>
                <Heading>{title}</Heading>
                {items.map((s) => (
                  <Row
                    key={s.key}
                    active={s.key === activeSubtitle}
                    disabled={!!s.disabledReason}
                    onClick={() => props.onSelectSubtitle(s.key)}
                    flag={<LangFlag lang={s.lang} hint={s.label} />}
                    title={s.label}
                    detail={s.disabledReason || s.detail}
                    trailing={loadingSubtitle === s.key ? <Loader2 size={13} className="animate-spin text-white/50 shrink-0" /> : undefined}
                  />
                ))}
                {group === 'online' && onlineLoading && (
                  <div className="flex items-center gap-2 px-3 h-9 text-[11px] text-white/40">
                    <Loader2 size={12} className="animate-spin" /> Searching subtitle sources…
                  </div>
                )}
              </div>
            )
          })}
          {!onlineLoading && !subtitles.some((s) => s.group === 'online') && (
            <p className="px-3 pt-2 text-[11px] text-white/35">No online subtitles found for this title.</p>
          )}
          <div className="px-1.5 pt-2 flex gap-1.5">
            <button
              type="button"
              onClick={props.onImport}
              className="flex-1 flex items-center justify-center gap-1.5 h-8 rounded-lg text-[11px] font-medium text-white/70 hover:text-white bg-white/[0.05] hover:bg-white/[0.1] transition-colors"
            >
              <FileUp size={12} /> Load file
            </button>
            {activeOption?.group === 'online' && props.onNextVersion && (
              <button
                type="button"
                onClick={props.onNextVersion}
                title="Out of sync? Try a different release of this language"
                className="flex-1 flex items-center justify-center gap-1.5 h-8 rounded-lg text-[11px] font-medium text-white/70 hover:text-white bg-white/[0.05] hover:bg-white/[0.1] transition-colors"
              >
                <RefreshCw size={12} /> Another version
              </button>
            )}
          </div>
        </div>
      </div>

      {/* Subtitle style */}
      <div className="border-t border-white/[0.06] shrink-0">
        <button
          type="button"
          onClick={() => setShowStyle((v) => !v)}
          className="w-full flex items-center justify-between px-4 h-10 text-[11px] text-white/60 hover:text-white transition-colors"
        >
          <span className="font-semibold uppercase tracking-[0.14em] text-[10px]">Subtitle timing &amp; style</span>
          <span className="flex items-center gap-2">
            {style.offset !== 0 && <span className="font-mono tabular-nums" style={{ color: 'var(--accent-hover)' }}>{style.offset > 0 ? '+' : ''}{style.offset.toFixed(1)}s</span>}
            <ChevronDown size={14} className={`transition-transform ${showStyle ? 'rotate-180' : ''}`} />
          </span>
        </button>
        {showStyle && (
          <div className="px-4 pb-3 grid grid-cols-2 gap-x-6 gap-y-2.5">
            <Stepper
              label="Delay (G / H)"
              value={`${style.offset > 0 ? '+' : ''}${style.offset.toFixed(1)}s`}
              onMinus={() => style.onOffset(-0.1)}
              onPlus={() => style.onOffset(0.1)}
              extra={style.offset !== 0 ? (
                <button type="button" title="Reset" onClick={style.onResetOffset} className="w-7 h-7 rounded-md flex items-center justify-center text-white/60 hover:text-white hover:bg-white/10"><RotateCcw size={12} /></button>
              ) : undefined}
            />
            <Stepper label="Size" value={`${style.size}%`} onMinus={() => style.onSize(Math.max(50, style.size - 10))} onPlus={() => style.onSize(Math.min(200, style.size + 10))} />
            <Stepper label="Position" value={`${style.position}%`} onMinus={() => style.onPosition(Math.max(2, style.position - 2))} onPlus={() => style.onPosition(Math.min(60, style.position + 2))} />
            <div className="flex items-center justify-between gap-2">
              <span className="text-[11px] text-white/55">Colour</span>
              <div className="flex items-center gap-1.5">
                {COLORS.map((c) => (
                  <button key={c} type="button" onClick={() => style.onColor(c)}
                    className="w-5 h-5 rounded-full transition-transform hover:scale-110"
                    style={{ background: c, boxShadow: style.color === c ? '0 0 0 2px rgba(14,14,16,1), 0 0 0 4px var(--accent-hover)' : '0 0 0 1px rgba(255,255,255,0.25)' }} />
                ))}
              </div>
            </div>
            <div className="col-span-2 flex items-center justify-between gap-2">
              <span className="text-[11px] text-white/55">Background</span>
              <div className="flex items-center gap-1 p-0.5 rounded-lg bg-white/[0.05]">
                {(['none', 'soft', 'solid'] as const).map((b) => (
                  <button key={b} type="button" onClick={() => style.onBackground(b)}
                    className={`px-3 h-7 rounded-md text-[11px] capitalize transition-colors ${style.background === b ? 'bg-white/[0.14] text-white font-semibold' : 'text-white/55 hover:text-white'}`}>
                    {b === 'none' ? 'Outline' : b}
                  </button>
                ))}
              </div>
            </div>
          </div>
        )}
      </div>
    </motion.div>
  )
}
