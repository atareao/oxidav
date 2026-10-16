/**
 * Conversión entre los valores del `<Form>` de alta/edición de contactos (F22),
 * el detalle cargado (`ContactDetail`) y los cuerpos de `POST`/`PATCH`.
 *
 * Son funciones **puras** (sin React) a propósito, para poder razonar sobre la
 * regla más delicada de la fase: el `PATCH` es **por presencia**.
 *
 * # La regla del diff (no simplificar)
 *
 * El merge del servidor sustituye **todas** las apariciones de las propiedades
 * presentes y deja intacto el resto. Pero además **re-renderiza** las líneas de
 * los grupos presentes con su propio formato (parámetros reordenados, prefijos
 * de grupo descartados, escapado normalizado). Por eso un grupo que el usuario
 * **no** ha tocado no debe viajar en el `PATCH`: enviarlo reescribiría esa parte
 * del payload que el cliente había escrito a su manera — es decir, degradaría
 * los datos que esta feature existe para preservar.
 *
 * De ahí que {@link buildUpdateInput} compare cada grupo contra el detalle
 * inicial y envíe **solo** lo que difiere. La comparación es estructural (los
 * arrays de objetos se comparan por valor, orden incluido).
 *
 * # Formas de array
 *
 * El formulario usa siempre las formas de **array** (`emails`, `tels`,
 * `organizations`…), nunca los escalares heredados (`email`, `phone`,
 * `organization`): el servidor responde `400` si llegan los dos, y el detalle
 * solo trae los arrays.
 */

import type {
  Address,
  ContactDetail,
  CreateContactInput,
  Gender,
  LabelledValue,
  StructuredName,
  UpdateContactInput,
} from '../../types/api'

/** Una fila de `TEL`/`EMAIL`/`IMPP` tal y como la guarda el formulario. */
export interface LabelledValueForm {
  value?: string
  types?: string[]
  pref?: number | null
}

/** Una fila de la lista `ADR` tal y como la guarda el formulario. */
export interface AddressForm {
  po_box?: string
  extended?: string
  street?: string
  locality?: string
  region?: string
  postal_code?: string
  country?: string
  label?: string
  types?: string[]
  pref?: number | null
}

/** Sub-formulario del `N` (los tres últimos componentes son listas). */
export interface NameForm {
  family?: string
  given?: string
  additional?: string[]
  prefix?: string[]
  suffix?: string[]
}

/** Sub-formulario del `GENDER`. */
export interface GenderForm {
  sex?: string
  identity?: string
}

/** Todos los valores que el `<Form>` de contacto mantiene. */
export interface ContactFormValues {
  fn: string
  name?: NameForm
  nicknames?: string[]
  birthday?: string
  anniversary?: string
  gender?: GenderForm
  tels?: LabelledValueForm[]
  emails?: LabelledValueForm[]
  impps?: LabelledValueForm[]
  addresses?: AddressForm[]
  geos?: string[]
  timezones?: string[]
  titles?: string[]
  roles?: string[]
  organizations?: string[][]
  categories?: string[][]
  notes?: string[]
  urls?: string[]
}

/**
 * Vista canónica y comparable de los grupos editables.
 *
 * Es la forma normalizada que comparten {@link groupsFromDetail} y
 * {@link groupsFromForm}, de modo que un formulario intacto y el detalle del que
 * nació producen objetos **iguales** y el diff sale vacío.
 */
export interface ContactGroups {
  fn: string | null
  name: StructuredName | null
  nicknames: string[]
  birthday: string | null
  anniversary: string | null
  gender: Gender | null
  tels: LabelledValue[]
  emails: LabelledValue[]
  impps: LabelledValue[]
  addresses: Address[]
  geos: string[]
  timezones: string[]
  titles: string[]
  roles: string[]
  organizations: string[][]
  notes: string[]
  categories: string[][]
  urls: string[]
}

