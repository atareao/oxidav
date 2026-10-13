# AGENTS.md — OxiDav

Servidor personal **CalDAV / CardDAV / WebDAV** con SPA de gestión.

**Estado: greenfield.** El repo solo contiene `Cargo.toml` (edition 2024, sin deps),
`src/main.rs` (hello world) y `.gitignore`. No hay `web/`, ni plan, ni commits.
Todo se scaffoldea desde cero según la especificación acordada.

La especificación completa vive fuera del repo; este archivo resume las
decisiones **vinculantes** que un agente no puede deducir del código actual.

## Stack (decisiones no obvias)
- **Backend:** Rust **edition 2024** + Axum + Tokio. Un único binario distribuible:
  los assets de React se embeben con `rust-embed`.
- **Frontend:** React 19 + **Ant Design v6** + Vite + TypeScript, en `web/`.
  Se compila a `web/dist/`.
- **Identidad:** **Pocket ID** (OIDC) es el único IdP. OxiDav **no almacena
  contraseñas principales**.
  - SPA: Authorization Code + PKCE → `Authorization: Bearer <JWT>`, validado
    contra el JWKS público de Pocket ID (`/.well-known/openid-configuration`).
  - Clientes DAV nativos (iOS, DAVx⁵, Thunderbird): **Basic Auth** con *app tokens*
    revocables (hash local en SQLite).
  - El `user_id` en BD es el claim `sub` de Pocket ID.
- **Búsqueda:** **SQLite FTS5 con tokenizer `trigram`** y ranking `bm25`.
  Sin índice externo ni sincronización de índices desde Rust.

## SQLite (PRAGMAs, esquema y gotchas)
- PRAGMAs obligatorios: `journal_mode=WAL`, `synchronous=NORMAL`,
  `temp_store=MEMORY`, `mmap_size=268435456`.
- Tablas: `collections`, `calendar_objects` (`.ics` crudo en `raw_payload`),
  `card_objects` (`.vcf` crudo en `raw_payload`). Cada fila guarda `etag`, `uid`,
  `created_at`, `updated_at`; `collections.sync_token` es incremental.
- FTS externas: `events_fts` (`content='calendar_objects'`) y `contacts_fts`
  (`content='card_objects'`).
- **Gotcha FTS5 external-content:** el índice se mantiene con triggers
  `AFTER INSERT/DELETE/UPDATE`; el delete debe emitir
  `INSERT INTO <fts>(<fts>, rowid, ...) VALUES('delete', ...)` con los valores
  *antiguos* antes de reinsertar. Sin esto la búsqueda se corrompe.
- El tokenizer `trigram` exige SQLite con FTS5 habilitado → usa `rusqlite` con
  feature `bundled`. Requiere ≥3 caracteres y hace substring matching.

## Protocolos y rutas
- DAV en `/dav/...`: `PROPFIND`, `REPORT`, `PUT`, `DELETE`, `MKCALENDAR`;
  respuestas XML `multistatus` con `quick-xml`; soporte **RFC 6578
  (sync-collection)** con `sync-token` incremental.
- URLs canónicas: `/dav/calendars/{username}/` y `/dav/addressbooks/{username}/`.
- REST JSON para la SPA en `/api/v1/...` con DTOs limpios: el cliente nunca
  parsea `.ics` ni `.vcf`.

## Layout esperado
- `src/`: `auth/{oidc,app_tokens}`, `db/{mod,search,models}`, `dav/`, `api/`.
- `web/`: Vite + React; OIDC en `web/src/services/auth.ts`.

## Planes y documentación
- `plans/` — un fichero por plan. Nombrado `PLAN-XXX-<slug>.md` (XXX = número
  secuencial). Al completarse un plan se renombra a `PLAN-XXX-<slug>-COMPLETED.md`
  (no se borra: queda como histórico).
- `docs/` — documentación de cada característica/feature añadida. Un fichero por
  feature, con el mismo slug que su plan de origen.
- Un plan nuevo se registra en `plans/` **antes** de implementar (ver flujo
  OpenSpec/TDD). Al cerrar la feature, el plan pasa a `-COMPLETED.md` y su
  documentación se consolida en `docs/`.

## Comandos
El workflow OpenSpec (SDD) + TDD y los comandos base por stack están definidos en
`~/.config/opencode/AGENTS.md` (auto-cargado). Específico de este repo:
- **Orden de build importa:** compila el frontend (`web/dist`) **antes** de
  `cargo build`, porque `rust-embed` lee esos assets en tiempo de compilación.
- Backend: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.
- Frontend (desde `web/`): `npx vitest run`, `npx tsc --noEmit`, `npm run lint`.
- Toolchain local: Rust 1.97 / Node 22. edition 2024 exige toolchain reciente (≥1.85).
