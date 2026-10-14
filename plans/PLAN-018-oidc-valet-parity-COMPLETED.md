# PLAN-018 — F17 OIDC con paridad de valet (cookie de sesión server-side)

## Objetivo

Sustituir el modelo de autenticación actual de OxiDav (**Authorization Code + PKCE
en el SPA**, `access_token` en `sessionStorage` y verificación de JWT `Bearer` contra
el JWKS de Pocket ID) por el de `valet` (`/data/rust/valet`): flujo **Authorization
Code server-side** con **cliente confidencial**, **cookie de sesión firmada HttpOnly**
y un frontend que **no maneja ningún token**. La autenticación DAV (Basic + app tokens)
se mantiene y **además** acepta la cookie de sesión.

## Alcance

Esta feature (F17) reemplaza el diseño de F04. Incluye:

1. **Cliente OIDC confidencial server-side**: el intercambio del código ocurre en el
   backend. La sesión viaja en una cookie `HttpOnly`; el SPA **no almacena ni envía
   tokens** y **nunca ve el `access_token`** ni gestiona refresco. La **única**
   materialización de token que llega al cliente es el `id_token_hint` embebido en el
   `end_session_url` del logout (patrón estándar de cierre de sesión SSO), que el cliente
   solo usa para navegar a una URL absoluta `https:` y nunca almacena.
2. **Cookie de sesión firmada** (`oxidav_session`) y **cookie de flujo**
   (`oxidav_oauth_flow`), ambas `HttpOnly` + `Secure` + `SameSite=Lax` + `Path=/`.
3. **Rutas de autenticación** bajo `/api/auth/*`: `GET /login`, `GET /callback`,
   `GET /me`, `POST /logout`.
4. **Middleware de sesión** que protege `/api/v1/*` (y cualquier `/api/*` no exento).
5. **DAV** (`/dav`) que sigue exigiendo Basic + app tokens y **acepta la cookie de
   sesión** cuando la auth está activa.
6. **Frontend espejo de `valet/frontend`**: `AuthContext`, `AuthProvider`,
   `LoginPage`, `AuthGate`; sin PKCE, sin `sessionStorage`, sin refresco, sin
   `Authorization`.
7. **Borrado** del código de verificación `Bearer` y de toda la maquinaria PKCE del
   cliente.
8. **Documentación**: `docs/F17-oidc-valet-parity.md`, nota de superación en
   `docs/F04-oidc-pocket-id.md`, `README.md` y `AGENTS.md`.

**FUERA DE ALCANCE:**

- Acceso programático con `Bearer <app_token>` (los app tokens existen para DAV; su
  uso como `Bearer` en la API REST queda fuera).
- **PKCE server-side** (paridad estricta con valet; se documenta como endurecimiento
  futuro).
- La feature de **criptografía** de valet (`aws_lc_rs`): es paridad de *diseño*, no de
  *features*.
- Migración automática de despliegues que usen `OXIDAV_POCKET_ID_ISSUER`.

## Motivación

- Con el flujo **server-side** la sesión viaja en una cookie `HttpOnly` y el SPA **no
  almacena ni envía tokens** ni **nunca ve el `access_token`**: se cierra la vía de
  exfiltración de tokens por XSS. La **única** materialización de token que llega al
  cliente es el `id_token_hint` embebido en el `end_session_url` del logout (patrón
  estándar de cierre de sesión SSO), que solo se usa para navegar a una URL absoluta
  `https:` y nunca se almacena.
- Se eliminan **635 líneas** de PKCE/refresh/discovery del frontend y **~1.100** de
  verificación `Bearer` del backend, sustituidas por **~1.400** líneas de flujo OIDC
  server-side y **~200** de contexto de React.
- **Paridad de diseño** con valet, que es el servidor de referencia del autor.

## Decisiones vinculantes

1. Cliente OIDC **confidencial** con `client_secret`; el intercambio del código ocurre
   en el backend. Razón: es el rol correcto para un servidor que **sí** puede custodiar
   un secreto (RFC 6749 §2.1); un cliente público es el que *no puede* guardar secretos
   (móvil, SPA).