/** Texto opcional normalizado: se recorta y un valor en blanco se vuelve `null`. */
function text(value: string | null | undefined): string | null {
  const trimmed = value?.trim() ?? ''
  return trimmed.length > 0 ? trimmed : null
}

/** Lista de textos normalizada: se recorta cada entrada y se descartan las vacías. */
function list(values: readonly string[] | undefined): string[] {
  return (values ?? []).map((value) => value.trim()).filter((value) => value.length > 0)
}

/** Lista de listas normalizada, descartando las filas que quedan vacías. */
function groupRows(rows: readonly string[][] | undefined): string[][] {
  return (rows ?? []).map(list).filter((row) => row.length > 0)
}

/** Fila etiquetada normalizada (compartida por `TEL`/`EMAIL`/`IMPP`). */
function labelled(value: string, types: readonly string[] | undefined, pref: number | null): LabelledValue {
  return { value: value.trim(), types: list(types), pref }
}

/** Todas las `LabelledValue` de un array del formulario, descartando las vacías. */
function labelledFromForm(rows: readonly LabelledValueForm[] | undefined): LabelledValue[] {
  return (rows ?? [])
    .map((row) => labelled(row.value ?? '', row.types, row.pref ?? null))
    .filter((row) => row.value.length > 0)
}

/** Todas las `LabelledValue` de un array del detalle, normalizadas. */
function labelledFromDetail(rows: readonly LabelledValue[]): LabelledValue[] {
  return rows
    .map((row) => labelled(row.value, row.types, row.pref))
    .filter((row) => row.value.length > 0)
}

/** ¿La dirección tiene algún componente (o etiqueta) con contenido? */
function hasAddressContent(row: Address): boolean {
  return (
    row.po_box !== null ||
    row.extended !== null ||
    row.street !== null ||
    row.locality !== null ||
    row.region !== null ||
    row.postal_code !== null ||
    row.country !== null ||
    row.label !== null
  )
}

/** Dirección normalizada a partir de una fila del formulario. */
function addressFromForm(row: AddressForm): Address {
  return {
    po_box: text(row.po_box),
    extended: text(row.extended),
    street: text(row.street),
    locality: text(row.locality),
    region: text(row.region),
    postal_code: text(row.postal_code),
    country: text(row.country),
    label: text(row.label),
    types: list(row.types),
    pref: row.pref ?? null,
  }
}

/** Dirección del detalle, normalizada (misma forma que la del formulario). */
function addressFromDetail(row: Address): Address {
  return {
    po_box: text(row.po_box),
    extended: text(row.extended),
    street: text(row.street),
    locality: text(row.locality),
    region: text(row.region),
    postal_code: text(row.postal_code),
    country: text(row.country),
    label: text(row.label),
    types: list(row.types),
    pref: row.pref,
  }
}

/**
 * `N` normalizado a partir de sus cinco componentes. Devuelve `null` cuando
 * todos están vacíos (un `N` sin contenido no es un dato).
 */
function nameFromParts(
  family: string | null | undefined,
  given: string | null | undefined,
  additional: readonly string[] | undefined,
  prefix: readonly string[] | undefined,
  suffix: readonly string[] | undefined,
): StructuredName | null {
  const result: StructuredName = {
    family: text(family),
    given: text(given),
    additional: list(additional),
    prefix: list(prefix),
    suffix: list(suffix),
  }
  const empty =
    result.family === null &&
    result.given === null &&
    result.additional.length === 0 &&
    result.prefix.length === 0 &&
    result.suffix.length === 0
  return empty ? null : result
}

/** `GENDER` normalizado; `null` cuando ni el sexo ni la identidad tienen texto. */
function genderFromParts(sex: string | null | undefined, identity: string | null | undefined): Gender | null {
  const normalizedSex = text(sex)
  const normalizedIdentity = text(identity)
  if (normalizedSex === null && normalizedIdentity === null) {
    return null
  }
  return { sex: normalizedSex, identity: normalizedIdentity }
}

