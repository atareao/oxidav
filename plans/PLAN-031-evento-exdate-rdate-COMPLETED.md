# PLAN-031 — F27: `EXDATE` y `RDATE` en el diálogo de eventos

**Estado: PROPUESTO — pendiente de aprobación del usuario. No se escribe código hasta que se apruebe.**

## Punto de partida (medido)

F26 (PLAN-030) cerró la edición de `TZID` y `RRULE` en la SPA, dejando **17/26** propiedades de `VEVENT` utilizables. `EXDATE` y `RDATE` **no existen en ninguna capa** hoy: `grep -rIn "EXDATE\|RDATE" src/ web/src/` → 0 resultados. La maquinaria correcta ya está montada (F24/F25):

- `raw_payload` es la fuente de verdad; la escritura es **quirúrgica** (`reconcile_ics`) y conserva **byte a byte** toda línea no modelada. Por eso hoy un `EXDATE` de un cliente DAV **se preserva** (el `PUT` guarda `raw` verbatim: `src/dav/caldav.rs:546`), pero es **invisible e ineditable** en la SPA.
- El patrón «proyección al leer, sin columnas» ya existe para las seis propiedades de F25 (`CATEGORIES`, `ATTACH`, …): `project_event` (`src/dav/ics/projection.rs`) deriva el detalle del payload sin añadir columna ni migración.

**Consecuencia:** F27 es el compañero natural de F26 y reutiliza **toda** la maquinaria. No hay migración ni columnas nuevas.

## Objetivo

1. **O1 — Proyección del detalle.** `EventProjection` y `EventDetailDto` ganan `exdate`/`rdate` (instantes en ms) y su texto **opaco** cuando el payload trae una variante no soportada, expuestos por `GET /api/v1/collections/{id}/events/{uid}` (y devueltos también por `POST` y `PATCH`).
2. **O2 — Edición.** Las dos propiedades se pueden fijar, añadir, quitar y **vaciar** desde la SPA, con `reconcile_ics` como **único** camino de escritura del payload.
3. **O3 — La lista no parsea.** `EventDto` (la lista del mes) sigue alimentándose **solo de columnas**: ni una columna nueva, ningún `.ics` parseado para listar.

## Arquitectura: proyección al leer, sin columnas (idéntico a F25)

| Aspecto | Decisión |
|---|---|
| Fuente de verdad | `raw_payload`; la escritura nunca recompone (F24) |
| Almacenamiento | **sin columnas** (como `CATEGORIES`/`ATTACH`): `EXDATE`/`RDATE` son multi-valor y no se consultan |
| Lectura | `project_event` lee las **líneas crudas** de nivel superior del primer `VEVENT` |
| Escritura | `reconcile_ics` reconcilia en su sitio; `build_ics` los emite en alta |
| Rechazado | columna por propiedad (esquema inútil: no se filtra por ellas) e híbrido (dos modelos para el mismo dato) |

## Subconjunto soportado y reglas opacas (disciplina F26)

Se modela **solo** el valor `DATE-TIME` (con `TZID` opcional), que es el que emite todo cliente real junto a un `DTSTART` `DATE-TIME`. Todo lo demás se considera **opaco** y se **preserva verbatim** (no se destruye):

- `VALUE=DATE` (todo el día) y `VALUE=PERIOD` (`RDATE` con `start/end`).
- Cualquier token de fecha-hora que `parse_datetime_with_tzid` no sepa leer.
- **Regla de conjunto:** si **alguna** línea de la propiedad no encaja en el subconjunto, **toda** la propiedad es opaca (no se modela a medias).

Un propiedad opaca se muestra **verbatim** en el diálogo (como `opaqueRrule` en F26) y **no se reescribe** salvo acción explícita del usuario («Quitar»).

## Contratos

### `src/dav/ics/mod.rs`

- `EventFields` += dos campos de **tres estados**:
  ```rust
  /// `EXDATE` — instantes (ms) excluidos de la recurrencia, ordenados y sin
  /// duplicados. `Some(vec![])` **limpia** la propiedad; `None` significa
  /// «no tocar las líneas `EXDATE` del payload» (son **opacas**, fuera del
  /// subconjunto soportado).
  pub exdate: Option<Vec<i64>>,
  /// `RDATE` — instantes (ms) añadidos a la recurrencia. Misma semántica de tres
  /// estados que `exdate`.
  pub rdate: Option<Vec<i64>>,
  ```
