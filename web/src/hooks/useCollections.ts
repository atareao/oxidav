/**
 * Loads the caller's collections.
 *
 * Loading state is **derived** from a request key rather than set synchronously
 * inside the effect (which the `react-hooks/set-state-in-effect` rule forbids):
 * the effect only writes state from the async callbacks, and `loading` is true
 * whenever the last settled key differs from the current one.
 */

import { useCallback, useEffect, useState } from 'react'
import { toErrorMessage } from '../services/api'
import { listCollections } from '../services/collections'
import type { Collection } from '../types/api'

export interface UseCollectionsResult {
  collections: Collection[]
  loading: boolean
  error: string | null
  reload: () => void
}

interface CollectionsState {
  items: Collection[]
  error: string | null
  key: string | null
}

export function useCollections(): UseCollectionsResult {
  const [reloadToken, setReloadToken] = useState(0)
  const [state, setState] = useState<CollectionsState>({
    items: [],
    error: null,
    key: null,
  })

  const requestKey = `collections#${reloadToken}`

  useEffect(() => {
    const controller = new AbortController()
    listCollections(controller.signal)
      .then((items) => {
        setState({ items, error: null, key: requestKey })
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          setState((previous) => ({
            ...previous,
            error: toErrorMessage(error),
            key: requestKey,
          }))
        }
      })
    return () => {
      controller.abort()
    }
  }, [requestKey])

  const reload = useCallback(() => {
    setReloadToken((token) => token + 1)
  }, [])

  return {
    collections: state.items,
    loading: state.key !== requestKey,
    error: state.error,
    reload,
  }
}
