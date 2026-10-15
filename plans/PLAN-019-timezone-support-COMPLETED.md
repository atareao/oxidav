# PLAN-019 — F18 Soporte de zonas horarias (TZID) extremo a extremo con VTIMEZONE

## Objetivo

Que un evento pueda quedar **anclado a una zona horaria IANA** de punta a punta
(parseo DAV, esquema, composición iCalendar y API REST), de modo que una recurrencia
(`RRULE`) se repita en **hora local** y no derive una hora en los cambios de horario
de verano.

## Alcance

Esta feature (F18) hace que el `TZID` de un evento sea un dato de primera clase y lo
propaga por toda la cadena. Incluye:

1. **TZID extremo a extremo**: esquema + parseo DAV + composición + API REST + SPA.
2. El `.ics` compuesto **incluye un bloque `VTIMEZONE` completo** derivado de la tzdb
   (RFC 5545 §3.2.19), no solo el parámetro `TZID`.
3. Arreglar el bug de parseo DAV (una hora de desfase en cualquier evento con `TZID`)
   y la aserción errónea de `keeps_rrule_as_text_without_expanding`.

**FUERA DE ALCANCE (no incluir en las tareas):**

- Aceptar hora local (`dtstart_local`) en la entrada REST: el instante en ms se mantiene
  como única fuente de verdad.
- Expandir `RRULE`.
- Preservar en el `.ics` recompuesto las propiedades no modeladas (asistentes, `VALARM`,
  propiedades `X-`) al hacer `PATCH` REST — **limitación preexistente** que se documentará
  pero no se arregla aquí.

## Motivación

- Hoy `calendar_objects` guarda `dtstart`/`dtend` como instante Unix en **milisegundos
  UTC** y el evento **no recuerda** qué hora local era. Un evento a las 10:00 en
  `Europe/Madrid` se almacena sin zona, así que una recurrencia no puede repetirse en
  hora local y **deriva una hora** en los cambios de horario de verano.
- El parseo DAV **consagra el bug**: `parse_datetime_ms` recibe solo el valor de la
  propiedad (no sus parámetros), de modo que `DTSTART;TZID=Europe/Madrid:20240315T100000`
  se lee como si fuera UTC. En `Europe/Madrid` (CET, UTC+1) el instante real es
  `1_710_493_200_000` (= 09:00 UTC), pero el test `keeps_rrule_as_text_without_expanding`
  afirma `1_710_496_800_000` (= 10:00 UTC): **exactamente una hora** de desfase.
- Para que la hora local sea inequívoca hace falta materializarla en el protocolo:
  el `.ics` debe llevar `DTSTART;TZID=…` **y** el bloque `VTIMEZONE` que define esa zona.

## Decisiones vinculantes

1. **El instante en ms sigue siendo la única fuente de verdad.** REST mantiene
   `dtstart`/`dtend` como `i64` Unix **ms** (UTC); el `tzid` es un **ancla** adicional
   opcional que solo determina cómo se compone el `.ics` y en qué hora local se repite la
   recurrencia. No se introduce una segunda representación temporal paralela.
2. **El bloque `VTIMEZONE` es obligatorio cuando hay `TZID`.** No basta el parámetro
   `TZID=`: RFC 5545 §3.2.19 exige que el `.ics` incluya la definición de la zona. Se
   deriva de la tzdb incrustada en `chrono-tz`.
3. **`chrono-tz` como dependencia directa, Rust puro, sin OpenSSL.** Es Rust puro con la
   tzdb IANA incrustada; respeta la restricción vinculante del repo (binario musl estático
   sin OpenSSL). **No** se activa la feature `chrono-tz` de `icalendar` (el parseo de
   fechas es manual).
4. **Retrocompatibilidad byte a byte sin `tzid`.** Un evento sin `tzid` debe producir la
   **misma salida `build_ics` que hoy** (`…Z`, sin `VTIMEZONE`). Las columnas nuevas son
   nullable → **ninguna fila existente cambia**; `NULL` = comportamiento actual.
5. **El `TZID` del bloque debe coincidir exactamente con el `TZID=` de `DTSTART`.** El
   nombre IANA introducido por el cliente se usa literalmente en ambos sitios.
6. **Validación estricta del `tzid`.** Un `tzid` que no sea un nombre IANA conocido →
   `ApiError::Validation` (**400**). Sin ambigüedad.
