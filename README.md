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
  - SPA: **OIDC Authorization Code server-side** contra **Pocket ID** con **cliente confidencial**: el backend intercambia el código, valida el `id_token` (RS256 + JWKS) y establece una **cookie de sesión firmada** (`oxidav_session`, `HttpOnly` + `Secure` + `SameSite=Lax`, 8 h). El SPA **no almacena ni envía tokens** (sin `Bearer`, sin `sessionStorage`) y **nunca ve el `access_token`**; la única excepción es el `id_token_hint` del `end_session_url` que devuelve el logout, que el SPA solo usa para navegar a una URL `https:`. OxiDav **no almacena contraseñas principales**.
  - Clientes DAV nativos (iOS, DAVx⁵, Thunderbird): **Basic Auth** con **app tokens** revocables (`oxidav_<id>_<secret>`, secreto hasheado en SQLite). El DAV **también** acepta la cookie de sesión cuando la auth está activa.
- **DAV**: CalDAV/CardDAV core, `PROPFIND` / `REPORT` / `PUT` / `DELETE` / `MKCALENDAR`, y **RFC 6578 (sync-collection)** con `sync-token` incremental (tombstones).
- **Zonas horarias**: soporte de `TZID` (IANA) extremo a extremo con bloque **`VTIMEZONE`** derivado de la tzdb (`chrono-tz`), de modo que las recurrencias (`RRULE`) se repiten en **hora local**.
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
| `OXIDAV_BIND_ADDR` | Dirección de escucha (p. ej. `0.0.0.0:8080`) | `0.0.0.0:8080` |
| `OXIDAV_DATABASE_URL` | Ruta de la base SQLite | `sqlite://data/oxidav.db` |
| `OXIDAV_LOG_LEVEL` | Filtro de log (p. ej. `info`) | — |
| `AUTH_ENABLED` | Activa la autenticación (`true`/`1`) | `false` |
| `AUTH_ISSUER_URL` | Issuer OIDC (Pocket ID) | — (vacío) |
| `AUTH_CLIENT_ID` | Client id del cliente confidencial | — (vacío) |
| `AUTH_CLIENT_SECRET` | Client secret del cliente confidencial | — (vacío) |
| `AUTH_REDIRECT_URL` | `redirect_uri` = `{base}/api/auth/callback` | — (vacío) |
| `AUTH_POST_LOGOUT_REDIRECT_URL` | `post_logout_redirect_uri` opcional para el cierre SSO | — (vacío) |
| `JWT_SECRET` | Secreto HS256 que firma las cookies de sesión y de flujo | — (vacío) |

Con `AUTH_ENABLED=true`, el backend **aborta el arranque** si falta `AUTH_ISSUER_URL`, `AUTH_CLIENT_ID`, `AUTH_CLIENT_SECRET`, `AUTH_REDIRECT_URL` o `JWT_SECRET`. Además, `AUTH_ISSUER_URL` y `AUTH_REDIRECT_URL` deben ser URLs **absolutas `https:`** (se admite `http` solo en hosts loopback —`localhost`, `127.0.0.1`, `::1`—) o el arranque falla nombrando la variable. `JWT_SECRET` debe ser un secreto real y estable: rotarlo invalida todas las sesiones. Ya **no** existen `OXIDAV_POCKET_ID_ISSUER` ni `OXIDAV_OIDC_AUDIENCE`.

El frontend ya no maneja tokens: no hay variables `VITE_OIDC_*` ni `Authorization: Bearer`; el cliente HTTP usa `credentials: "include"`.

### Autenticación en el navegador (HTTPS)

La autenticación activa requiere HTTPS por **dos** motivos:

1. La cookie de sesión es `Secure`: el navegador **solo la guarda por HTTPS**. Sobre `http://`, el login redirige al IdP y vuelve, pero la cookie se descarta y la sesión no persiste.
2. Es una **validación fail-closed**: con `AUTH_ENABLED=true`, el backend **aborta el arranque** si `AUTH_ISSUER_URL` o `AUTH_REDIRECT_URL` no son URLs absolutas `https:` (el `client_secret` viaja en el POST del token y el `code` vuelve por el `redirect_uri`, así que ambos canales deben ir cifrados). La única excepción son los hosts loopback (`localhost`, `127.0.0.1`, `::1`), donde se admite `http` para pruebas locales.

Sirve la app detrás de HTTPS (p. ej. TLS terminado por un proxy inverso). En local, el modo soportado es `AUTH_ENABLED=false` (usuario dev).

### Configurar el cliente en Pocket ID

Registra en Pocket ID un cliente **confidencial** (con secreto, no público) para OxiDav:

- **Client secret**: cópialo en `AUTH_CLIENT_SECRET`.
- **Redirect URI** = `{base}/api/auth/callback` — p. ej. `https://oxidav.example.com/api/auth/callback` (el mismo valor que `AUTH_REDIRECT_URL`).
- **Post-logout redirect URI** (opcional) = `{base}/` o la que prefieras; va en `AUTH_POST_LOGOUT_REDIRECT_URL`.

## Docker

```bash
docker build -t oxidav .
docker run --rm -p 8080:8080 -v oxidav-data:/app/data --env-file .env oxidav
```

La imagen es multi-stage: `node:22-alpine3.23` construye la SPA → `rust:1.98.1-alpine3.23` compila el binario estático **musl** → runtime mínimo `alpine:3.23` con **usuario no-root** `10001`. Hay un `compose.yaml` de ejemplo.

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
- `docs/` — documentación de cada feature (`F01`…`F17`).
- `Dockerfile`, `compose.yaml`, `build.rs`.

## Estado

Todas las features **F01–F17 están completadas**: scaffolding (backend/frontend), capa de datos, app tokens, REST, CalDAV, CardDAV, sync-collection, búsqueda, y las vistas Calendario / Contactos / Sync / Ajustes, más empaquetado. La autenticación OIDC vive en [F17](docs/F17-oidc-valet-parity.md), que supera el modelo de F04. El detalle de cada una está en `docs/` y los planes en `plans/`.

## Licencia

Este proyecto se distribuye bajo la licencia **MIT**. Consulta el fichero [LICENSE](LICENSE).
