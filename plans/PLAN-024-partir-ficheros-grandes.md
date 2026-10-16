# PLAN-024 — Partir los ficheros grandes de F22

**Estado:** propuesto (pendiente de aprobación). **Rama prevista:** `refactor/split-large-files`, desde `development@c8e335a`.

## Objetivo

F22 dejó dos ficheros por encima de cualquier otro del repo. Las auditorías de cierre lo señalaron como la única deuda seria pendiente:

| Fichero | Líneas | Antes de F22 |
|---|---:|---|
| `src/dav/vcf/mod.rs` | 1 737 | 689 (`vcf.rs`) |
| `src/api/contacts.rs` | 1 487 | 630 |
| `src/dav/vcf/merge.rs` | 1 196 | — (nuevo en F22) |
| *(mayor del repo antes de F22)* | *1 108* | `src/dav/ics.rs` |

El objetivo es **separar responsabilidades**, no reescribir lógica. **Todo el refactor es de movimiento de código**: ningún cambio de comportamiento, ninguna firma pública que cambie, ningún test que se pierda.

## Invariantes que hay que respetar (criterios de aceptación)

1. **Rutas públicas intactas**: `oxidav::dav::vcf::{parse_vcf, project_card, build_vcf, ParsedCard, CardFields, VcfError, CardProjection, is_forbidden_char}` y `oxidav::dav::vcf::merge::*` siguen resolviendo **exactamente** igual. Nada fuera de los ficheros movidos debe cambiar ni una línea.
2. **Cero tests perdidos**: el recuento de la suite es **551 pasando / 0 fallando** antes y después de **cada** paso, y el binario `--lib` mantiene su número exacto de tests (298).
3. **Backend y frontend verdes**: `cargo test --locked --no-fail-fast`, `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`; y `npm run test` (19 ficheros / 174 tests) al final, ya que el frontend no se toca pero se verifica.
4. **Ningún `#[allow(dead_code)]`**, ningún test `#[ignore]`, ninguna función que quede sin usar.
5. **`compose.yaml` fuera de todos los commits** (está modificado en el árbol por su dueño).

## Paso 1 — `♻️ refactor: extraer la proyección a vcf/projection.rs`

De `src/dav/vcf/mod.rs`, mover a **`src/dav/vcf/projection.rs`** todo lo que es la vista tipada de lectura:

- `CardProjection` (línea 449) y sus auxiliares: `StructuredName` (511), `Gender` (526), `MultiValue` (535), `MediaValue` (546), `Address` (560), `RawProperty` (585).
- `project_card` (627), `raw_property_value` (695), `structured_name` (703), `text_list` (721), `gender` (731), `text_or_uri_multi` (743), `text_multi` (752), `uri_multi` (761), `type_tokens` (775), `type_token` (785), `address` (793), `media_value` (819), `media_type_of` (840), `others` (862), `raw` (933), `raw_parameters` (944), `any_property_value` (1001).
- Y **solo** los tests del `#[cfg(test)] mod tests` (1009–1737) que prueban esos elementos (proyección, `projects_*`, `projection_agrees_with_the_denormalized_parse`, `projects_verbatim_scalar_text`, `projects_type_tokens_without_the_extension_prefix`).

En `mod.rs` queda: documentos de módulo, `pub mod merge; pub mod projection;` y la **reexportación** `pub use projection::{...}` para que las rutas públicas no cambien (incluidos los tipos que usa `src/api/dto.rs`).

**Verificación del paso:** `--lib` con los mismos 298 tests; `551/0`; `fmt`/`clippy` limpios; `grep -rn 'dav::vcf::' src/ tests/` sin cambios respecto a antes.

## Paso 2 — `♻️ refactor: extraer la composición a vcf/build.rs`

De `src/dav/vcf/mod.rs`, mover a **`src/dav/vcf/build.rs`**: `CardFields` (328), `build_vcf` (359), `safe_uid` (394), `escape_text` (424) y sus tests. `mod.rs` vuelve a reexportar (`pub use build::{CardFields, build_vcf};`) para no tocar a los llamantes.

Tras el paso 2, `mod.rs` debería quedar en el entorno de **500–600 líneas**: documentos, `VcfError`, `ParsedCard`, `parse_vcf`, el pre-paso endurecido (`has_envelope`, `ensure_formatted_name`, `logical_lines`, `property_name`, `has_property`, `derive_formatted_name`, `format_structured_name`, `inject_property`, `text_or_uri_value`, `pref_of`, `preferred_entry`), `is_forbidden_char` y sus tests.

## Paso 3 — `♻️ refactor: un solo parseo en project_card`