7. **Clasificación de observancias robusta.** `span.dst_offset != 0` → `BEGIN:DAYLIGHT`,
   si no `BEGIN:STANDARD`. Así se clasifica correctamente el DST negativo de
   `Europe/Dublin`.
8. **El `raw_payload` se preserva verbatim.** El `.ics` recibido por DAV se guarda tal
   cual (`raw.to_string()`); la composición recomposed solo aplica a `build_ics` de REST.
9. **Migración aditiva** (`ALTER TABLE … ADD COLUMN`), sin reescritura de filas.

## Prerequisitos (fuera del código)

- Rama `feature/timezone-support` creada desde `development` (Git Flow del repo).
- Commits con **Conventional Commits + gitmoji** (p. ej.
  `✨ feat: soporte de zonas horarias TZID`).
- Añadir `chrono-tz = "0.10"` a `Cargo.toml` **y refrescar `Cargo.lock`** (`cargo check`)
  porque la CI usa `cargo test --locked`; un lock desactualizado rompe la CI.
  La 0.10.4 ya está en la caché local del registry; **no** está todavía en `Cargo.lock`.
- `chrono` 0.4.45 **ya está** en el árbol de dependencias vía `icalendar`
  (verificado con `cargo tree -i chrono`), así que no se añade un stack de fechas nuevo.
- Medir el tamaño de la imagen con **`podman`** antes/después (hay que construir
  `web/dist` **antes** de `cargo build`).

## Contratos / Interfaces

### Esquema — `migrations/0004_timezone.sql`

```sql
ALTER TABLE calendar_objects ADD COLUMN dtstart_tzid TEXT;
ALTER TABLE calendar_objects ADD COLUMN dtend_tzid TEXT;
```

Ambas nullable → ninguna fila existente cambia. `NULL` = comportamiento actual (`…Z`).

### `src/db/models.rs`

Añadir `dtstart_tzid: Option<String>` y `dtend_tzid: Option<String>` a `CalendarObject`,
`NewCalendarObject` y `CalendarObjectUpdate` (todos con `dtstart: Option<i64>`,
`dtend: Option<i64>`).

### `src/db/mod.rs`

Incluir las dos columnas nuevas en INSERT/SELECT/UPDATE y en el row mapper.

### `src/dav/vtimezone.rs` (fichero NUEVO) — derivación y renderizado del `VTIMEZONE`

API disponible de `chrono-tz` 0.10.4 (verificada leyendo el fuente):

- `chrono_tz::{Tz, TZ_VARIANTS}`, `Tz: FromStr` (parseo del nombre IANA).
- `chrono_tz::TimeSpans::timespans(&Tz) -> FixedTimespanSet`.
- `FixedTimespanSet { pub first: FixedTimespan, pub rest: &'static [(i64, FixedTimespan)] }`
  — `rest` es la lista **ordenada** de `(epoch_segundos_UTC_de_la_transicion, FixedTimespan)`.
- `FixedTimespan { pub utc_offset: i32, pub dst_offset: i32, pub name: Option<&'static str> }`
  (segundos).
- `chrono_tz::GapInfo`, `chrono::LocalResult::{Single, Ambiguous, None}`,
  `TimeZone::from_local_datetime`.
- `chrono_tz::OffsetName::{tz_id, abbreviation}`.

Algoritmo (sin búsqueda heurística: las transiciones son datos):

- Para cada entrada `(t, span)` de `rest` **dentro de la ventana elegida**, con `prev` =
  la entrada anterior (`first` si es la primera):
  - `TZOFFSETFROM` = `prev.utc_offset + prev.dst_offset`
  - `TZOFFSETTO` = `span.utc_offset + span.dst_offset`
  - `DTSTART` = `t` expresado en la hora local del offset **anterior** (RFC 5545 §3.6.5:
    la onset se especifica en la hora local del `TZOFFSETFROM`), es decir
    `t + prev.utc_offset + prev.dst_offset`, formateado `YYYYMMDDTHHMMSS` **sin** `Z`.
  - `TZNAME` = `span.name` si existe (es opcional en el RFC).
  - Clasificación: `span.dst_offset != 0` → `BEGIN:DAYLIGHT`, si no `BEGIN:STANDARD`
    (regla robusta frente al DST negativo de `Europe/Dublin`).
