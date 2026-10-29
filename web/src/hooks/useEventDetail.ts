/**
 * Carga el detalle completo de un evento **bajo demanda** (F25).
 *
 * Espejo de {@link file://./useContactDetail.ts}: a diferencia de
 * {@link file://./useEvents.ts}, que carga la colección entera, este hook solo
 * pide cuando hay un evento seleccionado (`target` distinto de `null`); con
 * `null` no se hace ninguna petición. Comparte con él las tres garantías del
 * repo: `AbortController` con limpieza, guarda de respuesta obsoleta (una
 * respuesta que llega tras abortar no pisa el estado) y estado de carga
 * **derivado** de una *request key* (nunca se escribe estado dentro del efecto,
 * que la regla `react-hooks/set-state-in-effect` prohíbe).
 */

import { useCallback, useEffect, useState } from 'react'
import { toErrorMessage } from '../services/api'
import { getEvent } from '../services/events'
import type { EventDetail } from '../types/api'

/** El evento cuyo detalle se quiere; `null` significa "nada seleccionado". */
export interface EventDetailTarget {
  collectionId: string
  uid: string
  /**
   * El `etag` de la fila que se está abriendo. Entra en la *request key* para
   * que guardar el evento invalide el detalle cacheado: tras un `PATCH` la lista
   * consolida la fila con un `etag` nuevo, así que reabrir **el mismo** evento
   * vuelve a pedir el detalle en lugar de mostrar el anterior al guardado.
   * (Sin él, la clave `collectionId#uid#reloadToken` no cambiaba al reabrir y el
   * diálogo mostraba el detalle obsoleto.)
   */
  etag: string
}

export interface UseEventDetailResult {
  detail: EventDetail | null
  loading: boolean
  error: string | null
  reload: () => void
}

interface DetailState {
  detail: EventDetail | null
  error: string | null
  key: string | null
}

export function useEventDetail(target: EventDetailTarget | null): UseEventDetailResult {
  const [reloadToken, setReloadToken] = useState(0)
  const [state, setState] = useState<DetailState>({
    detail: null,
    error: null,
    key: null,
  })

  const collectionId = target?.collectionId ?? null
  const uid = target?.uid ?? null
  const etag = target?.etag ?? null

  // La *request key* identifica la petición vigente: cambio de objetivo, de
  // `etag` o recarga cambian la clave y, por derivación, dejan `loading` en
  // `true`. El `etag` es lo que hace que **guardar** invalide el detalle: la
  // lista consolida la fila con un `etag` nuevo tras el `PATCH`, de modo que
  // reabrir el mismo evento fuerza una petición fresca.
  const requestKey =
    collectionId === null || uid === null
      ? null
      : `${collectionId}#${uid}#${etag ?? ''}#${reloadToken}`

  useEffect(() => {
    if (collectionId === null || uid === null || requestKey === null) {
      return undefined
    }
    const controller = new AbortController()
    getEvent(collectionId, uid, controller.signal)
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