/** Vista canónica de los grupos a partir del detalle cargado (valor inicial). */
export function groupsFromDetail(detail: ContactDetail): ContactGroups {
  return {
    fn: text(detail.fn ?? detail.full_name),
    name:
      detail.name === null
        ? null
        : nameFromParts(
            detail.name.family,
            detail.name.given,
            detail.name.additional,
            detail.name.prefix,
            detail.name.suffix,
          ),
    nicknames: list(detail.nicknames),
    birthday: text(detail.birthday),
    anniversary: text(detail.anniversary),
    gender:
      detail.gender === null ? null : genderFromParts(detail.gender.sex, detail.gender.identity),
    tels: labelledFromDetail(detail.tels),
    emails: labelledFromDetail(detail.emails),
    impps: labelledFromDetail(detail.impps),
    addresses: detail.addresses.map(addressFromDetail).filter(hasAddressContent),
    geos: list(detail.geos),
    timezones: list(detail.timezones),
    titles: list(detail.titles),
    roles: list(detail.roles),
    organizations: groupRows(detail.organizations),
    notes: list(detail.notes),
    categories: groupRows(detail.categories),
    urls: list(detail.urls),
  }
}

/** Vista canónica de los grupos a partir de los valores del formulario. */
export function groupsFromForm(values: ContactFormValues): ContactGroups {
  return {
    fn: text(values.fn),
    name: nameFromParts(
      values.name?.family,
      values.name?.given,
      values.name?.additional,
      values.name?.prefix,
      values.name?.suffix,
    ),
    nicknames: list(values.nicknames),
    birthday: text(values.birthday),
    anniversary: text(values.anniversary),
    gender: genderFromParts(values.gender?.sex, values.gender?.identity),
    tels: labelledFromForm(values.tels),
    emails: labelledFromForm(values.emails),
    impps: labelledFromForm(values.impps),
    addresses: (values.addresses ?? []).map(addressFromForm).filter(hasAddressContent),
    geos: list(values.geos),
    timezones: list(values.timezones),
    titles: list(values.titles),
    roles: list(values.roles),
    organizations: groupRows(values.organizations),
    notes: list(values.notes),
    categories: groupRows(values.categories),
    urls: list(values.urls),
  }
}

/**
 * Valores iniciales del formulario para un detalle (modo edición).
 *
 * Se toman **del detalle completo**, no del contacto ligero de la tabla: el
 * ligero solo trae cuatro campos y no tiene los grupos.
 */
export function initialValuesFromDetail(detail: ContactDetail): ContactFormValues {
  return {
    fn: detail.fn ?? detail.full_name ?? '',
    name:
      detail.name === null
        ? undefined
        : {
            family: detail.name.family ?? undefined,
            given: detail.name.given ?? undefined,
            additional: [...detail.name.additional],
            prefix: [...detail.name.prefix],
            suffix: [...detail.name.suffix],
          },
    nicknames: [...detail.nicknames],
    birthday: detail.birthday ?? undefined,
    anniversary: detail.anniversary ?? undefined,
    gender:
      detail.gender === null
        ? undefined
        : {
            sex: detail.gender.sex ?? undefined,
            identity: detail.gender.identity ?? undefined,
          },
    tels: detail.tels.map((row) => ({ value: row.value, types: [...row.types], pref: row.pref })),
    emails: detail.emails.map((row) => ({ value: row.value, types: [...row.types], pref: row.pref })),
    impps: detail.impps.map((row) => ({ value: row.value, types: [...row.types], pref: row.pref })),
    addresses: detail.addresses.map((row) => ({
      po_box: row.po_box ?? undefined,
      extended: row.extended ?? undefined,
      street: row.street ?? undefined,
      locality: row.locality ?? undefined,
      region: row.region ?? undefined,
      postal_code: row.postal_code ?? undefined,
      country: row.country ?? undefined,
      label: row.label ?? undefined,
      types: [...row.types],
      pref: row.pref,
    })),
    geos: [...detail.geos],
    timezones: [...detail.timezones],
    titles: [...detail.titles],
    roles: [...detail.roles],
    organizations: detail.organizations.map((row) => [...row]),
    notes: [...detail.notes],
    categories: detail.categories.map((row) => [...row]),
    urls: [...detail.urls],
  }
}

