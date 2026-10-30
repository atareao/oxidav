# PLAN-032 — F28: `SEQUENCE` editable y `CREATED`/`LAST-MODIFIED` de solo lectura

**Estado: PROPUESTO — pendiente de aprobación del usuario. No se escribe código hasta que se apruebe.**

## Punto de partida (medido)

F27 (PLAN-031) dejó el diálogo de eventos con `TZID`, `RRULE`, `EXDATE` y `RDATE` editables y, sobre todo, consolidó la maquinaria correcta: `raw_payload` es la fuente de verdad, `reconcile_ics` edita el `.ics` línea a línea conservando lo no modelado, y el detalle se **proyecta al leer** (`project_event`) sin columnas ni migración (patrón de F25/F27).

Cuenta de partida (medida en `a36c8fd`):

- Backend: `cargo test --locked` → **658 / 0**.
- Frontend: `npm run test` → **331 / 0** (23 ficheros).
- Versión: **0.7.0**. Rama `development` == `origin/development`.

Las tres propiedades de esta fase **no existen en ninguna capa** (ni proyección, ni DTO, ni formulario): `SEQUENCE`, `CREATED`, `LAST-MODIFIED`. Hoy se **conservan** (gracias a F24, porque `reconcile_ics` no las modela y no toca sus líneas) pero no se pueden ver ni editar.

**Aviso de nomenclatura (colisión evitada):** el DTO ya expone `created_at`/`updated_at`, que son las **columnas de la fila** (cuándo se insertó/actualizó el registro en SQLite), **no** las propiedades ICS. Las de F28 son propiedades del `.ics` y se exponen como **`created`** y **`last_modified`** (ms Unix) para no confundirlas. Un `.ics` puede llevar `CREATED` mucho antes que el `created_at` de la fila, y un `PUT` de un cliente DAV puede cambiar `LAST-MODIFIED` sin que la fila se reinserte.

## Objetivo

1. **O1 — Proyección del detalle.** `project_event` lee `SEQUENCE`, `CREATED` y `LAST-MODIFIED` del primer `VEVENT` de nivel `VCALENDAR`, y `EventDetailDto` las expone (por `GET`/`POST`/`PATCH`).
2. **O2 — Edición de `SEQUENCE` únicamente.** `SEQUENCE` se puede fijar desde la SPA (y por `POST`/`PATCH`), con `reconcile_ics` como **único** camino de escritura. `CREATED`/`LAST-MODIFIED` son **de solo lectura** (se muestran, nunca se reescriben): al no entrar en `EventFields` ni en `RECONCILE_ORDER`, `reconcile_ics` deja sus líneas intactas **sin coste**.
3. **O3 — Conservación y lista intactas.** El invariante de F24 (un `.ics` rico editado con un `PATCH` que solo cambia el título altera **exactamente dos líneas**) sigue en pie, ahora con las tres en juego. `EventDto` (la lista del mes) **no cambia de forma**: sigue alimentándose solo de columnas, sin parsear.

## Decisión de arquitectura: proyección al leer, sin columnas, sin migración

Es la misma decisión que F25/F27 (y antes el vCard completo, PLAN-023):

| | Ya hecho (F25/F27) | Este plan |
|---|---|---|
| Fuente de verdad | `raw_payload`; la escritura **nunca recompone** | igual (`reconcile_ics`) |
| Propiedades «muchas» | **proyectadas al leer**, sin columnas | `SEQUENCE`/`CREATED`/`LAST-MODIFIED`, igual |
| Detalle | `GET /events/{uid}` → `EventDetailDto` desde la fila + proyección | aditivo: tres campos más |
| Escritura | solo lo modelado en `RECONCILE_ORDER` | solo `SEQUENCE` se une a esa lista |

**Novedad de esta fase — el eje lectura/escritura se separa por primera vez.** F25 y F27 modelaron propiedades **editables**; aquí `CREATED`/`LAST-MODIFIED` son **solo lectura**. Esto es *más barato*, no más caro: una propiedad que no está en `RECONCILE_ORDER` ni en `EventFields` es, por construcción, preservada verbatim (F24) y no necesita emisión, validación, herencia en `update()` ni disciplina D2. La única propiedad con camino de escritura es `SEQUENCE`.

