/**
 * Descubrimiento de zonas horarias IANA para el selector de eventos (F18).
 *
 * El backend (`validate_tzid`) exige una zona IANA **real** y guarda la forma
 * canónica, así que el selector debe ofrecer solo identificadores válidos. La
 * fuente más fiable en el navegador es `Intl.supportedValuesOf('timeZone')`, una
 * API reciente que no existe en todos los motores; por eso se consulta de forma
 * defensiva (existe, es función, dentro de `try/catch`) y se degrada a `[]` en
 * lugar de romper la vista.
 *
 * `Intl.supportedValuesOf` no incluye `UTC` (no es una zona geográfica, pero sí
 * un identificador IANA válido y la zona que devuelve el navegador cuando el
 * sistema está en UTC). Se añade explícitamente para que el valor que propone
 * `browserTimeZone` siempre esté seleccionable.
 */

/**
 * Zonas IANA conocidas por el motor vía `Intl.supportedValuesOf('timeZone')`,
 * garantizando `UTC`. Devuelve `[]` (sin lanzar) cuando la API no está
 * disponible o falla.
 */
export function availableTimeZones(): string[] {
  if (typeof Intl.supportedValuesOf !== 'function') {
    return []
  }

  let zones: string[]
  try {
    zones = Intl.supportedValuesOf('timeZone')
  } catch {
    return []
  }

  return zones.includes('UTC') ? zones : [...zones, 'UTC']
}

/**
 * Zona horaria del navegador, o `undefined` si el motor no la resuelve.
 *
 * Idéntica al cómputo que hacía `Calendar.tsx`: se trata una cadena vacía como
 * ausencia de zona, no como valor válido.
 */
export function browserTimeZone(): string | undefined {
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone
  return zone === undefined || zone === '' ? undefined : zone
}