- `RECONCILE_ORDER` pasa de `[&str; 14]` a `[&str; 16]`, insertando `"RDATE"` y `"EXDATE"` **después** de `"RRULE"` (y antes de `"STATUS"`).
- Emisión (`build_ics` **y** `reconcile_ics` comparten los mismos helpers):
  - Una **sola** línea por propiedad: `RDATE:<v1>,<v2>,…` y `EXDATE:<v1>,<v2>,…`.
  - Cada instante se formatea **siempre en UTC** (`format_datetime_ms`, sufijo `Z`): `EXDATE:<v1>Z,<v2>Z,…` y `RDATE:<v1>Z,…`, **nunca** `;TZID=`. Los instantes se ordenan ascendente y se deduplican. **Por qué UTC y no la zona de `DTSTART`:** emitir en zona local es *lossy* en la hora ambigua del cambio de hora de otoño (la segunda pasada de la hora repetida se formatea a hora local y al re-parsear se resuelve a la **primera** pasada, moviendo el instante −1 h en silencio). Un valor UTC hace el round-trip **exacto** y es RFC 5545 válido aunque `DTSTART` lleve `TZID`.
  - `build_ics` emite la línea **solo si `Some(v)` con `v` no vacío**; `None` y `Some(vec![])` no producen línea (mismo resultado).
- `reconcile_ics`: `final_values` pasa de array fijo a `Vec<(&'static str, Vec<String>)>` y **solo se empuja** la entrada `RDATE`/`EXDATE` cuando el campo es `Some` (si es `None`, la propiedad **ni se incluye**: sus líneas quedan intactas). Con `Some`:
  - `Some(vec)` no vacío → se sustituye la **primera** ocurrencia por la línea unida y se **eliminan las demás** (misma regla multi-valor→una-línea que `CATEGORIES`); si no hay ocurrencia, se **inserta** en su posición canónica.
  - `Some(vec![])` → se **eliminan** todas las ocurrencias (vaciar limpia).
- Nuevo helper compartido (junto a `property_name`/`property_value`): `pub(crate) fn property_param(text: &str, name: &str) -> Option<&str>` — extrae un parámetro del *head* de la línea (`;NAME=value`), insensible a mayúsculas, con los `value` entre comillas despojadas, y el último ocurre si se repite.
- Con la propiedad ausente/opaca la salida de `reconcile_ics` sigue siendo **byte a byte idéntica** (contrato con test), y una propiedad modelada **no cambia** sus líneas cuando no se edita más allá de la normalización canónica de instantes.

### `src/dav/ics/projection.rs`

- `EventProjection` +=
  ```rust
  /// Instantes (ms) de todas las líneas `EXDATE` **del subconjunto soportado**,
  /// ordenados y sin duplicados. Vacío también cuando la propiedad es opaca.
  pub exdate: Vec<i64>,
  /// Texto **verbatim** de las líneas `EXDATE` cuando la propiedad **no** encaja
  /// en el subconjunto soportado (informativo; su preservación la garantiza
  /// `reconcile_ics` al no tocar las líneas). `None` si es modelable o ausente.
  pub exdate_opaque: Option<String>,
  /// Igual que `exdate`.
  pub rdate: Vec<i64>,
  /// Igual que `exdate_opaque`.
  pub rdate_opaque: Option<String>,
  ```
- Nuevo lector `project_recur_dates(lines, stacks, target, target_end, name) -> (Vec<i64>, Option<String>)`:
  - Sin líneas → `(vec![], None)`.
  - Por cada línea: si lleva `VALUE=DATE`/`VALUE=PERIOD` → opaca; si no, se parte el valor por comas y cada token se resuelve con `parse_datetime_with_tzid(token, property_param(line, "TZID"))`.
  - Si **alguna** línea es opaca o algún token no parsea → `(vec![], Some(texto_verbatim))`, donde `texto_verbatim` es la concatenación de los **textos de línea completos** (`NAME;PARAM:value`), unidos con `"; "`.
  - Si todas encajan → `(instantes ordenados y deduplicados, None)`.

