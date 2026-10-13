# PLAN-007 — F06 API REST `/api/v1`

## Objetivo

Implementar la API REST JSON bajo `/api/v1` que consume la SPA de React, exponiendo DTOs limpios (sin `raw_payload` ni `secret_hash`), el CRUD de colecciones, paginación por `limit`/`offset` y un manejo uniforme de errores. Cablear el `Router` protegido con el middleware `require_auth` de F04 de forma real sobre `AppState`. La capa DAV (F07–F09) y las vistas de la SPA (F11–F14) quedan fuera de este plan.

## Alcance

- Nuevo módulo `src/api/` con:
  - `src/api/mod.rs` — construcción del `Router` de `/api/v1` y montaje de rutas.
  - `src/api/dto.rs` — DTOs JSON de request/response, separados de los modelos de BD.
  - `src/api/error.rs` — `ApiError` central + `impl IntoResponse`.
  - `src/api/collections.rs` — handlers HTTP del CRUD de colecciones.
- CRUD completo de **colecciones** (`collections`): listar, crear, obtener, actualizar parcialmente y eliminar.
- Paginación (`limit`/`offset`) con metadatos (`total`, `limit`, `offset`) y clamp de `limit`.
- Endpoint auxiliar `GET /api/v1/me` para exponer el `sub` autenticado y demostrar el middleware.
- Cableado del `Router` de `/api/v1` en `src/router.rs`, protegido con `require_auth` (F04).

**FUERA DE ALCANCE:**

- Endpoints REST de objetos de calendario y contactos (se exponen en F07/F08 o se amplían en un plan posterior).
- Endpoints DAV (`PROPFIND`, `REPORT`, `PUT`, `DELETE`, `MKCALENDAR`, sync-collection): F07–F09.
- Gestión de app tokens vía REST (F13 o extensión posterior de F06).
- Vistas/componentes de la SPA (F11–F14).

## Ampliación de la capa de datos (necesaria para F06)

La capa de datos de F03 solo expone `create_collection`, `list_collections` (sin scoping ni
paginación) y `delete_collection`. F06 debe **añadir** en `src/db/`:

- `get_collection(pool, id: &str, user_id: &str) -> Result<Option<Collection>, DbError>` (scopeado).
- `update_collection(pool, id: &str, user_id: &str, name: Option<&str>, color: Option<&str>) -> Result<Option<Collection>, DbError>`
  (parcial; `None` no modifica ese campo).
- `count_collections(pool, user_id: &str) -> Result<i64, DbError>`.
- Extender `list_collections` a
  `list_collections(pool, user_id: &str, limit: i64, offset: i64) -> Result<Vec<Collection>, DbError>`
  (scopeado + paginado); **actualizar las llamadas existentes** a la nueva firma.

Recordatorio del modelo real (F03): `collections.id` es `TEXT` (String), `created_at`/`updated_at`
están en **milisegundos**, y la columna `type` se mapea con `CollectionKind` (`Calendar`/`Addressbook`).

## Decisiones técnicas

