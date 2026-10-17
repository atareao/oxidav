# PLAN-028 — F24: el `PATCH` no debe destruir el `.ics` + `LOCATION`/`URL`

**Estado: PROPUESTO — pendiente de aprobación del usuario. No se escribe código hasta que se apruebe.**

## El problema (medido, con fichero y línea)

El `PUT` del DAV guarda el payload entrante **verbatim** (`src/dav/caldav.rs:543` y `:563` → `raw_payload: raw.to_string()`). Pero el `PATCH` de la API REST lo **recompone desde cero**:

- `src/api/events.rs:333` → `raw_payload: build_ics(&fields)`
- `build_ics` (`src/dav/ics.rs:329`) emite **exactamente siete** propiedades: `UID`, `DTSTAMP`, `SUMMARY`, `DESCRIPTION`, `DTSTART`, `DTEND`, `RRULE`. Nada más, y en un orden fijo.

Consecuencia **hoy**, en producción y sin tocar nada:

> Un evento creado por Thunderbird, iOS o DAVx⁵ que lleve `VALARM` (la alarma), `ATTENDEE`, `LOCATION`, `CATEGORIES`, `X-*`… pierde **todas** esas propiedades en cuanto alguien le cambia el título desde la SPA. Sin aviso, sin confirmación y sin recuperación: el payload anterior se sobrescribe.

Y `LOCATION` y `URL` **no existen en ninguna capa** (ni parser, ni BD, ni DTO, ni formulario). Auditoría de las propiedades estándar: **7 de 26** utilizables.

## Objetivo

1. **O1 — Conservación.** Un `PATCH` de un evento **no puede destruir** nada del `.ics` almacenado que no esté modificando. Las propiedades y subcomponentes no modelados (`VALARM`, `ATTENDEE`, `CATEGORIES`, `X-*`, `VTODO`, un segundo `VEVENT`…) sobreviven **byte a byte**.
2. **O2 — `LOCATION` y `URL` como campos de primera clase** en las cuatro capas: parser → BD (columna) → DTO/API → diálogo de creación/edición.
3. **O3 — Un evento se puede dejar sin descripción, sin ubicación y sin URL desde la SPA.** Hoy la UI nunca envía un campo vacío (`web/src/pages/Calendar.tsx:287-291` y `:328-335`); la API **sí** sabe vaciar (`normalize_optional` convierte `""` en `None`, `src/api/events.rs:408-427`). Es la UI la que no lo usa.

## Fuera de alcance (decidido con el usuario, no por omisión)

- `RECURRENCE-ID` e instancias desprendidas: exige **relajar `uq_calendar_objects_collection_uid`** (`migrations/0001_init.sql:63`) para admitir dos filas con el mismo `UID`, más UI propia. Feature aparte.
- `ORGANIZER` / `ATTENDEE`: **no se modelan**; solo se garantiza que no se destruyen (lo cubre O1).
- `VALARM` en el formulario: no se edita, solo se conserva.
- Envío de invitaciones (iMIP / RFC 6638), `VFREEBUSY`.
- Metadatos de servidor editables (`SEQUENCE`, `CREATED`, `LAST-MODIFIED`): solo se refresca `DTSTAMP`. Queda anotado que `SEQUENCE` es el contador de revisiones que esperan los clientes: es deuda declarada.
- `DUE`: es una propiedad de `VTODO`, no de `VEVENT`.
- Indexar `LOCATION` en FTS (`events_fts` indexa `summary` y `description`): deuda.
- `TZID` y `RRULE` en la interfaz: fase posterior ("C").
- `ATTACH`: fase posterior ("B").

## Decisión de diseño: reconciliación quirúrgica del payload, línea a línea

**Rechazado** — recompilar el `.ics` desde el mapa de propiedades del parser: `icalendar 0.17.14` guarda las propiedades de un componente en un `BTreeMap` (`properties: BTreeMap<String, Property>`, `src/components.rs:27`). Aunque **conserva los duplicados** en `multi_properties: BTreeMap<String, Vec<Property>>` (`:28`), un `BTreeMap` **destruye el orden original** y entrega valores ya normalizados. Reconstruir desde ahí reescribe el payload entero: no es pérdida de datos, pero viola "byte a byte" y depende del criterio de la librería.

**Rechazado** — serializar con `icalendar`: está declarado como `default-features = false, features = ["parser"]` (`Cargo.toml:92`), es decir **solo parser**. Por eso `build_ics` es artesanal.

**Elegido** — una función nueva que **reconcilia** el payload almacenado contra los valores finales ya validados:

```rust
/// Reconcilia las propiedades *modeladas* del primer `VEVENT` de `raw` con
/// `fields`, dejando intacta —byte a byte— cualquier otra línea del payload.
pub fn reconcile_ics(raw: &str, fields: &EventFields) -> Result<String, IcsError>
```

