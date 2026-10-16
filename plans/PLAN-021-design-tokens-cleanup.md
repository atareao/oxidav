# PLAN-021 — Limpieza de tokens de diseño (F20)

## Objetivo

Dejar **una sola fuente de verdad para el diseño**: `theme.token` de Ant Design en
`web/src/AuthenticatedApp.tsx`. Hoy `web/src/index.css` declara **14 variables CSS de
las que 10 no las consume nadie** y **ningún fichero `.ts`/`.tsx` consume ninguna
variable CSS**; esa fachada muerta invita a crear una segunda fuente de verdad. Además,
la pila de fuentes está duplicada literalmente entre `index.css` y `AuthenticatedApp.tsx`.

## Alcance

Esta feature (F20) es una **limpieza exclusivamente de frontend**. Consiste en
**retirar la fachada muerta de variables CSS** de `web/src/index.css`, dejar solo lo que
el propio CSS consume y hacer que el tema vivo (los tokens de Ant Design) referencia la
pila de fuentes en lugar de repetirla literalmente.

**FUERA DE ALCANCE (no incluir en las tareas):**

- **Backend, esquema, rutas DAV/REST y contrato de datos**: no se toca ni una línea.
- **CSS que Ant Design inyecta**: no se toca ni se «gestiona».
- **Marca nueva / `colorPrimary`**: no se cambia; se mantiene el por defecto de AntD
  («pulido sin marca nueva»).
- **CSS Modules, `:export`, inyección en runtime desde `main.tsx` o un módulo
  `theme/tokens.ts`**: era la opción «consolidación total», **descartada**.
- **«Cablear» los literales sueltos** (`padding: 24`, `gap: 8`, etc.) a variables CSS:
  era la opción «capa CSS viva», **descartada**.
- **`docs/F19-frontend-redesign.md`**: **sin cambios** (ver más abajo).

## Motivación

### Diagnóstico medido

Inventario de las **14 variables** de `web/src/index.css`:

| Variable | Valor | Consumidor | Estado |
|---|---|---|---|
| `--space-1` | 4px | nadie | MUERTA |
| `--space-2` | 8px | nadie | MUERTA |
| `--space-3` | 12px | nadie | MUERTA |
| `--space-4` | 16px | `.page` (index.css:87) | viva, interna |
| `--space-5` | 24px | `.page` @768 (index.css:92) | viva, interna |
| `--space-6` | 32px | nadie | MUERTA |
| `--space-7` | 48px | nadie | MUERTA |
| `--radius-sm` | 6px | nadie | MUERTA |
| `--radius-md` | 10px | nadie | MUERTA |
| `--radius-lg` | 14px | nadie | MUERTA |
| `--radius-pill` | 999px | nadie | MUERTA |
| `--font-sans` | pila system-ui… | `body` (index.css:60) | viva y DUPLICADA en TS |
| `--font-mono` | pila mono… | nadie | MUERTA |
| `--line-height-base` | 1.5 | `body` (index.css:61) | viva, interna |

Total: **10 muertas, 3 vivas internas, 1 viva y duplicada.**

### Hechos verificados (no suposiciones)

- `grep "var(--"` sobre todo `web/src` da **4 coincidencias y las 4 están DENTRO de
  `index.css`**. Ningún `.ts`/`.tsx` consume variables CSS.
- `antd@6.6.6` (`es/theme/themes/seed.js`): el `fontFamily` por defecto de Ant Design es
  **OTRA pila** (más larga, incluye `Noto Sans`), por lo que **no se puede omitir el token
  `fontFamily`** sin cambiar el render.
- `antd@6.6.6` deriva su escala de espaciado con `sizeUnit:4, sizeStep:4`
  (`genSizeMapToken.js`): `sizeXXS=4, sizeXS=8, sizeSM=12, size=16, sizeMD=20, sizeLG=24,
  sizeXL=32, sizeXXL=48`. La escala `--space-*` del CSS la **calca**.
- `antd@6.6.6` (`genRadius.js`): con `borderRadius:10` deriva `SM=6, LG=12, XS=2`; nuestra
  `--radius-lg:14` **NO coincide** con el 12 que derivaría AntD (la fachada **ya discrepa**
  del tema vivo).
