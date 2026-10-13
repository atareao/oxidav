-- NOTE: foreign-key enforcement is NOT set here. `PRAGMA foreign_keys` is a
-- no-op inside a migration transaction (it is per-connection and
-- transaction-scoped); it would silently mislead. It is instead enabled on every
-- pool connection by `init_pool` via `SqliteConnectOptions::foreign_keys(true)`,
-- which is what makes `ON DELETE CASCADE` effective at runtime.

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
