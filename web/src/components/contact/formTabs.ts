/**
 * Pestañas del formulario de contacto (F23).
 *
 * Única fuente de verdad de dos cosas que el `<ContactForm>` y sus tests
 * comparten con la vista de detalle:
 *
 * 1. La **lista ordenada** de pestañas editables, con las mismas claves y
 *    etiquetas que `ContactDetailTabs` (`names`, `contact`, `addresses`, `work`,
 *    `additional`).
 * 2. El **mapa campo → pestaña** que usan el salto al primer error
 *    (`onFinishFailed`) y el punto rojo de cada pestaña.
 *
 * El mapa se resuelve por el **primer segmento** del `name` path de Ant Design,
 * de modo que cubre también los campos anidados (`['name', 'given']`) y los de
 * dentro de un `Form.List` (`['tels', 0, 'value']`) sin enumerarlos uno a uno.
 *
 * Reglas del salto (ver el plan):
 * - Un campo fuera de las pestañas (hoy solo `fn`, siempre visible) mapea a
 *   `null`: no hay que cambiar de pestaña.
 * - Un campo **sin mapeo** no puede hacer que el salto falle en silencio: cae a
 *   la primera pestaña.
 */

import type { ContactFormValues } from './contactFormValues'

/** Clave de una pestaña editable del formulario (espejo del detalle). */
export type ContactFormTabKey = 'names' | 'contact' | 'addresses' | 'work' | 'additional'

/** Una pestaña del formulario: su clave estable y su etiqueta visible. */
export interface ContactFormTab {
  key: ContactFormTabKey
  label: string
}

/** Las cinco pestañas editables, en su orden. */
export const CONTACT_FORM_TABS: readonly ContactFormTab[] = [
  { key: 'names', label: 'Nombres' },
  { key: 'contact', label: 'Contacto' },
  { key: 'addresses', label: 'Direcciones' },
  { key: 'work', label: 'Laboral' },
  { key: 'additional', label: 'Adicional' },
]

/** Pestaña inicial del diálogo. */
export const DEFAULT_CONTACT_FORM_TAB: ContactFormTabKey = 'names'

/**
 * Mapa campo (clave de primer nivel de {@link ContactFormValues}) → pestaña.
 *
 * Un valor `null` significa que el campo vive **fuera** de las pestañas (siempre
 * visible): hoy solo `fn`, el único obligatorio. El tipo `Record<keyof
 * ContactFormValues, …>` obliga al compilador a que **todas** las claves estén
 * cubiertas.
 */
export const FIELD_TABS: Record<keyof ContactFormValues, ContactFormTabKey | null> = {
  fn: null,
  name: 'names',
  nicknames: 'names',
  birthday: 'names',
  anniversary: 'names',
  gender: 'names',
  tels: 'contact',
  emails: 'contact',
  impps: 'contact',
  addresses: 'addresses',
  geos: 'addresses',
  timezones: 'addresses',
  titles: 'work',
  roles: 'work',
  organizations: 'work',
  notes: 'additional',
  categories: 'additional',
  urls: 'additional',
}

/** Nombre de campo tal y como lo entrega Ant Design: un array de segmentos. */
export type FieldNamePath = readonly (string | number)[]

/**
 * ¿La clave es una de las de `ContactFormValues`?
 *
 * Se comprueba **propiedad propia** (`Object.hasOwn`), no `in`: este último
 * recorre la cadena de prototipos y daría por conocidas claves heredadas de
 * `Object.prototype` (`'toString'`, `'constructor'`, `'hasOwnProperty'`…). Con
 * `in`, `tabForField(['toString'])` devolvería `Object.prototype.toString` —una
 * función— en vez del fallback, y aguas arriba esa función se contaría como una
 * «pestaña distinta» dejando todos los paneles inactivos.
 */
function isKnownField(key: string): key is keyof ContactFormValues {
  return Object.hasOwn(FIELD_TABS, key)
}

/**
 * Pestaña de un campo a partir de su `name` path de Ant Design.
 *
 * - Los campos anidados y los de un `Form.List` se resuelven por su primer
 *   segmento (`['name', 'given']` → `names`; `['tels', 0, 'value']` → `contact`).
 * - Un campo fuera de las pestañas (`fn`) devuelve `null` (no hay que saltar).
 * - Un campo sin mapeo cae a {@link DEFAULT_CONTACT_FORM_TAB}: el salto nunca
 *   falla en silencio.
 */
export function tabForField(name: FieldNamePath): ContactFormTabKey | null {
  const [first] = name
  if (typeof first !== 'string') {
    return null
  }
  if (!isKnownField(first)) {
    return DEFAULT_CONTACT_FORM_TAB
  }
  return FIELD_TABS[first]
}

/** Campo con error, en la forma mínima que necesita {@link tabErrors}. */
export interface FieldErrorLike {
  name: FieldNamePath
  errors: readonly string[]
}

/** Conjunto de pestañas que tienen al menos un campo con error. */
export function tabErrors(fields: readonly FieldErrorLike[]): Set<ContactFormTabKey> {
  const withErrors = new Set<ContactFormTabKey>()
  for (const field of fields) {
    if (field.errors.length === 0) {
      continue
    }
    const tab = tabForField(field.name)
    if (tab !== null) {
      withErrors.add(tab)
    }
  }
  return withErrors
}
