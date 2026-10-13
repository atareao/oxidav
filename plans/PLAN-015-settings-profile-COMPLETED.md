# PLAN-015 — F14 Ajustes y Perfil (SPA) + OIDC PKCE del frontend

## Objetivo

Cerrar el hueco de autenticación del frontend implementando el flujo OIDC Authorization Code + PKCE contra Pocket ID, y añadir la vista SPA de Ajustes/Perfil (perfil, tema y gestión de colecciones).

## Alcance

**Dentro del alcance:**

1. **OIDC Authorization Code + PKCE (frontend)** en `web/src/services/auth.ts`:
   - Login con Pocket ID vía redirect (generación de `code_verifier`/`code_challenge`).
   - Intercambio del `code` por tokens contra el token endpoint.
   - Guardado de `access_token` / `refresh_token` / expiración (memoria + `sessionStorage`, nunca `localStorage`).
   - `refresh` y `logout`.
   - Guard de rutas SPA que redirige a login si no hay sesión válida.
   - Configuración por variables de entorno Vite (`VITE_OIDC_*`).
2. **Cliente HTTP con refresh** en `web/src/services/api.ts`:
   - Inyección de `Authorization: Bearer <token>`.
   - Renovación automática al expirar: `401 → refresh → reintento` (una vez).
3. **Vista Ajustes/Perfil** en `web/src/pages/Settings.tsx`:
   - Perfil: claim `sub` / username desde `GET /api/v1/me`.
   - Conmutador de tema claro/oscuro reutilizando `ThemeContext`.
   - Gestión de colecciones: listar, crear, renombrar y borrar con confirmación contra `GET/POST/PATCH/DELETE /api/v1/collections`.

**Fuera del alcance:**

- Cambios en el flujo OIDC del backend (F04 ya valida JWT RS256 contra el JWKS de Pocket ID).
- Nuevos endpoints REST (F06 ya expone `/api/v1/me` y `/api/v1/collections`).
- Swagger/OpenAPI (documentado como inexistente; no se aborda en F14).
- Vistas Calendario/Contactos/Sync (F11–F13): solo se les añade el guard de rutas, sin reescribirlas.

## Decisiones técnicas

- **Estado actual / deuda:** F04 implementó OIDC en el backend (validación JWT RS256 contra el JWKS de Pocket ID), pero el frontend nunca implementó el flujo PKCE. `web/src/services/auth.ts` contiene solo un proveedor de token mínimo con `TODO`, deuda documentada en `docs/F11/F12/F13`. F14 cierra ese hueco.
- **Base existente:** React 19 + Ant Design v6 + Vite + TypeScript en `web/`; rutas SPA con `BrowserRouter` (F08); vistas Calendario/Contactos/Sync (F11–F13) asumen un token Bearer.
- **Almacenamiento de tokens:** `access_token` y `refresh_token` en memoria como fuente primaria, respaldados en `sessionStorage` para sobrevivir recargas de pestaña. Se descarta `localStorage` por su persistencia ante cierre de navegador y exposición a XSS de larga vida; `sessionStorage` acota el riesgo a la vida de la pestaña.
- **Gestión del `code_verifier`:** generado por sesión de login, almacenado en `sessionStorage` (clave efímera) y consumido/eliminado tras el intercambio del `code`. `code_challenge` = `BASE64URL(SHA-256(verifier))`, método `S256`.
- **Biblioteca OIDC:** se elige **implementación propia con `crypto.subtle`** frente a `oidc-client-ts`, para evitar una dependencia adicional, mantener control total sobre el almacenamiento y el ciclo de refresh, y poder mockear el flujo en jsdom sin adaptar una librería de terceros. Se documentará en `docs/F14-settings-profile.md`.
- **Refresh del token:** el cliente HTTP detecta `401`, solicita refresh (con bloqueo para evitar refrescos concurrentes) y reintenta la petición original una sola vez. Fallo del refresh → limpieza de sesión y redirect a login.
- **Testing OIDC sin red:** mocks de `fetch` y de `crypto.subtle` (o `vi.stubGlobal`) para simular discovery, token endpoint y expiración; nunca se contacta Pocket ID real en tests.
- **Testing del guard y la vista:** Vitest + React Testing Library con `MemoryRouter` para el guard y mocks de los servicios API para la vista Ajustes.

## Contratos / Interfaces

**`web/src/services/auth.ts`:**

```ts
export function login(): Promise<void>;              // genera PKCE y redirige a Pocket ID
export function handleRedirectCallback(): Promise<void>; // valida state, intercambia code por tokens
export function getAccessToken(): string | null;     // token válido (refresca si expiró)
export function refresh(): Promise<void>;            // renovación con el refresh_token
export function logout(): Promise<void>;             // revoca/limpia sesión y redirige
export function isAuthenticated(): boolean;          // hay sesión con token utilizable
```

