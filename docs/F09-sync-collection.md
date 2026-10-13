# F09 — WebDAV sync-collection (RFC 6578)

## Qué hace

Sincronización incremental eficiente para clientes DAV: en vez de re-descargar toda la colección, el cliente envía su `sync-token` y recibe solo los cambios (altas, ediciones y **borrados**) desde ese punto.

## Log de cambios (tombstones)

Nueva tabla `sync_log` `(collection_id, resource_uid, resource_kind, change, sync_token, changed_at)`; se escribe en la MISMA transacción que cada mutación + bump de `sync_token` (extendido en la capa de datos de F03). `change ∈ {added, updated, deleted}`. **Sin `ON DELETE CASCADE` desde los objetos**: los tombstones de borrados deben sobrevivir para poder reportarse. Al **borrar una colección**, sus entradas de `sync_log` se eliminan en cascada (el cliente resincroniza desde cero).

## Formato del `sync-token`

URI opaca `http://oxidav/ns/sync/<n>` (prefijo `SYNC_TOKEN_URI_PREFIX` + `collections.sync_token`), parseable de vuelta a número. Token inválido/desconocido → `403` con cuerpo DAV `<d:error><d:valid-sync-token/></d:error>`.

## Semántica del REPORT

- **Sync inicial** (token vacío/ausente): `207` con todos los recursos vivos (`getetag` + `calendar-data`/`address-data` si se pide) + `<d:sync-token>` actual.
- **Incremental** (token N): cambios con `sync_token > N`; `added`/`updated` → response `200` con `getetag`; `deleted` → response con `<d:status>HTTP/1.1 404 Not Found</d:status>` **a nivel de `response`** (SIN `propstat`, como exige RFC 6578 §3.2) y el `href` del recurso; incluye el `sync-token` nuevo.
- `Depth`: `sync-collection` solo se define con `Depth: 0`; otro valor → `400`.
- `sync-level`: `1` soportado; `infinite` → `400`.
- Filtrado por tipo: un REPORT de calendario no reporta tarjetas ni viceversa.

## Descubrimiento

Las colecciones anuncian `sync-collection` en `DAV:supported-report-set`.

## Cobertura

Ambos protocolos (CalDAV y CardDAV), helpers compartidos en `src/dav/common.rs`.

## Limitaciones conocidas

El log de cambios **crece sin poda** (se compactará por antigüedad en el futuro); compatibilidad real con DAVx⁵/iOS requiere pruebas manuales.

## Cómo verificar

`cargo test` (tests de sync en `tests/caldav.rs` y `tests/carddav.rs`, más atomicidad/cascade en `tests/data_layer.rs`), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

## Relacionado

- [PLAN-010 — sync-collection](../plans/PLAN-010-sync-collection-COMPLETED.md)
- [PLAN-001 — Roadmap General](../plans/PLAN-001-general.md)
- [F03 — Data layer](./F03-data-layer.md)
- [F07 — CalDAV core](./F07-caldav-core.md)
- [F08 — CardDAV core](./F08-carddav-core.md)
- [AGENTS.md](../AGENTS.md)
