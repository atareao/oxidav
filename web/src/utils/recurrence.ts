/**
 * Recurrencia de eventos — espejo puro de la validación `RRULE` del backend
 * (F07/F18, `src/api/events.rs`).
 *
 * El servidor acepta la regla como cadena cruda, pero solo si cabe en
 * {@link MAX_RRULE_LEN} y usa el alfabeto `A-Z 0-9 = ; , -`; además exige que
 * `FREQ` sea conocida, los enteros (`INTERVAL`/`COUNT`) sean `>= 1`, `BYDAY`
 * solo acompañe a `WEEKLY` y `UNTIL` use `YYYYMMDD`. Este módulo reproduce esas
 * reglas en el cliente para que la SPA **nunca** envíe algo que el servidor
 * rechazaría, y para poder resumir en español una regla ya guardada.
 *
 * Principio rector: **no destruimos reglas importadas que no entendemos**. Si
 * el texto no encaja exactamente en el subconjunto soportado, `parseRrule`
 * devuelve `null` (regla "opaca"): el editor la conserva tal cual y
 * `describeRrule` la muestra verbatim en vez de inventarse un resumen.
 *
 * El módulo es puro (sin React ni acceso a red) y determinista: `composeRrule`
 * emite las partes en un orden fijo, de modo que dos reglas equivalentes
 * producen la misma cadena y el round-trip es estable.
 */

/** Frecuencias soportadas por el backend. */
export type RecurFreq = 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY'

/** Cómo termina la recurrencia. */
export type RecurEndKind = 'never' | 'count' | 'until'

/** Días de la semana en notación iCalendar. */
export type Weekday = 'MO' | 'TU' | 'WE' | 'TH' | 'FR' | 'SA' | 'SU'

/** Modelo estructurado de una regla de recurrencia editable. */
export interface RecurrenceRule {
  freq: RecurFreq
  /** Cada cuántos períodos; >= 1 (1 = todos). */
  interval: number
  endKind: RecurEndKind
  /** Requerido cuando `endKind === 'count'`; >= 1. */
  count?: number
  /** Requerido cuando `endKind === 'until'`; formato `YYYYMMDD`. */
  until?: string
  /** Solo para `freq === 'WEEKLY'`; días de la semana. */
  byDay?: Weekday[]
}

/** Longitud máxima de `RRULE` aceptada por el servidor. */
export const MAX_RRULE_LEN = 1024

/** Orden canónico de los días (el mismo que emite `composeRrule`). */
const WEEKDAY_ORDER: readonly Weekday[] = ['MO', 'TU', 'WE', 'TH', 'FR', 'SA', 'SU']

/** Alfabeto permitido por el servidor: `A-Z 0-9 = ; , -`. */
const RRULE_ALPHABET = /^[A-Z0-9=;,-]+$/

/** Claves de `RRULE` que el cliente se atreve a reescribir. */
const KNOWN_KEYS = new Set(['FREQ', 'INTERVAL', 'COUNT', 'UNTIL', 'BYDAY'])

const FREQ_SET = new Set<string>(['DAILY', 'WEEKLY', 'MONTHLY', 'YEARLY'])
const WEEKDAY_SET = new Set<string>(WEEKDAY_ORDER)

/** Etiquetas legibles para el selector de frecuencia. */
export const FREQ_OPTIONS: ReadonlyArray<{ value: RecurFreq; label: string }> = [
  { value: 'DAILY', label: 'Diaria' },
  { value: 'WEEKLY', label: 'Semanal' },
  { value: 'MONTHLY', label: 'Mensual' },
  { value: 'YEARLY', label: 'Anual' },
]

/** Etiquetas legibles (plural) para el selector de días. */
export const WEEKDAY_OPTIONS: ReadonlyArray<{ value: Weekday; label: string }> = [
  { value: 'MO', label: 'Lunes' },
  { value: 'TU', label: 'Martes' },
  { value: 'WE', label: 'Miércoles' },
  { value: 'TH', label: 'Jueves' },
  { value: 'FR', label: 'Viernes' },
  { value: 'SA', label: 'Sábado' },
  { value: 'SU', label: 'Domingo' },
]

/** Nombres en plural y minúscula para los resúmenes hablados. */
const WEEKDAY_PLURAL: Record<Weekday, string> = {
  MO: 'lunes',
  TU: 'martes',
  WE: 'miércoles',
  TH: 'jueves',
  FR: 'viernes',
  SA: 'sábados',
  SU: 'domingos',
}

