# PLAN-029 — F25: contenido y estado del `VEVENT` (`STATUS`, `CLASS`, `TRANSP`, `CATEGORIES`, `ATTACH`, `DURATION`)

**Estado: PROPUESTO — pendiente de aprobación del usuario. No se escribe código hasta que se apruebe.**

## Punto de partida (medido)

F24 (PLAN-028) dejó **9 de 26** propiedades utilizables y, sobre todo, dejó la maquinaria correcta: `raw_payload` es la fuente de verdad y `reconcile_ics` edita el `.ics` línea a línea conservando lo no modelado.

Las seis propiedades de esta fase **no existen en ninguna capa** (ni parser, ni proyección, ni DTO, ni formulario): `STATUS`, `CLASS`, `TRANSP`, `CATEGORIES`, `ATTACH`, `DURATION`. Hoy se **conservan** (gracias a F24) pero no se pueden ver ni editar.

## Objetivo

1. **O1 — Proyección del detalle.** Un `EventDetailDto` derivado del payload al leer, con las propiedades nuevas y las ya modeladas, expuesto por `GET /api/v1/collections/{id}/events/{uid}` (y devuelto también por `POST` y `PATCH`).
2. **O2 — Edición.** Las seis propiedades se pueden fijar y vaciar desde la SPA, con `reconcile_ics` como único camino de escritura del payload.
3. **O3 — La lista no parsea.** `EventDto` (la lista del mes) sigue alimentándose **solo de columnas**: no se añade ni una columna y no se parsea ningún `.ics` para listar.

## Decisión de arquitectura: proyección al leer, como los contactos

El repo ya resolvió este problema una vez, para el vCard completo (PLAN-023):

| | Contactos (ya hecho) | Eventos con este plan |
|---|---|---|
| Fuente de verdad | `raw_payload`; la escritura **nunca recompone** (`src/api/contacts/mod.rs:11`, `:365`) | igual desde F24 (`reconcile_ics`) |
| Propiedades «muchas» | **proyectadas al leer** (`project_card`, `src/dav/vcf/projection.rs:199`), **sin columnas** | `project_event`, igual |
| Detalle | `GET /contacts/{uid}` → `ContactDetailDto` construido desde la fila + la proyección (`src/api/contacts/mod.rs:278`, `dto.rs:508`) | `GET /events/{uid}` → `EventDetailDto`, igual |
| Diálogo de edición | **pide el detalle aparte** (`useContactDetail(editTarget)`, `web/src/pages/Contacts.tsx:198-212`) | `useEventDetail`, igual |

**Consecuencias, dichas por delante:** no hay migración ni columnas nuevas; el diálogo de edición gana **una petición** con estado de carga (como ya hace el de contactos); y la proyección es **falible**, así que un `raw_payload` corrupto da **500** con mensaje (exactamente lo que hace hoy el detalle de contactos), en vez de un 200 con datos inventados.

**Rechazado:** columna por propiedad (el esquema crecería y `CATEGORIES`/`ATTACH` son multi-valor, que en columnas se modelan mal) e híbrido (dos modelos mentales en la misma feature para el mismo tipo de dato).

## Reestructuración previa: `dav/ics.rs` → `dav/ics/`

`src/dav/ics.rs` tiene ya más de 2.200 líneas y esta fase le añadiría la proyección. Se convierte en directorio, **espejo exacto** de `src/dav/vcf/`:

```
src/dav/ics/mod.rs          # lo que hoy es ics.rs
src/dav/ics/projection.rs   # la proyección (nueva)
```

Es el mismo movimiento que hizo PLAN-025 con `dav/xml.rs` → `dav/xml/{mod,write,parse}.rs`, con su disciplina: **movimiento puro** (nada de reescrituras), suite completa verde **antes y después**, y comparación de la superficie `pub` de `dav::ics` para demostrar que no se pierde ni se añade nada.

## Decisiones de diseño

