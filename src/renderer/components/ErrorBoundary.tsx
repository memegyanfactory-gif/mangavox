import { Component, type ErrorInfo, type ReactNode } from 'react'
import { AlertTriangle, ArrowLeft, RotateCcw } from 'lucide-react'
import { invoke } from '../hooks/useIpc'
import { IPC } from '../../shared/ipc-channels'
import { useUiStore } from '../stores/ui.store'
import { reportError } from '../debug/debug-bridge'

interface Props {
  children: ReactNode
  playback?: boolean
}

interface State {
  error: Error | null
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Log locally only (not via console.error, which would forward a duplicate), then forward with the component stack.
    console.log('[renderer-boundary]', error, info.componentStack)
    reportError('boundary', error, { componentStack: info.componentStack ?? undefined })
  }

  private reset = (): void => {
    this.setState({ error: null })
  }

  private returnToCatalog = async (): Promise<void> => {
    await invoke(IPC.PLAYBACK_SESSION_STOP).catch(() => null)
    useUiStore.getState().replace('movies')
    this.reset()
  }

  render() {
    if (!this.state.error) return this.props.children
    return (
      <div className="h-full w-full flex items-center justify-center bg-[var(--base)] p-8">
        <div className="glass-strong max-w-lg w-full rounded-2xl p-6 text-center">
          <AlertTriangle size={30} className="mx-auto mb-3 text-amber-400" />
          <h2 className="text-lg font-semibold text-white">
            {this.props.playback ? 'Playback stopped safely' : 'This page ran into a problem'}
          </h2>
          <p className="text-sm text-[var(--text-2)] mt-2">
            MangaVox contained the failure. Your saved progress and library were not removed.
          </p>
          <details className="mt-4 text-left">
            <summary className="text-xs text-[var(--text-3)] cursor-pointer">Diagnostic details</summary>
            <pre className="mt-2 p-3 rounded-lg bg-black/40 text-[10px] text-[var(--text-3)] overflow-auto max-h-32 whitespace-pre-wrap">
              {this.state.error.message}
            </pre>
          </details>
          <div className="flex justify-center gap-2 mt-5">
            <button onClick={this.reset} className="px-4 py-2 rounded-lg bg-[var(--surface-2)] text-xs text-white flex items-center gap-2">
              <RotateCcw size={13} /> Retry page
            </button>
            <button onClick={() => void this.returnToCatalog()} className="px-4 py-2 rounded-lg bg-emerald-500/15 text-xs text-emerald-300 flex items-center gap-2">
              <ArrowLeft size={13} /> Return to catalog
            </button>
          </div>
        </div>
      </div>
    )
  }
}

