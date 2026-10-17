# F24 — El `.ics` se conserva al editar y `LOCATION`/`URL` son campos de primera clase

Plan: `plans/PLAN-028-ics-preservacion-location-url-COMPLETED.md`.

## Qué entrega

El `.ics` guardado es el artefacto que sincronizan los clientes CalDAV. Hasta F24, el `PATCH` de la SPA lo **recomponía desde cero**: `build_ics` (`src/dav/ics.rs`) emitía exactamente siete propiedades (`UID`, `DTSTAMP`, `SUMMARY`, `DESCRIPTION`, `DTSTART`, `DTEND`, `RRULE`) y todo lo demás **desaparecía**. Un evento de Thunderbird o iOS con `VALARM` (la alarma), `ATTENDEE`, `LOCATION`, `CATEGORIES` o `X-*` los perdía en cuanto alguien le cambiaba el título desde la web, sin aviso y sin vuelta atrás.

- El `PATCH` ya no recompone: **reconcilia**. `reconcile_ics(raw, &fields)` toca **solo** las propiedades modeladas del primer `VEVENT` y reemite **verbatim** todo lo demás, con su plegado original: `VALARM` (y el `SUMMARY` **de la alarma**, que no es el del evento), `ATTENDEE` × N, `CATEGORIES`, `X-*`, un segundo `VEVENT`, un `VTODO`…
- **`LOCATION` y `URL`** son campos de primera clase en las cuatro capas: parser → columnas (migración `0005`) → DTO/API → diálogo de creación/edición.
- Desde la SPA se puede **vaciar** descripción, ubicación y URL (antes era imposible: la interfaz no enviaba nunca un campo vacío).
- El `PATCH` toma el **payload almacenado** como fuente de verdad al mezclar: `petición → payload → columna`.

## El fallo que cierra, medido

- `src/dav/caldav.rs` (`put_event`) guarda el payload **verbatim**: el `.ics` de entrada sobrevive intacto.
- `src/api/events.rs` (`update`) lo **reemplazaba** por `build_ics(&fields)`.
- `build_ics` solo conoce siete propiedades.

Auditoría de las propiedades estándar de un `VEVENT`: **7 de 26** utilizables antes de F24.

## Decisiones de diseño

| Decisión | Por qué |
|---|---|
| Reconciliación **línea a línea** del payload, no recompilar desde el mapa de propiedades del parser | `icalendar 0.17` guarda las propiedades de un componente en un `BTreeMap`: aunque conserva los duplicados, **destruye el orden** y normaliza los valores. Reconstruir desde ahí reescribe el payload entero; reconciliar deja las líneas no tocadas **byte a byte** |
| No serializar con `icalendar` | Está declarado `default-features = false, features = ["parser"]`: **solo parser**. Por eso `build_ics` es artesanal |
| Un **único serializador**: `build_ics` y `reconcile_ics` comparten los helpers de línea (`summary_line`, `dtstart_line`, …) | Dos rutas de escritura no pueden divergir |
| `build_ics` sin `location`/`url` sigue siendo **byte a byte idéntico** al de antes | Es el contrato que mantiene la feature inerte para todo evento que no use los campos nuevos |
| El `.ics` es la **fuente de verdad** del *merge*; la columna es un índice | Las columnas de `*_tzid` (`0004`) y `location`/`url` (`0005`) son **aditivas**: una fila escrita antes de la migración las tiene a `NULL` aunque su payload lleve la propiedad. Tomar la columna primero reescribiría **en silencio** un campo que el `PATCH` ni menciona |
| `VTIMEZONE`: se **añade** si falta la zona final, y **nunca** se borra uno existente | Un bloque no referenciado es válido; borrarlo arriesga romper una referencia (por ejemplo un `TZID` dentro de un `VALARM`). La acumulación es el coste aceptado |
| Solo se valida **lo que entra por la API**; un valor heredado pasa tal cual | Revalidarlo daría `400` a **cualquier** `PATCH` de un evento importado con `URL:mailto:…` o `ftp://…` (esquemas fuera de la lista blanca) o con una `LOCATION` larga |
| La SPA envía `description`/`location`/`url` **solo si el usuario los ha cambiado** | Una fila antigua puede llevar un `LOCATION` en el `.ics` que la lista no conoce, así que el diálogo lo muestra **vacío**: enviarlo siempre lo borraría sin que el usuario se entere. Vaciar el campo **es** un cambio, así que el borrado explícito sigue funcionando |
| El validador del formulario **acepta el valor con que se abrió** y solo exige `http(s)://` si cambia | Sin eso, un evento con `URL:mailto:` importado por un cliente nativo queda **imposible de renombrar** |
| `URL` en la lista blanca del servidor (`http`/`https`) y, además, **no se pinta como enlace** si no lo cumple | El `URL` se emite **verbatim** en el `.ics` (una URI no se escapa), así que la validación es una frontera de seguridad contra la inyección de líneas y contra un `javascript:` como enlace |