| Decisión | Por qué |
|---|---|
| **`DURATION` y `DTEND` son excluyentes** (RFC 5545) y lo hace cumplir **el servidor**, no solo la UI | Si el `PATCH` trae `duration`, el evento resultante lleva `DURATION` y **se elimina `DTEND`**; si trae `dtend`, lleva `DTEND` y **se elimina `DURATION`**; si trae **ambos**, `400` con mensaje explícito. Así la exclusión no depende de que un cliente se porte bien |
| En la SPA, **radio**: «Termina a las…» / «Dura…» | Es la traducción directa de la exclusión, y evita el estado imposible |
| **`dtend` NO se deriva nunca de `DURATION`** | Si el DTO devolviera un `dtend` calculado, la SPA lo reenviaría y convertiría el `DURATION` en `DTEND` **sin que el usuario lo tocara**: es exactamente el borrado silencioso que F24 elimina. La tarjeta calcula el fin **solo para pintar**, y ese valor no viaja nunca |
| `STATUS`/`CLASS`/`TRANSP` con **lista blanca** de los valores estándar, normalizados a mayúsculas; desconocido → `400` | Es la frontera de lo que entra por la API. `CLASS` admite `x-name`/`iana-token` en el RFC: un valor así **heredado** de un cliente DAV **no se revalida** (regla de F24) y se muestra tal cual |
| `CATEGORIES`: el DTO expone una lista **plana** y se emite **una sola línea** con las comas separando y **cada ítem escapado** | Una coma dentro de una categoría se escribe `\,` y sigue siendo expresable. Editar **colapsa** la estructura de ocurrencias (varias líneas `CATEGORIES`) a una sola: normalización deliberada, como la de propiedades duplicadas de F24 |
| `ATTACH`: el DTO expone **solo el URI** y se emite **una línea por valor** | Es la forma habitual en los `.ics` reales. Obliga a extender `reconcile_ics` para propiedades **multi-valor** (ver contratos). Los parámetros (`FMTTYPE`, `SIZE`) **se pierden al editar** la línea: límite aceptado y documentado |
| `ATTACH` y `URL` comparten **lista blanca `http`/`https`** | Los adjuntos se pintan como enlaces en la SPA: el razonamiento de la `URL` vale igual. Un adjunto `mailto:`/`ftp:` **heredado** se conserva (no se revalida) pero no se puede crear |
| Vaciar un campo `""` (o una lista `[]`) **elimina sus líneas** | Misma convención que F24 |
| `DURATION` es **texto opaco validado** (gramática de duración de RFC 5545), como `RRULE` | No se expande ni se convierte; se valida y se emite verbatim |

## Contratos

### `src/dav/ics/` (lectura, escritura y reconciliación)

- `EventFields` += `status`, `class_`, `transp`, `duration` (`Option<String>`) y `categories`, `attach` (`Vec<String>`).
- `build_ics` emite, tras las actuales: `CATEGORIES` (una línea, ítems escapados), `ATTACH` (una línea por valor, **verbatim**, sin escapar), y tras `RRULE`: `STATUS`, `CLASS`, `TRANSP`, `DURATION`. Con todo a `None`/vacío la salida sigue siendo **byte a byte idéntica** (contrato con test).
- `RECONCILE_ORDER` crece (hoy `[&str; 8]`) con las seis nuevas, en el orden canónico de emisión.
- `reconcile_ics` gana soporte **multi-valor** para `ATTACH`: la primera ocurrencia se sustituye por **el bloque completo de valores** (una línea cada uno) y **las demás ocurrencias se eliminan**; el resto del payload sigue intacto. `CATEGORIES` es multi-valor en el payload pero se reconcilia como una sola línea.
- La **proyección** (`src/dav/ics/projection.rs`) es un lector **nuevo y separado** de `parse_ics`: lee las **líneas crudas** (mismos helpers de despliegue/nombre/valor que ya tiene el módulo), no el modelo tipado, para conservar verbatim lo que el crate reformatearía. `ParsedEvent` **no cambia** (alimenta las columnas del `PUT`).

### Proyección

```rust
pub struct EventProjection {
    pub uid: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub url: Option<String>,
    pub categories: Vec<Vec<String>>, // una entrada por línea, espejo de `CardProjection::categories`
    pub attach: Vec<String>,          // solo URI, una entrada por ocurrencia
    pub status: Option<String>,
    pub class: Option<String>,
    pub transp: Option<String>,
    pub duration: Option<String>,
    pub dtstart_ms: i64,
    pub dtend_ms: Option<i64>,
    pub dtstart_tzid: Option<String>,
    pub dtend_tzid: Option<String>,
    pub rrule: Option<String>,
}

pub fn project_event(raw: &str) -> Result<EventProjection, IcsError>;
```

### API

