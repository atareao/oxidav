-- migrations/0003_sync_log.sql
-- F09: log de cambios (tombstones) para WebDAV sync-collection (RFC 6578).
--
-- Cada mutación de objeto escribe una fila aquí en la MISMA transacción que la
-- mutación y el bump de `collections.sync_token` (ver los write-paths de F03),
-- de modo que `sync-token` y log nunca se desincronizan.
--
-- `collection_id` referencia `collections(id)` (TEXT, como el resto del
-- esquema) con ON DELETE CASCADE: al borrar una colección desaparecen sus
-- entradas de log junto con sus objetos. NO hay FK hacia `calendar_objects` ni
-- `card_objects` a propósito: los tombstones (`change = 'deleted'`) deben
-- SOBREVIVIR al borrado del objeto para poder reportarlo en el barrido
-- incremental.

CREATE TABLE IF NOT EXISTS sync_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    collection_id TEXT    NOT NULL,
    resource_uid  TEXT    NOT NULL,
    resource_kind TEXT    NOT NULL CHECK (resource_kind IN ('calendar', 'card')),
    change        TEXT    NOT NULL CHECK (change IN ('added', 'updated', 'deleted')),
    sync_token    INTEGER NOT NULL,
    changed_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE
);

-- Barrido incremental: cambios de una colección con sync_token > N, en orden.
CREATE INDEX IF NOT EXISTS idx_sync_log_collection_token
    ON sync_log (collection_id, sync_token);

-- Deduplicación / último cambio por recurso dentro de una colección.
CREATE INDEX IF NOT EXISTS idx_sync_log_collection_uid
    ON sync_log (collection_id, resource_uid, sync_token);