2. **Se elimina el soporte `Bearer`** del backend. Sin el token en el SPA no queda
   ningún llamante; mantenerlo sería la única desviación de valet y reabriría la
   superficie XSS. Para acceso programático futuro la pieza correcta son los **app
   tokens** (`Bearer <app_token>`); queda **fuera de alcance**.
3. `AUTH_ENABLED=false` por defecto. Con la autenticación desactivada la API responde
   como usuario dev (`sub="dev"`, `name="Dev User"`). Al habilitarla,
   `AuthConfig::validate()` es **fail-closed** y aborta el arranque si falta cualquier
   ajuste.
4. `/dav` sigue exigiendo Basic + app tokens **siempre**, y **además** acepta la cookie
   de sesión cuando `AUTH_ENABLED` está activa. El riesgo de CSRF en las mutaciones DAV
   (`PUT`/`DELETE`/`MKCALENDAR`) queda mitigado por `SameSite=Lax`, que no envía la
   cookie en peticiones cross-site que no sean navegación GET.
5. Las rutas de autenticación viven bajo `/api/auth/*` (paridad valet), **no** bajo
   `/api/v1/auth/*`.
6. Nombres de variables de entorno **idénticos a valet**: `AUTH_ENABLED`,
   `AUTH_ISSUER_URL`, `AUTH_CLIENT_ID`, `AUTH_CLIENT_SECRET`, `AUTH_REDIRECT_URL`,
   `AUTH_POST_LOGOUT_REDIRECT_URL`, `JWT_SECRET`. Se retiran
   `OXIDAV_POCKET_ID_ISSUER` y `OXIDAV_OIDC_AUDIENCE`.
7. **Cookies**: `oxidav_session` (sesión) y `oxidav_oauth_flow` (flujo); `HttpOnly` +
   `Secure` + `SameSite=Lax` + `Path=/`. Sesión de 8 h
   (`SESSION_TTL_SECS = 8*3600`), flujo de 10 min.
8. **Sin `refresh_token`**: al expirar la sesión se vuelve a pasar por el IdP. Se
   elimina toda la maquinaria de refresco.
9. **No se copia** la feature de criptografía de valet: valet usa `jsonwebtoken` con
   `aws_lc_rs`; OxiDav usa `rust_crypto` deliberadamente para no arrastrar
   OpenSSL/`aws-lc-sys` y conservar el binario estático musl. Es paridad de *diseño*,
   no de *features*.
10. **Sin PKCE server-side** (paridad estricta con valet). Se documenta como
    endurecimiento futuro posible (protege de *code injection*/mix-up y no requiere nada
    que no tengamos).

## Prerequisitos (fuera del código)

- Reconfigurar el cliente en Pocket ID: pasa de público (PKCE) a **confidencial**, con
  secreto. `redirect_uri` = `{base}/api/auth/callback`; `post_logout_redirect_uri`
  opcional.
- **HTTPS**: la cookie es `Secure`, así que sobre `http://` el navegador no la guarda y
  el login no persiste. Debe quedar documentado en el README.
- `JWT_SECRET` debe ser un secreto real y estable: rotarlo invalida todas las sesiones.
- **Desarrollo sin CORS**: `web/vite.config.ts` ya proxya `/api` (y `/dav`) hacia
  `http://localhost:8080`, de modo que las peticiones del SPA son *same-origin* y la
  cookie viaja sin CORS con credenciales ni cambios en `tower-http`. Como la cookie es
  `Secure` y en local se sirve por `http://`, el modo soportado en desarrollo es
  `AUTH_ENABLED=false` (usuario dev).

## Contratos / Interfaces

### `src/config.rs`

Campos nuevos en `Config`: `auth_enabled: bool`, `auth_issuer_url`, `auth_client_id`,
`auth_client_secret`, `auth_redirect_url`, `auth_post_logout_redirect_url`,
`jwt_secret` (todos `String`). El issuer deja de ser obligatorio (se elimina el
`ConfigError::MissingVar` asociado).

### `src/auth/` — espejo de `valet/src/auth.rs`

**Tipos:**

- `AuthConfig { enabled, issuer_url, client_id, client_secret, redirect_url,
  jwt_secret, post_logout_redirect_url }`, con `validate()` **fail-closed** (lista las
  variables vacías) y `from_config(&Config) -> Result<Self, AuthError>`.
