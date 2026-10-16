# PLAN-020 — Rediseño UX transversal del SPA (F19)

## Objetivo

Dar a la SPA un **aspecto y un comportamiento deliberados** —sistema de diseño,
navegación solo en cabecera, responsive real y accesibilidad— en lugar del AntD de
fábrica, **sin tocar backend, esquema ni API**.

## Alcance

Esta feature (F19) es un **rediseño exclusivamente de frontend**. Comprende cuatro
bloques:

- **A — Sistema de diseño**: tokens de tema en el `ConfigProvider`, `index.css`
  reescrito (variables, reset mínimo, utilidades), iconografía real
  (`@ant-design/icons`) en lugar de emoji, y eliminación de los estilos inline de
  layout/espaciado.
- **B — Shell y navegación responsive**: **eliminación del `Sider`**; navegación en la
  cabecera (en línea en escritorio, `Drawer` con hamburguesa en móvil); marca única;
  landmark `<main>`.
- **C — Páginas: densidad y estados**: retirada de los encabezados de página
  redundantes, `h1` oculto visualmente, homogeneización de estados de carga/vacío/error
  y revisión de la responsividad de tablas y anchos fijos.
- **D — Accesibilidad**: `h1` por página, `document.title` por ruta, landmarks,
  `aria-label` en botones de solo icono, labels de formulario, foco visible y contraste
  AA.

**FUERA DE ALCANCE (no incluir en las tareas):**

- **Backend, esquema, rutas DAV/REST y contrato de datos**: no se toca ni una línea.
- **Marca nueva**: no hay logo, ni paleta propia, ni cambio de `colorPrimary` (se
  mantiene el por defecto de AntD). Identidad = «pulido sin marca nueva».
- **Expansión de `RRULE`**.
- **i18n**: el SPA sigue en español; no se introduce infraestructura de traducciones.
- **PWA**: sin manifest, sin service worker, sin modo offline.
- **CORS y proxy**: `web/vite.config.ts` proxya `/api` y `/dav` a
  `http://localhost:8080` y las peticiones son same-origin; **no se añade CORS ni se
  modifica el proxy**.
- **`web/package.json` `"version": "0.0.0"`**: no se versiona; no lo «arregles».
- **Cableado `ThemeProvider` → `ConfigProvider` → `darkAlgorithm`**: es correcto
  (`web/src/AuthenticatedApp.tsx:28-35`, `web/src/contexts/ThemeContext.tsx`); no se
  rehace, solo se **extiende** con tokens.

## Motivación

La auditoría del SPA (verificada contra el repositorio) deja la app funcional pero sin
capa de diseño:

1. **Cero tokens de marca.** `web/src/AuthenticatedApp.tsx:31-35`: el `ConfigProvider`
   solo cambia `algorithm` (claro/oscuro); no hay `theme.token` ni overrides de
   `components`. El aspecto es AntD de fábrica.
2. **Sin iconografía.** `@ant-design/icons` **no está instalado** (`web/package.json`
   no lo lista; 0 imports en `src/`). El menú no tiene iconos y el conmutador de tema
   usa emoji `☀️`/`🌙` en `web/src/components/AppLayout.tsx:99`, que se renderiza
   distinto según el sistema operativo.
3. **Responsive frágil.** **Cero** media queries en todo el proyecto. Solo existe el
   `breakpoint="lg"` del `Sider` (`web/src/components/AppLayout.tsx:58`). Las 3 tablas
   usan `scroll={{ x: 'max-content' }}` (`web/src/pages/Sync.tsx:344`,
   `web/src/pages/Contacts.tsx:591`, `web/src/pages/Settings.tsx:361`) y hay anchos
   fijos: `minWidth: 200` (`web/src/pages/Contacts.tsx:534`,
   `web/src/pages/Calendar.tsx:430`), `width: 220` (`Contacts.tsx:551`),
   `minWidth: 700` (`Calendar.tsx:473`).