### `src/api/dto.rs`

- `EventDetailDto` += `pub exdate: Vec<i64>`, `pub exdate_opaque: Option<String>`, `pub rdate: Vec<i64>`, `pub rdate_opaque: Option<String>` (rellenados desde la proyección en `EventDetailDto::new`). Se serializan como `exdate: number[]`, `exdate_opaque: string|null`, etc.
- `CreateEventRequest` y `UpdateEventRequest` += `pub exdate: Option<Vec<i64>>` y `pub rdate: Option<Vec<i64>>`.
- `UpdateEventRequest::is_empty` incorpora ambos (`exdate.is_none() && rdate.is_none()`).

### `src/api/events.rs`

- `const MAX_RECUR_DATES: usize = 64;` (espejo de `MAX_CATEGORIES`).
- `fn validate_instant_list(items: &[i64], field: &str) -> Result<Vec<i64>, ApiError>`:
  - cada valor pasa por `check_instant` (`supported_instant_range_ms`, años 1..=9999) → fuera de rango = `400`;
  - salida **ordenada ascendente y sin duplicados**;
  - `> MAX_RECUR_DATES` → `400` con mensaje explícito;
  - lista vacía = limpiar (no error).
- `create`: `let exdate = validate_instant_list(request.exdate.as_deref().unwrap_or_default(), "exdate")?;` (ídem `rdate`); se pasan a `EventFields` como `Some(exdate)` / `Some(rdate)`. `NewCalendarObject` **no cambia** (sin columna).
- `update` (regla de merge uniforme de F24/F25):
  - `Some(values)` → `Some(validate_instant_list(values, "exdate")?)` — un `[]` **limpia**;
  - `None` → hereda de la proyección: si `stored.exdate_opaque.is_some()` → `None` (**no tocar**), si no → `Some(stored.exdate.clone())`; sin proyección usable → `Some(vec![])` (caso residual conocido, igual que las seis de F25).
  - Se pasan a `EventFields.exdate`/`.rdate` y siguen hacia `reconcile_ics`.
- Solo se valida lo que **entra por la API**; un valor heredado del payload **no** se revalida (regla de F24).

### `web/src/types/api.ts`

- `EventDetail` += `exdate: number[]`, `exdate_opaque: string | null`, `rdate: number[]`, `rdate_opaque: string | null`.
- `CreateEventInput` y `UpdateEventInput` += `exdate?: number[]`, `rdate?: number[]`.
- `CalendarEvent` (la lista) **no cambia**.

### `web/src/utils/recurDates.ts` (+ `recurDates.test.ts`)

Módulo **puro** (sin React ni red), espejo de lo que el servidor espera:
- `sortUniqueInstants(values: number[]): number[]` — ordena ascendente y deduplica (misma forma canónica que el servidor).
- `sameInstantSet(a: number[], b: number[]): boolean` — comparación **independiente del orden** (disciplina D2).
- `describeRecurDates(instants: readonly number[]): string` — resumen legible para la vista del diálogo usando el locale/formato de `dayjs` del navegador («3 fechas: 15/03/2026 10:00, …»); `''` con lista vacía.

### `web/src/pages/Calendar.tsx`

- `EventFormValues` += `exdate?: Dayjs[]`, `rdate?: Dayjs[]`, `clearExdate?: boolean`, `clearRdate?: boolean`.
- `EventChanges` (`Pick<EventDetail, …>`) += `'exdate' | 'rdate'`.
- `formValuesFromDetail`: `exdate: (detail.exdate_opaque === null ? detail.exdate : []).map((ms) => dayjs(ms))` (ídem `rdate`).
- `formInitialValues` (alta): `exdate: []`, `rdate: []`.
- `handleSubmit`:
  - **alta** — `input.exdate`/`input.rdate` = instantes (`sortUniqueInstants`) si la lista no está vacía.
  - **edición (D2)** — solo se envía `exdate`/`rdate` cuando **cambia** respecto del baseline (`sameInstantSet` contra `editDetail.exdate`/`.rdate`); vaciar → `[]`; si el baseline es **opaco** y no se toca → se **omite**; si el baseline es opaco y el usuario marca «Quitar» → `[]`.
  - capa **optimista** actualizada de forma coherente con lo enviado.
