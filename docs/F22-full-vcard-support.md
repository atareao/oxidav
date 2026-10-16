# F22 — Soporte completo de vCard y vista por pestañas

**Estado:** completada. **Rama:** `feature/full-vcard-support`. **Base:** `development@54824ce`. **13 commits.**

## Qué resuelve

Hasta ahora un contacto era **cuatro campos** (`FN`, `EMAIL`, `TEL`, `ORG`) y la escritura **recomponía** el `.vcf` desde esos campos: cualquier otra propiedad que el cliente hubiera escrito (`PHOTO`, `BDAY`, `X-ABLabel`, grupos de Apple, `REV`, `TZ`, `GEO`, `IMPP`, `CATEGORIES`…) se **destruía en silencio** en el primer `PATCH`.

F22 cambia el modelo: el payload original es la única verdad, la lectura lo proyecta **completo** y la escritura solo toca las propiedades que el cliente ha editado.

## Arquitectura

- **`card_objects.raw_payload` es la única fuente de verdad.** Cero migraciones: el esquema no cambia.
- **Lectura:** `project_card(raw)` (`src/dav/vcf/mod.rs`) proyecta el payload completo en una vista tipada por grupos. Un único parseo por petición.
- **Escritura:** `merge_properties(raw, &[PropertyEdit])` (`src/dav/vcf/merge.rs`) sustituye **solo** las líneas lógicas de las propiedades editadas y deja el resto **byte a byte**. `create` = esqueleto + merge; `update` = merge sobre el payload almacenado. Un solo camino de escritura.
- **Columnas desnormalizadas re-derivadas:** tras cada escritura se vuelve a parsear el payload y se recalcula `fn`/`email`/`phone`/`organization`/`notes`. Invariante: *columnas == `parse_vcf(raw_payload)`*.
- **`VERSION` y `UID` no se tocan nunca** en un merge, y el primitivo lo impone.

## Contrato de la API

- `GET /api/v1/collections/{id}/contacts/{uid}` → DTO de detalle con los ocho grupos.
- `PATCH` **por presencia**: un grupo **presente** sustituye **todas** sus apariciones; **ausente** no se toca; **vacío** (`[]`, `{}`, `""`) **borra**.
- `POST /api/v1/collections/{id}/contacts/import` con `Content-Type: text/vcard` y el fichero en crudo: guarda cada tarjeta **verbatim** en **una sola transacción** (un `uid` duplicado no deja escritura parcial). Límite de cuerpo 16 MiB **solo en esa ruta**.
- Los escalares heredados (`email`, `phone`, `organization`) equivalen a un array de un elemento; mandarlos **a la vez** que su array es `400`.

## Decisiones vinculantes

1. Los `TYPE` se validan contra un alfabeto acotado (`[A-Za-z0-9-]`, ≤32 caracteres, ≤8 por propiedad) y `PREF` en `1..=100`; se acepta la unión de los vocabularios 3.0 y 4.0 (desviación consciente de RFC, documentada).
2. Texto escapado (RFC 6350 §3.4); URIs verbatim; valores estructurados con `;` como separador de componentes.
3. El `uid` es inmutable en `PATCH`.
4. Ninguna dependencia nueva. Ningún cambio en el protocolo DAV: el `PUT` ya almacenaba verbatim.

## Hallazgos que cambiaron el contrato durante la implementación

Cinco puntos donde la medición contradijo el plan inicial:

- **`VERSION`, `REV`, `BDAY` y `ANNIVERSARY` se leen de las líneas crudas.** El modelo tipado normaliza y **miente**: `BDAY:--0203` (cumpleaños sin año) volvía como **`00000203`**, inventando el año 0.
- **Los `TYPE` se proyectan sin el prefijo `X-`** que añade el `Display` de `vcard4` (`TYPE=internet` salía como `X-internet`).
- **Las sub-listas del `N`** (`additional`/`prefix`/`suffix`) se separan con **coma sin escapar** (RFC 6350 §6.2.2): unirlas y luego escaparlas producía un valor no conforme.
- **`StructuredName.additional/prefix/suffix` son listas**, no cadenas (el plan inicial decía lo contrario y estaba equivocado).
- **El DTO de detalle no es superconjunto del ligero**: omite los escalares `email`/`phone`/`organization` para no duplicar el dato (`email` vs `emails[0].value`). Es una desviación consciente de la decisión 11 del plan.

## Limitaciones conocidas (medidas, no supuestas)

