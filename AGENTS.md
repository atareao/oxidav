# AGENTS.md — OxiDav

Servidor personal **CalDAV / CardDAV / WebDAV** con SPA de gestión.

**Estado: entregado hasta F17, versión 0.2.0.** El repo contiene `src/` completo
(API REST + DAV + auth OIDC), `web/` (SPA React 19 + Ant Design v6 + Vite),
`migrations/`, `tests/`, `plans/` (PLAN-001…PLAN-018; los de feature cerrada
llevan sufijo `-COMPLETED.md`, salvo `PLAN-001-general.md`), `docs/` (F01…F17),
`CHANGELOG.md`, `README.md` y el `Dockerfile` / `compose.yaml` / `build.rs` de
empaquetado. Existen los tags `v0.1.0`, `v0.1.1` y `v0.2.0`.

La especificación completa vive fuera del repo; este archivo resume las
decisiones **vinculantes** que un agente no puede deducir del código actual.

## Stack (decisiones no obvias)
- **Backend:** Rust **edition 2024** + Axum + Tokio. Un único binario distribuible:
  los assets de React se embeben con `rust-embed`.
- **Frontend:** React 19 + **Ant Design v6** + Vite + TypeScript, en `web/`.
  Se compila a `web/dist/`.
- **Identidad:** **Pocket ID** (OIDC) es el único IdP. OxiDav **no almacena
  contraseñas principales**.
  - SPA: **Authorization Code server-side** con **cliente confidencial**: el backend
    intercambia el código y establece una **cookie de sesión firmada**
    (`oxidav_session`, `HttpOnly` + `Secure` + `SameSite=Lax`, 8 h) y una **cookie de
    flujo** (`oxidav_oauth_flow`, 10 min) que transporta `state`/`nonce`. El SPA **no
    almacena ni envía tokens** y **nunca ve el `access_token`**; ya **no existe `Bearer`**
    ni PKCE en el SPA. La única excepción es el `id_token_hint` embebido en el
    `end_session_url` que devuelve `logout` (el IdP cierra SSO con él): el SPA solo lo usa
    para navegar a una URL `https:`, nunca lo almacena.
  - Variables: `AUTH_ENABLED` (default `false`), `AUTH_ISSUER_URL`, `AUTH_CLIENT_ID`,
    `AUTH_CLIENT_SECRET`, `AUTH_REDIRECT_URL`, `AUTH_POST_LOGOUT_REDIRECT_URL`,
    `JWT_SECRET`. Sustituyen a `OXIDAV_POCKET_ID_ISSUER` y `OXIDAV_OIDC_AUDIENCE`.
  - Con `AUTH_ENABLED=true`, `AuthConfig::validate()` es **fail-closed**: exige esas
    variables no vacías y que `AUTH_ISSUER_URL`/`AUTH_REDIRECT_URL` sean URLs absolutas
    `https:` (se admite `http` solo en hosts loopback —`localhost`, `127.0.0.1`, `::1`);
    si no, **aborta el arranque** nombrando la variable.
  - Clientes DAV nativos (iOS, DAVx⁵, Thunderbird): **Basic Auth** con *app tokens*
    revocables (hash local en SQLite); el DAV **además** acepta la cookie de sesión. Con
    la cookie y métodos que mutan estado (`PUT`/`DELETE`/`MKCALENDAR`), exige evidencia de
    *same-origin* (`Sec-Fetch-Site: same-origin`/`none` o `Origin` propio) o responde
    **403** (cierra el hueco de un subdominio hermano *same-site*).
  - El `user_id` en BD es el claim `sub` de Pocket ID.
- **Búsqueda:** **SQLite FTS5 con tokenizer `trigram`** y ranking `bm25`.
  Sin índice externo ni sincronización de índices desde Rust.

## Criptografía del árbol de dependencias (restricción vinculante)
OxiDav se distribuye como **un binario estático musl sin OpenSSL**. Todo el árbol
de dependencias debe preservar esa propiedad; **no es una preferencia, es un
requisito de empaquetado**.
- `jsonwebtoken` **con la feature `rust_crypto`**. **No** migrar a `aws_lc_rs`
  (el backend que usa el proyecto de referencia `valet`): `aws_lc_rs` arrastra
  `aws-lc-sys` y rompe la propiedad de binario estático sin OpenSSL. Es paridad
  de diseño con `valet`, no paridad de features.
