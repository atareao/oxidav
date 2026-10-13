# F08 — CardDAV core

## Qué hace

Núcleo CardDAV en `/dav/addressbooks/{username}/...`: descubrimiento de libretas, subida/lectura/borrado de contactos `.vcf` y consultas, para clientes nativos (iOS, DAVx⁵, Thunderbird).

## Rutas y métodos

| Ruta | Método | Respuesta |
|---|---|---|
| `/dav/` | `OPTIONS` | Anuncia capacidades DAV, incluyendo `addressbook` |
| `/dav/addressbooks/{username}/` (o `/{addressbook}/`) | `MKCOL` | `201` + `Location` (adopta el segmento de ruta como id saneado) |
| home / colección / recurso | `PROPFIND` | `207 Multistatus` |
| `/{uid}.vcf` | `GET` / `PUT` / `DELETE` | Lectura / creación-actualización / borrado del contacto |
| `addressbook-query`, `addressbook-multiget` | `REPORT` | `207 Multistatus` |

Auth: Basic app tokens (F05); `{username} == sub` (si no, `403`); la libreta debe pertenecer al usuario; una colección de tipo `calendar` usada como addressbook → `404`.

## Reutilización con CalDAV (`common.rs`)

F08 extrajo a `src/dav/common.rs` los helpers DAV compartidos (`Depth`, `MAX_ID_LEN`, `SYNC_TOKEN_URI_PREFIX`, `ensure_owner`/403, condicionales RFC 7232, ETags, saneo de id, negociación PROPFIND) para que CalDAV y CardDAV no divergieran.

## Parseo `.vcf`

Crate `vcard4` (`parse_loose`, tolerante con vCard 3.0/4.0); extrae `UID`/`FN`/`EMAIL`/`TEL`/`ORG`/`NOTE` con selección PREF-aware; fallback de `FN` desde `N`/`UID`. Slicing endurecido (sin pánicos con UTF-8 malformado).

## ETags y condicionales

`If-Match` fuerte / `If-None-Match` débil (RFC 9110) → `412`; compartido con CalDAV vía `common.rs`.

## Seguridad aplicada

Escapado del `addressbook-home-set` (evita inyección XML); **percent-encoding** de hrefs (emitir codificados, decodificar al parsear `multiget`) para interoperar con clientes reales; `Location` validada como `HeaderValue`.

## Limitaciones conocidas

`addressbook-query` filtra en memoria (no reutiliza FTS5 `search_contacts` todavía) — se debe documentar el motivo; filtros CardDAV avanzados (por tipo/param) no soportados; compatibilidad real con iOS/DAVx⁵/Thunderbird requiere pruebas manuales.

## Cómo verificar

`cargo test` (incluye `tests/carddav.rs` con Basic Auth + SQLite reales), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

## Relacionado

- [PLAN-009 — F08 CardDAV core](../plans/PLAN-009-carddav-core-COMPLETED.md)
- [PLAN-001 — Roadmap General OxiDav](../plans/PLAN-001-general.md)
- [F07 — CalDAV core](./F07-caldav-core.md)
- [F03 — Capa de datos](./F03-data-layer.md)
- [AGENTS.md](../AGENTS.md)