- `FlowClaims { state, nonce, exp, iat }`.
- `Claims { sub, email: Option<String>, name: Option<String>, exp, iat,
  id_token: Option<String> }`.
- `UserIdentity { sub, email: Option<String>, name: Option<String> }`.
- `OidcDiscovery { authorization_endpoint, token_endpoint, jwks_uri,
  end_session_endpoint: Option<String> }`.
- `TokenResponse { id_token, access_token: Option<String>, token_type: Option<String> }`.
- `AuthError` (`thiserror`) con las variantes de valet: `MissingHeader`,
  `InvalidToken(String)`, `ExpiredToken`, `ConfigError(String)`, `InvalidSession`,
  `InvalidState`, `InvalidNonce`, `InvalidIssuer`, `InvalidAudience`, `DiscoveryFailed`,
  `JwksUnavailable`, `TokenExchangeFailed`.

**Constantes:** `SESSION_COOKIE_NAME = "oxidav_session"`, `SESSION_TTL_SECS = 8*3600`,
`ID_TOKEN_LEEWAY_SECS = 5`, `HTTP_TIMEOUT_SECS = 10`, `MAX_BODY_BYTES = 64*1024`,
`DISCOVERY_TTL = 300s`, `JWKS_TTL = 300s`, `FLOW_TTL = 600s`,
`MAX_PENDING_STATES = 1024`.

**Funciones (semántica de valet):**

- `emit_session` / `validate_session`: HS256 firmado con `jwt_secret`; `leeway = 0`
  (una sesión expirada debe rechazarse de inmediato para que el contrato de
  `/api/auth/me` se cumpla).
- `emit_flow_token` / `validate_flow_token`: HS256; llevan `state`+`nonce` firmados
  para que no puedan plantarse desde otro origen (*cookie tossing*).
- `generate_state_and_nonce()`: par aleatorio de **32 bytes** en base64url sin padding.
- `StateStore { states: Mutex<HashMap<String,(String,Instant)>> }` con `issue()`,
  `consume()` (*single-use*), purga por `FLOW_TTL` y tope `MAX_PENDING_STATES` con
  evicción del más antiguo (defensa contra *memory DoS* en un endpoint no
  autenticado); recupera el mutex envenenado sin entrar en pánico.
- `http_client()`: cliente `reqwest` compartido (`OnceLock`), timeout de 10 s y pool 4.
- `discover()` y `fetch_jwks()`: caché en `RwLock<HashMap<...,(doc,Instant)>>` con TTL,
  y `read_limited_json()` que rechaza cuerpos mayores que `MAX_BODY_BYTES` tanto por
  `content-length` como por suma de chunks (requiere el feature `stream` de `reqwest`).
- `exchange_code()`: POST `application/x-www-form-urlencoded` con
  `grant_type=authorization_code`, `code`, `redirect_uri`, `client_id`, `client_secret`.
- `same_origin()`: comparación de *origin* de URL (defensa en profundidad RFC 8414)
  para rechazar un `jwks_uri` fuera del origen del issuer.
- `validate_id_token()`: exige `kid`, `RS256`, leeway `ID_TOKEN_LEEWAY_SECS`,
  `iss` == issuer, `aud` == client_id, `nonce` == esperado; devuelve `UserIdentity`.
- `build_end_session_url()`: añade `id_token_hint` (y `post_logout_redirect_uri` si
  está configurado); `None` si no hay id_token.

### Rutas — `GET /api/auth/login`, `GET /api/auth/callback`, `GET /api/auth/me`, `POST /api/auth/logout`

| Ruta | Comportamiento |
|---|---|
| `GET /api/auth/login` | 404 si la auth está desactivada; descubre; `state_store().issue()`; emite la cookie de flujo firmada; 302 hacia `authorization_endpoint` con `response_type=code`, `client_id`, `redirect_uri`, `scope=openid profile email`, `state`, `nonce`. |
| `GET /api/auth/callback` | 404 si está desactivada; la cookie de flujo se limpia **siempre**, pase lo que pase; valida la cookie, compara `state`, y **consume el state una sola vez ANTES del intercambio de red** (para que una callback reenviada no canjee el código dos veces); descubre; `exchange_code`; `validate_id_token`; emite la sesión; responde con `Set-Cookie` de limpieza del flujo + cookie de sesión; 302 a `/`. |
| `GET /api/auth/me` | 200 con `{sub,email,name}` de la sesión, o 401; con la auth desactivada responde el usuario dev. |
| `POST /api/auth/logout` | Limpia la cookie de sesión y devuelve `{"end_session_url": string\|null}` (la URL de cierre SSO si la sesión conservaba `id_token`). |

