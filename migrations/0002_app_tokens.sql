-- migrations/0002_app_tokens.sql
-- F05: app tokens revocables para clientes DAV nativos (Basic Auth).

CREATE TABLE IF NOT EXISTS app_tokens (
    id            TEXT    PRIMARY KEY,   -- parte PÚBLICA del token: oxidav_<id>_<secret>
    user_id       TEXT    NOT NULL,      -- claim `sub` de Pocket ID (identidad real)
    name          TEXT    NOT NULL,      -- etiqueta legible: "iPhone", "DAVx5", "Thunderbird"
    secret_hash   TEXT    NOT NULL,      -- SHA-256/HMAC-SHA256 (hex) del <secret>; NUNCA el secret en claro
    created_at    INTEGER NOT NULL,      -- epoch en segundos
    last_used_at  INTEGER,               -- epoch en segundos; NULL si nunca se ha usado
    revoked_at    INTEGER                -- epoch en segundos; NULL = activo
);

-- Búsqueda de tokens por usuario (listado / "mis dispositivos").
CREATE INDEX IF NOT EXISTS idx_app_tokens_user_id
    ON app_tokens (user_id);

-- Filtrado de tokens activos por usuario (listado y chequeos de estado).
CREATE INDEX IF NOT EXISTS idx_app_tokens_user_active
    ON app_tokens (user_id, revoked_at);
