# PLAN-030 — Zona horaria (TZID) y recurrencia (RRULE) en el diálogo de eventos (F26)

## Estado

Borrador — **pendiente de aprobación** (compuerta SDD). No se escribe código ni tests hasta que el usuario apruebe este plan.

## Objetivo

Exponer en el diálogo de alta/edición de eventos de la SPA dos propiedades que el backend **ya valida y persiste** pero que la UI no ofrece al usuario: la **zona horaria** (`TZID`, F18) y la **recurrencia** (`RRULE`, F07). Hoy la zona se auto-rellena en silencio (zona del navegador al crear; ancla preservada al editar) y la regla de recurrencia ni se muestra ni se puede fijar (en edición se preserva por omisión del `PATCH`, pero es invisible). F26 las convierte en campos editables de primera clase, con un **constructor estructurado** de recurrencia (frecuencia, intervalo, fin, días) que compone el texto RFC 5545.

**Backend: sin cambios.** `validate_tzid` y `validate_rrule` ya existen en `src/api/events.rs`; `CreateEventRequest`/`UpdateEventRequest` y `EventDetailDto` ya llevan `tzid`/`rrule`. F26 es **solo frontend**. No hay migración ni cambio de API.

## Contexto verificado (no hay que re-descubrir)

- Backend (`src/api/events.rs`):
  - `validate_rrule`: límite `MAX_RRULE_LEN = 1024`; alfabeto permitido **solo** `A-Z`, `0-9`, `=`, `;`, `,`, `-`; un valor en blanco se normaliza a `None`. Deliberadamente rechaza caracteres de control (frontera anti-inyección de content-line).
  - `validate_tzid`: exige una zona IANA real (`vtimezone::is_valid_tzid`) y devuelve el nombre **canónico**; `""` → `400`.
  - `PATCH` (semántica de limpieza, `src/api/events.rs:409-449`):
    - `rrule`: `Some("")` → `validate_rrule` devuelve `None` → **la regla se elimina**. Ausente (`None`) → se conserva la existente.
    - `tzid`: ausente → conserva **ambas** anclas (no se puede limpiar); `Some("")` → `400`. Un `tzid` presente aplica la **misma** zona a `DTSTART` **y** `DTEND`.
  - `create`: `tzid`/`rrule` opcionales; omitidos = sin zona / sin regla.
- Tipos TS (`web/src/types/api.ts`): `CreateEventInput.rrule?: string`, `.tzid?: string`; `UpdateEventInput.rrule?: string`, `.tzid?: string`; `CalendarEvent.dtstart_tzid/dtend_tzid/rrule: string | null`; `EventDetail extends CalendarEvent`. **No hay que tocarlos.**
- UI actual (`web/src/pages/Calendar.tsx`, 1257 líneas):
  - `browserTimeZone()` (líneas ~246-249): devuelve `Intl.DateTimeFormat().resolvedOptions().timeZone` o `undefined`.
  - `formValuesFromDetail(detail)` (líneas ~205-238): construye los valores iniciales del formulario de edición desde el detalle. **Hoy NO incluye `tzid` ni recurrencia.**
  - `formInitialValues` (líneas ~790-806): rama de alta con `{summary:'', description:'', location:'', url:'', range: defaultRange(), durationMode:'end'}`. **Hoy NO incluye `tzid`.**
  - `handleSubmit` (líneas ~500-763):
    - Alta: `const tzid = browserTimeZone()` (auto, invisible), `input.tzid = tzid` si `!== undefined`, optimista `dtstart_tzid/dtend_tzid: tzid ?? null`, `rrule: null` (nunca se envía).
    - Edición: `const anchorTzid = target.dtstart_tzid`; `input.tzid = anchorTzid` si `!== null` (preserva); no envía `rrule`.
  - Formulario (`<Form>` dentro de `<Modal>`, líneas ~1094-1252): campos `summary`, `description`, `location`, `url`, `status`, `class`, `transp`, `categories`, `attach`, `durationMode`/`durationValue`/`durationUnit`, `range`. **No hay campos de zona ni de recurrencia.**
  - Actualizaciones optimistas: `EventChanges = Pick<CalendarEvent,'summary'|'description'|'location'|'url'|'dtstart'|'dtend'|'dtstart_tzid'|'dtend_tzid'> & Partial<Pick<EventDetail,'status'|'class'|'transp'|'categories'|'attach'|'duration'>>` (líneas ~276-281). **Falta `rrule`.**
