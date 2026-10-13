# PLAN-006 — F05 App tokens + Basic Auth DAV

## Objetivo

Implementar el sistema de **app tokens revocables** que permite a los clientes DAV
nativos (iOS CalDAV/CardDAV, DAVx⁵, Thunderbird) autenticarse contra `/dav` con
**Basic Auth**, sin almacenar ni transmitir la contraseña principal del usuario
(gestionada íntegramente por Pocket ID). El plan entrega el almacén de tokens en
SQLite, la generación/hashing/verificación de secretos de alta entropía y el
middleware/extractor que resuelve `Authorization: Basic` a un `AuthUser { sub }`.
No cubre JWT de la SPA (F04) ni los handlers DAV concretos (F07/F08).

## Alcance

- **Nueva migración** `migrations/0002_app_tokens.sql` que crea la tabla
  `app_tokens` y sus índices de consulta.
- **Módulo** `src/auth/app_tokens.rs`: generación de tokens, hashing y
  verificación del secret, revocación, listado y resolución de un token completo
  a `AuthUser`.
- **Módulo** `src/auth/basic.rs`: extracción y validación de la cabecera
  `Authorization: Basic`, decodificación base64 de `username:token` y producción
  de `AuthUser` (o `401` con `WWW-Authenticate`).
- **Repos** sobre la capa de datos existente (F03: `init_pool`, `DbError`, repos)
  para `app_tokens`, sin acceso SQL nuevo fuera del repositorio.
- **Tipos compartidos** `AuthUser { sub }` y `AuthError`/`TokenError` con
  `thiserror`, reutilizables por F04.

**FUERA de alcance:**

- Endpoints HTTP REST de gestión de tokens (crear/listar/revocar desde la SPA):
  se exponen en **F06/F13**.
- Handlers y lógica DAV concreta (`PROPFIND`, `REPORT`, `PUT`, `sync-collection`):
  **F07/F08**.
- Verificación del JWT de la SPA (Authorization Code + PKCE contra el JWKS de
  Pocket ID): **F04**.
- Alta/baja de usuarios y cualquier gestión de la contraseña principal
  (propiedad de Pocket ID).

## Decisiones técnicas

### Formato del token

El token completo tiene la forma `oxidav_<id>_<secret>`:

- **Prefijo fijo `oxidav_`**: facilita el reconocimiento visual y permite
  rechazar tempranamente identificadores con formato ajeno.
- **`<id>`** — identificador **público** (no secreto) usado para **localizar la
  fila** en `app_tokens` mediante una búsqueda indexada O(log n) antes de
  comparar el secret. Se genera con CSPRNG y se codifica en Base64 URL-safe o
  hex (p. ej. 16 bytes → 22–32 caracteres). Puede aparecer en logs sin riesgo.
- **`<secret>`** — secreto de **alta entropía ≥256 bits** generado con CSPRNG
  (`getrandom` / `rand::rngs::OsRng`), codificado en Base64 URL-safe sin padding.
  El `<secret>` **nunca se persiste en claro**: solo su hash.
- El parseo del token se hace con `splitn(3, '_')` + validación de prefijo, y es
  estricto con el número de segmentos.

### Hashing y comparación

- **SHA-256 (o HMAC-SHA256 con pepper) + comparación en tiempo constante.**
  El secret es una cadena aleatoria de ≥256 bits producida por un CSPRNG, **no
  una contraseña elegida por un humano**. No existe un espacio de candidatos
  enumerable ni atacable por diccionario, por lo que no aporta nada aplicar una
  KDF con coste (Argon2/scrypt/PBKDF2): no hay nada que "endurecer".
- **Se usa `SHA-256`** sobre el secret (o `HMAC-SHA256` con un pepper del
  servidor) y la comparación del digest almacenado contra el digest calculado se
  realiza con la crate **`subtle`** (`ConstantTimeEq`) para evitar filtrar
  información por diferencias temporales.
- **Punto revisable:** la elección SHA-256 frente a una KDF costosa se documenta
  como decisión explícita y **revisable**. Si en el futuro el formato cambiara a
  secretos derivados de algo de baja entropía (p. ej. códigos cortos), habría que
  migrar a Argon2id. El campo `secret_hash` incluiría entonces un prefijo de
  algoritmo (p. ej. `$argon2id$...`) para permitir coexistencia durante la
  migración; **hoy se almacena hex plano del SHA-256** y se reserva la
  posibilidad de versionar el formato.

