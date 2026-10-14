# F13 — REST de app tokens + Vista Sincronización y Dispositivos (SPA)

## Qué hace

Añade a OxiDav la gestión de **app tokens** vía REST y la **vista Sincronización y Dispositivos** de la SPA, para conectar clientes DAV nativos (iOS, DAVx⁵, Thunderbird) que usan Basic Auth.

## Endpoints (backend)

Bajo `/api/v1`, todos bajo `require_auth`.

| Método | Ruta | Éxito | Comportamiento |
|---|---|---|---|
| `GET` | `/api/v1/app-tokens` | `200` | Lista de metadatos de los tokens del usuario. |
| `POST` | `/api/v1/app-tokens` | `201` | Crea el token y devuelve el token en claro **una vez**. |
| `PATCH` | `/api/v1/app-tokens/{id}` | `200` | Renombra el token (metadatos actualizados). |
| `DELETE` | `/api/v1/app-tokens/{id}` | `204` | Revoca el token (sin cuerpo). |

## Seguridad

- El **secreto/token en claro** solo aparece en la respuesta de creación; **nunca** en listados, logs ni por serialización (`AppTokenCreated`/`BasicCredentials` tienen `Debug` manual que lo redacta).
- **Anti-IDOR**: listar/renombrar/revocar se scopean por `AuthUser.sub`; id ajeno → `404`.
- Comparación del secreto en **tiempo constante** (`subtle`), con señuelo para ids desconocidos.

## Concurrencia

Las transacciones de escritura usan **`BEGIN IMMEDIATE`** (`BEGIN_WRITE`/`begin_write` en `src/db/mod.rs`) para evitar `SQLITE_BUSY_SNAPSHOT` (SQLite 517) bajo escritores concurrentes. Esto también eliminó un test **flaky** de `tests/caldav.rs`.

## Frontend

`web/src/pages/Sync.tsx`: **URLs DAV copiables** (CalDAV `/dav/calendars/<usuario>/`, CardDAV `/dav/addressbooks/<usuario>/`) con instrucciones para clientes comunes; **gestor de app tokens** (tabla, crear con Modal, diálogo que muestra el token **una única vez** —el `Modal` se desmonta al cerrar—, copiar con fallback, revocar con confirmación). Servicios en `web/src/services/appTokens.ts` (`listAppTokens`, `createAppToken`, `renameAppToken`, `revokeAppToken`), copiado robusto en `web/src/utils/clipboard.ts`.

## DTOs

`AppTokenMetadata`: `id`, `name`, `created_at`, `last_used_at`. `AppTokenCreated`: lo anterior + `token` (claro, solo en `201`). Nunca `secret_hash`.

## Cómo verificar

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (backend 357, incl. `tests/api.rs`: anti-IDOR con dos usuarios, secreto una sola vez, `401`, envelope). Frontend (desde `web/`): `npx tsc -b`, `npm run lint`, `npx vitest run` (33), `npm run build`.

## Deuda / pendiente

- OIDC PKCE en frontend: implementado en **F14** (`docs/F14-settings-profile.md`).

## Relacionado

- [`plans/PLAN-014-sync-devices-COMPLETED.md`](../plans/PLAN-014-sync-devices-COMPLETED.md)
- [`plans/PLAN-001-general.md`](../plans/PLAN-001-general.md)
- [`docs/F05-app-tokens-basic-auth.md`](F05-app-tokens-basic-auth.md)
- [`docs/F06-rest-api.md`](F06-rest-api.md)
- [`docs/F07-caldav-core.md`](F07-caldav-core.md)
- [`docs/F08-carddav-core.md`](F08-carddav-core.md)
- [`web/README.md`](../web/README.md)
- [`AGENTS.md`](../AGENTS.md)
