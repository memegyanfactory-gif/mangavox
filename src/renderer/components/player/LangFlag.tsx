import { useState } from 'react'
import { resolveLanguage } from './languages'

/** Small rounded country flag for a language, falling back to a code chip. */
export function LangFlag({ lang, hint, size = 18 }: { lang?: string | null; hint?: string | null; size?: number }) {
  const info = resolveLanguage(lang, hint)
  const [failed, setFailed] = useState(false)
  const width = size
  const height = Math.round(size * 0.72)

  if (!info.country || failed) {
    return (
      <span
        className="inline-flex items-center justify-center shrink-0 rounded-[3px] font-bold uppercase text-white/70"
        style={{ width, height, fontSize: Math.max(7, Math.round(size * 0.42)), background: 'rgba(255,255,255,0.12)', letterSpacing: 0 }}
        aria-hidden
      >
        {(info.code || '?').slice(0, 2)}
      </span>
    )
  }
  return (
    <img
      src={`https://flagcdn.com/${info.country}.svg`}
      alt=""
      aria-hidden
      draggable={false}
      onError={() => setFailed(true)}
      className="shrink-0 rounded-[3px] object-cover"
      style={{ width, height, boxShadow: '0 0 0 1px rgba(255,255,255,0.14)' }}
    />
  )
}