4. **Identidad duplicada.** «OxiDav» aparece simultáneamente en el `Sider`
   (`web/src/components/AppLayout.tsx:71`, texto plano con `color: '#fff'` hardcodeado)
   y en el `Header` (`AppLayout.tsx:91`).
5. **Estilos ad-hoc.** 39 usos de `style={{}}` inline en TSX. `web/src/index.css` tiene
   16 líneas y es prácticamente el boilerplate de Vite: sin escala de espaciado, sin
   tipografía, sin utilidades.
6. **Accesibilidad fina.** Solo 25 apariciones de `aria-`/`role=`/`alt=` en ~3.700
   líneas de TSX.
7. **`color-scheme: light dark` descoordinado.** `web/src/index.css:2` declara
   `color-scheme: light dark`, así que el **navegador** elige claro/oscuro según el
   sistema operativo, ignorando el conmutador de la app: si la app está en oscuro y el
   SO en claro, el `body` y AntD discrepan. La paleta la controla
   `web/src/contexts/ThemeContext.tsx`, que persiste en `localStorage` bajo la clave
   `oxidav.theme`.
8. **Estados de carga/vacío inconsistentes.** `Empty` aparece en
   `web/src/pages/Calendar.tsx:465`, `Contacts.tsx:576/585`, `Settings.tsx:359`,
   `Sync.tsx:342`, pero con formas distintas: en Calendar es un `Empty` suelto junto a
   un `Spin` (`Calendar.tsx:462`) y en las demás va como `emptyText` dentro de la tabla.
9. **Título de documento estático.** `web/index.html` ya tiene `lang="es"` y
   `<title>OxiDav</title>`. Falta actualizar el `document.title` por ruta (p. ej.
   «Calendario · OxiDav»).

### Lo que NO está roto (no lo «arregles»)

- El cableado `ThemeProvider` → `ConfigProvider` → `darkAlgorithm` es correcto
  (`web/src/AuthenticatedApp.tsx:28-35`, `web/src/contexts/ThemeContext.tsx`).
- `web/src/components/AppLayout.test.tsx` **no menciona el Sider ni el menú**: solo
  prueba el control de cierre de sesión. Quitar el Sider no rompe ese test.
- El `getByText('Calendario')` de `web/src/pages/Settings.test.tsx:142` es una
  **etiqueta de tipo de colección** dentro de una tabla, no el encabezado de página.
  Quitar los encabezados no rompe los tests de páginas.

## Decisiones vinculantes

1. **Fuera el Sider.** La navegación pasa a la cabecera (`Header`). En escritorio, los
   4 items van en línea en el header. En móvil, el header muestra marca + botón
   hamburguesa y el menú se abre en un `Drawer` de AntD.
2. **Se eliminan los encabezados de página** `Typography.Title level={2}` de las 4
   vistas (Calendario, Contactos, Sincronización, Ajustes) por redundantes con la
   navegación.
3. **Cada página conserva un `h1` real en el DOM pero oculto visualmente** (clase
   utilitaria tipo `sr-only`). Cero redundancia visual, estructura semántica intacta.
   **Decisión cerrada: no se proponen alternativas.**
4. **Bloques en alcance: los cuatro** — A (sistema de diseño), B (shell y navegación
   responsive), C (páginas: densidad y estados), D (accesibilidad).
5. **Identidad: «pulido sin marca nueva».** Se mantiene el `colorPrimary` por defecto
   de AntD y no se introduce logo ni paleta propios. Se afinan radios, tipografía,
   densidades y espaciado.
6. **Sin cambios en la API ni en el backend.** El contrato de datos de la SPA no se
   toca.
7. **Detección responsive con la API de AntD** (`Grid.useBreakpoint()`), no con media
   queries sueltas, para no salirse del sistema de diseño.
8. **`color-scheme` sigue el modo real de la app**, empujado desde
   `ThemeContext`/un efecto sobre `<html>`, **no** el del sistema operativo.

## Contratos que NO se pueden romper (crítico)