- `reqwest` con `default-features = false` + la feature `rustls` (**nunca**
  `native-tls`, que enlazaría OpenSSL).

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
- El tokenizer `trigram` exige SQLite con FTS5 habilitado → el driver es
  **`sqlx 0.8`** con la feature `sqlite` (que trae `libsqlite3-sys` con
  `bundled`, compilando SQLite con FTS5). Requiere ≥3 caracteres y hace
  substring matching.

## Protocolos y rutas
- DAV en `/dav/...`: `PROPFIND`, `REPORT`, `PUT`, `DELETE`, `MKCALENDAR`;
  respuestas XML `multistatus` con `quick-xml`; soporte **RFC 6578
  (sync-collection)** con `sync-token` incremental.
- URLs canónicas: `/dav/calendars/{username}/` y `/dav/addressbooks/{username}/`.
- REST JSON para la SPA en `/api/v1/...` con DTOs limpios: el cliente nunca
  parsea `.ics` ni `.vcf`.
- `GET /healthz` es la sonda de salud; un `/api/v1/*` desconocido responde un
  **404 JSON** uniforme.

## Layout
- `src/`: `auth/{session,routes,middleware,basic,app_tokens}`, `db/{mod,search,models}`,
  `dav/`, `api/`, más `config.rs`, `router.rs`, `assets.rs`, `lib.rs` y `main.rs`.
- `web/`: Vite + React; el SPA consume la sesión vía `/api/auth/*` (contexto en
  `web/src/contexts/AuthContext.ts`).
- `migrations/`: esquema SQL embebido en compilación con
  `sqlx::migrate!("./migrations")`.

## Autenticación y desarrollo local
- Con `AUTH_ENABLED=false` (el **default**) el servidor arranca sin IdP y la API
  responde como usuario dev (`sub="dev"`); `GET /api/auth/login` y
  `/api/auth/callback` devuelven **404** (la ruta no existe). Ese es el modo de
  desarrollo soportado.
- La cookie de sesión es `Secure`, así que **sobre `http://` el navegador no la
  guarda**: la autenticación real exige **HTTPS**. No intentes "arreglarlo"
  desactivando `Secure`.
- `AUTH_ENABLED` solo acepta los literales `"true"` y `"1"` (con `trim`);
  cualquier otra cosa (incluida una errata) deja la autenticación apagada.
- **No hace falta CORS con credenciales**: `web/vite.config.ts` proxya `/api` (y
  `/dav`) a `http://localhost:8080`, así que las peticiones del SPA son
  *same-origin* y la cookie viaja.
- El `StateStore` de `state`/`nonce` vive **en el proceso**: en un despliegue con
  varias instancias detrás de un balanceador hacen falta *sticky sessions* o un
  almacén compartido; si no, el callback falla con **401**.

## Gotchas de test (costó un CI en rojo)
Las cachés de descubrimiento OIDC y de JWKS son **globales al proceso**, con clave
por URL y TTL de **300 s**. Los tests que levantan un `wiremock` en un puerto
**efímero** pueden recibir un puerto reciclado ya cacheado, con lo que
`discover()` devuelve la entrada rancia, **no hace el `fetch`** y el test falla
según el orden y el reparto de puertos. Por eso:
- cualquier test que ejercite `discover`, `fetch_jwks` o `validate_id_token`
  contra un mock debe llamar antes a **`auth::session::clear_caches()`** (o
  hacerlo en el helper que monta el servidor, para cubrir a todos);
- lección general: **una suite verde en local no garantiza CI verde** — un test
  dependiente del orden se cae en el runner aunque pase en tu máquina. Si añades
  tests con `wiremock`, limpia el estado compartido.

## Verificación local de la imagen
- **`docker` no está instalado; `podman` y `buildah` sí** (`/usr/bin/`). Verifica
  la imagen con `podman`, no des la tarea de Docker por imposible.
- Receta mínima: construir, arrancar, comprobar `GET /healthz` (200), la SPA en
  `/`, un 404 JSON en `/api/v1/nope`, un 401 en `PROPFIND /dav/`, que el proceso
  corre como **uid 10001** y que el `HEALTHCHECK` de podman queda *healthy*.
- El `HEALTHCHECK` usa **`wget` de busybox**
  (`wget -q --spider http://127.0.0.1:8080/healthz`): Alpine no tiene `bash`, así
  que no uses construcciones de bash en el healthcheck ni en el `ENTRYPOINT`.
  (Podman solo registra `HEALTHCHECK` con `--format docker`; con el manifiesto OCI
  por defecto lo descarta, y por eso `compose.yaml` declara un healthcheck
  equivalente.)
