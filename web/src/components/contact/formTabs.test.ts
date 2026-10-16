/**
 * Tests de `formTabs` (F23).
 *
 * El módulo es la **única** fuente de verdad de las pestañas del formulario de
 * contacto y del mapa campo → pestaña que usan el salto al primer error y el
 * punto rojo. Se comprueba:
 * - la lista de pestañas tiene las cinco claves esperadas en su orden;
 * - el mapa cubre **todas** las claves de `ContactFormValues` (recorrido de un
 *   objeto de valores **completo**, no de la lista que devuelve
 *   `emptyFormValues`, que omite `birthday`/`anniversary`, ni una lista escrita a
 *   mano) y sus claves anidadas;
 * - un campo sin mapeo no hace fallar el salto en silencio (cae a la primera
 *   pestaña) y `fn` (fuera de las pestañas) no apunta a ninguna;
 * - las claves heredadas de `Object.prototype` (`toString`, `constructor`…) **no**
 *   se confunden con campos conocidos.
 */

import { describe, expect, it } from 'vitest'
import type { ContactFormValues } from './contactFormValues'
import {
  CONTACT_FORM_TABS,
  DEFAULT_CONTACT_FORM_TAB,
  FIELD_TABS,
  tabErrors,
  tabForField,
  type FieldNamePath,
} from './formTabs'

/**
 * Valores que cubren **todas** las claves de `ContactFormValues`.
 *
 * El tipo `Required<ContactFormValues>` obliga al compilador a exigir aquí cada
 * clave de la interfaz: si alguien añade un campo al formulario y no lo añade a
 * este objeto, el `typecheck` falla. El test de cobertura recorre sus claves para
 * exigir un mapeo en `FIELD_TABS`, de modo que un campo nuevo sin pestaña pone el
 * test en rojo. Incluye a propósito `birthday` y `anniversary`, que
 * `emptyFormValues` (valores de alta) no lleva por ser opcionales.
 */
const ALL_FORM_VALUES: Required<ContactFormValues> = {
  fn: 'Ada Lovelace',
  name: { family: 'Lovelace', given: 'Ada', additional: [], prefix: [], suffix: [] },
  nicknames: ['Ada'],
  birthday: '1815-12-10',
  anniversary: '1835-07-08',
  gender: { sex: 'F', identity: 'mujer' },
  tels: [{ value: '+34 600 000 000', types: ['cell'], pref: 1 }],
  emails: [{ value: 'ada@example.com', types: ['work'], pref: null }],
  impps: [{ value: 'ada@im.example.com', types: [], pref: null }],
  addresses: [{ street: 'Calle 1', locality: 'Madrid', types: ['home'], pref: null }],
  geos: ['geo:40.4,-3.7'],
  timezones: ['Europe/Madrid'],
  titles: ['Matemática'],
  roles: ['Ingeniera'],
  organizations: [['Acme']],
  notes: ['Nota'],
  categories: [['Amigos']],
  urls: ['https://example.com'],
}

/** Recorre un valor cualquiera y devuelve los caminos hasta sus hojas (no arrays). */
function leafPaths(value: unknown, prefix: FieldNamePath = []): FieldNamePath[] {
  if (Array.isArray(value)) {
    return value.flatMap((item, index) => leafPaths(item, [...prefix, index]))
  }
  if (typeof value === 'object' && value !== null) {
    return Object.entries(value).flatMap(([key, child]) => leafPaths(child, [...prefix, key]))
  }
  return [prefix]
}

describe('CONTACT_FORM_TABS', () => {
  it('declara las cinco pestañas editables en su orden', () => {
    expect(CONTACT_FORM_TABS.map((tab) => tab.key)).toEqual([
      'names',
      'contact',
      'addresses',
      'work',
      'additional',
    ])
    expect(CONTACT_FORM_TABS.map((tab) => tab.label)).toEqual([
      'Nombres',
      'Contacto',
      'Direcciones',
      'Laboral',
      'Adicional',
    ])
  })

  it('abre por la primera pestaña («Nombres»)', () => {
    expect(DEFAULT_CONTACT_FORM_TAB).toBe('names')
  })
})

