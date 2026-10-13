# PLAN-009 — F08 CardDAV core

## Objetivo

Implementar el núcleo CardDAV en `/dav/addressbooks/{username}/...` para que los
clientes nativos (iOS, DAVx⁵, Thunderbird) puedan descubrir addressbooks,
sincronizar contactos `.vcf` y consultarlos por `REPORT`. La feature reutiliza la
infraestructura DAV ya construida en F07 (helpers XML, `DavError`, autenticación
Basic, ETags y condicionales RFC 9110). No incluye sincronización incremental
completa ni la SPA de gestión.

## Alcance

- **Dentro:**
  - `src/dav/carddav.rs` — handlers CardDAV y props específicas.
  - `src/dav/vcf.rs` — parseo y serialización de tarjetas `.vcf`.
  - Método `OPTIONS` con anuncio de `addressbook` en `DAV:`/`Allow`.
  - `MKCOL` extendido (creación de addressbook).
  - `PROPFIND` sobre home, colección (`AddressBook`) y recurso (`contact`).
  - `GET` / `PUT` / `DELETE` de recursos `/{uid}.vcf`.
  - `REPORT` con `addressbook-query` y `addressbook-multiget`.
  - Montaje bajo `/dav/addressbooks/{username}/...`.
- **Fuera:**
  - `sync-collection` incremental completo (F09).
  - La SPA de gestión (features de frontend).
  - Filtros CardDAV avanzados (prop-filter con `match-type`, collation completa,
    `limit`/`nresults`, ordered-by, parametrización de tipos de teléfono,
    búsqueda en propiedades arbitrarias).

## Decisiones técnicas

- **Reutilización de infraestructura DAV.** F08 extiende `src/dav/` con
  `carddav.rs` y `vcf.rs`, análogos a `caldav.rs`/`ics.rs` de F07. Se reutilizan
  `xml.rs` (construcción de `multistatus` y parseo de `PROPFIND`) y
  `DavError`. No se duplican helpers XML, condicionales ni resolución de
  colecciones: los puntos comunes con CalDAV se factorizan a helpers compartidos
  en `mod.rs`/`xml.rs` en lugar de copiarse.
- **Creación de addressbook vía `MKCOL` extendido.** CardDAV (RFC 6352 §5.1) no
  define `MKCALENDAR`; define un `MKCOL` extendido cuyo cuerpo es un
  `<d:mkcol>` con `resourcetype` conteniendo `collection` + `addressbook`. Se
  **acepta también `MKCOL` sin cuerpo** como caso simplificado que crea un
  addressbook por defecto en la ruta indicada (comportamiento permisivo para
  clientes que no envían el cuerpo extendido, p. ej. algunos flujos de iOS).
  Ambos caminos producen el mismo recurso (colección de tipo `addressbook`) y
  responden `201 Created`; si la colección ya existe, `405 Method Not Allowed`.
- **Parseo `.vcf` con crate dedicado.** Se usa un crate de parseo vCard (p. ej.
  `vcard`; evaluar alternativa `vcard4`/similar según licencia y mantenimiento)
  para extraer `UID`, `FN`, `EMAIL`, `TEL`, `ORG` y `NOTE`. El payload `.vcf`
  crudo se guarda íntegro en `calendar`/`card_objects.raw_payload`, y los campos
  desnormalizados (`uid`, `fn`, `email`, `phone`, `organization`, `notes`) se
  rellenan para alimentar la búsqueda FTS de F03. **Limitaciones documentadas:**
  ante múltiples `EMAIL` o `TEL`, se **selecciona el primero** (preferencia por
  el marcado `TYPE=pref` si existe) y se almacena el valor primario en la columna
  desnormalizada; el resto permanece solo en `raw_payload`. Ante ausencia de
  `FN`, se hace fallback determinista (p. ej. `N` formateado, o el `UID`) y se
  marca la tarjeta como válida pero con `fn_` sintético.
- **Props CardDAV expuestas.** `resourcetype` (`collection` + `addressbook`),
  `displayname`, `getctag` (basado en `sync_token`/mtime de la colección),
  `sync-token`, `getetag` por recurso, `getcontenttype: text/vcard; charset=utf-8`,
  y `addressbook-home-set` sobre el recurso principal del usuario.
- **ETags y peticiones condicionales.** Mismos criterios que F07: comparación
  fuerte para `If-Match` (fallo → `412 Precondition Failed`), comparación débil
  para `If-None-Match` sobre escrituras (fallo → `412`), ETag calculado a partir
  del contenido o de `updated_at`.
