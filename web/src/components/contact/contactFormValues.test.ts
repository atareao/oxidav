/**
 * Tests unitarios de `contactFormValues` (FIX-9 de la auditoría de F22).
 *
 * Es el módulo donde vive la regla más delicada de la feature: el `PATCH` es
 * **por presencia** y solo deben viajar los grupos que difieren del detalle. Se
 * cubren la comparación estructural (`deepEqual`), la normalización (un
 * formulario intacto produce `{}`) y la semántica de borrado de `name`/`gender`
 * vacíos.
 */

import { describe, expect, it } from 'vitest'
import type { ContactDetail } from '../../types/api'
import {
  buildUpdateInput,
  deepEqual,
  groupsFromDetail,
  groupsFromForm,
  initialValuesFromDetail,
} from './contactFormValues'

/** Detalle mínimo por defecto; cada test lo completa con `overrides`. */
function makeDetail(overrides: Partial<ContactDetail> = {}): ContactDetail {
  return {
    id: 'c1',
    uid: 'uid-1',
    collection_id: 'ab-1',
    fn: 'Ada Lovelace',
    full_name: 'Ada Lovelace',
    etag: 'etag-1',
    created_at: 0,
    updated_at: 0,
    version: '4.0',
    rev: null,
    name: null,
    nicknames: [],
    photos: [],
    birthday: null,
    anniversary: null,
    gender: null,
    tels: [],
    emails: [],
    impps: [],
    addresses: [],
    geos: [],
    timezones: [],
    titles: [],
    roles: [],
    organizations: [],
    logos: [],
    notes: [],
    categories: [],
    urls: [],
    sounds: [],
    others: [],
    ...overrides,
  }
}

/** Diff entre un detalle y los valores iniciales que de él se derivan. */
function diffFromDetail(detail: ContactDetail, mutate?: (values: ReturnType<typeof initialValuesFromDetail>) => void) {
  const values = initialValuesFromDetail(detail)
  mutate?.(values)
  return buildUpdateInput(groupsFromDetail(detail), groupsFromForm(values))
}

describe('deepEqual', () => {
  it('distingue `null` de un array vacío', () => {
    expect(deepEqual(null, [])).toBe(false)
    expect(deepEqual([], null)).toBe(false)
  })

  it('detecta cambio cuando el mismo contenido está en distinto orden', () => {
    expect(deepEqual(['a', 'b'], ['b', 'a'])).toBe(false)
    expect(deepEqual(['a', 'b'], ['a', 'b'])).toBe(true)
  })

  it('no detecta cambio entre objetos con las mismas claves en distinto orden', () => {
    expect(deepEqual({ a: 1, b: 2 }, { b: 2, a: 1 })).toBe(true)
    expect(deepEqual({ a: 1, b: 2 }, { a: 1, b: 3 })).toBe(false)
  })
})

describe('normalización contra el detalle', () => {
  it('un detalle con espacios sobrantes no genera diff (formulario intacto → {})', () => {
    const detail = makeDetail({
      fn: '  Ada Lovelace  ',
      name: {
        family: '  Lovelace ',
        given: 'Ada  ',
        additional: ['  Augusta '],
        prefix: [],
        suffix: [],
      },
      tels: [{ value: '  +34 600  ', types: [' work '], pref: 1 }],
      notes: ['  una nota  '],
      organizations: [[' Analytical ', ' Engines ']],
    })

    expect(diffFromDetail(detail)).toEqual({})
  })
})

describe('organizations / categories (arrays de arrays)', () => {
  const detail = makeDetail({
    organizations: [['Analytical Engines', 'Investigación']],
    categories: [['amigos', 'vip']],
  })

  it('no envía nada si el contenido interno no cambia', () => {
    expect(diffFromDetail(detail)).toEqual({})
  })

  it('envía el grupo cuando cambia un componente interno', () => {
    expect(
      diffFromDetail(detail, (values) => {
        values.organizations = [['Analytical Engines', 'Ventas']]
      }),
    ).toEqual({ organizations: [['Analytical Engines', 'Ventas']] })
  })
})

describe('buildUpdateInput', () => {
  it('envía todos y solo los grupos que difieren', () => {
    const detail = makeDetail({
      fn: 'Ada Lovelace',
      tels: [{ value: '+111', types: [], pref: null }],
      notes: ['Nota original'],
    })

    const input = diffFromDetail(detail, (values) => {
      values.fn = 'Ada Byron'
      values.tels = [{ value: '+222', types: [], pref: null }]
      values.notes = ['Nota nueva']
    })

    expect(input).toEqual({
      fn: 'Ada Byron',
      tels: [{ value: '+222', types: [], pref: null }],
      notes: ['Nota nueva'],
    })
    expect(Object.keys(input).sort()).toEqual(['fn', 'notes', 'tels'])
  })

  it('vaciar un grupo lo envía como `[]`', () => {
    const detail = makeDetail({ tels: [{ value: '+111', types: [], pref: null }] })

    expect(
      diffFromDetail(detail, (values) => {
        values.tels = []
      }),
    ).toEqual({ tels: [] })
  })

  it('tras vaciar `gender` envía el objeto con ambos campos nulos', () => {
    const detail = makeDetail({ gender: { sex: 'F', identity: 'mujer' } })

    expect(
      diffFromDetail(detail, (values) => {
        values.gender = { sex: undefined, identity: undefined }
      }),
    ).toEqual({ gender: { sex: null, identity: null } })
  })

  it('tras vaciar `name` envía el objeto con los cinco componentes vacíos', () => {
    const detail = makeDetail({
      name: { family: 'Lovelace', given: 'Ada', additional: [], prefix: [], suffix: [] },
    })

    expect(
      diffFromDetail(detail, (values) => {
        values.name = { family: '', given: '', additional: [], prefix: [], suffix: [] }
      }),
    ).toEqual({
      name: { family: null, given: null, additional: [], prefix: [], suffix: [] },
    })
  })

  it('borra `birthday`/`anniversary` enviando cadena vacía', () => {
    const detail = makeDetail({ birthday: '1815-12-10', anniversary: '1835-07-08' })

    expect(
      diffFromDetail(detail, (values) => {
        values.birthday = ''
        values.anniversary = ''
      }),
    ).toEqual({ birthday: '', anniversary: '' })
  })
})
