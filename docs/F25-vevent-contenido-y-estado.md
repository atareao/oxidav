# F25 — Contenido y estado del `VEVENT`: `STATUS`, `CLASS`, `TRANSP`, `CATEGORIES`, `ATTACH` y `DURATION`

Plan: `plans/PLAN-029-vevent-contenido-y-estado.md` (cerrado como `-COMPLETED`).

## Qué entrega

Las seis propiedades del «contenido y estado» de un `VEVENT` dejan de ser invisibles: se **leen** (proyección del detalle), se **editan** desde el diálogo y se **escriben** en el `.ics` conservando todo lo demás.

- **Lectura**: `project_event` (`src/dav/ics/projection.rs`) proyecta el `.ics` y un **`EventDetailDto`** nuevo —superconjunto de `EventDto`— lo expone por `GET /api/v1/collections/{id}/events/{uid}`. `POST` y `PATCH` también devuelven ese detalle.
- **Escritura**: `build_ics` y `reconcile_ics` modelan las seis, con la misma disciplina que F24 —el payload se edita **línea a línea** y lo no modelado se conserva byte a byte— ahora con soporte **multi-valor**.
- **Edición**: el diálogo se rellena **desde el detalle** (nuevo `useEventDetail`) y gana Estado, Visibilidad, Transparencia, Categorías (etiquetas), Adjuntos (lista de URLs) y Duración con radio «Termina a las…» / «Dura…».
- La **lista del mes no cambia**: sigue leyendo solo columnas, sin proyectar ni parsear ningún `.ics`, y no expone ninguna de las seis.

## La arquitectura: proyección al leer (y por qué)

El repo ya había resuelto «muchas propiedades» una vez, para el vCard completo (PLAN-023): `raw_payload` es la **única fuente de verdad**, las propiedades no viven en columnas y el **detalle se proyecta al leer**. F25 hace lo mismo para los eventos, que es exactamente el patrón que el detalle de contactos usa:

| | Contactos | Eventos con F25 |
|---|---|---|
| Fuente de verdad | `raw_payload`; la escritura nunca recompone | igual (desde F24) |
| Propiedades «muchas» | `project_card`, sin columnas | `project_event`, sin columnas |
| Detalle | `GET one` → `ContactDetailDto` | `GET one` → `EventDetailDto` |
| Diálogo de edición | pide el detalle aparte | `useEventDetail`, igual |

**Consecuencias, dichas por delante:** no hay migración ni columnas nuevas; el diálogo de edición gana **una petición** (con estado de carga, como el de contactos); y la proyección es **falible**, así que un `raw_payload` que no se puede proyectar da **500** con motivo, en vez de un `200` con datos inventados.

## Decisiones de diseño

| Decisión | Por qué |
|---|---|
| **`DURATION` y `DTEND` son excluyentes y lo hace cumplir el servidor** | Fijar una elimina la otra en el payload; traer las dos en la misma petición es `400`. La exclusión no depende de que el cliente se porte bien |
| Radio en la SPA: «Termina a las…» / «Dura…» | Es la traducción directa de la exclusión; evita el estado imposible |
| **`dtend` no se deriva nunca de `DURATION`** | Si el DTO devolviera un fin calculado, la SPA lo reenviaría y convertiría el `DURATION` en `DTEND` sin que el usuario lo toque: el borrado silencioso que F24 elimina |
| `STATUS`/`CLASS`/`TRANSP` con **lista blanca**, normalizados a mayúsculas; desconocido → `400` | Es la frontera de lo que entra por la API. Un valor raro **heredado** de un cliente DAV no se revalida y se muestra tal cual (regla de F24) |
| `CATEGORIES`: el DTO da una lista **plana**; se emite **una** línea con las comas separando y **cada ítem escapado** | Una coma dentro de una categoría se escribe `\,` y sigue siendo expresable. Editar colapsa varias líneas en una: normalización deliberada |
| `ATTACH`: **solo el URI**, y **una línea por valor** | Es la forma habitual en los `.ics` reales. Obliga a que `reconcile_ics` aprenda **multi-valor**. Los parámetros (`FMTTYPE`, `SIZE`) se pierden al editar la línea: límite aceptado |
| `ATTACH` comparte la lista blanca `http`/`https` con `URL` | Los adjuntos se pintan como enlaces: el razonamiento de la `URL` vale igual |
| Vaciar (`""` o `[]`) **elimina las líneas** | Misma convención que F24 |
| `DURATION` es **texto opaco validado** con la gramática de RFC 5545 | No se expande ni se convierte: se valida y se emite verbatim |

### `reconcile_ics` con multi-valor

La primera ocurrencia de una propiedad modelada se sustituye por **el bloque completo de líneas** y **las demás se eliminan**. Así `ATTACH` con tres ocurrencias y dos valores queda con dos, y `ATTACH` con `[]` desaparece. `CATEGORIES`, que puede venir en varias líneas, colapsa a **una sola**.

