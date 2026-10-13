# PLAN-016 — F15 Empaquetado y despliegue

## Objetivo

Empaquetar OxiDav como un **único binario distribuible** que sirve la API REST (`/api/v1`), DAV (`/dav`) y la SPA React embebida, y desplegarlo mediante una **imagen Docker multi-stage** que arranca como usuario no-root.

## Alcance

Esta feature (F15, última) cierra el proyecto. Incluye:

1. **`rust-embed`**: añadir la dependencia, embeber `web/dist` en el binario y servir los assets estáticos desde el handler de Axum con `Content-Type` correcto, cabeceras `ETag`/`Cache-Control` razonables (assets con hash inmutables vs `index.html` no-cache) y **SPA fallback** a `index.html` para rutas del cliente, excluyendo `/api/*` y `/dav/*`.
2. **Orden de build**: documentar y automatizar que **`web/dist` se construye ANTES de `cargo build`** (rust-embed lee los assets en tiempo de compilación). Añadir un `build.rs`/`just`/script (o documentarlo en README) y un fallback de dev si `web/dist` no existe.
3. **Dockerfile multi-stage** (delegado a `@docker-expert`): stage Node (build de la SPA), stage Rust (compilación release con `web/dist` ya presente), imagen final mínima (distroless o debian-slim) con usuario no-root y puerto configurable. Configuración por variables de entorno `OXIDAV_*`.
4. **Verificación**: tests del handler de estáticos (asset existente, `Content-Type`, fallback SPA a `index.html`, 404 para `/api`/`/dav` inexistentes sin devolver HTML) y un smoke test del binario.

**Fuera de alcance:** cambios funcionales en la API, DAV, OIDC o la SPA; pipeline CI/CD (más allá del build local reproducible).

## Decisiones técnicas

- **Backend Rust/Axum** sirve API + DAV + SPA desde un solo proceso. Los assets de React (F02, `web/dist`) se embeben con `rust-embed`.
- **`rust-embed` como dependencia** con feature `debug-embed` opcional: en debug puede servirse desde disco (`web/dist` en vivo) para agilizar el desarrollo; en release se embebe siempre.
- **SPA fallback obligatorio**: la SPA usa `BrowserRouter` (rutas limpias), por lo que cualquier ruta desconocida que **no** empiece por `/api` ni `/dav` debe devolver `index.html` con `200` (requisito bloqueante documentado en `web/README.md`).
- **Política de caché por tipo de asset:**
  - Assets Vite con hash en el nombre (`assets/*-[hash].js|css`): `Cache-Control: public, max-age=31536000, immutable` + `ETag` derivado del hash/contenido.
  - `index.html` y demás HTML: `Cache-Control: no-cache` (revalidación) para no romper despliegues.
  - Respuestas 404 de rutas `/api`/`/dav`: JSON de error, nunca HTML.
- **Configuración de Axum**: `ServeDir`/handler propio con `tower-http` para capa de estáticos; el fallback se registra en el router tras las rutas `/api` y `/dav`.
- **Config por variables de entorno `OXIDAV_*`**: bind address/puerto, ruta de la BD SQLite y parámetros OIDC (authority/audience), con valores por defecto razonables.
- **Orden de build**: el frontend (`web/dist`) debe existir antes de `cargo build`. Se automatiza con script/`just` y se documenta en README.

## Contratos / Interfaces

**Router de estáticos (orden de matching):**

| Ruta | Handler | Respuesta |
|---|---|---|
| `/api/v1/*` | API REST | JSON; 401 sin token |
| `/dav/*` | DAV | XML `multistatus`; nunca HTML |
| `/assets/*`, `/*.{js,css,png,...}` | Estáticos embebidos | Bytes del asset con `Content-Type` |
| cualquier otra (fallback) | SPA fallback | `index.html` con `200` |

**`Content-Type` y caché por extensión:**

| Extensión | `Content-Type` | `Cache-Control` |
|---|---|---|
| `.html` | `text/html; charset=utf-8` | `no-cache` |
| `.js` | `text/javascript; charset=utf-8` | `public, max-age=31536000, immutable` |
| `.css` | `text/css; charset=utf-8` | `public, max-age=31536000, immutable` |
| `.svg` | `image/svg+xml` | `public, max-age=31536000, immutable` |
| `.png` | `image/png` | `public, max-age=31536000, immutable` |
| `.ico` | `image/x-icon` | `public, max-age=31536000, immutable` |
| `.woff2` | `font/woff2` | `public, max-age=31536000, immutable` |
| `.json` | `application/json` | `no-cache` |