- `@ant-design/cssinjs@2.1.2` (`lib/hooks/useCacheToken.js:115`) llama a `transformToken()`
  sin condición y antd v6 construye siempre el objeto `cssVar`
  (`antd/es/theme/useToken.js`): AntD **ya emite sus tokens como variables `--ant-*` en
  runtime por defecto**. (No aparecen en `web/dist/assets/index-*.css`, que solo contiene
  `index.css`, porque la inyección es en runtime.)

## Decisiones vinculantes

1. **`theme.token` de `AuthenticatedApp.tsx` es la única fuente de verdad del diseño.**
   `index.css` solo contiene lo que Ant Design no expresa: reset, `color-scheme`,
   utilidades del propio CSS y las variables que el propio CSS consume.
2. **Invariante nuevo: ninguna variable CSS declarada sin consumidor.** Toda custom
   property de `index.css` debe aparecer al menos una vez como `var(--x)`.
3. **Se eliminan las 10 variables muertas** y se colapsan `--space-4`/`--space-5` en una
   única variable semántica `--page-padding`.
4. **La pila de fuentes vive en un solo fichero.** `--font-sans` se mantiene como única
   declaración; el tema de AntD la **referencia** con `'var(--font-sans)'`.
5. **`color-scheme: light` por defecto se mantiene** (sigue siendo necesario: la pantalla
   de login se renderiza fuera del `ThemeProvider`).
6. **`colorPrimary` no se toca.** El tema mantiene el `colorPrimary` por defecto de AntD.
7. **`pageLayout.ts` no cambia funcionalmente.** Solo se completa su comentario para dejar
   claro que es **layout de página**, no tema de diseño.
8. **`docs/F19-frontend-redesign.md` no se toca.** (Verificado: la afirmación sobre los
   `marginBottom: 16` es correcta en su ámbito; el literal que sobrevive —`Sync.tsx:412`—
   está dentro de un `Modal`, fuera del `Flex`, y no es un bloque de nivel superior.)

## Prerequisitos (fuera del código)

- Rama `feature/design-tokens-cleanup` creada desde `development` @ `84e1009` (Git Flow
  del repo).
- Commits con **Conventional Commits + gitmoji** (p. ej.
  `♻️ refactor: limpieza de tokens de diseño`).
- Trabajar con el toolchain del repo: **Node 22**. Todos los comandos de este plan se
  ejecutan **desde `web/`**.
- Recordar el **orden de build**: `web/dist` se compila **antes** de `cargo build`
  (`rust-embed` lee los assets en tiempo de compilación). Aquí **no hay cambio de backend**,
  así que la build de release es solo smoke.

## Contratos / Interfaces

### 1. `web/src/index.css`

- Eliminar las **10 variables muertas**: `--space-1`, `--space-2`, `--space-3`,
  `--space-6`, `--space-7`, `--radius-sm`, `--radius-md`, `--radius-lg`, `--radius-pill`,
  `--font-mono`.
- Colapsar `--space-4` y `--space-5` en **una sola** variable semántica `--page-padding`
  (16px), con override a 24px dentro de `@media (min-width: 768px)`; la utilidad `.page`
  pasa a consumir `var(--page-padding)`.
- Mantener `--font-sans` (única fuente de la pila de fuentes) y `--line-height-base`.
- Mantener `color-scheme: light` por defecto (sigue siendo necesario: la pantalla de login
  se renderiza fuera del `ThemeProvider`).
- **Resultado: en `:root` quedan exactamente 3 variables** (`--font-sans`,
  `--line-height-base`, `--page-padding`), **todas consumidas**.
- Añadir un **comentario de cabecera** que declare explícitamente que **el sistema de diseño
  vivo es `theme.token` en `AuthenticatedApp.tsx`** y que `index.css` solo contiene lo que
  Ant Design no expresa (reset, `color-scheme`, utilidades y las variables que el propio CSS
  consume).
- **Invariante nuevo a respetar: ninguna variable CSS declarada sin consumidor.**

### 2. `web/src/AuthenticatedApp.tsx`

- Cambiar `DESIGN_TOKENS.fontFamily`: de la cadena literal a `'var(--font-sans)'` (AntD
  emite `font-family: var(--font-sans)`, que resuelve contra `:root`).
