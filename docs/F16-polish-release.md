# F16 — Pulido de entrega

## Qué hace

Cierra los "opcionales de release" pendientes tras F01–F15, sin tocar funcionalidad DAV/REST:
publicación de la imagen en GHCR, code-splitting de la SPA, licencia MIT y redirect
post-login.

## 1. Publicación de la imagen en GHCR

El job `docker` de la CI (`.github/workflows/ci.yml`) publica `ghcr.io/<owner>/<repo>`:

- Permisos de job: `contents: read` + `packages: write`.
- `docker/login-action@v4` contra `ghcr.io` (solo cuando **no** es PR).
- `docker/metadata-action@v6` genera tags: `main` (rama), `latest` (rama por defecto),
  `{{version}}` y `{{major}}.{{minor}}` (tags `v*`), `sha-<short>` y `pr-<n>`.
- `docker/build-push-action@v7` con `push: true` en `main`/tags y `load: true` en PR
  (buildx no permite ambas salidas a la vez).
- El workflow se dispara en `push` a `main` y a tags `v*`, en PR a `main` y a mano.

**Smoke test en todos los eventos**: en PR prueba la imagen cargada localmente; en `main`/tags
hace `docker pull` del primer tag publicado y lo arranca. Verifica `/healthz` = `{"status":"ok"}`,
`/` con `Content-Type: text/html` (SPA) y `/api/v1/me` sin token = `401`.

## 2. Code-splitting de la SPA

- Las 6 páginas se cargan con `React.lazy` + `<Suspense>` (exportan named exports).
- `RouteFallback` (con `role="status"` + `aria-label`) es el fallback compartido. El boundary vive
  **dentro de `AppLayout`** (alrededor del `<Outlet />`) para que el shell Sider/Header no se
  desmonte al cargar una página; `/login` y `/callback` (sin layout) tienen su propio boundary.
- `vite.config.ts` usa la API de rolldown (Vite 8) `build.rollupOptions.output.codeSplitting.groups`
  para aislar `react-vendor`; antd se reparte de forma natural entre las rutas lazy.
- Resultado: **ningún chunk supera 500 kB** (mayor: `react-vendor` ≈ 258 kB) → sin el warning de Vite.

## 3. Licencia MIT

- `LICENSE` (MIT, © 2026 Lorenzo Carbonell «atareao»).
- README § Licencia referencia el fichero.

## 4. Redirect post-login (`from`)

El login con Pocket ID es un redirect de **página completa**, así que el estado de React Router no
sobrevive; la ruta de destino se persiste en `sessionStorage`.

- `RequireAuth` envía `state.from = pathname + search + hash` al rebotar a `/login`.
- `Login` reenvía ese `from` a `login(from)`.
- `auth.ts`: `RETURN_TO_STORAGE_KEY`, `sanitizeReturnTo` (solo rutas internas; rechaza externas,
  protocolo-relativas `//`, caracteres de control, backslash, y `/login`|`/callback` de forma
  case-insensitive e ignorando slash final), `consumeReturnTo` (single-use) y limpieza en
  `logout()` / `__resetAuthForTests()`.
- `Callback` navega a `consumeReturnTo() ?? '/calendar'`.

## Verificación

- Frontend (`web/`): `npx tsc -b`, `npm run lint`, `npx vitest run` (**96 tests**), `npm run build`
  (− warning de chunks).
- CI: jobs `frontend`, `backend` y `docker` en verde.

## Notas

- Se eligió `sessionStorage` (no estado de router) porque el round-trip OIDC recarga la página.
- Menor aceptado: `RouteFallback` usa `minHeight: 100vh` también dentro del layout (transitorio,
  solo mientras carga un chunk de página).