- **UI** (dentro del bloque de recurrencia, `shouldUpdate`): dos `Form.List` — **«Fechas excluidas» (`exdate`)** y **«Fechas extra» (`rdate`)** — con un `DatePicker showTime` por entrada, botón «Quitar» por entrada y «Añadir fecha». El bloque se muestra cuando hay `recurFreq` **o** el evento ya trae `exdate`/`rdate` (para no ocultar datos existentes). Si la propiedad es **opaca**: texto verbatim «… actuales: <opaque>` + `Checkbox` «Quitar» (patrón `opaqueRrule` de F26). Etiquetas y `aria-label` en español, coherentes con F25/F26.

## Decisiones de diseño (y por qué)

| Decisión | Por qué |
|---|---|
| Instantes en **ms** (`Vec<i64>`), no tokens crudos | Simétrico con `dtstart`/`dtend`, permite `DatePicker` real y validación con `check_instant`; los tokens crudos obligarían a editar `20260315T100000Z` a mano |
| Emisión **siempre en UTC** | Un valor UTC hace el round-trip **exacto** (incluida la hora ambigua del fall-back, donde la emisión en zona local lo corrompe −1 h). Es RFC 5545 válido: `EXDATE`/`RDATE` admiten `DATE-TIME` UTC aunque `DTSTART` lleve `TZID`. Además **no** añade `VTIMEZONE` |
| **Sin** `tzid` propio por ocurrencia | Los valores `DATE-TIME` con `TZID` se **resuelven a instante** al leer; se reemiten en **UTC** (normalización a un instante sin pérdida) |
| **Tres estados** (`None` vs `Some(vec![])`) en `EventFields` | Es la única forma de que `reconcile_ics` distinga «no tocar una propiedad opaca» de «limpiar la propiedad» |
| `VALUE=DATE`/`VALUE=PERIOD` → **opaco** | El modelo de eventos del repo es `DATE-TIME`; convertir todo-el-día a `…T000000Z` cambiaría el tipo de valor en silencio |
| Una sola línea por propiedad (comas) | Forma habitual en `.ics` reales; simplifica reconciliación (como `CATEGORIES`) |
| Validación con `check_instant` + tope `64` | Frontera de seguridad idéntica a `dtstart`/`dtend`: evita instantes imposibles (`i64::MAX`) y listas abusivas |
| `MAX_RECUR_DATES = 64` | Espejo exacto de `MAX_CATEGORIES` |

## Escenarios (BDD)

1. **Proyección — ausencia.** Un `.ics` sin `EXDATE`/`RDATE` proyecta `exdate: []`, `rdate: []`, `exdate_opaque: null`, `rdate_opaque: null`.
2. **Proyección — modelable.** `EXDATE;TZID=Europe/Madrid:20260315T100000,20260322T100000` → dos instantes en ms, ordenados, `exdate_opaque: null`.
3. **Proyección — UTC.** `RDATE:20260401T090000Z` → un instante, `rdate_opaque: null`.
4. **Proyección — opaco.** `EXDATE;VALUE=DATE:20260315` → `exdate: []` y `exdate_opaque` con el texto de la línea.
5. **Proyección — mixto.** Una línea modelable y otra `VALUE=DATE` → **toda** la propiedad opaca.
6. **`build_ics` — byte-idéntico.** Con `exdate: None`/`rdate: None` (o `Some(vec![])`) la salida **no** contiene `EXDATE:`/`RDATE:` y es **byte a byte idéntica** a antes de F27.
7. **`build_ics` — emisión UTC.** Con o sin `dtstart_tzid`, los instantes se emiten **siempre en UTC**: `EXDATE:<…Z>,<…Z>` (nunca `;TZID=`), y el round-trip por `parse_datetime_ms` es **exacto**, incluida la hora ambigua del fall-back de `Europe/Madrid` (segunda pasada).
8. **`reconcile_ics` — opaco intacto.** Payload con `EXDATE;VALUE=DATE:…` y `fields.exdate = None` → la línea sale **verbatim**.
9. **`reconcile_ics` — limpiar.** `fields.exdate = Some(vec![])` y payload con dos `EXDATE` → todas las líneas `EXDATE` **desaparecen**.
10. **`reconcile_ics` — sustituir/colapsar.** Payload con dos líneas `EXDATE` y `Some(vec![a,b])` → queda **una** línea con `a,b`; el resto del payload intacto.
11. **`reconcile_ics` — insertar.** Payload sin `EXDATE` y `Some(vec![a])` → se inserta en su posición canónica (tras `RRULE`), dentro del `VEVENT`.
12. **`validate_instant_list`.** Ordena y deduplica; `400` por fuera de rango (años 1..=9999); `400` si supera `MAX_RECUR_DATES`; `[]` no es error.
13. **REST alta.** `POST` con `exdate` → el `.ics` almacenado lleva `EXDATE` y el detalle lo devuelve.
14. **REST edición — hereda.** `PATCH` sin `exdate` sobre un evento con `EXDATE` modelable → se conserva.
15. **REST edición — limpia.** `PATCH` con `exdate: []` → el `.ics` pierde sus `EXDATE`.
16. **REST edición — opaco.** `PATCH` que no menciona `exdate` sobre un evento con `EXDATE;VALUE=DATE` → la línea sigue verbatim.
17. **Diálogo — alta.** Añadir dos «Fechas excluidas» y una «Fecha extra» → el `POST` lleva `exdate`/`rdate` con los instantes correctos.
18. **Diálogo — edición D2 sin cambios.** Abrir un evento con `exdate` y guardar sin tocarlo → **no** se envía `exdate` en el `PATCH`.
19. **Diálogo — edición D2 con cambio.** Añadir/quitar una fecha → se envía el `exdate` resultante (orden canónico).
20. **Diálogo — limpiar.** Quitar todas las entradas → se envía `exdate: []`.
21. **Diálogo — opaco.** Evento con `exdate_opaque` → se muestra verbatim; marcar «Quitar» → `exdate: []`.

## Tareas (TDD)

### Tarea 1: capa `ics` — parse/emisión/reconciliación (`src/dav/ics/mod.rs` + tests)
- [ ] **Paso 1 (RED):** escribir tests de escenarios 6-11 (byte-idéntico sin la propiedad, emisión con/sin zona, opaco intacto, limpiar, colapsar, insertar) → `cargo test` en **rojo**.
- [ ] **Paso 2 (GREEN):** añadir `exdate`/`rdate` a `EventFields`; ampliar `RECONCILE_ORDER` (16), `build_ics` (línea única, orden+dedup, **UTC siempre**) y `reconcile_ics` (`Vec` de `final_values`, entrada **solo** si `Some`); añadir `property_param`. Actualizar las construcciones de `EventFields` en los tests existentes.
- [ ] **Paso 3 (REFACTOR):** `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test` verdes.

### Tarea 2: proyección (`src/dav/ics/projection.rs` + tests)
- [ ] **Paso 1 (RED):** tests de escenarios 1-5.
- [ ] **Paso 2 (GREEN):** añadir los cuatro campos a `EventProjection` y `project_recur_dates`.
- [ ] **Paso 3 (REFACTOR):** fmt/clippy/test.

### Tarea 3: DTO y REST (`src/api/dto.rs`, `src/api/events.rs` + tests)
- [ ] **Paso 1 (RED):** tests de `validate_instant_list` (escenario 12) y de alta/edición (13-16) en el patrón existente de `src/api/events.rs` / `tests/api.rs`.
- [ ] **Paso 2 (GREEN):** campos en `EventDetailDto`/`CreateEventRequest`/`UpdateEventRequest` (+`is_empty`), `MAX_RECUR_DATES`, `validate_instant_list`, wiring de create/update.
- [ ] **Paso 3 (REFACTOR):** fmt/clippy/test.

### Tarea 4: tipos y util puro (`web/src/types/api.ts`, `web/src/utils/recurDates.ts` + test)
- [ ] **Paso 1 (RED):** `recurDates.test.ts` (`sortUniqueInstants`, `sameInstantSet`, `describeRecurDates`).
- [ ] **Paso 2 (GREEN):** implementar el util y ampliar los tipos.
- [ ] **Paso 3 (REFACTOR):** `npm run typecheck`, `npm run lint`, `npm run test`.

### Tarea 5: diálogo (`web/src/pages/Calendar.tsx`)
- [ ] **Paso 1:** `EventFormValues`/`EventChanges`/`formValuesFromDetail`/`formInitialValues`/optimista con `exdate`/`rdate`.
- [ ] **Paso 2:** lógica D2 en `handleSubmit` (omitir si no cambia; `[]` si se limpia; `None` si opaco intacto).
- [ ] **Paso 3:** UI: dos `Form.List` («Fechas excluidas»/«Fechas extra») con `DatePicker showTime`, y el bloque opaco verbatim + «Quitar».

### Tarea 6: tests de integración del diálogo (`web/src/pages/Calendar.test.tsx`)
- [ ] **Paso 1 (RED):** escenarios 17-21 con los helpers existentes (`makeDetail`, `renderCalendar`, espías de `message`) → rojo primero.
- [ ] **Paso 2 (GREEN):** suite en verde.
- [ ] **Paso 3 (REFACTOR):** `npm run test`, `npm run typecheck`, `npm run lint` limpios.

### Tarea 7: documentación y cierre
- [ ] **Paso 1:** `docs/F27-evento-exdate-rdate.md` (alcance, subconjunto soportado, opacidad, límites: sin `VALUE=DATE`/`PERIOD`, normalización de zona a la de `DTSTART`).
- [ ] **Paso 2:** verificación end-to-end manual: crear con `exdate`, reabrir, comprobar `EXDATE` en el `.ics` servido; importar un `.ics` con `EXDATE` opaco y comprobar que sobrevive a un `PATCH` de `summary`.
- [ ] **Paso 3:** renombrar a `plans/PLAN-031-evento-exdate-rdate-COMPLETED.md` y anotar `## Resultado`.

