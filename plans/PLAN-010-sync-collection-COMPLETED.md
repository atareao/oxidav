# PLAN-010 — F09 WebDAV sync-collection (RFC 6578)

## Objetivo

Implementar WebDAV `sync-collection` (RFC 6578) para CalDAV y CardDAV: reportar de forma incremental qué recursos cambiaron (altas, ediciones y **borrados** vía tombstones) desde un `sync-token` opaco, con soporte de sincronización inicial completa.

## Alcance

**Dentro:**

- Nueva migración `0003` con la tabla `sync_log` (log de cambios / tombstones) e índices.
- Escritura atómica de entradas en `sync_log` en la **misma transacción** que cada mutación de objeto (extiende los write-paths de F03) y en el borrado en cascada de colecciones.
- Funciones de acceso: `record_change`, `changes_since`, `next_sync_token`.
- Utilidades de token: `format_sync_token` / `parse_sync_token` y constantes.
- Handler `report_sync_collection` y parser del cuerpo `REPORT` (`<d:sync-collection>`), compartidos en `src/dav/common.rs`.
- Rutas `REPORT` para CalDAV (`src/dav/caldav.rs`) y CardDAV (`src/dav/carddav.rs`).
- `sync-level` `1` (miembros de la colección); rechazo explícito de `infinite`.
- Sync inicial (sin token) e incremental (con token).
- Tests TDD (migración, atomicidad, tombstones, inicial, incremental, round-trip de token, auth).

**Fuera (non-goals):**

- Poda / compactación del log de cambios (MVP conserva todo; ver *Riesgos*).
- `sync-level` `infinite` (recursivo/hijo-profundo) — se rechaza.
- Sincronización jerárquica multinivel más allá de los miembros directos de la colección.
- Cambios en el modelo de ETags ni en la negociación PROPFIND existente (F07/F08).

## Decisiones técnicas

1. **Log de cambios (tombstones).** Nueva tabla `sync_log` con columnas `collection_id`, `resource_uid`, `resource_kind`, `change`, `sync_token`, `changed_at`.
   - `change ∈ {added, updated, deleted}`.
   - `resource_kind ∈ {calendar, card}`.
   - Se escribe en la **MISMA transacción** que la mutación del objeto (extender los write-paths de F03) y también en el borrado en cascada de una colección (un `deleted` por recurso afectado).
   - Índice compuesto `(collection_id, sync_token)` para el barrido incremental.
   - **Sin** `ON DELETE CASCADE` desde `calendar_objects` / `card_objects`: los tombstones deben **sobrevivir** al borrado del objeto para poder reportarlo. La FK apunta a `collections` (con `ON DELETE CASCADE`), no a los objetos.
2. **`sync-token` opaco.** Valor expuesto = `SYNC_TOKEN_URI_PREFIX` (`http://oxidav/ns/sync/`) + el entero de `collections.sync_token`.
   - `format_sync_token(n: i64) -> String` construye la URI; `parse_sync_token(s: &str) -> Result<i64, _>` la revierte a número.
   - Token **inválido / malformado o desconocido** → respuesta de error `403` con `valid-sync-token` (precondición DAV). (Se documenta como decisión de diseño: se elige `403` sobre `409` porque el token es una precondición de estado, no un conflicto de escritura.)
   - Token **vacío o ausente** → sincronización **inicial**.
3. **Sync inicial vs. incremental.**
   - **Inicial** (sin `<d:sync-token>` en la petición): devuelve todos los recursos actuales de la colección, cada uno con `200` + `<d:getetag>`, más el `<d:sync-token>` actual al final del `multistatus`.
   - **Incremental** (con token `N`): devuelve los cambios con `sync_token > N`, **una entrada por recurso** (la más reciente): `added`/`updated` → `200` + `getetag`; `deleted` → `<d:status>HTTP/1.1 404 Not Found</d:status>` **a nivel de `response`**, sin `propstat` (RFC 6578 §3.2). Cierra con el `<d:sync-token>` actual.
4. **`sync-level`.**
   - `1` (miembros directos de la colección): soportado.
   - `infinite`: **rechazado** con `400 Bad Request` (no implementado en el MVP).
   - Ausente: se asume `1`.
