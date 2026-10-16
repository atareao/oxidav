# F21 — Proyecto de TypeScript propio para los tests

> El plan de origen es [`plans/PLAN-022-tsconfig-test-project-COMPLETED.md`](../plans/PLAN-022-tsconfig-test-project-COMPLETED.md).
> Cierra la desviación que F20 aceptó: el proyecto de la aplicación había pasado a declarar
> los tipos de `node` para un solo fichero de pruebas.

## Qué implementa

Separar la configuración de TypeScript en **tres proyectos** con fronteras explícitas:

| Proyecto | Cubre | `types` |
| --- | --- | --- |
| `tsconfig.app.json` | el código de la aplicación (`src`, sin los tests) | `["vite/client"]` |
| `tsconfig.node.json` | `vite.config.ts` (sin cambios) | `["node"]` |
| `tsconfig.test.json` (**nuevo**) | los 14 ficheros `*.test.ts(x)` y `src/test/**/*` | `["vite/client", "vitest/globals", "node"]` |

`tsconfig.json` sigue siendo el fichero *solution* (`files: []`) y ahora referencia los tres.

Antes de F21, `tsconfig.app.json` declaraba `types: ["vite/client", "vitest/globals", "node"]`
con `include: ["src"]`, de modo que el código de aplicación veía `process`, `fs`, `path` y los
globales de Vitest **sin necesitarlos**.

## El diagnóstico (medido, no supuesto)

- Ningún fichero de aplicación usaba APIs de Node: el barrido de `node:`, `require(`,
  `process.`, `__dirname` y `__filename` en `src` excluyendo `*.test.*` daba **0 resultados**.
- El **único** consumidor de los tipos de `node` era `src/design-tokens.test.ts` (`node:fs`,
  `node:path`, `process.cwd()`).
- Los 13 ficheros de test importaban de `'vitest'` explícitamente: nadie dependía de los
  globales sueltos.
- `vite/client` sí lo usa la aplicación de verdad (`import.meta.env` en `src/services/api.ts`
  y `src/contexts/AuthProvider.tsx`).

De ahí las dos decisiones que definen la feature: **mover** los tipos de `node` y
`vitest/globals` al proyecto de test —no borrarlos— y **no tocar** `vite/client`.

## Qué cambia

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

Hereda del proyecto de app lo que interesa (`jsx`, `lib` con DOM,
`moduleResolution: bundler`, `verbatimModuleSyntax`, `strict`, `noEmit`…) y sobrescribe
explícitamente `include`, `exclude`, `types` y `tsBuildInfoFile` para no depender de la
herencia.

El `"exclude": []` **no sobra**: sin él, el proyecto de test heredaría el `exclude` del
proyecto de app y se excluiría a sí mismo sus propios ficheros.

### 2. `web/tsconfig.app.json`

`types` pasa a `["vite/client"]` y se añade:

```json
"exclude": ["src/**/*.test.ts", "src/**/*.test.tsx", "src/test/**"]
```

`include: ["src"]` se mantiene.

### 3. `web/tsconfig.json`

Tercera referencia: `{ "path": "./tsconfig.test.json" }`.

### 4. Comentarios corregidos

- `web/src/contexts/AuthProvider.tsx`: citaba `plans/PLAN-018-oidc-valet-parity.md`, que ya no
  existe con ese nombre. Era la última referencia obsoleta a un plan del repositorio.
- `web/src/design-tokens.test.ts`: afirmaba que los tipos de `node` los declaraba
  `tsconfig.app.json`.

## El invariante nuevo

`web/src/tsconfig-invariants.test.ts` fija ocho invariantes y **no usa globs caseros**: lee las
configuraciones con el propio parser de TypeScript (`ts.readConfigFile` +
`ts.parseJsonConfigFileContent`, necesario porque los `tsconfig` llevan comentarios JSONC) y
compara los `fileNames` **reales** de cada proyecto.

| | Invariante |
| --- | --- |
| I1 | El proyecto de app no declara el tipo `node` |
| I2 | El proyecto de app no declara `vitest/globals` |
| I3 | El proyecto de app conserva `vite/client` |
| I4 | No hay solapes: ningún fichero de test en el proyecto de app y **todos** los del disco en el de test |
| I5 | `src/test/setup.ts` pertenece al proyecto de test |
| I6 | El proyecto de app sigue compilando `src/main.tsx` y `src/App.tsx` |
| I7 | El fichero *solution* referencia el proyecto de test |
| I8 | El proyecto de test declara `node` y `vitest/globals` |