## Hallazgos de la auditoría, cerrados

1. **Bloqueante (frontend): el validador de `URL` hacía ineditable cualquier evento con una URL heredada no `http(s)`.** Se podía renombrar **solo** borrando la URL, es decir destruyéndola. Ahora el validador acepta el valor de partida y solo valida lo que cambia, con un test de regresión que demuestra que un evento con `URL:mailto:…` se guarda sin perderla.
2. **El validador del formulario no espejaba al del servidor.** Un espacio final pasaba en el navegador y el servidor respondía `400`. Ahora se recorta y se rechazan espacios y caracteres de control, con el mismo criterio.
3. **`location`/`url` no se recortaban** (a diferencia de `description`). Ahora se recortan, y la comparación del envío se hace entre valores recortados: un valor heredado con un espacio final que el usuario **no** toca se considera igual y **no** se envía, así que se conserva tal cual.
4. **El envío selectivo dependía de que el `Form` se remontara.** Se añadió `key` al formulario, pero **no bastaba** —se reprodujo con un test en rojo—: AntD mezcla `initialValues` *sobre* el store y gana el store, así que hizo falta además `form.resetFields()`, que es el mecanismo real del patrón de la casa (`ContactForm.tsx` usa los dos).
5. **Faltaba un test negativo del enlace.** Añadido: con `url: "javascript:alert(1)"` y `data:text/html,…` **no** se pinta ningún `<a>`.

Notas de la auditoría de backend, **sin hallazgos bloqueantes**:

6. Un `URL:javascript:…` **metido por un cliente DAV** (`PUT`, que no valida esquemas) se guarda verbatim, porque decididamente **no** se revalida lo heredado. El riesgo es auto-infligido (no hay compartición entre usuarios) y está **mitigado en el pintado**, que no genera enlace si el esquema no es `http(s)`. Queda dicho, no escondido.
7. Si un payload tuviera un `VEVENT` **sin `UID` ni `DTSTART`** utilizable, `parse_ics` falla, el *merge* cae a las columnas (`NULL`) y `reconcile_ics` borraría esas líneas. Es inalcanzable por las vías de escritura (`PUT`/`POST` exigen un `.ics` parseable y `build_ics` siempre emite `UID` y `DTSTART`).

## Verificación

- Backend: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings` y `cargo test --locked` → **594 pasando / 0 fallando** (antes: 563). Los tests del módulo `dav::ics` pasaron de 39 a 49.
- Frontend (desde `web/`): `npm run test` → **20 ficheros / 229 tests** (antes: 217), `npm run typecheck`, `npm run lint` y `npm run build` limpios.
- El test bandera **mide el fallo**: siembra un `.ics` con `VALARM`, dos `ATTENDEE`, `CATEGORIES`, `LOCATION` y una `X-`; hace un `PATCH` que solo cambia el título; y exige que entre el payload de antes y el de después difieran **exactamente dos líneas** (`SUMMARY` y `DTSTAMP`), con el mismo número de líneas. Con la implementación anterior fallaba; con la reconciliación pasa.
- Prueba negativa: `PATCH` con `url: "javascript:alert(1)"` → `400` y el payload almacenado **no cambia**.

## Deuda que queda (dicha, no escondida)

- **No hay *backfill* de las columnas nuevas.** Una fila escrita antes de `0005` tiene `location`/`url` a `NULL` aunque su payload lleve las líneas (exactamente lo que `0004` aceptó para `*_tzid`, y su test lo afirma por diseño). El `.ics` sigue siendo la fuente de verdad y la columna se **cura** en la primera escritura —por eso el `PATCH` no pierde el dato—, pero hasta entonces la lista y el diálogo no muestran la ubicación de esos eventos. **Está pendiente decidir** si se quiere un *backfill*.
- `RECURRENCE-ID` e instancias desprendidas: imposible sin relajar el índice único `uq_calendar_objects_collection_uid`. Feature propia.
- `ORGANIZER`/`ATTENDEE`, `ATTACH`, `STATUS`, `CLASS`, `TRANSP`, `CATEGORIES`, `EXDATE`/`RDATE`, `DURATION`: no modeladas todavía. Las que no se modelan **se conservan** (lo garantiza esta feature).
- `VALARM`: se conserva, no se edita.
- `SEQUENCE`/`CREATED`/`LAST-MODIFIED`: no expuestos. `DTSTAMP` se refresca en **cada** `PATCH` (intencionado).
- Un `DTSTART;TZID=<zona no IANA>` heredado se reemite como UTC: es la degradación documentada de F18, sin cambio.
- `LOCATION` no se indexa en la búsqueda FTS.
