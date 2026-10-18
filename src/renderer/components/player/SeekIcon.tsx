import { useState } from 'react'
import { motion } from 'framer-motion'

/** 10-second skip icon: circular arrow with the number inside, spins on click. */
export function SeekIcon({ dir, size = 26, onClick, title }: { dir: 'back' | 'forward'; size?: number; onClick: () => void; title: string }) {
  const [n, setN] = useState(0)
  const spin = dir === 'forward' ? 360 : -360
  return (
    <motion.button
      onClick={(e) => { e.stopPropagation(); setN((v) => v + 1); onClick() }}
      whileHover={{ scale: 1.12 }}
      whileTap={{ scale: 0.88 }}
      className="relative text-white/85 hover:text-white shrink-0"
      style={{ width: size, height: size }}
      title={title}
    >
      <motion.svg
        key={n}
        initial={{ rotate: 0 }}
        animate={{ rotate: n ? spin : 0 }}
        transition={{ duration: 0.45, ease: [0.22, 1, 0.36, 1] }}
        viewBox="0 0 24 24" width={size} height={size} fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"
        style={{ position: 'absolute', inset: 0, scale: dir === 'forward' ? '-1 1' : '1 1' } as React.CSSProperties}
      >
        <path d="M3 12a9 9 0 1 0 3-6.7" />
        <path d="M3 4v5h5" />
      </motion.svg>
      <span className="absolute inset-0 flex items-center justify-center text-[8px] font-bold pointer-events-none" style={{ paddingTop: 1 }}>10</span>
    </motion.button>
  )
}
