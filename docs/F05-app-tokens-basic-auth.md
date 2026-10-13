# F05 — App tokens + Basic Auth DAV

## Qué hace

Genera **app tokens revocables** para clientes DAV nativos (iOS, DAVx⁵, Thunderbird)
que autentican con **Basic Auth** contra `/dav`, sin exponer la contraseña principal
(gestionada por Pocket ID).

## Formato y almacenamiento del token

- Formato `oxidav_<id>_<secret>`; `<id>` = 16 bytes hex (32 chars, validado),
  `<secret>` = 32 bytes (256 bits) CSPRNG.
- **Nunca** se guarda el secret en claro: se almacena `SHA-256` (o `HMAC-SHA256` si hay
  pepper) en `secret_hash`.
- Comparación en **tiempo constante** (`subtle`). En `verify`, el digest se calcula
  **siempre** (hash señuelo si el id no existe) para no filtrar la existencia del token
  por tiempo.
- Pepper opcional vía `OXIDAV_TOKEN_PEPPER` (cacheado una vez con `OnceLock`).
- Se documenta por qué **no** se usa Argon2id (secret de 256 bits aleatorios; punto
  revisable).

## Esquema (migración 0002)

Tabla `app_tokens(id PK, user_id, name, secret_hash, created_at, last_used_at,
revoked_at)` + índices (`user_id`, y `user_id, revoked_at`). Revocación por borrado
lógico (`revoked_at`).

## Contratos

- `generate_token()`, `parse_token`, `hash_secret`/`verify_secret`, `AppToken` (sin
  `secret_hash`).
- Repos: `create_app_token`, `list_app_tokens`, `revoke_app_token`, `touch_last_used`,
  `spawn_touch_last_used`, `verify_app_token -> Result<AuthUser, ...>`.
- `DavAuth` (extractor `FromRequestParts<AppState>`) y `require_basic_auth` para `/dav`;
  `401` + `WWW-Authenticate: Basic realm="oxidav"`.

## Rendimiento

- `spawn_touch_last_used`: actualización de `last_used_at` **best-effort y acotada**
  (semáforo global con límite de concurrencia; las actualizaciones sobrantes se
  descartan) para no bloquear la request ni saturar WAL.

## Cómo verificar

`cargo test` (incluye tests de migración, generación, hashing no reversible, revocación,
parseo Basic y persistencia de `last_used_at`), `cargo clippy --all-targets -- -D
warnings`, `cargo fmt --check`.

## Relacionado

- [`plans/PLAN-006-app-tokens-basic-auth-COMPLETED.md`](../plans/PLAN-006-app-tokens-basic-auth-COMPLETED.md)
- [`docs/F03-data-layer.md`](./F03-data-layer.md)
- [`docs/F04-oidc-pocket-id.md`](./F04-oidc-pocket-id.md)
- [`AGENTS.md`](../AGENTS.md)
