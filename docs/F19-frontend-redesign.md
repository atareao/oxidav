# F19 — Rediseño UX transversal del SPA

> El plan de origen es [`plans/PLAN-020-frontend-redesign.md`](../plans/PLAN-020-frontend-redesign.md).
> Es una feature **exclusivamente de frontend**: no toca el backend, ni el
> esquema, ni las rutas DAV/REST, ni el contrato de datos.

## Qué implementa

Un repaso transversal de la experiencia del SPA de OxiDav (`web/`, React 19 +
Ant Design v6 + Vite + TypeScript) en cuatro frentes:

1. Un **sistema de diseño** mínimo y coherente (tokens de tema + hoja de estilos
   propia con variables, reset y utilidades).
2. Un **shell responsive** sin `Sider`: la navegación vive en la cabecera.
3. Un **conjunto homogéneo de páginas**: ritmo vertical unificado, anchos
   fluidos, estados de error/carga/vacío consistentes y jerarquía semántica
   correcta.
4. **Accesibilidad**: `document.title` por ruta, etiquetas en los controles de
   solo icono, `label` en los formularios, foco visible y landmarks.

La decisión de estilo declarada fue «**pulido sin marca nueva**»: no se
introdujo logo, ni paleta propia, ni `colorPrimary` personalizado.

## El bug corregido: `color-scheme`

`web/src/index.css` declaraba `color-scheme: light dark`, lo que **delegaba la
decisión al sistema operativo** (vía `prefers-color-scheme`) e **ignoraba el
conmutador de tema de la app**. El síntoma era una discrepancia real: con la app
en oscuro y el SO en claro, el `body` y los componentes de Ant Design quedaban
en esquemas distintos.

Ahora:

- El CSS fija `color-scheme: light` como valor **por defecto**, que es el que
  corresponde a la pantalla de **login**, renderizada **fuera** del
  `ThemeProvider`.
- `ThemeContext` **empuja el modo real de la app** sobre `<html>` mediante un
  efecto.

El arreglo está cubierto por un test que simula un SO en oscuro y comprueba que
la app hace caso a **su propio** conmutador, no al del sistema.

## Sistema de diseño

### Dependencia

Se añade `@ant-design/icons@^6.3.4`. Solo se importan **8 iconos** con imports
nombrados (nunca el paquete entero):

`SunOutlined`, `MoonOutlined`, `MenuOutlined`, `CalendarOutlined`,
`ContactsOutlined`, `SyncOutlined`, `SettingOutlined`, `LogoutOutlined`.

### Tema (`web/src/AuthenticatedApp.tsx`)

`ConfigProvider` recibe `theme.token` y `theme.components`, **manteniendo** el
cambio de `algorithm` claro/oscuro que ya existía.

`token`:

| Token | Valor |
| --- | --- |
| `borderRadius` | `10` |
| `fontFamily` | pila de sistema (`system-ui`, `-apple-system`, `'Segoe UI'`, `Roboto`, …) |
| `fontSize` | `14` |
| `controlHeight` | `36` |
| `lineWidth` | `1` |
| `wireframe` | `false` |

`components`:

| Componente | Tokens |
| --- | --- |
| `Layout` | `headerHeight: 56`, `headerPadding: '0 16px'` |
| `Menu` | `itemBorderRadius: 8`, `itemHeight: 40` |
| `Card` | `bodyPadding: 20`, `headerHeight: 48` |
| `Table` | `cellPaddingBlock: 10` |
| `Button` | `contentFontSize: 14` |

**`colorPrimary` se deja en el valor por defecto de Ant Design.** Es coherente
con la decisión de «pulido sin marca nueva» descrita arriba.

### Hoja de estilos (`web/src/index.css`)

El fichero era prácticamente el boilerplate de Vite (16 líneas). Se reescribió
para aportar:

- **Variables CSS**: escala de espaciado en múltiplos de 4 px (hasta 48 px),
  radios de 6 / 10 / 14 px y píldora, familias tipográficas y altura de línea.
- Un **reset mínimo**.
- **Utilidades**, entre ellas `.sr-only` (para los `h1` ocultos) y `.page`.

La utilidad `.page` lleva una **media query** (`@media (min-width: 768px)`) para
regular su espaciado. Es un matiz deliberado: el plan reservó los breakpoints de
Ant Design **para las decisiones de renderizado** —y esas siguen tomándose con
`Grid.useBreakpoint()`—, mientras que esta media query es puramente una utilidad
de espaciado CSS que **no decide qué se renderiza**.

## Shell y navegación (sin `Sider`)

- **Se eliminó por completo el `Sider` lateral.** La navegación vive ahora en la
  cabecera.
- La marca «OxiDav» aparece **una sola vez** (antes estaba duplicada en el
  `Sider` y el `Header`). Es un **enlace a `/calendar`**, no un encabezado.
- **Escritorio:** los 4 items —Calendario, Contactos, Sincronización, Ajustes—
  en línea, con icono y el activo resaltado, dentro de un
  `<nav aria-label="Navegación principal">`.
- **Móvil:** botón hamburguesa (`MenuOutlined`) que abre un `Drawer` de Ant
  Design con los mismos items.
- El **conmutador de tema** y el **cierre de sesión** pasan a ser botones **de
  solo icono** con `aria-label`. Los emoji `☀️`/`🌙` del conmutador se
  sustituyeron por iconos.