- **El botón de cerrar sesión debe conservar su nombre accesible** que case con
  `/cerrar sesión/i` **y** su atributo `title="Cerrar sesión"`:
  `web/src/components/AppLayout.test.tsx` lo busca con
  `getByRole('button', { name: /cerrar sesión/i })` y comprueba
  `toHaveAttribute('title', 'Cerrar sesión')`. Además debe seguir mostrando `loading` y
  `disabled` cuando `loggingOut` es `true`.
- **`AppLayout` debe seguir siendo renderizable sin `<AuthProvider>`**: lee
  `AuthContext` directamente con `useContext` (no `useAuth()`) y solo muestra el
  control de cierre cuando hay sesión (`web/src/components/AppLayout.tsx:46-49`). Hay
  tests de layout que no montan el provider.
- **El `Suspense` con `RouteFallback` dentro del contenido** (`AppLayout.tsx:121`) debe
  permanecer: mantiene el shell montado mientras se descarga el chunk de la página
  perezosa.
- **`web/index.html` no debe perder `lang="es"`.**
- **`web/package.json` mantiene `"version": "0.0.0"`** y **no se versiona**.
- **No añadir CORS ni tocar el proxy** (`web/vite.config.ts`).

## Prerequisitos (fuera del código)

- Rama `feature/frontend-redesign` creada desde `development` (Git Flow del repo).
- Commits con **Conventional Commits + gitmoji** (p. ej.
  `✨ feat: rediseño UX transversal del SPA`).
- Trabajar con el toolchain del repo: **Node 22** (el `Dockerfile` fija
  `node:22-alpine3.23`). Todos los comandos de este plan se ejecutan **desde `web/`**.
- **Medir el tamaño de `web/dist` antes y después** (no hay budget de tamaño en la CI).
- Recordar el **orden de build**: `web/dist` se compila **antes** de `cargo build`
  (`rust-embed` lee los assets en tiempo de compilación). En release, `build.rs` aborta
  si falta `web/dist/index.html`.

## Contratos / Interfaces

### `web/package.json`

Añadir `@ant-design/icons` a `dependencies` con la versión **`^6.3.4`** (sus peer deps
son solo `react`/`react-dom >= 16`, así que encaja con React 19). Mantener
`"version": "0.0.0"`.

### `web/src/AuthenticatedApp.tsx` (extender, no rehacer)

El `ConfigProvider` gana `theme.token` (radios, tipografía, densidad, alturas de
control) y overrides puntuales en `theme.components`, **manteniendo el cambio de
`algorithm` claro/oscuro** que ya existe. `colorPrimary` se queda por defecto.

```tsx
<ConfigProvider
  theme={{
    algorithm: isDark ? theme.darkAlgorithm : theme.defaultAlgorithm,
    token: { /* radios, fontFamily, controlHeight, ... */ },
    components: { /* Layout, Menu, Card, Table, ... */ },
  }}
>
```

### `web/src/index.css` (reescritura)

- Variables CSS (escala de espaciado, tipografía, radios).
- Reset mínimo.
- Utilidades, **incluida `sr-only`** (la que usan las 4 páginas para su `h1` oculto).
- **Sustituir `color-scheme: light dark`** por un `color-scheme` que **siga el modo
  real de la app**, empujado desde `ThemeContext`/un efecto sobre `<html>`.

### `web/src/components/AppLayout.tsx` (reescritura)

- **Eliminar el `Sider`** (`AppLayout.tsx:58`).
- Header con **marca única** «OxiDav» a la izquierda, **como enlace, no como
  encabezado** (el `h1` vive oculto en cada página).
- Navegación en línea en escritorio; `Drawer` + botón hamburguesa en móvil.
- Controles de tema y cierre como **botones con icono** (con `aria-label`).
- Detección responsive con `Grid.useBreakpoint()`.
- Landmark `<main>`.
- **Conservar** el `Suspense` + `RouteFallback` (`AppLayout.tsx:121`) y el patrón
  `useContext(AuthContext)` con la guarda de sesión (`AppLayout.tsx:46-49`).