### Pepper opcional

- El pepper se lee de la variable de entorno **`OXIDAV_TOKEN_PEPPER`**.
- Si está presente, se usa como clave de `HMAC-SHA256` (en lugar de SHA-256
  directo): añade una capa de defensa de "secreto del servidor" que un dump de la
  BD no compromete por sí solo.
- Si está ausente, se degrada a `SHA-256(secret)` documentando la pérdida de esa
  capa. El pepper **no se almacena en la BD**; si se pierde o rota, los tokens
  existentes dejan de verificar (se debe documentar el procedimiento de rotación:
  regenerar tokens o mantener el pepper antiguo durante una ventana).

### Rotación y revocación

- **Revocación**: borrado lógico marcando `revoked_at` (epoch segundos). Un token
  revocado falla la verificación inmediatamente (`AuthError::TokenRevoked`), sin
  necesidad de reiniciar el servidor, porque la comprobación va a la BD.
- **Rotación**: un usuario puede crear varios tokens con `name` distintivo y
  revocar los antiguos. No hay expiración temporal automática en F05 (se puede
  añadir después sobre `created_at`).
- La revocación y el listado se filtran siempre por `user_id` para impedir que un
  token revoque o vea los de otro usuario.

### Basic Auth

- Se parsea `Authorization: Basic <base64>`, se decodifica el base64 y se parte
  por el **primer `:`** en `username:token` (`splitn(2, ':')`), de modo que el
  username puede contener `:` sin romper el parseo y el token se toma como el
  resto completo.
- El `username` **no identifica al usuario** por sí mismo: es informativo para el
  cliente. La identidad real (`user_id = claim sub`) sale del token verificado en
  la BD. Si se quisiera reforzar, se puede comprobar que el `username` coincide
  con el del token cuando exista ese mapeo, pero no es obligatorio en F05.
- **Respuesta de error:** cualquier fallo (cabecera ausente, esquema distinto de
  `Basic`, base64 inválido, token inexistente/malformado/revocado) produce
  **`401 Unauthorized`** con la cabecera
  **`WWW-Authenticate: Basic realm="oxidav", charset="UTF-8"`**. El cuerpo del
  error es el mínimo posible y **no distingue** entre "no existe" y "revocado"
  hacia el cliente, para no filtrar información.
- La cabecera `Authorization` y los secretos **nunca se loguean**.

### Actualización de `last_used_at`

- Al verificar correctamente un token se actualiza `last_used_at` de forma
  **best-effort**: la escritura se hace en un `tokio::spawn` (fire-and-forget) o
  mediante un canal a una tarea de fondo, **sin bloquear ni hacer fallar** la
  petición si la escritura falla.
- Se documenta el compromiso: en modo WAL las escrituras concurrentes pueden
  serializarse; se acepta que `last_used_at` tenga granularidad gruesa y que se
  pueda perder alguna actualización bajo alta concurrencia. Un fallo al actualizar
  no debe degradar la autenticación.

## Esquema (migración 0002)

Nueva migración `migrations/0002_app_tokens.sql`. Debe ser **idempotente** (uso
de `IF NOT EXISTS`) y compatible con el ejecutor de migraciones de F03.

```sql
-- migrations/0002_app_tokens.sql
-- F05: app tokens revocables para clientes DAV nativos (Basic Auth).

CREATE TABLE IF NOT EXISTS app_tokens (
    id            TEXT    PRIMARY KEY,   -- parte PÚBLICA del token: oxidav_<id>_<secret>
    user_id       TEXT    NOT NULL,      -- claim `sub` de Pocket ID (identidad real)
    name          TEXT    NOT NULL,      -- etiqueta legible: "iPhone", "DAVx5", "Thunderbird"
    secret_hash   TEXT    NOT NULL,      -- SHA-256/HMAC-SHA256 (hex) del <secret>; NUNCA el secret en claro
    created_at    INTEGER NOT NULL,      -- epoch en segundos
    last_used_at  INTEGER,               -- epoch en segundos; NULL si nunca se ha usado
    revoked_at    INTEGER                -- epoch en segundos; NULL = activo
);

-- Búsqueda de tokens por usuario (listado / "mis dispositivos").
CREATE INDEX IF NOT EXISTS idx_app_tokens_user_id
    ON app_tokens (user_id);

-- Filtrado de tokens activos por usuario (listado y chequeos de estado).
CREATE INDEX IF NOT EXISTS idx_app_tokens_user_active
    ON app_tokens (user_id, revoked_at);
```

