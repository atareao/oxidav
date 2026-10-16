# PLAN-022 — Proyecto de TypeScript propio para los tests (F21)

## Objetivo

Que el proyecto de la aplicación (`web/tsconfig.app.json`) **deje de ver** los tipos de
Node y los globales de Vitest, y que un proyecto nuevo (`web/tsconfig.test.json`) sea el
**dueño** de esas dos cosas.

Hoy `web/tsconfig.app.json` declara `types: ["vite/client", "vitest/globals", "node"]` y su
`include` es `["src"]`, que barre también los 13 ficheros `*.test.ts(x)` y
`src/test/setup.ts`. El resultado es que el código de aplicación arrastra, sin necesitarlos,
`process`, `fs`, `path` y los globales de test. F20 tuvo que **añadir `node`** a esa lista
para un único fichero de test (`src/design-tokens.test.ts`) y lo dejó documentado como
desviación aceptada; esta feature cierra esa desviación.

## Alcance

Feature (F21) **exclusivamente de frontend**, y dentro del frontend **solo configuración de
TypeScript** (más dos comentarios). No cambia nada de runtime.

**FUERA DE ALCANCE (no incluir en las tareas):**

- **Código de aplicación**: no se toca ni una línea, salvo los **comentarios** de
  `web/src/contexts/AuthProvider.tsx` y de `web/src/design-tokens.test.ts` (ver Contratos 5 y 6).
- **`web/vite.config.ts`**: sin cambios. En particular `test.globals: true` **se queda**
  (ver Decisiones vinculantes, punto 4).
- **`web/tsconfig.node.json`**: sin cambios; sigue cubriendo `vite.config.ts`.
- **`web/eslint.config.js`**: sin cambios (no hay linting con información de tipos; verificado).
- **Dependencias**: ninguna nueva ni ninguna versión movida.
- **Backend, esquema, rutas DAV/REST, contrato de datos, CI, `Dockerfile`, `compose.yaml`**:
  sin cambios.
- **Los 13 ficheros de test**: sin cambios funcionales (ya importan de `'vitest'` explícitamente).

## Motivación

### Enmienda por auditoría (2026-10-16)

La implementación y la auditoría posterior ajustaron estos puntos del plan; donde este
documento diga otra cosa, **manda esta enmienda**:

- **Los recuentos.** El plan habla de 13 ficheros de test y 116 tests: es la foto **previa**
  a esta feature. Tras F21 son **14 ficheros y 124 tests**, porque el propio test de
  invariantes es uno de ellos.
- **El patrón `src/test/**` bastó.** No hubo que cambiarlo a `src/test/**/*`: verificado con
  el parser de TypeScript (`setup.ts` queda fuera del proyecto de app y dentro del de test).
- **El código de error de la mutación (a) es TS2591**, no el TS2580 que estimaba el plan:
  `Cannot find name 'process'`. El mensaje sugiere «instalar `@types/node`», engañoso aquí:
  ya está instalado, lo que se quitó fue su declaración en el proyecto de app.
- **Guarda añadida por auditoría.** `readProjectConfig` ahora falla si
  `ts.parseJsonConfigFileContent` devuelve diagnósticos —`readConfigFile` no avisa de un
  `extends` roto, así que antes se colaba— y cachea el parseo de cada proyecto para no
  repetirlo en cada invariante.
- **`docs/F20-design-tokens-cleanup.md`** describe la decisión contraria: en su tabla de
  hallazgos dice que el `tsconfig.test.json` separado «se descartó por superficie», y en
  «Desviaciones del plan» que hubo que añadir `"node"` a `tsconfig.app.json`. Es el registro
  histórico de F20 y **se deja como está**, pero F21 **revierte esa decisión**.
- **`compose.yaml`** estaba modificado en el árbol de trabajo por el usuario: quedó **fuera
  de todos los commits** de F21.

### Diagnóstico medido

- `web/tsconfig.json`: estilo *solution*, con `files: []` y **2 referencias**
  (`./tsconfig.app.json`, `./tsconfig.node.json`).
- `web/tsconfig.app.json`: `include: ["src"]`, `types: ["vite/client", "vitest/globals", "node"]`.
- `web/tsconfig.node.json`: `include: ["vite.config.ts"]`, `types: ["node"]`.
- Bajo `src/` hay **13** ficheros `*.test.ts(x)` más `src/test/setup.ts`.
  El directorio `src/test/` contiene **únicamente** `setup.ts`.

