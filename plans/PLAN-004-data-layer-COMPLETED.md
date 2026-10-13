# PLAN-004 — F03 Capa de datos

## Objetivo

Implementar la capa de persistencia de OxiDav: un pool SQLite asíncrono con los
PRAGMAs de rendimiento y durabilidad, un conjunto de migraciones versionadas que
crean el esquema (`collections`, `calendar_objects`, `card_objects`) y dos índices
FTS5 externos con tokenizer `trigram` mantenidos por triggers. Expone una API
interna reutilizable (creación de pool + funciones de búsqueda con ranking `bm25`)
sobre la que construirán F06–F10. No contiene handlers HTTP, autenticación ni DAV.

## Alcance

**Dentro:**

- Módulo `src/db/` con `mod.rs` (pool, PRAGMAs, `DbError`), `models.rs` (structs de
  fila/DTO internos) y `search.rs` (consultas FTS5 con `bm25`).
- Helpers de migración y de creación del pool (`init_pool`, aplicación de PRAGMAs).
- Ficheros SQL versionados en `migrations/` (una migración inicial con todo el esquema).
- Esquema completo: `collections`, `calendar_objects`, `card_objects`, sus índices de
  apoyo y las FTS externas `events_fts` / `contacts_fts`.
- Triggers `AFTER INSERT/DELETE/UPDATE` que mantienen ambas FTS externas (patrón
  external-content correcto).
- API interna: `init_pool(database_url) -> Result<SqlitePool, DbError>`, `DbError`,
  repositorios mínimos de colecciones/objetos y `search_events` / `search_contacts`.
- Tests (unitarios + integración) que cubren PRAGMAs, migraciones, FTS5/trigram,
  triggers, `bm25`, cascade delete y unicidad.

**Fuera (explícito):**

- Endpoints HTTP `/api/v1/...` y `/dav/...` (F06–F09).
- Autenticación OIDC/Bearer y app tokens (F04/F05).
- Parseo de `.ics` / `.vcf`, RRULE y serialización DAV (F07/F08).
- Vista SPA, DTOs públicos REST y routing de búsqueda expuesta (F10/F11+).
- Composición del pool en `AppState` y wiring en `main.rs` (se hará cuando F04/F06 lo consuman).

## Decisiones técnicas

- **Driver: `sqlx` con SQLite.** `sqlx` es asíncrono y encaja de forma natural con
  Tokio/Axum: el pool (`SqlitePool`) es `Send + Sync + Clone`, se comparte en el
  `AppState` sin mutex ni `spawn_blocking`, y permite `await` directo en los handlers
  futuros. La especificación ya usa `SqliteConnectOptions`, que es la vía de `sqlx`
  para configurar PRAGMAs y el modo de apertura. `rusqlite` es síncrono: obligaría a
  envolver cada acceso en `tokio::task::spawn_blocking` o en un pool de hilos, con
  más fricción y peor ergonomía en un servidor async. Se elige `sqlx`.
- **Versión objetivo:** `sqlx` 0.8.x (alineada con el ecosistema actual). Features:
  `sqlite`, `runtime-tokio`, `macros` y `migrate` (esta última para `sqlx::migrate!`).
- **SQLite empaquetado:** se habilita el enlace estático de SQLite (feature
  transitiva `sqlite` de `sqlx` que activa `libsqlite3-sys` con `bundled`) para
  garantizar un build con **FTS5 y el tokenizer `trigram`** habilitados, sin depender
  del SQLite del sistema. La versión empaquetada (3.45+) incluye FTS5 `trigram`.
- **Migraciones:** ficheros SQL versionados `migrations/0001_init.sql` gestionados por
  `sqlx::migrate!` (macro) o `sqlx::migrate::Migrator`, aplicados en `init_pool` de
  forma idempotente. Una sola migración inicial contiene el esquema completo.
- **DSN / variable de entorno:** `OXIDAV_DATABASE_URL`, ya definida en F01 con default
  `sqlite://data/oxidav.db`. `init_pool` normaliza opciones vía
  `SqliteConnectOptions::from_str(url)`.