Además se exige que **cada `tsconfig` parsee sin diagnósticos**: `readConfigFile` no avisa de
un `extends` roto, así que sin esa guarda un fallo de configuración podría pasar por
«parseable».

Partición verificada: 45 ficheros `.ts`/`.tsx` bajo `src/` = **30** en el proyecto de app +
**15** en el de test (14 tests + `setup.ts`), **disjuntos y exhaustivos**. No hay `.d.ts`
propios bajo `src/`.

## Hallazgos de la auditoría y su resolución

| Hallazgo | Severidad | Resolución |
| --- | --- | --- |
| `readProjectConfig` ignoraba `parsed.errors` | media (un `extends` roto se colaba) | **Corregido**: falla nombrando los diagnósticos; probado con una mutación |
| El helper reparseaba cada `tsconfig` en cada `it` | baja | **Corregido**: caché por nombre de fichero |
| El test de invariantes importa el compilador completo en runtime | baja | **Aceptado**: es lo que permite comprobar los `fileNames` reales |
| El proyecto de test sigue los imports de la app (necesita `vite/client` y DOM) | baja | **Aceptado y documentado**: un `process` mal usado sigue dando error en el proyecto de app, que es donde el desarrollador mira |
| El proyecto de test extiende del de app en vez de una base común | baja | **Aceptado**: hoy no hay nada del app que el test no deba heredar, y `types` ya está aislado |
| `npm run build` también type-checkea los tests | baja | **Aceptado**: es un cambio neto **cero**; antes también ocurría, porque los tests vivían dentro de `include: ["src"]` |

**Sin bloqueantes**: la configuración y los invariantes pasaron la auditoría sin defectos que
impidieran cerrar la feature.

## Desviaciones del plan

1. **Los recuentos.** El plan habla de 13 ficheros de test y 116 tests: es la foto **previa** a
   la feature. Tras F21 son **14 ficheros y 124 tests**, porque el propio test de invariantes es
   uno de ellos.
2. **El patrón `src/test/**` bastó.** El plan preveía el ajuste a `src/test/**/*` como posible
   necesidad; no hizo falta. Verificado con el parser de TypeScript.
3. **El código de error de la mutación (a) es TS2591**, no el «TS2580» que estimaba el plan:
   `Cannot find name 'process'`. El mensaje del compilador sugiere «instalar `@types/node`»,
   engañoso aquí precisamente porque ya está instalado: lo que se quitó fue su declaración en
   el proyecto de app.
4. **`compose.yaml`** estaba modificado en el árbol de trabajo por el usuario y quedó **fuera
   de todos los commits** de F21.

## Verificación

```text
# Frontend (desde web/)
npm run test        → 14 ficheros / 124 tests, 0 fallos
npm run typecheck   → tsc -b limpio (tres proyectos)
npm run lint        → eslint limpio
npm run build       → tsc -b && vite build limpio
```

Pruebas negativas (mutación), las cuatro revertidas:

| Mutación | Esperado | Observado |
| --- | --- | --- |
| `process.argv0` en un fichero de app | `typecheck` falla | **TS2591** «Cannot find name 'process'» |
| Un `.test.ts` con `describe()` sin import de `vitest` | `typecheck` pasa | pasa (el proyecto de test sí tiene los globales) |
| Quitar `"src/test/**"` del `exclude` del app | el invariante falla | **I4** falla nombrando `src/test/setup.ts` |
| `extends` a un fichero inexistente | el invariante falla | falla con «must parse without errors: Cannot read file…» |

### Bundle

**Delta 0 B.** La suma de `web/dist` es idéntica a la que dejó F20 (1 270 610 B) y **todos** los
assets conservan su hash, incluido el CSS (`index-DY2fJ0xB.css`). El cambio es solo
configuración de TypeScript: nada entra en el bundle.

## Relacionado

- [`plans/PLAN-022-tsconfig-test-project-COMPLETED.md`](../plans/PLAN-022-tsconfig-test-project-COMPLETED.md) — el plan de esta feature.
- [`docs/F20-design-tokens-cleanup.md`](./F20-design-tokens-cleanup.md) — la feature anterior, donde se aceptó la desviación que F21 cierra. Su tabla de hallazgos (fila del `tsconfig.test.json` separado) y su apartado «Desviaciones del plan» describen la decisión que aquí se **revierte**; se dejan como registro histórico.
- [`AGENTS.md`](../AGENTS.md) — stack y comandos del frontend, y el orden de build.
- `web/tsconfig.json`, `web/tsconfig.app.json`, `web/tsconfig.node.json`, `web/tsconfig.test.json`.
- `web/src/tsconfig-invariants.test.ts` — los invariantes que fijan la frontera.
