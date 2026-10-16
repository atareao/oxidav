# PLAN-027 — El harness de tests no debe llenar el disco

**Estado:** completado. **Rama:** `fix/test-harness-dir`, desde `development@565d04e`.

## El incidente que lo motiva

Al verificar el post-merge de F23, **212 de 551 tests del backend fallaron** con `init_pool: SqliteError { code: 778, message: "disk I/O error" }`. No era una regresión del código: el juego de tests **había llenado el tmpfs de `/tmp`**. En `/tmp/opencode/oxidav-tests` —la carpeta donde los tests crean sus bases de datos— había **156 671 ficheros ocupando 24 G**, con el tmpfs al 80 %. Limpiándolos, el backend volvió a **551 / 0**.

Y engaña: se manifiesta como *«212 tests roto»* con el código perfecto. Un fallo que miente cuesta horas. **Eso es un defecto, no una anécdota.**

## Las tres causas (diagnosticadas, no supuestas)

1. **Ocho ficheros duplican el helper** `temp_db_url`, cada uno con su propia copia divergente: `tests/{api,dav,caldav,carddav,data_layer,app_tokens,app_tokens_migration,timezone_migration}.rs`. No hay `tests/common/`.
2. **El nombre lleva el PID** (`api-{tag}-{pid}-{n}.db`), así que **cada ejecución crea un juego nuevo de ficheros** que nadie elimina. El crecimiento es ilimitado por diseño.
3. **El `remove_file(&path)` deja los hermanos**: borra el `.db` pero **no** el `-wal` ni el `-shm`. De ahí los 122 420 `.db` acompañados de 17 191 `-wal` y 17 060 `-shm` huérfanos.
4. Y sobre todo: **la ruta es fija y está en un tmpfs respaldado por RAM** (`/tmp/opencode/oxidav-tests`). No se puede redirigir. Un tmpfs es justo el sitio donde menos debe vivir algo que crece sin límite.

## Objetivo

Un **único** helper compartido que (a) permita **elegir el directorio**, (b) **limpie lo suyo** siempre (los tres ficheros), y (c) **barra los restos de ejecuciones muertas**, de modo que la carpeta quede **acotada** en vez de crecer sin fin.

## Contratos

- **Nuevo `tests/common/mod.rs`** con el helper único. Firma:
  ```rust
  pub fn temp_db_url(prefix: &str, tag: &str) -> String
  ```
  Devuelve **exactamente** el mismo formato que hoy (`sqlite://<ruta absoluta>`) para no tocar ni una línea de los tests que lo usan más allá de la llamada.
- **Resolución del directorio, por este orden**:
  1. la variable de entorno **`OXIDAV_TEST_DIR`** si está definida y no vacía;
  2. si no, **`std::env::temp_dir()`** (que **respeta `TMPDIR`**);
  3. y dentro, el subdirectorio **`oxidav-tests`**.
  Se crea con `create_dir_all` y se informa del directorio elegido solo en caso de error.
- **Nombre del fichero**: mantener el prefijo que ya usa cada fichero de test (`api`, `dav`, `caldav`, `carddav`, …) para poder distinguirlos, e incluir el PID y un contador como hoy: `<prefix>-<tag>-<pid>-<n>.db`.
- **Limpieza de la familia completa**: antes de devolver la URL, eliminar **`<ruta>`, `<ruta>-wal` y `<ruta>-shm`** si existen.
- **Barrido de restos muertos**: al resolver el directorio, recorrerlo y eliminar los ficheros cuyo **PID ya no existe** (`/proc/<pid>` ausente; el PID se lee del nombre, contando **desde la derecha**: penúltimo campo separado por `-`). Añadir además un segundo criterio conservador: los que tengan **más de 24 horas**. El barrido **nunca** puede tocar ficheros de un PID **vivo** — el peligro real es borrarle el fichero a otro proceso de tests que esté corriendo (p. ej. con `cargo nextest`, que sí paraleliza binarios). Esto debe ser explícito en el código.
- **`#![allow(dead_code)]`** en `tests/common/mod.rs`: es un módulo compartido que cada binario de test incluye y cada uno usa un subconjunto; es lo idiomático y **no** enmascara nada. Nada de `#[allow(dead_code)]` en el resto.

## Fuera de alcance

- No se toca el código de producción ni `src/`.
- No se cambia qué prueba cada test, ni sus aserciones, ni las migraciones.
- No se añade ninguna dependencia.

