# PLAN-002 — F01 Scaffolding backend

## Objetivo

Establecer la base técnica del backend de OxiDav: dependencias mínimas, configuración
por entorno, servidor Axum con apagado ordenado, observabilidad estructurada y un
endpoint de salud. Este plan prepara el terreno para las features posteriores (DB,
auth, DAV y API REST) sin implementar ninguna de ellas, dejando una estructura de
módulos y un pipeline de arranque probados y listos para extenderse.

## Alcance

- Añadir dependencias base en `Cargo.toml`: `axum`, `tokio` (features `rt-multi-thread`, `macros`, `signal`), `tower`/`tower-http` (tracing, cors), `tracing` + `tracing-subscriber` (env-filter), `serde`/`serde_json`, y gestión de config por entorno (`std::env` o `config` crate).
- Definir una `Config` (bind address, nivel de log, y placeholders para `database_url` y `pocket_id_issuer` que se consumirán en F03/F04) leída de variables de entorno.
- Arrancar un servidor Axum con graceful shutdown (SIGINT/SIGTERM) y logging estructurado.
- Endpoint `GET /healthz` que responde 200 con JSON `{"status":"ok"}`.
- Mantener `rust-embed` como dependencia preparada (feature `embed` del frontend), pero sin servir aún `web/dist`.
- Deja `src/main.rs` estructurándose hacia módulos (`config.rs`, `app.rs`/`router.rs`), pero sin crear aún `db/`, `dav/`, `api/`.

## Contratos / Interfaces

### Config

```rust
struct Config {
    bind_addr: SocketAddr,
    log_level: String,
    database_url: String,
    pocket_id_issuer: String,
}

impl Config {
    fn from_env() -> Result<Config, ConfigError>;
}
```

- `Config::from_env()` lee de variables de entorno, aplica valores por defecto documentados y devuelve `ConfigError` cuando un valor obligatorio falta o es inválido.

### Router

```rust
fn build_router(state) -> axum::Router;
```

- Monta `GET /healthz`. Preparado para recibir estado (por ahora mínimo/`()` o un placeholder inyectable).

### Variables de entorno

| Variable | Valor por defecto | Descripción |
|---|---|---|
| `OXIDAV_BIND_ADDR` | `0.0.0.0:8080` | Dirección/socket de escucha (`SocketAddr`). |
| `OXIDAV_DATABASE_URL` | `sqlite://data/oxidav.db` | Placeholder consumido en F03. |
| `OXIDAV_POCKET_ID_ISSUER` | *(sin defecto)* | Issuer OIDC, obligatorio; consumido en F04. Su ausencia produce `ConfigError`. |
| `RUST_LOG` | `info` | Nivel de log para `tracing-subscriber` (env-filter). |

### Formato de error de config

`ConfigError` debe ser un `enum` que distinga, como mínimo, entre variable ausente
(`MissingVar { name }`) y valor inválido (`InvalidValue { name, reason }`), e
implemente `std::error::Error` + `Display` con mensajes que nombren la variable.

### Endpoint de salud

- `GET /healthz` → `200 OK`, `Content-Type: application/json`, body `{"status":"ok"}`.

## Tareas (TDD)

1. - [ ] **Añadir dependencias y estructura de módulos.**
   - Añadir a `Cargo.toml`: `axum`, `tokio` (`rt-multi-thread`, `macros`, `signal`), `tower`, `tower-http` (`tracing`, `cors`), `tracing`, `tracing-subscriber` (`env-filter`), `serde` (`derive`), `serde_json` y `rust-embed` (preparada).
   - Declarar en `src/main.rs` los módulos `config` y `app`/`router` (vacíos de lógica aún).

2. - [ ] **RED — test unitario de `Config::from_env` con valores por defecto.**
   - Verificar que sin `OXIDAV_BIND_ADDR`/`OXIDAV_DATABASE_URL` se obtienen `0.0.0.0:8080` y `sqlite://data/oxidav.db`, y que `log_level` cae a `info`.
   - Ejecutar `cargo test` y confirmar fallo (símbolos inexistentes).

3. - [ ] **RED — test unitario de `Config::from_env` con variables presentes.**
   - Fijar variables de entorno (p. ej. con serialización de tests y guardas) y comprobar el parseo correcto del `SocketAddr` y del resto de campos.

4. - [ ] **RED — test unitario de error de config.**
   - Sin `OXIDAV_POCKET_ID_ISSUER` → `ConfigError::MissingVar { name }`.
   - Con `OXIDAV_BIND_ADDR` inválido → `ConfigError::InvalidValue { name, reason }`.

5. - [ ] **GREEN — implementar `Config` y `ConfigError`.**
   - `Config::from_env()` con defaults y validaciones; `ConfigError` con `Display` + `Error`.
   - Ejecutar `cargo test` hasta verde.

6. - [ ] **RED — test de integración del router: `GET /healthz`.**
   - Usar `tower::ServiceExt::oneshot` sobre `build_router(...)`.
   - Verificar `200`, `Content-Type` JSON y body `{"status":"ok"}`.

7. - [ ] **RED — test de integración de ruta desconocida.**
   - `GET /desconocida` → `404 Not Found`.

8. - [ ] **GREEN — implementar `build_router` y el handler `/healthz`.**
   - Handler que serializa `{"status":"ok"}` con `serde_json`; `build_router` monta la ruta.
   - Ejecutar `cargo test` hasta verde.

9. - [ ] **GREEN — arranque del servidor con graceful shutdown y observabilidad.**
   - `main.rs`: inicializar `tracing-subscriber` con `env-filter` a partir de `RUST_LOG`, cargar `Config`, construir el router, servir con `axum::serve` y apagado ordenado ante SIGINT/SIGTERM (`tokio::signal`).
   - Documentar el cierre limpio y el log de arranque (bind address).

10. - [ ] **REFACTOR — limpieza y verificación estática.**
    - `cargo fmt` y `cargo fmt --check`.
    - `cargo clippy -- -D warnings` (cero warnings).
    - `cargo build` sin warnings.

11. - [ ] **CONSOLIDATE — verificación manual del binario.**
    - Arrancar el binario, comprobar log estructurado y `curl -i http://<bind>/healthz` → `200 {"status":"ok"}`.
    - Enviar SIGINT y SIGTERM y confirmar parada limpia.

## Criterios de aceptación

- `cargo build` compila sin warnings; `cargo clippy -- -D warnings` limpio.
- `cargo test` en verde (unitarios de config + integración de `/healthz`).
- El binario arranca, loguea y responde `GET /healthz` con 200.
- Parada limpia con SIGINT/SIGTERM.

## Riesgos

- **Elección de la librería de config:** `std::env` minimiza dependencias pero obliga a validación manual; el crate `config` aporta más flexibilidad (ficheros + entorno) a costa de complejidad y superficie de deps.
- **Feature flags de `tokio`:** una selección incompleta (`rt-multi-thread`, `macros`, `signal`) impide compilar el runtime o el manejo de señales; revisar caso por caso.
- **Versión de Axum:** cambios de API entre 0.7 y 0.8+ (p. ej. `axum::serve`, tipos de router) pueden invalidar ejemplos; fijar y usar 0.8+.
- **Compatibilidad con edition 2024:** requiere toolchain reciente (≥1.85); verificar que las versiones de crates seleccionadas compilan bajo edition 2024.

## Referencias

- `plans/PLAN-001-general.md` (F01).
- `AGENTS.md`.