/** Devuelve `true` si `value` no está vacío-tras-trim y es un entero `>= 1`. */
function isPositiveInteger(value: string): boolean {
  return /^\d+$/.test(value) && Number(value) >= 1
}

/**
 * Normaliza un número al entero `>= 1`, o `undefined` si no lo es.
 *
 * Defensa en profundidad para `composeRrule`: un `2.5` (InputNumber sin
 * `precision`) o un `null` nunca deben colarse en la `RRULE` (el `.` y el `null`
 * no están en el alfabeto del servidor).
 */
function toPositiveInt(value: number | undefined): number | undefined {
  if (value === undefined) {
    return undefined
  }
  const size = Math.trunc(value)
  return Number.isFinite(size) && size >= 1 ? size : undefined
}

/** Normaliza un valor `UNTIL` a `YYYYMMDD`, o `null` si no encaja. */
function normalizeUntil(value: string): string | null {
  // `YYYYMMDD` o `YYYYMMDDT<hhmmss>Z`; el resto (p. ej. con guiones) se rechaza.
  if (/^\d{8}$/.test(value)) {
    return value
  }
  if (/^\d{8}T\d{6}Z$/.test(value)) {
    return value.slice(0, 8)
  }
  return null
}

/** Ordena y deduplica días según el orden canónico MO…SU. */
function canonicalWeekdays(days: readonly Weekday[]): Weekday[] {
  const seen = new Set(days)
  return WEEKDAY_ORDER.filter((day) => seen.has(day))
}

/**
 * Comprueba si un texto respeta el alfabeto y el límite del servidor.
 *
 * Espejo exacto de `validate_rrule`: `true` sii `value` no está vacío-tras-trim,
 * mide como mucho {@link MAX_RRULE_LEN} y todos sus caracteres pertenecen a
 * `A-Z 0-9 = ; , -`. Un valor vacío o solo espacios devuelve `false`.
 */
export function isRruleAlphabet(value: string): boolean {
  if (value.trim() === '' || value.length > MAX_RRULE_LEN) {
    return false
  }
  return RRULE_ALPHABET.test(value)
}

/**
 * Compone la cadena `RRULE` a partir de una regla estructurada.
 *
 * Orden fijo de partes, omitiendo lo no aplicable: `FREQ` siempre; `INTERVAL`
 * solo si es `> 1`; `BYDAY` solo para `WEEKLY` con días; y el fin (`COUNT`,
 * `UNTIL=…T235959Z` o nada). Los días se emiten en orden canónico.
 *
 * **Garantía:** el resultado siempre respeta el alfabeto/límite del servidor
 * (`A-Z0-9=;,-`, {@link MAX_RRULE_LEN}). Por eso `interval`/`count` se normalizan
 * defensivamente a entero `>= 1` y solo se emiten si lo son; un `UNTIL` que no
 * sea `YYYYMMDD` se descarta. Así, aunque un valor llegue malformado (p. ej.
 * `2.5` o `null`), la SPA **nunca** compone una regla que el servidor rechace.
 */
export function composeRrule(rule: RecurrenceRule): string {
  const parts = [`FREQ=${rule.freq}`]

  const interval = toPositiveInt(rule.interval)
  if (interval !== undefined && interval > 1) {
    parts.push(`INTERVAL=${interval}`)
  }

  if (rule.freq === 'WEEKLY' && rule.byDay !== undefined && rule.byDay.length > 0) {
    parts.push(`BYDAY=${canonicalWeekdays(rule.byDay).join(',')}`)
  }

  if (rule.endKind === 'count') {
    const count = toPositiveInt(rule.count)
    if (count !== undefined) {
      parts.push(`COUNT=${count}`)
    }
  } else if (rule.endKind === 'until' && rule.until !== undefined && /^\d{8}$/.test(rule.until)) {
    parts.push(`UNTIL=${rule.until}T235959Z`)
  }

  return parts.join(';')
}

/**
 * Parsea una cadena `RRULE` al subconjunto soportado.
 *
 * Devuelve `null` (regla "opaca", no reescribible) ante cualquier desviación:
 * alfabeto/longitud fuera de rango, claves desconocidas o repetidas, `FREQ`
 * ausente/desconocida, enteros inválidos, `BYDAY` sin `WEEKLY`, `UNTIL` con otro
 * formato o fin ambiguo (`COUNT` y `UNTIL` a la vez).
 */