- **DTOs vs. modelos de BD.** Los DTOs viven en `src/api/dto.rs` y están deliberadamente separados de los modelos de persistencia (`src/db/models.rs`). El cliente **nunca** ve `raw_payload` (JSON crudo `.ics`/`.vcf`) ni `secret_hash`. La conversión modelo → DTO es explícita (`impl From<&Collection> for CollectionDto`), lo que aísla el contrato HTTP de los cambios internos de esquema.
- **Serialización.** `serde` con feature `derive` (`Serialize`/`Deserialize`). Los timestamps, almacenados como `INTEGER` epoch en **milisegundos** en SQLite, se exponen como enteros epoch UTC (`i64`). Se evita añadir dependencias de fecha (`chrono`/`time`) en el MVP; **decisión revisable** si la SPA necesita formato ISO-8601.
- **Envoltura de error uniforme.** Un tipo `ApiError` central mapea a códigos HTTP y se serializa siempre como `{"error": {"code": "<código>", "message": "<mensaje>"}}`. Códigos: `400` validación, `401` no autenticado, `404` no encontrado, `409` conflicto, `500` interno. Los errores internos (`DbError`) **nunca** filtran detalles: el mensaje al cliente es genérico y el detalle real va a `tracing` (`tracing::error!`).
- **Scoping por usuario.** Todas las operaciones filtran por `AuthUser.sub` (== `collections.user_id`). Un recurso perteneciente a otro usuario se trata como inexistente → `404` (no `403`), evitando revelar la existencia de recursos ajenos (mitigación de IDOR).
- **Paginación y clamp.** Paginación por `limit`/`offset`. El `limit` se clampa con el mismo criterio que la búsqueda de F03: si `limit <= 0` se aplica el valor por defecto; si supera el tope máximo, se recorta al tope. La respuesta incluye `total`, `limit` y `offset` efectivos (ya clampeados), de modo que el cliente conoce el valor realmente aplicado.
- **Cableado de `require_auth`.** El `Router` de `/api/v1` se construye en `src/api/mod.rs` y se monta en `src/router.rs` envuelto con `require_auth` (F04) y `AppState`. Todas las rutas exigen `Authorization: Bearer <JWT>`; sin token válido → `401` con cabecera `WWW-Authenticate`.
- **Códigos de estado.** `200` lectura/actualización, `201` creación, `204` borrado sin cuerpo, `400` validación, `401` no autenticado, `404` no encontrado, `409` conflicto, `500` interno.
- **Idempotencia de `DELETE`.** Se adopta un **`404` explícito** cuando el recurso no existe (o no pertenece al usuario), por coherencia con el resto de operaciones de scoping y para que la SPA pueda distinguir "borrado con éxito" de "recurso inexistente". No se devuelve `204` silencioso ante un `id` desconocido. Una segunda llamada sobre un recurso ya borrado devuelve `404`.

## Contratos / Interfaces

### DTOs (`src/api/dto.rs`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionDto {
    pub id: String, // UUID (collections.id es TEXT PRIMARY KEY)
    pub name: String,
    pub r#type: CollectionType, // "calendar" | "addressbook"
    pub color: Option<String>,
    pub sync_token: i64,
    pub created_at: i64, // epoch UTC (milisegundos)
    pub updated_at: i64, // epoch UTC (milisegundos)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectionType {
    Calendar,
    Addressbook,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateCollectionRequest {
    pub name: String,
    pub r#type: CollectionType,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateCollectionRequest {
    pub name: Option<String>,  // parcial
    pub color: Option<String>, // parcial (null explícito no soportado en MVP)
}

#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeDto {
    pub sub: String,
}
```

### Validación de entrada

- `name`: no vacío (tras `trim`); longitud máxima razonable.
- `type`: debe ser uno de los valores del enum (`calendar` | `addressbook`); cualquier otro → `400`.
- `color`: si se envía, debe ser un color hexadecimal válido (`^#[0-9a-fA-F]{6}$`); si no → `400`.

### Errores (`src/api/error.rs`)

```rust
pub enum ApiError {
    Validation(String), // 400
    Unauthorized,       // 401
    NotFound,           // 404
    Conflict(String),   // 409
    Internal,           // 500 (detalle solo a tracing)
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // cuerpo: {"error": {"code": "...", "message": "..."}}
        // 401 añade cabecera WWW-Authenticate
        // Internal: mensaje genérico, detalle a tracing::error!
    }
}

impl From<DbError> for ApiError {
    fn from(e: DbError) -> Self {
        tracing::error!(error = ?e, "error de base de datos en la API");
        ApiError::Internal
    }
}
```

### Endpoints

Todos requieren `Authorization: Bearer <JWT>`; sin token válido → `401` con `WWW-Authenticate`.

| Método | Ruta | Éxito | Errores | Cuerpo de respuesta |
|---|---|---|---|---|
| `GET` | `/api/v1/me` | `200` | `401` | `MeDto { sub }` |
| `GET` | `/api/v1/collections?limit=&offset=` | `200` | `401` | `Page<CollectionDto>` (solo las del usuario) |
| `POST` | `/api/v1/collections` | `201` | `400`, `401` | `CollectionDto` |
| `GET` | `/api/v1/collections/{id}` | `200` | `401`, `404` | `CollectionDto` |
| `PATCH` | `/api/v1/collections/{id}` | `200` | `400`, `401`, `404` | `CollectionDto` |
| `DELETE` | `/api/v1/collections/{id}` | `204` | `401`, `404` | (sin cuerpo) |

