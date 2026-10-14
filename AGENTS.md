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
- `src/`: `auth/{session,routes,middleware,basic,app_tokens}`, `db/{mod,search,models}`,
  `dav/`, `api/`.
- `web/`: Vite + React; el SPA consume la sesión vía `/api/auth/*` (contexto en
  `web/src/contexts/AuthContext.ts`).

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

Release: PR de `development` → `main`. Al mergear, la imagen se publica en GHCR.

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

### Versionado
- `.vampus.yml` — gestión de versión: `vampus upgrade --major|--minor|--patch`.
- `cliff.toml` — config de `git-cliff`, genera `CHANGELOG.md`.

Reglas de bump:
- `feat` → **minor**.
- `feat!` / `BREAKING CHANGE` → **major**.
- el resto → **patch**.

### CI

| Workflow | Disparadores | Qué hace |
|:---|:---|:---|
| `ci.yml` | PR y push a `development`/`main` | frontend + backend: fmt, clippy, test. |
| `image.yml` | push a `main` (ignora cambios solo de docs), tags `v*` y `workflow_dispatch` | construye la imagen, smoke test y publica en `ghcr.io/atareao/oxidav`. |

Etiquetas de imagen que publica `image.yml`:

| Evento | Tags |
|:---|:---|
| tag `v*` | `vX.Y.Z`, `X.Y`, `latest` |
| push a `main` | `latest`, `sha-<sha7>` |
| `workflow_dispatch` | `sha-<sha7>` |

Los secretos/tokens de registro **no** hacen falta más allá del `GITHUB_TOKEN`,
que ya usa `image.yml`.

## Comandos
El workflow OpenSpec (SDD) + TDD y los comandos base por stack están definidos en
`~/.config/opencode/AGENTS.md` (auto-cargado). Específico de este repo:
- **Orden de build importa:** compila el frontend (`web/dist`) **antes** de
  `cargo build`, porque `rust-embed` lee esos assets en tiempo de compilación.
- Backend: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.
- Frontend (desde `web/`): `npx vitest run`, `npx tsc --noEmit`, `npm run lint`.
- Toolchain local: Rust 1.97 / Node 22. edition 2024 exige toolchain reciente (≥1.85).
