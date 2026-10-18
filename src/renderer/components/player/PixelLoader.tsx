import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { Check } from 'lucide-react'

export interface LoaderStep { label: string; state: 'done' | 'active' | 'todo' }

const COLS = 38
const ROWS = 7
const CELL = 11
const GAP = 3

/** Deterministic 0..1 noise so the pattern is stable across renders. */
function noise(i: number): number {
  const x = Math.sin(i * 12.9898 + 78.233) * 43758.5453
  return x - Math.floor(x)
}

/**
 * Contribution-graph style loader: pixels light up column by column as the
 * stream gets closer to playing. The shown value eases toward `progress` and
 * creeps forward slightly while waiting, so it never looks frozen.
 */
export function PixelLoader({ progress, steps, title, detail }: {
  progress: number
  steps: LoaderStep[]
  title: string
  detail?: ReactNode
}) {
  const [shown, setShown] = useState(0)
  const targetRef = useRef(progress)
  targetRef.current = Math.max(0, Math.min(1, progress))

  useEffect(() => {
    const id = window.setInterval(() => {
      setShown((value) => {
        const target = targetRef.current
        if (value < target) return Math.min(target, value + Math.max(0.004, (target - value) * 0.14))
        // Waiting inside a stage: creep a little past the target, never further.
        return target < 1 && value < target + 0.05 ? value + 0.0006 : value
      })
    }, 50)
    return () => window.clearInterval(id)
  }, [])

  const cells = useMemo(() => Array.from({ length: COLS * ROWS }, (_, i) => {
    const col = Math.floor(i / ROWS)
    // Fill left→right with a ragged edge, like a graph being drawn in.
    const threshold = Math.min(0.995, (col + noise(i) * 2.2) / (COLS + 1.2))
    const level = 1 + Math.floor(noise(i + 991) * 4) // 1..4 intensity
    return { threshold, level, delay: noise(i + 37) * 1.2 }
  }), [])

  const alpha = [0, 0.28, 0.48, 0.72, 1]
  const pct = Math.round(shown * 100)

  return (
    <div className="flex flex-col items-center gap-5 w-full">
      <style>{`
        @keyframes mv-pixel-wait { 0%,100% { opacity: .35 } 50% { opacity: 1 } }
        @keyframes mv-pixel-pop { 0% { transform: scale(.4) } 70% { transform: scale(1.18) } 100% { transform: scale(1) } }
      `}</style>

      <div
        className="grid p-3 rounded-xl"
        style={{
          gridTemplateRows: `repeat(${ROWS}, ${CELL}px)`,
          gridAutoFlow: 'column',
          gridAutoColumns: `${CELL}px`,
          gap: GAP,
          background: 'rgba(0,0,0,0.35)',
          border: '1px solid rgba(255,255,255,0.07)',
        }}
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
      >
        {cells.map((cell, i) => {
          const lit = shown >= cell.threshold
          const frontier = !lit && cell.threshold - shown < 0.07
          return (
            <span
              key={i}
              className="rounded-[2.5px]"
              style={{
                width: CELL,
                height: CELL,
                background: lit
                  ? `rgba(var(--accent-rgb), ${alpha[cell.level]})`
                  : frontier ? 'rgba(var(--accent-rgb), 0.16)' : 'rgba(255,255,255,0.06)',
                animation: lit
                  ? 'mv-pixel-pop 320ms cubic-bezier(.22,1,.36,1) both'
                  : frontier ? `mv-pixel-wait 1.4s ease-in-out ${cell.delay}s infinite` : undefined,
              }}
            />
          )
        })}
      </div>

      <div className="flex items-baseline gap-3">
        <span className="text-base font-semibold text-white/90">{title}</span>
        <span className="text-sm font-semibold tabular-nums" style={{ color: 'var(--accent-hover)' }}>{pct}%</span>
      </div>

      <div className="flex items-center gap-2">
        {steps.map((step, i) => (
          <div key={step.label} className="flex items-center gap-2">
            {i > 0 && <span className="w-5 h-px" style={{ background: step.state === 'todo' ? 'rgba(255,255,255,0.12)' : 'rgba(var(--accent-rgb),0.6)' }} />}
            <span className={`flex items-center gap-1.5 text-[11px] font-medium ${step.state === 'todo' ? 'text-white/30' : step.state === 'active' ? 'text-white' : 'text-white/60'}`}>
              <span
                className="w-3.5 h-3.5 rounded-full flex items-center justify-center"
                style={{
                  background: step.state === 'done' ? 'var(--accent)' : 'transparent',
                  border: step.state === 'done' ? 'none' : `1.5px solid ${step.state === 'active' ? 'var(--accent-hover)' : 'rgba(255,255,255,0.2)'}`,
                  animation: step.state === 'active' ? 'mv-pixel-wait 1.2s ease-in-out infinite' : undefined,
                }}
              >
                {step.state === 'done' && <Check size={9} strokeWidth={3.5} className="text-white" />}
              </span>
              {step.label}
            </span>
          </div>
        ))}
      </div>

      {detail}
    </div>
  )
}