El path param `{id}` de las rutas `GET`/`PATCH`/`DELETE` es un **`String`** (UUID de `collections.id`, que en BD es `TEXT PRIMARY KEY`). Un `id` malformado (no parseable como UUID) o inexistente (o perteneciente a otro usuario) → `404`, sin distinguir entre ambos casos.

Cuerpo de error uniforme:

```json
{
  "error": {
    "code": "not_found",
    "message": "El recurso solicitado no existe"
  }
}
```

## Tareas (TDD)

- [x] **Tarea 0: Ampliar la capa de datos.**
  - [x] **Paso 1 (RED):** tests en `tests/` para `get_collection` (scopeado), `update_collection`
        (parcial: solo modifica lo enviado), `count_collections` y `list_collections`
        scopeado+paginado.
  - [x] **Paso 2 (GREEN):** implementar las funciones en `src/db/` y actualizar las llamadas
        existentes a `list_collections`.
        `cargo test data_layer`

- [x] **Tarea 1: Router `/api/v1` montado + `GET /me` con autenticación.**
  - Crear: `src/api/mod.rs`, `src/api/error.rs`, `src/api/dto.rs`.
  - Modificar: `src/router.rs` (montar `/api/v1` con `require_auth`).
  - [x] **Paso 1 (RED):** test `GET /api/v1/me` sin token → `401` con `WWW-Authenticate`; con JWT válido (helpers de test de F04) → `200` y `{ "sub": "..." }`.
        `cargo test api::me`
  - [x] **Paso 2 (GREEN):** construir el `Router` de `/api/v1`, montarlo protegido con `require_auth` y `AppState`.
        `cargo test api::me`

- [x] **Tarea 2: `POST` crea colección → aparece en `GET` listado.**
  - Crear: `src/api/collections.rs`.
  - [x] **Paso 1 (RED):** `POST` con `{name, type, color}` → `201` con `CollectionDto`; `GET` listado → incluye la colección creada.
        `cargo test api::collections::create`
  - [x] **Paso 2 (GREEN):** handler de creación + listado con scoping por `sub`.
        `cargo test api::collections`

- [x] **Tarea 3: Validación `400`.**
  - [x] **Paso 1 (RED):** `name` vacío → `400`; `type` no soportado → `400`; `color` no hexadecimal → `400`; envoltura JSON correcta.
        `cargo test api::collections::validation`
  - [x] **Paso 2 (GREEN):** validación de entrada en create (y patch).
        `cargo test api::collections::validation`

- [x] **Tarea 4: Scoping `GET /{id}` de otro usuario → `404`.**
  - [x] **Paso 1 (RED):** crear colección con usuario A, pedirla autenticado como B → `404`.
        `cargo test api::collections::scoping`
  - [x] **Paso 2 (GREEN):** filtrar siempre por `user_id = AuthUser.sub`.
        `cargo test api::collections::scoping`

- [x] **Tarea 5: `PATCH` parcial actualiza solo lo enviado.**
  - [x] **Paso 1 (RED):** `PATCH {name}` cambia nombre y conserva `color`; `PATCH {color}` cambia color y conserva `name`.
        `cargo test api::collections::update`
  - [x] **Paso 2 (GREEN):** aplicar únicamente los campos `Some(..)`.
        `cargo test api::collections::update`

- [x] **Tarea 6: `PATCH`/`GET`/`DELETE` sobre inexistente → `404`.**
  - [x] **Paso 1 (RED):** con `id` inexistente, los tres endpoints devuelven `404`.
        `cargo test api::collections::not_found`
  - [x] **Paso 2 (GREEN):** mapear "no encontrado" de la capa de datos a `ApiError::NotFound`.
        `cargo test api::collections::not_found`