- Comentario: la pila de fuentes se define **una sola vez** en `index.css`; el tema la
  referencia.
- **El resto de `DESIGN_TOKENS` y `COMPONENT_TOKENS` NO cambia.** `colorPrimary` sigue
  ausente del tema (default de AntD).

### 3. `web/src/pages/pageLayout.ts`

- **Sin cambios funcionales.** Completar el comentario para dejar claro que es **layout de
  página** (ritmo vertical y anchos fluidos), **no** tema de diseño, y que **no duplica
  tokens** de Ant Design.

### 4. `docs/F19-frontend-redesign.md`

- **Sin cambios.** (Verificado: la afirmación sobre los `marginBottom: 16` es correcta en
  su ámbito; el literal que sobrevive —`Sync.tsx:412`— está dentro de un `Modal`, fuera del
  `Flex`, y no es un bloque de nivel superior.)

### 5. `docs/F20-design-tokens-cleanup.md`

- Documento **nuevo** del feature. Se redactará al **cerrar**, no en este paso.

## Dependencias

- **No hay dependencias nuevas.** No se añade nada a `web/package.json` ni a `Cargo.toml`.
- `web/package-lock.json` no cambia.
- No hay cambios en `Cargo.lock` (feature exclusivamente de frontend).

## Testing (red-green-refactor)

**RED:**

- `web/src/AuthenticatedApp.test.tsx` — reforzar la aserción existente: el token expuesto
  por `ConfigProvider` debe tener `fontFamily === 'var(--font-sans)'` (hoy es la cadena
  literal → **rojo**).
- Nuevo test de invariantes (p. ej. `web/src/design-tokens.test.ts`) que lea ficheros con
  `node:fs` y verifique:
  1. **Sin variables huérfanas:** toda custom property declarada en `index.css` aparece al
     menos una vez como `var(--x)` en `web/src` → hoy **rojo** (10 huérfanas).
  2. **Pila de fuentes en un solo sitio:** el literal `system-ui` aparece en
     `web/src/index.css` y **NO** en `web/src/AuthenticatedApp.tsx` → hoy **rojo**.
  3. **Sin escala fantasma:** `index.css` no declara `--space-1`, `--space-6`, `--radius-`,
     ni `--font-mono` → hoy **rojo**.

**GREEN:** implementar los cambios 1–3 hasta verde.

**REFACTOR:** `npm run lint` limpio, comentarios claros.

## Tareas (TDD)

### Fase 1 — Tests (RED)

- [ ] **Paso 1 (RED):** Reforzar la aserción de `web/src/AuthenticatedApp.test.tsx`: el
      token de `ConfigProvider` debe exponer `fontFamily === 'var(--font-sans)'` (hoy es la
      cadena literal → rojo). `npm run test`
- [ ] **Paso 2 (RED):** Crear `web/src/design-tokens.test.ts` que lea ficheros con
      `node:fs` y verifique los tres invariantes (sin variables huérfanas; pila de fuentes
      en un solo sitio; sin escala fantasma). Confirmar **rojo** en los tres.
      `npm run test`

### Fase 2 — Implementación (GREEN)

- [ ] **Paso 3 (GREEN):** Editar `web/src/index.css`: eliminar las 10 variables muertas,
      colapsar `--space-4`/`--space-5` en `--page-padding` (16px + override 24px @768) y
      hacer que `.page` consuma `var(--page-padding)`; mantener `--font-sans`,
      `--line-height-base` y `color-scheme: light`; añadir el comentario de cabecera.
      Quedan exactamente 3 variables, todas consumidas. `npm run test`
- [ ] **Paso 4 (GREEN):** Editar `web/src/AuthenticatedApp.tsx`: `DESIGN_TOKENS.fontFamily`
      pasa a `'var(--font-sans)'` con el comentario de fuente única. Resto de tokens sin
      cambios. `npm run test`
- [ ] **Paso 5 (GREEN):** Completar el comentario de `web/src/pages/pageLayout.ts` (layout
      de página, no tema; sin duplicar tokens). Sin cambios funcionales. `npm run test`

### Fase 3 — Refactor y verificación

- [ ] **Paso 6 (REFACTOR):** `npm run lint` limpio; revisar que los comentarios de los tres
      ficheros son claros y coherentes entre sí.
