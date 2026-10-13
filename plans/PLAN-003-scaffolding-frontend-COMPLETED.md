# PLAN-003 — F02 Scaffolding frontend

## Objetivo

Levantar el esqueleto de la SPA de OxiDav en `web/` con React 19, TypeScript y Vite, integrando Ant Design v6 como sistema de diseño con tema claro/oscuro conmutable. Definir el layout de aplicación, el enrutado de las cuatro secciones principales (`/calendar`, `/contacts`, `/sync`, `/settings`) y la configuración de build que produce `web/dist/` para el embebido posterior con `rust-embed`. En esta fase las vistas son únicamente placeholders: no se implementa funcionalidad real. Al terminar, `npm run build` debe generar assets estáticos listos para ser consumidos por el binario Rust.

## Alcance

- Crear `web/` con Vite + React 19 + TypeScript (`@vitejs/plugin-react`), `package.json`, `tsconfig.json`, `vite.config.ts`.
- Instalar y configurar **Ant Design v6** con `ConfigProvider` y soporte de tema claro/oscuro mediante `theme.darkAlgorithm` (estado de tema en un contexto, sin persistencia todavía).
- Layout base con `Layout`/`Sider`/`Header`/`Content` y navegación con `Menu` a las cuatro rutas: `/calendar`, `/contacts`, `/sync`, `/settings` (react-router).
- Páginas placeholder por vista con tipografía Antd (`Typography.Title`).
- Configurar `vite.config.ts` con `base: "./"` (para servir desde el binario), `build.outDir: "dist"` y proxy de desarrollo hacia el backend en `/api` y `/dav`.
- Scripts npm: `dev`, `build`, `preview`, `lint`, `test` (Vitest), `typecheck` (`tsc --noEmit`).
- Configurar Vitest + `@testing-library/react` + `jsdom` con un smoke test del layout/rutas.

## Contratos / Interfaces

- Estructura de carpetas objetivo: `web/src/{components,pages,services,contexts}`, `App.tsx`, `main.tsx`.
- `pages/` con `Calendar.tsx`, `Contacts.tsx`, `SyncDevice.tsx`, `Settings.tsx` (placeholders).
- Rutas: `/calendar` (default), `/contacts`, `/sync`, `/settings`; redirección de `/` a `/calendar`.
- Contrato de build: `npm run build` produce `web/dist/index.html` + assets con rutas relativas.

## Tareas (TDD / verificación)

- [ ] **Paso 1:** Scaffold del proyecto Vite + React 19 + TypeScript en `web/`.
      `npm create vite@latest web -- --template react-ts`
- [ ] **Paso 2:** Ajustar `package.json` con nombre, `private: true` y scripts `dev`, `build`, `preview`, `lint`, `test`, `typecheck`.
      `"build": "tsc -b && vite build"`, `"typecheck": "tsc --noEmit"`, `"test": "vitest run"`
- [ ] **Paso 3:** Instalar dependencias base y de desarrollo.
      `npm i react@19 react-dom@19 react-router-dom antd@6`
      `npm i -D vite @vitejs/plugin-react typescript @types/react @types/react-dom vitest jsdom @testing-library/react @testing-library/jest-dom @testing-library/user-event eslint`
- [ ] **Paso 4:** Configurar `vite.config.ts` con `base: "./"`, `build.outDir: "dist"` y proxy de desarrollo hacia el backend.
      ```ts
      export default defineConfig({
        base: "./",
        plugins: [react()],
        build: { outDir: "dist" },
        server: {
          proxy: {
            "/api": "http://localhost:8080",
            "/dav": "http://localhost:8080",
          },
        },
        test: { environment: "jsdom", setupFiles: ["./src/test/setup.ts"] },
      });
      ```
- [ ] **Paso 5:** Configurar Vitest (entorno `jsdom`) y fichero de setup con `@testing-library/jest-dom`.
      `web/src/test/setup.ts` → `import "@testing-library/jest-dom";`
- [ ] **Paso 6:** Crear el contexto de tema (`contexts/ThemeContext.tsx`) con estado claro/oscuro y su provider (sin persistencia).
      ```tsx
      type Mode = "light" | "dark";
      const ThemeContext = createContext<{ mode: Mode; toggle: () => void } | null>(null);
      ```
- [ ] **Paso 7:** Envolver `App` en `ConfigProvider` con `theme={{ algorithm: mode === "dark" ? theme.darkAlgorithm : theme.defaultAlgorithm }}` y `locale` en español.
      `web/src/main.tsx` + `web/src/App.tsx`
- [ ] **Paso 8:** Implementar el layout base en `components/AppLayout.tsx` (`Layout` + `Sider` + `Header` + `Content`) con un `Menu` que enlaza a las cuatro rutas y un botón de conmutación de tema en el `Header`.
      `web/src/components/AppLayout.tsx`
- [ ] **Paso 9:** Definir el router en `App.tsx` con `Routes`/`Route`: `/` redirige a `/calendar`; rutas `/calendar`, `/contacts`, `/sync`, `/settings`.
      `web/src/App.tsx`
- [ ] **Paso 10:** Crear las páginas placeholder con `Typography.Title`.
      `web/src/pages/{Calendar,Contacts,SyncDevice,Settings}.tsx`
- [ ] **Paso 11 (RED):** Escribir el smoke test con RTL que renderiza `App` y comprueba la navegación a cada ruta y que se muestra su título.
      `web/src/App.test.tsx` → `expect(await screen.findByRole("heading", { name: /calendar/i })).toBeInTheDocument()`
- [ ] **Paso 12 (GREEN):** Ejecutar el smoke test y completar lo mínimo para que pase.
      `npx vitest run`
- [ ] **Paso 13 (REFACTOR):** Pasar type-check y lint sin errores.
      `npx tsc --noEmit`
      `npm run lint`
- [ ] **Paso 14:** Verificar el contrato de build: compilación a `web/dist/` con rutas relativas.
      `npm run build`

## Criterios de aceptación

- `npm run build` genera `web/dist/` correctamente y sin errores de tipos.
- `npx tsc --noEmit` limpio; `npm run lint` limpio.
- `npx vitest run` en verde (smoke test de layout/rutas).
- `npm run dev` sirve la SPA y el cambio de tema claro/oscuro funciona visualmente.

## Riesgos

- **Disponibilidad/estabilidad de Antd v6:** puede haber cambios de API respecto a v5; revisar la documentación vigente al fijar la versión.
- **Peer deps con React 19:** algunas dependencias del ecosistema Antd pueden no declarar React 19 como peer; posible necesidad de `overrides` o banderas de instalación.
- **Compatibilidad de `base` relativo con react-router:** el enrutado en modo history puede requerir ajustar el `basename` o migrar a `HashRouter` para funcionar servido desde el binario.
- **Tamaño del bundle inicial:** Antd completo puede inflar el bundle; valorar tree-shaking e imports específicos más adelante.

## Referencias

- `plans/PLAN-001-general.md` (F02).
- `AGENTS.md` (orden de build rust-embed, `web/dist`).
