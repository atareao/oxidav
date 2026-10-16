# PLAN-026 — El diálogo de edición, con pestañas (F23)

**Estado:** propuesto (diseño aprobado por el usuario). **Rama prevista:** `feature/dialog-tabs`, desde `development@a7a332a`.

## Objetivo

El detalle de un contacto se lee por pestañas (`ContactDetailTabs`, F22: **8 pestañas**), pero al editarlo aparece un **scroll largo de cinco secciones apiladas con `<Divider>`** (`ContactForm`). La misma información, dos idiomas distintos. F23 alinea el diálogo con el detalle: **el cuerpo del formulario pasa a pestañas con las mismas claves y etiquetas**.

## Alcance

- El cuerpo del diálogo pasa a `<Tabs>` con **cinco pestañas editables**, con las claves y etiquetas que ya usa el detalle: **`names` «Nombres» · `contact` «Contacto» · `addresses` «Direcciones» · `work` «Laboral» · `additional` «Adicional»**.
- **`Nombre` (FN) se queda FUERA de las pestañas**, siempre visible, porque es el único campo obligatorio: dentro de una pestaña oculta, un error de validación sería invisible.
- **Salto a la pestaña del primer error** en `onFinishFailed`: se activa esa pestaña, se enfoca el campo y **no se envía nada**. Las pestañas con error muestran un **punto rojo** en su etiqueta.
- El **payload del PATCH no cambia**: el formulario sigue mandando **solo los grupos modificados** (`buildUpdateInput`).

## Fuera de alcance (decidido, no olvidado)

- **No se añaden pestañas nuevas.** `Control` y `Servidor` del detalle son datos del servidor (no editables) y **`Otros` (`X-*`) sigue siendo solo lectura**: el PATCH **no acepta `others`** —está solo en el DTO de detalle, no en los tipos de escritura—. Hacerlo editable sería otro cambio, con backend.
- **Fotos, logos y sonidos no se editan**: se preservan (acuerdo de F22 sobre binarios).
- **En el formulario las cinco pestañas están siempre**, aunque estén vacías: un formulario enseña campos vacíos. La regla de «ocultar la pestaña sin contenido» es del **detalle** (que enseña datos), y no se traslada.
- No se toca el `Drawer` del detalle, ni el backend, ni los DTO.

## Contratos

- `ContactFormProps` **no cambia**: `{ form: FormInstance<ContactFormValues>; initialValues: ContactFormValues; onFinish: (values: ContactFormValues) => void }`.
- **Nuevo módulo `web/src/components/contact/formTabs.ts`** (nombres orientativos): la **única** fuente de verdad de (a) la lista de pestañas `{ key, label }` en su orden, y (b) el mapa **campo → pestaña** que usa el salto al error. El mapa debe cubrir **todas** las claves de `ContactFormValues` que son editables por el usuario, incluidas las de dentro de `Form.List` (`tels`, `emails`, `impps`, `addresses`, `organizations`, `categories`, `notes`) y las anidadas (`name.*`, `gender.*`). Una clave sin pestaña conocida no puede hacer que el salto falle en silencio: cae a la primera pestaña y **queda cubierto por un test**.
- **El punto rojo** se calcula a partir de los errores del formulario, no de estado duplicado: nada de mantener una copia de los valores.

## Escenarios (BDD)

1. **El diálogo se abre por pestañas**
   Given un contacto en edición, When se abre el diálogo, Then el cuerpo es un `Tabs` con las etiquetas «Nombres», «Contacto», «Direcciones», «Laboral» y «Adicional», y **«Nombres» está activa**; y el campo «Nombre» (FN) está **visible siempre**, fuera de las pestañas.
2. **Cambiar de pestaña no pierde valores**
   Given un teléfono añadido en «Contacto» y una dirección añadida en «Direcciones», When salgo de «Contacto» a «Nombres» y vuelvo, Then las filas siguen ahí, con sus valores, y al enviar viajan en el payload.
3. **Un error en otra pestaña te lleva a él**
   Given un TYPE inválido en un teléfono mientras estoy en «Nombres», When pulso Guardar, Then el formulario **activa «Contacto»**, enfoca el campo del error y **no llama a `onFinish`**.
4. **El error visible no salta de pestaña**
   Given el campo Nombre vacío y estando en cualquier pestaña, When pulso Guardar, Then se muestra el error de «Nombre» (que ya está a la vista) y **no cambia la pestaña activa**.
5. **El payload no cambia**
   Given un contacto con todos los grupos, When modifico **solo** un teléfono y envío, Then el payload es **exactamente** el que producía el formulario antes de F23: solo el grupo `tels`. Y sin tocar nada, el payload es vacío.
6. **Las pestañas con error se señalan**
   Given errores en dos grupos, When intento guardar, Then las dos pestañas correspondientes muestran su punto rojo.