5. **Poda del log.** El MVP **conserva todo** el historial de `sync_log`. La poda (por antigüedad o por retención tras un token seguro) queda documentada como trabajo futuro; sin poda no hay riesgo de reportar `deleted` incorrectos.
6. **Reutilización.** El mapeo número↔token, el parseo del cuerpo `REPORT` y la composición del `multistatus` de sync viven en `src/dav/common.rs` y son consumidos por `caldav.rs` y `carddav.rs`. Nada de lógica HTTP duplicada por protocolo.

## Esquema (migración 0003)

Migración `migrations/0003_sync_log.sql` (sqlx, SQLite). Se aplica con `sqlx migrate` / ejecutor de migraciones del proyecto.

```sql
-- 0003_sync_log.sql
-- Log de cambios (tombstones) para WebDAV sync-collection (RFC 6578).

CREATE TABLE sync_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    collection_id INTEGER NOT NULL,
    resource_uid  TEXT    NOT NULL,
    resource_kind TEXT    NOT NULL CHECK (resource_kind IN ('calendar', 'card')),
    change        TEXT    NOT NULL CHECK (change IN ('added', 'updated', 'deleted')),
    sync_token    INTEGER NOT NULL,
    changed_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE
);

-- Barrido incremental: cambios de una colección con sync_token > N, en orden.
CREATE INDEX idx_sync_log_collection_token
    ON sync_log (collection_id, sync_token);

-- Deduplicación/último cambio por recurso dentro de una colección.
CREATE INDEX idx_sync_log_collection_uid
    ON sync_log (collection_id, resource_uid, sync_token);
```

**Manejo de tombstones.** La FK `collection_id → collections(id)` usa `ON DELETE CASCADE`: al borrar una colección entera se eliminan sus entradas de log (coherente, dejan de poder consultarse). En cambio, **no existe FK hacia `calendar_objects` ni `card_objects`**: por eso, al borrar un recurso concreto, el objeto desaparece de su tabla pero la fila `sync_log` con `change='deleted'` **permanece** y es lo que permite reportar el `404` en el barrido incremental. Cada entrada `sync_token` es igual al valor de `collections.sync_token` tras avanzar la escritura que la generó, garantizando monotonía y orden.

## Contratos / Interfaces

Enums y tipos de dominio (`src/db/models.rs` o equivalente):

```rust
/// Tipo de cambio registrado en el log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Updated,
    Deleted,
}

/// Tipo de recurso afectado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum ResourceKind {
    Calendar,
    Card,
}

/// Entrada del log de cambios tal como se reporta en el sync.
#[derive(Debug, Clone)]
pub struct SyncChange {
    pub resource_uid: String,
    pub resource_kind: ResourceKind,
    pub change: ChangeKind,
    pub sync_token: i64,
    pub etag: Option<String>, // None para deleted (tombstone)
}
```

Acceso a datos — escritura atómica y lectura del log (`src/db/mod.rs`):

```rust
/// Registra un cambio en el log DENTRO de la transacción `tx` que ya está
/// mutando el objeto. Nunca abre su propia transacción (atomicidad F03).
pub async fn record_change(
    tx: &mut SqliteConnection,
    collection_id: i64,
    resource_uid: &str,
    resource_kind: ResourceKind,
    change: ChangeKind,
) -> Result<(), DbError>;

/// Devuelve, para `collection_id`, el último cambio por recurso con
/// `sync_token > since`. Para `change='deleted'` no hay etag.
/// `since = None` => sync inicial (todos los recursos vivos, etags actuales).
pub async fn changes_since(
    pool: &SqlitePool,
    collection_id: i64,
    since: Option<i64>,
) -> Result<Vec<SyncChange>, DbError>;

/// Token actual de la colección (valor de `collections.sync_token`).
pub async fn next_sync_token(
    pool: &SqlitePool,
    collection_id: i64,
) -> Result<i64, DbError>;
```

Utilidades de token (`src/dav/common.rs`):

