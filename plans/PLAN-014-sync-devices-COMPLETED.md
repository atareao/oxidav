# PLAN-014 — F13 REST de app tokens + Vista Sincronización y Dispositivos (SPA)

## Objetivo

Añadir la API REST de gestión de *app tokens* (crear, listar, revocar) y la vista SPA **Sincronización y Dispositivos**, que muestre las URLs DAV copiables, instrucciones para clientes nativos y un gestor de tokens con revelado único del secreto.

## Alcance

**Incluye:**

- Endpoints REST `/api/v1/app-tokens` bajo `require_auth`, scopeados por `AuthUser.sub`:
  - `GET /api/v1/app-tokens` — lista de metadatos de los tokens del usuario.
  - `POST /api/v1/app-tokens` — creación; devuelve el secreto en claro **una sola vez**.
  - `DELETE /api/v1/app-tokens/{id}` — revocación.
  - `PATCH /api/v1/app-tokens/{id}` — (opcional) renombrar; se decide en *Decisiones técnicas*.
- DTOs de API y envelope de error de F06.
- Vista SPA `web/src/pages/Sync.tsx` y servicio `web/src/services/appTokens.ts`.
- Tests backend (unitarios/integración) y frontend (Vitest + RTL).

**No incluye:**

- Cambios en la lógica de hashing/verificación de `src/auth/app_tokens.rs` (ya existe).
- Flujo OIDC Authorization Code + PKCE completo (heredado de F04; ver *Riesgos*).
- Cambios en las vistas Calendario (F11) ni Contactos (F12) más allá de reutilizar patrones.
- Gestión de tokens de otros usuarios (prohibido por diseño).

## Decisiones técnicas

- **Reutilización de la capa existente.** Los handlers REST delegan en `create_app_token`, `list_app_tokens`, `revoke_app_token` y `verify_app_token` de `src/auth/app_tokens.rs`. No se duplica la lógica de generación ni de verificación (comparación en tiempo constante ya implementada).
- **Scopeado por usuario (anti-IDOR).** Todas las operaciones toman el `sub` como propietario; las consultas incluyen siempre el filtro por `user_id`. Un token de otro usuario se trata como inexistente (`404`), nunca como acceso denegado (`403`), para no filtrar existencia.
- **Revelado único del secreto.** El formato completo es `oxidav_<id>_<secret>`. El `POST` es el único punto donde se devuelve el secreto en claro (y el token completo). Los DTOs de listado (`AppTokenMetadata`) no contienen secreto ni hash; el DTO de creación (`AppTokenCreated`) sí.
- **`PATCH` de renombrado: se implementa.** Es de bajo coste, mejora la usabilidad y encaja con el modelo (`name` editable). Validación de `name` no vacío (`400`). Devuelve `200` con los metadatos actualizados.
- **Envelope de error (F06).** Todas las respuestas de error usan `{"error":{"code","message"}}`. Códigos: `400` (payload inválido), `401` (no autenticado), `404` (token inexistente o de otro usuario).
- **DTOs limpios.** El cliente nunca recibe el `secret_hash` ni columnas internas. Serialización explícita de campos permitidos.
- **Frontend — patrón heredado de F11/F12.** Se reutiliza `App.useApp()` para notificaciones/modales de Antd, `confirmLoading` en botones de acción asíncrona, refresco de datos tras mutaciones y `AbortController` para cancelar peticiones en desmontaje.
- **URLs DAV absolutas.** La vista compone la URL absoluta a partir de `window.location.origin` (o valor configurable), formando `/dav/calendars/<usuario>/` y `/dav/addressbooks/<usuario>/`.
- **Secreto en UI no persistente.** El diálogo de creación muestra el token en claro una única vez; no se guarda en estado global, ni en `localStorage`, ni en logs. Únicamente expone un botón de copiar al portapapeles.

## Contratos / Interfaces

### Endpoints REST (bajo `/api/v1`, `require_auth`)

