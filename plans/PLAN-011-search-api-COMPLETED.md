# PLAN-011 — F10 API de búsqueda (`/api/v1/search`)

## Objetivo

Exponer la búsqueda FTS5 ya existente (F03) a la SPA mediante endpoints REST JSON bajo `/api/v1/search`, garantizando el aislamiento de datos por usuario y registrando el contrato de DTOs y errores que consumirá el frontend.

## Alcance

Incluye:

- Crear `GET /api/v1/search/events?q=&limit=` → búsqueda de eventos (calendario).
- Crear `GET /api/v1/search/contacts?q=&limit=` → búsqueda de contactos (agenda).
- Decidir y, en su caso, añadir `GET /api/v1/search?q=&limit=` como endpoint agregado (eventos + contactos en una sola respuesta).
- Introducir scoping por usuario sobre las funciones de F03 mediante join con `collections` (o variantes nuevas).
- Definir los DTOs de resultado (sin `raw_payload`) y reutilizar el envelope de errores y el middleware de autenticación de F06.

No incluye:

- Cambios en la lógica de tokenización/ranking FTS5 (trigram + bm25) definida en F03.
- Indexación o sincronización de índices desde Rust (ya resuelta por triggers en F03).
- Endpoints DAV (`/dav/...`) ni cambios en el modelo de datos.
- Paginación por cursor, ordenamiento alternativo o facetas (fuera de alcance de F10).

## Decisiones técnicas

1. **Aislamiento por usuario (crítico).** Las funciones de F03 (`search_events` / `search_contacts` en `src/db/search.rs`) buscan en **todas** las colecciones **sin scoping por usuario**. Exponerlas tal cual desde la API produce un **IDOR**: cualquier usuario autenticado vería resultados de otros. F10 **DEBE** filtrar por `collections.user_id == AuthUser.sub`.
   - Opción A (recomendada): añadir variantes `search_events_scoped(pool, user_id, query, limit)` y `search_contacts_scoped(pool, user_id, query, limit)` que hagan `JOIN collections ON collections.id = <tabla>.collection_id WHERE collections.user_id = ?`. Mantiene intactas las firmas y tests de F03.
   - Opción B: modificar las funciones existentes para aceptar `user_id` y actualizar sus tests en F03.
   - En ambos casos el filtro por `user_id` es **obligatorio** y debe probarse con dos usuarios distintos.
2. **Mantenimiento de la semántica de F03.** Se conserva `bm25 ASC` (mejor coincidencia primero), el clamp de `limit` con `MAX_SEARCH_LIMIT=200`, y el comportamiento de query <3 caracteres → `[]` (limitación del tokenizer `trigram`, que exige ≥3 caracteres).
3. **Endpoints.** Dos endpoints específicos (`/search/events`, `/search/contacts`) con contrato claro y DTOs tipados. El endpoint agregado `GET /api/v1/search?q=` es opcional: si se añade, debe devolver ambos conjuntos en un único JSON (`{ "events": [...], "contacts": [...] }`) y compartir la misma validación de `q`/`limit`; si no se añade, documentar la decisión en `docs/F10-search-api.md`.
4. **Query vacía o corta no es error.** `q` vacío o con menos de 3 caracteres devuelve `200` con lista vacía (no `400`), reflejando fielmente la limitación de trigram de F03 y evitando romper la UI.
5. **DTOs sin payload crudo.** Los DTOs exponen campos ya parseados y **nunca** `raw_payload` (`.ics`/`.vcf` crudo). Esto preserva la separación de F06: el cliente no parsea formatos DAV.
6. **Errores uniformes.** Se reutiliza `ApiError` de F06 con el envelope `{"error":{"code","message"}}`. No se filtran detalles internos (rutas de BD, errores de SQLite) al cliente.
7. **Autenticación.** Todos los endpoints van bajo `require_auth`; el `user_id` de scoping se obtiene del claim `sub` (`AuthUser.sub`) del JWT validado. Sin token → `401`.

## Contratos / Interfaces

**Endpoints**

| Método | Ruta | Query | Respuesta |
|---|---|---|---|
| GET | `/api/v1/search/events` | `q` (string), `limit` (int, opcional) | `200` `EventSearchResult[]` / `401` |
| GET | `/api/v1/search/contacts` | `q` (string), `limit` (int, opcional) | `200` `ContactSearchResult[]` / `401` |
| GET | `/api/v1/search` (opcional) | `q` (string), `limit` (int, opcional) | `200` `{ events: [], contacts: [] }` / `401` |

**DTO de resultado de evento** (`EventSearchResult`)

- `id`: identificador interno del objeto de calendario.
- `uid`: UID iCalendar.
- `collection_id`: colección a la que pertenece.
- `summary`: título del evento.
- `dtstart`: inicio (formato serializado acordado).
- `dtend`: fin (formato serializado acordado).

**DTO de resultado de contacto** (`ContactSearchResult`)

- `id`: identificador interno del objeto de tarjeta.
- `uid`: UID vCard.
- `collection_id`: libreta a la que pertenece.
- `fn`: nombre formateado.
- `email`: correo.
- `phone`: teléfono.
- `organization`: organización.

> Ambos DTOs **excluyen** `raw_payload` por diseño.

**Comportamiento de `q` / `limit`**

- `q` ausente, vacío o con longitud < 3 → `200` con lista vacía.
- `limit` ausente → valor por defecto; `limit` por encima del máximo se clamp a `MAX_SEARCH_LIMIT=200`.
- Ranking: `bm25 ASC`.

**Errores** (envelope F06)

