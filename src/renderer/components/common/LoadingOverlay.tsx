import React, { createContext, useContext } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import logoUrl from '../../assets/logo.png';

// Context to control overlay visibility
const AppReadyContext = createContext<{ ready: boolean; setReady: (r: boolean) => void }>({
  ready: false,
  setReady: () => {}
});

export const useAppReady = () => useContext(AppReadyContext);

export const AppReadyProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [ready, setReady] = React.useState(false);
  return (
    <AppReadyContext.Provider value={{ ready, setReady }}>
      {children}
    </AppReadyContext.Provider>
  );
};

interface LoadingOverlayProps {
  show: boolean;
}

export const LoadingOverlay: React.FC<LoadingOverlayProps> = ({ show }) => (
  <AnimatePresence>
    {show && (
      <motion.div
        className="fixed inset-0 z-[200] flex flex-col items-center justify-center"
        style={{ background: 'var(--base, #0B0A10)' }}
        initial={{ opacity: 1 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0, scale: 1.04 }}
        transition={{ duration: 0.45, ease: [0.4, 0, 0.2, 1] }}
      >
        {/* Ambient glow */}
        <div
          className="absolute inset-0 pointer-events-none"
          style={{
            background: 'radial-gradient(ellipse 60% 40% at 50% 55%, rgba(124,58,237,0.12) 0%, transparent 70%)',
          }}
        />

        <motion.div
          className="relative flex flex-col items-center gap-6"
          initial={{ opacity: 0, y: 16 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ delay: 0.1, duration: 0.5, ease: [0.4, 0, 0.2, 1] }}
        >
          {/* Logo */}
          <motion.img
            src={logoUrl}
            alt="MangaVox"
            className="w-16 h-16 rounded-2xl object-contain"
            style={{ boxShadow: '0 8px 40px rgba(124,58,237,0.35)' }}
            animate={{ scale: [1, 1.04, 1] }}
            transition={{ duration: 2.4, repeat: Infinity, ease: 'easeInOut' }}
          />

          {/* App name */}
          <div className="text-center flex flex-col items-center gap-1">
            <span
              className="text-xl font-bold tracking-tight"
              style={{ color: 'var(--text-1, #f4f4f5)' }}
            >
              MangaVox
            </span>
            <span
              className="text-[11px] tracking-[0.2em] uppercase"
              style={{ color: 'var(--text-3, #71717a)' }}
            >
              Starting up…
            </span>
          </div>

          {/* Spinner bar */}
          <div className="w-32 h-0.5 rounded-full overflow-hidden" style={{ background: 'rgba(255,255,255,0.07)' }}>
            <motion.div
              className="h-full rounded-full"
              style={{ background: 'linear-gradient(90deg, #7C3AED, #2563EB, #7C3AED)' }}
              animate={{ x: ['-100%', '100%'] }}
              transition={{ duration: 1.4, repeat: Infinity, ease: 'easeInOut' }}
            />
          </div>
        </motion.div>
      </motion.div>
    )}
  </AnimatePresence>
);