## Tareas (TDD)

1. **RED** — `formTabs.test.ts`: el mapa **cubre todas** las claves de `ContactFormValues` (recorrido del tipo/objeto real, no una lista escrita a mano) y la lista de pestañas tiene las cinco claves esperadas en orden.
2. **RED** — `ContactForm.test.tsx` (ampliar el existente): escenario 1 (estructura de pestañas y FN fuera), escenario 2 (preservación al cambiar de pestaña), escenarios 3 y 4 (salto al error y no-salto), escenario 6 (puntos rojos).
3. **RED** — payload: escenario 5, reutilizando el camino real (`handleSubmit`/`buildUpdateInput`) con un grupo cambiado y con ninguno.
4. **GREEN** — implementar `formTabs.ts` y las pestañas en `ContactForm`, con el `onFinishFailed` que salta y la marca de error por pestaña.
5. **REFACTOR** — sin cambiar el comportamiento; `npm run lint` y `npm run typecheck` limpios.

## Verificación de cierre

1. **174 tests actuales + los nuevos**, todos verdes (`npm run test`), y `npm run typecheck` / `npm run lint` / `npm run build` limpios.
2. **El payload del PATCH, byte a byte**: comparar la salida de `buildUpdateInput` para el mismo caso (un grupo cambiado, y ninguno) antes y después de F23 — debe ser **idéntica**.
3. **Comprobación real en el navegador** (servidor real + SPA): abrir un contacto, **cambiar de pestaña y verificar que los valores siguen**, provocar el error en una pestaña oculta y comprobar que **te lleva a ella** y que al guardar se aplica el cambio. Es la red de seguridad de que las pestañas no rompen la edición.
4. Backend intacto: **551 / 0** (no se toca).
5. Al cerrar: `docs/F23-dialogo-edicion-pestanas.md`, y este plan se renombra a `-COMPLETED.md`.

## Versionado

Es un `feat` → **minor**: la próxima versión pasaría a **0.5.0**.

---

## Cierre (PLAN-026 completado)

Dos commits:

```
7354ebc 📝 docs: añadir PLAN-026 (el diálogo de edición, con pestañas, F23)
135f242 ✨ feat: el diálogo de edición va con pestañas, como el detalle (F23)
```

### Resultado

- El cuerpo del diálogo es un `<Tabs>` con las cinco pestañas del detalle (`names`, `contact`, `addresses`, `work`, `additional`); **`fn` fuera**, siempre visible.
- **Salto a la pestaña del primer error** (la activa, la enfoca y no envía) y **punto rojo con descripción accesible** en las que tengan error.
- **El payload del PATCH no cambia.** `buildUpdateInput`/`groupsFromForm` intactos.
- Backend **sin tocar**.

### Verificación

- **20 ficheros / 217 tests** verdes (eran 174; F23 añade 43); `typecheck`, `lint` y `build` limpios; backend **551 / 0**.
- **RED demostrado con evidencia, no afirmado**: con la implementación anterior en su sitio (restaurada desde una copia, y restituida después verificando el `sha256` byte a byte), **fallan 8 de los 17 tests** del componente. Los dos que no fallan son los del payload: **guarda de contrato**, no prueba de F23 — y así queda dicho.
- **`Contacts.test.tsx`: +11 líneas de navegación, 0 aserciones tocadas** (verificado con el diff: solo `user.click` a la pestaña antes de tocar sus campos).

### Desviaciones y hallazgos de la auditoría

La auditoría de React marcó diez cosas. Se cerraron **cuatro** antes de dar la feature por buena (el color-solo del punto rojo, el `shouldUpdate` que re-renderizaba todo en cada tecla, el fallback de foco dependiente de los ids de AntD —ahora acotado y **fijado con un test**— y el camino de foco sin unificar). Dos detalles se documentan como **deuda** en `docs/F23`: `ContactForm.tsx` ha crecido a 785 líneas (candidato a extraer `TabFields`) y la descripción accesible se fija por DOM porque AntD v6 no reenvía props al botón de pestaña.

Un hallazgo de la auditoría era un **falso positivo** y se dice: señaló el cambio de `compose.yaml` como contaminación de F23. **No lo es**: es una modificación del dueño del repositorio que está en el árbol de trabajo desde antes de F22, y se ha quedado **fuera de todos los commits**. No se revierte: no es de F23, pero tampoco es basura.

### Nota de proceso

Los dos primeros encargos a `frontend-dev` terminaron **en silencio y con la suite en rojo** (6 fallos). Se resolvió **troceando el trabajo en encargos atómicos** con ficheros disjuntos y verificación propia por CLI en cada paso; de ahí que los arreglos se hicieran en cuatro encargos pequeños en vez de dos grandes. Nada se dio por bueno sin comprobarlo.

### Versionado

Es un `feat` → **minor**: la próxima versión sería **0.5.0**.