- Entorno: `Intl.supportedValuesOf('timeZone')` disponible (Node 22 / jsdom 30 exponen el `Intl` de Node; 418 zonas, incluye `Europe/Madrid`). Vitest en entorno `jsdom` con `web/src/test/setup.ts` (stubs de `ResizeObserver` y `matchMedia`).
- Patrón D2 ya establecido en F24/F25 (réplica en `handleSubmit`): un `PATCH` **solo envía lo que cambió** respecto del valor con que se abrió el diálogo; un valor **legado no descomponible** (p. ej. `DURATION:P1DT2H`) se muestra verbatim y **no se reescribe** si el usuario no lo toca. F26 reutiliza exactamente esa disciplina para `RRULE`.

## Arquitectura

Dos módulos **puros** nuevos en `web/src/utils/` (sin React, para razonarlos y testearlos aislados), más cambios acotados en `Calendar.tsx`:

1. `web/src/utils/recurrence.ts` — construir/parsear/describir una `RRULE` (espejo del alfabeto del servidor).
2. `web/src/utils/timezone.ts` — lista de zonas IANA y zona del navegador (se **mueve** aquí `browserTimeZone` desde `Calendar.tsx`).
3. `Calendar.tsx` — campos de formulario, valores iniciales, lógica de envío (con disciplina D2) y capa optimista.

### Contratos (TypeScript)

`web/src/utils/recurrence.ts`

```ts
export type RecurFreq = 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY'
export type RecurEndKind = 'never' | 'count' | 'until'
export type Weekday = 'MO' | 'TU' | 'WE' | 'TH' | 'FR' | 'SA' | 'SU'

export interface RecurrenceRule {
  freq: RecurFreq
  /** Cada cuántos períodos; `>= 1` (1 = todos). */
  interval: number
  endKind: RecurEndKind
  /** Requerido cuando `endKind === 'count'`; `>= 1`. */
  count?: number
  /** Requerido cuando `endKind === 'until'`; formato `YYYYMMDD`. */
  until?: string
  /** Solo para `freq === 'WEEKLY'`; días de la semana. */
  byDay?: Weekday[]
}

export const MAX_RRULE_LEN = 1024
export const FREQ_OPTIONS: ReadonlyArray<{ value: RecurFreq; label: string }>
export const WEEKDAY_OPTIONS: ReadonlyArray<{ value: Weekday; label: string }>

/** Compone el texto RFC 5545 (alfabeto `A-Z0-9=;,-`). Determinista y ordenado. */
export function composeRrule(rule: RecurrenceRule): string
/** Parsea una regla estructuralmente conocida; `null` si es opaca (no reescribible). */
export function parseRrule(text: string): RecurrenceRule | null
/** Resumen legible en español de una regla (o el texto crudo si es opaca). */
export function describeRrule(text: string): string
/** Espejo EXACTO del alfabeto/límite del servidor (`validate_rrule`). */
export function isRruleAlphabet(value: string): boolean
```

`web/src/utils/timezone.ts`

```ts
/** Zonas IANA conocidas vía `Intl.supportedValuesOf('timeZone')`; `[]` si no está. */
export function availableTimeZones(): string[]
/** Zona del navegador o `undefined` (idéntica a la actual `browserTimeZone`). */
export function browserTimeZone(): string | undefined
```

### Composición de la `RRULE` (contrato de `composeRrule`)

Partes unidas por `;`, en este orden, **omitiendo** lo no aplicable:

1. `FREQ=<freq>`
2. `INTERVAL=<n>` — solo si `interval > 1`
3. `BYDAY=<MO,WE>` — solo si `freq === 'WEEKLY'` y `byDay` no vacío
4. Fin: `COUNT=<n>` (si `endKind === 'count'`) o `UNTIL=<YYYYMMDD>T235959Z` (si `endKind === 'until'`); nada si `endKind === 'never'`

Ejemplos: `FREQ=DAILY`, `FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE`, `FREQ=MONTHLY;COUNT=5`, `FREQ=YEARLY;UNTIL=20261231T235959Z`.