**Rechazado:** autogestionar `LAST-MODIFIED` en el servidor (refrescarlo a «ahora» en cada escritura) — cambiaría el invariante de dos líneas, haría el valor no reversible por el usuario y añadiría una semántica de propiedad que OxiDav no tiene por qué poseer (no hace *scheduling* iTIP). Y modelar `CREATED`/`LAST-MODIFIED` como columnas → migración innecesaria y riesgo de divergir del payload.

## Decisiones de diseño

| Decisión | Por qué |
|---|---|
| **`SEQUENCE` es un entero no negativo**, `0 ≤ n ≤ 2147483647` (RFC 5545 §3.8.7.4), en `range` **cerrado** sobre el valor | Es la frontera de lo que entra por la API. Fuera de rango → `400`. Se emite verbatim como `SEQUENCE:<n>` |
| **`SEQUENCE` con tres estados + opaco**, como `EXDATE`/`RDATE` | Un `SEQUENCE:abc` heredado de un cliente no es modelable: se preserva (**opaco**) y no se reescribe; el DTO lo pinta pero no lo edita. Un `SEQUENCE` modelable se hereda en `PATCH`. `None` en `EventFields` = «no tocar» |
| **No hay «borrar `SEQUENCE`» por `PATCH`** | Ausente = «no cambiar» (misma limitación MVP que `tzid` y los escalares de F25, documentada). Fijarlo es lo que la UI necesita |
| **`CREATED`/`LAST-MODIFIED` NO entran en `EventFields` ni en `RECONCILE_ORDER`** | Es lo que las hace de solo lectura *y* preservadas: `reconcile_ics` no emite ni elimina sus líneas, así que un `PATCH` ajeno las deja byte a byte. Cero maquinaria |
| **`CREATED`/`LAST-MODIFIED` se proyectan como instantes UTC (`Option<i64>` ms)**, con `parse_datetime_ms` (suffix-less = UTC) | RFC 5545 §3.8.7.1/§3.8.7.3: ambas **deben** ir en UTC. Un valor no parseable → `None` (informativo), sin pérdida: la línea queda intacta igualmente |
| **El DTO expone las de ICS como `created`/`last_modified`** (y `sequence`/`sequence_opaque`) | Evita la colisión con `created_at`/`updated_at` (columnas). Aditivo: ningún campo existente cambia de nombre ni tipo |
| **`SEQUENCE` se emite en el orden canónico al final** (`…TRANSP`, `SEQUENCE`) | Mínimo *churn*: no reordena ninguna propiedad existente, así que el contrato byte-a-byte de `build_ics` con `sequence: None` se mantiene |
| **La tarjeta/lista NO pinta las tres** | Regla de F25 (F11): la lista devuelve el DTO ligero y no lleva estas propiedades. Mostrarlas en la lista sería peor que no mostrarlas (desaparecerían al recargar) |

## Contratos

### `src/dav/ics/mod.rs` (escritura de `SEQUENCE`)

- `EventFields` += `sequence: Option<i64>` (`None` = no tocar las líneas `SEQUENCE` del payload; `Some(n)` = fijar/emitir).
- `RECONCILE_ORDER` crece de 16 a **17**: se añade `"SEQUENCE"` **al final** (tras `TRANSP`).
- Nueva `fn sequence_line(n: i64) -> String` → `SEQUENCE:<n>`.
- `build_ics`: emite `SEQUENCE` **solo** cuando `fields.sequence.is_some()` (con `None`, salida **byte a byte idéntica** — contrato con test). `CREATED`/`LAST-MODIFIED` no se emiten (no hay campo).
- `reconcile_ics`: `("SEQUENCE", …)` entra en `final_values` **solo** cuando `fields.sequence` es `Some` (con `None`, las líneas `SEQUENCE` existentes quedan intactas). `CREATED`/`LAST-MODIFIED` **nunca** aparecen en `final_values` → sus líneas jamás se tocan.
- `ParsedEvent` **no cambia** (alimenta las columnas; `SEQUENCE`/`CREATED`/`LAST-MODIFIED` no son columnas).

### `src/dav/ics/projection.rs` (lectura)

