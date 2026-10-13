# F03 — Capa de datos

## Qué hace

Implementa la persistencia de OxiDav sobre **SQLite asíncrono** (`sqlx`): un pool
`SqlitePool` configurado con los PRAGMAs de rendimiento y durabilidad (WAL), una
migración versionada que crea el esquema DAV (tablas, índices y dos índices FTS5
externos con tokenizer `trigram` mantenidos por triggers) y una **API interna
reutilizable** (repositorios mínimos + búsqueda con ranking `bm25`). Es la base
sobre la que construyen F06–F10. No contiene handlers HTTP, autenticación ni DAV, y
**no** se cablea todavía en `AppState`/`main.rs`.

## Decisiones técnicas

- **Driver `sqlx 0.8.6`** async con features `sqlite`, `runtime-tokio`, `macros` y
  `migrate`. El pool `SqlitePool` es `Send + Sync + Clone` y encaja con Tokio/Axum
  sin `spawn_blocking`.
- **SQLite bundled** (`libsqlite3-sys 0.30.1`): la feature `sqlite` activa el enlace
  estático con `-DSQLITE_ENABLE_FTS5`, de modo que **FTS5 + tokenizer `trigram`**
  quedan garantizados sin depender del SQLite del sistema. Verificado por el spike.
- **Migraciones versionadas** en `migrations/` gestionadas con `sqlx::migrate!`,
  idempotentes. Una sola migración inicial (`0001_init.sql`) contiene todo el esquema.
- **`data/` no se versiona:** el DSN por defecto es `sqlite://data/oxidav.db` y el
  directorio se crea en `init_pool` si procede; `/data` está en `.gitignore`.

## PRAGMAs aplicados

Se aplican en cada conexión del pool vía `SqliteConnectOptions` (setters tipados donde
existen) y `SqliteConnectOptions::pragma`:

| PRAGMA | Valor | Motivo |
|---|---|---|
| `journal_mode` | `WAL` | Lecturas concurrentes con escrituras. |
| `synchronous` | `NORMAL` | Compromiso seguridad/velocidad sobre WAL. |
| `temp_store` | `MEMORY` | Temporales (incl. índices FTS) en RAM. |
| `mmap_size` | `268435456` | 256 MiB de I/O mapeado en memoria. |
| `foreign_keys` | `ON` | Habilita FK y `ON DELETE CASCADE`. |
| `busy_timeout` | `5000` | 5 s de espera ante bloqueo por escritor concurrente. |

## Esquema

- Tablas `collections`, `calendar_objects` y `card_objects`. Las dos últimas guardan
  el `.ics` / `.vcf` crudo en la columna `raw_payload`, más `etag`, `uid` y los
  timestamps `created_at` / `updated_at` (Unix en milisegundos).
- Índices de apoyo (`collection_id`, `uid`, `etag`, `dtstart` / `fn`) y un
  `UNIQUE(collection_id, uid)` por tabla de objetos, que impone la unicidad DAV de
  UID dentro de cada colección.
- **FTS externas** `events_fts` (sobre `calendar_objects`) y `contacts_fts` (sobre
  `card_objects`), declaradas con `content=...`, `content_rowid='rowid'` y
  `tokenize='trigram'`. Al ser external-content no almacenan el texto: el índice se
  sincroniza con triggers.

## Mantenimiento del índice (gotcha)

Con `content='<tabla>'` la FTS5 es *external-content*: no guarda copia del texto y su
índice se mantiene desde triggers `AFTER INSERT/DELETE/UPDATE` (6 en total, 3 por
tabla). El patrón correcto exige que, **en el DELETE y en la primera mitad del
UPDATE**, se emita la fila especial

```sql
INSERT INTO <fts>(<fts>, rowid, <cols>) VALUES('delete', old.rowid, old.<cols>);
```