export function parseRrule(text: string): RecurrenceRule | null {
  if (!isRruleAlphabet(text)) {
    return null
  }

  const segments = text.split(';')
  const values = new Map<string, string>()

  for (const segment of segments) {
    const separator = segment.indexOf('=')
    if (separator <= 0) {
      return null
    }
    const key = segment.slice(0, separator)
    const value = segment.slice(separator + 1)
    if (!KNOWN_KEYS.has(key) || value === '' || values.has(key)) {
      return null
    }
    values.set(key, value)
  }

  const freq = values.get('FREQ')
  if (freq === undefined || !FREQ_SET.has(freq)) {
    return null
  }

  let interval = 1
  const intervalRaw = values.get('INTERVAL')
  if (intervalRaw !== undefined) {
    if (!isPositiveInteger(intervalRaw)) {
      return null
    }
    interval = Number(intervalRaw)
  }

  let byDay: Weekday[] | undefined
  const byDayRaw = values.get('BYDAY')
  if (byDayRaw !== undefined) {
    if (freq !== 'WEEKLY') {
      return null
    }
    const days = byDayRaw.split(',')
    if (days.length === 0 || days.some((day) => !WEEKDAY_SET.has(day))) {
      return null
    }
    byDay = canonicalWeekdays(days as Weekday[])
  }

  const countRaw = values.get('COUNT')
  const untilRaw = values.get('UNTIL')
  if (countRaw !== undefined && untilRaw !== undefined) {
    return null
  }

  const rule: RecurrenceRule = { freq: freq as RecurFreq, interval, endKind: 'never' }

  if (countRaw !== undefined) {
    if (!isPositiveInteger(countRaw)) {
      return null
    }
    rule.endKind = 'count'
    rule.count = Number(countRaw)
  } else if (untilRaw !== undefined) {
    const until = normalizeUntil(untilRaw)
    if (until === null) {
      return null
    }
    rule.endKind = 'until'
    rule.until = until
  }

  if (byDay !== undefined) {
    rule.byDay = byDay
  }

  return rule
}

/** Frase de frecuencia, p. ej. "todos los días" o "cada 2 semanas". */
function translateFreq(rule: RecurrenceRule): string {
  switch (rule.freq) {
    case 'DAILY':
      return rule.interval === 1 ? 'todos los días' : `cada ${rule.interval} días`
    case 'WEEKLY':
      return rule.interval === 1 ? 'todas las semanas' : `cada ${rule.interval} semanas`
    case 'MONTHLY':
      return rule.interval === 1 ? 'todos los meses' : `cada ${rule.interval} meses`
    case 'YEARLY':
      return rule.interval === 1 ? 'todos los años' : `cada ${rule.interval} años`
  }
}

/** Une nombres de días en lenguaje natural: "lunes", "lunes y miércoles", "a, b y c". */
function joinWeekdays(days: readonly Weekday[]): string {
  const names = days.map((day) => WEEKDAY_PLURAL[day])
  if (names.length <= 1) {
    return names.join('')
  }
  return `${names.slice(0, -1).join(', ')} y ${names[names.length - 1]}`
}

/** Formatea `YYYYMMDD` como `DD/MM/YYYY`. */
function formatUntil(until: string): string {
  return `${until.slice(6, 8)}/${until.slice(4, 6)}/${until.slice(0, 4)}`
}

/**
 * Resumen legible en español de una regla `RRULE`.
 *
 * Usa {@link parseRrule}: si la regla es opaca, devuelve el texto verbatim sin
 * lanzar (nunca inventamos un resumen de algo que no entendemos).
 */
export function describeRrule(text: string): string {
  const rule = parseRrule(text)
  if (rule === null) {
    return text
  }

  let description = `Se repite ${translateFreq(rule)}`
  if (rule.byDay !== undefined && rule.byDay.length > 0) {
    description += ` los ${joinWeekdays(rule.byDay)}`
  }
  if (rule.endKind === 'count' && rule.count !== undefined) {
    description += `, ${rule.count} veces`
  } else if (rule.endKind === 'until' && rule.until !== undefined) {
    description += `, hasta el ${formatUntil(rule.until)}`
  }

  return description
}