Algoritmo:

1. **Desplegado (unfold)**: partir el payload en líneas físicas y recomponer las lógicas (una línea lógica empieza sin espacio y continúa con las que empiezan por un espacio), conservando de qué físicas viene cada una.
2. **Localizar el `VEVENT` objetivo** con seguimiento de **profundidad** de componentes (`BEGIN:` / `END:`): el **primer** `VEVENT` de nivel de `VCALENDAR`, el mismo que elige `parse_ics` (`src/dav/ics.rs:98`, `.next()`).
3. **Reconciliar** cada propiedad modelada, **solo en el nivel superior** de ese `VEVENT` (nunca dentro de un `VALARM` anidado, donde una línea `SUMMARY:` pertenece a la alarma y no al evento):
   - valor final presente → sustituir la línea lógica existente **en su sitio**; si no existe, **insertar** en la posición canónica;
   - valor final ausente → **eliminar** la línea lógica (así se vacía un campo).
   - Conjunto modelado: `SUMMARY`, `DESCRIPTION`, `LOCATION`, `URL`, `DTSTART`, `DTEND`, `RRULE`, `DTSTAMP`.
4. **`VTIMEZONE`**: si la zona final de `DTSTART`/`DTEND` no está definida en el `VCALENDAR`, **añadir** su bloque con el renderizador existente (`vtimezone::render_vtimezone`). **Nunca se elimina** un `VTIMEZONE` que quede huérfano: un bloque no referenciado es válido y borrarlo arriesga romper una referencia (por ejemplo un `TZID` dentro de un `VALARM`). La acumulación se documenta como coste aceptado.
5. **Refold y unión con CRLF**: solo se vuelven a plegar las líneas **reescritas**; las no tocadas se emiten **verbatim**, con sus bytes y su plegado originales.

**Reutilización obligatoria**: la *emisión* de cada línea modelada se extrae a helpers compartidos (`emit_summary_line`, `emit_dtstart_line`, …) usados por `build_ics` **y** por `reconcile_ics`, para que exista **un solo** serializador y las dos rutas no puedan divergir.

**Contrato que no se puede romper**: `build_ics` sin `tzid` es **byte a byte idéntico** desde antes de F18, y hay un test que lo fija (`build_ics_without_tzid_is_byte_identical`, `src/dav/ics.rs:1004`). `LOCATION`/`URL` se emiten **solo si no son `None`**, así que los tests existentes siguen verdes; se añade uno nuevo que fija la identidad con `location: None, url: None`.

**Verificado, NO es un bug: no hay doble escape.** El parser devuelve los valores de texto **desescapados**; lo garantizan los tests `build_ics_escapes_reserved_characters` (`src/dav/ics.rs:982`) y `build_ics_round_trips_all_fields` (`:872`), ambos en verde. `LOCATION` es texto libre y sigue el mismo camino: `escape_text` al emitir, desescapado al leer. No hay nada que "arreglar" aquí.

**Red de seguridad**: si `reconcile_ics` no encuentra un `VEVENT` o el payload no tiene la estructura esperada, el `PATCH` **cae a `build_ics`** (comportamiento actual) en vez de fallar la petición, y registra el caso. No debería ocurrir nunca (toda fila nace de un `PUT`/`POST` ya parseado); se documenta como red.

## Contratos

### `src/dav/ics.rs`

- `EventFields` += `location: Option<String>`, `url: Option<String>`.
- `ParsedEvent` += `location: Option<String>`, `url: Option<String>`.
- `parse_ics`: lee `LOCATION` y `URL` (`event.get_location()` y `event.property_value("URL")`, `src/components.rs:449` y `:338` de la librería).
- `reconcile_ics`: nueva función `pub`, descrita arriba.

### BD

`migrations/0005_event_location_url.sql`:

```sql
-- migrations/0005_event_location_url.sql
-- F24: LOCATION y URL como campos de primera clase del VEVENT.

ALTER TABLE calendar_objects ADD COLUMN location TEXT;
ALTER TABLE calendar_objects ADD COLUMN url      TEXT;
```

- `CalendarObject` (`src/db/models.rs`) += `location`, `url`.
- `CalendarObjectUpdate` (`src/db/mod.rs`) += `location`, `url`.
- `insert_calendar_object` / `update_calendar_object` y los `SELECT`: incluir las dos columnas.

### API

- `EventDto` += `location`, `url`.
- `CreateEventRequest` += `location`, `url` (opcionales).
- `UpdateEventRequest` += `location`, `url` (opcionales; **vacío ⇒ `None` ⇒ se elimina la línea**). `is_empty()` debe contemplarlos.
- Validación:
  - `location`: `normalize_optional(..., MAX_LOCATION_LEN)`; texto libre, se escapa al emitir.
  - `url`: **nueva** `validate_url`; debe ser una URI absoluta con esquema en **`http` o `https`** (lista blanca). El motivo es de seguridad, no de estilo: la SPA pinta la URL como enlace y un `javascript:` sería XSS. Longitud acotada. Vacío ⇒ `None`.