### `web/src/components/AppLayout.test.tsx` (actualizar)

Añadir tests del nuevo comportamiento (items de navegación, estado activo, apertura del
drawer) **sin romper los tests de cierre de sesión existentes** ni los contratos de
accesibilidad de arriba.

### Páginas — `Calendar.tsx`, `Contacts.tsx`, `Sync.tsx`, `Settings.tsx`

- Quitar `Typography.Title level={2}` e introducir el `h1` oculto visualmente con **el
  mismo enfoque idéntico en las 4 páginas**.
- Homogeneizar estados de carga / vacío / error (mismo patrón para `Empty`,
  `Spin`/`Skeleton` y el `Alert` de error con botón «Reintentar»).
- Revisar la responsividad de las tablas y los anchos fijos del hallazgo 3.
- Unificar el espaciado (hoy `marginBottom: 16` repetido).

## Dependencias

- **Frontend**: `@ant-design/icons@^6.3.4` (nueva dependencia de producción).
- No hay cambios en `Cargo.toml` ni en `Cargo.lock` (feature exclusivamente de
  frontend).
- `web/package-lock.json` se actualiza al instalar; commitearlo con el cambio.

## Testing (red-green-refactor)

- **layout (shell)**: la navegación contiene los 4 items; el item de la ruta activa
  queda marcado como activo; el botón hamburguesa abre el `Drawer` y este lista los
  mismos items.
- **contratos de accesibilidad (regresión)**: el botón de cierre de sesión sigue
  accesible por `name: /cerrar sesión/i`, con `title="Cerrar sesión"`, y sigue en
  `loading`/`disabled` cuando `loggingOut` es `true`; `AppLayout` sigue renderizando sin
  `<AuthProvider>`.
- **tema**: el `ConfigProvider` aplica `theme.token` y conserva el `algorithm`
  claro/oscuro; el `color-scheme` del documento sigue el modo de la app.
- **páginas**: cada una de las 4 vistas expone **un `h1` en el DOM** (oculto
  visualmente) y **no** renderiza el `Typography.Title level={2}` anterior.
- **estados**: cada vista presenta un patrón homogéneo de carga / vacío / error, y el
  `Alert` de error ofrece un botón «Reintentar» operativo.
- **documento**: `document.title` cambia por ruta (p. ej. «Calendario · OxiDav») y
  vuelve al valor base al desmontar.
- **iconografía**: los controles de tema usan iconos de `@ant-design/icons` (no
  emoji).

## Tareas (TDD)

### Fase 1 — Dependencia e iconografía (Bloque A)

- [ ] **Paso 1:** Instalar la dependencia y fijarla en `package.json`.
      Desde `web/`: `npm install @ant-design/icons@^6.3.4`
- [ ] **Paso 2:** **Verificar el nombre real del export** de los iconos que se van a
      usar (p. ej. `SunOutlined` / `MoonOutlined` / `BulbOutlined`) **contra la versión
      instalada** antes de importarlos; no darlos por supuestos de memoria.
      `node -e "const i=require('@ant-design/icons'); console.log(Object.keys(i).filter(k=>/Sun|Moon|Bulb/.test(k)))"`
- [ ] **Paso 3 (RED):** Test (vitest) de que el conmutador de tema renderiza un icono
      de `@ant-design/icons` y **no** el emoji `☀️`/`🌙`. `npm run test`
- [ ] **Paso 4 (GREEN):** Sustituir los emoji de `web/src/components/AppLayout.tsx:99`
      por los iconos verificados (con `aria-label`). `npm run test`

### Fase 2 — Sistema de diseño (Bloque A)

- [ ] **Paso 5 (RED):** Test de que el `ConfigProvider` recibe `theme.token` (valores
      no vacíos) y **mantiene** el cambio de `algorithm` claro/oscuro.
      `npm run test`
