# OxiDav

**Servidor personal CalDAV / CardDAV / WebDAV en Rust con SPA de gestión.**

## Qué es

OxiDav es un **único binario distribuible** (Rust + Axum + Tokio) que sirve:

- **CalDAV, CardDAV y WebDAV** en `/dav`.
- **API REST JSON** en `/api/v1`.
- **`healthz`**.
- La **SPA de administración** (React 19 + Ant Design 6 + Vite + TypeScript), **embebida en el binario** con `rust-embed` y con **SPA fallback**.

## Características

- **Datos**: SQLite (vía `sqlx`, `libsqlite3-sys` *bundled*), `journal_mode=WAL`, **búsqueda FTS5 con tokenizer `trigram` y ranking `bm25`**.
- **Autenticación**:
  - SPA: **OIDC Authorization Code + PKCE** contra **Pocket ID** (JWT RS256 validado contra el JWKS público). OxiDav **no almacena contraseñas principales**.
  - Clientes DAV nativos (iOS, DAVx⁵, Thunderbird): **Basic Auth** con **app tokens** revocables (`oxidav_<id>_<secret>`, secreto hasheado en SQLite).
- **DAV**: CalDAV/CardDAV core, `PROPFIND` / `REPORT` / `PUT` / `DELETE` / `MKCALENDAR`, y **RFC 6578 (sync-collection)** con `sync-token` incremental (tombstones).
- **REST `/api/v1`**: colecciones, eventos, contactos, búsqueda y app tokens.
- **SPA**: vistas **Calendario**, **Contactos** (import/export `.vcf`), **Sincronización y Dispositivos** (URLs DAV + gestor de tokens) y **Ajustes** (perfil, tema claro/oscuro, colecciones).

## Requisitos

- **Rust ≥ 1.85** (edition 2024; el proyecto se desarrolla con 1.97).
- **Node 22** (para construir la SPA).
- Opcional: **Docker** para desplegar.

## Orden de build

`rust-embed` lee `web/dist` **en tiempo de compilación**, así que **el frontend se construye ANTES del backend**:

```bash
cd web && npm ci && npm run build   # genera web/dist
cargo build --release --locked      # embebe web/dist en el binario
```

`build.rs` crea un placeholder en *debug* si falta `web/dist`, y **aborta** la compilación *release* si no hay un `index.html` real.

## Ejecución

```bash
cargo run --release
```

Variables de entorno:

| Variable | Descripción | Default |
| --- | --- | --- |
| `OXIDAV_BIND_ADDR` | Dirección de escucha (p. ej. `0.0.0.0:8080`) | Requerida |
| `OXIDAV_DATABASE_URL` | Ruta de la base SQLite | Requerida |
| `OXIDAV_POCKET_ID_ISSUER` | Issuer de Pocket ID (OIDC) | Requerida |
| `OXIDAV_OIDC_AUDIENCE` | Audience esperada | Opcional (estricto si se define) |
| `OXIDAV_LOG_LEVEL` | Filtro de log (p. ej. `info`) | — |

El frontend usa `VITE_OIDC_AUTHORITY`, `VITE_OIDC_CLIENT_ID`, `VITE_OIDC_REDIRECT_URI` y `VITE_OIDC_SCOPE`.

## Docker

```bash
docker build -t oxidav .
docker run --rm -p 8080:8080 -v oxidav-data:/app/data --env-file .env oxidav
```

La imagen es multi-stage: `node:22` construye la SPA → `rust` compila → runtime `debian-slim` con **usuario no-root**. Hay un `compose.yaml` de ejemplo.

## Tests

```bash
# Backend
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

# Frontend (desde web/)
npx tsc -b
npm run lint
npx vitest run
npm run build
```

Estado: **379 tests backend** y **72 tests frontend**, todos verdes.

## Estructura del proyecto

- `src/` — backend (`api/`, `auth/`, `dav/`, `db/`, `assets.rs`, `router.rs`, `config.rs`, `main.rs`, `lib.rs`).
- `migrations/` — esquema SQLite (`0001_init`, `0002_app_tokens`, `0003_sync_log`).
- `tests/` — tests de integración (api, caldav, carddav, assets, data_layer, app_tokens, …).
- `web/` — SPA React/TS (`src/pages`, `src/services`, `src/components`, `src/contexts`).
- `plans/` — un plan por feature (`PLAN-XXX-<slug>[-COMPLETED].md`).
- `docs/` — documentación de cada feature (`F01`…`F15`).
- `Dockerfile`, `compose.yaml`, `build.rs`.

## Estado

Todas las features **F01–F15 están completadas**: scaffolding (backend/frontend), capa de datos, OIDC, app tokens, REST, CalDAV, CardDAV, sync-collection, búsqueda, y las vistas Calendario / Contactos / Sync / Ajustes, más empaquetado. El detalle de cada una está en `docs/` y los planes en `plans/`.

## Licencia

Este proyecto se distribuye bajo la licencia **MIT**. Consulta el fichero [LICENSE](LICENSE).
