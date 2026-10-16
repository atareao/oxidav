# PLAN-023 — vCard completo en CardDAV y vista por pestañas (F22)

## Objetivo

Que OxiDav deje de tener un contacto "a medias". Hoy el servidor modela un contacto con **seis** campos (`uid`, `fn`, `email`, `phone`, `organization`, `notes`); todo lo demás que trae una vCard real (varios `TEL`/`EMAIL`/`ADR` con `TYPE`/`PREF`, `NICKNAME`, `PHOTO`, `BDAY`, `ANNIVERSARY`, `GENDER`, `IMPP`, `GEO`, `TZ`, `TITLE`, `ROLE`, `LOGO`, `CATEGORIES`, `URL`, `SOUND`, `REV`, `X-*`…) existe en `raw_payload` pero **no se ve**, y —peor— **se destruye** en cuanto el usuario edita el contacto desde el SPA.

Esta feature persigue tres resultados concretos:

1. **Ver todo.** `GET /api/v1/collections/{id}/contacts/{uid}` devuelve la proyección completa de la vCard, agrupada en las categorías del RFC 6350, y el SPA la muestra en un `<Drawer>` con **pestañas** por categoría.
2. **No perder nada.** `PATCH` deja de recomponer el `.vcf` desde cero: pasa a ser un **merge quirúrgico por propiedad** sobre el `raw_payload`, de modo que una propiedad que el cliente no toca queda **byte a byte** como estaba.
3. **Editar lo editable.** El formulario de alta/edición cubre los campos de texto y estructurados (incluidos los multivalor con `TYPE`/`PREF`), **no** los binarios (`PHOTO`, `LOGO`, `SOUND`), que se muestran y se preservan.

## Alcance

Feature (F22) **fullstack**: backend (parseo, contrato REST, merge) + frontend (tipos, servicio, hook, componentes de pestañas y formulario).

**FUERA DE ALCANCE (no incluir en las tareas):**

- **Esquema y migraciones: cero.** No hay `0005_*.sql`. `raw_payload` sigue siendo la **única verdad**; la proyección completa se **deriva al leer**. Las columnas desnormalizadas (`fn`, `email`, `phone`, `organization`, `notes`) se siguen manteniendo, pero **re-derivadas** del payload resultante.
- **Búsqueda / FTS: sin cambios.** `contacts_fts` mantiene sus cinco columnas y sus tres triggers. Buscar por `NICKNAME`/`TITLE`/`CATEGORIES`/`ADR` queda como follow-up (decisión tomada con el usuario).
- **Binarios: no se editan.** `PHOTO`/`LOGO`/`SOUND` se exponen en la lectura (URI o `data:` URI para base64 embebido) y se preservan byte a byte al escribir, pero el formulario no los modifica. Sin límite de tamaño en lectura (documentado); sin escritura de binarios no hay nada que acotar en escritura.
- **`web/src/utils/vcf.ts`**: el parser cliente de la importación **no** crece. No se duplica el RFC 6350 en el navegador.
- **Protocolo DAV: sin cambios.** `PUT` de CardDAV ya almacena el payload del cliente **verbatim**; no hay nada que arreglar ahí. `PROPFIND`/`REPORT`/`sync-collection` intactos.
- **Cripto / dependencias:** ninguna dependencia nueva ni versión movida. `vcard4` 0.7.3 (ya presente) se usa más, no se cambia.
- **CI, `Dockerfile`, `compose.yaml`, `.vampus.yml`:** sin cambios.
- **`card_objects` y sus repositorios** (`insert_card_object`, `update_card_object`, `get_card_object`, `list_card_objects_window`): **sin cambios de firma**. Sus structs (`NewCardObject`, `CardObjectUpdate`) tampoco cambian: solo cambia **qué** payload se les entrega.

## Motivación

### El defecto que obliga a esta feature (medido, no supuesto)

`src/api/contacts.rs:272-290` construye un `CardFields` (6 campos) a partir de las **columnas** y llama a `build_vcf`. Es decir: **todo `PATCH` recompone el `.vcf` desde esos seis campos**. Consecuencia: si un contacto llegó por CardDAV desde un móvil con `PHOTO`, `BDAY` o `X-ABLabel`, **editar el nombre desde el SPA borra el resto de la tarjeta**. Lo mismo en `create` (`:170-189`), donde la tarjeta nace ya recortada.

El propio módulo lo sabe y lo documenta como limitación: `src/dav/vcf.rs:31-45` reconoce que `EMAIL`/`TEL` múltiples se colapsan a un solo valor y que `ORG`/`NOTE` se quedan en la primera aparición. Lo que no documenta es que esas limitaciones son **destructivas** al escribir.

### Diagnóstico medido

Backend:

- `src/dav/vcf.rs` (689 líneas): `parse_vcf` (usa `vcard4::parse_loose`), `ParsedCard { uid, fn_, email, phone, organization, notes }`, `CardFields` (los mismos 6) y `build_vcf` (emite `BEGIN:VCARD`, `VERSION:4.0`, `UID`, `FN` + los opcionales, con folding a 75 octetos). Ya tiene una pre-pasada endurecida (`logical_lines`, `property_name`, `has_property`, `inject_property`) que demuestra que la cirugía de líneas se sabe hacer con cuidado en este repo.
- `src/api/contacts.rs` (630 líneas): `list` → `Vec<ContactDto>`; `get_one` → `ContactDto`; `create`/`update` → composición por `CardFields`; `delete_one`. Validación propia del repo: `validate_required_text`, `normalize_optional`, `validate_uid`, `validate_stored_uid`, `is_forbidden_char` (rechazan control chars y U+2028/U+2029 → la frontera anti-inyección).
- `src/api/dto.rs` (608 líneas): `ContactDto` (10 campos) y `ContactSearchResult` (6). `CreateContactRequest` acepta `fn`/`email`/`phone`/`organization`/`uid`; `UpdateContactRequest`, cuatro campos y **no** puede limpiar a `null` (limitación MVP documentada).
- `migrations/0001_init.sql`: `card_objects(id, collection_id, uid, fn, email, phone, organization, notes, etag, raw_payload, created_at, updated_at)`; `contacts_fts(fn, email, phone, organization, notes)` `content='card_objects'` + triggers `card_objects_ai/ad/au` (el `ad`/`au` emiten `('delete', …)` con los valores **antiguos**).
- `src/db/mod.rs:180`: `compute_etag(raw_payload, updated_at)` = FNV-1a del payload **más** `updated_at`. Por tanto el ETag cambia en **cualquier** escritura, aunque el payload sea idéntico. La invariante de bytes hay que exigirla sobre `raw_payload`, no sobre el ETag.
- Rutas: `src/api/mod.rs:88-99` (`/collections/{id}/contacts`, `/collections/{id}/contacts/{uid}`, `/search/contacts`).
- **No hay `DefaultBodyLimit` en ningún sitio** → el límite por defecto de axum (2 MB) aplica a los extractores `Bytes`.

Crate `vcard4` 0.7.3 (leída en `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/vcard4-0.7.3`):

- `struct Vcard` modela **36 propiedades**: `source`, `kind`, `xml`, `formatted_name`, `name`, `nickname`, `photo`, `bday`, `anniversary`, `gender`, `url`, `address`, `tel`, `email`, `impp`, `lang`, `title`, `role`, `logo`, `org`, `member`, `related`, `timezone`, `geo`, `categories`, `note`, `prod_id`, `rev`, `sound`, `uid`, `client_pid_map`, `key`, `fburl`, `cal_adr_uri`, `cal_uri`, `extensions`.
- `impl fmt::Display for Vcard` (líneas 329-464): serializa **las 36**. (Nota de método: una primera comprobación con `-A90` sugería que se perdían once; era un artefacto de la ventana truncada y se descartó midiendo el rango real del `impl`. No inventar conclusiones a partir de ventanas truncadas.)
- Parámetros modelados: `language`, `value`, `pref`, `alt_id`, `pid`, `types`, `media_type`, `calscale`, `sort_as`, `geo`, `timezone`, `label`, `extensions`.
- `parse_loose` = `VcardParser::new(input, false)` → **ignora las propiedades que generan error**. Consecuencia: las propiedades desconocidas que **no** empiecen por `X-` se **descartan** al parsear; las `X-` sí se conservan (`ExtensionProperty`). Los grupos (`item1.TEL`) se conservan por propiedad.
- `Vcard::parse_photo_jpeg` existe y reconoce el formato `ENCODING=b;TYPE=JPEG` que escribe la app Contactos de macOS.

Frontend:

- `web/src/pages/Contacts.tsx` (679 líneas): tabla + `<Drawer>` con un `<Descriptions>` de 5 filas + `<Modal>`/`<Form>` de 4 campos; actualizaciones optimistas con `useOptimistic`; import/export `.vcf`.
- `web/src/types/api.ts` (171 líneas): `Contact` con los 4 campos editables; `CreateContactInput`/`UpdateContactInput` con los mismos.
- `web/src/services/contacts.ts` (108 líneas), `web/src/hooks/useContacts.ts` (113 líneas).
- `web/src/utils/vcf.ts` (241 líneas): parser/serializador **cliente** limitado a `FN`/`EMAIL`/`TEL`/`ORG`/`UID`, vCard 3.0, documentado como "convenience interchange format, not a full RFC 6350 parser".
- Suite frontend: 14 ficheros / **124 tests**. Bundle medido: **1 270 610 B**.

Backend (tests): `tests/api.rs` (2607 líneas), `tests/data_layer.rs` (1624), `tests/carddav.rs` (1004), entre otros.

### Hechos verificados (no suposiciones)

- `build_vcf` y `CardFields` se usan **solo** en `src/api/contacts.rs` (líneas 170 y 272) y en 8 referencias dentro de los tests de `src/dav/vcf.rs`. No hay más consumidores.
- `CardObjectUpdate` ya lleva `raw_payload`, así que **reemplazar el payload entero es lo que el repositorio hace hoy**: no hay que cambiar la capa de datos para pasar a un merge.
- El repositorio tolera módulos grandes (`src/dav/ics.rs` tiene 1108 líneas), así que dividir `vcf.rs` es una decisión de claridad, no de límite técnico.

## Decisiones vinculantes