- **Política de `data/`:** el DSN usa una ruta relativa; el directorio `data/` puede no
  existir en un checkout limpio. `init_pool` garantiza su creación (`create_dir_all`)
  antes de abrir la conexión cuando la URL apunta a un fichero local, y falla con
  `DbError` claro si no puede crearlo. No se versiona contenido de `data/` (ignorado
  en `.gitignore`).

## Prerrequisito / Spike FTS5+trigram

**Punto de entrada TDD: este test debe pasar ANTES de escribir cualquier otra cosa.**

Test de humo, en `src/db/mod.rs` (o `tests/fts_spike.rs`) contra una base en memoria
(`sqlite::memory:` o fichero temporal):

1. Crear una tabla virtual con el tokenizer:
   ```sql
   CREATE VIRTUAL TABLE fts_probe USING fts5(body, tokenize='trigram');
   ```
2. Insertar filas y consultar por substrings de **≥3 caracteres** (p. ej.
   `WHERE fts_probe MATCH 'xampl'` sobre `"un ejemplo"`), verificando que encuentra
   coincidencias internas (comportamiento substring propio de `trigram`).
3. Confirmar que el ranking `bm25(fts_probe)` está disponible.

Si el SQLite empaquetado **no** trae FTS5/`trigram`, plan B documentado y ordenado por
preferencia:

- **(B1)** Forzar el build bundled correcto: revisar `Cargo.lock`/features para que
  `libsqlite3-sys` se compile con `bundled` y FTS5 activado (`SQLITE_ENABLE_FTS5`); es
  habitual que falte solo activar la feature.
- **(B2)** Cambiar a `rusqlite` con feature `bundled` para la capa de datos, aceptando
  `spawn_blocking` en la API (implica reevaluar la decisión de driver en este plan).
- **(B3)** Documentar como requisito de sistema: compilar OxiDav contra un `libsqlite3`
  del sistema con FTS5 habilitado (último recurso; rompe el binario autocontenido).

La decisión final se registra en `docs/F03-data-layer.md`.

## PRAGMAs

Lista exacta a aplicar en cada conexión del pool mediante
`SqliteConnectOptions` y/o un `after_connect` que ejecute `PRAGMA`:

| PRAGMA | Valor | Motivo |
|---|---|---|
| `journal_mode` | `WAL` | Lecturas concurrentes con escrituras; durabilidad y rendimiento. |
| `synchronous` | `NORMAL` | Compromiso entre seguridad y velocidad sobre WAL. |
| `temp_store` | `MEMORY` | Temporales (incl. índices FTS) en RAM. |
| `mmap_size` | `268435456` | 256 MiB de I/O mapeado en memoria. |
| `foreign_keys` | `ON` | Habilita FK y `ON DELETE CASCADE`. |
| `busy_timeout` | `5000` | 5 s de espera ante bloqueo por escritor concurrente. |

Cómo aplicarlos con `sqlx`:

- `journal_mode`, `synchronous`, `foreign_keys`, `busy_timeout` se configuran en el
  builder de `SqliteConnectOptions` (`.journal_mode(SqliteJournalMode::Wal)`,
  `.synchronous(SqliteSynchronous::Normal)`, `.foreign_keys(true)`,
  `.busy_timeout(Duration::from_millis(5000))`).
- `temp_store` y `mmap_size` no tienen setter tipado en todas las versiones: se aplican
  con `.pragma("temp_store", "MEMORY")` / `.pragma("mmap_size", "268435456")` o vía un
  hook `after_connect` que ejecute `PRAGMA temp_store = MEMORY;` y
  `PRAGMA mmap_size = 268435456;`.
- Nota WAL: `journal_mode=WAL` es persistente en el fichero (no por conexión), se
  aplica una vez pero es inocuo reaplicarlo.

## Esquema y migraciones

Migración inicial `migrations/0001_init.sql` (esquema completo):