Helpers de cookies (`flow_cookie`, `session_cookie`, `clear_cookie`), `redirect_to`,
`with_cookies`, `unix_now`.

### Middleware de sesión

`session_middleware`: si no hay `auth_config` o está desactivada, es *pass-through*.
**Exentos**: `/healthz` y todo `/api/auth/*`. Cualquier otra ruta `/api/*` exige cookie
de sesión válida, o 401. `/api/v1/*` (la API REST) queda detrás de este middleware.

### `/dav`

`require_basic_auth` intenta **primero** la cookie de sesión válida (si la auth está
activa) y **después** el `Authorization: Basic` con app tokens; en ambos casos se
resuelve la misma identidad de usuario que consumen las colecciones.

### `AppState`

`AppState` pasa a llevar `auth_config: Option<Arc<AuthConfig>>` (forma de valet) en
lugar de `oidc: Arc<OidcConfig>` + `jwks: Arc<JwksCache>`. Desaparecen
`OidcConfig::discover`-offline y el `offline_pool` asociado al JWKS.

### `main.rs`

Construye `AuthConfig::from_config(&config)` y **aborta el arranque** si la validación
falla (fail-closed).

### Código a borrar

- `src/auth/oidc.rs` completo (**1141 líneas**): `OidcConfig`, `JwksCache`,
  `require_auth`, `bearer_token`, `AuthUser` como `FromRequestParts`.
- Se conserva un tipo de identidad de usuario, pero se alimenta de los *claims* de la
  sesión (y de los app tokens en el DAV), nunca de un `Bearer`.
- Frontend: `web/src/services/auth.ts`, `web/src/pages/Login.tsx`,
  `web/src/pages/Callback.tsx`, `web/src/components/RequireAuth.tsx` y sus tests.

### Contrato frontend — espejo de `valet/frontend`

- `web/src/contexts/AuthContext.ts`: `AuthUser { sub, email?, name? }`,
  `AuthContextValue { user, loading, loggingOut, logout }`, `useAuth()`.
- `web/src/contexts/AuthProvider.tsx`: resuelve la sesión al montar con
  `GET /api/auth/me` (promesa cacheada en un `ref` para que el doble montaje de
  `StrictMode` no lance dos peticiones); escucha un evento global de 401 y deja
  `user=null`; `logout()` hace `POST /api/auth/logout`, libera la sesión local y navega
  a `window.location.assign(end_session_url)`, aceptándolo **solo si es una URL
  absoluta `https:`** (descarta `http:`, `javascript:`, relativas y vacías para evitar
  un *open redirect*) y cayendo si no al `BASE_URL` de la app.
- `web/src/components/LoginPage.tsx`: pantalla sin sesión con un `<button>` que navega
  a `/api/auth/login`. Debe construirse **sin arrastrar `antd` al bundle inicial**.
- `web/src/App.tsx`: `AuthGate` — mientras carga, indicador; sin usuario, `LoginPage`;
  con usuario, la app autenticada cargada con `lazy()` dentro de un `ErrorBoundary` y un
  `Suspense`.
- `web/src/services/api.ts`: `credentials: "include"` en todas las peticiones; **sin**
  cabecera `Authorization`; el evento de 401 global no se dispara en las rutas
  `/auth/*`.
- Se elimina: PKCE (`generateCodeVerifier`, `deriveCodeChallenge`), `sessionStorage`,
  el refresco, el `tokenProvider`, las variables `VITE_OIDC_*` y la ruta `/callback`
  del router.

## Dependencias

- **Producción**: `axum-extra` (feature `cookie`), `cookie` (para
  `cookie::time::Duration`), `url`, `futures`; y el feature **`stream`** a `reqwest`
  (lo necesita la lectura acotada del cuerpo).
- **Dev**: `wiremock` (servidor JWKS falso en los tests).
- **No** se cambia la feature de criptografía de `jsonwebtoken`.

