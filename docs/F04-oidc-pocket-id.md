# F04 — Autenticación OIDC (Pocket ID)

> [!warning] Modelo superado por F17
> Este documento es **histórico**. El modelo que describe —el SPA hace
> Authorization Code + PKCE y envía `Authorization: Bearer <JWT>`, que el backend
> verifica contra el JWKS de Pocket ID— quedó **superado por
> [`F17 — OIDC con paridad de valet`](./F17-oidc-valet-parity.md)**, donde el
> backend hace el flujo server-side con cliente confidencial y una cookie de sesión
> `HttpOnly`. El soporte `Bearer` ya no existe y `OXIDAV_POCKET_ID_ISSUER` /
> `OXIDAV_OIDC_AUDIENCE` fueron retirados. Se conserva tal cual el resto del
> documento, como registro.

## Qué hace

Verifica los JWT **Bearer** que la SPA obtiene de Pocket ID, contra el JWKS del IdP,
y protege `/api/v1`. No implementa el flujo PKCE del navegador (vive en el frontend)
ni la Basic Auth de `/dav` (F05).

## Decisiones técnicas

- `jsonwebtoken 11` (feature `rust_crypto`, backend puro Rust, sin OpenSSL) verificando
  **RS256**; rechaza `alg=none` y algoritmos simétricos.
- `reqwest 0.13` (json + rustls) para discovery OIDC y descarga del JWKS.
- `JwksCache` con **caché TTL**, selección por `kid`, **refresco ante `kid` desconocido**
  y **backoff** para evitar tormentas de refresco si el IdP está caído.
- **Fail-closed**: si no hay claves válidas cacheadas, se rechaza.
- Validaciones: `iss` (== `pocket_id_issuer`, normalizado sin barra final), `aud`
  (opcional, ver env var), `exp`/`nbf`/`iat` con tolerancia de clock skew, y `sub` no
  vacío.

## Configuración

- `OXIDAV_POCKET_ID_ISSUER` (obligatoria; ya de F01).
- `OXIDAV_OIDC_AUDIENCE` (opcional; si se define, se valida `aud` estrictamente).

## Contratos

- `OidcConfig { issuer, audience: Option<String>, jwks_uri }` (`new`/`discover`).
- `JwksCache` (`get_key(kid)` + `refresh()`).
- `Claims`, `AuthUser { sub }` con `impl FromRequestParts<AppState>`, y `require_auth`
  (middleware con `from_fn_with_state` de axum 0.8).
- Errores: `401` + `WWW-Authenticate: Bearer`, con **cuerpo genérico** (sin filtrar
  detalles internos como la URL del JWKS o el modo de fallo; el detalle solo va a
  `tracing`).

## Integración

- `AppState` incorpora `oidc`/`jwks` (y el pool `db`). `AppState::from_config` construye
  un estado **offline** (sin red).
- `main.rs` hace discovery OIDC **best-effort con timeout (5 s) y no fatal**: si el IdP
  no responde, arranca igual y los tokens se rechazan fail-closed hasta que sea
  alcanzable.

## Cómo verificar

`cargo test` (suite completa; los tests de OIDC no usan red: generan un par RS256 y un
JWKS local), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

## Relacionado

- [`plans/PLAN-005-oidc-pocket-id-COMPLETED.md`](../plans/PLAN-005-oidc-pocket-id-COMPLETED.md)
- [`plans/PLAN-001-general.md`](../plans/PLAN-001-general.md)
- [`docs/F01-scaffolding-backend.md`](./F01-scaffolding-backend.md)
- [`AGENTS.md`](../AGENTS.md)