- **`EventDetailDto`** (nuevo, superconjunto de `EventDto`): lo de `EventDto` más `categories: Vec<String>` (plano), `attach: Vec<String>`, `status`, `class_`, `transp`, `duration`. Se construye desde la fila y la proyección, como `ContactDetailDto::new`.
- `GET /events/{uid}`: devuelve `EventDetailDto` (**misma ruta**, superconjunto: aditivo). `POST` y `PATCH` también lo devuelven, para que la SPA reciba un objeto completo y coherente.
- `list` sigue devolviendo `EventDto` (columnas, sin parsear) — **la lista no cambia de forma**.
- `CreateEventRequest` y `UpdateEventRequest` += las seis; `is_empty()` las contempla.
- Validación nueva: `validate_status`, `validate_class`, `validate_transp` (listas blancas), `validate_duration` (gramática + cota), `validate_categories` (cotas por ítem y nº de ítems), `validate_attach` (URI `http`/`https`, sin control ni espacios: **se emite verbatim**, así que es frontera de seguridad contra la inyección de líneas).
- `update()`: aplica la exclusividad `DURATION`/`DTEND` **antes** del `reconcile`, y mantiene la regla de F24 (payload como fuente de verdad, solo se valida lo que entra por la API).
- `PUT` del DAV: **sin cambios** (verbatim + columnas actuales).

### Frontend

- `web/src/types/api.ts`: `EventDetail` (nuevo tipo), y las seis en `CreateEventInput`/`UpdateEventInput`.
- `web/src/hooks/useEventDetail.ts` (nuevo): espejo de `useContactDetail` (bajo demanda, `AbortController`, guarda de respuesta obsoleta, carga **derivada** de una clave de petición).
- `web/src/pages/Calendar.tsx`: el diálogo se rellena **desde el detalle** (con estado de carga) y gana:
  - **Estado**: `STATUS`, `CLASS`, `TRANSP` (`Select` con sus valores).
  - **Categorías**: `Select mode="tags"` (etiquetas).
  - **Adjuntos**: lista de URLs (`Form.List`, añadir/quitar) con la misma validación de esquema que la `URL`.
  - **Duración**: radio **«Termina a las…» / «Dura…»**; el segundo compone una duración (`PT30M`, `PT2H`, `P1D`) a partir de un número y una unidad.
  - El envío sigue la regla **D2** de F24 (solo se envía lo que el usuario cambia; en listas, comparación por valor). Con «Dura…» **no** se envía `dtend`; con «Termina a las…» **no** se envía `duration`.
- La tarjeta del evento muestra las **categorías** como etiquetas, los **adjuntos** como enlaces seguros (`rel="noopener noreferrer"`, solo `http(s)`) y el fin calculado cuando la exclusión deja fuera el `DTEND`.

## Escenarios (BDD)

**F1 — La proyección lee las seis.** *Dado* un `.ics` con `STATUS:CONFIRMED`, `CLASS:PRIVATE`, `TRANSP:TRANSPARENT`, `CATEGORIES:trabajo,personal`, dos `ATTACH` http y `DURATION:PT1H` *entonces* la proyección los devuelve, y las categorías una entrada por línea.

**F2 — Listas blancas.** *Dado* `PATCH {status: "MAYBE"}` *entonces* `400` y el payload no cambia. `confirmed` (minúsculas) se acepta y se normaliza a `CONFIRMED`.

**F3 — Gramática de `DURATION`.** *Dado* `PATCH {duration: "1 hora"}` *entonces* `400`; `PT1H`, `P1D` y `PT15M` se aceptan y se emiten verbatim.

**F4 — Exclusividad, en el servidor.** *Dado* un evento con `DTEND` *cuando* `PATCH {duration: "PT1H"}` *entonces* el payload lleva `DURATION` y **no** lleva `DTEND`, y el DTO devuelve `dtend: null`. Simétrico con `{dtend}` sobre un evento con `DURATION`. Y `PATCH {duration, dtend}` juntos → `400`.

**F5 — `dtend` no se inventa.** *Dado* un evento con `DURATION` y sin `DTEND` *entonces* el detalle devuelve `dtend: null` y `duration` con su valor: **no** hay un fin derivado en la respuesta. Un `PATCH` que solo cambia el título lo deja igual.