## Testing (red-green-refactor)

**Backend unitario** (`src/auth/`):
- Ida y vuelta de sesión; rechazo de sesión expirada y de firma manipulada.
- Ida y vuelta y rechazo del *flow token*; `StateStore` (*single-use*, TTL, tope con el
  más reciente superviviente).
- `AuthConfig::validate` fail-closed (cada variable vacía por separado) y no-op al
  estar desactivada.
- `validate_id_token` contra un JWKS servido por `wiremock`: token válido; issuer
  erróneo; audiencia errónea; expirado; nonce erróneo; firma manipulada; `alg=none`;
  confusión de algoritmo HS256 firmado con el módulo RSA público; `jwks_uri` en otro
  origen.
- `discover` rechaza un cuerpo sobredimensionado.
- `build_end_session_url` (con `id_token_hint`, con `post_logout_redirect_uri`
  configurado, y `None` sin id_token).

**Backend integración** (`tests/auth.rs`):
- `login` con la auth desactivada → 404 y con la activada → 302 + `Set-Cookie` de flujo.
- `callback` con `state` inválido → 401.
- `me` sin cookie → 401 y con la auth desactivada → usuario dev.
- `logout` → `end_session_url` nulo sin id_token.
- Middleware: `/api/v1/*` → 401 sin cookie y 200 con cookie válida; `/healthz` y
  `/api/auth/*` exentos.

**DAV**:
- Una cookie de sesión válida se acepta en `/dav`; sin cookie y sin Basic → 401.
- Los tests de DAV existentes deben seguir verdes.

**Frontend** (vitest):
- `AuthProvider` (`me` correcto; `me` con 401 → no autenticado; 401 global → no
  autenticado).
- `LoginPage` navega a `/api/auth/login`.
- `AuthGate` (cargando / login / app).
- `logout` navega a un `end_session_url` `https:` y **descarta** `http:`, `javascript:`
  y relativas.
- El cliente HTTP envía `credentials: include` y no `Authorization`.

## Tareas (TDD)

### Fase 1 — Configuración

- [x] **Paso 1 (RED):** Escribir los tests de `AuthConfig::validate` fail-closed: por
      cada variable vacía (`AUTH_ISSUER_URL`, `AUTH_CLIENT_ID`, `AUTH_CLIENT_SECRET`,
      `AUTH_REDIRECT_URL`, `JWT_SECRET`) se obtiene error; con todas presentes, `Ok`; con
      la auth desactivada, `Ok` sin exigir nada.
- [x] **Paso 2 (RED):** Ejecutar `cargo test` y confirmar el fallo de los nuevos tests
      con la suite legacy en verde.
- [x] **Paso 3 (GREEN):** Añadir los campos nuevos a `Config` (`auth_enabled`,
      `auth_issuer_url`, `auth_client_id`, `auth_client_secret`, `auth_redirect_url`,
      `auth_post_logout_redirect_url`, `jwt_secret`) y retirar `OXIDAV_POCKET_ID_ISSUER`
      y `OXIDAV_OIDC_AUDIENCE` junto con el `ConfigError::MissingVar` asociado al issuer.
- [x] **Paso 4 (GREEN):** Implementar `AuthConfig::from_config(&Config)` y su
      `validate()`. `cargo test` en verde.

### Fase 2 — Tipos y primitivas

- [x] **Paso 5 (RED):** Tests de sesión: ida y vuelta de `emit_session`/`validate_session`;
      rechazo de sesión expirada (`leeway = 0`) y de firma manipulada.
- [x] **Paso 6 (RED):** Tests del *flow token*: ida y vuelta de
      `emit_flow_token`/`validate_flow_token`; rechazo de firma manipulada.
- [x] **Paso 7 (RED):** Tests de `StateStore`: `consume()` *single-use* (segundo intento
      falla), purga por `FLOW_TTL`, y tope `MAX_PENDING_STATES` con evicción del más
      antiguo (el más reciente sobrevive).
- [x] **Paso 8 (RED):** Tests de `validate_id_token` con JWKS servido por `wiremock`:
      token válido; issuer erróneo; audiencia errónea; expirado; nonce erróneo; firma
      manipulada; `alg=none`; HS256 firmado con el módulo RSA público; `jwks_uri` en otro
      origen (`same_origin`).