## Verificación (comandos)

- Backend: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` (refrescar `Cargo.lock` solo si cambian dependencias — aquí no).
- Frontend (desde `web/`): `npm run test` (`vitest run`), `npm run typecheck` (`tsc -b`), `npm run lint`, `npm run build`.
- Auditoría: `@react-reviewer` (frontend) y `@rust-reviewer` (backend) antes de cerrar.

## Fuera de alcance

- `VALUE=DATE` (todo el día) y `VALUE=PERIOD` (`RDATE`); se preservan **opacos**, no se editan.
- `RECURRENCE-ID` (instancias), `SEQUENCE`/`CREATED`/`LAST-MODIFIED`, `ORGANIZER`/`ATTENDEE` (features siguientes).
- Indicadores de exclusión/fechas extra en la lista o el detalle (decisión F26: solo en el diálogo).
- Columnas nuevas o migración: **ninguna**.

## Riesgos y notas

- **Churn en tests de `EventFields`:** añadir dos campos obliga a actualizar muchas construcciones literales en `src/dav/ics/mod.rs`; es mecánico, pero hay que hacerlo con cuidado para no alterar lo que cada test comprueba.
- **Normalización a UTC:** un `EXDATE`/`RDATE` importado como hora local con `TZID` se **resuelve a su instante** al leer y se reemite en **UTC** (mismo instante, otra representación). Es deliberado: garantiza round-trip exacto y evita la corrupción silenciosa de la hora ambigua del fall-back.
- **Multi-valor→una-línea:** editar **colapsa** varias líneas `EXDATE` en una sola (normalización deliberada, como `CATEGORIES` en F25).
- **Sin cambios en DAV:** el `PUT` sigue guardando `raw` verbatim; `EXDATE`/`RDATE` ya sobreviven hoy. Esta feature solo los hace **visibles y editables**.

## Correcciones de la auditoría (ronda 1 de backend)

`@rust-reviewer` auditó el backend y estos hallazgos se cerraron:

- **🟠 M-2 (decisión arquitectónica):** se abandona la emisión en la zona de `DTSTART` y `EXDATE`/`RDATE` se emiten **siempre en UTC**. Motivo: la emisión local es *lossy* en la hora ambigua del fall-back (corrompía el instante −1 h en silencio). Tests: `build_ics_emits_recur_dates_always_in_utc` y `build_ics_recur_dates_round_trip_exactly_across_the_fall_back_hour`.
- **🟡 m-3:** `reconcile_keeps_opaque_exdate_byte_for_byte_when_none` (igualdad completa, no `contains`).
- **🟡 m-4:** cobertura de `RDATE` y de RDATE+EXDATE combinados (`reconcile_inserts_rdate_and_exdate_in_canonical_order`, etc.).
- **🟡 m-5:** rechazo *fail-fast* por longitud de entrada en `validate_instant_list` (el recuento por entradas recibidas ya era el correcto; el cambio solo evita buffering parcial y prueba la semántica declarada).
- **🟡 m-6:** tests REST `create_rejects_out_of_range_exdate`, `create_rejects_too_many_distinct_exdates` y `patch_with_empty_exdate_clears_an_opaque_exdate` (E2E del botón «Quitar»).
- **🟢 n-7:** `projects_dedups_instants_across_exdate_lines`, `projects_rdate_from_multiple_lines_ordered`.

Veredicto de la re-auditoría: **APROBADO CON CAMBIOS**; los 7 hallazgos, cerrados.

## Resultado

**Estado: CERRADO.** Plan ejecutado de principio a fin (TDD y auditorías incluidas). El plan de origen era `plans/PLAN-031-evento-exdate-rdate.md`.

### Rojo → verde, medido

- Backend: la suite pasó de **638 → 658** tests tras las Tareas 1-3 y las correcciones de auditoría; `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` siempre limpios.
- Frontend: la suite pasó de **305 → 331** tests (23 ficheros); `npm run typecheck` y `npm run lint` limpios.

### Lo que se cumplió

- **O1** — `EventProjection`/`EventDetailDto` exponen `exdate`/`rdate` (ms) y su texto opaco; `GET`/`POST`/`PATCH` los devuelven.
- **O2** — `EXDATE`/`RDATE` editables desde la SPA (añadir/quitar/vaciar y «Quitar» sobre opacas), con `reconcile_ics` como único camino de escritura.
- **O3** — la lista del mes sigue alimentándose solo de columnas; **sin migración ni columnas nuevas**.
- Los 21 escenarios (6-16 backend; 17-21 frontend) cubiertos con test.

### Desviaciones y correcciones (dichas, no escondidas)

1. **Emisión UTC (M-2 de la auditoría).** El plan original emitía en la zona de `DTSTART`; se **cambió a UTC** porque la emisión local es *lossy* en la hora ambigua del fall-back. Contrato y escenario 7 del plan se actualizaron en consecuencia.
2. **`preserve={false}` en `Form.List`.** El primer intento usó ese prop, que **antd v6 no acepta** (rompió el typecheck). Se resolvió gateando el envío a `rule !== null` en alta.
3. **`describeRecurDates`.** La firma del plan pasó a `(instants: readonly number[]): string` (sin `locale`), como se implementó.

### Hallazgos de la auditoría, cerrados

- **Backend (`@rust-reviewer`, ronda 1 + re-auditoría):** 🟠 M-2 (emisión UTC), 🟡 m-3/m-4/m-5/m-6, 🟢 n-7. Nota: m-5 se retiró como hallazgo tras comprobar que el recuento por entradas recibidas ya era correcto; el cambio quedó como rechazo *fail-fast*.
- **Frontend (`@react-reviewer`, ronda 1 + re-auditoría):** 🟡 cobertura (RDATE opaca + `clearRdate` + dedup + descarte al limpiar frecuencia) y 🟡 valores ocultos (`preserve`→gate), 🟢 comentario obsoleto.

### Ficheros

- **Backend:** `src/dav/ics/mod.rs`, `src/dav/ics/projection.rs`, `src/api/dto.rs`, `src/api/events.rs`, `tests/api.rs`.
- **Frontend:** `web/src/types/api.ts`, `web/src/utils/recurDates.ts` (+ `.test.ts`), `web/src/pages/Calendar.tsx`, `web/src/pages/Calendar.test.tsx`.
- **Docs:** `docs/F27-evento-exdate-rdate.md`.

### Fuera de alcance (sin tocar)

`VALUE=DATE`/`VALUE=PERIOD` (opacos, preservados), `RECURRENCE-ID`, `SEQUENCE`/`CREATED`/`LAST-MODIFIED`, `ORGANIZER`/`ATTENDEE`, indicadores en la lista, y **cualquier** migración/columna.

### Versionado

Feature nueva (`feat`) → bump **minor**; sin cambios de esquema. `web/package.json` no se versiona.