- [x] **Tarea 7: Paginación y clamp.**
  - [x] **Paso 1 (RED):** `limit`/`offset` devuelven el subconjunto correcto; `total` refleja el número total del usuario; `limit <= 0` → default; `limit` por encima del tope → clampado y reflejado en la respuesta.
        `cargo test api::collections::pagination`
  - [x] **Paso 2 (GREEN):** cálculo de `total` (COUNT) + clamp de `limit` con el criterio de F03.
        `cargo test api::collections::pagination`

- [x] **Tarea 8: `DELETE` elimina (y cascadea objetos) y desaparece del listado.**
  - [x] **Paso 1 (RED):** `DELETE` → `204`; `GET` listado ya no lo incluye; los objetos asociados (`calendar_objects`/`card_objects`) se eliminan en cascada; segundo `DELETE` → `404`.
        `cargo test api::collections::delete`
  - [x] **Paso 2 (GREEN):** borrado scopeado por usuario + verificación de cascada.
        `cargo test api::collections::delete`

- [x] **Tarea 9: Formato de la envoltura de error y no filtración interna.**
  - [x] **Paso 1 (RED):** un `500` (fallo de BD simulado) devuelve `{"error": {"code": "internal", "message": "..."}}` sin detalle interno; `400`/`404` usan el mismo formato.
        `cargo test api::error::envelope`
  - [x] **Paso 2 (GREEN):** `impl IntoResponse for ApiError` con cuerpo uniforme y detalle solo a `tracing`.
        `cargo test api::error::envelope`

- [x] **Tarea 10: Calidad.**
  - [x] **Paso 1:** `cargo fmt --check`
  - [x] **Paso 2:** `cargo clippy -- -D warnings`
  - [x] **Paso 3:** `cargo test` (suite completa en verde)

## Criterios de aceptación

- [x] Existe el módulo `src/api/` con `mod.rs`, `dto.rs`, `error.rs` y `collections.rs`.
- [x] El `Router` de `/api/v1` está montado en `src/router.rs` y protegido con `require_auth` (F04).
- [x] Todos los endpoints (`GET /me`, `GET/POST/GET{id}/PATCH{id}/DELETE{id}` de collections) responden con los códigos y cuerpos especificados.
- [x] Sin token válido, cualquier endpoint devuelve `401` con cabecera `WWW-Authenticate`.
- [x] Un recurso de otro usuario devuelve `404` (scoping por `AuthUser.sub`).
- [x] Validación de entrada devuelve `400` (nombre vacío, tipo inválido, color inválido).
- [x] `404` explícito en `PATCH`/`GET`/`DELETE` sobre recursos inexistentes o ajenos.
- [x] La paginación devuelve `limit`/`offset` clampeados y un `total` correcto.
- [x] `DELETE` elimina la colección y cascadea sus objetos; deja de aparecer en el listado.
- [x] Los DTOs nunca exponen `raw_payload` ni `secret_hash`; los errores internos no filtran detalles al cliente.
- [x] `cargo fmt --check`, `cargo clippy -- -D warnings` y `cargo test` pasan en verde.

## Riesgos

- **Coherencia DTO ↔ BD.** Un cambio en el esquema puede desalinear los DTOs; la conversión explícita (`From<&Collection>`) reduce el acoplamiento pero exige mantener el mapeo al día.
- **Validación de entrada.** Validaciones incompletas o inconsistentes entre `POST` y `PATCH` pueden permitir estados inválidos; conviene centralizar las reglas.
- **Scoping (IDOR).** Un filtro olvidado en algún handler expone recursos ajenos; cubrirlo con tests por endpoint.
- **Formato de error estable para la SPA.** Cambiar la envoltura rompe al cliente; tratarlo como contrato congelado y cubrirlo con tests.
- **Evolución de contrato sin versionado adicional.** `/api/v1` es el único prefijo de versión; cambios incompatibles requerirían `/api/v2` o campos aditivos, no romper `v1`.

## Referencias

- `plans/PLAN-001-general.md` (F06)
- `docs/F03-data-layer.md`
- `docs/F04-oidc-pocket-id.md`
- `AGENTS.md`
