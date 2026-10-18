import { useEffect } from 'react'
import { motion, useReducedMotion } from 'framer-motion'
import logoUrl from '../assets/logo.png'

export function SplashScreen({ onDone }: { onDone: () => void }) {
  const reducedMotion = useReducedMotion()

  useEffect(() => {
    const timer = window.setTimeout(onDone, reducedMotion ? 500 : 1300)
    return () => window.clearTimeout(timer)
  }, [onDone, reducedMotion])

  return (
    <div className="fixed inset-0 z-[100] flex flex-col items-center justify-center" style={{ background: 'var(--base)' }}>
      <div
        aria-hidden
        className="absolute inset-0 pointer-events-none"
        style={{ background: 'radial-gradient(520px 320px at 50% 46%, rgba(var(--accent-rgb), 0.14), transparent 70%)' }}
      />
      <motion.img
        src={logoUrl}
        alt="MangaVox"
        className="relative w-14 h-14 rounded-xl object-contain"
        initial={{ opacity: 0, scale: 0.9 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ duration: 0.4, ease: [0.4, 0, 0.2, 1] }}
      />
      <motion.div
        className="relative mt-4 text-[22px] font-extrabold tracking-tight"
        initial={{ opacity: 0, y: 4 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.12, duration: 0.35 }}
      >
        <span style={{ color: 'var(--text-1)' }}>MANGA</span>
        <span style={{ color: 'var(--accent-hover)' }}>VOX</span>
      </motion.div>
      <motion.div
        className="relative mt-1 text-[10px] font-semibold tracking-[0.22em] uppercase"
        style={{ color: 'var(--text-3)' }}
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        transition={{ delay: 0.25 }}
      >
        Read · Watch · Listen
      </motion.div>
      <div className="relative mt-7 h-[2px] w-40 rounded-full overflow-hidden" style={{ background: 'var(--surface-4)' }}>
        <motion.div
          className="h-full rounded-full"
          style={{ background: 'var(--accent-gradient)' }}
          initial={{ width: '0%' }}
          animate={{ width: '100%' }}
          transition={{ duration: reducedMotion ? 0.4 : 1.1, ease: 'easeInOut' }}
        />
      </div>
    </div>
  )
}
