# PLAN-025 — Partir `dav/xml.rs` y `auth/session.rs`

**Estado:** propuesto (pendiente de aprobación). **Rama prevista:** `refactor/split-xml-session`, desde `development@9108c0b` (ya incluye la 0.4.0).

## Objetivo

PLAN-024 partió los dos ficheros que F22 había hecho crecer. La corrección de su baseline destapó que los mayores del repo, en realidad, eran otros dos que nadie había mirado:

| Fichero | Líneas | Tests | Qué mezcla hoy |
|---|---:|---:|---|
| `src/auth/session.rs` | 2 014 | 28 | configuración y validación de `AuthConfig`, emisión/verificación de JWT, `state`/`nonce`, el `StateStore`, cliente HTTP, cachés de discovery/JWKS, `discover`, `fetch_jwks`, `exchange_code`, validación del `id_token` y montaje del `end_session_url` |
| `src/dav/xml.rs` | 1 499 | 26 | constantes de espacio de nombres y `DavNs`, los tipos del modelo, la **serialización** del `207 Multi-Status` y el **parseo** de `PROPFIND`/REPORT |

El objetivo es **separar responsabilidades**, no reescribir lógica. **Todo el refactor es movimiento de código**: ningún cambio de comportamiento, ninguna firma pública que cambie, ningún test que se pierda. Si acaso se descubre una redundancia real, se hace en un paso propio y aislado, con su justificación; **no se inventa ninguna**.

## Invariantes que hay que respetar (criterios de aceptación)

1. **Rutas públicas intactas**: `oxidav::auth::session::{...}` (incluidos `AuthConfig`, `Claims`, `FlowClaims`, `UserIdentity`, `StateStore`, `emit_session`, `validate_session`, `emit_flow_token`, `validate_flow_token`, `generate_state_and_nonce`, `clear_caches`, `discover`, `fetch_jwks`, `exchange_code`, `validate_id_token`, `build_end_session_url`, `SESSION_COOKIE_NAME`, `SESSION_TTL_SECS`, `ID_TOKEN_LEEWAY_SECS`, `AuthError`) y `oxidav::dav::xml::{...}` (constantes `NS_*`, `DavNs`, `dav_ns_from_uri`, `PropValue`, `DavProp`, `DavResponse`, `PropName`, `ParseSyncCollection`, `ParsePropfind`, `TimeRange`, `ParseCalendarQuery`, `ParseAddressbookQuery`, `ParseAddressbookMultiget`, `CardDavReport`, `XmlError`, `ParseMkcalendar`, `multistatus`, `multistatus_with_sync_token`, `valid_sync_token_error`, `status_line`, `escape`, `parse_propfind`, `parse_sync_collection`, `report_root_name`, `parse_calendar_query`, `parse_carddav_report`, `parse_addressbook_query`, `parse_addressbook_multiget`, `parse_mkcalendar`) siguen resolviendo **exactamente** igual. Nada fuera de los ficheros movidos cambia ni una línea.
2. **Cero tests perdidos**: **551 pasando / 0 fallando** y **298** en `--lib`, antes y después de **cada** paso. Los 28 tests de `session.rs` y los 26 de `xml.rs` cambian de fichero, no de cantidad.
3. `cargo fmt --all --check` y `cargo clippy --all-targets --locked -- -D warnings` limpios. Frontend verde (19 ficheros / 174 tests), ya que no se toca.
4. **Ningún `pub` nuevo** (lo que cruce módulos, `pub(super)`/`pub(crate)`), ningún `#[allow(dead_code)]`, ningún `#[ignore]`.
5. **`compose.yaml` fuera de todos los commits** (está modificado en el árbol por su dueño).

## Advertencia previa: aquí se toca autenticación

`src/auth/session.rs` **no es un fichero cualquiera**: implementa el flujo OIDC. Dos cosas que este plan **no puede alterar** y que un agente debe leer antes de empezar:

