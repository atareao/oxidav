# OxiDav — Web SPA

Interfaz de usuario de **OxiDav**, un servidor personal CalDAV / CardDAV / WebDAV
escrito en Rust. Esta SPA gestiona calendarios, contactos, dispositivos/tokens de
sincronización y ajustes mediante la API REST JSON del backend (`/api/v1/...`).
En esta fase (F02) las vistas son **placeholders**: el objetivo es el esqueleto,
el layout y el enrutado.

## Stack

- **React 19** + **TypeScript** (modo `strict`).
- **Ant Design v6** (`ConfigProvider` + `theme.darkAlgorithm` / `defaultAlgorithm`,
  locale `es_ES`) como sistema de diseño.
- **Vite 8** (`@vitejs/plugin-react`).
- **React Router v7** (`BrowserRouter`, URLs limpias).
- **Vitest** + **@testing-library/react** + **jsdom** para tests.
- **ESLint** (flat config) para lint.

## Comandos

| Comando            | Descripción                                              |
| ------------------ | -------------------------------------------------------- |
| `npm run dev`      | Servidor de desarrollo Vite (proxy `/api` y `/dav` → `http://localhost:8080`). |
| `npm run build`    | Type-check (`tsc -b`) + build de producción a `web/dist/`. |
| `npm run preview`  | Sirve localmente el build de `dist/`.                    |
| `npm run lint`     | `eslint .`                                               |
| `npm run test`     | `vitest run` (ejecución única).                          |
| `npm run typecheck`| `tsc -b` (comprueba los proyectos referenciados).        |

> **Orden de build (repo completo):** compilar el frontend (`web/dist`) **antes**
> de `cargo build`, porque `rust-embed` embebe esos assets en tiempo de compilación.

## Contrato de build

`npm run build` genera `web/dist/index.html` + `dist/assets/*` con **rutas
relativas** (`base: "./"` en `vite.config.ts`), de modo que `rust-embed` pueda
servirlos desde el binario.

## ⚠️ Requisito bloqueante para F15 (SPA fallback)

La SPA usa `BrowserRouter` con rutas limpias (`/calendar`, `/contacts`, `/sync`,
`/settings`) y **no** un `HashRouter`. Por tanto, al servir `dist/` desde el
binario Rust, el backend **DEBE implementar SPA fallback**:

> Servir `index.html` para **cualquier** ruta que no corresponda a un asset
> estático ni a los prefijos `/api/` / `/dav/`. De lo contrario, un deep-link
> o un refresco de página en `/calendar` devolverá **404**.

Esto debe quedar implementado y verificado como parte de **F15** (integración
`rust-embed` + servido de assets). Sin este fallback la navegación por URL
directa está rota aunque el build sea correcto.

Si en el futuro la SPA se monta bajo un **subpath** (p. ej. `/app`), habrá que
añadir `basename={import.meta.env.BASE_URL}` (o el valor explícito) al
`BrowserRouter` de `src/main.tsx`.
