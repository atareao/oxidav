# PLAN-005 — F04 Autenticación OIDC (Pocket ID)

## Objetivo

Implementar la **verificación de tokens OIDC** emitidos por Pocket ID para proteger
la API REST de la SPA (`/api/v1`). El backend debe validar la firma **RS256** de cada
Bearer JWT contra el JWKS publicado por el IdP, comprobar los claims esenciales
(`iss`, `aud`, `exp`/`nbf`/`iat`) y exponer el resultado como un extractor de Axum
(`AuthUser`) reutilizable por los handlers. La feature no emite tokens ni gestiona
sesiones: solo los consume y los rechaza de forma segura (*fail-closed*).

## Alcance

- Módulo `src/auth/oidc.rs` (registrado en `src/auth/mod.rs`): configuración OIDC,
  descubrimiento del `jwks_uri`, verificación de firma y claims, tipo `AuthUser`.
- **JWKS fetcher**: descarga del documento de descubrimiento
  (`{issuer}/.well-known/openid-configuration`) y de `jwks_uri` con `reqwest`.
- **Cache de claves** con TTL, indexada por `kid`, con refresco ante `kid` desconocido
  (rotación de claves del IdP).
- **Verificación de firma** (RS256) y de **claims** (`iss`, `aud`, `exp`, `nbf`, `iat`)
  con tolerancia de *clock skew*.
- **Extractor `AuthUser`** (`impl FromRequestParts<AppState>`) que parsea
  `Authorization: Bearer <jwt>` y devuelve `401` + `WWW-Authenticate: Bearer` cuando
  falta o es inválido.
- **Middleware/función `require_auth`** aplicable a un `Router` de Axum, más un
  **router `/api/v1` de ejemplo** con un handler protegido para demostrar la
  integración end-to-end (F06 traerá los endpoints REST reales).
- Tests unitarios/integración que **no dependen de red**: par de claves RS256 generado
  en el propio test, JWKS local y JWT firmado en memoria.

**FUERA DE ALCANCE:**

- Flujo **Authorization Code + PKCE del navegador** (vive en el frontend, `web/`).
- **Basic Auth con app tokens** para clientes DAV nativos (`/dav`, F05).
- **Endpoints REST de negocio** de la SPA (`/api/v1/...`, F06).
- Emisión, rotación o almacenamiento de credenciales por parte de OxiDav.

## Decisiones técnicas

- **Crates:**
  - `jsonwebtoken` — decodificación y verificación de JWT (`Validation`, `DecodingKey`).
  - `reqwest` con **`rustls-tls`** (sin OpenSSL) — descarga de `openid-configuration`
    y JWKS; coherente con el objetivo de binario distribuible estático.
  - Cache: `moka` (cache asíncrona con TTL nativo) **o** `DashMap` **o**
    `RwLock<HashMap<String, CachedKey>>` + `Instant` de expiración. Criterio: si
    `moka` añade demasiado peso, usar `RwLock<HashMap>` con TTL calculado a mano.
  - Serialización/deserialización: `serde` / `serde_json`.
- **Algoritmo:** **RS256** exclusivamente (algoritmos asimétricos de Pocket ID).
  Se restringe `Validation::algorithms` a `[Algorithm::RS256]`, con lo que se
  **rechazan `alg=none` y algoritmos simétricos (HS256/HS384/HS512)** — incluida la
  variante de confundir una clave pública RSA usada como secreto HMAC.
- **Selección de clave:** se toma el `kid` del header del JWT y se busca en la cache.
  Si falta, se fuerza un **refresh** del JWKS y se reintenta **una sola vez**; si el
  `kid` sigue ausente → `401`.
- **Política de cache/rotación:**
  - TTL configurable (por defecto ~15 min) para el conjunto de claves.
  - Refresco *on-demand* ante `kid` desconocido (cubre rotaciones sin reiniciar).
  - *Coalescing* del refresco para evitar tormentas de peticiones concurrentes.
  - Las claves se mantienen válidas hasta que expira su TTL; un `kid` nuevo obliga a
    refrescar aunque el TTL no haya vencido.
- **Fail-closed:** si el IdP no responde y **no** hay claves cacheadas válidas, toda
  petición autenticada se rechaza con `401` (nunca se "acepta por defecto").
- **Tolerancia de reloj (clock skew):** margen pequeño (por defecto `60s`) aplicado a
  `exp`, `nbf` e `iat` mediante `Validation::leeway`.
- **Audience:** validar `aud` contra el *client id* de la SPA cuando Pocket ID lo
  emite; si `audience` es `None` en configuración, se omite esa validación (con
  advertencia en logs).

## Contratos / Interfaces

- **Configuración:**

  ```rust
  pub struct OidcConfig {
      pub issuer: String,          // = Config.pocket_id_issuer
      pub audience: Option<String>,// client id de la SPA (si Pocket ID lo emite)
      pub jwks_uri: String,        // resuelto desde /.well-known/openid-configuration
  }
  ```

- **Cache de claves:**

  ```rust
  pub struct JwksCache { /* claves por kid + expiración */ }

  impl JwksCache {
      pub async fn get_key(&self, kid: &str) -> Option<DecodingKey>;
      pub async fn refresh(&self) -> Result<(), AuthError>;
  }
  ```

