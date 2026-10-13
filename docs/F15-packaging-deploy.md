# F15 — Empaquetado y despliegue

## Qué hace

Convierte OxiDav en un **único binario distribuible**: sirve la API REST, DAV, `healthz` **y la
SPA embebida** (assets de `web/dist` embebidos con `rust-embed` en tiempo de compilación), más un
**Dockerfile multi-stage** para desplegar.

## Orden de build (crítico)

`rust-embed` lee `web/dist` **al compilar**. Por tanto: **frontend primero**, luego backend.

```
cd web && npm ci && npm run build   # -> web/dist
cargo build --release --locked      # embebe web/dist
```

`build.rs` garantiza que `web/dist` existe: en **debug** crea un placeholder (con aviso) si falta;
en **release** **aborta** si no hay un `index.html` real, para no distribuir un binario con
placeholder.

## Estáticos y SPA fallback

`src/assets.rs` sirve los assets embebidos desde el **mapa de `rust-embed`** (sin acceso a
filesystem → sin path traversal). El handler está montado como **fallback** del router, con
prioridad para `/healthz`, `/api/v1/*` y `/dav/*`. Reglas:

- Un asset existente → `200` con `Content-Type` por extensión.
- Ruta de cliente desconocida (sin extensión) → `index.html` (`200`).
- Asset inexistente **con** extensión → `404`.
- `/api/*` y `/dav/*` **nunca** devuelven HTML (JSON `404`); se normalizan los segmentos `.`/`..`
  y se percent-decodea **antes** de clasificar, de modo que `/./api/...` o `/foo/../api/...` no
  burlan el aislamiento.
- Solo `GET`/`HEAD` tocan los estáticos.

## Caché

- `index.html` y demás → `Cache-Control: no-cache` (evita servir una SPA obsoleta tras desplegar).
- Assets hasheados bajo `/assets/` → `public, max-age=31536000, immutable`.
- `ETag` fuerte + `If-None-Match` (incl. `W/`/`*`) → `304`.

## Dockerfile multi-stage

Tres stages: **`web`** (`node:22-slim`, construye `web/dist`) → **`builder`**
(`rust:1.97-bookworm`, compila con `web/dist` embebido, `--locked`) → **runtime**
(`debian:bookworm-slim`, **usuario no-root** `10001`, solo `ca-certificates`). Sin OpenSSL
(`jsonwebtoken`/`rust_crypto`, `reqwest`/`rustls`). `.dockerignore` excluye `target/`,
`web/node_modules/`, `web/dist/`. `web/dist` se copia **desde el stage `web`**, nunca del host.
Incluye `.env` de ejemplo vía `compose.yaml` con volumen para la base SQLite y las envs `OXIDAV_*`.

## Configuración (env)

`OXIDAV_BIND_ADDR`, `OXIDAV_DATABASE_URL`, `OXIDAV_POCKET_ID_ISSUER`, `OXIDAV_OIDC_AUDIENCE`,
`OXIDAV_LOG_LEVEL`, `RUST_LOG`.

## Cómo verificar

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (379, incl.
`tests/assets.rs`: `Content-Type`, `no-cache`/`immutable`/`304`, SPA fallback, `/api`/`/dav` sin
HTML, traversal 404, métodos, `HEAD`). Frontend: `npx tsc -b`, `npm run lint`, `npx vitest run`
(72), `npm run build`. Docker: `docker build -t oxidav .` (no verificable en el entorno de
desarrollo por ausencia de Docker CLI).

## Nota

**Deuda**: sin code-splitting (chunk > 500 kB); Vite `base: './'` asume
rutas de un solo segmento (si se añaden rutas anidadas, valorar `base: '/'`).

## Relacionado

- [`plans/PLAN-016-packaging-deploy-COMPLETED.md`](../plans/PLAN-016-packaging-deploy-COMPLETED.md)
- [`plans/PLAN-001-general.md`](../plans/PLAN-001-general.md)
- [`docs/F02-scaffolding-frontend.md`](F02-scaffolding-frontend.md)
- [`web/README.md`](../web/README.md)
- [`AGENTS.md`](../AGENTS.md)
