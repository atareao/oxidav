/**
 * Título del documento por ruta (F19, Bloque D).
 *
 * `web/index.html` fija un `<title>OxiDav</title>` estático. Este hook lo
 * actualiza según la ruta activa y lo **restaura al título base** al desmontar,
 * de modo que un cambio de vista se refleje en la pestaña y el historial del
 * navegador sin dejar títulos colgando.
 */

import { useEffect } from 'react'

/** Título base, idéntico al `<title>` de `web/index.html`. */
export const BASE_TITLE = 'OxiDav'

/** Ruta raíz → título con sufijo de marca. */
const ROUTE_TITLES: ReadonlyArray<readonly [string, string]> = [
  ['/calendar', `Calendario · ${BASE_TITLE}`],
  ['/contacts', `Contactos · ${BASE_TITLE}`],
  ['/sync', `Sincronización · ${BASE_TITLE}`],
  ['/settings', `Ajustes · ${BASE_TITLE}`],
]

/**
 * Título correspondiente a un `pathname`. Casa la ruta exacta o cualquier
 * subruta suya (`/contacts/42` → Contactos); el resto cae al título base.
 */
export function titleForPath(pathname: string): string {
  return (
    ROUTE_TITLES.find(
      ([route]) => pathname === route || pathname.startsWith(`${route}/`),
    )?.[1] ?? BASE_TITLE
  )
}

/** Aplica `title` a `document.title` y restaura {@link BASE_TITLE} al desmontar. */
export function useDocumentTitle(title: string): void {
  useEffect(() => {
    document.title = title
    return () => {
      document.title = BASE_TITLE
    }
  }, [title])
}