**Variables de entorno `OXIDAV_*`:**

| Variable | Descripción | Default |
|---|---|---|
| `OXIDAV_BIND_ADDR` | Dirección:puerto de escucha | `0.0.0.0:8080` |
| `OXIDAV_DB_PATH` | Ruta del fichero SQLite | `./oxidav.db` |
| `OXIDAV_OIDC_AUTHORITY` | Issuer/authority de Pocket ID | — (requerido) |
| `OXIDAV_OIDC_AUDIENCE` | Audience esperada del JWT | — (requerido) |
| `OXIDAV_LOG_LEVEL` | Nivel de logging (`tracing`) | `info` |

## Tareas (TDD)

- [x] **Backend/infra**

- [x] **Paso 1 (RED):** Escribir tests del handler de estáticos.
      - Asset existente → `200`, `Content-Type` correcto, `ETag`/`Cache-Control` esperados.
      - Ruta de cliente desconocida (`/calendario`) → `200` y cuerpo = `index.html`.
      - `/api/inexistente` y `/dav/inexistente` → `404`, **sin** HTML.
- [x] **Paso 2 (RED):** Ejecutar `cargo test` y confirmar fallo de los nuevos tests con la suite legacy en verde.
- [x] **Paso 3 (GREEN):** Añadir `rust-embed`, embeber `web/dist`, implementar handler de estáticos + SPA fallback + capa `tower-http`.
- [x] **Paso 4 (GREEN):** Implementar la configuración por variables `OXIDAV_*`.
- [x] **Paso 5 (GREEN):** `cargo test` en verde (100%).
- [x] **Paso 6 (REFACTOR):** `cargo fmt --check` y `cargo clippy -- -D warnings` sin avisos; re-ejecutar `cargo test`.
- [x] **Paso 7:** Documentar el orden de build (`web/dist` antes de `cargo build`) y añadir el script/`just` que lo automatiza, con fallback de dev cuando `web/dist` no existe.
- [x] **Paso 8 (delegado a `@docker-expert`):** Dockerfile multi-stage (Node build → Rust release → imagen mínima no-root) y `.dockerignore`.
- [x] **Paso 9 (verificación Docker):** Construir la imagen y comprobar que arranca y sirve `/` (SPA) y `/api/v1/me` (401 sin token).

## Criterios de aceptación

- El binario único sirve la SPA, la API REST y DAV.
- Las rutas del cliente caen en `index.html` (SPA fallback); las rutas `/api`/`/dav` inexistentes devuelven 404 JSON, nunca HTML.
- Los assets se sirven con `Content-Type` correcto y cabeceras de caché acordes a la tabla.
- La imagen Docker multi-stage arranca como usuario no-root y responde en el puerto configurable.
- `cargo test` en verde; build reproducible (mismo orden documentado `web/dist` → `cargo build`).

## Riesgos

- **`web/dist` ausente en tiempo de compilación**: `rust-embed` falla el build. Mitigación: script que compila la SPA antes, y fallback de dev (`debug-embed`/servir desde disco).
- **Caching agresivo de `index.html`**: rompe despliegues (clientes con HTML antiguo apuntando a assets eliminados). Mitigación: `no-cache` explícito para HTML.
- **Fuga de rutas `/api`/`/dav` al fallback SPA**: devolver HTML en errores de API. Mitigación: excluir esos prefijos del fallback y test de 404 JSON.
- **Tamaño de la imagen**: binario release + assets. Mitigación: imagen mínima (distroless/debian-slim), build release sin símbolos de debug.
- **Toolchain Rust/Node**: edition 2024 exige Rust ≥1.85 (local 1.97) y Node 22. Mitigación: fijar versiones en el Dockerfile.

## Referencias

- `plans/PLAN-001-general.md` — feature F15.
- `AGENTS.md` — orden de build y stack.
- `web/README.md` — requisito bloqueante de SPA fallback.
- `docs/F02-scaffolding-frontend.md` — scaffolding del frontend.