/** Valores iniciales del formulario para el alta (todo vacío). */
export function emptyFormValues(): ContactFormValues {
  return {
    fn: '',
    name: { family: '', given: '', additional: [], prefix: [], suffix: [] },
    nicknames: [],
    gender: { sex: undefined, identity: undefined },
    tels: [],
    emails: [],
    impps: [],
    addresses: [],
    geos: [],
    timezones: [],
    titles: [],
    roles: [],
    organizations: [],
    categories: [],
    notes: [],
    urls: [],
  }
}

/**
 * Cuerpo de `POST` (alta): se envían solo los grupos con contenido; un grupo
 * vacío no se incluye.
 */
export function buildCreateInput(values: ContactFormValues): CreateContactInput {
  const groups = groupsFromForm(values)
  const input: CreateContactInput = { fn: groups.fn ?? '' }
  if (groups.name !== null) {
    input.name = groups.name
  }
  if (groups.nicknames.length > 0) {
    input.nicknames = groups.nicknames
  }
  if (groups.birthday !== null) {
    input.birthday = groups.birthday
  }
  if (groups.anniversary !== null) {
    input.anniversary = groups.anniversary
  }
  if (groups.gender !== null) {
    input.gender = groups.gender
  }
  if (groups.tels.length > 0) {
    input.tels = groups.tels
  }
  if (groups.emails.length > 0) {
    input.emails = groups.emails
  }
  if (groups.impps.length > 0) {
    input.impps = groups.impps
  }
  if (groups.addresses.length > 0) {
    input.addresses = groups.addresses
  }
  if (groups.geos.length > 0) {
    input.geos = groups.geos
  }
  if (groups.timezones.length > 0) {
    input.timezones = groups.timezones
  }
  if (groups.titles.length > 0) {
    input.titles = groups.titles
  }
  if (groups.roles.length > 0) {
    input.roles = groups.roles
  }
  if (groups.organizations.length > 0) {
    input.organizations = groups.organizations
  }
  if (groups.notes.length > 0) {
    input.notes = groups.notes
  }
  if (groups.categories.length > 0) {
    input.categories = groups.categories
  }
  if (groups.urls.length > 0) {
    input.urls = groups.urls
  }
  return input
}

/**
 * Comparación estructural: primitivos por valor, arrays por longitud y posición
 * (orden incluido) y objetos por nombre de clave (sin depender del orden de
 * inserción, que sí varía entre el servidor y el formulario).
 *
 * Exportada solo para poder cubrirla en tests unitarios; el resto del módulo la
 * usa internamente.
 */
export function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) {
    return true
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b)) {
      return false
    }
    return a.length === b.length && a.every((item, index) => deepEqual(item, b[index]))
  }
  if (typeof a === 'object' && a !== null && typeof b === 'object' && b !== null) {
    const aRecord = a as Record<string, unknown>
    const bRecord = b as Record<string, unknown>
    const aKeys = Object.keys(aRecord).sort()
    const bKeys = Object.keys(bRecord).sort()
    return (
      aKeys.length === bKeys.length &&
      aKeys.every((key, index) => key === bKeys[index] && deepEqual(aRecord[key], bRecord[key]))
    )
  }
  return false
}

/**
 * Cuerpo de `PATCH` (edición): **solo** los grupos que difieren del detalle
 * inicial. Ver la nota de cabecera sobre por qué esto es obligatorio.
 *
 * Semántica por campo:
 * - Grupos de array: un grupo vaciado se envía como `[]` (que es como se borra).
 * - `birthday`/`anniversary`: se borran enviando `''` (el servidor emite `BDAY:`,
 *   que re-parsea como vacío).
 * - `name`: un `N` vaciado viaja como objeto **presente** con los cinco
 *   componentes vacíos; el servidor lo interpreta como borrado del `N`.
 * - `gender`: un `GENDER` vaciado viaja como `{sex:null,identity:null}`; el
 *   servidor lo interpreta como borrado del `GENDER`.
 *
 * Devuelve un objeto vacío cuando no hay ningún cambio.
 */