| Método | Ruta | Éxito | Errores |
|---|---|---|---|
| `GET` | `/api/v1/app-tokens` | `200` lista de `AppTokenMetadata` | `401` |
| `POST` | `/api/v1/app-tokens` | `201` `AppTokenCreated` (secreto en claro) | `400`, `401` |
| `PATCH` | `/api/v1/app-tokens/{id}` | `200` `AppTokenMetadata` | `400`, `401`, `404` |
| `DELETE` | `/api/v1/app-tokens/{id}` | `204` sin cuerpo | `401`, `404` |

### DTOs (Rust)

```rust
// Metadatos: NUNCA incluye secreto ni hash.
struct AppTokenMetadata {
    id: String,
    name: String,
    created_at: String,   // ISO-8601
    last_used_at: Option<String>, // ISO-8601, null si nunca usado
}

// Respuesta de creación: único punto donde aparece el secreto.
struct AppTokenCreated {
    id: String,
    name: String,
    token: String,        // oxidav_<id>_<secret>  (secreto en claro, una sola vez)
    created_at: String,
}

struct CreateAppTokenRequest {
    name: String,         // no vacío → si no, 400
}

struct RenameAppTokenRequest {
    name: String,         // no vacío → si no, 400
}
```

### Envelope de error (F06)

```json
{ "error": { "code": "string_identificador", "message": "descripción legible" } }
```

### Frontend — servicio `web/src/services/appTokens.ts`

```ts
export interface AppTokenMetadata { id: string; name: string; createdAt: string; lastUsedAt: string | null }
export interface AppTokenCreated { id: string; name: string; token: string; createdAt: string }

listAppTokens(signal?: AbortSignal): Promise<AppTokenMetadata[]>
createAppToken(name: string, signal?: AbortSignal): Promise<AppTokenCreated>
renameAppToken(id: string, name: string, signal?: AbortSignal): Promise<AppTokenMetadata>
revokeAppToken(id: string, signal?: AbortSignal): Promise<void>
```

### Frontend — vista `web/src/pages/Sync.tsx`

- Sección **URLs DAV**: CalDAV `/dav/calendars/<usuario>/`, CardDAV `/dav/addressbooks/<usuario>/`, compuestas desde `window.location.origin`; botón copiar por URL.
- Sección **Instrucciones**: notas breves para iOS, DAVx⁵ y Thunderbird (Basic Auth con app token).
- Sección **App tokens**: tabla (nombre, creado, último uso), botón «Crear» (Modal con campo nombre), diálogo de revelado único con aviso y botón copiar, botón revocar con confirmación.

## Tareas (TDD)

### Backend

- [x] **Paso 1 (RED):** Escribir tests de `GET /api/v1/app-tokens`: devuelve solo los tokens del usuario autenticado; el listado **no** expone secreto ni hash.
- [x] **Paso 2 (RED):** Test anti-IDOR con dos usuarios: el usuario A **no** ve ningún token de B en el listado.
- [x] **Paso 3 (RED):** Escribir tests de `POST /api/v1/app-tokens`: responde `201`, devuelve `id` y token completo `oxidav_<id>_<secret>`; el valor creado funciona con `verify_app_token`; verificar que el secreto solo aparece en esta respuesta.
- [x] **Paso 4 (RED):** Validación `400` en `POST` con `name` vacío o payload inválido; `401` sin autenticación; envelope `{"error":{"code","message"}}`.
- [x] **Paso 5 (RED):** Escribir tests de `DELETE /api/v1/app-tokens/{id}`: `204` al revocar token propio (y el token deja de verificar); `404` para id inexistente; `404` para token de otro usuario (anti-IDOR).
- [x] **Paso 6 (RED):** Tests de `PATCH /api/v1/app-tokens/{id}`: `200` renombra y devuelve metadatos; `400` nombre vacío; `404` id inexistente o de otro usuario.
- [x] **Paso 7 (GREEN):** Implementar los handlers REST delegando en `src/auth/app_tokens.rs`, con DTOs y envelope de F06; registrar rutas en el router `/api/v1` bajo `require_auth`.
- [x] **Paso 8 (GREEN):** `cargo test` — todos los tests verdes, sin romper los 346 existentes.
- [x] **Paso 9 (REFACTOR):** `cargo clippy -- -D warnings` y `cargo fmt --check`.

