# F06 — API REST `/api/v1`

## Qué hace

API JSON que consume la SPA: CRUD de colecciones, paginación y errores uniformes; protegida con el middleware `require_auth` de F04 (Bearer JWT validado contra Pocket ID).

## Alcance y arquitectura

Módulo `src/api/` (`mod.rs` router, `dto.rs`, `error.rs`, `collections.rs`); montado en `src/router.rs` bajo `/api/v1` con `require_auth`. Amplía la capa de datos con `get_collection`, `update_collection`, `count_collections` y `list_collections` scopeado+paginado.

## Endpoints

| Método | Ruta | Éxito | Errores |
|---|---|---|---|
| `GET` | `/api/v1/me` | `200` | `401` |
| `GET` | `/api/v1/collections?limit=&offset=` | `200` | `401` |
| `POST` | `/api/v1/collections` | `201` | `400`, `401` |
| `GET` | `/api/v1/collections/{id}` | `200` | `401`, `404` |
| `PATCH` | `/api/v1/collections/{id}` | `200` | `400`, `401`, `404` |
| `DELETE` | `/api/v1/collections/{id}` | `204` | `401`, `404` |

El path param `{id}` es un `String` (UUID; `collections.id` es `TEXT`).

## Decisiones

- DTOs separados de los modelos de BD; **nunca** exponen `raw_payload` ni `secret_hash`.
- Timestamps en **milisegundos** (epoch UTC, `i64`); decisión revisable (ISO-8601 si la SPA lo pide).
- **Scoping por `AuthUser.sub`**; recurso ajeno → `404` (anti-IDOR). `DELETE` scopeado también a nivel SQL (`WHERE id=? AND user_id=?`).
- **Envelope de error uniforme** `{"error":{"code","message"}}` para TODOS los errores, incluido el `401` del middleware y la query de paginación inválida. Los errores internos (`DbError`) no filtran detalle (solo a `tracing`).
- **Paginación** `limit`/`offset` clampeados (`DEFAULT_COLLECTION_LIMIT=20`, `MAX_COLLECTION_LIMIT=200`) y reportados efectivos; `total` por `COUNT`.
- Validación: `name` no vacío, `type` ∈ {calendar,addressbook}, `color` hex `^#[0-9a-fA-F]{6}$`. `PATCH` vacío → `400`.
- Códigos: `200/201/204/400/401/404/409/500`.

## Cómo verificar

`cargo test` (incluye `tests/api.rs` con JWT RS256 y SQLite reales), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

## Relacionado

- [`plans/PLAN-007-rest-api-COMPLETED.md`](../plans/PLAN-007-rest-api-COMPLETED.md)
- [`plans/PLAN-001-general.md`](../plans/PLAN-001-general.md)
- [`docs/F03-data-layer.md`](F03-data-layer.md)
- [`docs/F04-oidc-pocket-id.md`](F04-oidc-pocket-id.md)
- [`AGENTS.md`](../AGENTS.md)
