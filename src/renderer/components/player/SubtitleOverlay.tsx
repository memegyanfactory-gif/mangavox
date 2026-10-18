import { Fragment, useEffect, useRef, useState, type ReactNode } from 'react'
import { activeCuesAt, type SubCue } from './subtitles'

export type SubBackground = 'none' | 'soft' | 'solid'

interface Props {
  cues: SubCue[] | null
  /** Absolute media time in seconds (accounts for transcode offsets). */
  getTime: () => number
  /** Positive delays subtitles, negative shows them earlier. */
  offset: number
  size: number
  color: string
  background: SubBackground
  /** Distance from the bottom edge, in percent of the frame height. */
  bottomPct: number
  /** Lift captions above the control bar while it is visible. */
  lifted: boolean
}

/** Render the inline tags subtitles actually use (<i>, <b>, <u>) and drop the rest. */
function renderLine(line: string, key: number): ReactNode {
  const parts = line.split(/(<\/?[ibu]>)/i)
  let italic = false
  let bold = false
  let underline = false
  const out: ReactNode[] = []
  parts.forEach((part, i) => {
    const tag = part.match(/^<(\/?)([ibu])>$/i)
    if (tag) {
      const on = tag[1] !== '/'
      const t = tag[2].toLowerCase()
      if (t === 'i') italic = on
      else if (t === 'b') bold = on
      else underline = on
      return
    }
    const text = part.replace(/<[^>]+>/g, '')
    if (!text) return
    out.push(
      <span key={i} style={{ fontStyle: italic ? 'italic' : undefined, fontWeight: bold ? 800 : undefined, textDecoration: underline ? 'underline' : undefined }}>
        {text}
      </span>,
    )
  })
  return <Fragment key={key}>{key > 0 && <br />}{out}</Fragment>
}

/**
 * Subtitles drawn by the player instead of native text tracks: they survive
 * video remounts (transcode / audio switches), follow the absolute timeline,
 * shift with the sync offset without mutating cues, and look the same everywhere.
 */
export function SubtitleOverlay({ cues, getTime, offset, size, color, background, bottomPct, lifted }: Props) {
  const [active, setActive] = useState<SubCue[]>([])
  const lastKey = useRef('')
  const getTimeRef = useRef(getTime)
  getTimeRef.current = getTime

  useEffect(() => {
    lastKey.current = ''
    if (!cues?.length) { setActive([]); return }
    let raf = 0
    const tick = () => {
      const found = activeCuesAt(cues, getTimeRef.current() - offset)
      const key = found.map((c) => `${c.start}:${c.end}`).join('|')
      if (key !== lastKey.current) { lastKey.current = key; setActive(found) }
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [cues, offset])

  if (!active.length) return null
  const bottom = active.filter((c) => !c.top)
  const top = active.filter((c) => c.top)
  const bg = background === 'none' ? 'transparent' : background === 'solid' ? 'rgba(0,0,0,0.9)' : 'rgba(8,8,8,0.62)'
  const shadow = background === 'none'
    ? '0 0 3px rgba(0,0,0,0.95), 0 0 2px rgba(0,0,0,0.95), 0 2px 4px rgba(0,0,0,0.9)'
    : '0 1px 2px rgba(0,0,0,0.6)'
  // Scales with the frame like broadcast captions: ~3.6vh at 100%.
  const fontSize = `calc(clamp(15px, 3.6vh, 46px) * ${size / 100})`

  const block = (list: SubCue[], position: 'top' | 'bottom') => (
    <div
      className="absolute left-0 right-0 flex flex-col items-center gap-1 px-[8%] pointer-events-none transition-[bottom] duration-300 ease-out"
      style={position === 'top' ? { top: '6%' } : { bottom: lifted ? `max(${bottomPct}%, 132px)` : `${bottomPct}%` }}
    >
      {list.map((cue) => (
        <div
          key={`${cue.start}:${cue.end}:${cue.text.length}`}
          className="text-center leading-[1.32] font-semibold whitespace-pre-wrap"
          style={{
            color,
            fontSize,
            textShadow: shadow,
            background: bg,
            padding: background === 'none' ? 0 : '0.1em 0.45em',
            borderRadius: background === 'none' ? 0 : 6,
            boxDecorationBreak: 'clone',
            WebkitBoxDecorationBreak: 'clone',
            fontFamily: 'system-ui, -apple-system, "Segoe UI", sans-serif',
            letterSpacing: '0.005em',
            maxWidth: '100%',
          }}
        >
          {cue.text.split('\n').map(renderLine)}
        </div>
      ))}
    </div>
  )

  return (
    <div className="absolute inset-0 z-[27] pointer-events-none">
      {top.length > 0 && block(top, 'top')}
      {bottom.length > 0 && block(bottom, 'bottom')}
    </div>
  )
}