```sql
PRAGMA foreign_keys = ON;

-- Colecciones: calendarios y libretas de direcciones del usuario.
CREATE TABLE collections (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    name        TEXT NOT NULL,
    type        TEXT NOT NULL CHECK (type IN ('calendar', 'addressbook')),
    color       TEXT NOT NULL DEFAULT '#1677ff',
    sync_token  INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- Objetos de calendario: .ics crudo + columnas desnormalizadas para consulta.
CREATE TABLE calendar_objects (
    id            TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    uid           TEXT NOT NULL,
    summary       TEXT,
    description   TEXT,
    dtstart       INTEGER,
    dtend         INTEGER,
    rrule         TEXT,
    etag          TEXT NOT NULL,
    raw_payload   TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

-- Objetos de contactos: .vcf crudo + columnas desnormalizadas para consulta.
CREATE TABLE card_objects (
    id            TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    uid           TEXT NOT NULL,
    fn            TEXT,
    email         TEXT,
    phone         TEXT,
    organization  TEXT,
    notes         TEXT,
    etag          TEXT NOT NULL,
    raw_payload   TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

-- Índices de apoyo.
CREATE INDEX idx_calendar_objects_collection ON calendar_objects(collection_id);
CREATE INDEX idx_calendar_objects_uid        ON calendar_objects(uid);
CREATE INDEX idx_calendar_objects_etag        ON calendar_objects(etag);
CREATE INDEX idx_calendar_objects_dtstart     ON calendar_objects(dtstart);

CREATE INDEX idx_card_objects_collection ON card_objects(collection_id);
CREATE INDEX idx_card_objects_uid        ON card_objects(uid);
CREATE INDEX idx_card_objects_etag       ON card_objects(etag);
CREATE INDEX idx_card_objects_fn         ON card_objects(fn);

-- Unicidad DAV: un mismo UID no puede repetirse dentro de una colección.
CREATE UNIQUE INDEX uq_calendar_objects_collection_uid
    ON calendar_objects(collection_id, uid);
CREATE UNIQUE INDEX uq_card_objects_collection_uid
    ON card_objects(collection_id, uid);

-- FTS externas (content-based) con tokenizer trigram.
CREATE VIRTUAL TABLE events_fts USING fts5(
    summary,
    description,
    content='calendar_objects',
    content_rowid='rowid',
    tokenize='trigram'
);

CREATE VIRTUAL TABLE contacts_fts USING fts5(
    fn,
    email,
    phone,
    organization,
    notes,
    content='card_objects',
    content_rowid='rowid',
    tokenize='trigram'
);

-- Triggers de mantenimiento: calendar_objects -> events_fts.
CREATE TRIGGER calendar_objects_ai AFTER INSERT ON calendar_objects BEGIN
    INSERT INTO events_fts(rowid, summary, description)
    VALUES (new.rowid, new.summary, new.description);
END;

CREATE TRIGGER calendar_objects_ad AFTER DELETE ON calendar_objects BEGIN
    INSERT INTO events_fts(events_fts, rowid, summary, description)
    VALUES ('delete', old.rowid, old.summary, old.description);
END;

CREATE TRIGGER calendar_objects_au AFTER UPDATE ON calendar_objects BEGIN
    INSERT INTO events_fts(events_fts, rowid, summary, description)
    VALUES ('delete', old.rowid, old.summary, old.description);
    INSERT INTO events_fts(rowid, summary, description)
    VALUES (new.rowid, new.summary, new.description);
END;

-- Triggers de mantenimiento: card_objects -> contacts_fts.
CREATE TRIGGER card_objects_ai AFTER INSERT ON card_objects BEGIN
    INSERT INTO contacts_fts(rowid, fn, email, phone, organization, notes)
    VALUES (new.rowid, new.fn, new.email, new.phone, new.organization, new.notes);
END;

CREATE TRIGGER card_objects_ad AFTER DELETE ON card_objects BEGIN
    INSERT INTO contacts_fts(contacts_fts, rowid, fn, email, phone, organization, notes)
    VALUES ('delete', old.rowid, old.fn, old.email, old.phone, old.organization, old.notes);
END;

CREATE TRIGGER card_objects_au AFTER UPDATE ON card_objects BEGIN
    INSERT INTO contacts_fts(contacts_fts, rowid, fn, email, phone, organization, notes)
    VALUES ('delete', old.rowid, old.fn, old.email, old.phone, old.organization, old.notes);
    INSERT INTO contacts_fts(rowid, fn, email, phone, organization, notes)
    VALUES (new.rowid, new.fn, new.email, new.phone, new.organization, new.notes);
END;
```