1. **`raw_payload` es la única verdad.** La proyección completa se deriva **al leer** (parseando el payload); las columnas desnormalizadas se derivan **al escribir** re-parseando el payload resultante. Invariante exigible y testeable: **para toda fila almacenada, las columnas desnormalizadas son iguales al resultado de `parse_vcf(raw_payload)`**.
2. **La escritura es un merge quirúrgico, no una recomposición.** Se sustituyen **solo** las líneas de las propiedades que el cliente editó; todo lo demás queda **byte a byte** intacto (incluidas las propiedades que el servidor no modela, los grupos, el orden y los parámetros tal y como los escribió el cliente).
3. **Un solo camino de escritura.** `create` = esqueleto mínimo (`BEGIN`/`VERSION:4.0`/`UID`/`FN`/`END`) **+ merge** de los campos restantes; `update` = merge sobre el payload almacenado. No hay dos implementaciones de composición.
4. **`VERSION` y `UID` no se tocan nunca** en un merge. El `UID` es la identidad; el `VERSION` lo exige RFC 6350 §6.7.1 como primera propiedad. Un `PATCH` **no** puede cambiar el `UID` (viaja en la URL).
5. **Parámetros con alfabeto acotado.** `TYPE` se acepta como una lista de *tokens* (`[A-Za-z0-9-]`, máximo 32 caracteres, máximo 8 por propiedad) y `PREF` como entero `1..=100`. **No** se valida contra el registro de tokens de la RFC: se acepta la unión de los vocabularios 3.0/4.0 (`work`, `home`, `cell`, `voice`, `fax`, `internet`…) y se emiten tal cual. Lo que se prohíbe por construcción es cualquier carácter que pueda romper la línea (`:`, `;`, `,`, CR, LF, comillas). Esto entra en conflicto conocido con la RFC (en 4.0 `TEL` no tiene `work`/`home`) y **se acepta a cambio de interoperabilidad**: se documenta.
6. **URI sí, texto sí: cada uno con su encoding.** Las propiedades de tipo texto se escapan (RFC 6350 §3.4: `\`, `,`, `;`, salto de línea); las de tipo URI (`URL`, `IMPP`, `GEO`, `TZ`, `PHOTO`-URI, `LOGO`) se emiten **verbatim** tras rechazar caracteres de control —escapar una URI la corrompe—. Los valores estructurados (`N`, `ADR`, `ORG`, `CATEGORIES`, `GENDER`) usan `;` **sin escapar** como separador de componentes y **escapan** los `;` internos de cada componente.
7. **Presencia = reemplazo; ausencia = intacto.** En el `PATCH`, una propiedad **ausente** no se toca; una propiedad **presente** sustituye **todas** sus apariciones; un array **vacío** (`[]`) **borra** la propiedad. Los cuatro campos escalares heredados (`fn`, `email`, `phone`, `organization`) conservan la semántica actual (`""` limpia, ausente no toca) para no romper el contrato existente.
8. **Rechazar, no sanear.** Todo campo nuevo pasa por la disciplina vigente (`is_forbidden_char`, longitudes acotadas, número de valores acotado) y devuelve `400`, nunca `500`. Un valor largo o con caracteres prohibidos **no se recorta en silencio**.
9. **`REV` no se fabrica.** Se muestra si está presente y se preserva; el servidor no lo refresca al escribir (lo contrario reescribiría el payload en cada operación y rompería la decisión 2).
10. **La vista agrupa y no dibuja vacío.** Ocho pestañas (las siete del encargo + «Otros / extensiones»); una pestaña **sin contenido no se renderiza**.
11. **La proyección no entra en la lista.** `GET .../contacts` sigue devolviendo el DTO ligero (la tabla no necesita ni fotos ni direcciones); el detalle completo es de `GET .../contacts/{uid}`. El DTO del detalle es **superconjunto** del ligero, así que el cambio es aditivo.
12. **El SPA sigue sin parsear vCard.** Ni en la vista ni en la edición: los tipos nuevos reflejan el DTO del servidor.

## Prerequisitos (fuera del código)

- Ninguno de infraestructura: no hay migración, no hay variable de entorno nueva, no hay dependencia nueva.
- **Medir antes del primer RED**: anotar el recuento de tests del backend (`cargo test --locked`) y el tamaño de bundle, para que el plan no afirme cifras que no se hayan medido. El recuento frontend de partida es **14 ficheros / 124 tests** y el bundle **1 270 610 B**.
- `compose.yaml` está modificado en el árbol de trabajo por el usuario: **queda fuera de todos los commits** de F22.

## Contratos / Interfaces

### 1. `src/dav/vcf.rs` → `src/dav/vcf/` (movimiento puro)

`vcf.rs` pasa a ser una carpeta:

- `src/dav/vcf/mod.rs`: el contenido actual **movido tal cual** (parseo, `ensure_formatted_name`, `inject_property`, `build_vcf`, tests) **más** la proyección completa (contrato 3).
- `src/dav/vcf/merge.rs`: el merge quirúrgico (contrato 2), con sus tests.

La ruta pública **no cambia** (`dav::vcf::parse_vcf`, `dav::vcf::build_vcf`), así que no hay que tocar ni un `use` fuera del módulo. El movimiento se prueba con la **suite existente sin modificar**: si algo se rompe, es que el movimiento no era puro.

### 2. `src/dav/vcf/merge.rs` (nuevo)

API pública, con el encoding a cargo del módulo (una sola casa para la de-serialización vCard, como ya documenta `contacts.rs`):

```rust
/// Una propiedad a sustituir en el payload: todas sus apariciones se reemplazan
/// por `lines` (vacío = borrar la propiedad por completo).
pub struct PropertyEdit {
    /// Nombre de la propiedad (`TEL`, `EMAIL`, `ADR`, `N`, `X-ABLabel`…).
    pub name: String,
    /// Líneas de contenido ya renderizadas (sin CRLF, sin folding).
    pub lines: Vec<String>,
}

/// Errores del merge.
pub enum MergeError {
    /// El payload no tiene un `END:VCARD` localizable.
    MissingTerminator,
    /// El payload no es una vCard (sin envoltura).
    NotACard,
}

/// Sustituye las propiedades de `edits` dejando **intacto** el resto del payload.
pub fn merge_properties(raw: &str, edits: &[PropertyEdit]) -> Result<String, MergeError>;
```

Contrato operativo (lo que el test debe poder demostrar):

- Se localizan las **líneas lógicas** (una línea física que empieza por espacio o tab **continúa** la anterior, RFC 6350 §3.2) y se clasifican por el **nombre de propiedad**: lo que hay antes del primer `:`, quitando el prefijo de grupo (`item1.TEL`) y los parámetros (`;TYPE=…`), comparado **sin distinguir mayúsculas**.
- Se eliminan **todas** las líneas lógicas cuyo nombre coincida, **incluidas sus continuaciones de folding**.
- Las líneas nuevas se insertan **antes del último `END:VCARD` lógico** (nunca dentro de un valor que contenga ese texto, y admitiendo `end:vcard` en minúsculas).
- El resto del payload se devuelve **byte a byte**: mismo terminador de línea, mismo orden, mismos parámetros.
- `edits` vacío → el payload se devuelve **idéntico**.
- **Idempotencia**: aplicar dos veces los mismos `edits` produce el mismo resultado.
- El payload resultante **parsea** y sus propiedades son las pretendidas (el merge nunca produce una tarjeta inválida).
- El **terminador de línea** del fichero se respeta (CRLF y LF, cada uno con su test).

Renderizado (en el mismo módulo o en `mod.rs`, pero con una sola implementación):

```rust
/// Renderiza una propiedad de texto (`FN`, `NICKNAME`, `TITLE`, `NOTE`, `CATEGORIES`…).
pub fn render_text_property(name: &str, value: &str, types: &[String], pref: Option<u8>) -> String;

/// Renderiza un valor estructurado con componentes separados por `;` (`N`, `ADR`, `ORG`).
pub fn render_structured_property(name: &str, components: &[String], types: &[String], pref: Option<u8>) -> String;

/// Renderiza una propiedad de tipo URI (`URL`, `IMPP`, `GEO`, `TZ`, `PHOTO`).
pub fn render_uri_property(name: &str, value: &str, types: &[String], pref: Option<u8>) -> String;
```

Los parámetros se validan aquí (alfabeto acotado, decisión 5) y una violación es un error de validación, no un saneado.

### 3. `src/dav/vcf/mod.rs` — proyección completa

```rust
/// Proyección tipada y completa de una vCard, para el DTO de detalle.
pub struct CardProjection {
    pub version: String,
    pub uid: String,
    pub fn_: Option<String>,
    pub rev: Option<String>,
    pub name: Option<StructuredName>,       // N, en sus 5 componentes
    pub nicknames: Vec<String>,             // NICKNAME
    pub photos: Vec<MediaValue>,            // PHOTO (URI o binario embebido)
    pub birthday: Option<String>,           // BDAY, tal cual venga
    pub anniversary: Option<String>,        // ANNIVERSARY
    pub gender: Option<Gender>,             // GENDER (sexo + identidad)
    pub tels: Vec<MultiValue>,              // TEL
    pub emails: Vec<MultiValue>,            // EMAIL
    pub impps: Vec<MultiValue>,             // IMPP
    pub addresses: Vec<Address>,            // ADR (7 componentes + LABEL)
    pub geos: Vec<String>,                  // GEO
    pub timezones: Vec<String>,             // TZ
    pub titles: Vec<String>,                // TITLE
    pub roles: Vec<String>,                 // ROLE
    pub organizations: Vec<Vec<String>>,    // ORG (componentes)
    pub logos: Vec<MediaValue>,             // LOGO
    pub notes: Vec<String>,                 // NOTE
    pub categories: Vec<Vec<String>>,       // CATEGORIES (una entrada por línea)
    pub urls: Vec<String>,                  // URL
    pub sounds: Vec<MediaValue>,            // SOUND
    /// Todo lo demás: `X-*` y las propiedades RFC fuera de los grupos anteriores.
    pub others: Vec<RawProperty>,
}

/// Un valor con sus parámetros.
pub struct MultiValue {
    pub value: String,
    pub types: Vec<String>,
    pub pref: Option<u8>,
}

/// `PHOTO`/`LOGO`/`SOUND`: o una URI, o un binario embebido como `data:` URI.
pub enum MediaValue {
    /// URI externa, tal cual venía.
    Uri(String),
    /// Contenido embebido, ya como `data:<media-type>;base64,<…>`.
    Data { media_type: Option<String>, data_uri: String },
}

/// Una propiedad no modelada explícitamente (pestaña «Otros / extensiones»).
pub struct RawProperty {
    pub name: String,
    pub group: Option<String>,
    pub parameters: Vec<(String, String)>,
    pub value: String,
}
```

`fn parse_vcf` **mantiene** su firma y su `ParsedCard` (los seis campos desnormalizados): es lo que usa CardDAV y de lo que se derivan las columnas. Se **añade** `project_card(raw) -> Result<CardProjection, VcfError>` (o equivalente) que reutiliza el mismo parseo.

### 4. `src/api/dto.rs`

DTO de detalle (**superconjunto** del ligero; aditivo):

```rust
pub struct ContactDetailDto {
    pub id: String,
    pub uid: String,
    pub collection_id: String,
    pub fn_: Option<String>,          // serde(rename = "fn")
    pub full_name: Option<String>,
    pub etag: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: Option<String>,
    pub rev: Option<String>,
    pub name: Option<StructuredNameDto>,
    pub nicknames: Vec<String>,
    pub photos: Vec<MediaDto>,
    pub birthday: Option<String>,
    pub anniversary: Option<String>,
    pub gender: Option<GenderDto>,
    pub tels: Vec<LabelledValueDto>,
    pub emails: Vec<LabelledValueDto>,
    pub impps: Vec<LabelledValueDto>,
    pub addresses: Vec<AddressDto>,
    pub geos: Vec<String>,
    pub timezones: Vec<String>,
    pub titles: Vec<String>,
    pub roles: Vec<String>,
    pub organizations: Vec<Vec<String>>,
    pub logos: Vec<MediaDto>,
    pub notes: Vec<String>,
    pub categories: Vec<Vec<String>>,
    pub urls: Vec<String>,
    pub sounds: Vec<MediaDto>,
    pub others: Vec<RawPropertyDto>,
}
```

Con `StructuredNameDto { family, given, additional, prefix, suffix }` (todos `Option<String>`), `AddressDto { po_box, extended, street, locality, region, postal_code, country, label, types, pref }`, `GenderDto { sex: Option<String>, identity: Option<String> }`, `LabelledValueDto { value, types, pref }`, `MediaDto { kind, uri, media_type, data_uri }` y `RawPropertyDto { name, group, parameters, value }`.

`ContactDto` (la lista) y `ContactSearchResult` **no cambian**.

Peticiones: `CreateContactRequest` y `UpdateContactRequest` ganan los campos nuevos, cada uno opcional. Nombres y tipos en snake_case, coherentes con el resto:

- `nicknames?: string[]`, `birthday?: string`, `anniversary?: string`, `gender?: {sex?, identity?}`,
- `tels?: LabelledValue[]`, `emails?: LabelledValue[]`, `impps?: LabelledValue[]`,
- `addresses?: AddressInput[]`, `geos?: string[]`, `timezones?: string[]`,
- `titles?: string[]`, `roles?: string[]`, `organizations?: string[][]`, `urls?: string[]`, `notes?: string[]`, `categories?: string[][]`,
- `name?: StructuredName`.
- Los binarios (`photos`, `logos`, `sounds`) **no** son editables y **no** aparecen en las peticiones.

`UpdateContactRequest::is_empty()` se amplía a todos los campos nuevos: un `PATCH` sin ningún campo sigue siendo `400`.

### 5. `src/api/contacts.rs`

- `get_one`: carga la fila, `project_card(&row.raw_payload)` y devuelve `ContactDetailDto`. Si el payload almacenado **no parsea** (solo posible con filas corruptas), se registra y se responde **`500`** — igual que hoy hace `validate_stored_uid` con un `uid` no canónico: es integridad del servidor, no un error del cliente.
- `create`: valida, mintea `id`/`uid`, construye el **esqueleto** (`BEGIN`, `VERSION:4.0`, `UID`, `FN`, `END`) y aplica el merge de todos los campos restantes. Luego re-parsea y deriva las columnas.
- `update`: valida, lee la fila, `validate_stored_uid`, construye los `PropertyEdit` de **solo** los campos presentes y aplica el merge **sobre el payload almacenado**. Luego re-parsea y re-deriva las columnas.
- Validación por campo (nuevos límites, todos rechazo → `400`): longitudes máximas por propiedad, número máximo de valores por propiedad (p. ej. 32 `TEL`), tipos de parámetro con alfabeto acotado, `PREF` en `1..=100`, `GENDER` restringido a los valores del RFC (`M`/`F`/`O`/`N`/`U` para el sexo) o rechazado.
- `MAX_*` nuevos junto a los existentes (`MAX_FN_LEN`, `MAX_EMAIL_LEN`, …) siguiendo el mismo patrón.

### 6. `src/api/mod.rs`

Sin rutas nuevas **en el alcance base**. Si se aprueba la importación del lado servidor (ver Preguntas abiertas), sería `POST /collections/{id}/contacts/import` con `Content-Type: text/vcard` y exigiría `DefaultBodyLimit` explícito, porque el límite por defecto de axum (2 MB) es **menor** que los 5 MB que hoy acepta el cliente.

### 7. `web/src/types/api.ts`

Tipos espejo de los DTO anteriores (`ContactDetail`, `StructuredName`, `Address`, `LabelledValue`, `Gender`, `Media`, `RawProperty`) y ampliación de `CreateContactInput`/`UpdateContactInput`. El comentario de cabecera sigue diciendo que son un espejo del backend.

### 8. `web/src/services/contacts.ts` y `web/src/hooks/useContactDetail.ts` (nuevo)

- `getContact(collectionId, uid, signal)` → `GET .../contacts/{uid}`.
- Hook nuevo con el patrón ya establecido en `useContacts`/`Contacts.tsx`: `AbortController`, guarda de respuesta obsoleta y estado derivado de una *request key* (nada de escribir estado dentro de un efecto, que la regla `react-hooks/set-state-in-effect` lo prohíbe).

### 9. `web/src/components/contact/ContactDetailTabs.tsx` (nuevo) y vecinos

- Componente de pestañas con las **ocho** categorías; una pestaña sin contenido **no se renderiza**; si no hay ninguna, se muestra un `<Empty>`.
- Renderizadores por categoría (tablas/`Descriptions`/listas de `Tag`s para `TYPE`, `Tag`/`Text` para `PREF`).
- `PHOTO`/`LOGO` como `<Avatar>`/`<Image>`: `data:` URI para binario embebido, `src` para URI externa; **`<img>` con `referrerPolicy="no-referrer"`** y sin inyección de HTML.
- La pestaña «Servidor» muestra `ETag`, colección y `created_at`/`updated_at` (formateados como en el resto de la app).
- La pestaña «Otros / extensiones» muestra nombre, grupo, parámetros y valor de cada propiedad no modelada.

### 10. `web/src/pages/Contacts.tsx`

- El `<Drawer>` deja de pintar el `<Descriptions>` de 5 filas y pinta `ContactDetailTabs`, con carga, error y reintento.
- El formulario gana `Form.List` para `TEL`/`EMAIL`/`IMPP`/`URL`/`ADR`/`CATEGORIES` y campos para los escalares nuevos.
- Se mantiene el patrón de actualización optimista para los campos que ya lo tenían; para el resto, la lista se refresca con la respuesta del servidor.
- El `Drawer` **deja de abrirse solo con el DTO de la lista**: hay una petición de detalle al abrir, con su estado de carga (esto cambia una interacción que hoy es síncrona; los tests deben reflejarlo).

### 11. `web/src/utils/vcf.ts`

Sin cambios funcionales en el alcance base: sigue siendo el parser/serializador **cliente** de la importación/exportación de conveniencia, limitado a los cinco campos. Se actualiza su comentario de cabecera para **decir explícitamente** que la importación desde el SPA es **lossy** (si se aprueba el contrato 6, ese comentario cambia).

## Dependencias

- **Ninguna nueva.** Ni en `Cargo.toml` ni en `web/package.json`.
- `vcard4` 0.7.3 se usa más (proyección y `extensions`), pero no cambia de versión.
- `Cargo.lock` **no** debería cambiar; si cambia, algo se ha colado.

## Testing (red-green-refactor)

### RED (tests primero)

1. **`merge.rs`** (unit): borrado de una propiedad con continuaciones de folding; borrado de **todas** las apariciones; inserción antes del último `END:VCARD` (incluido uno en minúsculas y uno que aparece dentro de un `NOTE`); **byte a byte** del resto; CRLF y LF; `edits` vacío = idéntico; idempotencia; sin `END:VCARD` = error; un valor que contiene `TEL:` no se confunde con la propiedad `TEL`; parámetros inválidos = error; encoding de texto vs URI vs estructurado.
2. **Proyección** (unit): una tarjeta realista (foto base64, `X-ABLabel`, `item1.TEL`, varios `TEL`/`EMAIL`/`ADR`, `TZ`, `GEO`, `IMPP`, `CATEGORIES`, `REV`) se proyecta sin perder nada; una tarjeta mínima produce pestañas vacías en vez de errores.
3. **Invariante de columnas** (unit): para un corpus de payloads, `columnas == parse_vcf(payload)`.
4. **API** (integración, `tests/api.rs`):
   - **Regresión del defecto**: un contacto creado por CardDAV con `PHOTO`+`BDAY`+`X-ABLabel`, al que se le hace `PATCH` de `fn`, **conserva** esas propiedades (el `raw_payload` difiere **solo** en la línea `FN`).
   - `GET .../contacts/{uid}` devuelve las ocho categorías; `others` recoge `X-*` y las RFC no modeladas.
   - `PATCH` con `[]` borra la propiedad; ausente la deja; presente la reemplaza entera.
   - Validación: `TYPE` inválido, `PREF` fuera de rango, control chars en cada campo nuevo, longitud excesiva, demasiados valores → **`400`** (nunca `500`).
   - Un `PATCH` no puede cambiar el `UID`; `VERSION` no se toca.
5. **Frontend** (vitest + RTL): el detalle se pide al abrir el `Drawer`; las pestañas se dibujan según contenido; el vacío oculta la pestaña; los multivalor se listan con sus `TYPE`/`PREF`; `PHOTO` como `data:` URI se pinta; error y reintento; el formulario envía arrays bien formados.

### GREEN

Implementación mínima para pasar los tests anteriores, en el orden de los contratos.

### REFACTOR

`cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `npm run lint` desde `web/`. Extraer lo que pida el código (validadores comunes, renderizadores por categoría) sin cambiar comportamiento. Re-medir bundle y anotar el delta.

## Tareas (TDD)

### Fase 0 — Prerrequisitos

- [ ] Medir y anotar: recuento de tests del backend (`cargo test --locked`) y bundle actual (`npm run build`). No escribir cifras no medidas.
- [ ] Crear la rama `feature/full-vcard-support` desde `development` actualizado.

### Fase 1 — Tests (RED)

- [ ] Mover `src/dav/vcf.rs` → `src/dav/vcf/mod.rs` (movimiento puro) y verificar que la suite existente pasa **sin tocarla**.
- [ ] Escribir los tests de `merge.rs` (borrado, folding, byte-identidad, CRLF/LF, idempotencia, errores, encoding) y verlos fallar.
- [ ] Escribir los tests de la proyección completa y el invariante de columnas; verlos fallar.
- [ ] Escribir los tests de los DTO y de la validación nueva (unit); verlos fallar.
- [ ] Escribir los tests de integración en `tests/api.rs`, **incluida la regresión de preservación**; verlos fallar.

### Fase 2 — Implementación (GREEN)

- [ ] Implementar `render_*` y `merge_properties` hasta verde.
- [ ] Implementar `project_card` y la derivación de columnas hasta verde.
- [ ] Añadir los DTO y ampliar las peticiones; validadores nuevos.
- [ ] Reescribir `create`/`update` con esqueleto + merge y `get_one` con la proyección.
- [ ] Eliminar lo que quede muerto (`CardFields` de 6 campos, o reducido a esqueleto) y actualizar sus 8 referencias de test.

### Fase 3 — Frontend

- [ ] Tipos y servicio; hook `useContactDetail` con aborto y guarda de obsolescencia.
- [ ] `ContactDetailTabs` y renderizadores por categoría; pestañas vacías fuera.
- [ ] `Drawer` con carga/error/reintento; `Form.List` en el formulario.
- [ ] Tests de componentes/página; verdes.

### Fase 4 — Refactor y verificación

- [ ] `cargo fmt --all` && `cargo clippy --all-targets -- -D warnings` && `cargo test --locked`.
- [ ] `npm run test` && `npm run typecheck` && `npm run lint` && `npm run build` desde `web/`.
- [ ] Bundle: medir y anotar el delta respecto a **1 270 610 B**.
- [ ] Verificar la partición de proyectos TypeScript con el test de invariantes de F21 (no debe romperse por los ficheros nuevos).
- [ ] Auditoría con `rust-reviewer` (backend) y `react-reviewer` (frontend); resolver hallazgos.
- [ ] `docs/F22-full-vcard-support.md` y renombrar este plan a `-COMPLETED.md`.

## Criterios de aceptación

1. **Preservación demostrada.** Un `PATCH` sobre un contacto importado por CardDAV con `PHOTO` + `BDAY` + `X-ABLabel` + `item1.TEL` deja el `raw_payload` **idéntico** salvo la línea de la propiedad editada (comparación byte a byte en un test de integración).
2. **Vista completa.** `GET .../contacts/{uid}` devuelve las ocho categorías; una tarjeta con `X-*` las expone en `others`; una tarjeta mínima no produce errores.
3. **Invariante de columnas.** Para todo payload del corpus: columnas desnormalizadas == resultado de `parse_vcf`.
4. **Un solo camino de escritura.** `create` y `update` comparten el merge; no queda composición alternativa que pueda divergir.
5. **Validación.** Todo campo nuevo rechaza control chars, longitudes excesivas, parámetros fuera de alfabeto y exceso de valores con **`400`**, nunca `500`; el `UID` es inmutable en `PATCH`; `VERSION` no se toca.
6. **Cero migraciones.** `migrations/` no cambia; `contacts_fts` y los triggers intactos; la búsqueda por `fn`/`email`/`phone`/`organization`/`notes` sigue igual.
7. **Cambio aditivo en el contrato.** `ContactDto` y `ContactSearchResult` conservan su forma; el detalle es un superconjunto; `DELETE` y el resto de endpoints intactos.
8. **UI.** Ocho pestañas como máximo, ninguna vacía dibujada; multivalor con sus `TYPE`/`PREF`; `PHOTO` renderizada; carga, error y reintento en el `Drawer`.
9. **Suites verdes.** Backend sin regresiones (con el recuento medido en la Fase 0), frontend 124 tests + los nuevos, `typecheck`/`lint`/`build` limpios.
10. **Bundle** medido y delta reportado.

## Riesgos

- **La cirugía de líneas del merge es la parte más delicada.** Folding, valores que imitan nombres de propiedad, terminadores en minúsculas heredados de un cliente. Mitigación: los tests unitarios del contrato 2 + el criterio de aceptación 1, que es una comparación byte a byte, no una comprobación de "contiene".
- **Eliminar `CardFields`/`build_vcf` toca 8 referencias de test** y el camino de escritura completo. Mitigación: la fase RED reescribe esos tests **antes** de tocar la implementación.
- **La lista y el detalle se desacoplan**: el `Drawer` pasa a depender de una petición. Mitigación: `useContactDetail` con aborto y guarda de obsolescencia, más tests de carga/error.
- **`PHOTO` en JSON.** Una foto embebida son cientos de KB de base64 en la respuesta de detalle. Es aceptable en un servidor personal con un usuario; se documenta y **no** se expone en la lista.
- **Interop 3.0/4.0 de `TYPE`.** Decisión 5: unión de vocabularios, alfabeto acotado; documentado como desviación consciente de la RFC.
- **Crecimiento del bundle** por `Tabs`/`Avatar`/`Form.List`, que ya vienen en AntD pero pueden arrastrar más código. Mitigación: medir y reportar.
- **La exportación `.vcf` del SPA sigue siendo lossy** (cinco campos). Mitigación: documentarlo de forma **explícita** en el comentario del fichero y en `docs/F22`; ver Preguntas abiertas.

## Preguntas abiertas (decidir antes de implementar)

1. **¿La importación pasa al servidor?** Hoy `web/src/utils/vcf.ts` parsea en el cliente un subconjunto y **pierde** el resto de cada tarjeta, que es el mismo defecto que arreglamos en el `PATCH`. Un `POST .../contacts/import` con `Content-Type: text/vcard` reutilizaría `parse_vcf` + `insert_card_object` (exactamente lo que hace el `PUT` de CardDAV) y almacenaría cada tarjeta **verbatim**. Recomendado: **sí, dentro de F22**, porque es el mismo defecto y la implementación es pequeña. Coste: una ruta nueva y `DefaultBodyLimit` explícito (el defecto de axum son 2 MB y el cliente acepta 5 MB).
2. **¿La exportación pasa al servidor?** Simétrico al anterior: un `GET .../contacts/{uid}?format=vcard` (o un `export`) devolvería el `.vcf` **fiel** del servidor sin exponer `raw_payload` en el JSON. Recomendado: **dejarlo para un plan aparte**, para no engordar F22.
3. **¿Debe el servidor refrescar `REV` al escribir?** Recomendado: **no** (decisión 9).

## Referencias

- RFC 6350 (vCard 4.0) — https://www.rfc-editor.org/rfc/rfc6350
- RFC 2426 (vCard 3.0) — https://www.rfc-editor.org/rfc/rfc2426
- RFC 6352 (CardDAV) — https://www.rfc-editor.org/rfc/rfc6352
- Crate `vcard4` 0.7.3 — modelo de 36 propiedades, `parse_loose` y `Display`.
- `src/dav/vcf.rs` (parseo y composición actuales), `src/api/contacts.rs` (escritura actual), `src/api/dto.rs`, `migrations/0001_init.sql`, `src/db/mod.rs:180` (`compute_etag`).
- `web/src/pages/Contacts.tsx`, `web/src/utils/vcf.ts`, `web/src/types/api.ts`, `web/src/hooks/useContacts.ts`.
- `plans/PLAN-009-carddav-core-COMPLETED.md` (CardDAV), `plans/PLAN-013-contacts-view-COMPLETED.md` (vista de contactos), `plans/PLAN-021-design-tokens-cleanup-COMPLETED.md` y `plans/PLAN-022-tsconfig-test-project-COMPLETED.md` (formato y convenciones recientes).

---

## Enmienda de cierre (F22 completada)

Registro de todo lo que la implementación **contradijo, precisó o añadió** respecto a lo previsto en este plan. Se conserva el plan original sin reescribir: la historia de la decisión está en los commits.

- **Decisión 11 (el DTO de detalle es superconjunto del ligero) — REVERTIDA.** El DTO implementado **omite** `email`, `phone` y `organization` para no duplicar el dato (`email` vs `emails[0].value`). El SPA es el único consumidor y usa los arrays. Aceptado y documentado en `docs/F22-full-vcard-support.md`.
- **`StructuredName.additional/prefix/suffix` son listas** (`Vec<String>`), no `Option<String>`: en RFC 6350 son componentes múltiples. El plan estaba equivocado.
- **Las sub-listas del `N` se separan con coma sin escapar** (RFC 6350 §6.2.2) — caso que el plan no cubría y que producía un `N` no conforme.
- **`VERSION`, `REV`, `BDAY` y `ANNIVERSARY` se leen verbatim de las líneas crudas**, porque el modelo tipado normaliza y llega a falsear (`BDAY:--0203` → `00000203`). No estaba previsto.
- **Los `TYPE` se proyectan sin el prefijo `X-`** que añade el `Display` de `vcard4`. No estaba previsto.
- **Borrado de objetos y textos opcionales**: un grupo presente y **totalmente vacío** (`{}`, `[]`, `""`) **borra** la propiedad. Es una extensión explícita de la decisión 7, que no decía cómo borrar `N` o `GENDER`.
- **La importación es atómica de verdad** (una transacción y prevalidación de duplicados): el plan pedía guardar verbatim, no dejaba claro el comportamiento ante un `uid` repetido.
- **El primitivo `merge_properties` impone la decisión 4** (ignora ediciones de `UID`/`VERSION`/`BEGIN`/`END` con `debug_assert`), en lugar de dejarla solo al llamante.
- **Las líneas insertadas por el merge se pliegan a 75 octetos** (RFC 6350 §3.2), como hacía `build_vcf`.
- **El límite de cuerpo de 16 MiB se aplica solo a la ruta de importación**, no globalmente.
- **Bundle**: +78 277 B (+6,2 %) sobre la línea base de F21.
- **Diferido por recomendación de las auditorías**: partir `src/dav/vcf/mod.rs` (1 714 líneas) y `src/api/contacts.rs` (1 321), y evitar el doble parseo de `project_card`.
