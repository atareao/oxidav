/**
 * Tests de `timezone` (F26 / F18).
 *
 * Comprueban el descubrimiento de zonas IANA para el selector de zona horaria
 * de eventos: la lista debe degradar a `[]` sin lanzar cuando el entorno no
 * implementa `Intl.supportedValuesOf`, y la zona del navegador debe resolverse
 * como en `Calendar.tsx` (`undefined` para cadena vacía).
 *
 * El escenario 9 parchea temporalmente `Intl.supportedValuesOf` a `undefined` y
 * lo restaura en `finally`, porque es una propiedad global escribible.
 */

import { describe, expect, it } from 'vitest'
import { availableTimeZones, browserTimeZone } from './timezone'

describe('availableTimeZones', () => {
  // Escenario 8.
  it('incluye Europe/Madrid y UTC cuando Intl.supportedValuesOf está disponible', () => {
    if (typeof Intl.supportedValuesOf !== 'function') {
      // Entorno sin la API: el contrato es la lista vacía (cubierto abajo).
      expect(availableTimeZones()).toEqual([])
      return
    }
    const zones = availableTimeZones()
    expect(Array.isArray(zones)).toBe(true)
    expect(zones).toContain('Europe/Madrid')
    expect(zones).toContain('UTC')
  })

  // Escenario 9.
  it('devuelve [] sin lanzar cuando Intl.supportedValuesOf no existe', () => {
    const original = Intl.supportedValuesOf
    try {
      // @ts-expect-error — simulamos un entorno sin la API.
      Intl.supportedValuesOf = undefined
      expect(availableTimeZones()).toEqual([])
    } finally {
      Intl.supportedValuesOf = original
    }
  })
})

describe('browserTimeZone', () => {
  it('devuelve una zona no vacía o undefined', () => {
    const zone = browserTimeZone()
    if (zone === undefined) {
      expect(zone).toBeUndefined()
    } else {
      expect(typeof zone).toBe('string')
      expect(zone.length).toBeGreaterThan(0)
    }
  })
})