## Escenarios (BDD)

1. **Se puede elegir el directorio**
   Given `OXIDAV_TEST_DIR=/ruta/alternativa`, When un test pide su URL, Then la base se crea **dentro de esa ruta** y `/tmp/opencode/oxidav-tests` **no** se toca.
2. **Sin la variable, respeta `TMPDIR`**
   Given `OXIDAV_TEST_DIR` sin definir y `TMPDIR=/otra/ruta`, When se pide la URL, Then el directorio base es `/otra/ruta/oxidav-tests` (no una ruta fija en código).
3. **Limpia los tres ficheros**
   Given un `x.db` con su `x.db-wal` y su `x.db-shm` de una ejecución anterior **del mismo nombre**, When se pide la URL para ese nombre, Then los tres ya no están (y el `.db` se recreará limpio).
4. **Barrre los restos de una ejecución muerta**
   Given en el directorio ficheros cuyo PID no existe, When se resuelve el directorio, Then se eliminan **esos** y **no** los de un PID vivo.
5. **No crece sin fin**
   Given el juego completo ejecutado **dos veces** seguidas, Then el número de ficheros del directorio **no se multiplica**: la segunda pasada no deja más restos que la primera.
6. **Nada cambia para los tests**
   Given la suite completa, When se ejecuta, Then **551 pasando / 0 fallando**, igual que antes del cambio.

## Tareas (TDD)

1. **RED** — nuevo binario de test **`tests/test_harness.rs`**, que prueba el helper **directamente** (es el sujeto bajo prueba): escenarios 1, 3, 4 y 5, más el formato de la URL (`sqlite://` y ruta absoluta). Debe **fallar** antes de implementar (no existe `tests/common/mod.rs`).
   - Ojo con el entorno: los escenarios que dependen de variables de entorno son **globales al proceso** y los tests corren en paralelo dentro de un binario. Usar el directorio propio de cada test y **no** depender del orden; si una variable global hace imposibles dos escenarios a la vez, **dilo** y resuélvelo (p. ej. probando la función de resolución con un parámetro explícito y un test separado para la lectura de la variable).
2. **GREEN** — implementar `tests/common/mod.rs` y sustituir las **ocho** copias por `mod common;` + la llamada nueva, **sin cambiar ninguna aserción**.
3. **REFACTOR** — `cargo fmt --all`, `cargo clippy --all-targets --locked -- -D warnings` y `cargo test --locked --no-fail-fast` limpios.

## Verificación de cierre

1. **551 pasando / 0 fallando** y `fmt`/`clippy` sin un solo aviso.
2. **La prueba del incidente, en real**: ejecutar la suite con `OXIDAV_TEST_DIR` apuntando a un directorio **nuevo fuera del tmpfs** (p. ej. dentro de `target/`) y comprobar que (a) las bases aparecen **ahí**, (b) `/tmp/opencode/oxidav-tests` **no** crece, y (c) tras **dos ejecuciones** el recuento de ficheros de ese directorio **no se multiplica**. Reportar los recuentos crudos.
3. **Los restos muertos se barren**: crear a mano ficheros con un PID inexistente y comprobar que desaparecen tras ejecutar un test, y que los de un PID vivo **no**.
4. `git diff -M --stat`: 8 ficheros quitando su copia del helper y uno nuevo con el definitivo.

## Versionado

Es un `fix` (de harness, no de producto) → **patch** en `git-cliff`.

---

## Resultado

### Rojo → Verde

El rojo se hizo contra un **stub que reproduce el comportamiento actual** (ruta fija, `remove_file` solo del `.db`, barrido no-op), no contra un error de compilación — así los tests demuestran que **discriminan**, no solo que compilan:

- Rojo: `cargo test --locked --test test_harness` → **6 pasando / 6 fallando**.
- Verde: **12 pasando / 0 fallando**, y la suite completa **563 / 0** (las 551 de antes + los 12 nuevos).

`cargo fmt --all -- --check` limpio y `cargo clippy --all-targets --locked -- -D warnings` sin un solo aviso.

### Las tres causas, cerradas

