/**
 * Tests de `recurrence` (F26 / F07).
 *
 * El módulo es un espejo **puro** de la validación de `RRULE` del backend
 * (`src/api/events.rs`): alfabeto `A-Z0-9=;,-`, límite `MAX_RRULE_LEN` y sin
 * reescritura de reglas "raras". Estos tests fijan los escenarios 1-7 del plan
 * más bordes del alfabeto/longitud, de modo que la SPA nunca envíe al servidor
 * una regla que este rechazaría.
 */

import { describe, expect, it } from 'vitest'
import {
  FREQ_OPTIONS,
  MAX_RRULE_LEN,
  WEEKDAY_OPTIONS,
  composeRrule,
  describeRrule,
  isRruleAlphabet,
  parseRrule,
  type RecurrenceRule,
} from './recurrence'

describe('catálogos de opciones', () => {
  it('expone las cuatro frecuencias en su orden', () => {
    expect(FREQ_OPTIONS.map((option) => option.value)).toEqual([
      'DAILY',
      'WEEKLY',
      'MONTHLY',
      'YEARLY',
    ])
    for (const option of FREQ_OPTIONS) {
      expect(option.label.length).toBeGreaterThan(0)
    }
  })

  it('expone los siete días en orden canónico (MO…SU)', () => {
    expect(WEEKDAY_OPTIONS.map((option) => option.value)).toEqual([
      'MO',
      'TU',
      'WE',
      'TH',
      'FR',
      'SA',
      'SU',
    ])
    for (const option of WEEKDAY_OPTIONS) {
      expect(option.label.length).toBeGreaterThan(0)
    }
  })
})

describe('composeRrule', () => {
  // Escenario 1 — DAILY simple: sin INTERVAL (==1) ni fin.
  it('compone una regla DAILY simple', () => {
    expect(composeRrule({ freq: 'DAILY', interval: 1, endKind: 'never' })).toBe('FREQ=DAILY')
  })

  // Escenario 2 — WEEKLY con intervalo + BYDAY + COUNT, en orden canónico.
  it('compone WEEKLY con intervalo, BYDAY (canónico) y COUNT', () => {
    const rule: RecurrenceRule = {
      freq: 'WEEKLY',
      interval: 2,
      byDay: ['MO', 'WE'],
      endKind: 'count',
      count: 5,
    }
    expect(composeRrule(rule)).toBe('FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=5')
  })

  it('reordena los días al orden canónico MO,TU,WE,TH,FR,SA,SU y deduplica', () => {
    const rule: RecurrenceRule = {
      freq: 'WEEKLY',
      interval: 1,
      byDay: ['WE', 'MO', 'WE', 'FR'],
      endKind: 'never',
    }
    expect(composeRrule(rule)).toBe('FREQ=WEEKLY;BYDAY=MO,WE,FR')
  })

  it('ignora BYDAY cuando la frecuencia no es WEEKLY', () => {
    const rule: RecurrenceRule = {
      freq: 'DAILY',
      interval: 1,
      byDay: ['MO'],
      endKind: 'never',
    }
    expect(composeRrule(rule)).toBe('FREQ=DAILY')
  })

  // Escenario 3 — YEARLY con UNTIL, normalizado a fin de día UTC.
  it('compone YEARLY con UNTIL en formato UTC de fin de día', () => {
    expect(composeRrule({ freq: 'YEARLY', endKind: 'until', until: '20261231', interval: 1 })).toBe(
      'FREQ=YEARLY;UNTIL=20261231T235959Z',
    )
  })

  it('omite INTERVAL cuando vale 1', () => {
    expect(composeRrule({ freq: 'MONTHLY', interval: 1, endKind: 'never' })).toBe('FREQ=MONTHLY')
  })

  it('incluye INTERVAL cuando es mayor que 1', () => {
    expect(composeRrule({ freq: 'MONTHLY', interval: 3, endKind: 'never' })).toBe(
      'FREQ=MONTHLY;INTERVAL=3',
    )
  })
})