- [x] **Paso 9 (RED):** Tests de `discover` con cuerpo sobredimensionado → rechazo;
      y de `build_end_session_url` (con `id_token_hint`, con
      `post_logout_redirect_uri` configurado, y `None` sin id_token).
- [x] **Paso 10 (GREEN):** Implementar en `src/auth/` los tipos (`AuthConfig`,
      `FlowClaims`, `Claims`, `UserIdentity`, `OidcDiscovery`, `TokenResponse`,
      `AuthError`), las constantes, `emit_session`/`validate_session`,
      `emit_flow_token`/`validate_flow_token`, `generate_state_and_nonce`, `StateStore`,
      `http_client`, `discover`, `fetch_jwks`, `read_limited_json`, `exchange_code`,
      `same_origin`, `validate_id_token` y `build_end_session_url`.
- [x] **Paso 11 (GREEN):** Añadir a `Cargo.toml` las dependencias de producción
      (`axum-extra` con feature `cookie`, `cookie`, `url`, `futures`), el feature
      `stream` a `reqwest` y `wiremock` en dev. `cargo test` en verde.

### Fase 3 — Rutas y middleware

- [x] **Paso 12 (RED):** Tests de integración (`tests/auth.rs`) de `login`: con la auth
      desactivada → 404; con la activada → 302 + `Set-Cookie` de flujo.
- [x] **Paso 13 (RED):** Tests de `callback` con `state` inválido → 401; `me` sin cookie
      → 401 y con la auth desactivada → usuario dev; `logout` → `end_session_url` nulo
      sin id_token.
- [x] **Paso 14 (RED):** Tests de integración del middleware: `/api/v1/*` → 401 sin
      cookie y 200 con cookie válida; `/healthz` y `/api/auth/*` exentos.
- [x] **Paso 15 (GREEN):** Implementar `GET /api/auth/login`, `GET /api/auth/callback`,
      `GET /api/auth/me`, `POST /api/auth/logout` con los helpers de cookies
      (`flow_cookie`, `session_cookie`, `clear_cookie`), `redirect_to`, `with_cookies` y
      `unix_now`. El `callback` limpia siempre la cookie de flujo y **consume el state
      una sola vez antes del intercambio de red**.
- [x] **Paso 16 (GREEN):** Implementar `session_middleware` (*pass-through* sin
      `auth_config` o desactivada; exentos `/healthz` y `/api/auth/*`; resto de `/api/*`
      exige cookie válida o 401).
- [x] **Paso 17 (GREEN):** Cambiar `AppState` a `auth_config: Option<Arc<AuthConfig>>` y
      actualizar `main.rs` para construir `AuthConfig::from_config(&config)` y abortar el
      arranque si la validación falla. `cargo test` en verde.

### Fase 4 — DAV

- [x] **Paso 18 (RED):** Tests de DAV: una cookie de sesión válida se acepta en `/dav`;
      sin cookie y sin Basic → 401. Verificar que la suite de DAV existente sigue verde.
- [x] **Paso 19 (GREEN):** Modificar `require_basic_auth` para intentar **primero** la
      cookie de sesión válida (si la auth está activa) y **después** el
      `Authorization: Basic` con app tokens, resolviendo la misma identidad de usuario.
      `cargo test` en verde.

### Fase 5 — Borrado del código viejo

- [x] **Paso 20:** Borrar `src/auth/oidc.rs` completo (1141 líneas: `OidcConfig`,
      `JwksCache`, `require_auth`, `bearer_token`, `AuthUser` como `FromRequestParts`),
      eliminar `oidc: Arc<OidcConfig>` + `jwks: Arc<JwksCache>` de `AppState`,
      `OidcConfig::discover`-offline y el `offline_pool` asociado al JWKS.
- [x] **Paso 21:** Conservar un tipo de identidad de usuario alimentado por los *claims*
      de la sesión (y por los app tokens en el DAV); **eliminar todo soporte `Bearer`**.
      `cargo test` en verde.

### Fase 6 — Frontend

- [x] **Paso 22 (RED):** Tests de `AuthProvider` (vitest): `me` correcto → autenticado;
      `me` con 401 → no autenticado; 401 global → no autenticado.