### Parseo (`parseRrule`) y reglas opacas

- Acepta solo las claves `FREQ`, `INTERVAL`, `COUNT`, `UNTIL`, `BYDAY`. **Cualquier otra clave** (`BYMONTHDAY`, `WKST`, `BYSETPOS`, `EXDATE`-como-parte…) ⇒ `null` (**opaca**): la UI **no la reescribe** salvo acción explícita del usuario.
- `FREQ` obligatoria y dentro del conjunto; `INTERVAL`/`COUNT` enteros `>= 1`; `BYDAY` solo con `FREQ=WEEKLY`; `UNTIL` acepta `YYYYMMDD` o `YYYYMMDDTHHMMSSZ` (se normaliza a `YYYYMMDD`).
- Texto en minúsculas ⇒ opaca (el servidor rechazaría minúsculas, pero un `.ics` importado puede traerlas; no lo destruimos).

### Comportamiento en el diálogo

**Alta (`create`)**
- Campo **Zona horaria** (Select buscable), por defecto la zona del navegador (`browserTimeZone()`), **limpiable** (vacío ⇒ se omite `tzid` ⇒ evento sin zona/UTC).
- Bloque **Repetición**: por defecto sin repetición. Si el usuario fija una frecuencia, se envía `input.rrule = composeRrule(...)`; si no, se omite.

**Edición (`update`)**
- **Zona horaria**: precargada con `detail.dtstart_tzid`; **no limpiable** (el backend no permite borrar la zona por `PATCH`) — sin botón de limpiar y con texto de ayuda. Si el valor no vacío difiere del baseline **o** se conserva, se envía `input.tzid = valor` (omitirlo también conservaría la zona; se mantiene el comportamiento actual de enviarla cuando no es nula para preservar el ancla).
- **Repetición (disciplina D2)**:
  - Regla existente **parseable** (`parseRrule(detail.rrule) !== null`): el constructor se precarga. Si el resultado compuesto **coincide** con el baseline ⇒ **no se envía** `rrule`. Si **cambia** ⇒ `input.rrule = compuesto`. Si el usuario **vacía** la frecuencia ⇒ `input.rrule = ''` (limpia).
  - Regla existente **opaca** (`parseRrule` ⇒ `null`): se muestra en solo lectura «Recurrencia actual: `<texto>``» y el constructor arranca vacío. Casos: (a) el usuario no toca nada ⇒ **no se envía** `rrule` (se preserva verbatim); (b) el usuario compone una regla ⇒ la sustituye; (c) el usuario marca la casilla «Quitar recurrencia» (campo `clearRecurrence`) ⇒ `input.rrule = ''`.
- Capa optimista: `EventChanges` incorpora `'rrule'`, y el optimista pinta el nuevo valor (o `null` al limpiar / al conservar el baseline opaco).

### UI (formulario)

- `Form.Item name="tzid" label="Zona horaria"`: `Select` `showSearch` con `options` = `availableTimeZones()` mapeadas a `{value,label}`; se garantiza que el valor actual esté entre las opciones (aunque no esté en la lista); `allowClear` **solo** en alta; `placeholder="UTC / sin zona"`; `aria-label="Zona horaria"`.
- `Form.Item name="recurFreq" label="Repetición"`: `Select allowClear` con `FREQ_OPTIONS` (Sin repetición / Diaria / Semanal / Mensual / Anual).
- Sub-bloque visible solo cuando `recurFreq` está fijado (patrón `shouldUpdate` ya usado por `durationMode`):
  - `recurInterval` — `InputNumber min={1}` (defecto 1), etiqueta «Cada».
  - Si `WEEKLY`: `recurByDay` — `Select mode="multiple"` con `WEEKDAY_OPTIONS`.
  - `recurEndKind` — `Radio.Group`: «Nunca» / «Después de N veces» / «Hasta».
  - Condicional: `recurCount` (`InputNumber min={1}`) o `recurUntil` (`DatePicker` de fecha).
  - Resumen en vivo: `Typography.Text type="secondary"` con `describeRrule(composeRrule(...))`.
- Si el baseline es **opaco**: `Typography.Text` «Recurrencia actual: `<texto>``» + `Checkbox name="clearRecurrence"` «Quitar recurrencia».
- `aria-label` en todos los controles nuevos.