- **Claims:**

  ```rust
  #[derive(Deserialize)]
  pub struct Claims {
      pub sub: String,
      pub iss: String,
      pub aud: Option<serde_json::Value>, // string o array
      pub exp: u64,
      pub nbf: Option<u64>,
      pub iat: Option<u64>,
  }
  ```

- **Extractor:**

  ```rust
  pub struct AuthUser { pub sub: String }

  impl FromRequestParts<AppState> for AuthUser { /* ... */ }
  ```

- **Middleware/función:**

  ```rust
  pub fn require_auth(router: Router<AppState>) -> Router<AppState>;
  ```

- **Códigos de respuesta:**
  - Token **ausente**, **malformado** o **inválido** (firma/claims) → `401`
    con cabecera `WWW-Authenticate: Bearer`.
  - Token **válido pero sin permisos** → `403` (reservado; F06 definirá la
    autorización por rol/colección).

## Tareas (TDD)

- [x] **Paso 1 — Parseo de `Authorization: Bearer`** (test-first).
      Casos: cabecera presente y correcta; ausente; esquema distinto de `Bearer`;
      valor vacío; espacios extra. Esperado: `401` + `WWW-Authenticate: Bearer` en
      los casos inválidos.
- [x] **Paso 2 — Verificación de firma con clave de test local.**
      Generar un par RS256 dentro del test, construir un **JWKS local** en memoria,
      firmar un JWT con la clave privada y validarlo con `JwksCache`. Sin acceso a red.
- [x] **Paso 3 — Rechazo de algoritmos no permitidos.**
      Tests con `alg=none` y `HS256` (incluida la trampa de usar la clave pública RSA
      como secreto HMAC) → deben fallar con `401`.
- [x] **Paso 4 — Validación de claims.**
      `iss` erróneo, `exp` caducado, `nbf` futuro, `aud` incorrecto → `401`.
      Verificar también el comportamiento dentro de la tolerancia de clock skew.
- [x] **Paso 5 — Selección por `kid` y rotación.**
      Token cuyo `kid` coincide con la cache → OK. Token con `kid` desconocido →
      se invoca `refresh()` una vez y, si aparece, se valida; si no, `401`.
- [x] **Paso 6 — Fail-closed.**
      Simular fallo de descarga del JWKS (mock de `reqwest`/servidor de test) sin
      claves cacheadas → toda petición autenticada responde `401`.
- [x] **Paso 7 — Extractor sobre handler de prueba.**
      Router de ejemplo con un handler protegido: petición con token válido → `200`;
      sin token → `401`; con token válido contra un handler que exige permisos
      (reservado) → `403`.
- [x] **Paso 8 — Calidad.**
      `cargo fmt --check`, `cargo clippy -- -D warnings` y `cargo test` en verde.

## Criterios de aceptación

- Existe `src/auth/oidc.rs` (registrado en `src/auth/mod.rs`) con la verificación
  OIDC y el extractor `AuthUser`.
- Un JWT RS256 válido y con claims correctos se acepta y produce `AuthUser { sub }`.
- `alg=none` y cualquier algoritmo simétrico (`HS*`) son rechazados con `401`.
- `iss` distinto de `Config.pocket_id_issuer`, `exp` caducado o `aud` incorrecto
  producen `401`.
- La cache selecciona la clave por `kid`; un `kid` desconocido dispara un refresco
  del JWKS y, si sigue ausente, `401`.
- Si la descarga del JWKS falla y no hay claves cacheadas válidas, la respuesta es
  `401` (*fail-closed*); nunca se autoriza por defecto.
- Las respuestas de rechazo incluyen `WWW-Authenticate: Bearer`.
- Un router `/api/v1` de ejemplo queda protegido por `require_auth`, demostrando
  `200` con token válido y `401` sin él.
- Toda la suite (`cargo test`) pasa **sin acceso a red**.
- `cargo fmt --check` y `cargo clippy -- -D warnings` sin warnings.

## Riesgos

- **Cache y rotación JWKS:** una cache demasiado larga retrasa la aceptación de claves
  nuevas; una demasiado corta golpea el IdP. Mitigación: TTL moderado + refresco
  on-demand por `kid` desconocido + coalescing.
- **Clock skew:** relojes desincronizados entre backend e IdP provocan falsos `401`.
  Mitigación: `leeway` configurable (~60s).
- **Disponibilidad del IdP (fail-closed vs caché):** un IdP caído con cache fría
  bloquea toda la API; con cache caliente se sigue operando hasta el TTL. Es el
  compromiso aceptado: la seguridad prima sobre la disponibilidad.
- **Validación de `aud`:** si Pocket ID no emite `aud` o lo emite como array, una
  validación estricta puede romper el flujo. Mitigación: `audience: Option<String>` y
  parseo tolerante (`string` o array).
- **Dependencia de `reqwest`/rustls y binario estático:** elección de `rustls` para
  evitar OpenSSL y facilitar el binario embebido; vigilar el peso y las features
  activadas de `reqwest`.
- **Tests sin red:** toda la verificación de firma debe usar claves generadas en el
  test y un JWKS local; no debe haber dependencia de Pocket ID real en CI.

## Referencias

- `plans/PLAN-001-general.md` (F04).
- `docs/F01-scaffolding-backend.md` (`Config.pocket_id_issuer`, `AppState`).
- `AGENTS.md` (stack, protocolos y flujo OpenSpec/TDD).