```rust
/// Prefijo URI de los sync-token emitidos. Valor opaco para el cliente.
pub const SYNC_TOKEN_URI_PREFIX: &str = "http://oxidav/ns/sync/";

/// Número -> URI opaca, p. ej. 7 -> "http://oxidav/ns/sync/7".
pub fn format_sync_token(value: i64) -> String;

/// URI opaca -> número. `Err` si el prefijo no coincide o el sufijo no es
/// un entero válido (el handler lo traduce a 403 valid-sync-token).
pub fn parse_sync_token(token: &str) -> Result<i64, SyncTokenError>;
```

Handler y parser del `REPORT` (compartidos en `src/dav/common.rs`; invocados desde `caldav.rs` y `carddav.rs`):

```rust
/// Cuerpo parseado de <d:sync-collection>.
#[derive(Debug, Clone)]
pub struct SyncCollectionRequest {
    pub sync_token: Option<String>, // None/"" => sync inicial
    pub sync_level: SyncLevel,      // Members (1) | Infinite
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncLevel {
    Members,   // sync-level 1
    Infinite,  // rechazado con 400
}

/// Parsea el XML del cuerpo REPORT. Error => 400.
pub fn parse_sync_collection(body: &[u8]) -> Result<SyncCollectionRequest, DavError>;

/// Handler compartido de REPORT sync-collection para una colección.
/// 403 valid-sync-token si el token es inválido; 400 si sync-level=infinite.
/// Devuelve un multistatus con getetag (200) o tombstones (404).
pub async fn report_sync_collection(
    /* estado: pool, auth ctx, path de colección, kind */
) -> Result<Response, DavError>;
```

## Tareas (TDD)

### Migración y esquema

- [x] **Test (RED):** test de `sync_log` — tras aplicar `0003`, la tabla existe con columnas/CHECKs y ambos índices; las FK `ON DELETE CASCADE` desde `collections` funcionan y **no** hay FK hacia los objetos.
- [x] **GREEN:** escribir `migrations/0003_sync_log.sql` y registrarla.
- [x] **REFACTOR:** `cargo fmt --check` / `cargo clippy -- -D warnings`.

### Registro de cambios atómico (extiende F03)

- [x] **Test (RED):** un `PUT` (o alta de objeto) deja exactamente una fila `sync_log` con `change='added'`/`updated` y `sync_token` == nuevo valor de `collections.sync_token`, **en la misma transacción** (fallo simulado de la mutación ⇒ no queda entrada de log ni token avanzado).
- [x] **Test (RED):** `DELETE` de un recurso deja una fila `change='deleted'` y el objeto ya no existe.
- [x] **Test (RED):** borrado de una colección entera elimina sus filas de `sync_log` (cascade) junto a sus objetos.
- [x] **GREEN:** implementar `record_change` y engancharlo en los write-paths de F03 y en el borrado en cascada.
- [x] **REFACTOR:** unificar con los helpers transaccionales existentes; clippy.

### `changes_since` con tombstones

- [x] **Test (RED):** con cambios `added`(t=2), `updated`(t=3), `deleted`(t=4), `changes_since(since=1)` devuelve una entrada por recurso con el cambio más reciente; el `deleted` viene sin etag.
- [x] **Test (RED):** `changes_since(since=None)` (sync inicial) devuelve todos los recursos vivos con su etag actual y **excluye** recursos con última operación `deleted`.
- [x] **GREEN:** implementar `changes_since` (última fila por `resource_uid`, `sync_token > since`).
- [x] **REFACTOR:** clippy; revisar plan de consulta con el índice `(collection_id, sync_token)`.

### Sync inicial

- [x] **Test (RED):** `REPORT` sin `<d:sync-token>` → `207` con todos los recursos (`200` + `getetag`) y un `<d:sync-token>` final igual a `next_sync_token`.
- [x] **GREEN:** rama inicial de `report_sync_collection` + `parse_sync_collection`.
- [x] **REFACTOR:** reutilizar el compositor de `multistatus` de F07/F08.

### Sincronización incremental (alta / edición / borrado)

- [x] **Test (RED):** token válido → altas y ediciones con `200` + `getetag`; borrados con `<d:status>HTTP/1.1 404 Not Found</d:status>` **a nivel de `response`**, sin `propstat` (RFC 6578 §3.2) (tombstone).
- [x] **Test (RED):** sin cambios desde el token → `207` sin propstats de cambios y `<d:sync-token>` actual.
- [x] **GREEN:** rama incremental del handler.
- [x] **REFACTOR:** clippy; asegurar `sync-level 1` por defecto.