```json
{ "error": { "code": "<código>", "message": "<mensaje>" } }
```

- Sin token válido → `401`.
- Error interno → `500` con envelope, sin detalles internos.

## Tareas (TDD)

- [x] **Tarea 0 — Tests de scoping (RED, imprescindible).** Escribir primero el test que crea dos usuarios con colecciones y objetos propios, y verifica que `search_events_scoped` / `search_contacts_scoped` de un usuario **no** devuelven objetos del otro. Ejecutar y confirmar fallo (rojo) antes de implementar.
      `cargo test search_scoped`
- [x] **Tarea 1 — Implementar scoping por usuario.** Añadir (o modificar) las funciones en `src/db/search.rs` para filtrar con `JOIN collections ... WHERE collections.user_id = ?`. Pasar a verde el test de la Tarea 0.
      `cargo test search_scoped`
- [x] **Tarea 2 — Handler `GET /api/v1/search/events`.** Test-first del handler: resultado con DTO `EventSearchResult` (campos esperados, sin `raw_payload`), scoping aplicado y ranking bm25.
      `cargo test search_events_endpoint`
- [x] **Tarea 3 — Handler `GET /api/v1/search/contacts`.** Test-first del handler: DTO `ContactSearchResult` completo, sin `raw_payload`, scoping aplicado.
      `cargo test search_contacts_endpoint`
- [x] **Tarea 4 — Query corta / vacía → lista vacía.** Test de `q=""` y `q="ab"` (<3 chars) devolviendo `200` con lista vacía en ambos endpoints.
      `cargo test search_short_query`
- [x] **Tarea 5 — Clamp de `limit`.** Test que verifica que `limit` > 200 se clamp a `MAX_SEARCH_LIMIT=200` y que `limit` por defecto es correcto.
      `cargo test search_limit_clamp`
- [x] **Tarea 6 — Envelope de errores.** Test que verifica el formato `{"error":{"code","message"}}` ante error, sin filtrar detalles internos.
      `cargo test search_error_envelope`
- [x] **Tarea 7 — `401` sin token.** Test que confirma `401` en los endpoints de búsqueda cuando falta el token / es inválido (`require_auth`).
      `cargo test search_unauthorized`
- [x] **Tarea 8 — Endpoint agregado (opcional).** Si se decide añadir `GET /api/v1/search?q=`, test-first del JSON `{ events, contacts }`; en caso contrario, registrar la decisión en `docs/F10-search-api.md`.
      `cargo test search_aggregate`
- [x] **Tarea 9 — Actualizar tests de F03.** Si se optó por modificar las firmas existentes (Opción B), actualizar y ejecutar los tests de F03 para evitar regresiones.
      `cargo test`
- [x] **Tarea 10 — Formato y lints.** Verde completo de tests + lint sin warnings.
      `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

## Criterios de aceptación

- [x] `GET /api/v1/search/events?q=&limit=` devuelve `200` con `EventSearchResult[]`.
- [x] `GET /api/v1/search/contacts?q=&limit=` devuelve `200` con `ContactSearchResult[]`.
- [x] **Anti-IDOR:** un usuario autenticado **nunca** recibe resultados de colecciones de otro usuario (verificado con test de dos usuarios).
- [x] Ningún DTO expone `raw_payload` (verificado en test de respuesta).
- [x] `q` vacío o < 3 caracteres devuelve `200` con lista vacía (no `400`).
- [x] `limit` se clamp a `MAX_SEARCH_LIMIT=200`; el valor por defecto está documentado y probado.
- [x] Ranking consistente con F03: `bm25 ASC`.
- [x] Errores usan el envelope `{"error":{"code","message"}}` sin filtrar detalles internos.
- [x] Sin token válido → `401` en todos los endpoints de búsqueda.
- [x] `cargo test`, `cargo clippy -- -D warnings` y `cargo fmt --check` en verde.

## Riesgos

- **Scoping olvidado (IDOR).** Riesgo más grave: reutilizar las funciones de F03 sin `user_id` filtra datos entre usuarios. Mitigación: test de dos usuarios como Tarea 0 (RED antes de implementar) y revisión explícita del `JOIN`.
- **Rendimiento.** El `JOIN` con `collections` y el ranking bm25 sobre FTS5 pueden degradar con muchos objetos; el tokenizer trigram indexa substrings y puede ser costoso. Mitigación: medir con volúmenes realistas y garantizar uso del índice (`collections.user_id` indexado).
- **Limitación trigram (<3 caracteres).** La búsqueda por substrings exige ≥3 caracteres; queries más cortas no devuelven nada. Mitigación: documentarlo en la UI y devolver `200` con lista vacía en lugar de un error.
- **Evolución del contrato de DTOs.** Añadir/quitar campos puede romper la SPA. Mitigación: fijar los campos en `docs/F10-search-api.md` y tratar cambios como breaking change versionado.
- **Cambio de firmas de F03.** Modificar `search_events` / `search_contacts` puede romper tests existentes (Opción B). Mitigación: preferir variantes `_scoped` (Opción A) y, si se modifica, actualizar los tests de F03 (Tarea 9).

## Referencias

- `plans/PLAN-001-general.md` — feature **F10** (API de búsqueda).
- `docs/F03-data-layer.md` — FTS5 trigram + bm25, `MAX_SEARCH_LIMIT=200`, `src/db/search.rs`.
- `docs/F06-rest-api.md` — router `/api/v1`, `require_auth`, DTOs, `ApiError` con envelope.
- `AGENTS.md` — stack (Rust 2024 + Axum + sqlx), convenciones y flujo OpenSpec/TDD.
