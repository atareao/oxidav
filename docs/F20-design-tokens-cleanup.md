# F20 — Limpieza de tokens de diseño

> El plan de origen es [`plans/PLAN-021-design-tokens-cleanup-COMPLETED.md`](../plans/PLAN-021-design-tokens-cleanup-COMPLETED.md).
> Es una feature **exclusivamente de frontend**: no toca el backend, ni el
> esquema, ni las rutas DAV/REST, ni el contrato de datos.

## Qué implementa

F20 deja **una sola fuente de verdad para el diseño**: `theme.token` /
`theme.components` de Ant Design, declarados en `web/src/AuthenticatedApp.tsx`.

El punto de partida era el contrario: `web/src/index.css` había crecido hasta ser
una **fachada que fingía ser un segundo sistema de diseño**. Tenía su propia
escala de espaciado, sus propios radios y su propia pila de fuentes, en paralelo
al tema de Ant Design que ya usaba la aplicación. La mayor parte de esa fachada
era, sin embargo, **código muerto**.

La decisión de estilo no cambia respecto a F19: «**pulido sin marca nueva**».
`colorPrimary` sigue en el valor por defecto de Ant Design y no se introduce
ninguna paleta propia.

## El diagnóstico (medido, no supuesto)

Antes de tocar nada se inventarió `web/src/index.css` y se cruzó con el resto del
código. Lo que apareció:

- `web/src/index.css` declaraba **14** variables CSS. De ellas, **10 no las
  consumía nadie** y las **4** restantes lo eran **solo dentro del propio
  `index.css`**. `grep "var(--"` sobre todo `web/src` devolvía 4 coincidencias y
  las 4 estaban dentro de `index.css`: **ningún** fichero `.ts`/`.tsx` consumía
  ninguna variable CSS.
- La **pila de fuentes** estaba duplicada literalmente entre `index.css`
  (`--font-sans`) y `AuthenticatedApp.tsx` (`DESIGN_TOKENS.fontFamily`). Dos
  declaraciones de lo mismo, en dos ficheros distintos.
- La escala `--space-*` **calcaba** los tokens `size*` de Ant Design:
  `sizeUnit: 4, sizeStep: 4` produce `sizeXXS=4 … sizeXXL=48`, exactamente el
  rango que replicaba el CSS. Era una segunda copia de una escala que el tema ya
  posee.
- Con `borderRadius: 10`, Ant Design deriva `SM=6`, `LG=12` y `XS=2`
  (`genRadius.js`). La `--radius-lg: 14` del CSS **ya discrepaba** del valor que
  AntD derivaría del tema vivo: la fachada no solo duplicaba, además mentía.
- Ant Design v6 **ya emite sus tokens como variables `--ant-*` en runtime por
  defecto**. Verificado en `@ant-design/cssinjs@2.1.2` y `antd@6.6.6`. No
  aparecen en el CSS estático porque la inyección es en tiempo de ejecución.
- **Un cuarto hallazgo, ya en auditoría:** la clase `.page` era **código
  muerto** —ningún elemento del SPA la aplicaba; el padding real lo aporta un
  `padding` inline en `web/src/components/AppLayout.tsx`—. Venía arrastrada de
  F19 y sostenía artificialmente la variable `--page-padding`.

En resumen: el CSS no era un sistema de diseño paralelo funcional, sino una
capa mayoritariamente inerte que invitaba a crear una segunda fuente de verdad.

## Qué cambia

Cinco ficheros, todos de frontend:

1. **`web/src/index.css`** — pasa de **14 variables a 2** (`--font-sans`,
   `--line-height-base`). Se eliminan las 10 muertas (`--space-1/-2/-3/-6/-7`,
   `--radius-sm/-md/-lg/-pill`, `--font-mono`) y, además, la utilidad muerta
   `.page` junto con `--page-padding` y el `@media (min-width: 768px)` que solo
   existía para ella. Se mantiene `color-scheme: light` por defecto (sigue siendo
   necesario: la pantalla de login se renderiza fuera del `ThemeProvider`). El
   **comentario de cabecera** declara explícitamente que el sistema de diseño vivo
   es el tema de Ant Design y que este fichero solo contiene lo que AntD no
   expresa: reset, `color-scheme`, utilidades y las variables que el propio CSS
   consume.
2. **`web/src/AuthenticatedApp.tsx`** — `DESIGN_TOKENS.fontFamily` pasa de la
   pila literal a `'var(--font-sans)'`. Nada más cambia; `colorPrimary` sigue en
   el valor por defecto.
3. **`web/src/pages/pageLayout.ts`** — **solo el comentario**. Se aclara que es
   **layout de página** (ritmo vertical y anchos fluidos), **no** tema, y que no
   duplica tokens (`PAGE_GAP` es el nombre semántico del tamaño `'middle'` de
   AntD). Sin cambios funcionales.
4. **`web/tsconfig.app.json`** — se añade `"node"` a `types` (ver Desviaciones).
5. **Tests** — nuevo `web/src/design-tokens.test.ts` (invariantes) y una aserción
   reforzada en `web/src/AuthenticatedApp.test.tsx` (el token expuesto por
   `ConfigProvider` debe tener `fontFamily === 'var(--font-sans)'`).