Notas sobre el esquema:

- **`id` es la parte pública** del token (`oxidav_<id>_<secret>`) y actúa como
  clave de búsqueda: gracias a la `PRIMARY KEY` (que crea implícitamente un índice
  único) el lookup por `id` antes de la comparación en tiempo constante es O(log n)
  y no requiere índice adicional. **La unicidad de `id` está garantizada por la
  PK.**
- **`secret_hash`** almacena el digest; **nunca** el secret. La comparación final
  se hace con `subtle` (`ConstantTimeEq`) sobre los bytes del digest.
- Los índices sobre `user_id` cubren listado y revocación; el índice compuesto
  `(user_id, revoked_at)` evita escaneos al filtrar tokens activos.
- `created_at`/`last_used_at`/`revoked_at` son **epoch en segundos** (`INTEGER`),
  coherente con el resto del esquema de F03.

## Contratos / Interfaces

### Modelos

```rust
/// Vista pública de un app token. NUNCA incluye `secret_hash`.
#[derive(Debug, Clone)]
pub struct AppToken {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}
```

### Generación y hashing

```rust
/// Genera un token nuevo con CSPRNG (getrandom / OsRng).
/// Devuelve (token_completo, id, secret).
///   token_completo = "oxidav_<id>_<secret>"
///   id             = identificador público (p. ej. 16 bytes Base64URL)
///   secret         = ≥256 bits de entropía, Base64URL sin padding
pub fn generate_token() -> (String, String, String);

/// Hash del secret para almacenar. Hex de SHA-256(secret) o
/// HMAC-SHA256(pepper, secret) si OXIDAV_TOKEN_PEPPER está definido.
pub fn hash_secret(secret: &str) -> String;

/// Comparación en tiempo constante (crate `subtle`) contra el hash almacenado.
pub fn verify_secret(secret: &str, stored_hash: &str) -> bool;

/// Separa "oxidav_<id>_<secret>" en (id, secret). Err si el formato es inválido.
pub fn parse_token(token: &str) -> Result<(String, String), TokenError>;
```

### Errores

```rust
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("formato de token inválido")]
    Malformed,
    #[error("token no encontrado")]
    NotFound,
    #[error("token revocado")]
    Revoked,
    #[error("secret no coincide")]
    SecretMismatch,
    #[error(transparent)]
    Db(#[from] DbError),
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("cabecera Authorization ausente")]
    MissingHeader,
    #[error("esquema de autenticación no soportado")]
    UnsupportedScheme,
    #[error("credenciales base64 inválidas")]
    InvalidBase64,
    #[error("credenciales malformadas")]
    MalformedCredentials,
    #[error(transparent)]
    Token(#[from] TokenError),
}

impl AuthError {
    /// Cabecera `WWW-Authenticate` a emitir junto al 401.
    pub fn www_authenticate(&self) -> &'static str {
        r#"Basic realm="oxidav", charset="UTF-8""#
    }
}
```

### Repositorio `app_tokens`

```rust
/// Crea un token y devuelve (id, token_completo). `token_completo` es de UN
/// SOLO USO: no se puede volver a obtener porque solo se guarda el hash.
pub async fn create_app_token(
    pool: &SqlitePool,
    user_id: &str,
    name: &str,
) -> Result<(String, String), TokenError>;

/// Lista las vistas públicas de los tokens de un usuario (sin secret_hash).
pub async fn list_app_tokens(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<AppToken>, TokenError>;

/// Marca `revoked_at` si el token pertenece al usuario. Idempotente.
pub async fn revoke_app_token(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> Result<(), TokenError>;

/// Actualiza `last_used_at` (best-effort, se invoca desde tarea de fondo).
pub async fn touch_last_used(pool: &SqlitePool, id: &str) -> Result<(), TokenError>;

/// Parsea el token, localiza por `id`, comprueba `revoked_at` y verifica el
/// secret en tiempo constante. Devuelve la identidad o un error genérico.
pub async fn verify_app_token(
    pool: &SqlitePool,
    token: &str,
) -> Result<AuthUser, TokenError>;
```

