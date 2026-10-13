# PLAN-008 — F07 CalDAV core

## Objetivo

Implementar el núcleo CalDAV de OxiDav bajo `/dav` para que clientes nativos
(iOS, DAVx⁵, Thunderbird) puedan descubrir calendarios, subir, leer y borrar
eventos `.ics`, y consultarlos por rango temporal. Se reutiliza la capa de datos
(F03) para persistir el `.ics` crudo y los campos desnormalizados, y la
autenticación Basic Auth (F05) para proteger el árbol DAV.

## Alcance

- Módulo `src/dav/`:
  - `mod.rs` — router DAV y montaje de rutas.
  - `xml.rs` — helpers de multistatus y parseo de cuerpos `PROPFIND`/`REPORT`.
  - `caldav.rs` — handlers CalDAV.
  - `ics.rs` — parseo y serialización `.ics`.
- Métodos y operaciones incluidas:
  - `OPTIONS` sobre `/dav/` con cabeceras `DAV`.
  - `MKCALENDAR` para crear colecciones.
  - `PROPFIND` sobre `/dav/`, calendar-home y colecciones.
  - `REPORT` con `calendar-query` y filtro `time-range`.
  - `PUT` y `DELETE` de recursos `.ics`.
- Exposición de la propiedad `sync-token` en las respuestas de `PROPFIND`
  (preparación para F09), aunque el `REPORT sync-collection` completo no se
  implementa aquí.
- FUERA de alcance:
  - CardDAV (F08).
  - `REPORT sync-collection` RFC 6578 completo (F09).
  - Expansión de `RRULE` y `calendar-multiget` avanzado (si no se cubren, ver
    limitaciones documentadas en Decisiones técnicas).
  - SPA de gestión (F06 / posteriores).

## Decisiones técnicas

- **XML:** se usa `quick-xml` para LEER los cuerpos de `PROPFIND`/`REPORT` y para
  ESCRIBIR las respuestas `207 Multistatus`. El parser se configura con DTD/XXE
  deshabilitado: no se resuelven entidades externas ni DTDs internas, evitando
  XXE y ataques *billion-laughs*. Se manejan los namespaces `DAV:` y
  `urn:ietf:params:xml:ns:caldav`.
- **Parseo `.ics`:** se usa el crate `icalendar` para extraer `UID`, `SUMMARY`,
  `DESCRIPTION`, `DTSTART`, `DTEND` y `RRULE`. El `.ics` crudo se guarda íntegro
  en `calendar_objects.raw_payload` y los campos anteriores se desnormalizan en
  columnas de `calendar_objects`.
- **Conversión de fechas:** las fechas se convierten a *epoch en milisegundos*,
  coherente con F03. Tratamiento documentado:
  - `VALUE=DATE` (todo el día): se interpreta como medianoche UTC del día
    indicado; `DTEND` exclusivo según RFC 5545. Se documenta que puede haber
    desviación de ±1 día en visualizaciones locales.
  - Zonas horarias: se soportan UTC (`Z`) y offsets explícitos (`±HHMM`). Los
    `TZID` con base VTIMEZONE **no** se resuelven en F07; se documenta como
    limitación y se guardan tal cual en `raw_payload` para su resolución futura.
  - `RRULE` se almacena como texto; **no** se expande en F07 (limitación
    documentada).
- **Auth:** `/dav` está protegido con `require_basic_auth` (F05). El `{username}`
  de la ruta debe coincidir con el `sub` autenticado (`AuthUser`); si no coincide
  se responde `403 Forbidden` (el recurso existe conceptualmente pero no es del
  usuario). Se documenta esta elección frente a `404`.
- **ETags:** se usa `calendar_objects.etag`. Se soportan los condicionales
  `If-Match` e `If-None-Match` en `PUT` y `DELETE`, devolviendo
  `412 Precondition Failed` en conflicto (p. ej. `If-Match` que no coincide, o
  `If-None-Match: *` sobre un recurso ya existente).
- **Códigos DAV:**
  - `207 Multi-Status` en `PROPFIND` y `REPORT`.
  - `201 Created` / `204 No Content` en `PUT`.
  - `200 OK` / `404 Not Found` en `DELETE`.
  - `201 Created` en `MKCALENDAR`.
