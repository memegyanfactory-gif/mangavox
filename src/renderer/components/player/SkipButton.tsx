import { AnimatePresence, motion } from 'framer-motion'
import { FastForward } from 'lucide-react'

interface Props {
  label: string | null
  visible: boolean
  fallback?: boolean
  onSkip: () => void
}

/** Netflix-style skip pill: slides in from the right, bottom-right of the stage. */
export function SkipButton({ label, visible, fallback, onSkip }: Props) {
  return (
    <AnimatePresence>
      {label && visible && (
        <motion.button
          key={label}
          initial={{ opacity: 0, x: 48 }}
          animate={{ opacity: 1, x: 0 }}
          exit={{ opacity: 0, x: 48 }}
          transition={{ duration: 0.28, ease: [0.22, 1, 0.36, 1] }}
          onClick={(e) => { e.stopPropagation(); onSkip() }}
          onDoubleClick={(e) => e.stopPropagation()}
          className="absolute right-6 bottom-28 z-[35] flex items-center gap-2 pl-4 pr-5 py-2.5 rounded-md text-sm font-semibold tracking-wide text-white hover:text-black hover:bg-white transition-colors"
          style={{
            background: 'rgba(20,14,10,0.78)',
            border: '1.5px solid rgba(255,255,255,0.75)',
            backdropFilter: 'blur(8px)',
            boxShadow: '0 8px 28px rgba(0,0,0,0.55)',
          }}
          title={fallback ? 'No chapter data - jumps ahead 85s (S)' : `${label} (S)`}
        >
          <FastForward size={16} fill="currentColor" /> {label}
        </motion.button>
      )}
    </AnimatePresence>
  )
}