- [x] **Paso 23 (RED):** Test de `LoginPage`: el botón navega a `/api/auth/login`.
- [x] **Paso 24 (RED):** Test de `AuthGate`: estado cargando (indicador), sin usuario
      (`LoginPage`), con usuario (app autenticada).
- [x] **Paso 25 (RED):** Test de `logout`: navega a un `end_session_url` `https:` y
      **descarta** `http:`, `javascript:` y relativas (cae al `BASE_URL`).
- [x] **Paso 26 (RED):** Test del cliente HTTP: envía `credentials: include` y **no**
      `Authorization`.
- [x] **Paso 27 (GREEN):** Implementar `web/src/contexts/AuthContext.ts` y
      `web/src/contexts/AuthProvider.tsx` (promesa cacheada en `ref` para `StrictMode`;
      escucha del 401 global; `logout()` con validación de URL `https:`).
- [x] **Paso 28 (GREEN):** Implementar `web/src/components/LoginPage.tsx` sin arrastrar
      `antd` al bundle inicial, y `web/src/App.tsx` con `AuthGate` (`lazy()` +
      `ErrorBoundary` + `Suspense`).
- [x] **Paso 29 (GREEN):** Actualizar `web/src/services/api.ts` con
      `credentials: "include"`, sin `Authorization` y sin disparar el evento de 401 en
      `/auth/*`.
- [x] **Paso 30:** Eliminar del frontend PKCE (`generateCodeVerifier`,
      `deriveCodeChallenge`), `sessionStorage`, el refresco, el `tokenProvider`, las
      variables `VITE_OIDC_*` y la ruta `/callback`; borrar
      `web/src/services/auth.ts`, `web/src/pages/Login.tsx`,
      `web/src/pages/Callback.tsx`, `web/src/components/RequireAuth.tsx` y sus tests.
- [x] **Paso 31 (GREEN):** `npx vitest run` en verde.

### Fase 7 — Documentación

- [x] **Paso 32:** Crear `docs/F17-oidc-valet-parity.md`: contrato de configuración,
      flujo, cookies y prerequisitos de Pocket ID y HTTPS.
- [x] **Paso 33:** Añadir en `docs/F04-oidc-pocket-id.md` la nota de que queda
      **superado** por F17.
- [x] **Paso 34:** Actualizar `README.md` con las variables de entorno nuevas, el
      requisito de HTTPS y la reconfiguración del cliente en Pocket ID.
- [x] **Paso 35:** Actualizar `AGENTS.md`: sección de identidad (cookie de sesión en
      lugar de `Bearer`) y nombres de las variables.

### Fase 8 — Verificación

- [x] **Paso 36 (REFACTOR):** `cargo fmt --check` y
      `cargo clippy --all-targets -- -D warnings` sin warnings; re-ejecutar `cargo test`.
- [x] **Paso 37 (REFACTOR):** Desde `web/`: `npx tsc -b`, `npm run lint`, `npx vitest run`
      y `npm run build` en verde.
- [x] **Paso 38:** Compilar el frontend **antes** del backend (`web/dist` → `cargo build`)
      porque `rust-embed` lee los assets en tiempo de compilación. `cargo test` en verde
      (100%).

## Criterios de aceptación

- El flujo es **Authorization Code server-side** con cliente **confidencial**: el
  intercambio del código ocurre en el backend; la sesión viaja en una cookie `HttpOnly` y
  el SPA **no almacena ni envía tokens** ni **nunca ve el `access_token`**. La **única**
  materialización de token que llega al cliente es el `id_token_hint` embebido en el
  `end_session_url` del logout, que solo se usa para navegar a una URL absoluta `https:`
  y nunca se almacena.
- Existen las rutas `GET /api/auth/login`, `GET /api/auth/callback`, `GET /api/auth/me`
  y `POST /api/auth/logout` bajo `/api/auth/*`.
- Las cookies `oxidav_session` y `oxidav_oauth_flow` se emiten `HttpOnly` + `Secure` +
  `SameSite=Lax` + `Path=/`; la sesión dura 8 h y el flujo 10 min.
- `AUTH_ENABLED=false` por defecto; con la auth desactivada la API responde el usuario
  dev (`sub="dev"`, `name="Dev User"`) y `login`/`callback` devuelven 404.