Hoy `project_card` llama a `parse_vcf` para **validar** y luego parsea otra vez para proyectar. Es una optimización menor señalada por la auditoría, **neutra en comportamiento**: se parsea una sola vez y el error devuelto es el mismo `VcfError` para los mismos payloads. Tests: los tres payloads de rechazo siguen dando exactamente el mismo error, y los casos de proyección no cambian.

## Paso 4 — `♻️ refactor: partir api/contacts.rs en mod/validate/edits`

De `src/api/contacts.rs` (1 487 líneas) a tres ficheros:

- **`src/api/contacts/mod.rs`**: documentos de módulo, `mod edits; mod validate;`, `ContactListQuery` (156), los handlers (`list` 186, `create` 213, `get_one` 301, `update` 337, `import` 427, `delete_one` 549), `split_cards` (514), `addressbook_collection` (574), los mapeadores de error (`merge_failure` 1251, `parse_created` 1260, `map_insert_error` 1275) y las constantes del borde HTTP (`MAX_IMPORT_BODY_BYTES` y compañía, en la parte que le toca).
- **`src/api/contacts/validate.rs`**: `validate_required_text` (596), `normalize_optional` (621), `validate_uid` (650), `validate_stored_uid` (687), `validate_value` (1223), `enforce_count` (1213), `invalid_parameters` (1238), `scalar_array_conflict` (1243), `is_blank` (1208), `blank_component` (1196), `blank_component_list` (1201), `name_is_empty` (1178), `gender_is_empty` (1191), `opt_str` (1168) y **los topes de campo** que consumen (`MAX_FN_LEN`, `MAX_EMAIL_LEN`, `MAX_PHONE_LEN`, `MAX_ORGANIZATION_LEN`, `MAX_UID_LEN`, `MAX_VALUES`, `MAX_TEXT_LEN`, `MAX_NOTE_LEN`, `MAX_URL_LEN`).
- **`src/api/contacts/edits.rs`**: `WritableFields` (706), `push_edit` (730), `build_edits` (743), `scalar_text_lines` (911), `text_line` (919), `uri_line` (929), `structured_line` (939), `render_text_lines` (952), `render_uri_lines` (961), `render_structured_lines` (970), `render_list_lines` (993), `render_labelled` (1016), `render_name` (1046), `join_name_list` (1070), `render_gender` (1087), `render_addresses` (1126): **cómo un campo del DTO se convierte en propiedades de vCard**.

El `#[cfg(test)] mod tests` (1284–1487) se **distribuye** al fichero del elemento que prueba; si un test cruza concerns, se queda en `mod.rs`.

La visibilidad que cruce módulos pasa a `pub(super)`/`pub(crate)` — **nunca** `pub` nuevo.

## Riesgos y mitigaciones

| Riesgo | Mitigación |
|---|---|
| Perder tests en el reparto del `mod tests` | Contar por fichero **antes y después** de cada paso; el total de `--lib` (298) y el global (551) no pueden moverse |
| Cambiar una ruta pública sin querer | `grep -rn` de `dav::vcf::` y `api::contacts::` antes/después; y los consumidores (`src/api/mod.rs`, `src/api/dto.rs`, `src/dav/carddav.rs`) sin tocar |
| Mover código y tocar lógica "de paso" | Cada paso es **movimiento + visibilidad + reexport**; el diff debe ser casi todo líneas movidas (`git diff -M --stat`) |
| Romper el orden del árbol de dependencias | Ninguna dependencia nueva; solo `mod`/`use` |

## Verificación de cierre

1. Backend **551/0** y frontend **19 ficheros / 174 tests**.
2. `fmt`, `clippy -D warnings`, `typecheck`, `lint` limpios.
3. **Recuento por fichero** de cada uno de los nuevos módulos y del total (debe bajar el máximo del repo).
4. `git diff -M --stat` de cada commit para evidenciar que es movimiento.
5. Prueba de humo end-to-end por HTTP (servidor real): importar un `.vcf` estilo Apple, leer el detalle y `PATCH {"fn"}` comprobando que todo lo demás queda idéntico — **el mismo guion usado para cerrar F22**, que es la red de seguridad de que el refactor no cambió el comportamiento.

## Fuera de alcance

- No se toca `src/dav/vcf/merge.rs` (tiene una responsabilidad clara y un tamaño comparable a `ics.rs`).
- No se toca el frontend.
- No se cambia ningún contrato de la API ni del DAV.
- No se aprovecha para "limpiar" otras cosas: **un refactor, un cambio de forma**.

## Versionado

Los commits son `refactor` → **patch** en `git-cliff`. No hay `feat`, así que si el refactor se entrega solo, la próxima versión es `0.3.2`, no `0.4.0`.