- Los dos **cachés** (`OidcCache`, `oidc_cache()`, `clear_caches()`) son **globales al proceso**, con clave por URL y TTL de **300 s**. Un test que ejercite `discover`, `fetch_jwks` o `validate_id_token` contra un `wiremock` en puerto efímero puede recibir un puerto reciclado ya cacheado y pasar o fallar según el orden: por eso todo test de esos caminos llama antes a **`clear_caches()`**. El refactor **no toca esa lógica en absoluto**: solo cambia de dónde cuelga el código.
- El backend criptográfico de `jsonwebtoken` es la feature **`rust_crypto`** (binario musl estático sin OpenSSL). No se añade ninguna dependencia y no se cambia ninguna feature.

## Paso 1 — `♻️ refactor: extraer la serialización a dav/xml/write.rs`

`src/dav/xml.rs` pasa a ser un módulo de directorio. Se mueve a **`src/dav/xml/write.rs`** todo lo que **escribe** XML:

`multistatus` (272), `multistatus_with_sync_token` (283), `valid_sync_token_error` (294), `multistatus_inner` (301), `write_response` (331), `write_propstat` (356), `write_prop` (367), `status_line` (405), `reason_phrase` (411), `escape` (437), `escape_into` (444), y **solo** los tests que prueban la serialización y el escapado.

En `dav/xml/mod.rs` quedan: documentos, constantes `NS_*`, `DavNs`+impl, `dav_ns_from_uri`, `XmlError`, los tipos del modelo (`PropValue`, `DavProp`, `DavResponse`, `PropName`, `ParseSyncCollection`, `ParsePropfind`, `TimeRange`+impl, `ParseCalendarQuery`, `ParseAddressbookQuery`, `ParseAddressbookMultiget`, `CardDavReport`, `ParseMkcalendar`) y **las reexportaciones** para que las rutas públicas no cambien.

## Paso 2 — `♻️ refactor: extraer el parseo a dav/xml/parse.rs`

Se mueve a **`src/dav/xml/parse.rs`** todo lo que **lee** XML:

`parse_propfind` (469), `parse_sync_collection` (541), `report_root_name` (677), `parse_calendar_query` (687), `parse_time_range` (727), `parse_carddav_report` (770), `parse_addressbook_query` (786), `parse_addressbook_multiget` (798), `root_element_name` (810), `element_texts` (847), `parse_mkcalendar` (945), `local_name` (1029), `namespace_of` (1038), `is_dav_namespace` (1051), y **solo** sus tests.

Tras los pasos 1 y 2, `dav/xml/mod.rs` debería quedar en el entorno de **300–450 líneas** (documentos, constantes, `DavNs`, `XmlError`, modelos y reexportaciones), `write.rs` en **250–350** y `parse.rs` en **650–800**. El rechazo de `<!DOCTYPE>` (anti-XXE / billion-laughs) y el namespace handling con `NsReader` **se mueven tal cual**, sin tocar la política de seguridad.

## Paso 3 — `♻️ refactor: extraer la configuración a auth/session/config.rs`

`src/auth/session.rs` pasa a ser un módulo de directorio. Se mueve a **`src/auth/session/config.rs`**: `AuthConfig` (61), `impl AuthConfig` (72) —con `validate()` y su política **fail-closed**— y los validadores de URL que usa: `same_origin` (683), `is_secure_url` (698), `is_loopback_host` (712).

**Sin tocar un ápice de la política**: los requisitos de `AUTH_*`, la admisión de `http` solo en loopback y el que `validate()` **aborte el arranque** nombrando la variable no se reformulan, se mueven.

## Paso 4 — `♻️ refactor: extraer tokens y OIDC a auth/session/{tokens,oidc}.rs`

Se parten los dos bloques restantes:

- **`src/auth/session/tokens.rs`**: `FlowClaims` (158), `Claims` (171), `UserIdentity` (186), `emit_session` (247), `validate_session` (255), `emit_flow_token` (276), `validate_flow_token` (295), `generate_state_and_nonce` (310), `unix_now` (323), `StateStore`+impl (340/344), `IdTokenClaims` (725), `validate_id_token` (737) y `build_end_session_url` (800).
- **`src/auth/session/oidc.rs`**: `OidcDiscovery` (196), `TokenResponse` (206), `http_client` (428), `OidcCache` (469), `oidc_cache` (474), `clear_caches` (496), `read_limited_json` (502), `discover` (538), `fetch_jwks` (605) y `exchange_code` (638).