describe('parseRrule', () => {
  // Escenario 1 (ida) — DAILY simple.
  it('parsea una regla DAILY simple', () => {
    expect(parseRrule('FREQ=DAILY')).toEqual<RecurrenceRule>({
      freq: 'DAILY',
      interval: 1,
      endKind: 'never',
    })
  })

  // Escenario 2 (ida).
  it('parsea WEEKLY con intervalo, BYDAY y COUNT', () => {
    expect(parseRrule('FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=5')).toEqual<RecurrenceRule>({
      freq: 'WEEKLY',
      interval: 2,
      byDay: ['MO', 'WE'],
      endKind: 'count',
      count: 5,
    })
  })

  // Escenario 3 (ida) — UNTIL largo se normaliza a YYYYMMDD.
  it('parsea YEARLY con UNTIL y lo normaliza a YYYYMMDD', () => {
    expect(parseRrule('FREQ=YEARLY;UNTIL=20261231T235959Z')).toEqual<RecurrenceRule>({
      freq: 'YEARLY',
      interval: 1,
      endKind: 'until',
      until: '20261231',
    })
  })

  it('acepta UNTIL corto (YYYYMMDD)', () => {
    expect(parseRrule('FREQ=DAILY;UNTIL=20261231')).toEqual<RecurrenceRule>({
      freq: 'DAILY',
      interval: 1,
      endKind: 'until',
      until: '20261231',
    })
  })

  it('normaliza BYDAY al orden canónico', () => {
    expect(parseRrule('FREQ=WEEKLY;BYDAY=FR,MO')).toEqual<RecurrenceRule>({
      freq: 'WEEKLY',
      interval: 1,
      byDay: ['MO', 'FR'],
      endKind: 'never',
    })
  })

  // Escenario 5 — clave desconocida ⇒ opaca (null).
  it('devuelve null ante una clave desconocida', () => {
    expect(parseRrule('FREQ=MONTHLY;BYMONTHDAY=15')).toBeNull()
  })

  it('devuelve null si falta FREQ', () => {
    expect(parseRrule('INTERVAL=2')).toBeNull()
  })

  it('devuelve null si FREQ es desconocida', () => {
    expect(parseRrule('FREQ=HOURLY')).toBeNull()
  })

  it('devuelve null si INTERVAL no es un entero >= 1', () => {
    expect(parseRrule('FREQ=DAILY;INTERVAL=0')).toBeNull()
    expect(parseRrule('FREQ=DAILY;INTERVAL=x')).toBeNull()
  })

  it('devuelve null si COUNT no es un entero >= 1', () => {
    expect(parseRrule('FREQ=DAILY;COUNT=0')).toBeNull()
    expect(parseRrule('FREQ=DAILY;COUNT=-1')).toBeNull()
  })

  it('devuelve null si UNTIL tiene otro formato', () => {
    expect(parseRrule('FREQ=DAILY;UNTIL=2026-12-31')).toBeNull()
  })

  it('devuelve null si BYDAY aparece sin WEEKLY', () => {
    expect(parseRrule('FREQ=DAILY;BYDAY=MO')).toBeNull()
  })

  it('devuelve null si BYDAY trae un día desconocido', () => {
    expect(parseRrule('FREQ=WEEKLY;BYDAY=XX')).toBeNull()
  })

  it('devuelve null si UNTIL y COUNT aparecen juntos (fin ambiguo)', () => {
    expect(parseRrule('FREQ=DAILY;COUNT=3;UNTIL=20261231')).toBeNull()
  })

  it('devuelve null para una clave repetida', () => {
    expect(parseRrule('FREQ=DAILY;FREQ=WEEKLY')).toBeNull()
  })

  it('devuelve null ante un segmento sin `=` o vacío', () => {
    expect(parseRrule('FREQ=DAILY;')).toBeNull()
    expect(parseRrule('FREQ=DAILY;;INTERVAL=2')).toBeNull()
  })

  // Escenario 6 — minúsculas / fuera de alfabeto.
  it('devuelve null ante minúsculas', () => {
    expect(parseRrule('freq=daily')).toBeNull()
  })

  it('devuelve null fuera del alfabeto', () => {
    expect(parseRrule('FREQ=DAILY ')).toBeNull()
    expect(parseRrule('FREQ=DAILY\n')).toBeNull()
  })

  it('devuelve null si el texto excede MAX_RRULE_LEN', () => {
    const huge = `FREQ=WEEKLY;BYDAY=${';'.repeat(MAX_RRULE_LEN)}` // > 1024 chars y ';' sobrante
    expect(huge.length).toBeGreaterThan(MAX_RRULE_LEN)
    expect(parseRrule(huge)).toBeNull()
  })
})

// Escenario 4 — round-trip.
describe('round-trip parseRrule(composeRrule(rule))', () => {
  const rules: RecurrenceRule[] = [
    { freq: 'DAILY', interval: 1, endKind: 'never' },
    { freq: 'DAILY', interval: 3, endKind: 'never' },
    { freq: 'WEEKLY', interval: 1, byDay: ['MO', 'WE'], endKind: 'never' },
    { freq: 'WEEKLY', interval: 2, byDay: ['MO', 'WE'], endKind: 'count', count: 5 },
    { freq: 'WEEKLY', interval: 1, byDay: ['SU'], endKind: 'count', count: 1 },
    { freq: 'MONTHLY', interval: 1, endKind: 'until', until: '20261231' },
    { freq: 'YEARLY', interval: 1, endKind: 'until', until: '20260101' },
  ]

  it.each(rules)('reproduce $freq (intervalo $interval, fin $endKind)', (rule) => {
    const text = composeRrule(rule)
    expect(parseRrule(text)).toEqual(rule)
    // Y volver a componer es idempotente.
    expect(composeRrule(parseRrule(text) as RecurrenceRule)).toBe(text)
  })
})

