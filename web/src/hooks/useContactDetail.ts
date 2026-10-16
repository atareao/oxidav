/**
 * Carga el detalle completo de un contacto **bajo demanda** (F22).
 *
 * A diferencia de {@link file://./useContacts.ts}, que carga una colección
 * entera, este hook solo pide cuando hay un contacto seleccionado (`target`
 * distinto de `null`); con `null` no se hace ninguna petición. Comparte con
 * `useContacts` las tres garantías del repo: `AbortController` con limpieza,
 * guarda de respuesta obsoleta (una respuesta que llega tras abortar no pisa el
 * estado) y estado de carga **derivado** de una *request key* (nunca se escribe
 * estado dentro del efecto, que la regla `react-hooks/set-state-in-effect`
 * prohíbe).
 */

import { useCallback, useEffect, useState } from 'react'
import { toErrorMessage } from '../services/api'
import { getContact } from '../services/contacts'
import type { ContactDetail } from '../types/api'

/** El contacto cuyo detalle se quiere; `null` significa "nada seleccionado". */
export interface ContactDetailTarget {
  collectionId: string
  uid: string
}

export interface UseContactDetailResult {
  detail: ContactDetail | null
  loading: boolean
  error: string | null
  reload: () => void
}

interface DetailState {
  detail: ContactDetail | null
  error: string | null
  key: string | null
}

export function useContactDetail(target: ContactDetailTarget | null): UseContactDetailResult {
  const [reloadToken, setReloadToken] = useState(0)
  const [state, setState] = useState<DetailState>({
    detail: null,
    error: null,
    key: null,
  })

  const collectionId = target?.collectionId ?? null
  const uid = target?.uid ?? null

  // La *request key* identifica la petición vigente: cambio de objetivo o
  // recarga cambian la clave y, por derivación, dejan `loading` en `true`.
  const requestKey =
    collectionId === null || uid === null ? null : `${collectionId}#${uid}#${reloadToken}`

  useEffect(() => {
    if (collectionId === null || uid === null || requestKey === null) {
      return undefined
    }
    const controller = new AbortController()
    getContact(collectionId, uid, controller.signal)
      .then((detail) => {
        // Guarda de obsolescencia: una petición más nueva (o el desmontaje) nos
        // abortó mientras esta respuesta viajaba, así que no debe pisar el estado.
        if (controller.signal.aborted) {
          return
        }
        setState({ detail, error: null, key: requestKey })
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          setState({ detail: null, error: toErrorMessage(error), key: requestKey })
        }
      })
    return () => {
      controller.abort()
    }
  }, [collectionId, uid, requestKey])

  const reload = useCallback(() => {
    setReloadToken((token) => token + 1)
  }, [])

  // El estado solo se expone cuando corresponde a la petición vigente: mientras
  // carga (o si no hay objetivo) no se filtra un detalle o error obsoletos.
  const isCurrent = collectionId !== null && uid !== null && state.key === requestKey

  return {
    detail: isCurrent ? state.detail : null,
    loading: collectionId !== null && uid !== null && !isCurrent,
    error: isCurrent ? state.error : null,
    reload,
  }
}