## Hallazgos de la auditoría, cerrados

1. **La tarjeta pintaba lo que la lista no lleva.** Pintaba categorías, adjuntos y un fin calculado, pero la lista del mes devuelve el DTO ligero, que por diseño **no** incluye las seis (hay un test que lo fija). En la realidad habrían aparecido solo en eventos creados o editados en la sesión y **habrían desaparecido al recargar el mes**, que es peor que no pintarlas — y el test lo enmascaraba simulando una lista irreal. Se retira el pintado (y el helper que quedó huérfano), conservando intacto lo de F24 (ubicación como texto y el enlace de `url`, con la comprobación de esquema y `rel="noopener noreferrer"`).
2. **El detalle no se refrescaba al reabrir el mismo evento.** La clave de la petición no cambiaba entre dos aperturas del mismo evento, así que tras guardar el diálogo mostraba el detalle anterior. Ahora la clave incluye el **`etag`**, que cambia en cada escritura.
3. **`update` se tragaba en silencio el fallo de proyección.** Ahora deja un `tracing::warn!` con el `uid` y el error: en una fila degenerada las seis se pierden y eso debe verse. El flujo no cambia.
4. **Faltaba cobertura de API** (solo había tests unitarios): el escapado de la coma en `CATEGORIES` y su borrado con `[]`, los dos `ATTACH` verbatim en orden y su borrado, el rechazo de `javascript:` en `POST` y en `PATCH`, y la exclusividad `DURATION`/`DTEND` en `POST`. Añadidos.
5. **Un test de F24 se debilitó temporalmente durante el desarrollo** —al pasar `CATEGORIES` a modelado, un `PATCH` que no la menciona la borra hasta que la API la hereda del payload— y se **restauró y reforzó** en el mismo plan: el test bandera vuelve a exigir que un `PATCH` que solo cambia el título conserve `CATEGORIES`, `ATTACH` (×2), `STATUS`, `CLASS` y `TRANSP` con **exactamente dos** líneas de diferencia.

## Verificación

- Backend: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings` y `cargo test --locked` → **623 pasando / 0 fallando** (antes: 594).
- Frontend (desde `web/`): `npm run test` → **20 ficheros / 236 tests** (antes: 229), `npm run typecheck`, `npm run lint` y `npm run build` limpios.
- **El invariante de F24 sigue en pie**, medido de extremo a extremo: un `.ics` rico (con `VALARM` y su propio `SUMMARY`, dos `ATTENDEE`, `X-`, y ahora también `CATEGORIES`, `ATTACH`, `STATUS`, `CLASS` y `TRANSP`) editado con un `PATCH` que **solo** cambia el título altera **exactamente dos líneas** (`SUMMARY` y `DTSTAMP`).
- Prueba negativa: `attach: ["javascript:alert(1)"]` → `400` y payload intacto, tanto en `POST` como en `PATCH`.
- Un `raw_payload` no proyectable → **500** con código `internal`, no un `200` con datos inventados.
- La lista del mes **no** expone ninguna de las seis (test que lo fija), así que sigue sin proyectar ni parsear.

## Deuda que queda (dicha, no escondida)

- **`ATTACH` pierde sus parámetros al editar** (`FMTTYPE`, `SIZE`): la línea se regenera. Aceptado al decidir que solo se modela el URI.
- **Un `ATTACH` heredado con esquema no permitido** (`mailto:`, `ftp://`) que llega de un cliente DAV se conserva, pero si el usuario **modifica la lista** el servidor la revalida entera y responde `400` (el error es explícito, y editar cualquier otra cosa del evento funciona: la lista no se reenvía si no cambia).
- **Un `DURATION` que no se descompone en un solo número+unidad** (`P1DT2H`) se muestra verbatim y no se reescribe; si el usuario pasa a «Termina a las…», ve un fin por defecto que no corresponde al `DURATION` (**visible**, no silencioso).
- **Un `.ics` que ya tuviera `DTEND` y `DURATION` a la vez** (imposible de crear por la API, pero posible vía `PUT` verbatim de un cliente) se conserva con los dos: no se toca lo que el `PATCH` no menciona.
- `RECURRENCE-ID` e instancias desprendidas siguen fuera: exigen relajar el índice único.
- `ORGANIZER`/`ATTENDEE`, `EXDATE`/`RDATE`, `SEQUENCE`/`CREATED`/`LAST-MODIFIED` y `VALARM` en el formulario: no modelados. Lo no modelado **se conserva**.
- `TZID` y `RRULE` siguen sin estar en la interfaz: el DTO los soporta, el formulario no los envía.
- Las columnas `location`/`url` de las filas anteriores a `0.6.0` siguen sin *backfill* (F24).