// Escenario 7 — describeRrule.
describe('describeRrule', () => {
  it('describe una regla DAILY simple', () => {
    expect(describeRrule('FREQ=DAILY')).toBe('Se repite todos los días')
  })

  it('describe WEEKLY con intervalo, días y COUNT', () => {
    const text = describeRrule('FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=5')
    expect(text).toContain('cada 2 semanas')
    expect(text).toContain('lunes')
    expect(text).toContain('miércoles')
    expect(text).toContain('5 veces')
  })

  it('describe MONTHLY/YEARLY con UNTIL', () => {
    expect(describeRrule('FREQ=MONTHLY')).toContain('meses')
    expect(describeRrule('FREQ=YEARLY;UNTIL=20261231')).toContain('2026')
  })

  it('describe los días de la semana en plural', () => {
    const text = describeRrule('FREQ=WEEKLY;BYDAY=MO,WE,FR')
    expect(text).toContain('lunes')
    expect(text).toContain('viernes')
  })

  it('enumera tres o más días con coma y «y» (a, b y c)', () => {
    expect(describeRrule('FREQ=WEEKLY;BYDAY=MO,WE,FR')).toContain('lunes, miércoles y viernes')
  })

  it('devuelve un resumen no vacío para toda regla válida', () => {
    for (const rule of [
      { freq: 'DAILY', interval: 1, endKind: 'never' },
      { freq: 'WEEKLY', interval: 2, byDay: ['MO'], endKind: 'count', count: 2 },
      { freq: 'YEARLY', interval: 1, endKind: 'until', until: '20261231' },
    ] satisfies RecurrenceRule[]) {
      const text = describeRrule(composeRrule(rule))
      expect(text.length).toBeGreaterThan(0)
    }
  })

  // Escenario 7 — regla opaca ⇒ verbatim, sin lanzar.
  it('devuelve el texto verbatim cuando la regla es opaca', () => {
    const opaque = 'FREQ=MONTHLY;BYMONTHDAY=15'
    expect(describeRrule(opaque)).toBe(opaque)
  })
})

describe('isRruleAlphabet', () => {
  it('acepta un texto dentro del alfabeto y del límite', () => {
    expect(isRruleAlphabet('FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=5')).toBe(true)
    expect(isRruleAlphabet('FREQ=YEARLY;UNTIL=20261231T235959Z')).toBe(true)
  })

  it('rechaza el valor vacío o solo espacios', () => {
    expect(isRruleAlphabet('')).toBe(false)
    expect(isRruleAlphabet('   ')).toBe(false)
  })

  it('rechaza minúsculas y caracteres fuera del alfabeto', () => {
    expect(isRruleAlphabet('freq=daily')).toBe(false)
    expect(isRruleAlphabet('FREQ=DAILY ')).toBe(false)
    expect(isRruleAlphabet('FREQ=DAILY+1')).toBe(false)
  })

  it('respeta MAX_RRULE_LEN (borde inclusivo)', () => {
    expect(isRruleAlphabet('A'.repeat(MAX_RRULE_LEN))).toBe(true)
    expect(isRruleAlphabet('A'.repeat(MAX_RRULE_LEN + 1))).toBe(false)
  })
})

/**
 * Canonicalización de formas **parseables pero no canónicas** (M1): el servidor
 * las acepta tal cual, así que la SPA no debe reescribirlas al guardar sin que
 * el usuario las toque. Estos tests fijan que `parseRrule` → `composeRrule`
 * produce una forma canónica estable, que es la que `Calendar.tsx` compara
 * contra el baseline para decidir si hay cambio real.
 */
function canonicalOf(text: string): string {
  const parsed = parseRrule(text)
  if (parsed === null) {
    throw new Error(`esperaba una regla parseable: ${text}`)
  }
  return composeRrule(parsed)
}

describe('canonicalización de formas no canónicas (M1)', () => {
  it('reordena BYDAY desordenado al orden canónico', () => {
    expect(canonicalOf('FREQ=WEEKLY;BYDAY=WE,MO')).toBe('FREQ=WEEKLY;BYDAY=MO,WE')
  })

  it('normaliza UNTIL corto (YYYYMMDD) a fin de día UTC', () => {
    expect(canonicalOf('FREQ=YEARLY;UNTIL=20261231')).toBe('FREQ=YEARLY;UNTIL=20261231T235959Z')
  })

  it('omite INTERVAL=1 explícito', () => {
    expect(canonicalOf('FREQ=DAILY;INTERVAL=1')).toBe('FREQ=DAILY')
  })

  it('es idempotente: recomponer la forma canónica no vuelve a cambiar', () => {
    const once = canonicalOf('FREQ=WEEKLY;BYDAY=WE,MO')
    expect(canonicalOf(once)).toBe(once)
  })
})