describe('FIELD_TABS', () => {
  it('cubre todas las claves de `ContactFormValues`', () => {
    const keys = Object.keys(ALL_FORM_VALUES)
    // `emptyFormValues` omite estas dos por ser opcionales; este objeto no, así
    // que el recorrido las visita de verdad.
    expect(keys).toContain('birthday')
    expect(keys).toContain('anniversary')
    for (const key of keys) {
      expect(Object.hasOwn(FIELD_TABS, key), `falta el mapeo de «${key}»`).toBe(true)
    }
  })

  it('resuelve una pestaña para toda clave anidada (salvo `fn`, que va fuera)', () => {
    for (const path of leafPaths(ALL_FORM_VALUES)) {
      const tab = tabForField(path)
      if (path[0] === 'fn') {
        expect(tab, 'fn va fuera de las pestañas').toBeNull()
      } else {
        expect(tab, `sin pestaña para ${path.join('.')}`).not.toBeNull()
      }
    }
  })

  it('mapea `birthday` y `anniversary` a la pestaña «Nombres»', () => {
    expect(tabForField(['birthday'])).toBe('names')
    expect(tabForField(['anniversary'])).toBe('names')
  })
})

describe('tabForField (claves heredadas)', () => {
  it.each(['toString', 'constructor', 'hasOwnProperty', 'valueOf', '__proto__'])(
    'no confunde la clave heredada «%s» con un campo conocido',
    (key) => {
      const tab = tabForField([key])
      expect(typeof tab, `«${key}» no debe resolver a una función`).not.toBe('function')
      expect(tab).toBe(DEFAULT_CONTACT_FORM_TAB)
    },
  )
})

describe('tabForField', () => {
  it.each<[string, FieldNamePath, ReturnType<typeof tabForField>]>([
    ['fn siempre visible', ['fn'], null],
    ['name', ['name'], 'names'],
    ['name.given', ['name', 'given'], 'names'],
    ['gender.sex', ['gender', 'sex'], 'names'],
    ['nicknames', ['nicknames'], 'names'],
    ['tels', ['tels'], 'contact'],
    ['tels[0].value (Form.List)', ['tels', 0, 'value'], 'contact'],
    ['emails[0].types (Form.List)', ['emails', 0, 'types'], 'contact'],
    ['impps', ['impps'], 'contact'],
    ['addresses[0].street (Form.List)', ['addresses', 0, 'street'], 'addresses'],
    ['geos', ['geos'], 'addresses'],
    ['timezones', ['timezones'], 'addresses'],
    ['titles', ['titles'], 'work'],
    ['roles', ['roles'], 'work'],
    ['organizations[0][0] (Form.List)', ['organizations', 0, 0], 'work'],
    ['notes[0] (Form.List)', ['notes', 0], 'additional'],
    ['categories[0][0] (Form.List)', ['categories', 0, 0], 'additional'],
    ['urls', ['urls'], 'additional'],
  ])('mapea %s', (_label, path, expected) => {
    expect(tabForField(path)).toBe(expected)
  })

  it('cae a la primera pestaña cuando la clave no tiene mapeo (no falla en silencio)', () => {
    expect(tabForField(['campo-desconocido'])).toBe(DEFAULT_CONTACT_FORM_TAB)
  })
})

describe('tabErrors', () => {
  it('reúne las pestañas que tienen algún campo con error, ignorando las demás', () => {
    const tabs = tabErrors([
      { name: ['fn'], errors: ['Introduce un nombre'] },
      { name: ['tels', 0, 'types'], errors: ['Solo letras, números y guiones'] },
      { name: ['addresses', 0, 'types'], errors: [] },
      { name: ['emails', 0, 'value'], errors: [] },
      { name: ['notes', 0], errors: ['Error de nota'] },
    ])

    expect([...tabs].sort()).toEqual(['additional', 'contact'])
  })

  it('no marca `fn` (está fuera de las pestañas) aunque tenga error', () => {
    expect([...tabErrors([{ name: ['fn'], errors: ['Introduce un nombre'] }])]).toEqual([])
  })
})
