# F07 — CalDAV core

## Qué hace

Núcleo CalDAV en `/dav`: descubrimiento de calendarios, subida/lectura/borrado de eventos `.ics` y consultas por rango temporal, para clientes nativos (iOS, DAVx⁵, Thunderbird).

## Rutas y métodos

| Ruta | Método | Respuesta |
|---|---|---|
| `/dav/` | `OPTIONS` | `200` + cabeceras `DAV: 1, 2, calendar-access`. |
| `/dav/calendars/{username}/` (o `/{collection}/`) | `MKCALENDAR` | `201` + `Location` (adopta el segmento de ruta como id de la colección si existe). |
| `/dav/calendars/{username}/` (home) y `/{collection}/` y `/{collection}/{uid}.ics` | `PROPFIND` | `207 Multistatus`. |
| `/…/{uid}.ics` | `GET` | `200` `text/calendar; charset=utf-8` + `ETag`; `404` si no existe. |
| `/…/{uid}.ics` | `PUT` | `201` (nuevo) / `204` (update) + `ETag`. |
| `/…/{uid}.ics` | `DELETE` | `204`; `404` si no existe. |
| `/…/{collection}/` | `REPORT` (calendar-query con `time-range`) | `207` con `getetag` + `calendar-data`. |

Autorización: Basic Auth con app tokens (F05); `{username}` debe == `sub` (si no `403`); colecciones/recursos siempre del usuario.

## XML y seguridad

`quick-xml` para leer `PROPFIND`/`REPORT` y escribir `multistatus` (namespaces `DAV:`, CalDAV, `cs:`). **DTD/entidades externas deshabilitadas** (rechazo explícito de `<!DOCTYPE>` → sin XXE ni billion-laughs).

## Parseo `.ics`

Crate `icalendar`; extrae `UID`/`SUMMARY`/`DESCRIPTION`/`DTSTART`/`DTEND`/`RRULE`. Se guarda el `.ics` crudo en `raw_payload` y los campos desnormalizados (epoch en **milisegundos**). Slicing por bytes endurecido (sin pánicos con entrada UTF-8 malformada).

## PROPFIND: negociación de propiedades

Soporta `allprop`, `propname` y `<d:prop>`; las props desconocidas se devuelven en un `propstat 404`. Cuerpo vacío = `allprop`.

## ETags y condicionales

`If-Match` con comparación **fuerte** (un candidato `W/"…"` nunca casa) e `If-None-Match` con comparación **débil** (RFC 9110); conflicto → `412`. `ETag` emitido en `201`/`204`/`GET`.

## Endurecimiento aplicado

Colección no-`calendar` (addressbook) tratada como `404`; `PUT` con `UNIQUE` violado → `409` (no `500`); `DELETE` honra `If-None-Match`; segmento de colección saneado (`sanitize_collection_id`); recurso exige sufijo `.ics`.

## Limitaciones conocidas

`REPORT` devuelve un set fijo de props (no negocia `<d:prop>` aún); `OPTIONS` solo en `/dav/`; `Depth: infinity` se degrada a `1`; **RRULE no se expande** (solo se guarda). La **limitación de zonas horarias queda levantada por [F18](./F18-timezone-support.md)**: el `TZID` se resuelve con la tzdb IANA embebida y el `.ics` compuesto incluye su bloque `VTIMEZONE`. Compatibilidad real con clientes DAV requiere pruebas manuales.

## Cómo verificar

`cargo test` (incluye `tests/caldav.rs` y `tests/dav.rs` con Basic Auth + SQLite reales), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

## Relacionado

- `plans/PLAN-008-caldav-core-COMPLETED.md`
- `plans/PLAN-001-general.md`
- `docs/F03-data-layer.md`
- `docs/F05-app-tokens-basic-auth.md`
- `AGENTS.md`