- [ ] **Paso 6 (GREEN):** Extender `web/src/AuthenticatedApp.tsx:31-35` con
      `theme.token` (radios, tipografía, densidad, alturas de control) y overrides
      puntuales en `theme.components`, sin tocar `colorPrimary`.
      `npm run test`
- [ ] **Paso 7 (RED):** Test de que el `color-scheme` del documento sigue el modo real
      de la app (claro/oscuro) y **no** el del sistema operativo.
- [ ] **Paso 8 (GREEN):** Reescribir `web/src/index.css`: variables CSS, reset mínimo,
      utilidades (incluida `sr-only`), y sustituir `color-scheme: light dark`
      (`index.css:2`) por un `color-scheme` empujado desde
      `web/src/contexts/ThemeContext.tsx` / un efecto sobre `<html>`.
      `npm run test`
- [ ] **Paso 9 (REFACTOR):** Sustituir los `style={{}}` inline que sean de
      layout/espaciado por props de AntD (`Space`, `Flex`, `Card`) o clases CSS. Los
      estilos con valores dinámicos pueden quedarse. `npm run lint`

### Fase 3 — Shell y navegación responsive (Bloque B)

- [ ] **Paso 10 (RED):** Test de layout: la navegación del header contiene los 4 items
      (Calendario, Contactos, Sincronización, Ajustes) y marca el activo.
      `npm run test`
- [ ] **Paso 11 (RED):** Test de layout: el botón hamburguesa (visible en móvil) abre
      el `Drawer` y este lista los mismos items.
- [ ] **Paso 12 (RED):** Tests de regresión de los contratos críticos: el control de
      cierre de sesión sigue accesible por `name: /cerrar sesión/i` con
      `title="Cerrar sesión"` y `loading`/`disabled` cuando `loggingOut`; `AppLayout`
      renderiza sin `<AuthProvider>`.
- [ ] **Paso 13 (GREEN):** Reescribir `web/src/components/AppLayout.tsx`: eliminar el
      `Sider` (`AppLayout.tsx:58`), header con marca única «OxiDav» como enlace,
      navegación en línea en escritorio, `Drawer` + hamburguesa en móvil con
      `Grid.useBreakpoint()`, controles con icono, landmark `<main>`. **Conservar** el
      `Suspense` + `RouteFallback` (`AppLayout.tsx:121`) y la lectura de `AuthContext`
      con `useContext` (`AppLayout.tsx:46-49`). `npm run test`
- [ ] **Paso 14 (REFACTOR):** Revisar el header en los tres anchos de referencia y
      limpiar los estilos inline residuales del layout. `npm run lint`

### Fase 4 — Páginas: densidad y estados (Bloque C)

- [ ] **Paso 15 (RED):** Test por página: cada una de las 4 vistas expone **un `h1` en
      el DOM** (oculto visualmente) y **no** renderiza el `Typography.Title level={2}`
      anterior. `npm run test`
- [ ] **Paso 16 (RED):** Test por página: el estado de vacío usa un patrón homogéneo y
      el `Alert` de error ofrece un botón «Reintentar» que dispara la recarga.
- [ ] **Paso 17 (GREEN):** Quitar los encabezados de página e introducir el `h1` oculto
      (misma clase `sr-only`, mismo enfoque en las 4) en
      `web/src/pages/Calendar.tsx`, `Contacts.tsx`, `Sync.tsx`, `Settings.tsx`.
      Homogeneizar `Empty`/`Spin`/`Skeleton`/`Alert` en las 4. `npm run test`
- [ ] **Paso 18 (GREEN):** Revisar la responsividad: tablas con
      `scroll={{ x: 'max-content' }}` (`Sync.tsx:344`, `Contacts.tsx:591`,
      `Settings.tsx:361`) y anchos fijos `minWidth: 200`
      (`Contacts.tsx:534`, `Calendar.tsx:430`), `width: 220` (`Contacts.tsx:551`),
      `minWidth: 700` (`Calendar.tsx:473`). Unificar el espaciado (`marginBottom: 16`
      repetido). `npm run test`
