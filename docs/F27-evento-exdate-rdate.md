# F27 — `EXDATE` y `RDATE` en el diálogo de eventos

Plan: `plans/PLAN-031-evento-exdate-rdate.md` (cerrado como `-COMPLETED`).

## Qué entrega

Dos campos editables nuevos en el diálogo de alta/edición de eventos de la SPA: **`EXDATE`** (exclusiones de una serie recurrente) y **`RDATE`** (fechas extra añadidas a la serie). Antes eran **invisibles e ineditables**: un `.ics` importado con `EXDATE`/`RDATE` **se preservaba** (el `PUT` de DAV guarda el payload verbatim y `reconcile_ics` no toca lo no modelado), pero la SPA no los mostraba ni permitía editarlos.

A diferencia de F26 (solo frontend), F27 abarca **backend y frontend**: proyección, reconciliación, DTO, API REST y diálogo. **Sin migración ni columnas nuevas.**

## Punto de partida

`EXDATE`/`RDATE` no existían en ninguna capa (`grep` → 0). La maquinaria correcta ya estaba montada por F24/F25: `raw_payload` es la única fuente de verdad; `reconcile_ics` edita el `.ics` línea a línea conservando lo no modelado; y el patrón «proyección al leer, sin columnas» ya se usaba para las seis propiedades de F25 (`CATEGORIES`, `ATTACH`, …).

## Arquitectura: proyección al leer, sin columnas

| Aspecto | Decisión |
|---|---|
| Fuente de verdad | `raw_payload`; la escritura nunca recompone (F24) |
| Almacenamiento | **sin columnas** (como `CATEGORIES`/`ATTACH`): son multi-valor y no se filtran |
| Lectura | `project_event` lee las **líneas crudas** del primer `VEVENT`; deriva los instantes (ms) |
| Escritura | `reconcile_ics` reconcilia en su sitio; `build_ics` los emite en alta |

## Subconjunto soportado y reglas opacas

Se modela **solo** el valor `DATE-TIME` (con `TZID` opcional). Todo lo demás es **opaco** y se **preserva verbatim** (no se destruye):

- `VALUE=DATE` (todo el día) y `VALUE=PERIOD` (`RDATE` con `start/end`).
- Cualquier token de fecha-hora no parseable.
- **Regla de conjunto:** si **alguna** línea de la propiedad no encaja, **toda** la propiedad es opaca (no se modela a medias).

Una propiedad opaca se muestra **verbatim** en el diálogo (patrón `opaqueRrule` de F26), sin lista editable, y solo ofrece un `Checkbox` «Quitar» que la limpia (`[]`). No se reescribe salvo esa acción explícita.

## Emisión siempre en UTC (decisión de la auditoría)

`EXDATE`/`RDATE` se emiten **siempre** como `DATE-TIME` **UTC** (`EXDATE:<v1>Z,<v2>Z,…`), **nunca** `;TZID=`, en **una sola** línea con los instantes ordenados y sin duplicados.

Motivo: emitir en la zona local de `DTSTART` es *lossy* en la **hora ambigua** del cambio de hora de otoño — la segunda pasada de la hora repetida se formatea a hora local y al re-parsear se resuelve a la **primera** pasada, moviendo el instante **−1 h en silencio** (corrupción de datos). Un valor UTC hace el round-trip **exacto** y es RFC 5545 válido aunque `DTSTART` lleve `TZID`. Al leer, se sigue resolviendo el `TZID` de cada línea para obtener el instante.

## La disciplina D2 (solo enviar lo que cambió)

- En **edición**, `exdate`/`rdate` solo viajan al `PATCH` si **cambian** respecto del baseline, comparando por **conjunto de instantes** (`sameInstantSet`, independiente del orden y de duplicados).
- Un baseline **opaco** que no se toca **no se envía** (clave omitida): el payload conserva su línea verbatim. El `Checkbox` «Quitar» envía `[]`.
- Vaciar entradas modelables envía el conjunto resultante (o `[]` si queda vacío).
- **En alta**, las fechas solo se envían si se definió una recurrencia (`rule !== null`); si el usuario limpia «Repetición», el bloque se oculta y las fechas acumuladas **no** viajan.
- La **capa optimista** refleja exactamente lo enviado.

## Contratos por capa

