# F10 — API de búsqueda (`/api/v1/search`)

## Qué hace

Expone el motor FTS5 (trigram + bm25) de OxiDav a la SPA para buscar eventos y contactos.

## Endpoints

| Método | Ruta | Query | Respuesta |
|---|---|---|---|
| GET | `/api/v1/search/events` | `q` (string), `limit` (int, opcional) | `200` `EventSearchResult[]` / `401` |
| GET | `/api/v1/search/contacts` | `q` (string), `limit` (int, opcional) | `200` `ContactSearchResult[]` / `401` |
| GET | `/api/v1/search` (agregado) | `q` (string), `limit` (int, opcional) | `200` `{ "events": [...], "contacts": [...] }` / `401` |

Todos los endpoints van bajo `require_auth`. Sin token válido → `401`.

## Aislamiento por usuario (anti-IDOR)

Las funciones de F03 (`search_events` / `search_contacts` en `src/db/search.rs`) buscaban en **todas** las colecciones, sin scoping por usuario. Exponerlas tal cual desde HTTP produciría un **IDOR**: cualquier usuario autenticado vería resultados de otros.

F10 añade variantes **scopeadas** — `search_events_scoped(pool, user_id, query, limit)` y `search_contacts_scoped(pool, user_id, query, limit)` — que filtran con:

```sql
JOIN collections ON collections.id = <tabla>.collection_id
WHERE collections.user_id = ?
```

El `user_id` sale del claim `sub` del JWT validado (`AuthUser.sub`). Los handlers SOLO usan las variantes scopeadas; el filtro por `user_id` es obligatorio y se prueba con dos usuarios distintos.

Las variantes sin scope se conservan intactas (mantienen firmas y tests de F03) y llevan un aviso de **"no usar desde HTTP"** en su documentación.

## Semántica de búsqueda

- **Tokenizer `trigram`** + ranking **`bm25 ASC`** (en FTS5 más negativo = más relevante, así que el más relevante primero).
- `q` con menos de 3 caracteres (tras `trim`) → lista vacía (limitación del tokenizer trigram, no es error).
- `limit` clampado a `1..=200` (default 20); valores por encima del máximo se clampan a `MAX_SEARCH_LIMIT=200`.
- **`q` se neutraliza como frase FTS5** (las comillas internas se duplican) para que metacaracteres/operadores (`"`, `AND`, `(`, `*`, …) NO produzcan error de sintaxis ni `500`; el input del usuario es siempre literal.

## DTOs

**`EventSearchResult`**: `id`, `uid`, `collection_id`, `summary`, `dtstart`, `dtend`.

**`ContactSearchResult`**: `id`, `uid`, `collection_id`, `fn`, `email`, `phone`, `organization`.

Ambos **nunca** exponen `raw_payload` ni `secret_hash`.

## Errores

Envelope uniforme de F06 (`{"error":{"code","message"}}`); `400` para `limit` inválido; sin filtrar detalles internos (rutas de BD, errores de SQLite) al cliente.

## Cómo verificar

- `cargo test` (incluye `tests/api.rs` con JWT + SQLite reales y tests anti-IDOR con dos usuarios)
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt --check`

## Relacionado

- [PLAN-011 — API de búsqueda](../plans/PLAN-011-search-api-COMPLETED.md)
- [PLAN-001 — Roadmap General](../plans/PLAN-001-general.md)
- [F03 — Data layer](./F03-data-layer.md)
- [F06 — API REST base](./F06-rest-api.md)
- [AGENTS.md](../AGENTS.md)
