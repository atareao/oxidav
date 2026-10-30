# F28 — `SEQUENCE` editable y `CREATED`/`LAST-MODIFIED` de solo lectura

Plan: `plans/PLAN-032-vevent-secuencia-y-marcas.md` (cerrado como `-COMPLETED`).

## Qué entrega

Tres propiedades del `VEVENT` nuevas en el diálogo de alta/edición de eventos de la SPA:

- **`SEQUENCE`** (revisión del evento, RFC 5545 §3.8.7.4) — **editable**: un entero no negativo.
- **`CREATED`** y **`LAST-MODIFIED`** (§3.8.7.1/§3.8.7.3) — **de solo lectura**: se muestran, nunca se reescriben desde OxiDav.

Antes las tres eran **invisibles**: un `.ics` importado que las llevaba **se preservaba** (el `PUT` de DAV guarda el payload verbatim y `reconcile_ics` no toca lo no modelado), pero la SPA no mostraba ni permitía editar ninguna.

Abarca **backend y frontend** (proyección, reconciliación, DTO, API REST y diálogo). **Sin migración ni columnas nuevas.**

## Punto de partida

`SEQUENCE`/`CREATED`/`LAST-MODIFIED` no existían en ninguna capa (`grep` → 0). La maquinaria correcta ya estaba montada por F24/F25/F27: `raw_payload` es la única fuente de verdad; `reconcile_ics` edita el `.ics` línea a línea conservando lo no modelado; y el patrón «proyección al leer, sin columnas» ya se usaba para F25 (`CATEGORIES`, `ATTACH`, …) y F27 (`EXDATE`, `RDATE`).

## Arquitectura: proyección al leer, sin columnas

| Aspecto | Decisión |
|---|---|
| Fuente de verdad | `raw_payload`; la escritura nunca recompone (F24) |
| Almacenamiento | **sin columnas** (como F25/F27): no se filtran por SQL |
| Lectura | `project_event` lee las **líneas crudas** del primer `VEVENT` |
| Escritura | solo `SEQUENCE` entra en `RECONCILE_ORDER`; `build_ics` lo emite en alta |

**Novedad de F28 — el eje lectura/escritura se separa por primera vez.** F25 y F27 modelaron propiedades **editables**; aquí `CREATED`/`LAST-MODIFIED` son **solo lectura**. Eso las hace **más baratas, no más caras**: una propiedad que no está en `EventFields` ni en `RECONCILE_ORDER` es, por construcción, preservada verbatim (F24) — `reconcile_ics` no tiene código que la emita ni la borre.

## Colisión de nomenclatura evitada

El DTO ya exponía `created_at`/`updated_at`, que son las **columnas de la fila** (cuándo se insertó/actualizó el registro en SQLite), **no** las propiedades ICS. Las de F28 se exponen como **`created`** y **`last_modified`** (ms Unix, UTC). Un `.ics` puede llevar `CREATED` muy anterior al `created_at` de la fila, y un `PUT` de un cliente DAV puede cambiar `LAST-MODIFIED` sin que la fila se reinserte.

## `SEQUENCE`: tres estados + opaco

- Entero no negativo en **rango cerrado** `0 ≤ n ≤ 2147483647` (RFC 5545 §3.8.7.4). Fuera de rango → `400`.
- En `EventFields` es `Option<i64>` con **tres estados**: `None` = «no tocar las líneas `SEQUENCE` del payload»; `Some(n)` = fijar/emitir.
- Un `SEQUENCE` **opaco** heredado de un cliente (`SEQUENCE:x-custom`) no es modelable: se preserva **verbatim** (la proyección lo refleja en `sequence_opaque` y el diálogo lo muestra sin editor).
- **No hay borrado por `PATCH`** (ausente = «no cambiar»), misma limitación MVP que `tzid` y los escalares de F25.
- Se emite **al final** del orden canónico (`…TRANSP`, `SEQUENCE`) para no reordenar nada: el contrato byte-a-byte de `build_ics` con `sequence: None` se mantiene.

## Las marcas de tiempo son inmutables por construcción

`CREATED`/`LAST-MODIFIED` se proyectan como instantes **UTC** (`Option<i64>` ms) con `parse_datetime_ms`; un valor no parseable → `None` (informativo), sin pérdida (la línea queda intacta igualmente). Como no entran en `EventFields`/`RECONCILE_ORDER`, un `PATCH` ajeno las deja **byte a byte**. Se **rechazó** autogestionar `LAST-MODIFIED` en el servidor (refrescarlo a «ahora»): rompería el invariante de dos líneas, haría el valor no reversible por el usuario y añadiría una semántica de *scheduling* iTIP que OxiDav no posee.

## La disciplina D2 (solo enviar lo que cambió)