- Formato de offset: `±HHMM` / `±HHMMSS`.
- Ventana: acotar por rango de `t` alrededor del evento (política propuesta: año del evento
  −1 … +5) para no emitir cientos de observancias; **la limitación de la ventana finita se
  declara explícitamente** (ver Riesgos).
- El `TZID` del bloque debe coincidir exactamente con el `TZID=` de `DTSTART`.

### `src/dav/ics.rs`

- Hacer llegar el parámetro `TZID` hasta el parseo de fechas (cambio de firma de
  `parse_datetime_ms` / del bucle de propiedades). Hoy `parse_datetime_ms` recibe solo el
  **valor** de la propiedad, no sus parámetros.
- Regla de resolución local→UTC, **documentada en el código**:
  - `LocalResult::Single(dt)` → ese instante.
  - `LocalResult::Ambiguous(a, _)` → el **primero** (offset previo a la transición,
    §3.3.5).
  - `LocalResult::None` (hora inexistente en el salto de primavera) → desplazar hacia
    delante con el offset previo al hueco (usando `GapInfo`).
- `build_ics()` (hoy en `src/dav/ics.rs:206`, emite `DTSTART:`/`DTEND:` en UTC) compone
  `DTSTART;TZID=<tz>:<hora local>` / `DTEND;TZID=<tz>:<hora local>` y añade el bloque
  `VTIMEZONE` cuando el evento tiene tzid. Sin tzid, la salida debe ser **byte-idéntica**
  a la actual.
- `EventFields` gana `dtstart_tzid`/`dtend_tzid`.
- `format_datetime_ms()` (`src/dav/ics.rs:243`) se mantiene para el camino sin tzid; se
  añade un formateador de hora local sin sufijo para el camino con tzid.
