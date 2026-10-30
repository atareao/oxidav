/**
 * Fechas de recurrencia — utilidades puras para editar `EXDATE`/`RDATE` (F27).
 *
 * `EXDATE` excluye instantes concretos de una serie recurrente y `RDATE` añade
 * instantes extra. El backend los expone (y acepta) ya resueltos a Unix epoch
 * **milisegundos** y en su **forma canónica**: ordenados ascendente y sin
 * duplicados. Este módulo espeja esa forma en el cliente para que la SPA nunca
 * envíe algo distinto de lo que el servidor guardaría, y para poder resumir en
 * español el conjunto al usuario.
 *
 * Principio rector (disciplina **D2**, la misma que en `recurrence.ts`): solo se
 * envía una lista cuando **cambia** respecto del baseline. Eso exige comparar
 * por **conjunto** (independiente del orden y de duplicados), no por igualdad
 * posicional de arrays: de ahí {@link sameInstantSet}.
 *
 * El módulo es puro (sin React ni acceso a red) y determinista.
 */

import dayjs from 'dayjs'

/**
 * Devuelve una copia de `values` en forma canónica: orden ascendente y sin
 * duplicados.
 *
 * Misma forma que produce el servidor al serializar `EXDATE`/`RDATE`, de modo
 * que comparar contra lo recibido por la API sea significativo. Nunca muta la
 * entrada: siempre devuelve un array nuevo.
 */
export function sortUniqueInstants(values: readonly number[]): number[] {
  return [...new Set(values)].sort((a, b) => a - b)
}

/**
 * Compara dos listas como **conjuntos de instantes**: `true` sii contienen
 * exactamente los mismos valores, sin importar el orden ni los duplicados.
 *
 * Es la base de la disciplina D2: la SPA decide si enviar `exdate`/`rdate`
 * comparando la selección actual contra el baseline con esta función, no con
 * una igualdad de arrays (que fallaría ante el mismo conjunto en otro orden).
 */
export function sameInstantSet(a: readonly number[], b: readonly number[]): boolean {
  const left = sortUniqueInstants(a)
  const right = sortUniqueInstants(b)
  if (left.length !== right.length) {
    return false
  }
  return left.every((value, index) => value === right[index])
}

/**
 * Resume un conjunto de instantes en español para la UI.
 *
 * `''` con lista vacía. Con una sola fecha, `"1 fecha: 15/03/2026 10:00"`; con
 * varias, `"3 fechas: …"` seguido de las fechas separadas por `", "`. Ordena y
 * deduplica antes de describir (véase {@link sortUniqueInstants}) para que el
 * texto no dependa del orden de selección. El formato de cada fecha es
 * `DD/MM/YYYY HH:mm` en la hora local del navegador.
 */
export function describeRecurDates(instants: readonly number[]): string {
  const canonical = sortUniqueInstants(instants)
  if (canonical.length === 0) {
    return ''
  }
  const rendered = canonical.map((instant) => dayjs(instant).format('DD/MM/YYYY HH:mm'))
  const label = canonical.length === 1 ? '1 fecha' : `${canonical.length} fechas`
  return `${label}: ${rendered.join(', ')}`
}