### Identidad y extractor

```rust
/// Identidad autenticada. Compartida con F04 (JWT de la SPA).
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub sub: String,
}

/// Extractor Axum 0.8 para rutas /dav protegidas con Basic Auth.
/// Implementa FromRequestParts<AppState>; devuelve 401 + WWW-Authenticate
/// vía IntoResponse para AuthError.
pub struct DavAuth(pub AuthUser);

impl<S> axum::extract::FromRequestParts<S> for DavAuth
where
    S: Send + Sync + AsRef<AppState>,   // ajustar al estado real de F01/F03
{
    type Rejection = AuthError;
    // ... lee "Authorization", valida esquema, decodifica base64,
    //     separa username:token, llama a verify_app_token y, en éxito,
    //     lanza touch_last_used en background ...
}

/// Alternativa/complemento: middleware `require_basic_auth` que puede montarse
/// sobre el subrouter `/dav` si se prefiere a nivel de capa.
pub async fn require_basic_auth(/* State, Request, Next */) -> Response;
```

## Tareas (TDD)

> Orden **test-first**: cada tarea se escribe como test que falle (RED) antes de
> implementar (GREEN), y se refactoriza (REFACTOR) manteniendo verde.

- [x] **Tarea 1 — Migración crea `app_tokens` + índices (idempotente).**
      Test: aplicar `migrations/0002_app_tokens.sql` sobre una BD SQLite
      en memoria/temporal de test; comprobar que `app_tokens` existe con todas
      las columnas, que `idx_app_tokens_user_id` y `idx_app_tokens_user_active`
      existen, y que **re-aplicarla no falla** (idempotencia).
      Comando: `cargo test --test app_tokens_migration` (o test en `src/db/`).

- [x] **Tarea 2 — `generate_token` produce `oxidav_<id>_<secret>` con entropía y
      unicidad esperadas.**
      Test: parsear el resultado, comprobar prefijo `oxidav_`, que `id` y
      `secret` no son vacíos, que el secret decodificado tiene ≥32 bytes (≥256
      bits), y que generar N=1000 tokens no repite `id` ni token completo.

- [x] **Tarea 3 — `hash_secret` / `verify_secret`.**
      Test: `hash_secret(s)` es determinista y estable; `verify_secret(s,
      hash_secret(s))` es `true`; `verify_secret(otro, hash)` es `false`; cadena
      vacía y hash malformado devuelven `false` sin panic. Smoke test de tiempo
      constante (misma ruta de código para acierto/fallo; comparación vía
      `subtle`).

- [x] **Tarea 4 — `create_app_token` guarda el hash y no el secret;
      `verify_app_token` acepta un token recién creado.**
      Test: crear token, consultar la fila y comprobar que `secret_hash` **no
      contiene** el secret en claro y que es igual a `hash_secret(secret)`;
      `verify_app_token(token_completo)` devuelve `AuthUser { sub: user_id }`.

- [x] **Tarea 5 — Revocación y errores de token.**
      Test: tras `revoke_app_token`, `verify_app_token` falla con
      `TokenError::Revoked`; token con `id` inexistente → `TokenError::NotFound`;
      token sin prefijo / sin separadores / con prefijo extraño →
      `TokenError::Malformed`. Verificar que revocar un token de otro usuario no
      afecta (filtrado por `user_id`).

- [x] **Tarea 6 — `touch_last_used` actualiza la marca.**
      Test: recién creado, `last_used_at` es `None`; tras `touch_last_used(id)`,
      `last_used_at` es `Some(t)` con `t >= created_at`. Comprobar que llamarlo
      con un `id` inexistente no produce error irrecuperable (best-effort).

- [x] **Tarea 7 — Parseo de `Authorization: Basic`.**
      Tests de tabla: base64 válido `user:token` → OK; base64 inválido →
      `AuthError::InvalidBase64`; cabecera ausente → `AuthError::MissingHeader`;
      esquema `Bearer ...` → `AuthError::UnsupportedScheme`; sin `:` →
      `MalformedCredentials`. Cada error se convierte en **401** con
      `WWW-Authenticate: Basic realm="oxidav", charset="UTF-8"`.