- **Rutas canónicas:**
  - `/dav/` — raíz / principal.
  - `/dav/calendars/{username}/` — calendar-home del usuario.
  - `/dav/calendars/{username}/{collection}/` — colección de calendario.
  - `/dav/calendars/{username}/{collection}/{uid}.ics` — recurso de evento.
- **Autorización por `sub`:** el `{username}` de la URL se compara con el claim
  `sub` de Pocket ID; cualquier discrepancia se traduce en `403`.

## Modelo de rutas y autorización

| Ruta | Método | Handler | Auth | Respuesta |
|---|---|---|---|---|
| `/dav/` | `OPTIONS` | `options` | Basic (F05) | `200` + cabeceras `DAV: 1, 2, calendar-access`, `Allow` |
| `/dav/calendars/{username}/` | `PROPFIND` | `propfind` | Basic + `sub`==`username` | `207` con colecciones del home |
| `/dav/calendars/{username}/` | `MKCALENDAR` | `mkcalendar` | Basic + `sub`==`username` | `201 Created` |
| `/dav/calendars/{username}/{collection}/` | `PROPFIND` | `propfind` | Basic + `sub`==`username` | `207` con recursos + `getetag` |
| `/dav/calendars/{username}/{collection}/` | `REPORT` | `report` | Basic + `sub`==`username` | `207` con `calendar-query` (`time-range`) |
| `/dav/calendars/{username}/{collection}/{uid}.ics` | `PUT` | `put_event` | Basic + `sub`==`username` | `201`/`204`, honra `If-Match`/`If-None-Match` |
| `/dav/calendars/{username}/{collection}/{uid}.ics` | `DELETE` | `delete_event` | Basic + `sub`==`username` | `200`/`204`/`404`, honra condicionales |
| `/dav/calendars/{username}/{collection}/{uid}.ics` | `GET` | `get_event` | Basic + `sub`==`username` | `200` con `.ics`, `ETag`, `Content-Type: text/calendar` |
| Cualquiera bajo `/dav` | * | middleware | Sin Basic válido | `401` + `WWW-Authenticate: Basic` |

## Contratos / Interfaces

- Tipos de recursos:

```rust
struct CalendarResource {
    href: String,
    etag: String,
    ics: String,
}

struct Collection {
    href: String,
    name: String,
    color: Option<String>,
    description: Option<String>,
}
```

- Helpers XML en `src/dav/xml.rs`:

```rust
fn multistatus(responses: &[DavResponse]) -> String;

struct ParsePropfind { /* props solicitadas */ }
struct ParseCalendarQuery { time_range: Option<TimeRange> }
struct TimeRange { start_ms: i64, end_ms: i64 }
```

- Parseo de `.ics` en `src/dav/ics.rs`:

```rust
fn parse_ics(raw: &str) -> Result<ParsedEvent, IcsError>;

struct ParsedEvent {
    uid: String,
    summary: Option<String>,
    description: Option<String>,
    dtstart_ms: i64,
    dtend_ms: Option<i64>,
    rrule: Option<String>,
}
```

- Handlers en `src/dav/caldav.rs`:

```rust
async fn mkcalendar(...) -> Result<impl IntoResponse, DavError>;
async fn propfind(...) -> Result<impl IntoResponse, DavError>;
async fn report(...) -> Result<impl IntoResponse, DavError>;
async fn put_event(...) -> Result<impl IntoResponse, DavError>;
async fn delete_event(...) -> Result<impl IntoResponse, DavError>;
async fn options(...) -> Result<impl IntoResponse, DavError>;
```

- Errores:

```rust
enum DavError { /* ... */ }

impl IntoResponse for DavError { /* multistatus / códigos correctos */ }
```

## Tareas (TDD)

- [x] **Tarea 1 — OPTIONS en `/dav/`**
      Test: `OPTIONS /dav/` responde `200` con `DAV: 1, 2, calendar-access`.
      Implementar `options` y el montaje del router en `src/dav/mod.rs`.

- [x] **Tarea 2 — MKCALENDAR**
      Test: `MKCALENDAR /dav/calendars/{username}/{col}/` → `201`; la colección
      aparece en el `PROPFIND` del calendar-home.
      Implementar `mkcalendar` sobre `collections` (F03).