### Hechos verificados (no suposiciones)

- **Ningún** fichero de aplicación (no-test) usa APIs de Node: el barrido de `node:`,
  `require(`, `process.`, `__dirname`, `__filename` en `src` excluyendo `*.test.*` da
  **0 resultados**.
- El **único** consumidor de los tipos de `node` es `src/design-tokens.test.ts`
  (líneas 13-14 y 20: `node:fs`, `node:path`, `process.cwd()`).
- Los **13** ficheros de test importan de `'vitest'` explícitamente, así que hoy nadie
  depende de los globales sueltos. Aun así `globals: true` sigue activo en Vitest: se
  **mueve**, no se borra.
- `vite/client` sí lo necesita la aplicación de verdad: `import.meta.env` aparece en
  `src/services/api.ts:14` y `src/contexts/AuthProvider.tsx:23`.
- `src/test/setup.ts` no usa Node ni globales de test: usa `ResizeObserver`/`matchMedia`
  (DOM) e importa `@testing-library/jest-dom/vitest`.
- **`tsc -b` con el TypeScript 6.0.3 del repo NO exige `composite: true`** en los proyectos
  referenciados: comprobado con un repro mínimo de dos proyectos sin `composite`, que sale
  OK. Una referencia a un proyecto inexistente da `error TS5083`. Por tanto se puede añadir
  un tercer proyecto referenciado sin tocar nada más.
- ESLint **no** usa `projectService` ni `project` → añadir un `tsconfig` no le afecta.
- La CI ejecuta `npx tsc -b` y `npm run build` (= `tsc -b && vite build`), de modo que el
  proyecto nuevo queda ejercitado en CI y en el build de la imagen.
- `web/tsconfig.app.json` contiene **comentarios JSONC** (`/* Bundler mode */`, `/* Linting */`),
  así que **no es `JSON.parse`-able**: el test de invariantes debe usar el parser de
  TypeScript (`ts.readConfigFile`).

## Decisiones vinculantes

1. `web/tsconfig.test.json` **extiende** `./tsconfig.app.json` y **sobrescribe explícitamente**
   `include`, `exclude`, `types` y `tsBuildInfoFile`. No se confía en la herencia de esas claves.
2. `web/tsconfig.app.json` pasa a `types: ["vite/client"]` y añade un `exclude` para los tests
   y para `src/test/`; `include: ["src"]` se mantiene.
3. `web/tsconfig.json` gana una **tercera referencia** al proyecto de test.
4. **Mover, no borrar**: `vitest/globals` viaja de app a test. `vite.config.ts` conserva
   `test.globals: true`. El invariante solo exige que la **app** no lo vea.
5. Los invariantes se comprueban con el **propio resolver de TypeScript**, no con globs
   caseros: el test usa `ts.readConfigFile` + `ts.parseJsonConfigFileContent` para obtener los
   `fileNames` reales de cada proyecto. Usar `node:fs`/`ts.sys` ahí es legítimo: ese fichero vive
   en el proyecto de test, que **sí** tiene tipos de Node — y ese es justamente el punto.
6. `npm run typecheck` y `npm run build` **siguen siendo `tsc -b`**: el proyecto de test queda
   cubierto por la misma orden. No se tocan los scripts de `package.json`.

## Prerequisitos (fuera del código)

- Ninguno: no hay variables de entorno, secretos ni servicios implicados.
- El plan se registra en `plans/` **antes** de implementar y se renombra a `-COMPLETED.md` al
  cerrarse (convención del repo; **no** hay `openspec/`).
- Nota de proceso: el arreglo del comentario de `AuthProvider.tsx` se pliega aquí en lugar de
  ir en un PR propio, como se acordó al cerrar el PR #20.

## Contratos / Interfaces

### 1. `web/tsconfig.test.json` (nuevo)

```json
{
  "extends": "./tsconfig.app.json",
  "compilerOptions": {
    "tsBuildInfoFile": "./node_modules/.tmp/tsconfig.test.tsbuildinfo",
    "types": ["vite/client", "vitest/globals", "node"]
  },
  "include": ["src/**/*.test.ts", "src/**/*.test.tsx", "src/test/**/*"],
  "exclude": []
}
```

Se heredan del proyecto de app (y eso es deseable, evita drift): `jsx`, `lib` (ES2023 + DOM),
`moduleResolution: bundler`, `verbatimModuleSyntax`, `noEmit`, `strict`, `erasableSyntaxOnly`, etc.