- **`LOGO`/`SOUND` embebidos en base64 se pierden en la proyección**: `vcard4` los modela como `UriProperty` y `parse_loose` descarta la propiedad al no poder parsear el base64 como URI. En `PHOTO` sí funciona (`TextOrUriProperty`). El payload **no** se pierde (sigue en `raw_payload` y el merge lo preserva): lo que no se puede es **mostrarlo**.
- **Las `URL` se normalizan** por el parser de URI (`https://example.com` → `https://example.com/`).
- **`others` cubre las 36 propiedades modeladas por el RFC más las `X-*`**; una propiedad desconocida que no empiece por `X-` la descarta `parse_loose`.
- **Al sustituir una propiedad con prefijo de grupo** (`item1.TEL`) se descarta el grupo: un `item1.X-ABLabel` asociado queda huérfano, **pero se conserva** (nunca se borra lo que no se pidió borrar).
- **Valores multilínea rechazados** por la validación de la API (decisión 8: rechazar, no sanear). El payload del cliente se preserva intacto.
- El **export** del SPA sigue siendo **lossy** (solo los campos básicos): el fichero completo vive en el servidor.
- La guarda del import en cliente es de 5 MB, más estricta que los 16 MiB del servidor (a propósito).
- Un cuerpo de import >16 MiB responde `413` en texto plano, no con el sobre JSON uniforme.
- El `Content-Type` del import no se valida (el cuerpo se parsea igual).

## Verificación

- **Backend: 551 tests, 0 fallos** (`cargo test --locked --no-fail-fast`), 15 binarios. `cargo fmt --all --check` y `cargo clippy --all-targets -- -D warnings` limpios.
- **Frontend: 19 ficheros / 174 tests, 0 fallos**. `npm run typecheck` y `npm run lint` limpios.
- **Bundle: 1 348 887 B** (línea base de F21: 1 270 610 B; **+78 277 B**, +6,2 %).
- **Verificación por mutación** (los tests no son vacuos): romper el separador de `render_list_property` hace fallar 3 tests; cambiar el media type de respaldo rompe 2; romper `deepEqual` del diff del formulario rompe 4.
- **End-to-end por HTTP real** (servidor arrancado con `AUTH_ENABLED=false`): `GET /healthz` 200; `POST …/contacts/import` con un `.vcf` estilo Apple → **201**; el detalle devuelve `birthday: "--0203"`, la foto como `data:image/jpeg;base64,…`, `item1.X-ABLabel` con su grupo y el `N` con sus sub-listas; tras `PATCH {"fn":"Ada Byron"}` → **200**, el nombre cambia y **photo, N, tels, emails, others, notes, categories, urls y uid quedan idénticos**.
- **Auditorías externas:** `@rust-reviewer` (0 bloqueantes, 2 importantes y 8 menores) y `@react-reviewer` (0 bloqueantes, 3 importantes y 9 menores); **todos los importantes arreglados** (sub-listas del `N`, atomicidad del import, validador de `PREF`, borrado de `N`/`GENDER`, `uid` parseado, guardas, plegado, claves estables, a11y, cobertura).

## Pendiente (recomendado, fuera de F22)

- **Partición de ficheros**: `src/dav/vcf/mod.rs` (1 714 líneas) y `src/api/contacts.rs` (1 321) superan al mayor anterior del repo (`ics.rs`, 1 108). Ambos revisores lo recomiendan; se deja como refactor propio, con su verificación.
- **`project_card` parsea dos veces** (validación + proyección): optimización menor.

## Commits

```
2472d40 ♻️ refactor: mover src/dav/vcf.rs a src/dav/vcf/ (F22)
7184fe9 📝 docs: añadir PLAN-023 (vCard completo y vista por pestañas, F22)
68c01af ✅ test + ♻️ merge quirúrgico de propiedades vCard (F22)
ec1c57c ✅ test + ♻️ proyección completa de la vCard (F22)
2971f10 ✅ test: contrato de la API de detalle, merge e importación (F22)
c0c3760 ✨ feat: detalle completo de la vCard y escritura por merge en la API (F22)
f99b593 🐛 fix: media type honesto en las fotos embebidas y cobertura de render_list_property (F22)
040d2db ✨ feat(web): tipos, servicio y hook del detalle del contacto (F22)
2ce31e2 ✨ feat(web): pestañas del detalle del contacto (F22)
579b021 ✨ feat(web): detalle real en el Drawer e importación en servidor (F22)
d4c374d ✨ feat(web): formulario de contacto completo con PATCH por diferencia (F22)
0481b8c 🐛 fix: arreglos de la auditoría (N, import atómica, uid parseado, guardas y plegado) (F22)
d701342 🐛 fix: borrar N, GENDER, BDAY y ANNIVERSARY desde el PATCH (F22)
2671acb 🐛 fix(web): arreglos de la auditoría React (F22)
```