- [ ] **Paso 19 (REFACTOR):** `npm run lint` y `npm run typecheck` limpios; limpiar
      duplicación de estilos entre páginas.

### Fase 5 — Accesibilidad (Bloque D)

- [ ] **Paso 20 (RED):** Test de que `document.title` cambia por ruta (p. ej.
      «Calendario · OxiDav») y vuelve al valor base al desmontar.
- [ ] **Paso 21 (RED):** Test de que todo botón de solo icono expone `aria-label` y
      todo control de formulario tiene label asociado.
- [ ] **Paso 22 (GREEN):** Implementar el `document.title` por ruta, los `aria-label`
      de botones de icono, los labels de formulario y los landmarks correctos (header
      de navegación + `<main>`). `npm run test`
- [ ] **Paso 23 (GREEN):** Garantizar **foco visible** (que el CSS propio no lo
      elimine) y **navegación por teclado** del `Drawer` y de la navegación; comprobar
      contraste AA con los tokens elegidos. `npm run test`
- [ ] **Paso 24 (REFACTOR):** `npm run lint`, `npm run typecheck` en verde.

### Fase 6 — Verificación

- [ ] **Paso 25 (REFACTOR):** Desde `web/`: `npm run test`, `npm run typecheck`,
      `npm run lint`, `npm run build` en verde.
- [ ] **Paso 26:** **Medir el delta de tamaño del build** (`web/dist`) antes/después
      de añadir `@ant-design/icons` y registrarlo (no hay budget en la CI).
- [ ] **Paso 27:** Compilar el frontend **antes** del backend (`web/dist` → `cargo
      build`) y hacer un smoke rápido de que el binario sigue sirviendo la SPA con
      `lang="es"` y `<title>OxiDav</title>`.
- [ ] **Paso 28:** Crear `docs/F19-frontend-redesign.md` con el sistema de diseño, el
      shell responsive y las decisiones de accesibilidad.

## Criterios de aceptación

- **El `Sider` ha desaparecido** de `web/src/components/AppLayout.tsx` y la navegación
  vive en la cabecera: 4 items en línea en escritorio; marca + hamburguesa + `Drawer`
  en móvil.
- **Las 4 páginas** (Calendario, Contactos, Sincronización, Ajustes) exponen **un `h1`
  real en el DOM oculto visualmente** con el mismo enfoque, y **ya no** renderizan
  `Typography.Title level={2}`.
- **Los estados de carga/vacío/error son homogéneos** en las 4 vistas y el error ofrece
  un botón «Reintentar» operativo.
- **`document.title` cambia por ruta** (p. ej. «Calendario · OxiDav») y `web/index.html`
  conserva `lang="es"`.
- **Todos los botones de solo icono tienen `aria-label`** y los controles de formulario
  tienen label asociado; el foco es visible y el `Drawer`/la navegación son navegables
  por teclado.
- **`@ant-design/icons@^6.3.4`** está en `web/package.json` y **no queda ningún emoji**
  como icono.
- **El `ConfigProvider` aplica `theme.token`** y **conserva** el cambio de `algorithm`
  claro/oscuro, con `colorPrimary` por defecto.
- **El `color-scheme` del documento sigue el modo real de la app**, no el del sistema
  operativo.
- **Contratos críticos intactos**: el botón de cerrar sesión mantiene `name:
  /cerrar sesión/i` y `title="Cerrar sesión"` (con `loading`/`disabled` bajo
  `loggingOut`); `AppLayout` renderiza sin `<AuthProvider>`; el `Suspense` +
  `RouteFallback` sigue en el contenido.
- **Sin cambios** en backend, esquema, rutas DAV/REST ni contrato de datos; **sin CORS**
  ni cambios en el proxy.
- **CI verde**: desde `web/`, `npm run test`, `npm run typecheck`, `npm run lint`,
  `npm run build`.
- **Delta de tamaño del build medido** antes/después y documentado.
- `docs/F19-frontend-redesign.md` creado.

## Riesgos

