# F23 — El diálogo de edición, con pestañas

## Qué entrega

El **detalle** de un contacto se lee por pestañas (`ContactDetailTabs`, F22). El **diálogo de edición** era un scroll largo de cinco secciones apiladas con `<Divider>`. La misma información, dos idiomas distintos. F23 alinea el diálogo con el detalle:

- El cuerpo del formulario es un **`<Tabs>`** con las **mismas claves y etiquetas** que el detalle: **Nombres · Contacto · Direcciones · Laboral · Adicional**.
- **`Nombre` (FN) se queda FUERA de las pestañas**, siempre visible: es el único campo obligatorio, y dentro de una pestaña oculta su error sería invisible.
- **`onFinishFailed` lleva a la pestaña del primer error**, la enfoca y **no envía**. Las pestañas con error llevan punto rojo **y descripción accesible**.
- **El payload del PATCH no cambia**: el formulario sigue mandando **solo los grupos modificados** (`buildUpdateInput`).

## Decisiones de diseño

| Decisión | Por qué |
|---|---|
| Las pestañas del diálogo usan **las claves del detalle** (`names`, `contact`, `addresses`, `work`, `additional`) | Que las dos vistas hablen el mismo idioma es todo el objetivo |
| **`fn` fuera** de las pestañas | Es el único campo obligatorio; oculto, un error de validación no se vería |
| **No se añaden pestañas** | `Control` y `Servidor` son datos del servidor (no editables) y **`Otros` (`X-*`) sigue siendo solo lectura**: el PATCH **no acepta `others`**. Las fotos, logos y sonidos no se editan: se preservan (acuerdo de F22 sobre binarios) |
| En el **formulario** las cinco pestañas están **siempre**, aunque estén vacías | Un formulario enseña campos vacíos. La regla de «ocultar la pestaña sin contenido» es del **detalle**, que enseña datos |
| El punto rojo lleva **`aria-describedby` + `title`** | El color por sí solo no es una señal accesible. Se usa una **descripción**, que **no cambia el nombre accesible** de la pestaña |

## Hallazgos de la auditoría, cerrados

1. **El punto rojo era solo color.** Ahora la pestaña con error lleva `title` y una descripción accesible enlazada con `aria-describedby` (texto `.sr-only` con `id`). **El nombre accesible de la pestaña no cambia** a propósito: otras pruebas consultan `getByRole('tab', { name: 'Contacto' })` con coincidencia exacta.
2. **`shouldUpdate={() => true}` re-renderizaba las cinco pestañas en cada tecla.** Sustituido por `onFieldsChange` con **guard de conjunto**: el estado solo cambia en las transiciones de error. Medido con un espía a `getFieldsError`: **6 llamadas por tecla antes, 0 después**.
3. **El fallback de foco dependía de la convención de ids de AntD** y buscaba en todo el `document`. Ahora la búsqueda está **acotada al nodo del `<Form>`**, y hay un **test que fija la convención** (`tels_0_types`): si AntD la cambia, el test se pone rojo en vez de perder el foco en silencio.
4. **El camino de foco solo se aplicaba al cambiar de pestaña.** Unificado: una sola ruta (instancia + verificación de `activeElement` + fallback acotado) para las dos ramas.

## Verificación

- **20 ficheros / 217 tests verdes**; `typecheck`, `lint` y `build` limpios. Backend intacto (**551 / 0**).
- **El RED está demostrado, no supuesto**: restaurada la implementación anterior (copia de seguridad previa, y restitución verificada **byte a byte** por `sha256`), **fallan 8 de los 17 tests** de `ContactForm.test.tsx`. Los que **no** fallan son los del payload: son **guarda de contrato**, no prueba de F23.
- **El payload del PATCH no cambió**: `buildUpdateInput` y `groupsFromForm` intactos, y sus pruebas siguen comprobando exactamente los mismos objetos.
- **`Contacts.test.tsx`**: **+11 líneas, todas de navegación** (activar la pestaña antes de tocar sus campos), **cero aserciones modificadas**. Es la consecuencia correcta de F23: los paneles inactivos van `hidden` y testing-library respeta el árbol de accesibilidad.

## Deuda que queda (dicha, no escondida)

- **`ContactForm.tsx` ha pasado de 513 a 785 líneas.** F23 le añadió el bloque de pestañas (`TabFields`, las etiquetas y el efecto de descripción). Es el candidato natural a partirse: extraer `TabFields` y las etiquetas a su propio módulo, con lo que además se podría probar pestaña a pestaña. No se hizo aquí para no mezclar un refactor con una feature.
- **La descripción accesible se fija por DOM** en un efecto acotado al nodo del `<Form>`, porque Ant Design v6 **no reenvía props arbitrarias** al botón `role="tab"`. Es un rodeo documentado; conviene revisarlo en una actualización de AntD y quitarlo si deja de hacer falta.
- **`emptyFormValues()` no incluye `birthday` ni `anniversary`** (son opcionales). Por eso el test de cobertura de `formTabs` usa un objeto **completo**, tipado como `Required<ContactFormValues>`: así el compilador y el test fallan si alguien añade un campo sin mapearlo a una pestaña.
