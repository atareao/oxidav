# F14 — Ajustes y Perfil (SPA) + OIDC PKCE del frontend

## Qué hace

Cierra el hueco de autenticación del frontend (el flujo **OIDC Authorization Code + PKCE** contra Pocket ID, pendiente desde F04) y añade la vista **Ajustes y Perfil** de la SPA.

## OIDC PKCE (frontend)

`web/src/services/auth.ts`: `login()` genera `code_verifier` (32 B) y `code_challenge` (S256) y redirige al `authorization_endpoint`; `handleRedirectCallback()` valida el **`state`** (CSRF, antes del discovery) y canjea el `code` en el `token_endpoint` (descubierto por `/.well-known/openid-configuration`), **validando en runtime** la respuesta (`access_token` string no vacío). Guarda `access_token`/`refresh_token`/`expiración` en **memoria + `sessionStorage`** (nunca `localStorage`). `refresh()`, `logout()` y `isAuthenticated()`. La limpieza de verifier/state y de la query se hace en **`finally`**. `post_logout_redirect_uri` apunta a una ruta de login (no al callback). **Modo dev**: sin `VITE_OIDC_AUTHORITY` degrada al token manual (`VITE_DEV_ACCESS_TOKEN`). Config por env Vite `VITE_OIDC_AUTHORITY/CLIENT_ID/REDIRECT_URI/SCOPE`.

## HTTP y guard

`web/src/services/api.ts` inyecta `Authorization: Bearer` y, ante `401`, hace **un único refresh coalescido** (`refreshPromise`) y un reintento; si falla → logout. El **token expirado con refresh válido** se refresca sin redirigir a login. Componente `web/src/components/RequireAuth.tsx` protege las rutas; en modo dev deja pasar (no rompe F11–F13). Ruta `/callback` para el retorno del proveedor.

## Vista Ajustes/Perfil

`web/src/pages/Settings.tsx`: **perfil** (`GET /api/v1/me` → claim `sub`), conmutador **tema claro/oscuro** (persistido en `localStorage`: preferencia no sensible), y **gestión de colecciones** (`GET/POST/PATCH/DELETE /api/v1/collections`: listar/crear/renombrar/borrar con confirmación; guardas síncronas anti-doble-submit). Servicios en `web/src/services/collections.ts` (`updateCollection`, `deleteCollection`).

## Cómo verificar

Backend: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (357). Frontend (desde `web/`): `npx tsc -b`, `npm run lint`, `npx vitest run` (72), `npm run build`.

## Deuda / pendiente

Sin deuda pendiente: el code-splitting del bundle y la redirección post-login se cerraron en **F16** (`docs/F16-polish-release.md`).

## Relacionado

- `plans/PLAN-015-settings-profile-COMPLETED.md`
- `plans/PLAN-001-general.md`
- `docs/F04-oidc-pocket-id.md`
- `docs/F06-rest-api.md`
- `docs/F13-sync-devices.md`
- `web/README.md`
- `AGENTS.md`