### Frontend

- [x] **Paso 10 (RED):** Test RTL: la vista renderiza las URLs DAV (CalDAV y CardDAV) compuestas desde `window.location.origin` y el botón copiar.
- [x] **Paso 11 (RED):** Test RTL: «Crear token» abre Modal con nombre, llama a `createAppToken` y muestra el diálogo de revelado único con el token en claro y el aviso de copiarlo.
- [x] **Paso 12 (RED):** Test RTL: revocar abre confirmación y, al aceptar, llama a `revokeAppToken` y refresca la tabla.
- [x] **Paso 13 (GREEN):** Implementar `web/src/services/appTokens.ts` y sustituir el placeholder de `web/src/pages/Sync.tsx` reutilizando el patrón de F11/F12 (`App.useApp()`, `confirmLoading`, refresco, `AbortController`).
- [x] **Paso 14 (GREEN):** `npx vitest run` verde y `npx tsc --noEmit` sin errores.
- [x] **Paso 15 (REFACTOR):** `npm run lint` sin warnings y `npm run build` correcto.

## Criterios de aceptación

- Backend verde: `cargo test` pasa al 100 % sin romper los 346 tests existentes.
- Frontend verde: `npx vitest run`, `npx tsc --noEmit` y `npm run build` correctos; `npm run lint` sin warnings.
- El secreto en claro aparece **solo** en la respuesta de `POST /api/v1/app-tokens`; nunca en listados, renombrado ni logs.
- Los DTOs de listado/renombrado no exponen `secret_hash` ni el secreto.
- Revocación y renombrado scopeados por usuario: un usuario no puede ver, renombrar ni revocar tokens de otro (`404`).
- Envelope de error uniforme `{"error":{"code","message"}}` en `400/401/404`.
- La vista muestra URLs DAV copiables, instrucciones para clientes y gestor de tokens funcional.

## Riesgos

- **Fuga del secreto.** Que el token en claro acabe en logs, caché HTTP, estado global de React o en el DTO de listado. Mitigación: revelado único en `POST`, diálogo no persistente en la UI, revisión de que ningún `tracing`/`console.log` incluya el secreto, y DTOs explícitos.
- **Revocación no scopeada (IDOR).** Que un usuario pueda revocar/renombrar tokens de otro. Mitigación: filtro por `user_id` en todas las consultas y tests anti-IDOR explícitos con dos usuarios.
- **Compatibilidad con el flujo OIDC PKCE aún no implementado** (heredado de F04). La vista y el uso de `require_auth` asumen un usuario autenticado; en ausencia de PKCE completo se opera en modo dev / token mínimo. Documentar el modo y no bloquear la feature.
- **Divergencia de contratos Rust/TS.** Desalineación entre los DTOs del backend y las interfaces de `appTokens.ts`. Mitigación: contratos explícitos en este plan y tests de serialización.
- **Regresión en la capa existente.** Tocar `src/auth/app_tokens.rs` podría afectar la autenticación DAV. Mitigación: no modificar la lógica de hashing/verificación; solo consumir sus funciones públicas.

## Referencias

- `plans/PLAN-001-general.md` — desglose de features (F13).
- `plans/PLAN-012-calendar-view-COMPLETED.md` — patrón de vista SPA (F11).
- `plans/PLAN-013-contacts-view-COMPLETED.md` — patrón de vista SPA (F12).
- `docs/F05-app-tokens-basic-auth.md` — app tokens y Basic Auth.
- `docs/F06-rest-api.md` — REST `/api/v1`, `require_auth` y envelope de error.
- `docs/F07-caldav-core.md` — CalDAV `/dav/calendars/{username}/`.
- `docs/F08-carddav-core.md` — CardDAV `/dav/addressbooks/{username}/`.
- `web/README.md` — convenciones del frontend (Vite + React 19 + Antd 6).
- `AGENTS.md` — decisiones vinculantes del proyecto.