- [ ] **Paso 7 (REFACTOR):** Desde `web/`: `npm run test` (línea base **112 tests en 12
      ficheros**, todos verdes más los nuevos), `npm run typecheck` (`tsc -b`),
      `npm run lint`, `npm run build` en verde.
- [ ] **Paso 8:** Medir el **delta del bundle** `web/dist`: esperado ≈ 0 o ligeramente
      negativo (no se añade código, solo se borra).
- [ ] **Paso 9:** Comprobar en el bundle construido que aparece `var(--font-sans)` y que la
      pila literal ya **no** se duplica en `AuthenticatedApp`.
- [ ] **Paso 10:** Smoke de build: `web/dist` **antes** de `cargo build` (rust-embed). Sin
      cambio de backend, así que es solo verificación de que el binario sigue sirviendo la
      SPA.
- [ ] **Paso 11 (cierre):** Escribir `docs/F20-design-tokens-cleanup.md` y renombrar este
      plan a `plans/PLAN-021-design-tokens-cleanup-COMPLETED.md`.

## Criterios de aceptación

- **`index.css` declara exactamente 3 variables** (`--font-sans`, `--line-height-base`,
  `--page-padding`) y **todas tienen consumidor**.
- **La pila de fuentes vive en un solo fichero**: `system-ui` está en `index.css` y **no**
  en `AuthenticatedApp.tsx`; el tema usa `'var(--font-sans)'`.
- **`theme.token` de AntD sin cambios salvo `fontFamily`**; `colorPrimary` por defecto.
- **`pageLayout.ts` intacto** funcionalmente (solo comentario).
- **`docs/F19-frontend-redesign.md` sin cambios.**
- **Tests verdes**: línea base de 112 tests más los nuevos de invariantes, todos en verde.
- **`npm run typecheck`, `npm run lint` y `npm run build` limpios.**
- **Delta del bundle ≈ 0 o ligeramente negativo**, medido y documentado.
- **Sin cambios** en backend, esquema, rutas DAV/REST ni contrato de datos; sin dependencias
  nuevas.
- Al cerrar: `docs/F20-design-tokens-cleanup.md` escrito y `PLAN-021` renombrado a
  `PLAN-021-design-tokens-cleanup-COMPLETED.md`.

## Riesgos

1. **`var()` dentro del token `fontFamily` de AntD.** AntD genera
   `font-family: var(--font-sans)`; al estar `--font-sans` en `:root`, **resuelve**. Riesgo
   residual: algún punto de AntD que necesite la familia **literal** en JS (no se ha
   detectado ninguno). Mitigación: **verificación explícita del CSS emitido**. **Plan B
   documentado**: definir la pila en un módulo TS mínimo y fijar `--font-sans` desde
   `main.tsx` antes del render.
2. **Tests que leen ficheros con `fs`.** vitest corre en Node con entorno jsdom;
   `node:fs` está disponible. Mitigación: **verificarlo en la primera ejecución**
   (Paso 2).
3. **Borrar una variable en uso.** Mitigado: solo hay **4 consumidores** y **3 sobreviven**
   (el 4.º se colapsa en `--page-padding`).

## Referencias

- `plans/PLAN-020-frontend-redesign-COMPLETED.md` — F19 definió el sistema de diseño
  actual; estructura y tono de la casa.
- `plans/PLAN-019-timezone-support-COMPLETED.md` — estructura y estilo del plan.
- `web/src/index.css` — las 14 variables (`--space-*`, `--radius-*`, `--font-*`),
  `body` (líneas 60-61) y `.page` (líneas 87/92).
- `web/src/AuthenticatedApp.tsx` — `theme.token` / `DESIGN_TOKENS` / `COMPONENT_TOKENS`.
- `web/src/pages/pageLayout.ts` — layout de página (ritmo vertical y anchos fluidos).
- `web/src/AuthenticatedApp.test.tsx` — aserción de `fontFamily`.
- `docs/F19-frontend-redesign.md` — documentación del sistema de diseño de F19 (sin
  cambios).
- `web/src/pages/Sync.tsx:412` — literal `marginBottom` dentro del `Modal` (no es bloque de
  nivel superior).
- `AGENTS.md` — flujo OpenSpec/TDD, orden de build del frontend y convenciones del SPA.
