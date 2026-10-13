# F02 — Scaffolding frontend

## Qué hace

Monta la SPA de gestión de OxiDav: un proyecto Vite + React 19 + TypeScript con Ant Design v6,
un layout base con menú de navegación y cuatro vistas placeholder (Calendario, Contactos,
Sincronización/Dispositivos y Ajustes). Define el contrato de rutas y el contrato de build
(`web/dist` con rutas relativas) que el backend embebe con `rust-embed` en F15. No implementa
lógica de negocio: cada página es un placeholder que se rellenará en F11–F14.

## Stack y versiones verificadas

| Paquete | Versión |
|---|---|
| React | 19.2.8 |
| react-dom | 19.2.8 |
| Ant Design | 6.6.6 |
| react-router-dom | 7.18.4 |
| Vite | 8.3.4 |
| Vitest | 5.0.4 |
| TypeScript | 6.0.2 |
| eslint | 10.x |

Ubicación: `web/`.

## Estructura

- `web/src/components/`, `web/src/pages/`, `web/src/contexts/`, `web/src/services/`, `web/src/test/`
- `web/src/App.tsx`, `web/src/main.tsx`
- `web/src/pages/{Calendar,Contacts,SyncDevice,Settings}.tsx` — placeholders, con **export con nombre**.

## Rutas y navegación

| Ruta | Destino |
|---|---|
| `/` | Redirige a `/calendar` |
| `/calendar` | Vista Calendario (default) |
| `/contacts` | Vista Contactos |
| `/sync` | Vista Sincronización y Dispositivos |
| `/settings` | Vista Ajustes |

Se usa `BrowserRouter` (URLs limpias, sin hash). El menú renderiza `<Link>` y además dispara
`onClick → navigate`. El `selectedKey` se resuelve por igualdad exacta de ruta o por
`key + '/'`, de modo que las subrutas de una sección mantienen el item seleccionado.

## Tema

`ThemeContext` expone un modo claro/oscuro conmutable (sin persistencia). El `ConfigProvider` de
Ant Design aplica `theme.darkAlgorithm` o `theme.defaultAlgorithm` según el modo, y fija el
locale en español.

## Build y contrato de embebido

`vite.config.ts` define `base: "./"`, `build.outDir: "dist"`, y un proxy de desarrollo para
`/api` y `/dav` → `http://localhost:8080`.

**Contrato:** `npm run build` genera `web/dist/` con rutas **RELATIVAS** (`./assets/...`), de modo
que los assets funcionan tanto servidos desde la raíz como embebidos por `rust-embed`.

**Orden de build:** compilar `web/dist` **ANTES** de `cargo build`, porque `rust-embed` lee esos
assets en tiempo de compilación.

## REQUISITO BLOQUEANTE PARA F15 — SPA fallback

Con `BrowserRouter` y URLs limpias, el backend **DEBE** servir `index.html` (SPA fallback) para
cualquier ruta que no sea un asset ni empiece por `/api/` o `/dav/`. De lo contrario, los
deep-links y el refresco directo de `/calendar` (o cualquier ruta) devolverán **404**. Si algún
día la SPA se sirve bajo un subpath, habrá que añadir `basename` al router del frontend.

## Comandos

| Comando | Descripción |
|---|---|
| `npm run dev` | Servidor de desarrollo Vite (con proxy `/api` y `/dav`) |
| `npm run build` | Compila a `web/dist/` |
| `npm run preview` | Sirve el build de producción localmente |
| `npm run lint` | ESLint |
| `npm run test` / `npx vitest run` | Tests (8 tests) |
| `npm run typecheck` | `tsc -b`, comprueba de verdad el proyecto |

## Notas y decisiones

- `typecheck` es `tsc -b` porque el `tsconfig.json` raíz es solo `references` y `tsc --noEmit` no comprobaba nada.
- Warning informativo de Vite por chunk > 500 kB (antd completo); optimizable más adelante con code-splitting.
- Assets muertos de la plantilla eliminados; `.oxlintrc.json` eliminado.

## Relacionado

- [`plans/PLAN-003-scaffolding-frontend-COMPLETED.md`](../plans/PLAN-003-scaffolding-frontend-COMPLETED.md)
- [`plans/PLAN-001-general.md`](../plans/PLAN-001-general.md)
- [`web/README.md`](../web/README.md)
- [`AGENTS.md`](../AGENTS.md)