usando los valores **ANTIGUOS** exactos. Si se omite o se usan los valores nuevos,
quedan tokens huérfanos en el índice y la búsqueda devuelve falsos positivos (el
`rowid` deja de estar sincronizado con la fila física). `content_rowid='rowid'` enlaza
la FTS con la fila de la tabla de contenido.

## API interna

- `pub type Db = SqlitePool;` — alias interno reutilizable.
- `pub async fn init_pool(database_url: &str) -> Result<SqlitePool, DbError>` — crea
  el directorio de datos si procede, construye `SqliteConnectOptions` con los PRAGMAs,
  abre el pool y aplica las migraciones (idempotente).
- `pub enum DbError` (`thiserror`): `Io` (creación de directorio), `Connect` /
  `InvalidUrl` (apertura del pool), `Migrate` (`MigrateError`) y `Query` (`sqlx::Error`).
- `models.rs`: structs espejo del esquema (`Collection`, `CalendarObject`,
  `CardObject`) con `FromRow`, más tipos de entrada para inserción/actualización. No
  son DTOs públicos REST (eso es F06).
- Repositorios: `create_collection`, `list_collections`, `delete_collection` (propaga
  a objetos por `ON DELETE CASCADE`), e `insert_` / `update_` / `delete_` de objetos de
  calendario y de contactos, con cálculo de `etag` (FNV-1a de 64 bits sobre
  `raw_payload` + `updated_at`).
- **Atomicidad:** cada mutación de objeto y el avance de
  `collections.sync_token = sync_token + 1` ocurren en la **misma transacción**
  (coherencia con RFC 6578). Un fallo de escritura no avanza el token.
- Búsqueda: `search_events` / `search_contacts` con `WHERE fts MATCH ?` y
  `ORDER BY bm25(fts) ASC` (en FTS5 más negativo = más relevante, así que el más
  relevante queda primero).

## Límites y decisiones de búsqueda

- **Tokenizer `trigram`:** mínimo **3 caracteres** (una consulta de 3 sí casa, de 2
  no). Una query con menos de 3 chars tras `trim` devuelve `Ok(vec![])` sin tocar la
  base de datos.
- **`limit` clampeado:** `<= 0 → 1` (nunca el `LIMIT -1` de SQLite, que significa
  *ilimitado*), y `> MAX_SEARCH_LIMIT (200) → 200`.

## Estructura de ficheros

| Fichero | Rol |
|---|---|
| `src/db/mod.rs` | Pool, PRAGMAs, `Db`, `DbError`, repositorios, helpers (`etag`, `sync_token`) |
| `src/db/models.rs` | Structs espejo del esquema + tipos de entrada |
| `src/db/search.rs` | `search_events` / `search_contacts` (FTS5 + `bm25`) |
| `migrations/0001_init.sql` | Migración inicial: esquema, índices, FTS externas y triggers |
| `tests/fts_spike.rs` | Spike FTS5/trigram (bloqueante) |
| `tests/data_layer.rs` | Tests de PRAGMAs, esquema, triggers, cascade, búsqueda y unicidad |

## Cómo verificar

- `cargo test` → **37 tests** (14 lib + 2 main + 18 data_layer + 1 spike + 2 router).
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt --check`

## Notas

- El pool aún **no** está cableado en `main.rs` / `AppState` (se hará cuando F04/F06 lo
  consuman).
- `PRAGMA foreign_keys` dentro de la migración es documental: es un no-op dentro de la
  transacción de `sqlx`; la garantía real la da el `.foreign_keys(true)` aplicado por
  el pool.

## Relacionado

- [`plans/PLAN-004-data-layer-COMPLETED.md`](../plans/PLAN-004-data-layer-COMPLETED.md)
- [`plans/PLAN-001-general.md`](../plans/PLAN-001-general.md)
- [`docs/F01-scaffolding-backend.md`](./F01-scaffolding-backend.md) (`OXIDAV_DATABASE_URL`, `Config`)
- [`AGENTS.md`](../AGENTS.md) (PRAGMAs, esquema, gotcha external-content, FTS5 trigram)