- La imagen resultante es un **ELF estático `static-pie`, *stripped*, de ~25 MB**
  (frente a los ~106 MB de la variante glibc): si un cambio la dispara muy por
  encima, algo se ha colado.
- **Orden de build**: `web/dist` se compila **antes** de `cargo build`
  (`rust-embed` lee los assets en tiempo de compilación). En release, `build.rs`
  aborta si falta `web/dist/index.html`.

## Publicación en GHCR y su verificación
El paquete `ghcr.io/atareao/oxidav` es **público**. Para comprobarlo sin
credenciales: pide un *token* anónimo con
`curl "https://ghcr.io/token?scope=repository:atareao/oxidav:pull&service=ghcr.io"`
y haz un `GET` del manifiesto con ese token (ocasionalmente da 404 transitorio:
reintenta con un token nuevo). El `gh` configurado **no** tiene los scopes
`read:packages`/`write:packages` y `/user/packages/...` responde 404, así que esa
vía no sirve: la visibilidad se hereda del repo público.

## Planes y documentación
- `plans/` — un fichero por plan. Nombrado `PLAN-XXX-<slug>.md` (XXX = número
  secuencial). Al completarse un plan se renombra a `PLAN-XXX-<slug>-COMPLETED.md`
  (no se borra: queda como histórico).
- `docs/` — documentación de cada característica/feature añadida. Un fichero por
  feature, con el mismo slug que su plan de origen.
- Un plan nuevo se registra en `plans/` **antes** de implementar (ver flujo
  OpenSpec/TDD). Al cerrar la feature, el plan pasa a `-COMPLETED.md` y su
  documentación se consolida en `docs/`.

## Git Flow
Gitflow con paridad respecto a `valet`. `main` es producción; `development` es la
base de integración. **Nunca se commitea directamente a `main`: todo entra por PR.**

### Ramas

| Rama | Origen | Destino | Propósito |
|:---|:---|:---|:---|
| `main` | — | — | Producción. Al mergear, publica la imagen en GHCR. |
| `development` | `main` | — | Integración. Base de toda feature. |
| `feature/<nombre>` | `development` | `development` | Nueva funcionalidad o cambio. |
| `hotfix/<nombre>` | `main` | `main` | Arreglo urgente en producción. |
| `release/<versión>` | `development` | `main` | (si aplica) Preparación de una release. |

### Flujo diario

Feature:

```bash
git checkout development && git pull origin development
git checkout -b feature/<nombre>
# ... cambios + commit ...
git push -u origin feature/<nombre>
# PR: feature/<nombre> -> development
```

Hotfix (desde `main`; después hay que sincronizar `development`):

```bash
git checkout main && git pull origin main
git checkout -b hotfix/<nombre>
# ... cambios + commit ...
git push -u origin hotfix/<nombre>
# PR: hotfix/<nombre> -> main
# tras mergear:
git checkout development && git merge main && git push origin development
```

### Flujo de release

```bash
git checkout development && git pull origin development
git checkout -b release/<versión>          # p. ej. release/0.2.0
vampus upgrade --minor                      # o --major / --patch
cargo check                                 # refresca la entrada de `oxidav` en Cargo.lock
git commit -am "chore(release): <versión>"
git push -u origin release/<versión>
# PR: release/<versión> -> main   (CI verde obligatorio)
# tras mergear:
git checkout main && git pull origin main
git tag -a v<versión> -m "<versión>" && git push origin v<versión>
# image.yml publica vX.Y.Z, X.Y y latest
git checkout development && git merge --ff-only origin/main && git push origin development
```

**Gotcha del changelog (obliga a dos tandas de PR):** `git-cliff` deriva las
secciones de los **tags**: si regeneras `CHANGELOG.md` antes de crear el tag, el
contenido sale bajo `[unreleased]` y no como `[X.Y.Z]`. Por eso el ciclo son dos
PR: primero el bump (`chore(release): X.Y.Z`) → `main` → tag; después
`git-cliff --output CHANGELOG.md` en una rama nueva → `development` → `main`; y
por último `gh release create v<versión> --notes-file <sección del changelog>`.

### Antes de commitear
Deja el árbol formateado o la CI falla:
- Backend: `cargo fmt --all`.
- Frontend (desde `web/`): `npm run lint`.

### Conventional Commits con gitmoji