- **`src/dav/ics/mod.rs`**: `EventFields` += `exdate`/`rdate` (`Option<Vec<i64>>`, **tres estados**: `None` = opaco/no tocar, `Some(vec![])` = limpiar, `Some(v)` = fijar). `RECONCILE_ORDER` 14→16 (`RDATE`,`EXDATE` tras `RRULE`). `recur_date_line` (UTC, orden+dedup). `property_param` (lectura de parámetros del *head*).
- **`src/dav/ics/projection.rs`**: `EventProjection` += `exdate`/`exdate_opaque`/`rdate`/`rdate_opaque`; `project_recur_dates`.
- **`src/api/dto.rs`**: 4 campos en `EventDetailDto`; `CreateEventRequest`/`UpdateEventRequest` += `exdate`/`rdate`.
- **`src/api/events.rs`**: `MAX_RECUR_DATES = 64`; `validate_instant_list` (rango años 1..=9999 vía `check_instant`, orden+dedup, rechazo *fail-fast* por longitud); wiring de create y regla de merge del update (heredar/opaco/limpiar).
- **`web/src/types/api.ts`**: `EventDetail` += los 4 campos; inputs += `exdate?`/`rdate?`.
- **`web/src/utils/recurDates.ts`** (NUEVO): `sortUniqueInstants`, `sameInstantSet`, `describeRecurDates`.
- **`web/src/pages/Calendar.tsx`**: campos del formulario, D2 en `handleSubmit`, dos `Form.List` («Fechas excluidas»/«Fechas extra») con `DatePicker showTime`, y bloque opaco verbatim + «Quitar».

## Hallazgos de las auditorías, cerrados

**Backend (`@rust-reviewer`):**
1. **M-2** — la emisión en zona de `DTSTART` corrompía el instante en la hora ambigua del fall-back → **emisión siempre en UTC**, con test de round-trip exacto en esa hora.
2. **m-3** — test byte a byte real (`assert_eq!`) de `reconcile` con `EXDATE` opaco.
3. **m-4** — cobertura de `RDATE` y de RDATE+EXDATE combinados (orden canónico).
4. **m-5** — rechazo *fail-fast* por longitud de entrada en `validate_instant_list`.
5. **m-6** — tests REST de `400` (rango/tope) y de «limpiar opaca» end-to-end.
6. **n-7** — proyección: dedup entre líneas y `RDATE` multilínea.

**Frontend (`@react-reviewer`):**
1. **Cobertura** — RDATE opaca + `clearRdate` + RDATE `[]` + dedup del formulario.
2. **Valores ocultos** — el intento con `preserve={false}` en `Form.List` rompía el typecheck (antd v6 no acepta ese prop); se resolvió condicionando el envío a `rule !== null` en alta.
3. **Nit** — comentario de test obsoleto corregido.

## Límites aceptados

- `VALUE=DATE` / `VALUE=PERIOD`: **opacos**, se preservan pero no se editan.
- **Normalización a UTC:** un `EXDATE`/`RDATE` local con `TZID` se resuelve a su instante y se reemite en UTC (mismo instante, otra representación); **deliberado** para garantizar round-trip exacto.
- **Multi-valor → una línea:** editar colapsa varias líneas `EXDATE`/`RDATE` en una sola (normalización, como `CATEGORIES` en F25).
- La lista del mes y el detalle no muestran indicadores de exclusión/fechas extra (decisión: solo en el diálogo).

## Verificación

- **Backend:** `cargo test --locked` → **658 pasando / 0 fallando**; `cargo fmt --check` limpio; `cargo clippy --all-targets -- -D warnings` limpio. Tests nuevos en `dav::ics` (emisión UTC, round-trip en el fall-back, byte-a-byte, RDATE/orden), `dav::ics::projection` (opacidad, dedup entre líneas, multilínea) y `tests/api.rs` (create/patch/opaco/400).
- **Frontend:** `npm run test` → **331 pasando / 0 fallando** (23 ficheros; 13 tests del bloque `F27 — EXDATE/RDATE`); `npm run typecheck` sin errores; `npm run lint` sin warnings.
- **Auditorías:** `@rust-reviewer` → **APROBADO CON CAMBIOS** (cerrados); `@react-reviewer` → **APROBADO**.