- `update()`: sustituir `build_ics(&fields)` por `reconcile_ics(&existing.raw_payload, &fields)`, con la caída descrita arriba.
- `create()`: sigue usando `build_ics` (no hay payload previo que conservar).
- `src/dav/caldav.rs` `put_event`: **sin cambios de flujo** (sigue guardando verbatim); gana la denormalización de `location` y `url` porque `ParsedEvent` ya las trae.

### Frontend

- `web/src/types/api.ts`: `CalendarEvent`, `CreateEventInput` y `UpdateEventInput` += `location`, `url`.
- `web/src/pages/Calendar.tsx`:
  - `EventFormValues` += `location`, `url`; `formInitialValues` los carga (`editing.location ?? ''`).
  - Dos `Form.Item` tras "Descripción": **Ubicación** (`Input`) y **URL** (`Input` con `type="url"`).
  - **Payload: `description`, `location` y `url` se envían SIEMPRE**, incluida la cadena vacía, para que la API los normalice a `None` y se puedan borrar (arregla O3 y, de paso, el borrado de la descripción, hoy imposible desde la SPA).
    - **Verificado como seguro**: el formulario se rellena con el evento (`formInitialValues`, `web/src/pages/Calendar.tsx:376-387`) y el `Modal` usa `destroyOnHidden`, así que enviar el campo no pisa valores ajenos.
  - `EventChanges` (capa optimista) += `location`, `url`; el enlace de la URL se pinta con `rel="noopener noreferrer"`.

## Escenarios (BDD)

Unitarios en `src/dav/ics.rs`; integración en `tests/api.rs`, `tests/caldav.rs` y `tests/event_location_url_migration.rs`.

**E1 — No se destruye lo no modelado.** *Dado* un payload con `SUMMARY`, `DESCRIPTION`, un `VALARM` con `TRIGGER`, dos `ATTENDEE`, `CATEGORIES`, `X-CUSTOM` y `LOCATION`; *cuando* un `PATCH` cambia **solo** `summary`; *entonces* la respuesta es `200`, el nuevo `SUMMARY` está presente y `VALARM`, los dos `ATTENDEE`, `CATEGORIES`, `X-CUSTOM` y `LOCATION` aparecen **verbatim** en el payload almacenado. (Hoy se pierden todos.)

**E2 — `LOCATION` nueva se inserta.** *Dado* un payload sin `LOCATION`; *cuando* `PATCH {location: "Sala 3"}`; *entonces* se emite exactamente una línea `LOCATION:Sala 3` y el resto del payload no cambia.

**E3 — `LOCATION` se borra.** *Dado* un payload con `LOCATION`; *cuando* `PATCH {location: ""}`; *entonces* no queda ninguna línea `LOCATION:` y el resto es idéntico.

**E4 — Caracteres reservados en `LOCATION`.** *Dado* `PATCH {location: "Sala 3; planta 2, ala \norte"}`; *entonces* en el wire se escapan `;`, `,` y `\`, y al reparsear vuelve el valor original (sin doble escape).

**E5 — `URL` con esquema hostil se rechaza.** *Dado* `PATCH {url: "javascript:alert(1)"}`; *entonces* `400` con error de validación y el payload almacenado **no cambia**. Y `https://ejemplo.org/x` se acepta y se emite como `URL:https://ejemplo.org/x`.

**E6 — Cambiar de zona añade `VTIMEZONE` y no borra la anterior.** *Dado* un payload con `DTSTART;TZID=Europe/Madrid:…` y su `VTIMEZONE`; *cuando* `PATCH {tzid: "Asia/Tokyo"}`; *entonces* `DTSTART` pasa a `TZID=Asia/Tokyo`, aparece el `VTIMEZONE` de Tokyo, el de Madrid **permanece** y `VALARM`/`X-*` siguen intactos.

**E7 — Un segundo `VEVENT` no se toca.** *Dado* un `VCALENDAR` con dos `VEVENT`; *cuando* se edita el primero; *entonces* el segundo queda **byte a byte** idéntico.

**E8 — El `PUT` del DAV sigue siendo verbatim.** *Dado* un `.ics` rico enviado por `PUT`; *entonces* el `GET` devuelve exactamente los mismos bytes (regresión; debe pasar ya, sin cambios).

**E9 — El `PUT` denormaliza `LOCATION` y `URL`.** *Dado* un `.ics` con `LOCATION` y `URL` enviado por `PUT`; *entonces* `GET /api/v1/collections/{id}/events/{uid}` los devuelve.