**F6 — `CATEGORIES`.** *Dado* `PATCH {categories: ["trabajo", "con, coma"]}` *entonces* en el payload hay **una** línea `CATEGORIES:trabajo,con\, coma` y al reproyectar vuelven los dos valores. `[]` borra la línea. Un `.ics` heredado con dos líneas `CATEGORIES` se colapsa a una al editar.

**F7 — `ATTACH`.** *Dado* `PATCH {attach: ["https://a.example/x", "https://b.example/y"]}` *entonces* el payload lleva **dos** líneas `ATTACH:…` **verbatim**, reproyectarlas da los dos valores y `[]` las borra. Un esquema no permitido (`javascript:`, `mailto:`) → `400` al crear; si **viene heredado** en el payload, se conserva y se reproyecta.

**F8 — La conservación de F24 sigue en pie.** *Dado* un `.ics` rico (con `VALARM`, dos `ATTENDEE`, `X-`, y ahora también las seis propiedades nuevas) *cuando* un `PATCH` cambia **solo** el título *entonces* difieren **exactamente dos líneas** (`SUMMARY` y `DTSTAMP`), con el mismo número de líneas.

**F9 — El detalle se proyecta.** *Dado* un `.ics` enviado por `PUT` al DAV *entonces* `GET /api/v1/collections/{id}/events/{uid}` devuelve las seis.

**F10 — Payload corrupto.** *Dada* una fila cuyo `raw_payload` no se puede proyectar *entonces* el detalle responde `500` con mensaje, **no** un `200` con datos inventados.

**F11 — La lista no cambia de forma.** *Dado* un listado de eventos *entonces* los elementos siguen trayendo **solo** los campos de `EventDto` (sin `categories`, sin `attach`, sin `status`): la lista no se ha tocado.

**F12 (frontend) — El diálogo pide el detalle.** *Dado* el diálogo en edición *entonces* se rellena con el detalle (incluidas las seis) y, mientras carga, lo indica; cambiar **solo** el título no envía ninguna de las seis (regla D2).

**F13 (frontend) — El radio de duración.** *Dado* el radio en «Dura…» con 1 hora *entonces* el `PATCH` lleva `duration` y **no** `dtend`; en «Termina a las…» lleva `dtend` y **no** `duration`.

**F14 (frontend) — La tarjeta.** *Dado* un evento con categorías, adjuntos y solo `DURATION` *entonces* la tarjeta pinta las etiquetas, los enlaces seguros y un fin calculado **localmente** (que no se envía nunca).

## Tareas (TDD)

1. **[REFACTOR]** `dav/ics.rs` → `dav/ics/{mod.rs,projection.rs}`. Movimiento puro; suite verde antes y después; comparar la superficie `pub`.
2. **[RED]** Proyección: tests de `project_event` (F1, y un caso de `CLASS:x-name` heredado).
3. **[GREEN]** `EventProjection` + `project_event` (lectura de líneas crudas).
4. **[RED]** Escritura: `build_ics` con las seis (F6, F7 y la identidad byte a byte con todo vacío) y `reconcile_ics` multi-valor (F6, F7, F8).
5. **[GREEN]** `EventFields`, helpers de línea, `RECONCILE_ORDER` y la reconciliación multi-valor.
6. **[RED]** Validación: F2, F3 y la exclusividad F4 (unitarios).
7. **[GREEN]** `validate_status`/`validate_class`/`validate_transp`/`validate_duration`/`validate_categories`/`validate_attach` y la exclusividad en `update()`.
8. **[RED]** Integración: F5, F9, F10, F11 en `tests/api.rs`.
9. **[GREEN]** `EventDetailDto`, `get_one`/`create`/`update` devolviéndolo, y los requests.
10. **[RED]** Frontend: F12, F13, F14 en `web/src/pages/Calendar.test.tsx` (+ el hook).
11. **[GREEN]** `useEventDetail`, tipos, formulario (estado, categorías, adjuntos, radio de duración) y tarjeta.
12. **[REFACTOR]** `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `npm run lint`.
13. **[CIERRE]** `docs/F25-vevent-contenido-y-estado.md` y el plan a `-COMPLETED.md` con `## Resultado`.

Commits previstos (un tipo por asunto): `♻️ refactor` (paso 1) → `✨ feat` (pasos 2-9) → `✨ feat` (pasos 10-11) → `📝 docs`.

## Riesgos

