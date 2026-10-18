/**
 * Responsive breakpoint hooks for the renderer.
 *
 * Breakpoints are measured in CSS pixels (window.innerWidth, i.e. after the
 * 0.85 renderer zoom) so they line up exactly with Tailwind's `sm:` / `lg:`
 * media queries:
 *
 *   'sm'  — width <  640  → phone-sized / compact (bottom tab bar is shown)
 *   'md'  — 640 ≤ w < 1024 → tablet-ish (top-bar tabs collapse into the drawer)
 *   'lg'  — width ≥ 1024  → full desktop layout
 *
 * All hooks share a single resize listener and re-render only when the
 * breakpoint (or touch capability) actually changes.
 */
import { useSyncExternalStore } from 'react'

export type Breakpoint = 'sm' | 'md' | 'lg'

export const BREAKPOINTS = { md: 640, lg: 1024 } as const

export function breakpointFor(width: number): Breakpoint {
  if (width < BREAKPOINTS.md) return 'sm'
  if (width < BREAKPOINTS.lg) return 'md'
  return 'lg'
}

const hasWindow = typeof window !== 'undefined'

function read(): Breakpoint {
  return hasWindow ? breakpointFor(window.innerWidth) : 'lg'
}

let current: Breakpoint = read()
const listeners = new Set<() => void>()

function onResize() {
  const next = read()
  if (next === current) return
  current = next
  for (const fn of listeners) fn()
}

function subscribe(fn: () => void) {
  if (listeners.size === 0 && hasWindow) {
    current = read()
    window.addEventListener('resize', onResize)
  }
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
    if (listeners.size === 0 && hasWindow) window.removeEventListener('resize', onResize)
  }
}

/** Current breakpoint: 'sm' (<640px) | 'md' (640–1023px) | 'lg' (≥1024px). */
export function useBreakpoint(): Breakpoint {
  return useSyncExternalStore(subscribe, () => current, () => 'lg' as Breakpoint)
}

/** True on compact (phone-width) layouts — same as `useBreakpoint() === 'sm'`. */
export function useIsCompact(): boolean {
  return useBreakpoint() === 'sm'
}

/** True below the desktop breakpoint ('sm' or 'md'). */
export function useIsBelowDesktop(): boolean {
  return useBreakpoint() !== 'lg'
}

// ── Touch / coarse pointer ────────────────────────────────────────────────
const coarseQuery = hasWindow && window.matchMedia ? window.matchMedia('(hover: none), (pointer: coarse)') : null

function subscribeTouch(fn: () => void) {
  coarseQuery?.addEventListener('change', fn)
  return () => coarseQuery?.removeEventListener('change', fn)
}

/** True when the primary input can't hover (touch screens). */
export function useIsTouch(): boolean {
  return useSyncExternalStore(subscribeTouch, () => !!coarseQuery?.matches, () => false)
}