**Guard de rutas:** componente que envuelve las rutas protegidas; si `!isAuthenticated()` redirige a la ruta de login.

**`web/src/services/api.ts`:** cliente que adjunta `Authorization: Bearer` y aplica la política `401 → refresh → reintento`.

**Tipos/servicios de colecciones (reutilizados):** tipos `Collection` y funciones `listCollections`, `createCollection`, `updateCollection`, `deleteCollection` mapeando a `GET/POST/PATCH/DELETE /api/v1/collections`.

**Config Vite:** `VITE_OIDC_AUTHORITY`, `VITE_OIDC_CLIENT_ID`, `VITE_OIDC_REDIRECT_URI`, `VITE_OIDC_SCOPE` (y demás `VITE_OIDC_*` necesarios).

## Tareas (TDD)

### Frontend

- [x] **Paso 0 — RED:** Escribir tests Vitest + RTL del flujo PKCE con mocks (generación de `code_verifier`/`code_challenge` S256, intercambio de `code`, guardado/uso del token, refresco ante `401`, `logout` y limpieza de sesión). Confirmar fallo.
      `npx vitest run web/src/services/auth.test.ts`
- [x] **Paso 1 — GREEN:** Implementar en `web/src/services/auth.ts` la generación PKCE con `crypto.subtle`, el redirect de login, `handleRedirectCallback` y el almacenamiento en memoria + `sessionStorage`.
      `npx vitest run web/src/services/auth.test.ts`
- [x] **Paso 2 — GREEN:** Implementar `refresh`, `getAccessToken` y `logout` (bloqueo anti-refresco-concurrente); ajustar tests hasta verde.
      `npx vitest run web/src/services/auth.test.ts`
- [x] **Paso 3 — RED/GREEN:** Tests del cliente HTTP: inyección de `Authorization: Bearer`, política `401 → refresh → reintento` (una vez) y fallo del refresh → logout/redirect. Implementar en `web/src/services/api.ts`.
      `npx vitest run web/src/services/api.test.ts`
- [x] **Paso 4 — RED/GREEN:** Tests del guard de rutas (con `MemoryRouter`): sesión válida permite el render; sin sesión redirige a login. Implementar el guard.
      `npx vitest run web/src/router/*.test.tsx`
- [x] **Paso 5 — RED/GREEN:** Tests de la vista Ajustes: render del perfil desde `GET /api/v1/me`, conmutador de tema con `ThemeContext`, y CRUD de colecciones (listar, crear, renombrar, borrar con confirmación). Implementar `web/src/pages/Settings.tsx`.
      `npx vitest run web/src/pages/Settings.test.tsx`
- [x] **Paso 6 — GREEN/REFACTOR:** Añadir la configuración por variables de entorno Vite (`VITE_OIDC_*`) y scripts/env de ejemplo. Ejecutar lint, types y build.
      `npx vitest run && npx tsc -b && npm run lint && npm run build`

### Backend

- [x] **Paso 7 — Verificación:** Confirmar que no se requieren cambios en el backend (F04 OIDC + F06 REST cubren `/api/v1/me` y `/api/v1/collections`). Verificar según sea necesario.
      `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

## Criterios de aceptación

- No queda ningún `TODO` de autenticación en el frontend.
- Las rutas protegidas exigen sesión válida; sin ella se redirige a login.
- El `access_token` se refresca automáticamente (renovación explícita y ante `401`).
- La vista Ajustes muestra el perfil, permite conmutar tema claro/oscuro y realizar el CRUD de colecciones (incluida confirmación al borrar).
- Los tokens nunca se persisten en `localStorage`.
- Todo verde: `npx vitest run`, `npx tsc -b`, `npm run lint`, `npm run build`; backend sin regresiones.

## Riesgos

- **Seguridad de los tokens en el cliente:** mitigado con memoria + `sessionStorage` y sin `localStorage`; XSS sigue siendo una amenaza residual inherente a las SPA.
- **Compatibilidad con Pocket ID:** requisitos de PKCE/`redirect_uri` deben coincidir con la configuración del cliente OIDC; validar el registro exacto de redirect URIs.
- **Condiciones de carrera del refresh:** varias peticiones con `401` simultáneo podrían disparar refrescos concurrentes; se mitiga con bloqueo y reintento único.
- **Complejidad de testear OIDC en jsdom:** `crypto.subtle`, redirects y callbacks no existen de forma nativa; requiere mocks cuidadosos.
- **Regresión en las vistas F11–F13:** introducir el guard podría romper el render de Calendario/Contactos/Sync; cubrir con tests de integración.

## Referencias

- `plans/PLAN-001-general.md` (F14)
- `docs/F04-oidc-pocket-id.md`
- `docs/F06-rest-api.md`
- `plans/PLAN-014-sync-devices-COMPLETED.md`
- `web/README.md`
- `AGENTS.md`