- **`@ant-design/icons` engorda el bundle.** **No hay budget de tamaño en la CI**:
  `web/vite.config.ts` solo separa un chunk `react-vendor` y **AntD no está separado**,
  así que los iconos entran en el bundle principal sin control. Mitigación: **medir el
  delta de tamaño del build antes/después** (Paso 26) e importar solo los iconos que se
  usan (imports nombrados, nunca el paquete completo).
- **`color-scheme` mal sincronizado.** Puede producir un **flash** o un fondo
  descoordinado al cargar (el navegador pinta antes de que React fije el modo).
  Mitigación: empujar `color-scheme` desde `ThemeContext`/un efecto sobre `<html>` y
  verificar visualmente la carga en claro y en oscuro con el SO en el modo contrario.
- **APIs de AntD v6 distintas de las de v4/v5.** El proyecto usa **Ant Design v6** y el
  implementador puede recordar de memoria la v4/v5. Mitigación: **verificar cada API
  contra la versión instalada** (props de `Layout`, `Menu`, `Drawer`, `Grid.useBreakpoint`,
  `theme.token`/`theme.components`) antes de escribir código; consultar la
  documentación de la versión, no la memoria.
- **Regresión en los contratos de accesibilidad y de layout.** Quitar el Sider y
  reescribir `AppLayout` puede alterar el botón de cerrar sesión o romper los tests de
  layout sin provider. Mitigación: tests de regresión explícitos (Paso 12) que
  congelan `name: /cerrar sesión/i`, `title="Cerrar sesión"` y el render sin
  `<AuthProvider>`.
- **Tests de páginas que dependen de textos.** `Settings.test.tsx:142` busca
  `'Calendario'`, pero es una etiqueta de tabla, no el encabezado; aun así, al tocar
  las páginas conviene re-ejecutar la suite completa para detectar acoplamientos
  ocultos a los encabezados retirados.
- **Responsive sin media queries.** Delegar todo en `Grid.useBreakpoint()` es
  consistente con el sistema, pero deja fuera comportamientos que AntD no cubre.
  Mitigación: verificar visualmente en 375/768/1440 px (Paso de verificación final).

## Verificación final

- Desde `web/`: `npm run test`, `npm run typecheck`, `npm run lint`, `npm run build` en
  verde.
- **Suite de tests del frontend**: los **85 tests de base** siguen verdes y la suite
  **crece** con los nuevos tests de layout, tema, páginas y accesibilidad.
- **Comprobación visual en al menos tres anchos**: **375 px** (móvil, con Drawer),
  **768 px** (tablet) y **1440 px** (escritorio, navegación en línea), en modo claro y
  oscuro.
- Confirmación de que `web/index.html` conserva `lang="es"` y de que el orden de build
  (`web/dist` antes de `cargo build`) se respeta.

## Referencias

- `plans/PLAN-019-timezone-support-COMPLETED.md` — estructura y tono de la casa.
- `plans/PLAN-018-oidc-valet-parity-COMPLETED.md` — estructura y estilo del plan.
- `web/src/AuthenticatedApp.tsx` — `ConfigProvider` (líneas 28-35).
- `web/src/contexts/ThemeContext.tsx` — paleta y persistencia (`oxidav.theme`).
- `web/src/components/AppLayout.tsx` — `Sider` (línea 58), marca duplicada (líneas
  71/91), emoji de tema (línea 99), guarda de sesión (líneas 46-49), `Suspense`
  (línea 121).
- `web/src/components/AppLayout.test.tsx` — tests de cierre de sesión.
- `web/src/pages/{Calendar,Contacts,Sync,Settings}.tsx` — encabezados, tablas y estados.
- `web/src/index.css` — `color-scheme: light dark` (línea 2).
- `web/index.html` — `lang="es"` y `<title>OxiDav</title>`.
- `web/vite.config.ts` — proxy `/api` y `/dav`; chunk `react-vendor`.
- `AGENTS.md` — flujo OpenSpec/TDD, orden de build del frontend y convenciones del SPA.