### 2. `web/tsconfig.app.json` (modificado)

- `"types": ["vite/client"]` (se quitan `"vitest/globals"` y `"node"`).
- Añadir `"exclude": ["src/**/*.test.ts", "src/**/*.test.tsx", "src/test/**"]`.
- `"include": ["src"]` **intacto**.

### 3. `web/tsconfig.json` (modificado)

- Añadir `{ "path": "./tsconfig.test.json" }` a `references`, junto a los dos existentes.

### 4. `web/src/tsconfig-invariants.test.ts` (nuevo)

Invariantes que debe fijar, leyendo las tres configuraciones con el parser de TypeScript:

- **I1** — El proyecto de app **no** ve Node: sus `compilerOptions.types` no incluyen `"node"`.
- **I2** — El proyecto de app **no** ve los globales de test: no incluye `"vitest/globals"`.
- **I3** — El proyecto de app **conserva** `"vite/client"`.
- **I4** — Cobertura sin solapes: **ningún** `fileName` del proyecto de app termina en
  `.test.ts`/`.test.tsx` ni está bajo `src/test/`, y **todo** fichero `*.test.ts(x)` que exista
  bajo `src/` aparece en los `fileNames` del proyecto de test.
- **I5** — `src/test/setup.ts` está en el proyecto de test.
- **I6** — Sanidad: el proyecto de app sigue incluyendo `src/main.tsx` y `src/App.tsx`
  (no hemos vaciado el proyecto).
- **I7** — `tsconfig.json` referencia `./tsconfig.test.json`.
- **I8** — El proyecto de test declara `"node"` y `"vitest/globals"`: es el dueño de ambos.

El fichero debe **fallar con un mensaje claro** si no encuentra `web/tsconfig.json` bajo
`process.cwd()` (Vitest corre con la raíz del paquete como cwd, igual que en F20).

### 5. `web/src/contexts/AuthProvider.tsx` (comentario, línea 9)

`plans/PLAN-018-oidc-valet-parity.md` → `plans/PLAN-018-oidc-valet-parity-COMPLETED.md`.
Es la **única** referencia obsoleta a un plan que quedaba en el repo tras el PR #20.

### 6. `web/src/design-tokens.test.ts` (comentario, líneas 9-10)

Hoy dice que los tipos de `node` los declara `tsconfig.app.json`. Tras F21 es falso: hay que
reescribirlo para que apunte a `tsconfig.test.json`.

## Dependencias

- **Sin cambios**: ni nuevas dependencias ni versiones movidas. Ya presentes y suficientes:
  `typescript` ~6.0.2 (6.0.3 instalada), `@types/node` ^24.13.3, `vitest` ^5.0.4.
- `web/package.json` y `web/package-lock.json`: **sin cambios**.

## Testing (red-green-refactor)

- **RED**: escribir `src/tsconfig-invariants.test.ts` contra el estado actual. `npm run test`
  debe **fallar** en I1, I2, I4, I5, I7 e I8 (no existe `tsconfig.test.json`; el proyecto de app
  barre los tests y declara `node` + `vitest/globals`). Los 116 tests existentes siguen verdes.
- **GREEN**: crear `tsconfig.test.json` y ajustar `tsconfig.app.json` y `tsconfig.json` →
  todo verde.
- **REFACTOR**: comentarios, nombres de helpers, y los dos comentarios obsoletos (Contratos 5 y 6).
- **Pruebas negativas (mutación), obligatorias** — son la prueba de que el refactor hace algo real:
  - **(a)** añadir `console.log(process.argv0)` a un fichero de app (p. ej. `src/App.tsx`) y
    ejecutar `npm run typecheck`: debe **fallar** con «Cannot find name 'process'». Demuestra que
    la app ya no ve Node. Revertir.
  - **(b)** añadir un fichero de test de usar y tirar con `describe('x', () => {})` **sin import
    de `'vitest'`** y ejecutar `npm run typecheck`: debe **pasar**. Demuestra que el proyecto de
    test sí tiene los globales. Borrar el fichero.
  - **(c)** quitar del `exclude` del proyecto de app una de las entradas de test y ejecutar
    `npm run test`: debe **fallar** en I4. Demuestra que el invariante detecta el fallo. Restaurar.