**Gotcha external-content (crítico):** con `content='...'` la FTS no almacena el texto;
el índice se sincroniza con triggers. En `DELETE` y en la primera mitad de `UPDATE` hay
que emitir la fila especial `INSERT INTO <fts>(<fts>, rowid, <cols>) VALUES('delete',
old.rowid, old.<cols>)` con los valores **antiguos** exactos; si no, el índice se
corrompe (quedan tokens huérfanos y la búsqueda devuelve falsos positivos).
`content_rowid='rowid'` enlaza la FTS con la fila física de la tabla de contenido.

## Contratos / Interfaces

**Pool y errores (`src/db/mod.rs`):**

- `pub type Db = SqlitePool;` (alias interno reutilizable).
- `pub async fn init_pool(database_url: &str) -> Result<SqlitePool, DbError>` — crea el
  directorio `data/` si procede, construye `SqliteConnectOptions` con los PRAGMAs,
  abre el pool y aplica las migraciones. Devuelve el pool listo para compartir.
- `pub enum DbError` con `#[derive(thiserror::Error)]` e implementación de `Display`:
  variantes mínimas `Io` (creación de directorio), `Connect(sqlx::Error)`,
  `Migrate(sqlx::migrate::MigrateError)` y `Query(sqlx::Error)`. Implementa
  `std::error::Error` vía `thiserror`.

**Modelos (`src/db/models.rs`):** structs de fila espejo del esquema (`Collection`,
`CalendarObject`, `CardObject`) con `FromRow` de `sqlx`, más tipos de entrada para
inserción/actualización. No son DTOs públicos REST (eso es F06).

**Repositorios mínimos (`src/db/mod.rs`):**

- Colecciones: `create_collection`, `list_collections(user_id)`, `delete_collection(id)`
  (el borrado debe propagar a objetos por `ON DELETE CASCADE`).
- `calendar_objects`: `insert_calendar_object`, `update_calendar_object`,
  `delete_calendar_object`; el `etag` se calcula en la escritura (p. ej. hash del
  `raw_payload` + `updated_at`, o contador; se documenta el algoritmo elegido).
- `card_objects`: `insert_card_object`, `update_card_object`, `delete_card_object` con
  la misma lógica de `etag`.
- Cada escritura actualiza `updated_at` y avanza `collections.sync_token` de la
  colección afectada (base para F09).

**Búsqueda (`src/db/search.rs`):**

- `pub async fn search_events(pool: &SqlitePool, query: &str, limit: i64)
  -> Result<Vec<CalendarObject>, DbError>` — consulta `events_fts` con
  `WHERE events_fts MATCH ?` y `ORDER BY bm25(events_fts) ASC LIMIT ?`.
- `pub async fn search_contacts(pool: &SqlitePool, query: &str, limit: i64)
  -> Result<Vec<CardObject>, DbError>` — análoga sobre `contacts_fts`.
- **Orden de ranking:** en FTS5, `bm25()` devuelve valores donde **más negativo = más
  relevante**; por eso el orden correcto es `ORDER BY bm25(fts) ASC` (el más relevante
  primero).
- **Mínimo de 3 caracteres:** el tokenizer `trigram` indexa y consulta por trigramas, de
  modo que consultas de longitud < 3 caracteres no pueden casar. Comportamiento
  definido: `query` con menos de 3 caracteres (tras `trim`) devuelve `Ok(vec![])` sin
  tocar la base de datos. Se documenta como limitación conocida de la UX de búsqueda.

## Tareas (TDD)

- [x] **Paso 1 — Spike FTS5/trigram (RED→GREEN).** Test que crea
      `CREATE VIRTUAL TABLE ... USING fts5(..., tokenize='trigram')`, inserta y consulta
      substrings de ≥3 chars, y usa `bm25`. Debe pasar antes de continuar. Añadir deps
      `sqlx` con features `sqlite`, `runtime-tokio`, `macros`, `migrate` y SQLite bundled.
- [x] **Paso 2 — PRAGMAs aplicados.** Test: tras `init_pool`, `PRAGMA journal_mode`
      devuelve `wal`; verificar además `synchronous=1` (`NORMAL`), `foreign_keys=1`,
      `temp_store=2` (`MEMORY`), `mmap_size=268435456` y `busy_timeout=5000`.
- [x] **Paso 3 — Migración crea tablas e índices.** Test: aplicar `migrations/0001_init.sql`;
      comprobar existencia de `collections`, `calendar_objects`, `card_objects`,
      `events_fts`, `contacts_fts` y de todos los índices (incl. los dos `UNIQUE`).
