/**
 * Loads the events of a calendar collection within a time window.
 *
 * Exposes {@link UseEventsResult.setEvents} so the page can keep the *base*
 * (server-confirmed) list in sync after mutations, feeding React 19's
 * `useOptimistic` on top of it. Like {@link file://./useCollections.ts}, the
 * loading flag is derived from a request key instead of set inside the effect.
 */

import { useCallback, useEffect, useState, type Dispatch, type SetStateAction } from 'react'
import { toErrorMessage } from '../services/api'
import { listEvents, type EventRange } from '../services/events'
import type { CalendarEvent } from '../types/api'

export interface UseEventsResult {
  events: CalendarEvent[]
  setEvents: Dispatch<SetStateAction<CalendarEvent[]>>
  loading: boolean
  error: string | null
  reload: () => void
}

interface EventsState {
  items: CalendarEvent[]
  error: string | null
  key: string | null
}

export function useEvents(
  collectionId: string | null,
  range: EventRange,
): UseEventsResult {
  const [reloadToken, setReloadToken] = useState(0)
  const [state, setState] = useState<EventsState>({
    items: [],
    error: null,
    key: null,
  })

  const requestKey = `${collectionId ?? '∅'}#${range.start}#${range.end}#${reloadToken}`

  useEffect(() => {
    if (collectionId === null) {
      return undefined
    }
    const controller = new AbortController()
    listEvents(collectionId, range, controller.signal)
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
  }, [collectionId, range, requestKey])

  const reload = useCallback(() => {
    setReloadToken((token) => token + 1)
  }, [])

  const setEvents = useCallback<Dispatch<SetStateAction<CalendarEvent[]>>>((update) => {
    setState((previous) => ({
      ...previous,
      items: typeof update === 'function' ? update(previous.items) : update,
    }))
  }, [])

  return {
    events: state.items,
    setEvents,
    loading: collectionId !== null && state.key !== requestKey,
    error: state.error,
    reload,
  }
}