- [x] **Tarea 3 — PROPFIND calendar-home y colección**
      Test: `PROPFIND` del home lista colecciones; `PROPFIND` de una colección
      lista recursos con `getetag`.
      Implementar parseo `ParsePropfind` y `multistatus`.

- [x] **Tarea 4 — PUT de evento (creación y actualización)**
      Test: `PUT` de `.ics` válido → `201`, desnormaliza
      `uid/summary/dtstart/dtend` y guarda `raw_payload`; `PUT` con mismo `UID`
      actualiza y genera ETag nuevo.
      Implementar `parse_ics` + `put_event` (F03 repos).

- [x] **Tarea 5 — Condicionales ETag**
      Test: `PUT` con `If-None-Match: *` sobre existente → `412`; `If-Match`
      incorrecto → `412`.

- [x] **Tarea 6 — GET/PROPFIND de recurso**
      Test: `GET`/`PROPFIND` de un recurso devuelve el `.ics` con `getetag` y
      `getcontenttype`.

- [x] **Tarea 7 — DELETE de recurso**
      Test: `DELETE` → `204` y desaparece; inexistente → `404`.

- [x] **Tarea 8 — REPORT calendar-query**
      Test: `REPORT calendar-query` con `time-range` filtra por solape de
      `dtstart/dtend` y devuelve `calendar-data`.
      Implementar `ParseCalendarQuery` y `report`.

- [x] **Tarea 9 — Autenticación y autorización**
      Test: `/dav` sin Basic → `401` con `WWW-Authenticate: Basic`; token válido
      abre; `{username}` distinto del `sub` → `403`.

- [x] **Tarea 10 — Unitarios de `parse_ics`**
      Test: evento simple, con `RRULE`, con `VALUE=DATE` (todo el día) y con
      fecha con offset.

- [x] **Tarea 11 — Calidad**
      `cargo fmt --check` y `cargo clippy -- -D warnings` limpios.

## Criterios de aceptación

- Todos los métodos responden con los códigos DAV especificados (`207`, `201`,
  `204`, `200`, `404`, `412`, `401`, `403`).
- Las respuestas `multistatus` son XML válido con namespaces `DAV:` y `caldav`
  correctos.
- Los eventos `.ics` se desnormalizan correctamente en `calendar_objects`
  (`uid`, `summary`, `description`, `dtstart_ms`, `dtend_ms`, `rrule`) y el
  `raw_payload` conserva el `.ics` original.
- Los ETags se emiten y los condicionales `If-Match`/`If-None-Match` se respetan
  con `412` en conflicto.
- `/dav` exige Basic Auth; el `{username}` debe coincidir con el `sub`.
- El parser XML no resuelve entidades externas ni DTDs (sin XXE ni
  *billion-laughs*).
- `REPORT calendar-query` filtra correctamente por solape temporal.
- Suite completa en verde (`cargo test`), `fmt` y `clippy -D warnings` limpios.

## Riesgos

- Robustez del parseo `.ics` y de `RRULE`: entradas malformadas o variantes
  exóticas pueden fallar; mitigar con errores explícitos (`IcsError`) y tests de
  casos límite.
- Zonas horarias: `TZID`/VTIMEZONE no se resuelven en F07; riesgo de desviación
  horaria en clientes con zonas locales.
- Compatibilidad real con iOS, DAVx⁵ y Thunderbird: requiere pruebas manuales;
  diferencias de expectativas en XML o cabeceras pueden romper el descubrimiento.
- Correctitud del XML `multistatus`: un error de namespaces invalida toda la
  respuesta DAV.
- XXE / *billion-laughs* en `quick-xml`: mitigado deshabilitando DTD y resolución
  de entidades externas.
- Cálculo del solape de `time-range`: sobre eventos sin `DTEND`, todo el día o
  recurrentes puede dar resultados imprecisos (RRULE sin expandir).

## Referencias

- `plans/PLAN-001-general.md` (F07)
- `docs/F03-data-layer.md`
- `docs/F05-app-tokens-basic-auth.md`
- `AGENTS.md`