- [x] **Tarea 8 — Handler de prueba protegido con Basic.**
      Test de integración (router Axum + `tower::ServiceExt::oneshot`): ruta de
      prueba con `DavAuth` → **200** con token válido; **401** sin cabecera; **401**
      con token inválido o revocado. Verificar que en 200 se lanzó (o al menos no
      bloqueó) `touch_last_used`.

- [x] **Tarea 9 — Calidad: `cargo fmt --check` y `clippy -- -D warnings`.**
      Comando: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`.
      Sin warnings ni formateo pendiente.

## Criterios de aceptación

- [x] La migración `0002_app_tokens.sql` crea `app_tokens` y sus índices y es
      idempotente (re-aplicable sin error).
- [x] El `secret_hash` almacenado es **irreversible en claro**: ninguna fila
      contiene el secret; `hash_secret(secret)` no es invertible y no se guarda el
      token completo en ningún sitio persistente.
- [x] La verificación del secret usa **comparación en tiempo constante**
      (`subtle`), sin ramas dependientes del contenido.
- [x] Un token **revocado** no autentica de forma inmediata; un token inexistente
      o malformado tampoco; los errores no filtran si el token "existía".
- [x] Las respuestas de fallo de Basic Auth son **401** con
      `WWW-Authenticate: Basic realm="oxidav", charset="UTF-8"`, y las válidas
      son **200**.
- [x] `touch_last_used` actualiza `last_used_at` sin bloquear ni hacer fallar la
      petición autenticada.
- [x] El prefijo opcional `OXIDAV_TOKEN_PEPPER` se aplica como HMAC cuando está
      presente y se degrada a SHA-256 cuando no, con comportamiento documentado.
- [x] `cargo test` en verde, `cargo fmt --check` limpio y
      `cargo clippy --all-targets -- -D warnings` sin warnings.
- [x] Ninguna cabecera `Authorization` ni secret aparece en logs.

## Riesgos

- **Elección SHA-256 vs KDF costosa.** Justificada para secretos CSPRNG de ≥256
  bits, pero es un **punto revisable**: si el modelo de amenaza o el formato de
  token cambiara (entropía baja), habría que migrar a Argon2id y versionar
  `secret_hash`.
- **Gestión del pepper (`OXIDAV_TOKEN_PEPPER`).** Su ausencia degrada la defensa;
  su pérdida/rotación invalida todos los tokens existentes. Hay que definir y
  documentar el procedimiento de rotación (mantener el antiguo durante una
  ventana o exigir regeneración).
- **Timing attacks.** Mitigado con `subtle`, pero el smoke test no *prueba*
  formalmente constancia temporal; conviene auditar que no haya comparaciones
  `==` accidentales sobre el digest en rutas de error.
- **`last_used_at` bajo concurrencia WAL.** En `journal_mode=WAL` las escrituras
  compiten; se aceptan actualizaciones perdidas o agrupadas y se garantiza que
  nunca afectan al resultado de la autenticación (best-effort).
- **Compatibilidad real de Basic Auth con clientes nativos.** iOS, DAVx⁵ y
  Thunderbird pueden tener particularidades (reintentos, cacheo de credenciales,
  exigencia de realm/charset, comportamiento ante 401). Requiere **pruebas
  manuales** con cada cliente, fuera del alcance de los tests automatizados de
  F05.
- **Fuga de secretos por formato.** El `<id>` es público y puede loguearse, pero
  el token completo y el secret nunca. Riesgo de que código de logging capture la
  cabecera; se debe enmascarar explícitamente.

## Referencias

- `plans/PLAN-001-general.md` — plan general; F05 (app tokens + Basic Auth DAV).
- `docs/F03-data-layer.md` — capa de datos SQLite/sqlx, migraciones, `init_pool`,
  `DbError` y convenciones de repos sobre las que se construye F05.
- `AGENTS.md` — decisiones vinculantes de OxiDav (stack, identidad Pocket ID, app
  tokens revocables, PRAGMAs SQLite, rutas DAV, flujo OpenSpec/TDD).