1. **Ocho copias duplicadas → una sola.** `tests/common/mod.rs` (205 líneas) es ahora el único helper; los 8 ficheros de test quedan con `mod common;` y `common::temp_db_url("<prefijo>", tag)`. Verificado: `grep -rln 'fn temp_db_url(tag: &str)' tests/` → **0**; `grep -rn 'static .*COUNTER' tests/*.rs` → **0**; en cada fichero se borraron además `use std::path::PathBuf;` y `use std::sync::atomic::{AtomicU64, Ordering};`, que solo usaba el helper. **Ni una aserción cambió.**
2. **La familia completa se limpia**: `remove_db_family` borra `<ruta>`, `<ruta>-wal` y `<ruta>-shm` (antes quedaban 17 191 `-wal` y 17 060 `-shm` huérfanos).
3. **El directorio es redirigible y ya no crece sin fin**: `resolve_dir_from` es **pura** y resuelve en orden `explicit` → `OXIDAV_TEST_DIR` (si no está vacía) → `std::env::temp_dir()` (respeta `TMPDIR`) → `/oxidav-tests`; y `sweep_stale`, protegido por un `Once` por proceso, borra los restos de PIDs muertos (y los de PID vivo con más de 24 h, caso de PID reciclado) **sin tocar nunca** los de un PID vivo reciente.

### La prueba del incidente, medida

Con `OXIDAV_TEST_DIR` apuntando a un directorio nuevo fuera del directorio viejo:

| Prueba | Resultado medido |
|---|---|
| Suite con el directorio redirigido | **563 / 0** |
| Ficheros que deja en ese directorio | **3** (una sola tanda: los del último binario, porque cada binario barre los del anterior) |
| Directorio viejo `/tmp/opencode/oxidav-tests` durante esa ejecución | **intacto** (253 ficheros, ni uno tocado) |
| Segunda pasada sobre el mismo directorio | **sigue en 3** — **no se multiplica** |
| Familia de PID muerto inyectada a mano (`api-dead-999999-9.db` + `-wal` + `-shm`, con `/proc/999999` inexistente) | **los 3 barridos**, `removed == 3`, y los ficheros de PID vivo intactos |
| Ejecución de un solo binario (`--test api`, 65 tests) | deja **84** ficheros = una tanda, no un acumulado |

Funciona porque el barrido de PIDs muertos acota el directorio **por sí solo**: ningún binario ve los restos de otro.

### Cambio de comportamiento (a tener en cuenta, no es un defecto)

Al respetar `std::env::temp_dir()`, el directorio por defecto **se mueve**. En esta máquina `TMPDIR` **no está definida**, así que `temp_dir()` es `/tmp` y el nuevo directorio por defecto es **`/tmp/oxidav-tests`**, no el `/tmp/opencode/oxidav-tests` que estaba incrustado en el código.

Consecuencia: el directorio viejo queda **huérfano** — nadie volverá a escribir en él. Tenía **253 ficheros / 55 M** de puros restos del incidente, que se han eliminado a mano (está fuera del repositorio, no es un cambio de código).

El nuevo directorio sigue sobre `tmpfs` (91 ficheros / 23 M medidos, acotado), pero ya **no importa**: con `OXIDAV_TEST_DIR` o `TMPDIR` se manda a donde se quiera, y el propio barrido impide que se repita el 156 671 / 24 G. Esa es la lección del incidente: **un tmpfs es el peor sitio para algo que crece sin límite, y ahora no crece.**

### Desviaciones

1. **El escenario 9 se reformuló.** Tal como estaba redactado ("crea un fichero de PID vivo y comprueba que sigue ahí") **no podía discriminar**: un barrido no-op nunca borra nada, así que la aserción se cumplía trivialmente. Se reescribió como un solo test con el muerto y el vivo en el **mismo** directorio: muerto borrado, vivo intacto, `removed == 3`. Así cae en rojo con el stub y discrimina en verde.
2. **`pick_dead_pid` se reescribió** para satisfacer `clippy::explicit_counter_loop` (que hacía fallar `-D warnings`). Era un defecto del fichero escrito en la fase roja, ajeno a los 12 tests, y se corrigió preservando el comportamiento.
3. **`#![allow(dead_code)]` en `tests/common/mod.rs`**: deliberado e idiomático — es un módulo compartido que cada binario de test incluye y usa un subconjunto. No enmascara nada.

### Ficheros

- `tests/common/mod.rs` (nuevo, 205 líneas) — el harness único.
- `tests/test_harness.rs` (nuevo) — 12 tests del propio harness.
- `tests/{api,dav,caldav,carddav,data_layer,app_tokens,app_tokens_migration,timezone_migration}.rs` — pierden su copia del helper y el `static COUNTER`.
- **Cero cambios en `src/`.** Cero dependencias nuevas.

---