### Round-trip de token y errores

- [x] **Test (RED):** `parse_sync_token(format_sync_token(n)) == n` para varios `n`, y prefijo/ formato inválidos producen `Err`.
- [x] **Test (RED):** token malformado o desconocido en `REPORT` → **`403 valid-sync-token`**.
- [x] **Test (RED):** `sync-level infinite` → **`400 Bad Request`**.
- [x] **GREEN:** implementar helpers de token y las ramas de error del handler.
- [x] **REFACTOR:** clippy.

### Auth y rutas

- [x] **Test (RED):** usuario sin acceso a la colección → `403`/`404` según el modelo de autorización DAV existente; `REPORT` autenticado correcto → `207`.
- [x] **Test (RED):** `REPORT` alcanzable tanto por CalDAV como por CardDAV sobre sus rutas canónicas.
- [x] **GREEN:** registrar la ruta `REPORT` en `src/dav/caldav.rs` y `src/dav/carddav.rs`.
- [x] **REFACTOR:** verificar que no hay lógica duplicada entre ambos.

### Cierre

- [x] **Test:** `cargo test` en verde.
- [x] **Calidad:** `cargo fmt --check` y `cargo clippy -- -D warnings` sin warnings.
- [x] **Documentación:** consolidar en `docs/F09-sync-collection.md` con el mismo slug.

## Criterios de aceptación

- `sync_log` existe (migración `0003`) con CHECKs, ambos índices y `ON DELETE CASCADE` solo desde `collections`.
- Cada mutación de objeto escribe su entrada `sync_log` en la **misma transacción**; un fallo de la mutación no deja log ni avanza el token.
- El borrado de un recurso produce un tombstone que **sobrevive** al objeto y se reporta como `404` en el barrido incremental.
- `REPORT` sin token devuelve sync inicial (todos los recursos + token actual); con token devuelve solo `sync_token > N`.
- `format_sync_token`/`parse_sync_token` hacen round-trip exacto; token inválido/desconocido → `403 valid-sync-token`.
- `sync-level 1` soportado; `infinite` → `400`.
- La lógica compartida vive en `common.rs`; CalDAV y CardDAV la consumen sin duplicar.
- `cargo test` verde, `cargo fmt --check` y `cargo clippy -- -D warnings` limpios.

## Riesgos

- **Crecimiento del log (sin poda):** el MVP conserva todo `sync_log`; en colecciones muy activas puede crecer indefinidamente. Mitigación futura: poda por retención/antigüedad y compactación de historial obsoleto. Documentado, fuera de alcance.
- **Atomicidad de la escritura del log:** olvidar enganchar `record_change` en algún write-path de F03 haría perder cambios en el sync. Cobertura: tests que verifican el log tras cada mutación y en el rollback.
- **Elección `403` vs `409`:** se fija `403 valid-sync-token`; si un cliente espera otro código, revisar compatibilidad en pruebas reales con DAVx⁵ / iOS / Thunderbird.
- **Semántica de tombstones en cascada:** borrar una colección elimina sus logs (no reportables); verificar que esto no confunde a clientes que sincronizan colecciones eliminadas.
- **Deduplicación por recurso:** `changes_since` debe devolver solo el último cambio por recurso; un `added` seguido de `deleted` en el mismo intervalo debe reportarse como `deleted`.

## Referencias

- `plans/PLAN-001-general.md` — feature **F09** (sync-collection RFC 6578).
- `docs/F03-data-layer.md` — capa de datos, transacciones, `collections.sync_token`.
- `docs/F07-caldav-core.md` — CalDAV, `multistatus`, `DavError`, PROPFIND/REPORT.
- `docs/F08-carddav-core.md` — CardDAV sobre helpers compartidos.
- `AGENTS.md` — stack (Rust edition 2024 + Axum + sqlx/SQLite), layout `src/dav/*`, `common.rs`.
- RFC 6578 — Collection Synchronization for WebDAV.