| Riesgo | Mitigación |
|---|---|
| El movimiento a `dav/ics/` cambia comportamiento | Suite verde antes y después + comparación de la superficie `pub` (disciplina de PLAN-025) |
| Romper la identidad byte a byte de `build_ics` | Las seis se emiten **solo** si están presentes; test de identidad con todo vacío |
| La reconciliación multi-valor duplica o pierde líneas de `ATTACH` | Tests con 2+ adjuntos, con adjuntos preexistentes y con `[]` (borrado) |
| Un `dtend` derivado convertiría `DURATION` en `DTEND` sin que el usuario lo toque | **No se deriva nunca**; test F5 |
| La proyección es falible y podría tumbar el detalle | Es el comportamiento **deseado** (mejor `500` que datos inventados), y es el que ya tiene el detalle de contactos; test F10 |
| El diálogo gana una petición y un estado de carga | Es el patrón ya establecido en contactos; test F12 |
| `EventDetailDto` cambia la forma de la respuesta de `GET`/`POST`/`PATCH` | Es un **superconjunto**: ningún campo existente cambia de nombre ni de tipo |
| Pérdida de `FMTTYPE`/`SIZE` al editar un `ATTACH` | Decidido y **documentado**, incluido en `docs/F25` |

## Verificación de cierre