```rust
pub struct EventProjection {
    // … existentes …
    /// `SEQUENCE` como entero del subconjunto soportado. `None` también cuando
    /// la propiedad es opaca o ausente.
    pub sequence: Option<i64>,
    /// Texto **verbatim** de la línea `SEQUENCE` cuando **no** es un entero
    /// válido (informativo; su preservación la garantiza `reconcile_ics` al no
    /// tocarla). `None` si es modelable o ausente.
    pub sequence_opaque: Option<String>,
    /// `CREATED` como milisegundos Unix (UTC). `None` si ausente o no parseable.
    pub created_ms: Option<i64>,
    /// `LAST-MODIFIED` como milisegundos Unix (UTC). `None` si ausente o no parseable.
    pub last_modified_ms: Option<i64>,
}
```

Lectura por líneas crudas (`first_value`, nivel `VEVENT`), como los escalares de F25. `SEQUENCE`: `value.trim().parse::<i64>()` dentro de `0..=2147483647` → modelable; si no, `(sequence=None, sequence_opaque=Some(línea verbatim))`. `CREATED`/`LAST-MODIFIED`: `parse_datetime_ms(value)` → `Option<i64>`.

### API

- **`EventDetailDto`** += `sequence: Option<i64>`, `sequence_opaque: Option<String>`, `created: Option<i64>`, `last_modified: Option<i64>` (los cuatro aditivos).
- `GET`/`POST`/`PATCH /events…` devuelven el `EventDetailDto` enriquecido.
- `list` sigue devolviendo `EventDto` (columnas) — **sin cambios**.
- `CreateEventRequest` y `UpdateEventRequest` += `sequence: Option<i64>`; `is_empty()` lo contempla. **No** se añaden `created`/`last_modified` a los *requests* (solo lectura).
- Nueva validación `validate_sequence(raw: Option<i64>)` con `MAX_SEQUENCE = 2_147_483_647`.
- `update()`: merge uniforme (F25) — `Some(n)` del request → validar y fijar; `None` → heredar del payload si es modelable, o `None` (no tocar / preservar el opaco) si es opaco o ausente.
- `PUT` del DAV: **sin cambios** (verbatim + columnas actuales).

### Frontend

- `web/src/types/api.ts`: `EventDetail` += `sequence: number | null`, `sequence_opaque: string | null`, `created: number | null`, `last_modified: number | null`; `CreateEventInput`/`UpdateEventInput` += `sequence?: number`.
- `web/src/pages/Calendar.tsx`:
  - **Metadatos (solo lectura)**: se muestran `Creado` y `Modificado` con `formatDateTime` (`web/src/utils/date.ts`) cuando no son `null`.
  - **Secuencia**: `Form.Item name="sequence"` con `InputNumber` (`min=0`, `max=2147483647`, `precision=0`). Si el baseline es **opaco** (`sequence_opaque !== null`), se muestra el texto verbatim y no se edita.
  - **D2**: `sequence` solo se envía si **cambió** respecto al baseline del detalle. En alta, solo si el usuario lo rellenó.
  - `CREATED`/`LAST-MODIFIED` **nunca** viajan en el cuerpo de la petición.

## Escenarios (BDD)

**F1 — La proyección lee las tres.** *Dado* un `.ics` con `SEQUENCE:3`, `CREATED:20240101T000000Z` y `LAST-MODIFIED:20240201T120000Z` *entonces* la proyección devuelve `sequence=Some(3)`, y los dos instantes en ms UTC.

**F2 — `SEQUENCE` opaco.** *Dado* un `.ics` con `SEQUENCE:x-custom` *entonces* `sequence=None` y `sequence_opaque` con la línea verbatim; el detalle lo expone y **no** lo edita.

**F3 — Validación de `SEQUENCE`.** *Dado* `PATCH {sequence: -1}` → `400`; `{sequence: 2147483648}` → `400`; `{sequence: 0}` → `200` y `SEQUENCE:0` en el payload.

**F4 — Escritura de `SEQUENCE`.** *Dado* un evento con `SEQUENCE:1` *cuando* `PATCH {sequence: 5}` *entonces* el payload lleva `SEQUENCE:5` y el DTO `sequence=5`. Un `PATCH` que **no** menciona `sequence` lo deja como estaba.

**F5 — Opaco preservado.** *Dado* un `.ics` con `SEQUENCE:x-custom` *cuando* `PATCH {summary: "otro"}` *entonces* la línea `SEQUENCE:x-custom` sigue **verbatim** y `sequence_opaque` la refleja.

