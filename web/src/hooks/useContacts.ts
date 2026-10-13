/**
 * Loads the contacts of an address book.
 *
 * Mirrors {@link file://./useEvents.ts}: exposes
 * {@link UseContactsResult.setContacts} so the page can keep the *base*
 * (server-confirmed) list in sync after mutations, feeding React 19's
 * `useOptimistic` on top of it. The loading flag is derived from a request key
 * instead of being written inside the effect (the
 * `react-hooks/set-state-in-effect` rule forbids the latter).
 */

import { useCallback, useEffect, useState, type Dispatch, type SetStateAction } from 'react'
import { toErrorMessage } from '../services/api'
import { CONTACTS_PAGE_SIZE, listContacts } from '../services/contacts'
import type { Contact } from '../types/api'

export interface UseContactsResult {
  contacts: Contact[]
  setContacts: Dispatch<SetStateAction<Contact[]>>
  loading: boolean
  error: string | null
  reload: () => void
}

interface ContactsState {
  items: Contact[]
  error: string | null
  key: string | null
}

/**
 * Drain every page of a collection's contacts.
 *
 * The backend applies `DEFAULT_COLLECTION_LIMIT = 20` when `limit` is omitted,
 * so a bare `listContacts(id)` silently truncates the listing (and therefore
 * the `.vcf` export) after the first page. Walking `limit=200&offset=k` until a
 * short (or empty) page comes back fetches the whole set. The listing endpoint
 * returns a bare array (no `total`), so exhaustion is a page shorter than the
 * requested size.
 */
async function drainContacts(collectionId: string, signal: AbortSignal): Promise<Contact[]> {
  const all: Contact[] = []
  let offset = 0
  for (;;) {
    const page = await listContacts(collectionId, { limit: CONTACTS_PAGE_SIZE, offset }, signal)
    all.push(...page)
    if (page.length < CONTACTS_PAGE_SIZE) {
      break
    }
    offset += page.length
  }
  return all
}

export function useContacts(collectionId: string | null): UseContactsResult {
  const [reloadToken, setReloadToken] = useState(0)
  const [state, setState] = useState<ContactsState>({
    items: [],
    error: null,
    key: null,
  })

  const requestKey = `${collectionId ?? '∅'}#${reloadToken}`

  useEffect(() => {
    if (collectionId === null) {
      return undefined
    }
    const controller = new AbortController()
    // `drainContacts` walks every page (the backend defaults to 20 per request),
    // so contacts beyond the first page stay visible and exportable.
    drainContacts(collectionId, controller.signal)
      .then((items) => {
        // Stale-response guard: a newer request (or unmount) aborted us while
        // this response was in flight, so it must not overwrite fresher state.
        if (controller.signal.aborted) {
          return
        }
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
  }, [collectionId, requestKey])

  const reload = useCallback(() => {
    setReloadToken((token) => token + 1)
  }, [])

  const setContacts = useCallback<Dispatch<SetStateAction<Contact[]>>>((update) => {
    setState((previous) => ({
      ...previous,
      items: typeof update === 'function' ? update(previous.items) : update,
    }))
  }, [])

  return {
    contacts: state.items,
    setContacts,
    loading: collectionId !== null && state.key !== requestKey,
    error: state.error,
    reload,
  }
}
