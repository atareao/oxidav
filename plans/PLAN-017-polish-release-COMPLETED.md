# PLAN-017 — Pulido de entrega (GHCR, code-splitting, licencia, redirect post-login)

**Estado:** En progreso
**Origen:** revisión posterior al cierre de F01–F15 (PLAN-016 packaging/deploy).
**Alcance:** 4 mejoras pequeñas e independientes de "release readiness".

## Objetivo

Cerrar los opcionales pendientes tras publicar el repo, sin tocar funcionalidad DAV/REST:

1. **Publicar la imagen Docker en GHCR** (`ghcr.io/atareao/oxidav`) desde la CI.
2. **Code-splitting** del bundle de la SPA (chunk > 500 kB).
3. **Licencia MIT**.
4. **Redirect post-login** usando el `from` que hoy se ignora.

## No-objetivos

- No se toca el backend (`src/`), ni el contrato DAV/REST, ni el esquema SQLite.
- No se añade funcionalidad de usuario nueva.

## Tareas

### T1 — Publicar imagen en GHCR
- [ ] Añadir `permissions: packages: write` y `contents: read` al job `docker`.
- [ ] `docker/login-action@v4` contra `ghcr.io` (solo cuando no es PR).
- [ ] `docker/metadata-action@v6` para tags/imágenes: `latest` en `main`, semver en tags `v*`, `sha-<short>`.
- [ ] `push: true` cuando no es PR; mantener `load: true` para el smoke test en PR.
- [ ] Mantener el smoke test end-to-end existente.

### T2 — Code-splitting
- [ ] Cargar las páginas con `React.lazy` + `Suspense` (fallback con `Spin`).
- [ ] `build.rollupOptions.output.manualChunks` para separar vendor (React) y antd.
- [ ] Sin regresiones en tests (`App.test.tsx` puede requerir `findBy*`/`waitFor`).

### T3 — Licencia MIT
- [ ] Añadir `LICENSE` (MIT, © 2026 Lorenzo Carbonell «atareao»).
- [ ] Actualizar `README.md` § Licencia y el badge/estado si aplica.

### T4 — Redirect post-login
- [ ] Persistir un `returnTo` interno en `sessionStorage` al iniciar el login.
- [ ] `RequireAuth` ya envía `state.from`; `Login` lo captura y lo pasa a `login()`.
- [ ] `Callback` navega a `returnTo` (o `/calendar`) tras el éxito y lo consume.
- [ ] Validar que `returnTo` es una ruta interna segura (empieza por `/`, no `//`, ni `/login`/`/callback`) para evitar open-redirect.

## Criterios de aceptación (verificación CLI)

- Frontend (`web/`): `npx tsc -b`, `npm run lint`, `npx vitest run`, `npm run build` → todo verde.
- Bundle: el build de Vite no debe emitir el warning de chunk > 500 kB.
- CI GitHub Actions: jobs `frontend`, `backend`, `docker` en verde; el job `docker` publica en GHCR solo en `main`/tags, no en PR.
- `LICENSE` existe y el README lo referencia.

## Riesgos

- Publicar la imagen requiere que el paquete GHCR sea público o esté vinculado al repo (visibilidad se ajusta en GitHub tras el primer push).
- `manualChunks` mal configurado puede romper el árbol de dependencias en runtime → validar con `npm run build` + smoke de CI.