- Nota en el código: `src/dav/ics.rs:20-22` documenta la limitación de F07 ("`DATE-TIME`
  sin sufijo se trata como UTC; la resolución de `TZID`/`VTIMEZONE` está fuera de alcance
  de F07"); **esta feature levanta esa limitación** y el comentario debe actualizarse.

### `src/dav/caldav.rs`

`put_event()` (~línea 500), que hoy hace `parse_ics(raw)` y persiste con
`NewCalendarObject`/`CalendarObjectUpdate`, pasa el tzid parseado a esos structs.

### Contrato REST (no rompe compatibilidad)

- `dtstart`/`dtend` siguen siendo Unix **ms** (instante UTC), sin cambios.
- Nuevo campo **opcional** `tzid` (`Option<String>`, nombre IANA) en
  `CreateEventRequest` y `UpdateEventRequest`. Con `tzid` presente, el evento queda
  anclado a esa zona: el `.ics` usa `DTSTART;TZID=…` + `VTIMEZONE` y la recurrencia se
  ancla en hora local.
- `EventDto` gana `dtstart_tzid`/`dtend_tzid` (`Option<String>`).
- Validación: `tzid` que no sea un nombre IANA conocido → `ApiError::Validation` (**400**).
- `src/api/events.rs`: nueva fn `validate_tzid`; `create()` (línea 163) y `update()`
  (línea 247), que hoy llaman a `build_ics(&EventFields{…})`, la aplican y propagan el
  tzid.

### SPA (`web/`)

Al crear/editar, enviar además
`tzid: Intl.DateTimeFormat().resolvedOptions().timeZone`; mostrar la zona. Mantener el
instante en ms que ya manda (el SPA manda `dtstart`/`dtend` como ms desde
`Date.getTime()`).

## Dependencias

- **Producción**: `chrono-tz = "0.10"` (Rust puro, tzdb IANA incrustada, sin OpenSSL;
  respeta la restricción vinculante del binario musl estático).
- `chrono` 0.4.45 ya presente vía `icalendar`; no se añade stack nuevo.
- **No** se activa la feature `chrono-tz` de `icalendar` (el parseo de fechas es manual).
- Hay que refrescar `Cargo.lock` (`cargo check`) y commitearlo; la CI usa `--locked`.

## Testing (red-green-refactor)

- **unit (parseo DAV):** `DTSTART;TZID=Europe/Madrid:20240315T100000` →
  `1_710_493_200_000` (= 09:00 UTC). **Corregir** la aserción errónea de
  `keeps_rrule_as_text_without_expanding` (`src/dav/ics.rs:427`), que hoy afirma
  `1_710_496_800_000` (= 10:00 UTC) y **consagra el bug**.
- **unit (huecos/ambigüedad DST):** hora inexistente por hueco de primavera
  (`Europe/Madrid` 2024-03-31T02:30) y hora ambigua (`Europe/Madrid` 2024-10-27T02:30) →
  aplicar y verificar la regla documentada (`None` → desplazar hacia delante con el offset
  previo; `Ambiguous` → el primero).
- **unit (`VTIMEZONE` de `Europe/Madrid`):** 2 observancias por año en la ventana,
  `TZOFFSETFROM`/`TZOFFSETTO` `+0100`/`+0200`, `DTSTART` **sin** `Z`.
- **unit (clasificación por zona):** `Europe/Dublin` (DST negativo) se clasifica
  correctamente; `Asia/Tokyo` (sin DST) → una sola `STANDARD`; `UTC` → caso límite.
- **sanity (muy recomendado):** renderizar el `VTIMEZONE` de **todos** los `TZ_VARIANTS`
  y comprobar que se vuelve a parsear.
- **regresión:** evento **sin** tzid → salida de `build_ics` **byte-idéntica** a la actual.
- **unit (validación):** `tzid` inválido (p. ej. `Mars/Olympus`) → **400**.
- **integración REST:** `POST` con tzid → `GET` devuelve `dtstart_tzid`; la ventana de
  consulta coloca el evento en el instante correcto.
- **integración DAV:** `PUT` DAV con `TZID` → el instante almacenado es correcto y el
  `GET` REST lo refleja; y round-trip DAV→REST preserva el tzid.

## Tareas (TDD)

### Fase 1 — Dependencia y esquema

- [ ] **Paso 1 (RED):** Añadir `chrono-tz = "0.10"` a `Cargo.toml`, refrescar
      `Cargo.lock` con `cargo check` y escribir el test de ida y vuelta de
      `Tz: FromStr` (`"Europe/Madrid".parse::<Tz>()` → `Ok`; `"Mars/Olympus"` → `Err`).
      `cargo test --locked`
- [ ] **Paso 2 (RED):** Crear `migrations/0004_timezone.sql` con las dos columnas
      nullable y añadir `dtstart_tzid`/`dtend_tzid` a `CalendarObject`,
      `NewCalendarObject` y `CalendarObjectUpdate`; incluir las columnas en INSERT/SELECT/
      UPDATE y en el row mapper de `src/db/mod.rs`. Test de persistencia: un evento sin
      tzid guarda `NULL` y se lee `None`.
- [ ] **Paso 3 (GREEN):** Pasar los tests de esquema a verde. `cargo test --locked`.

### Fase 2 — Módulo `vtimezone`

- [ ] **Paso 4 (RED):** Test de render del `VTIMEZONE` de `Europe/Madrid` → 2 observancias
      por año en la ventana, `TZOFFSETFROM`/`TZOFFSETTO` `+0100`/`+0200`, `DTSTART`
      **sin** `Z`.
- [ ] **Paso 5 (RED):** Test de clasificación: `Europe/Dublin` (DST negativo) y
      `Asia/Tokyo` (sin DST → una `STANDARD`), `UTC` (caso límite).
- [ ] **Paso 6 (RED):** Test sanity: renderizar el `VTIMEZONE` de **todos** los
      `TZ_VARIANTS` y comprobar que se vuelve a parsear.
- [ ] **Paso 7 (GREEN):** Crear `src/dav/vtimezone.rs` con el algoritmo de derivación de
      observancias a partir de `chrono_tz::TimeSpans::timespans`, el formateo `±HHMM`/
      `±HHMMSS`, la clasificación `DAYLIGHT`/`STANDARD` y la ventana (año del evento −1 …
      +5). `cargo test --locked`.

### Fase 3 — Parseo y composición en `src/dav/ics.rs`

- [ ] **Paso 8 (RED):** Test que hoy falla (y que **corrige** el bug): parse
      `DTSTART;TZID=Europe/Madrid:20240315T100000` → `1_710_493_200_000`; **corregir** la
      aserción de `keeps_rrule_as_text_without_expanding`.
- [ ] **Paso 9 (RED):** Tests de `LocalResult`: hora inexistente (2024-03-31T02:30) y hora
      ambigua (2024-10-27T02:30) → regla documentada.
- [ ] **Paso 10 (RED):** Test de regresión: evento **sin** tzid → salida de `build_ics`
      **byte-idéntica** a la actual.
- [ ] **Paso 11 (RED):** Test de composición con tzid: `DTSTART;TZID=<tz>:<hora local>` +
      bloque `VTIMEZONE` con `TZID` idéntico al del parámetro.
- [ ] **Paso 12 (GREEN):** Cambiar la firma de `parse_datetime_ms` (y del bucle de
      propiedades) para hacer llegar el parámetro `TZID`; implementar la resolución
      local→UTC (`Single` / `Ambiguous` → el primero / `None` → desplazar hacia delante
      con `GapInfo`). Añadir `dtstart_tzid`/`dtend_tzid` a `EventFields`. `build_ics`
      compone con tzid + `VTIMEZONE` y sin tzid mantiene la salida actual byte a byte.
      Actualizar el comentario de limitación de F07 (`src/dav/ics.rs:20-22`).
      `cargo test --locked`.

### Fase 4 — DAV (`put_event`)

- [ ] **Paso 13 (RED):** Test de integración DAV: `PUT` con `TZID` → el instante
      almacenado es correcto (`Europe/Madrid` 10:00 → 09:00 UTC) y el round-trip DAV→REST
      preserva el tzid.
- [ ] **Paso 14 (GREEN):** `put_event()` en `src/dav/caldav.rs` (~línea 500) pasa el tzid
      parseado a `NewCalendarObject`/`CalendarObjectUpdate`. `cargo test --locked`.

### Fase 5 — API REST

- [ ] **Paso 15 (RED):** Test de validación: `POST`/`PATCH` con `tzid="Mars/Olympus"` →
      **400**.
- [ ] **Paso 16 (RED):** Test de integración: `POST` con tzid → `GET` devuelve
      `dtstart_tzid`; `dtstart`/`dtend` siguen siendo ms UTC sin cambios.
- [ ] **Paso 17 (GREEN):** Añadir `tzid: Option<String>` a `CreateEventRequest`/
      `UpdateEventRequest` y `dtstart_tzid`/`dtend_tzid` a `EventDto` en `src/api/dto.rs`;
      implementar `validate_tzid` y aplicar/propagar el tzid en `create()` (línea 163) y
      `update()` (línea 247) de `src/api/events.rs`. `cargo test --locked`.

### Fase 6 — SPA

- [ ] **Paso 18 (RED):** Test (vitest) de que al crear/editar se envía
      `tzid: Intl.DateTimeFormat().resolvedOptions().timeZone` manteniendo `dtstart`/`dtend`
      en ms; y de que se muestra la zona.
- [ ] **Paso 19 (GREEN):** Implementar en `web/src/**` el envío del `tzid` y la
      visualización de la zona. `npm run test`.

### Fase 7 — Documentación

- [ ] **Paso 20:** Crear `docs/F18-timezone-support.md`: contrato (`tzid`, `VTIMEZONE`,
      ventana de observancias), regla local→UTC y limitación de la ventana.
- [ ] **Paso 21:** Actualizar la limitación en `docs/F07-*.md` (la resolución de
      `TZID`/`VTIMEZONE` deja de estar fuera de alcance).
- [ ] **Paso 22:** Actualizar `README.md` y `AGENTS.md` (dependencia `chrono-tz` + regla de
      tamaño de imagen).

### Fase 8 — Verificación

- [ ] **Paso 23 (REFACTOR):** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
      sin warnings, `cargo test --locked` en verde.
- [ ] **Paso 24 (REFACTOR):** Desde `web/`: `npm run test`, `npm run typecheck`,
      `npm run lint`, `npm run build` en verde.
- [ ] **Paso 25:** Compilar el frontend **antes** del backend (`web/dist` → `cargo build`,
      `rust-embed` lee los assets en tiempo de compilación). Construir la imagen con
      **podman** + smoke (`GET /healthz` 200, SPA en `/`, 404 JSON en `/api/v1/nope`, 401 en
      `PROPFIND /dav/`) y **medir el tamaño** vs. la imagen actual (referencia ~25 MB en
      `AGENTS.md`).

## Criterios de aceptación

- Un evento con `tzid` se almacena con su zona (columnas `dtstart_tzid`/`dtend_tzid`) y su
  `.ics` compuesto lleva `DTSTART;TZID=<tz>` + un bloque `VTIMEZONE` válido cuyo `TZID`
  coincide exactamente con el del parámetro.
- Un evento **sin** tzid produce una salida de `build_ics` **byte-idéntica** a la actual y
  sus filas nuevas quedan `NULL` (retrocompatibilidad total).
- El **bug de la hora** está corregido: `DTSTART;TZID=Europe/Madrid:20240315T100000` →
  `1_710_493_200_000`, y la aserción errónea de `keeps_rrule_as_text_without_expanding` se
  ha corregido.
- El `VTIMEZONE` derivado es válido y **re-parseable** (sanity sobre todos los
  `TZ_VARIANTS`).
- Un `tzid` no IANA → **400** (`ApiError::Validation`).
- `EventDto` expone `dtstart_tzid`/`dtend_tzid`; `dtstart`/`dtend` siguen siendo ms UTC
  (contrato REST sin cambios incompatibles).
- El SPA envía `tzid = Intl.DateTimeFormat().resolvedOptions().timeZone` al crear/editar.
- `docs/F18-timezone-support.md` creado, `docs/F07-*.md` actualizado, `README.md` y
  `AGENTS.md` reflejan la dependencia y la regla de tamaño.
- **Tamaño de la imagen medido** con `podman` antes/después y documentado.
- CI en verde: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test --locked`; `npm run test`, `npm run typecheck`, `npm run lint`,
  `npm run build`.

## Riesgos

- **Tamaño del binario.** La tzdb se incrusta (el fuente de `chrono-tz` son ~8 MB, 7 MB de
  ellos tablas generadas). Mitigación: medir la imagen con **`podman`** antes/después;
  referencia ~25 MB en `AGENTS.md`; si sube mucho, revisar.
- **`--locked`.** Hay que refrescar `Cargo.lock` (`cargo check`) y commitearlo, o la CI
  falla.
- **Ventana finita de observancias del `VTIMEZONE`.** La política (año del evento −1 … +5)
  acota los datos pero deja fuera observancias más allá de la ventana; **documentar la
  limitación** en `docs/F18-timezone-support.md`.
- **Regresión en el camino sin tzid.** Tocar `parse_datetime_ms`/`build_ics` puede alterar
  la salida actual. Mitigación: test de regresión byte a byte.
- **`TZID` no IANA.** Un nombre desconocido debe rechazarse con 400, no colarse hasta la
  composición.
- **`PATCH` REST sigue recomponiendo el `.ics`** y descartando propiedades no modeladas
  (**limitación preexistente, fuera de alcance**); no se arregla aquí, solo se documenta.

## Referencias

- RFC 5545 §3.2.19 (`VTIMEZONE`), §3.3.5 (hora local ambigua), §3.6.5 (onsets del
  `VTIMEZONE` en la hora local del `TZOFFSETFROM`).
- `src/dav/ics.rs` — `parse_datetime_ms`, `format_datetime_ms()` (línea 243), `build_ics()`
  (línea 206), comentario de limitación F07 (líneas 20-22), test
  `keeps_rrule_as_text_without_expanding` (línea 427).
- `src/dav/caldav.rs` — `put_event()` (~línea 500).
- `src/api/events.rs` — `create()` (línea 163), `update()` (línea 247).
- `src/db/models.rs` — `CalendarObject`, `NewCalendarObject`, `CalendarObjectUpdate`.
- `migrations/0001_init.sql` — `calendar_objects` (`dtstart`/`dtend` ms UTC).
- `plans/PLAN-011-search-api-COMPLETED.md` — estilo de las fases TDD.
- `plans/PLAN-018-oidc-valet-parity-COMPLETED.md` — estructura y estilo del plan.
- `AGENTS.md` — restricción de binario musl estático sin OpenSSL, regla de tamaño de
  imagen y flujo OpenSpec/TDD.

## Cierre — resultado, desviaciones y hallazgos

Registro de lo que se construyó, de lo que se apartó del enunciado original y de lo que
apareció por el camino, para que el documento no mienta sobre lo entregado.

### Resultado

- **Backend**: **465 tests en verde** (antes de la feature: 432), `cargo fmt --check`
  limpio y `cargo clippy --all-targets -- -D warnings` con **0 warnings**.
- **Frontend**: **85 tests** en verde, `tsc -b` / `eslint` / `vite build` limpios.
- **Imagen**: **26,6 MB** (26.621.836 bytes) frente a los ~25 MB de referencia
  premusurables. El incremento (~1,6 MB) es la tzdb IANA embebida por `chrono-tz`; queda
  muy por debajo del peor caso estimado (~7 MB), que correspondía al tamaño de *fuente*
  del crate, no a los datos compilados.
- **Smoke de la imagen con `podman`**: `GET /healthz` → 200; SPA en `/` → 200 con
  `<title>OxiDav</title>`; `/api/v1/nope` → **404 `application/json`**; `PROPFIND /dav/` →
  **401**; proceso como **uid 10001**; binario *stripped* de 17,2 MB.

### Desviaciones respecto al plan original

1. Se añadió `chrono-tz = "0.10"` **y `chrono` como dependencia directa**
   (`default-features = false`, `features = ["std"]`), aunque el plan no preveía lo
   segundo.
2. El `VTIMEZONE` **no** se deriva de las tablas de transiciones de `chrono-tz`: en 0.10.4
   `TimeSpans`/`FixedTimespanSet` **no son API pública** (el módulo `timezone_impl` es
   privado). Se reconstruyen las transiciones **conductualmente** (muestreo cada 7 días +
   búsqueda binaria). El plan daba por sentado que esa API era alcanzable: era un **error
   de análisis** del propio plan.
3. Se **omiten** las observancias cuya transición **no cambia el offset total** (solo
   reorganiza el desglose base/DST): no hay cambio de reloj de pared y una observancia con
   `TZOFFSETFROM == TZOFFSETTO` la rechazan algunos clientes.
4. La API REST **acota el rango de instantes** a los años 1..9999 (**400** fuera de rango),
   cosa que el plan no contemplaba.
5. Se **endurecieron los caminos de composición** para que no puedan entrar en pánico con
   milisegundos arbitrarios.
6. El SPA **no re-ancla al editar**: conserva el ancla existente del evento (y la omite si
   no la tenía). El plan decía «enviar la zona del navegador al crear **y** al editar»; se
   cambió porque, al ser `dtstart` un instante fijo, re-anclar cambiaría en silencio la
   hora local de la recurrencia. Se añadió además un `format` explícito al `RangePicker`
   para no depender del formato por defecto del locale.
7. El REST aplica **un único `tzid`** a `DTSTART` y `DTEND` (el DAV sí admite dos
   distintos).

### Hallazgos

- Un **bug latente preexistente** que el plan sí identificó y que quedó corregido:
  `parse_datetime_ms` trataba `DTSTART;TZID=…:<local>` como UTC (una hora de desfase en
  `Europe/Madrid`), y un test consagraba el error.
- Un **bug de borde** encontrado en la auditoría, **no previsto en el plan**: el bucle de
  muestreo nunca examinaba el tramo final de la ventana, así que se perdían las
  transiciones de los últimos <7 días (`Africa/Casablanca` y `Africa/El_Aaiun` emitían 14
  observancias en vez de 15). Corregido y con test de regresión.
- La **separación mínima real** entre dos transiciones consecutivas en la ventana moderna
  es de **9 días** (`America/Asuncion`), no «meses» como afirmaba el primer comentario; el
  margen del paso de 7 días es de **2 días**.
- El onset que se perdía corresponde a **`20291230`** en la tzdb que empaqueta `chrono-tz`
  0.10.4 (la fecha `20291231` que citaba el plan es la de algunas tzdata más nuevas).
- Se añadió cobertura de la **ruta de actualización** de la migración
  (`tests/timezone_migration.rs`: aplicar `0004` sobre una base ya poblada con `0001`–`0003`),
  verificando que las **FTS5 de contenido externo siguen funcionando** tras el
  `ALTER TABLE`.
- Nota de mantenimiento: `sqlx-sqlite` 0.8.6 cachea statements preparados por conexión sin
  invalidarlos ante un cambio de esquema; el `ALTER TABLE` de la migración solo corre en el
  arranque (antes de preparar ningún statement), así que producción está a salvo, pero
  cualquier DDL futuro sobre un pool vivo con statements ya cacheados contra esa tabla
  podría provocar un pánico en `sqlx`.
