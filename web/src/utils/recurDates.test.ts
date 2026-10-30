/**
 * Tests de `recurDates` (F27).
 *
 * Cubren las utilidades puras que respaldan la edición de `EXDATE`/`RDATE` en
 * la SPA: la forma canónica (orden ascendente y sin duplicados) que espeja la
 * del servidor, la comparación de conjuntos que sostiene la disciplina D2
 * (enviar solo lo que cambió) y el resumen legible en español.
 *
 * Las fechas de `describeRecurDates` se construyen por componentes
 * (`new Date(2026, 2, 15, 10, 0)`) para que el formato esperado no dependa del
 * huso horario del runner: `dayjs` formatea en hora local, así que una fecha
 * local se imprime igual esté donde esté el test.
 */

import { describe, expect, it } from 'vitest'
import { describeRecurDates, sameInstantSet, sortUniqueInstants } from './recurDates'

describe('sortUniqueInstants', () => {
  it('devuelve una copia ordenada ascendente', () => {
    expect(sortUniqueInstants([300, 100, 200])).toEqual([100, 200, 300])
  })

  it('elimina duplicados', () => {
    expect(sortUniqueInstants([200, 100, 200, 100])).toEqual([100, 200])
  })

  it('no muta la entrada', () => {
    const input = [300, 100, 200, 100]
    const snapshot = [...input]
    sortUniqueInstants(input)
    expect(input).toEqual(snapshot)
  })

  it('devuelve [] para una lista vacía', () => {
    expect(sortUniqueInstants([])).toEqual([])
  })
})

describe('sameInstantSet', () => {
  it('es true con el mismo conjunto en distinto orden', () => {
    expect(sameInstantSet([100, 200, 300], [300, 100, 200])).toBe(true)
  })

  it('es true con duplicados en un lado', () => {
    expect(sameInstantSet([100, 100, 200], [200, 100])).toBe(true)
  })

  it('es false con elementos distintos', () => {
    expect(sameInstantSet([100, 200], [100, 300])).toBe(false)
  })

  it('es false con distinta longitud (no reintroduce duplicados)', () => {
    expect(sameInstantSet([100, 200, 300], [100, 200])).toBe(false)
  })

  it('es false entre [] y [x]', () => {
    expect(sameInstantSet([], [100])).toBe(false)
  })

  it('es true entre dos listas vacías', () => {
    expect(sameInstantSet([], [])).toBe(true)
  })
})

describe('describeRecurDates', () => {
  it('devuelve cadena vacía para una lista vacía', () => {
    expect(describeRecurDates([])).toBe('')
  })

  it('describe una sola fecha', () => {
    const one = new Date(2026, 2, 15, 10, 0).getTime()
    expect(describeRecurDates([one])).toBe('1 fecha: 15/03/2026 10:00')
  })

  it('describe varias fechas en orden, con el recuento delante', () => {
    const first = new Date(2026, 2, 15, 10, 0).getTime()
    const second = new Date(2026, 2, 22, 10, 0).getTime()
    const third = new Date(2026, 2, 29, 10, 0).getTime()
    // Entrada desordenada y con duplicado: el resumen ordena y deduplica antes.
    expect(describeRecurDates([third, first, second, first])).toBe(
      '3 fechas: 15/03/2026 10:00, 22/03/2026 10:00, 29/03/2026 10:00',
    )
  })
})