export function buildUpdateInput(initial: ContactGroups, next: ContactGroups): UpdateContactInput {
  const input: UpdateContactInput = {}
  if (!deepEqual(initial.fn, next.fn) && next.fn !== null) {
    input.fn = next.fn
  }
  if (!deepEqual(initial.name, next.name)) {
    // Presente con los cinco componentes vacíos = borrar el `N` (contrato F22).
    input.name = next.name ?? { family: null, given: null, additional: [], prefix: [], suffix: [] }
  }
  if (!deepEqual(initial.nicknames, next.nicknames)) {
    input.nicknames = next.nicknames
  }
  if (!deepEqual(initial.birthday, next.birthday)) {
    input.birthday = next.birthday ?? ''
  }
  if (!deepEqual(initial.anniversary, next.anniversary)) {
    input.anniversary = next.anniversary ?? ''
  }
  if (!deepEqual(initial.gender, next.gender)) {
    // Presente con ambos nulos = borrar el `GENDER` (contrato F22).
    input.gender = next.gender ?? { sex: null, identity: null }
  }
  if (!deepEqual(initial.tels, next.tels)) {
    input.tels = next.tels
  }
  if (!deepEqual(initial.emails, next.emails)) {
    input.emails = next.emails
  }
  if (!deepEqual(initial.impps, next.impps)) {
    input.impps = next.impps
  }
  if (!deepEqual(initial.addresses, next.addresses)) {
    input.addresses = next.addresses
  }
  if (!deepEqual(initial.geos, next.geos)) {
    input.geos = next.geos
  }
  if (!deepEqual(initial.timezones, next.timezones)) {
    input.timezones = next.timezones
  }
  if (!deepEqual(initial.titles, next.titles)) {
    input.titles = next.titles
  }
  if (!deepEqual(initial.roles, next.roles)) {
    input.roles = next.roles
  }
  if (!deepEqual(initial.organizations, next.organizations)) {
    input.organizations = next.organizations
  }
  if (!deepEqual(initial.notes, next.notes)) {
    input.notes = next.notes
  }
  if (!deepEqual(initial.categories, next.categories)) {
    input.categories = next.categories
  }
  if (!deepEqual(initial.urls, next.urls)) {
    input.urls = next.urls
  }
  return input
}

/** La entrada preferida de un multivalor: menor `PREF`, o la primera. */
function preferredValue(rows: readonly LabelledValue[]): string | null {
  const present = rows.filter((row) => row.value.length > 0)
  if (present.length === 0) {
    return null
  }
  const best = present.reduce((candidate, row) =>
    (row.pref ?? Number.MAX_SAFE_INTEGER) < (candidate.pref ?? Number.MAX_SAFE_INTEGER)
      ? row
      : candidate,
  )
  return best.value
}

/** Campos del DTO ligero derivados de los grupos (para la capa optimista). */
export interface LightContactFields {
  fn: string
  email: string | null
  phone: string | null
  organization: string | null
}

/**
 * Deriva los cuatro campos del DTO **ligero** que usa la tabla a partir de los
 * grupos, replicando la heurística de `parse_vcf` en el servidor: correo y
 * teléfono preferidos por menor `PREF` (o el primero), y `organization` de la
 * primera `ORG` uniendo sus componentes con `"; "`.
 */
export function lightFieldsFromGroups(groups: ContactGroups): LightContactFields {
  return {
    fn: groups.fn ?? '',
    email: preferredValue(groups.emails),
    phone: preferredValue(groups.tels),
    organization: groups.organizations[0]?.join('; ') ?? null,
  }
}