**F6 — `CREATED`/`LAST-MODIFIED` de solo lectura.** *Dado* un `.ics` con ambas *cuando* un `PATCH` cambia **solo** el título *entonces* sus dos líneas quedan **byte a byte** y el detalle devuelve los mismos instantes. `last_modified`/`created` no son campos del request: enviarlos es ignorado.

**F7 — El invariante de F24 sigue en pie.** *Dado* un `.ics` rico (con `VALARM`, `ATTENDEE`, `X-`, `SEQUENCE`, `CREATED`, `LAST-MODIFIED`) *cuando* un `PATCH` cambia **solo** el título *entonces* difieren **exactamente dos líneas** (`SUMMARY` y `DTSTAMP`), con el mismo número de líneas.

**F8 — La identidad byte a byte de `build_ics`.** *Dado* un `EventFields` con `sequence: None` *entonces* `build_ics` es idéntico al de antes de F28 (test guardián).

**F9 — La lista no cambia de forma.** *Dado* un listado de eventos *entonces* los elementos siguen trayendo **solo** los campos de `EventDto` (sin `sequence`, sin `created`, sin `last_modified`).

**F10 (frontend) — El diálogo.** *Dado* el diálogo en edición *entonces* muestra `Creado`/`Modificado` y un campo `Secuencia` editable; con baseline opaco muestra el texto y no lo edita.

**F11 (frontend) — D2.** *Dado* el diálogo abierto sobre un evento con `SEQUENCE:2` *cuando* se cambia **solo** el título *entonces* el `PATCH` **no** envía `sequence`; si se cambia a `4`, sí lo envía.

## Tareas (TDD)

1. **[RED]** Proyección: tests de `project_event` para `SEQUENCE` (modelable, opaco, ausente) y `CREATED`/`LAST-MODIFIED` (F1, F2).
2. **[GREEN]** `EventProjection` += los cuatro campos y su lectura por líneas crudas.
3. **[RED]** Escritura: `build_ics` con `SEQUENCE: Some` (y la identidad byte a byte con `None`, F8), `RECONCILE_ORDER` (17), y `reconcile_ics` que fija/no toca `SEQUENCE` y **jamás** toca `CREATED`/`LAST-MODIFIED` (F4, F6, F7).
4. **[GREEN]** `EventFields.sequence`, `sequence_line`, la entrada en `RECONCILE_ORDER` y la rama `Some` en `reconcile_ics`.
5. **[RED]** Validación: `validate_sequence` (F3, unitarios).
6. **[GREEN]** `MAX_SEQUENCE`, `validate_sequence` y el *wiring* de `create()`.
7. **[RED]** Integración: F3, F4, F5, F6, F9 en `tests/api.rs` (+ el detalle que expone `created`/`last_modified` de un `PUT`).
8. **[GREEN]** `EventDetailDto` += los cuatro; `get_one`/`create`/`update`; los requests; el merge en `update()`.
9. **[RED]** Frontend: F10, F11 en `web/src/pages/Calendar.test.tsx`.
10. **[GREEN]** tipos, campos del diálogo (metadatos + `Secuencia`), D2.
11. **[REFACTOR]** `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `npm run lint`.
12. **[CIERRE]** `docs/F28-vevent-secuencia-y-marcas.md` y el plan a `-COMPLETED.md` con `## Resultado`.

Commits previstos (un tipo por asunto): `✨ feat` (pasos 1-8) → `✨ feat` (pasos 9-10) → `📝 docs`.

## Riesgos

| Riesgo | Mitigación |
|---|---|
| Confundir `CREATED`/`LAST-MODIFIED` (ICS) con `created_at`/`updated_at` (columnas) | Nombres distintos en el DTO (`created`/`last_modified`) y test que fija que un `.ics` con `CREATED` muy anterior al `created_at` de fila los expone **distintos** |
| Romper la identidad byte a byte de `build_ics` | `SEQUENCE` se emite **solo** si `Some`; test de identidad con `None` (F8) |
| `SEQUENCE` opaco se pierde al editar | Tres estados + opaco (como `EXDATE`): `None` en `EventFields` = no tocar; test F5 |
| Un `PATCH` ajeno toca `CREATED`/`LAST-MODIFIED` | No están en `RECONCILE_ORDER` ni en `EventFields`: no hay código que las emita; test F6 |
| El invariante de dos líneas se rompe | `SEQUENCE` heredado (no mencionado) no se reescribe; test F7 |
| `EventDetailDto` cambia la forma de `GET`/`POST`/`PATCH` | Es **aditivo**: ningún campo existente cambia de nombre ni tipo |