## Escenarios (BDD)

**Constructor de recurrencia (`recurrence.ts`)**

1. *Dada* `{freq:'DAILY', interval:1, endKind:'never'}` *cuando* `composeRrule` *entonces* devuelve `FREQ=DAILY`.
2. *Dada* `{freq:'WEEKLY', interval:2, byDay:['MO','WE'], endKind:'count', count:5}` *cuando* `composeRrule` *entonces* `FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=5` (orden determinista).
3. *Dada* `{freq:'YEARLY', endKind:'until', until:'20261231'}` *entonces* `FREQ=YEARLY;UNTIL=20261231T235959Z`.
4. *Dada* `FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=5` *cuando* `parseRrule` *entonces* el objeto shell la reproduce (round-trip).
5. *Dada* `FREQ=MONTHLY;BYMONTHDAY=15` (clave desconocida) *entonces* `parseRrule` devuelve `null` (opaca).
6. *Dado* texto con minúsculas o fuera de alfabeto *entonces* `parseRrule` ⇒ `null` y `isRruleAlphabet` ⇒ `false`.
7. *Dada* una regla *entonces* `describeRrule` devuelve un resumen en español no vacío; una opaca se devuelve verbatim sin lanzar.

**Zona horaria (`timezone.ts`)**

8. *Dado* un runtime con `Intl.supportedValuesOf` *entonces* `availableTimeZones()` incluye `Europe/Madrid` y `UTC`.
9. *Dado* un runtime sin `Intl.supportedValuesOf` (parcheado) *entonces* `availableTimeZones()` devuelve `[]` sin lanzar.

**Diálogo — alta (`Calendar.tsx`)**

10. *Dado* el diálogo de alta *cuando* se envía sin tocar zona ni repetición *entonces* `createEvent` recibe `tzid` = zona del navegador e **sin** `rrule`.
11. *Dado* el diálogo de alta *cuando* el usuario fija «Semanal, cada 2, lunes y miércoles, 5 veces» y guarda *entonces* `input.rrule === 'FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=5'` y el evento optimista pinta esa regla.
12. *Dado* el diálogo de alta *cuando* el usuario vacía la zona *entonces* `input` **no** lleva la clave `tzid` y el optimista pinta `dtstart_tzid/dtend_tzid = null`.

**Diálogo — edición (`Calendar.tsx`)**

13. *Dado* un evento con `rrule` parseable *cuando* se abre y se guarda sin tocarlo *entonces* `updateEvent` **no** incluye `rrule`.
14. *Dado* un evento con `rrule` parseable *cuando* se cambia la frecuencia y se guarda *entonces* `input.rrule` lleva la regla nueva.
15. *Dado* un evento con `rrule` parseable *cuando* el usuario vacía la frecuencia *entonces* `input.rrule === ''`.
16. *Dado* un evento con `rrule` **opaca** *cuando* se guarda sin tocarla *entonces* `updateEvent` **no** incluye `rrule`; *cuando* se marca «Quitar recurrencia» *entonces* `input.rrule === ''`.
17. *Dado* un evento con `dtstart_tzid='Europe/Madrid'` *cuando* se abre *entonces* el campo zona muestra `Europe/Madrid` y **no** ofrece limpiar; *cuando* se cambia a otra zona y se guarda *entonces* `input.tzid` es la nueva; *cuando* se guarda sin tocarla *entonces* `input.tzid === 'Europe/Madrid'`.
18. *Dado* un evento sin zona *cuando* se abre *entonces* el campo zona está vacío y `updateEvent` **no** incluye `tzid` si no se toca.

## Tareas

### Tarea 1: util de recurrencia (`web/src/utils/recurrence.ts` + test)

**Archivos:**
- Crear: `web/src/utils/recurrence.ts`
- Crear: `web/src/utils/recurrence.test.ts`