- **Alcance de `REPORT`.** Se implementan:
  - `addressbook-query`: filtro por coincidencia de texto simple (substring sobre
    `fn`/`email`, reutilizando FTS5 `trigram` de F03), con `prop` request básico.
  - `addressbook-multiget`: recuperación por lista de `href`.
  Fuera del alcance: prop-filters complejos, `match-type` != `contains`, rangos
  de fechas, colaciones completas, `limit`/`nresults`, `ordered-by`.

## Modelo de rutas y autorización

Autenticación Basic obligatoria en todo `/dav` (`require_basic_auth`, F07).
`{username}` debe coincidir con el `sub`/usuario autenticado; en caso contrario
`403 Forbidden`. La dirección `{addressbook}` debe pertenecer al usuario
autenticado (colección de tipo `addressbook`), si no `404`/`403`.

| Ruta | Método | Handler | Autorización |
|---|---|---|---|
| `/dav/` | `OPTIONS` | `options` (compartido) | Basic Auth |
| `/dav/` | `PROPFIND` | `propfind_root` (compartido) | Basic Auth (`{username}` == sub) |
| `/dav/addressbooks/{username}/` | `PROPFIND` | `propfind_addressbook_home` | `{username}` == sub, si no `403` |
| `/dav/addressbooks/{username}/` | `MKCOL` | `mkcol_addressbook` | `{username}` == sub, si no `403` |
| `/dav/addressbooks/{username}/{addressbook}/` | `PROPFIND` | `propfind_collection` | addressbook del usuario, si no `404`/`403` |
| `/dav/addressbooks/{username}/{addressbook}/` | `REPORT` | `report` (`addressbook-query`, `addressbook-multiget`) | addressbook del usuario |
| `/dav/addressbooks/{username}/{addressbook}/{uid}.vcf` | `GET` | `get_card` | addressbook del usuario |
| `/dav/addressbooks/{username}/{addressbook}/{uid}.vcf` | `PUT` | `put_card` | addressbook del usuario |
| `/dav/addressbooks/{username}/{addressbook}/{uid}.vcf` | `DELETE` | `delete_card` | addressbook del usuario |
| `/dav/addressbooks/{username}/{addressbook}/{uid}.vcf` | `PROPFIND` | `propfind_resource` | addressbook del usuario |

Reglas transversales:
- Sin cabecera `Authorization` válida → `401 Unauthorized` + `WWW-Authenticate`.
- `{username}` distinto del autenticado → `403 Forbidden`.
- Colección inexistente o ajena → `404 Not Found` (sin filtrar existencia).

## Contratos / Interfaces

- **Recurso DAV de contacto:**

  ```rust
  pub struct AddressbookResource {
      pub href: String,
      pub etag: String,
      pub vcf: String,
  }
  ```

- **Parseo de tarjeta:**

  ```rust
  pub struct ParsedCard {
      pub uid: String,
      pub fn_: String,
      pub email: Option<String>,
      pub phone: Option<String>,
      pub organization: Option<String>,
      pub notes: Option<String>,
  }

  pub fn parse_vcf(raw: &str) -> Result<ParsedCard, VcfError>;
  ```

  Errores: `VcfError` cubre payload vacío, ausencia de `BEGIN:VCARD`/`END:VCARD`,
  ausencia de `UID` (a decidir: se puede generar o rechazar como `422`) y fallos
  de sintaxis del crate subyacente.

- **Handlers (firmas Axum 0.8):**

  ```rust
  async fn mkcol_addressbook(/* Path<..>, State<..>, body */) -> Result<StatusCode, DavError>;
  async fn propfind_addressbook_home(/* ... */) -> Result<Response, DavError>;
  async fn propfind_collection(/* ... */) -> Result<Response, DavError>;
  async fn propfind_resource(/* ... */) -> Result<Response, DavError>;
  async fn get_card(/* ... */) -> Result<Response, DavError>;
  async fn put_card(/* ... */) -> Result<StatusCode, DavError>;
  async fn delete_card(/* ... */) -> Result<StatusCode, DavError>;
  async fn report(/* ... */) -> Result<Response, DavError>;
  ```

- **Ledger de props CardDAV:**

  | Prop | Ámbito | Valor |
  |---|---|---|
  | `resourcetype` | home / colección | `collection` + `addressbook` |
  | `displayname` | colección | nombre del addressbook |
  | `getctag` | colección | ctag derivado de `sync_token`/mtime |
  | `sync-token` | colección | token incremental (base para F09) |
  | `getetag` | recurso | ETag del `.vcf` |
  | `getcontenttype` | recurso | `text/vcard; charset=utf-8` |
  | `addressbook-home-set` | principal | `/dav/addressbooks/{username}/` |

## Tareas (TDD)