## El invariante nuevo

El cambio no es solo borrar deuda: fija la regla que impide que vuelva.

> **Ninguna custom property declarada en `index.css` sin consumidor real**, y **la
> pila de fuentes en un solo fichero.**

El test que lo fija, `web/src/design-tokens.test.ts`, tiene tres matices que
importan:

- **Escanea el código de producción `.ts`/`.tsx`/`.css`**, excluyendo los
  ficheros `.test.*`, para que la propia aserción no haga de «consumidor» y se
  auto-valide.
- **Ignora los comentarios.** Sin esto, un `var(--x)` mencionado en un comentario
  enmascararía la retirada del uso real: era un **falso positivo real** detectado
  en auditoría.
- **Acepta `var(--x)` con espacios o con _fallback_** (`var(--x, …)`), para no
  dar por huérfana una variable que sí se usa con valor de reserva.

## Hallazgos de la auditoría y su resolución

| Hallazgo | Severidad | Resolución |
| --- | --- | --- |
| `compose.yaml` modificado fuera del alcance de la feature | — | **Dejado sin commitear a propósito** (decisión del usuario); no entra en el PR |
| `.page` / `--page-padding` eran código muerto | alta (perpetuaba la fachada) | **Eliminados**; el plan se enmendó de 3 a 2 variables |
| El escaneo de consumidores contaba los comentarios | alta (falso positivo) | **Corregido**: el test ignora comentarios |
| `var(--x)` no cubría _fallback_ ni espacios | media | **Corregido** con la regex |
| `process.cwd()` como base de rutas del test | baja | **Aceptado y documentado** (bajo `jsdom`, `import.meta.url` no es una URL `file:`) |
| `types: ["node"]` en `tsconfig.app.json` | baja | **Vía mínima que funciona**; se evaluó un `tsconfig.test.json` separado (que además arreglaría la fuga preexistente de `vitest/globals`) y se descartó por superficie para esta feature |
| Coherencia de comentarios y acoplamiento del test al literal `system-ui` | baja | **Aceptados**, documentados |

## Desviaciones del plan

1. **De 3 variables a 2.** El plan preveía **colapsar** `--space-4` y `--space-5`
   en `--page-padding` y dejar **3** variables. La auditoría demostró que `.page`
   era código muerto —nadie la aplicaba—, así que se **eliminaron** ambas
   variables en vez de consolidarlas, y quedan **2**. El plan se enmendó en
   consecuencia.
2. **Se tocó la configuración de build.** El plan decía **no tocar la
   configuración de build**. Hubo que añadir `"node"` a `types` en
   `tsconfig.app.json` porque las pruebas leen los fuentes con `node:fs`. Los
   enfoques sin builtins de Node se descartaron **con evidencia**: `import.meta.url`
   no es una URL `file:` bajo el entorno `jsdom`, y los imports `?raw` de Vite
   devuelven **cadena vacía** bajo Vitest 5 + `jsdom` (lo que dejaba el test
   vacuo, demostrado con una prueba de mutación).

## Verificación

Tests del frontend: **116 en verde (13 ficheros)**, partiendo de una línea base de
**112 (12 ficheros)**.

```bash
# Frontend (desde web/)
npm run test
npm run typecheck
npm run lint
npm run build
```

Los cuatro comandos quedan limpios.

### Delta del bundle

| Métrica | Antes (F19) | Después | Delta |
| --- | --- | --- | --- |
| `web/dist` | 1 271 004 B | 1 270 610 B | **−394 B** |

El signo es el esperado: esta feature **borra, no añade**. La reducción es
pequeña porque el CSS eliminado era corto, pero la dirección confirma que no se
introdujo nada nuevo.

Comprobaciones adicionales sobre el **bundle construido**:

- La pila `system-ui` aparece **una sola vez** (en el CSS) y **ningún** JS la
  repite; el tema referencia `var(--font-sans)`.
- El CSS construido declara exactamente `--font-sans` y `--line-height-base`.
- **Pruebas de mutación**: inyectar `--zz-temp` en `:root` hace fallar el test de
  huérfanas, lo que demuestra que el test **no es vacuo**.
- **Smoke del binario de release** (`target/release/oxidav`, sin OIDC): `GET /healthz` → 200; la SPA en `/` → 200 con `lang="es"` y `<title>OxiDav</title>`; `GET /api/v1/nope` → 404 con el cuerpo JSON de error uniforme; `PROPFIND /dav/` → 401; y el **CSS embebido en el binario** (`/assets/index-DY2fJ0xB.css`) declara exactamente `--font-sans` y `--line-height-base`. El proceso paró de forma limpia.

## Relacionado

- [`plans/PLAN-021-design-tokens-cleanup-COMPLETED.md`](../plans/PLAN-021-design-tokens-cleanup-COMPLETED.md)
- [`docs/F19-frontend-redesign.md`](./F19-frontend-redesign.md) — el sistema de diseño que esta feature consolida
- [`AGENTS.md`](../AGENTS.md) — stack del frontend (React 19 + Ant Design v6 + Vite + TypeScript)