- [ ] **Paso 1 (RED):** escribir `recurrence.test.ts` cubriendo los escenarios 1-7 (`composeRrule` determinista, `parseRrule` round-trip y opacidad, `isRruleAlphabet` espejo del servidor, `describeRrule`) → `npm run test` en **rojo**.
- [ ] **Paso 2 (GREEN):** implementar `recurrence.ts` con los tipos/constantes/funciones del contrato → `npm run test` verde.
- [ ] **Paso 3 (REFACTOR):** `npm run typecheck` y `npm run lint` limpios.

### Tarea 2: util de zonas horarias (`web/src/utils/timezone.ts` + test)

**Archivos:**
- Crear: `web/src/utils/timezone.ts`
- Crear: `web/src/utils/timezone.test.ts`
- Modificar: `web/src/pages/Calendar.tsx:246-249` (eliminar `browserTimeZone` local; importar desde `utils/timezone`)

- [ ] **Paso 1 (RED):** `timezone.test.ts` con los escenarios 8-9 (incluye el caso sin `Intl.supportedValuesOf`, parcheando temporalmente el global y restaurándolo).
- [ ] **Paso 2 (GREEN):** implementar `timezone.ts`; mover `browserTimeZone` y actualizar el import en `Calendar.tsx` (sin cambiar comportamiento).
- [ ] **Paso 3 (REFACTOR):** `typecheck` + `lint`.

### Tarea 3: valores iniciales y capa optimista (`Calendar.tsx`)

**Archivos:**
- Modificar: `web/src/pages/Calendar.tsx` (`EventFormValues` ~256-274; `EventChanges` ~276-281; `formValuesFromDetail` ~205-238; `formInitialValues` ~790-806)

- [ ] **Paso 1:** extender `EventFormValues` con `tzid?`, `recurFreq?`, `recurInterval?`, `recurEndKind?`, `recurCount?`, `recurUntil?`, `recurByDay?`, `clearRecurrence?`.
- [ ] **Paso 2:** añadir `'rrule'` al `Pick` de `EventChanges`.
- [ ] **Paso 3:** en `formValuesFromDetail`, derivar los campos de recurrencia desde `parseRrule(detail.rrule)` (opaca ⇒ constructor vacío) y `tzid: detail.dtstart_tzid ?? undefined`.
- [ ] **Paso 4:** en `formInitialValues` (rama alta), añadir `tzid: browserTimeZone()` y recurrencia vacía.

### Tarea 4: lógica de envío (`Calendar.tsx`, `handleSubmit` ~500-763)

**Archivos:**
- Modificar: `web/src/pages/Calendar.tsx`

- [ ] **Paso 1:** alta — `input.tzid` desde el campo (omitir si vacío); `input.rrule` = `composeRrule` si hay regla; actualizar el objeto optimista (`dtstart_tzid/dtend_tzid/rrule`).
- [ ] **Paso 2:** edición — `input.tzid` desde el campo si no vacío (omitir si vacío ⇒ preserva); `input.rrule` con la disciplina D2 (escenarios 13-16: sin cambios ⇒ omitir; cambio ⇒ compuesto; vaciado/vaciar ⇒ `''`; opaca intacta ⇒ omitir).
- [ ] **Paso 3:** actualizar el `changes` optimista de edición con `rrule`/`dtstart_tzid`/`dtend_tzid` coherentes con lo enviado.

### Tarea 5: campos de UI en el diálogo (`Calendar.tsx` ~1094-1252)

**Archivos:**
- Modificar: `web/src/pages/Calendar.tsx`

- [ ] **Paso 1:** `Select` de zona (escenario 17-18): buscable, opciones de `availableTimeZones()` + valor actual garantizado, `allowClear` solo en alta, `aria-label`.
- [ ] **Paso 2:** bloque de repetición: `Select` de frecuencia, y con `shouldUpdate` el sub-bloque (`InputNumber` intervalo, `Select multiple` de días si semanal, `Radio.Group` de fin, `InputNumber`/`DatePicker` condicionales, resumen `describeRrule`).
- [ ] **Paso 3:** si el baseline es opaco: texto verbatim «Recurrencia actual» + `Checkbox` «Quitar recurrencia».

### Tarea 6: tests de integración del diálogo (`Calendar.test.tsx`)

**Archivos:**
- Modificar: `web/src/pages/Calendar.test.tsx`