**E10 — Crear con `LOCATION` y `URL`.** *Dado* un `POST` con `location` y `url`; *entonces* `201`, el DTO las trae y el `.ics` las contiene.

**E11 — Migración desde el esquema 0004.** *Dada* una BD con el esquema y los datos previos a 0005; *cuando* se aplica la migración; *entonces* existen las columnas `location` y `url`, las filas conservan sus datos y el enlace `rowid` con `events_fts` sigue intacto.

**E12 — Identidad byte a byte de `build_ics` sin `location`/`url`.** *Dado* el mismo `EventFields` de F18 con `location: None, url: None`; *entonces* la salida es idéntica byte a byte a la anterior al cambio.

**E13 (frontend) — El formulario envía y vacía.** *Dado* el diálogo en edición de un evento con descripción, ubicación y URL; *entonces* los tres campos vienen rellenos; al vaciarlos y guardar, el `PATCH` incluye `''` en los tres y la tarjeta deja de mostrarlos. Con una URL inválida, el formulario no permite guardar.

## Tareas (TDD)

1. **[RED]** `src/dav/ics.rs`: tests de `reconcile_ics` (E1, E2, E3, E7, E12). Falla: la función no existe.
2. **[GREEN]** `reconcile_ics` + extraer los helpers de emisión compartidos y refactorizar `build_ics` para usarlos **sin cambiar un byte**.
3. **[RED]** `LOCATION`/`URL`: tests de `parse_ics` (E4 lectura desescapada), `build_ics` (emisión + E12) y `validate_url` (E5).
4. **[GREEN]** `EventFields`, `ParsedEvent`, `parse_ics`, `build_ics` y `validate_url`.
5. **[RED]** `tests/event_location_url_migration.rs` (E11), copiando el patrón de `tests/timezone_migration.rs` y `tests/app_tokens_migration.rs`.
6. **[GREEN]** `migrations/0005_…sql` + `CalendarObject` + `CalendarObjectUpdate` + los statements de BD.
7. **[RED]** `tests/api.rs`: E1 de extremo a extremo, E3, E5, E9, E10 y `is_empty` con los campos nuevos.
8. **[GREEN]** DTOs + `create`/`update` + `reconcile_ics` en el `PATCH` + denormalización en `put_event`.
9. **[RED]** `web/src/pages/Calendar.test.tsx`: E13.
10. **[GREEN]** tipos + formulario + payload + enlace seguro.
11. **[REFACTOR]** `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `npm run lint`.
12. **[CIERRE]** `docs/F24-preservacion-ics-location-url.md`, y el plan renombrado a `-COMPLETED.md` con sección `## Resultado`.

Commits previstos, en este orden (un tipo por asunto):

1. `🐛 fix: el PATCH conserva las propiedades no modeladas del .ics`
2. `✨ feat: LOCATION y URL como campos de primera clase`
3. `✅ test: migración y cobertura de LOCATION/URL`

## Riesgos

| Riesgo | Mitigación |
|---|---|
| Romper la identidad byte a byte de `build_ics` | Helpers compartidos + test E12 explícito |
| Reconciliar dentro de un `VALARM` (una línea `SUMMARY:` de la alarma) | Seguimiento de profundidad obligatorio; E1 incluye un `VALARM` con su propio `SUMMARY` |
| Payload degenerado que no se puede reconciliar | Caída documentada a `build_ics` + test |
| XSS por URL (`javascript:`) | Lista blanca `http`/`https` en el servidor **y** `type="url"` + `rel="noopener noreferrer"` en la SPA |
| Enviar siempre `description` pisa datos ajenos | Verificado que `formInitialValues` carga el evento; test E13 |
| Migración sobre una BD viva | `ALTER TABLE ADD COLUMN` aditivo, sin reescritura de tabla; test E11 |
| `--locked` roto | No se añaden dependencias; si `Cargo.lock` cambia por otra razón, `cargo check` y commitearlo |

## Verificación de cierre

- Backend: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` (hoy **563/0**; debe subir, nunca bajar).
- Frontend (desde `web/`): `npm run test`, `npm run typecheck`, `npm run lint`, `npm run build`.
- Prueba del bug, medida y con evidencia: `PUT` de un `.ics` **realista** con `VALARM`, dos `ATTENDEE`, `CATEGORIES` y `X-CUSTOM`; `PATCH` cambiando solo el título; `GET`; **diff**: solo deben cambiar las líneas `SUMMARY` y `DTSTAMP`.
- Prueba negativa: `PATCH` con `url: "javascript:..."` → `400` y payload intacto.

## Versionado

`feat` → **minor** (0.6.0). Aunque la conservación es un `fix`, el plan **añade capacidad visible al usuario** (`LOCATION` y `URL` en la API y en el diálogo) y la regla del repo es que manda el tipo más alto.

## Resultado

*(Se completa al cerrar el plan.)*