- En **edición**, `sequence` solo viaja al `PATCH` si **cambió** respecto del baseline (`editDetail.sequence`) y no es `null`.
- Un baseline **opaco** (`sequence_opaque !== null`) **no se envía nunca**: el payload conserva su línea verbatim.
- Vaciar una secuencia modelable **no la envía** (no hay borrado por `PATCH` en el MVP).
- **En alta**, `sequence` solo se envía si el usuario lo rellenó.
- `created`/`last_modified` **nunca** viajan en el cuerpo de la petición (garantía de tipo: no existen en los *requests*).
- La **capa optimista** refleja lo enviado.

## Contratos por capa

- **`src/dav/ics/mod.rs`**: `EventFields` += `sequence` (`Option<i64>`); `RECONCILE_ORDER` 16→17 con `"SEQUENCE"` al final; `sequence_line`; `MAX_SEQUENCE` (constante canónica central, `pub(crate)`). `build_ics` emite `SEQUENCE` **solo** si `Some`; `reconcile_ics` añade la entrada **solo** si `Some`. `CREATED`/`LAST-MODIFIED` **jamás** en `final_values`.
- **`src/dav/ics/projection.rs`**: `EventProjection` += `sequence`/`sequence_opaque`/`created_ms`/`last_modified_ms` (lectura de líneas crudas de nivel `VEVENT`).
- **`src/api/dto.rs`**: `EventDetailDto` += `sequence`/`sequence_opaque`/`created`/`last_modified`; `CreateEventRequest`/`UpdateEventRequest` += `sequence`.
- **`src/api/events.rs`**: `validate_sequence` (rango cerrado → `400`), wiring de create y merge tres-estados del update.
- **`web/src/types/api.ts`**: `EventDetail` += los 4 campos; inputs += `sequence?`.
- **`web/src/pages/Calendar.tsx`**: precarga de `sequence` (salvo baseline opaco), bloque de **metadatos solo lectura** (`Creado`/`Modificado` con `formatDateTime`), campo `Secuencia` (`InputNumber min=0 max=2147483647 precision=0`) o texto verbatim si es opaco, y D2 en `handleSubmit`.

## Hallazgos de las auditorías, cerrados

**Backend (`@rust-reviewer`):**
1. 🟡 **Cobertura del `PUT` real** — el plan exigía verificar el detalle desde un `PUT` realista con las tres propiedades + `VALARM`/`ATTENDEE`/`X-`; se añadió el test end-to-end.
2. 🟢 **`SEQUENCE` no canónico** — caracterización: un `SEQUENCE:02` heredado se normaliza a `SEQUENCE:2` (comportamiento intencional, consistente con F25/F27).
3. 🟢 **`MAX_SEQUENCE` duplicada** — centralizada en una única definición `pub(crate)` (REFACTOR).

**Frontend (`@react-reviewer`):**
1. 🟡 **D2 con baseline opaco end-to-end** — se añadió el test que guarda y afirma que `sequence` **no** viaja.
2. 🟢 **Alta con/sin `sequence`** y **vaciado modelable** — tests añadidos.
3. 🟢 **Asimetría del optimista** — se retiró `sequence` del objeto optimista de alta (no se usa ni se pinta).

## Límites aceptados

- `CREATED`/`LAST-MODIFIED`: **solo lectura**; OxiDav nunca los reescribe (no autogestiona `LAST-MODIFIED`).
- `SEQUENCE` opaco (`x-custom`): se preserva pero **no** se edita.
- **Sin borrado por `PATCH`** de `SEQUENCE` (ausente = «no cambiar»), como `tzid`.
- **Normalización de forma:** un `SEQUENCE` modelable no canónico (`02`) se reescribe canónico (`2`) al heredarse en un `PATCH` (mismo instante/valor, otra forma textual).
- La lista del mes y las tarjetas no pintan las tres propiedades (regla F25/F11: solo en el diálogo).

## Verificación

- **Backend:** `cargo test --locked` → **681 pasando / 0 fallando**; `cargo fmt --all -- --check` limpio; `cargo clippy --all-targets -- -D warnings` limpio. Tests nuevos en `dav::ics` (emisión/identidad byte a byte, reconcile en sitio/inserción/opaco, marcas intactas), `dav::ics::projection` (modelable/opaco/ausente, marcas UTC) y `tests/api.rs` (validación `400`, escritura/herencia, opaco preservado, marcas verbatim, invariante de dos líneas, forma de la lista, `PUT`+`GET`).
- **Frontend:** `npm run test` → **340 pasando / 0 fallando** (23 ficheros; bloque `F28 — SEQUENCE / CREATED / LAST-MODIFIED`); `npm run typecheck` sin errores; `npm run lint` sin warnings; `npm run build` OK.
- **Auditorías:** `@rust-reviewer` → **APROBADO** (hallazgos de cobertura cerrados); `@react-reviewer` → **APROBADO** (hallazgos de cobertura cerrados).
