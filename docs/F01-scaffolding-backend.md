# F01 — Scaffolding backend

> [!warning] Modelo superado por F17
> Documento **histórico**: `OXIDAV_POCKET_ID_ISSUER` fue retirado junto con el
> resto del modelo `Bearer`; el vigente está en
> [`F17 — OIDC con paridad de valet`](./F17-oidc-valet-parity.md).

## Qué hace

Levanta el esqueleto del servidor OxiDav: un binario Axum + Tokio (edition 2024) que carga su
configuración desde variables de entorno, construye un `Router` con el endpoint de salud
`/healthz`, y arranca con `tracing` y apagado ordenado ante `SIGINT`/`SIGTERM`. Sirve como raíz
del roadmap: sin este esqueleto no arranca ninguna otra feature. No incluye persistencia real
(la `database_url` es un placeholder que consume F03) ni autenticación (F04/F05).

## Contratos e interfaces

- `struct Config { bind_addr: SocketAddr, log_level: String, database_url: String, pocket_id_issuer: String }`
- `ConfigError` enum con variantes `MissingVar { name }` / `InvalidValue { name, reason }` (implementa `Error` + `Display` vía `thiserror`).
- `Config::from_env() -> Result<Config, ConfigError>`
- `AppState` (envoltura de `Arc<Config>`, clonable barato)
- `build_router(state: AppState) -> axum::Router`

## Endpoints

- `GET /healthz` → `200` con `content-type: application/json` y body `{"status":"ok"}`. Ruta desconocida → `404`.

## Variables de entorno

| Variable | Default | Comportamiento |
|---|---|---|
| `OXIDAV_BIND_ADDR` | `0.0.0.0:8080` | Vacío ≡ ausente → default |
| `OXIDAV_DATABASE_URL` | `sqlite://data/oxidav.db` | Placeholder, lo consumirá F03 |
| `OXIDAV_POCKET_ID_ISSUER` | — (obligatoria) | Sin default → `MissingVar`; la usará F04 |
| `RUST_LOG` | `info` | Filtro de `tracing-subscriber` (env-filter) |

## Arranque y apagado

El runtime es Tokio multihilo (`#[tokio::main]`). El logging usa `tracing` +
`tracing-subscriber` con `EnvFilter` alimentado por `RUST_LOG`. El servidor Axum se lanza con
apagado ordenado: escucha `SIGINT` y `SIGTERM` y, al recibirlos, deja de aceptar conexiones y
finaliza la future del servidor limpiamente. Si falla la instalación del handler de señal, se
loguea el error y el proceso **sigue corriendo** (la future de apagado no completa, pero el
servidor no se cae).

## Estructura de módulos

| Fichero | Rol |
|---|---|
| `src/main.rs` | Binario: arranque, runtime, señales |
| `src/lib.rs` | Crate lib, expone `config`/`router` para tests de integración |
| `src/config.rs` | `Config`, `ConfigError`, `from_env` |
| `src/router.rs` | `AppState`, `build_router`, `/healthz` |
| `tests/router.rs` | Tests de integración (HTTP) |

## Cómo ejecutar y verificar

- `cargo test` → **10 tests** (6 unit en lib + 2 de shutdown + 2 de integración).
- `cargo fmt --check`.
- `cargo clippy --all-targets -- -D warnings`.

## Notas y decisiones

- Se añadió `src/lib.rs` para permitir tests de integración con `tower::ServiceExt::oneshot`.
- `tower` (feature `util`) está en `[dev-dependencies]`.
- `tower-http` (feature `cors`) y `serde` (feature `derive`) se retiraron con `// TODO(F06)` hasta que se usen.
- Sin `unwrap`/`expect`/`panic` en código de producción.

## Relacionado

- [`plans/PLAN-002-scaffolding-backend-COMPLETED.md`](../plans/PLAN-002-scaffolding-backend-COMPLETED.md)
- [`plans/PLAN-001-general.md`](../plans/PLAN-001-general.md)
- [`AGENTS.md`](../AGENTS.md)