| Tipo | Emoji | Tipo | Emoji |
|:---|:---:|:---|:---:|
| `feat` | ✨ | `ci` | 👷 |
| `fix` | 🐛 | `revert` | ⏪️ |
| `hotfix` | 🚑️ | `deps` | ⬆️ |
| `docs` | 📝 | `breaking` | 💥 |
| `refactor` | ♻️ | | |
| `perf` | ⚡ | | |
| `style` | 💄 | | |
| `test` | ✅ | | |
| `chore` | 🔧 | | |

Ejemplo de mensaje: `✨ feat: add dark mode toggle`.

El commit del bump de versión es **`chore(release): X.Y.Z`**, y `cliff.toml` lo
**omite** del changelog (igual que `chore(deps)` y `release`). El parser `^build`
agrupa los commits `build:` en una sección **Build**.

### Versionado
- `.vampus.yml` — gestión de versión: `vampus upgrade --major|--minor|--patch`.
- `cliff.toml` — config de `git-cliff`, genera `CHANGELOG.md`.

Reglas de bump:
- `feat` → **minor**.
- `feat!` / `BREAKING CHANGE` → **major**.
- el resto → **patch**.
- Mientras el proyecto esté en **0.x**, un `feat` que además **rompe la
  configuración** (p. ej. el cambio de variables de entorno de 0.2.0) sube
  **minor**, no major.

**La versión solo vive en `Cargo.toml`** (gestionada por `.vampus.yml`).
`web/package.json` mantiene su `0.0.0` y **no** se versiona: no lo "arregles".

### CI

| Workflow | Disparadores | Qué hace |
|:---|:---|:---|
| `ci.yml` | PR y push a `development`/`main` | frontend + backend: fmt, clippy, test. |
| `image.yml` | push a `main` (ignora cambios solo de docs), tags `v*` y `workflow_dispatch` | construye la imagen, smoke test y publica en `ghcr.io/atareao/oxidav`. |

Los dos jobs de `ci.yml` se llaman exactamente **`Frontend (Node 22)`** y
**`Backend (Rust stable)`**: son los nombres que usa la protección de rama como
*required status checks*, así que **renombrarlos rompería la protección**.

Etiquetas de imagen que publica `image.yml`:

| Evento | Tags |
|:---|:---|
| tag `v*` | `vX.Y.Z`, `X.Y`, `latest` |
| push a `main` | `latest`, `sha-<sha7>` |
| `workflow_dispatch` | `sha-<sha7>` |

Los secretos/tokens de registro **no** hacen falta más allá del `GITHUB_TOKEN`,
que ya usa `image.yml`.

## Protección de ramas (configurada en GitHub, no en el repo)

| Rama | PR requerido | Checks requeridos | Force push | Borrado | Admin incluido |
|:---|:---|:---|:---:|:---:|:---:|
| `main` | sí (0 aprobaciones) | `Frontend (Node 22)`, `Backend (Rust stable)` | ❌ | ❌ | sí |
| `development` | no | — | ❌ | ❌ | — |

`main` es estricta: **nadie, ni el admin, commitea directamente**; todo entra por
PR con CI verde. `development` **no exige PR a propósito**, porque el sync
`main → development` tras cada release es un push directo (ver flujo de release);
solo está protegida contra reescrituras y borrados.

## Comandos
El workflow OpenSpec (SDD) + TDD y los comandos base por stack están definidos en
`~/.config/opencode/AGENTS.md` (auto-cargado). Específico de este repo:
- **Orden de build importa:** compila el frontend (`web/dist`) **antes** de
  `cargo build`, porque `rust-embed` lee esos assets en tiempo de compilación.
- Backend (como en la CI): `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test --locked`.
- **`--locked` importa:** si cambias dependencias o la versión del paquete y
  **no refrescas `Cargo.lock`**, la CI falla. Refréscalo con `cargo check` (o el
  `cargo` que corresponda) y commitéalo como parte del cambio.
- Frontend (desde `web/`): `npm run test` (`vitest run`),
  `npm run typecheck` (`tsc -b`), `npm run lint`, `npm run build`
  (`tsc -b && vite build`).
- Toolchain local: **Rust/cargo 1.97.0** y **Node 22.23.2**. El `Dockerfile` fija
  **`node:22-alpine3.23`**, **`rust:1.98.1-alpine3.23`** y **`alpine:3.23`**
  (musl). edition 2024 exige toolchain reciente (≥1.85).