## Verificación de cierre

- Backend: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` (línea base **658/0**; debe subir, nunca bajar).
- Frontend (desde `web/`): `npm run test` (línea base **23 ficheros / 331**), `npm run typecheck`, `npm run lint`, `npm run build`.
- `PUT` de un `.ics` **realista** con `SEQUENCE` + `CREATED` + `LAST-MODIFIED` + `VALARM` + `ATTENDEE` + `X-`: el detalle las proyecta todas; un `PATCH` que solo cambia el título altera **dos** líneas.
- Prueba negativa: `PATCH {sequence: -1}` → `400` y payload intacto.
- Prueba negativa: `PUT` con `SEQUENCE:x-custom` → `PATCH` del título → la línea sigue byte a byte.

## Versionado

`feat` → **minor**: 0.7.0 → **0.8.0**.

## Resultado

**Estado: CERRADO.** Plan ejecutado de principio a fin (TDD y auditorías incluidas). El plan de origen era `plans/PLAN-032-vevent-secuencia-y-marcas.md`.

### Rojo → verde, medido

- Backend: la suite pasó de **658 → 681** tests; `cargo fmt --all -- --check` y `cargo clippy --all-targets -- -D warnings` siempre limpios.
- Frontend: la suite pasó de **331 → 340** tests (23 ficheros); `npm run typecheck`, `npm run lint` y `npm run build` limpios.

### Lo que se cumplió

- **O1** — `project_event`/`EventDetailDto` exponen `sequence`/`sequence_opaque`/`created`/`last_modified`; `GET`/`POST`/`PATCH` los devuelven.
- **O2** — `SEQUENCE` editable desde la SPA (con `reconcile_ics` como único camino de escritura); `CREATED`/`LAST-MODIFIED` **solo lectura**, preservados verbatim por construcción.
- **O3** — el invariante de F24/F7 se sostiene (un `PATCH` de solo título altera **exactamente dos** líneas, `SUMMARY` y `DTSTAMP`, aun con las tres propiedades presentes); la lista del mes **no cambia de forma**; sin migración ni columnas.

### Escenarios cubiertos

F1–F9 (backend) y F10–F11 (frontend) con test real. El escenario **F7** se cubre además con un `.ics` rico que incluye `SEQUENCE`/`CREATED`/`LAST-MODIFIED`.

### Desviaciones y correcciones (dichas, no escondidas)

1. **REFACTOR `MAX_SEQUENCE`.** La constante quedó duplicada en `projection.rs` y `events.rs`; se centralizó en una única definición `pub(crate)` en `dav::ics` (hallazgo 🟢 de la auditoría).
2. **Herencia normaliza la forma.** Un `SEQUENCE` modelable no canónico (`SEQUENCE:02`) se reescribe canónico (`SEQUENCE:2`) al heredarse en un `PATCH`; comportamiento intencional (consistente con F25/F27), fijado con test de caracterización.

### Hallazgos de la auditoría, cerrados

- **Backend (`@rust-reviewer`):** 🟡 cobertura del detalle desde un `PUT` real (cerrado), 🟢 `SEQUENCE` no canónico (caracterización), 🟢 `MAX_SEQUENCE` duplicada (centralizada).
- **Frontend (`@react-reviewer`):** 🟡 D2 con baseline opaco end-to-end (cerrado), 🟢 alta con/sin `sequence` y vaciado modelable (tests añadidos), 🟢 asimetría del optimista (retirada).

### Ficheros

- **Backend:** `src/dav/ics/mod.rs`, `src/dav/ics/projection.rs`, `src/api/dto.rs`, `src/api/events.rs`, `tests/api.rs`.
- **Frontend:** `web/src/types/api.ts`, `web/src/pages/Calendar.tsx`, `web/src/pages/Calendar.test.tsx`.
- **Docs:** `docs/F28-vevent-secuencia-y-marcas.md`.

### Fuera de alcance (sin tocar)

Autogestionar `LAST-MODIFIED` en el servidor, borrado de `SEQUENCE` por `PATCH`, edición de `CREATED`/`LAST-MODIFIED`, indicadores en la lista, y **cualquier** migración/columna.

### Versionado

Feature nueva (`feat`) → bump **minor**; sin cambios de esquema. `web/package.json` no se versiona.