`AuthError` se queda en `mod.rs` (lo usan los tres) y `mod.rs` reexporta todo para que las rutas públicas no cambien. El reparto exacto entre `tokens` y `oidc` puede ajustarse si un elemento encaja mejor en la otra mitad —p. ej. `validate_id_token` si termina usando la caché—, **pero siempre moviéndolo, nunca duplicándolo**, y explicando el ajuste en el reporte.

Tras el paso 4, `auth/session/mod.rs` debería quedar en el entorno de **250–400 líneas** (documentos, constantes, `AuthError`, reexportaciones), `config.rs` en **200–300**, `tokens.rs` en **400–550** y `oidc.rs` en **400–550**.

## Riesgos y mitigaciones

| Riesgo | Mitigación |
|---|---|
| Tocar, «de paso», la política fail-closed o la lógica de los cachés OIDC | Paso 3 y 4 son movimiento + visibilidad + reexport; la política y los TTL se mueven verbatim. El test de `wiremock` que limpia cachés es la red |
| Perder tests en el reparto del `mod tests` | Contar por fichero **antes y después** de cada paso; los totales (28 y 26, y **298** en `--lib`) no pueden moverse |
| Cambiar una ruta pública sin querer | `grep -rn 'session::'` y `grep -rn 'xml::'` antes/después; los consumidores (`src/config.rs`, `src/router.rs`, `src/auth/{routes,middleware,basic,mod}.rs`, `src/main.rs`, `src/dav/{common,carddav,caldav,mod}.rs`, `tests/`) **sin tocar** |
| Mover código y tocar lógica «de paso» | Cada paso es movimiento; el `diff` debe ser casi todo líneas movidas (`git diff -M --stat`) |
| Romper el árbol de dependencias | Ninguna dependencia nueva; solo `mod`/`use` |

## Verificación de cierre

1. Backend **551/0** y **298** en `--lib`; frontend **19 ficheros / 174 tests**; `fmt`, `clippy`, `typecheck`, `lint` limpios.
2. **Movimiento puro comprobado, no supuesto**: por cada bloque movido, comparar la región extraída del commit base con su nuevo hogar (`sed -n '/^<marcador>/,/^#\[cfg(test)\]/p'` + `diff`) y explicar cada línea que aparezca; y por **multiconjunto de líneas** ordenadas cuando el reparto del `mod tests` haga inservible el diff lineal.
3. **Recuento por fichero** de cada módulo nuevo y del total del repo.
4. `git diff -M --stat` de cada commit.
5. **Prueba de humo end-to-end por HTTP** con un servidor real: `healthz` 200, SPA 200, 404 JSON en `/api/v1/nope`, **401 en `PROPFIND /dav/`**, y una lectura DAV real (`PROPFIND` con `Depth: 1` o un `GET` de colección) que ejercite el `multistatus` **serializado** y el **parseo** del cuerpo — el mismo guion que cerró F22 y PLAN-024, que es la red de seguridad de que el refactor no cambió el comportamiento.
6. Los tests de `wiremock` de `session.rs` (discovery/JWKS/`id_token`, con `clear_caches`) deben seguir verdes: son la cobertura específica del fichero más delicado.

## Fuera de alcance

- No se toca `src/dav/vcf/`, `src/api/contacts/` (recién partidos) ni el frontend.
- No se cambia ningún contrato de la API ni del DAV.
- No se añaden tests nuevos ni se reescriben los existentes: **es un refactor, no una mejora**.

## Versionado

Los commits son `refactor` → **patch** en `git-cliff`. Si se entrega solo, la próxima versión es `0.4.1`.

---

## Verificación y reporte

Reporta: la ruta final del fichero escrito, su `wc -l`, `grep -c "^## "`, y el nombre del fichero en `plans/`. **No ejecutes git** (no tienes shell): yo commiteo.