- [x] **Paso 4 — Cascade delete.** Test: insertar colección + objetos, borrar la
      colección y comprobar que sus `calendar_objects`/`card_objects` desaparecen.
- [x] **Paso 5 — Triggers INSERT.** Test: insertar objeto y comprobar que aparece en
      `events_fts`/`contacts_fts` (consulta `MATCH` por substring).
- [x] **Paso 6 — Triggers UPDATE (delete+insert).** Test: actualizar `summary`/`fn`;
      verificar que el índice refleja el valor nuevo, que el antiguo ya no casa y que no
      hay duplicados (`COUNT` de coincidencias estable).
- [x] **Paso 7 — Triggers DELETE.** Test: borrar objeto y comprobar que sus tokens
      desaparecen del índice (sin falsos positivos).
- [x] **Paso 8 — Búsqueda con bm25 y substring trigram.** Test: varias filas con distinta
      relevancia; verificar que `search_events`/`search_contacts` casan substrings internos
      y devuelven en orden `bm25 ASC` (más relevante primero); `query` < 3 chars → `[]`.
- [x] **Paso 9 — Unicidad `(collection_id, uid)`.** Test: segundo insert con mismo
      `collection_id`+`uid` → error de constraint; distinto `collection_id` o distinto
      `uid` → OK.
- [x] **Paso 10 — Calidad.** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
      `cargo test` en verde.

## Criterios de aceptación

- [x] El spike FTS5+`trigram` pasa: FTS5 disponible y consultas por substring de ≥3 chars
      funcionan con `bm25`.
- [x] `init_pool` aplica los 6 PRAGMAs; `PRAGMA journal_mode` devuelve `wal` a través del pool.
- [x] Las migraciones crean el esquema completo (3 tablas + 2 FTS externas + índices, incl.
      los dos `UNIQUE`) de forma idempotente.
- [x] `ON DELETE CASCADE` borra los objetos de una colección eliminada.
- [x] Los triggers mantienen `events_fts` y `contacts_fts` en INSERT, UPDATE (delete+insert,
      sin duplicados ni valores obsoletos) y DELETE, siguiendo el patrón external-content.
- [x] `search_events`/`search_contacts` ordenan por `bm25 ASC` (más relevante primero) y
      devuelven `[]` para consultas de < 3 caracteres.
- [x] La unicidad `(collection_id, uid)` se impone en ambas tablas de objetos.
- [x] `cargo test` verde y `cargo clippy --all-targets -- -D warnings` / `cargo fmt --check`
      sin warnings.
- [x] No hay `unwrap`/`expect`/`panic` en código de producción de la capa de datos.

## Riesgos

- **Disponibilidad de FTS5/`trigram` en el SQLite empaquetado:** mitigado por el spike como
  primer paso y por el plan B (forzar `bundled`, `rusqlite`, o requisito de sistema).
- **Coste de reindexado (trigram):** el índice trigram es voluminoso y cada escritura dispara
  los triggers; en inserciones masivas puede ser lento. Aceptable en escala personal;
  vigilar `mmap_size` y `temp_store`.
- **Colisión de `rowid` en external-content:** el mapeo FTS↔tabla depende de `rowid`; un
  `VACUUM` o un `INSERT ... ON CONFLICT` mal planteado podría desincronizar el índice. Hay
  que mantener el patrón `'delete'` correcto y evitar reescribir `rowid`.
- **Concurrencia de escritura con WAL y sqlx async:** WAL permite un solo escritor a la vez;
  bajo carga concurrente, `busy_timeout=5000` evita `SQLITE_BUSY` espurios pero no lo elimina
  del todo. Revisar tamaño de pool y serialización de escrituras si aparecen bloqueos.

## Referencias

- [`plans/PLAN-001-general.md`](./PLAN-001-general.md) (feature F03, dependencias y riesgo FTS5).
- [`AGENTS.md`](../AGENTS.md) (PRAGMAs, esquema, gotcha external-content, triggers, FTS5 trigram).
- [`docs/F01-scaffolding-backend.md`](../docs/F01-scaffolding-backend.md) (`OXIDAV_DATABASE_URL`, `Config`).