- **Detección responsive** con la API de Ant Design (`Grid.useBreakpoint()`), no
  con media queries sueltas. Regla explícita: el modo móvil se activa **solo
  cuando `xs === true`**, porque en un viewport de escritorio todos los
  breakpoints llegan como `false`/`undefined` y cualquier condición basada en
  «falsy» clasificaría el escritorio como móvil.
- El landmark `<main>` lo aporta `Layout.Content` de Ant Design.
- Se conservaron el `Suspense` + `RouteFallback` del contenido (mantienen el
  shell montado mientras se descarga el chunk de la página perezosa) y la
  lectura de `AuthContext` con `useContext` directo, para que el layout siga
  siendo renderizable sin `<AuthProvider>`.

## Páginas

- Se **retiraron los encabezados `Typography.Title level={2}`** de las 4 vistas
  por redundantes con la navegación.
- Para no dejar el documento sin estructura, cada vista expone ahora un **`h1`
  real en el DOM pero oculto visualmente** con la clase `sr-only`:
  «Calendario», «Contactos», «Sincronización y dispositivos» y «Ajustes». Al
  estar oculto por `clip-path` (y **no** por `display:none` ni `aria-hidden`),
  **sigue presente en el árbol de accesibilidad** y los lectores de pantalla
  conservan la estructura.
- **Ritmo vertical unificado:** los bloques de nivel superior de cada página se
  apilan con `<Flex vertical gap={PAGE_GAP}>`, con `PAGE_GAP = 'middle'`
  definido una sola vez en `web/src/pages/pageLayout.ts`. Desaparecen los
  `marginBottom: 16` repetidos.
- **Anchos fluidos:** los anchos fijos de los controles de barra de herramientas
  (`minWidth: 200`, `width: 220`) se sustituyen por `fluidControlStyle`
  (`width: clamp(150px, 50vw, 240px)`), también definido en `pageLayout.ts`.
- Las **3 tablas** mantienen `scroll={{ x: 'max-content' }}`. La vista de
  Calendario **no tiene `Table`** (su lista se renderiza en un `Card`), así que
  no aplica.
- **Estados homogéneos:**
  - error mediante un `<Alert type="error" showIcon>` con botón «Reintentar»
    operativo;
  - carga de la lista principal mediante la prop `loading` del componente;
  - vacío mediante `emptyText` con `<Empty>`.
  - Se conservan los estados propios del selector de colecciones, que pertenecen
    a otro control.
- **Prop deprecada corregida:** Ant Design v6 deprecó `message` en `Alert` en
  favor de **`title`**. Calendario y Contactos usaban `message`; se migraron a
  `title` (Sincronización y Ajustes ya usaban `title`).

## Accesibilidad

- **`document.title` por ruta**, mediante el hook `web/src/hooks/useDocumentTitle.ts`
  (con `titleForPath`):

  | Ruta | Título |
  | --- | --- |
  | `/calendar` | Calendario · OxiDav |
  | `/contacts` | Contactos · OxiDav |
  | `/sync` | Sincronización · OxiDav |
  | `/settings` | Ajustes · OxiDav |
  | cualquier otra | OxiDav |

  Al desmontar se **restaura** el título base `OxiDav`, que es el que fija
  `web/index.html`.
- Los **3 únicos botones de solo icono** del proyecto (hamburguesa, tema, cierre
  de sesión) exponen `aria-label`.
- Los **11 `Form.Item`** de las 4 vistas tienen `label` asociado.
- **El foco visible no se anula**: no hay ninguna regla `outline: none` en el
  CSS propio.
- **Landmarks** presentes: `<header>`, `<nav aria-label="Navegación principal">`
  y `<main>`.

## Verificación

Tests del frontend: **112 en verde (12 ficheros)**, partiendo de una línea base
de **85**.

```bash
# Frontend (desde web/)
npm run typecheck
npm run lint
npm run build
```

Los tres comandos quedan limpios.

### Delta del bundle

| Métrica | Antes | Después | Delta |
| --- | --- | --- | --- |
| `web/dist` | 1 257 547 B | 1 271 004 B | **+13 457 B (+1,07 %)** |

El coste corresponde a los 8 iconos usados. El plan lo había señalado como
**riesgo** porque la CI **no tiene budget de tamaño**, así que la cifra queda
registrada aquí.

## Notas de implementación (desviaciones del plan)

- **El Paso 4 del plan** —sustituir los emoji del conmutador de tema— se **plegó
  dentro de la reescritura del shell**, porque el plan lo situaba en un fichero
  que otra fase reescribía por completo; hacerlo antes habría sido trabajo
  tirado.
- **`index.css` incorpora una media query** (`@media (min-width: 768px)`) para
  la utilidad de espaciado `.page`. Matiz importante: el plan decidió usar los
  breakpoints de Ant Design **para las decisiones de renderizado**, y eso se
  cumple; la media query es puramente una utilidad de espaciado CSS y **no
  decide qué se renderiza**.

## Limitaciones / fuera de alcance de F19

- **El SPA sigue sin selector de zona horaria**: la zona se fija al crear el
  evento. Es una limitación **heredada de F18** y **no** se abordó en F19.

## Relacionado

- [`plans/PLAN-020-frontend-redesign.md`](../plans/PLAN-020-frontend-redesign.md)
- [`docs/F18-timezone-support.md`](./F18-timezone-support.md) — limitación del selector de zona horaria heredada
- [`AGENTS.md`](../AGENTS.md) — stack del frontend (React 19 + Ant Design v6 + Vite + TypeScript)