- Comandos (desde `web/`): `npm run test`, `npm run typecheck`, `npm run lint`, `npm run build`.
  Opcional: `npx tsc -b --dry` para inspeccionar el orden de proyectos.

## Tareas (TDD)

### Fase 1 — Tests (RED)

- [ ] T1 Crear `web/src/tsconfig-invariants.test.ts` con I1–I8, leyendo las configuraciones con
      `ts.readConfigFile` + `ts.parseJsonConfigFileContent`, y con los helpers necesarios.
- [ ] T2 `npm run test` → confirmar **RED** en los invariantes nuevos y los 116 tests previos
      intactos.

### Fase 2 — Implementación (GREEN)

- [ ] T3 Crear `web/tsconfig.test.json`.
- [ ] T4 Ajustar `web/tsconfig.app.json` (`types` y `exclude`).
- [ ] T5 Añadir la referencia del proyecto de test en `web/tsconfig.json`.
- [ ] T6 `npm run test` verde, `npm run typecheck` verde, `npm run build` verde.

### Fase 3 — Refactor y verificación

- [ ] T7 Actualizar los comentarios obsoletos (Contratos 5 y 6).
- [ ] T8 Ejecutar las pruebas negativas (a), (b) y (c), y revertir cada mutación.
- [ ] T9 `npm run lint` limpio, `npx tsc -b` limpio, `npm run build` limpio.
- [ ] T10 Escribir `docs/F21-tsconfig-test-project.md` y renombrar este plan a
      `PLAN-022-tsconfig-test-project-COMPLETED.md`.

## Criterios de aceptación

- **`web/tsconfig.test.json` existe** y es el dueño de `node` y `vitest/globals`.
- **`web/tsconfig.app.json` declara `types: ["vite/client"]`** y excluye los tests y `src/test/`.
- **`web/tsconfig.json` referencia los tres proyectos.**
- **El invariante detecta un fallo real**, probado por la mutación (c).
- **La mutación (a) confirma** que la aplicación ya no ve `process`.
- **Los 116 tests existentes siguen verdes**, más los nuevos invariantes.
- **`npm run test`, `npm run typecheck`, `npm run lint` y `npm run build` limpios.**
- **Delta de bundle ≈ 0**: nada de lo cambiado entra en el bundle; los assets emitidos deben
  conservar sus hashes (o equivalentes), medido y documentado.
- **Sin cambios** en `vite.config.ts`, `tsconfig.node.json`, `eslint.config.js`, `package.json`,
  `package-lock.json`, backend, CI ni `Dockerfile`.
- Al cerrar: `docs/F21-tsconfig-test-project.md` escrito y el plan renombrado a
  `PLAN-022-tsconfig-test-project-COMPLETED.md`.

## Riesgos

- **`process.cwd()` en el test de invariantes**: depende de que Vitest corra desde `web/`.
  Mitigación: comprobar la existencia de `tsconfig.json` y fallar con un mensaje explícito.
- **El proyecto de test también type-checkea ficheros de app** (los sigue por los imports), así
  que necesita `vite/client` y `lib` DOM: ambos incluidos/heredados. Si faltase algo, se vería
  en `tsc -b`.
- **`tsc -b` pasa a construir tres proyectos**: build algo más lento y los tests se type-checkean
  también en `npm run build`. No es un cambio neto: hoy ya estaban dentro de `src`.
- **`include: ["src/test/**/*"]` cogería cualquier fichero futuro de ese directorio.** Es
  intencionado: `src/test/` es el directorio de soporte de los tests.
- **Un test futuro sin importar de `'vitest'` seguirá funcionando** (globals activos) porque el
  proyecto de test declara `vitest/globals`.

## Referencias

- `plans/PLAN-021-design-tokens-cleanup-COMPLETED.md` — F20, donde se aceptó la desviación que
  aquí se cierra.
- `docs/F20-design-tokens-cleanup.md` — su documentación.
- `web/tsconfig.json`, `web/tsconfig.app.json`, `web/tsconfig.node.json`.
- `web/vite.config.ts` (`test.globals`, `test.setupFiles`).
- `web/src/design-tokens.test.ts` — el único consumidor de los tipos de Node.
- `web/src/test/setup.ts` — único fichero de soporte de tests.
- `.github/workflows/ci.yml` — `npx tsc -b`, `npm run lint`, `npx vitest run`, `npm run build`.
- `AGENTS.md` (raíz) — stack, convención `plans/`/`docs/` y comandos del frontend.