- Con la auth activada, `AuthConfig::validate()` es fail-closed y aborta el arranque si
  falta cualquier ajuste.
- `/api/v1/*` exige cookie de sesión válida (401 sin ella); `/healthz` y `/api/auth/*`
  están exentos.
- `/dav` sigue exigiendo Basic + app tokens y **además** acepta la cookie de sesión; la
  suite de DAV existente sigue verde.
- **No** queda ningún soporte `Bearer` en el backend ni ningún manejo de tokens en el
  frontend: `credentials: "include"` y sin `Authorization`.
- `logout` devuelve `{"end_session_url": string|null}`; el frontend solo navega a URLs
  absolutas `https:`.
- Toda la suite backend (`cargo test`) y frontend (`npx vitest run`) pasa; `cargo fmt
  --check`, `cargo clippy --all-targets -- -D warnings`, `npx tsc -b` y `npm run lint`
  sin warnings.
- `docs/F17-oidc-valet-parity.md`, `docs/F04-oidc-pocket-id.md`, `README.md` y
  `AGENTS.md` reflejan el nuevo modelo.

## Riesgos

- **Cambio de modelo**: cualquier despliegue que use `OXIDAV_POCKET_ID_ISSUER` deja de
  arrancar igual. Mitigación: el proyecto es greenfield reciente, así que se documenta
  en vez de migrarse.
- **`Secure` + http local**: en desarrollo por `http://` el navegador no guarda la
  cookie y el login no persiste. Mitigación: documentarlo y recomendar HTTPS.
- **Regresión en DAV** al tocar `require_basic_auth`. Mitigación: la suite de DAV
  existente debe seguir verde.
- **Pérdida de `Bearer`**: si algún consumidor externo lo usaba, se rompe. Mitigación:
  no existe ninguno hoy en el repo; la alternativa documentada son los app tokens.

## Cierre — desviaciones y endurecimientos

Registro de lo que se apartó del enunciado original y de lo que se endureció por
encima de `valet`, para que el documento no mienta sobre lo construido.

- **Módulo**: la implementación vive en `src/auth/session.rs` (más `routes.rs` y
  `middleware.rs`), no en un `src/auth.rs` único.
- **Endurecimientos por encima de valet** (los motivó una auditoría de seguridad y se
  cerraron todos):
  - se exige `https` en `AUTH_ISSUER_URL` y `AUTH_REDIRECT_URL`, salvo hosts loopback;
  - se valida el origen del `end_session_endpoint`: si no comparte origen con el issuer,
    se descarta;
  - el cliente HTTP OIDC no sigue redirecciones (un `307`/`308` desde el token endpoint
    reenviaría el cuerpo con el `client_secret`);
  - en `/dav`, los métodos que mutan estado autenticados por cookie exigen evidencia
    *same-origin* (`Sec-Fetch-Site`/`Origin`) y responden `403` si falta.
- **Guard de CSRF en `/dav`**: lo introdujo la decisión nº 4 (aceptar también la cookie
  en `/dav`); el guard es su mitigación, porque `SameSite` protege frente a *sitios*
  distintos pero no frente a un subdominio hermano *same-site*.
- **`AuthError`**: mensajes de error en español, por coherencia con el resto de
  `src/auth/`.
- **`jsonwebtoken`**: mantiene la feature `rust_crypto` de OxiDav (no se copia el
  `aws_lc_rs` de valet), para no arrastrar OpenSSL/`aws-lc-sys` y conservar el binario
  estático musl.
- **Limitación documentada**: el `StateStore` de `state`/`nonce` vive en el proceso; en un
  despliegue multi-instancia hacen falta *sticky sessions* o un almacén compartido.

## Referencias

- `/data/rust/valet` — servidor de referencia (paridad de diseño).
- `plans/PLAN-005-oidc-pocket-id-COMPLETED.md` — F04, modelo superado por F17.
- `plans/PLAN-016-packaging-deploy-COMPLETED.md` — F15, orden de build `web/dist` →
  `cargo build` (`rust-embed`).
- `AGENTS.md` — stack, protocolos, layout y flujo OpenSpec/TDD.
- RFC 6749 §2.1 (roles de cliente), RFC 8414 (metadata de autorización, `same_origin`).
