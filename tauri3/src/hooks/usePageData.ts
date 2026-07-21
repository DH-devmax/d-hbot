import { useCallback, useEffect, useRef, useState } from 'react'
import { readableError } from '../api/client'
import type { LoadState } from '../types'

export function usePageData<T>(loader: () => Promise<T>, dependencies: unknown[], isEmpty: (value: T) => boolean) {
  const [data, setData] = useState<T | null>(null)
  const dataRef = useRef<T | null>(null)
  const [state, setState] = useState<LoadState>('idle')
  const [error, setError] = useState('')

  const reload = useCallback(async (quiet = false) => {
    if (!quiet) setState('loading')
    setError('')
    try {
      const next = await loader()
      dataRef.current = next
      setData(next)
      setState(isEmpty(next) ? 'empty' : 'ready')
      return next
    } catch (reason) {
      setError(readableError(reason))
      setState(dataRef.current == null ? 'error' : 'offlineCached')
      return null
    }
  // dependencies intentionally define the page resource identity.
  }, dependencies)

  useEffect(() => {
    dataRef.current = null
    setData(null)
    setState('idle')
    void reload()
  }, [reload])
  return { data, setData, state, error, reload }
}