- [x] **Test 1 — `OPTIONS` anuncia `addressbook`.** Aserción sobre cabeceras `DAV:`/`Allow` incluyendo `addressbook`. Primero en rojo, luego handler.
- [x] **Test 2 — `MKCOL` crea addressbook.** `MKCOL` con cuerpo `<d:mkcol>` (y variante sin cuerpo) → `201`; la colección aparece en el `PROPFIND` del home con `resourcetype` `addressbook`. Repetición → `405`.
- [x] **Test 3 — `PROPFIND` home/colección/recurso.** Props CardDAV correctas (incl. `addressbook-home-set` en el principal y `getetag` en recurso); negociación `allprop`/`propname`/`prop` coherente con F07.
- [x] **Test 4 — `PUT` crea/actualiza tarjeta.** `PUT` `.vcf` nuevo → `201` con `ETag`; verifica desnormalización de `fn`/`email`/`phone`. Actualización → `204`.
- [x] **Test 5 — Condicionales ETag.** `If-Match` con ETag obsoleto → `412`; `If-None-Match: *` sobre recurso existente al crear → `412`; mismos criterios fuerte/débil que F07.
- [x] **Test 6 — `GET` recurso.** Devuelve el `.vcf` crudo con `Content-Type: text/vcard` y `ETag` correcto.
- [x] **Test 7 — `DELETE`.** Existente → `204`; inexistente → `404`.
- [x] **Test 8 — `REPORT`.** `addressbook-query` con filtro de texto devuelve el subconjunto esperado en `multistatus`; `addressbook-multiget` resuelve por `href` (incluyendo `404` para href ausente).
- [x] **Test 9 — Autenticación.** Sin `Authorization` → `401`; `{username}` ajeno → `403`; addressbook de otro usuario → `404`/`403`.
- [x] **Test 10 — Parseo `vcf` unitario.** Tarjeta simple completa; múltiples `EMAIL`/`TEL` (se elige primario/primero); tarjeta sin `FN` (fallback determinista).
- [x] **Test 11 — Calidad.** `cargo fmt --check`, `cargo clippy -- -D warnings` y `cargo test` en verde; sin warnings.

## Criterios de aceptación

- `OPTIONS /dav/` anuncia `addressbook` y los métodos soportados.
- `MKCOL` (extendido y simplificado) crea un addressbook visible en el `PROPFIND` del home con `resourcetype` `addressbook`.
- `PROPFIND` devuelve props CardDAV correctas en home, colección y recurso, con `getetag` por recurso y `addressbook-home-set` en el principal.
- `PUT` crea (`201`) y actualiza (`204`) `.vcf`, guardando `raw_payload` y desnormalizando `fn`/`email`/`phone`/`organization`/`notes`.
- Condicionales `If-Match`/`If-None-Match` responden `412` según los criterios de F07.
- `GET` devuelve el `.vcf` crudo con `Content-Type: text/vcard; charset=utf-8` y `ETag`.
- `DELETE` responde `204`/`404` según exista el recurso.
- `REPORT` soporta `addressbook-query` (filtro de texto) y `addressbook-multiget` (por href).
- Autorización: sin Basic → `401`; `{username}` ajeno → `403`; addressbook ajeno → `404`/`403`.
- Suite de tests en verde, `cargo fmt --check` limpio y `cargo clippy -- -D warnings` sin warnings.
- No se duplica lógica DAV de F07: helpers de `xml.rs`/`DavError` reutilizados.

## Riesgos

- **Crate `.vcf`:** API y madurez del crate elegido; casos límite (vCard 3.0 vs 4.0, líneas plegadas, encodings/quoted-printable) pueden requerir parsing defensivo y tests unitarios adicionales.
- **Múltiples valores:** la decisión de tomar solo el primario (EMAIL/TEL) pierde información en las columnas desnormalizadas; aceptable porque `raw_payload` es la fuente de verdad, pero debe documentarse y cubrirse con tests.
- **Compatibilidad real con clientes:** iOS, DAVx⁵ y Thunderbird difieren en descubrimiento y en `MKCOL`; el `MKCOL` simplificado mitiga casos, pero conviene validar con clientes reales.
- **Reutilización sin duplicar DAV:** riesgo de copiar/pegar lógica de CalDAV; exige factorizar helpers compartidos antes de extender.
- **`MKCOL` extendido vs simplificado:** soportar ambos aumenta superficie de prueba y puede enmascarar clientes mal formados; decidir y documentar el comportamiento por defecto.

## Referencias

- `plans/PLAN-001-general.md` (feature F08).
- `docs/F07-caldav-core.md`.
- `docs/F03-data-layer.md`.
- `AGENTS.md`.