- Backend: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` (línea base **594/0**; debe subir, nunca bajar).
- Frontend (desde `web/`): `npm run test` (línea base **20 ficheros / 229**), `npm run typecheck`, `npm run lint`, `npm run build`.
- `PUT` de un `.ics` **realista** con las seis propiedades + `VALARM` + `ATTENDEE` + `X-`: el detalle las proyecta todas; un `PATCH` que solo cambia el título altera **dos** líneas.
- Exclusividad, medida: `PATCH {duration}` deja el payload con `DURATION` y sin `DTEND`; `PATCH {duration, dtend}` → `400`.
- Prueba negativa: `attach: ["javascript:alert(1)"]` → `400` y payload intacto.

## Versionado

`feat` → **minor**: 0.6.0 → **0.7.0**.

## Resultado

**Estado: COMPLETADO.** Rama `feature/f25-vevent-contenido-estado`. Tipo `feat` → **minor (0.7.0)** cuando se libere.

### Rojo → verde, medido

| Momento | Backend | Frontend |
|---|---|---|
| Antes de empezar | **594 / 0** | 20 ficheros / **229** |
| Tras la proyección (paso 2-3) | 601 / 0 (RED real: 7 tests fallando **en ejecución**, con stub, no por compilación) | — |
| Tras la escritura (paso 4-5) | 608 / 0 (RED demostrado revirtiendo solo la implementación: 6 de 7 fallan por comportamiento; el séptimo es el guardián del contrato de bytes) | — |
| Tras la API (paso 9) | 620 / 0 | — |
| Tras el frontend (paso 10-11) | — | 236 / 0 |
| **Cierre** | **623 / 0** | 20 ficheros / **236** |

### Lo que se cumplió

- Las **seis** propiedades se leen (proyección), se exponen (`EventDetailDto`), se editan (diálogo) y se escriben (`build_ics` + `reconcile_ics`).
- El **invariante de F24 se mantiene**, ahora con las seis en juego: un `.ics` rico editado con un `PATCH` que solo cambia el título altera **exactamente dos líneas**.
- El **contrato de identidad byte a byte** de `build_ics` con las seis vacías sigue fijado por test.
- La **lista no cambia de forma** (test que lo fija): sigue leyendo solo columnas.
- `dav/ics.rs` → `dav/ics/{mod,projection}.rs` fue un **rename puro**: 0 inserciones y 0 borrados, mismo `md5`, 31 items `pub` antes y después.
- Sin migración y sin columnas nuevas.

### Desviaciones y correcciones (dichas, no escondidas)

1. **El escenario F14 estaba mal planteado y se retira.** El plan prometía que **la tarjeta** del calendario pintaría las categorías, los adjuntos y un fin calculado cuando el evento solo tiene `DURATION`. Eso es **incompatible con F11**, que este mismo plan exige: la lista del mes devuelve el DTO ligero y **no** lleva las seis. Se implementó, y la auditoría lo cazó: esas cosas solo habrían aparecido en eventos creados o editados en la sesión y **habrían desaparecido al recargar el mes** — peor que no pintarlas — mientras el test lo enmascaraba simulando una lista irreal. Se retira el pintado y el test pasa a simular lo que la lista devuelve de verdad. **Las seis viven en el diálogo.** (Contradicción del plan, detectada por la auditoría, no por los tests.)
2. **Más commits de los previstos** (4 en el plan → 9 ejecutados): se verificó y commiteó cada paso por separado (plan, refactor, proyección, escritura, API, frontend, dos de cierre de auditoría y documentación). Sale una historia más fina que un único commit gigante; se documenta por transparencia.
3. **Un test de F24 se debilitó de forma temporal** y se restauró en el mismo plan. Al pasar `CATEGORIES` a modelada, un `PATCH` que no la menciona la borra **hasta que la API la hereda del payload** (tarea 9), así que el test bandera no podía sostener su promesa en ese punto intermedio: se ajustó dejando un **alambre trampa** (`assert!(!stored.contains("CATEGORIES:"))`) que fallaría en cuanto la herencia funcionase, y en la tarea 9 se **restauró y reforzó** (ahora exige conservar `CATEGORIES`, `ATTACH` ×2, `STATUS`, `CLASS` y `TRANSP` con dos líneas de diferencia). El trampa desapareció, verificado.
4. **`reconcile_ics` multi-valor: `ATTACH` reemplaza la lista entera** (no añade). Es la semántica de un `PATCH` y está documentado; conviene saberlo al consumir la API.

### Hallazgos de la auditoría, cerrados

- **La tarjeta pintaba lo que la lista no lleva** (el punto 1 de arriba): retirado.
- **El detalle no se refrescaba al reabrir el mismo evento tras guardarlo**: la clave de petición ahora incluye el `etag`, que cambia en cada escritura.
- **`update` se tragaba el fallo de proyección sin log**: ahora deja un `tracing::warn!` con el `uid` y el error (el flujo no cambia).
- **Faltaba cobertura de API** para F6/F7/F4 (solo había unitarios): añadidos el escapado de la coma y el borrado con `[]` en `CATEGORIES`, los dos `ATTACH` verbatim y su borrado, el rechazo de `javascript:` en `POST` y `PATCH`, y la exclusividad en `POST`.
- Sin hallazgos bloqueantes en backend (`reconcile_ics` multi-valor, exclusividad, validaciones y la proyección resistieron el repaso).

### Incidentes de proceso (y qué los cazó)

- **Dos subagentes terminaron sin informe** y uno dejó el árbol en **rojo** (2 fallos: `getEvent` sin simular en dos tests de F24). La verificación **por CLI**, no el resumen del subagente, los detectó y permitió devolver el trabajo con el diagnóstico exacto en vez de rehacerlo.
- **Un subagente tocó un test que tenía prohibido tocar** (el bandera de F24), pero lo hizo de forma honesta: documentó la deuda y dejó el alambre trampa. Se aceptó como estado temporal con la restauración como requisito explícito del paso siguiente.
- **Un subagente agotó su límite de pasos** dejando `clippy` y `fmt` sin ejecutar (el árbol no habría pasado la CI). Detectado y corregido antes de commitear.

### Ficheros

- `src/dav/ics/mod.rs` (antes `src/dav/ics.rs`), `src/dav/ics/projection.rs` (nuevo), `src/dav/vcf/mod.rs` (una referencia en prosa).
- `src/api/dto.rs`, `src/api/events.rs`.
- `web/src/types/api.ts`, `web/src/services/events.ts`, `web/src/hooks/useEventDetail.ts` (nuevo), `web/src/pages/Calendar.tsx`, `web/src/pages/Calendar.test.tsx`.
- Tests: `tests/api.rs`, y los `#[cfg(test)]` de `src/dav/ics/{mod,projection}.rs`.
- `docs/F25-vevent-contenido-y-estado.md`.

### Fuera de alcance (sin tocar)

`RECURRENCE-ID`/instancias desprendidas, `ORGANIZER`/`ATTENDEE`, `EXDATE`/`RDATE`, `SEQUENCE`/`CREATED`/`LAST-MODIFIED`, `VALARM` en el formulario, envío de invitaciones, `TZID`/`RRULE` en la interfaz, indexar las nuevas propiedades en FTS y el *backfill* de `location`/`url`. **Todo lo no modelado se conserva.**

### Versionado

`feat` → **minor**: 0.6.0 → **0.7.0**. **No se ha liberado**: el *bump*, el tag y la imagen son un paso aparte.