- [ ] **Paso 1 (RED):** añadir los escenarios 10-18 con los helpers existentes (`makeEvent`/`makeDetail`, `renderCalendar`, espías de `message`). Verificar primero en **rojo**.
- [ ] **Paso 2 (GREEN):** dejar la suite en verde.
- [ ] **Paso 3 (REFACTOR):** `npm run test`, `npm run typecheck`, `npm run lint` limpios en `web/`.

### Tarea 7: documentación y cierre

**Archivos:**
- Crear: `docs/F26-evento-tzid-recurrencia-ui.md`
- Renombrar al cerrar: `plans/PLAN-030-evento-tzid-rrule-ui.md` → `plans/PLAN-030-evento-tzid-rrule-ui-COMPLETED.md`

- [ ] **Paso 1:** documentar la feature (alcance, disciplinas D2, limitación: la zona no se puede limpiar por `PATCH`; el constructor cubre el subconjunto RFC 5545 y las reglas opacas se preservan).
- [ ] **Paso 2:** verificación end-to-end manual contra el binario publicado (N+1): abrir el diálogo, fijar zona y recurrencia, guardar, reabrir y comprobar `TZID`/`RRULE` en el `.ics` servido.
- [ ] **Paso 3:** renombrar el plan a `-COMPLETED.md` y anotar `## Resultado`.

## Verificación (comandos)

Desde `web/`: `npm run test`, `npm run typecheck`, `npm run lint`.

## Fuera de alcance

- `EXDATE`/`RDATE`, `RECURRENCE-ID`, `ORGANIZER`/`ATTENDEE`, `SEQUENCE`/`CREATED`/`LAST-MODIFIED` (futuros planes).
- Expansión de recurrencia (sigue sin expandirse; limitación F07 documentada).
- Cualquier cambio en el backend, el esquema o la API.
- Indicadores de recurrencia/zona en la lista o el detalle (decisión: solo en el diálogo).

## Riesgos y notas

- **Alfabeto del servidor vs. texto compuesto:** `composeRrule` solo emite `A-Z0-9=;,-`, de modo que toda regla del constructor pasa `validate_rrule`; el `UNTIL` en UTC (`...T235959Z`) es válido y está dentro del alfabeto.
- **Reglas opacas:** nunca se reescriben sin acción explícita (misma doctrina que `DURATION`/`location`/`url` en F24/F25), evitando perder partes que la UI no modela.
- **Zona no limpiable en edición:** limitación del DTO (`PATCH` no distingue ausente de `null`); se refleja en la UI (sin botón de limpiar) y se documenta.
- **`Intl.supportedValuesOf`:** si falta, la lista queda vacía pero el campo sigue funcionando con el valor actual (garantizado entre las opciones) y entrada libre; no se rompe.
- **jsdom/Node:** usan el `Intl` de Node, así que los tests ven la lista completa real.

## Resultado

Cerrado el 2026-10-30. **Solo frontend**; backend, esquema y `web/src/types/api.ts` intactos. Las 7 tareas, completadas.

- **Módulos nuevos**: `web/src/utils/recurrence.ts` (+ `recurrence.test.ts`), `web/src/utils/timezone.ts` (+ `timezone.test.ts`).
- **`web/src/pages/Calendar.tsx`** (+ `Calendar.test.tsx`): campos *Zona horaria* y *Repetición* (constructor estructurado + resumen en vivo), disciplina D2 con **comparación canónica** del baseline, y capa optimista extendida (`rrule`).
- **Verificación**: `npm run test` → 305 passed / 0 failed; `npm run typecheck` y `npm run lint` limpios; `npm run build` OK.
- **Auditoría `@react-reviewer`**: una ronda de hallazgos → 🔴 **B1** (crash/`400` al borrar los controles de fin por los `null` de antd v6) y 🟠 **M1** (la comparación D2 textual reescribía reglas no canónicas y cambiaba el fin en `UNTIL` corto), más m1/m2/m3; corregidos con tests nuevos y **APROBADO** en la re-revisión.
- **Docs**: `docs/F26-evento-tzid-recurrencia-ui.md`.
- **Límites**: la zona no se puede quitar en edición; el constructor cubre un subconjunto de RFC 5545 (el resto se preserva verbatim); sin indicadores en lista/detalle.
