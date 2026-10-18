import { useState, useCallback, useEffect } from 'react'

declare global {
  interface Window {
    mangavox: {
      invoke: (channel: string, ...args: unknown[]) => Promise<unknown>
      on: (channel: string, callback: (...args: unknown[]) => void) => () => void
    }
  }
}

const ipc = window.mangavox

export function useIpcInvoke<T>(channel: string, ...args: unknown[]) {
  const [data, setData] = useState<T | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<Error | null>(null)

  const fetch = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const result = (await ipc.invoke(channel, ...args)) as T
      setData(result)
    } catch (e) {
      setError(e instanceof Error ? e : new Error(String(e)))
    } finally {
      setLoading(false)
    }
  }, [channel, ...args])

  useEffect(() => {
    fetch()
  }, [fetch])

  return { data, loading, error, refetch: fetch }
}

export async function invoke<T>(channel: string, ...args: unknown[]): Promise<T> {
  return (await ipc.invoke(channel, ...args)) as T
}

export function onEvent(channel: string, callback: (...args: unknown[]) => void): () => void {
  return ipc.on(channel, callback)
}
