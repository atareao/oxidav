/**
 * Formulario de alta/edición de contactos (F22/F23).
 *
 * Cubre todos los campos **no binarios** del detalle, organizados por las mismas
 * pestañas que la vista (`ContactDetailTabs`, F22): `Nombres`, `Contacto`,
 * `Direcciones`, `Laboral` y `Adicional`. El cuerpo se hace desplazable desde el
 * `Modal` que lo monta.
 *
 * `Nombre` (FN) va **fuera** de las pestañas, siempre visible: es el único campo
 * obligatorio y, dentro de una pestaña oculta, un error de validación sería
 * invisible. Al pulsar Guardar con errores, `onFinishFailed` lleva a la pestaña
 * del **primer** error y enfoca el campo; las pestañas con error llevan un punto
 * rojo. La pestaña activa es estado local (no viaja en `ContactFormProps`).
 *
 * Los grupos multivalor con `TYPE`/`PREF` (`TEL`/`EMAIL`/`IMPP`) y las
 * direcciones (`ADR`) usan `Form.List` para poder añadir, quitar y editar filas.
 * Los paneles inactivos **no se desmontan** (por defecto en Ant Design), así que
 * los valores de los `Form.List` sobreviven al cambiar de pestaña. Los binarios
 * (`PHOTO`/`LOGO`/`SOUND`) **no** aparecen: se muestran y se preservan, no se
 * editan.
 *
 * El componente es **presentacional**: recibe la instancia del formulario y los
 * valores iniciales, y entrega los valores por `onFinish`. Toda la lógica de
 * diff/conversión vive en {@link file://./contactFormValues.ts}.
 */

import { Button, Card, Flex, Form, Input, InputNumber, Select, Space, Tabs, Typography } from 'antd'
import type { FormInstance, FormProps, TabsProps } from 'antd'
import { useEffect, useRef, useState } from 'react'
import type { ContactFormValues } from './contactFormValues'
import {
  CONTACT_FORM_TABS,
  DEFAULT_CONTACT_FORM_TAB,
  tabErrors,
  tabForField,
  type ContactFormTabKey,
  type FieldNamePath,
} from './formTabs'

/** Alfabeto que acepta el servidor para un *token* `TYPE` (decisión 5). */
const TYPE_PATTERN = /^[A-Za-z0-9-]{1,32}$/

/** Máximo de `TYPE` por propiedad, espejo del backend. */
const MAX_TYPES = 8

/** Vocabulario de `TYPE` más habitual (unión 3.0/4.0), como sugerencias. */
const TYPE_PRESETS = ['work', 'home', 'cell', 'voice', 'fax', 'internet', 'other']

const typeOptions = TYPE_PRESETS.map((token) => ({ value: token, label: token }))

/** Valida los `TYPE` de una fila (alfabeto acotado y número máximo). */
function validateTypes(_rule: unknown, value: string[] | undefined): Promise<void> {
  const tokens = value ?? []
  if (tokens.length > MAX_TYPES) {
    return Promise.reject(new Error(`Máximo ${MAX_TYPES} tipos`))
  }
  if (tokens.some((token) => !TYPE_PATTERN.test(token))) {
    return Promise.reject(new Error('Solo letras, números y guiones'))
  }
  return Promise.resolve()
}

/**
 * Valida el `PREF` de una fila: un entero en `1..=100` o vacío. Sin esto el
 * `<InputNumber>` solo *sugiere* el rango (con `min`/`max`) pero deja teclear
 * `0` o `200`, que el servidor rechaza con `400`.
 */
function validatePref(_rule: unknown, value: number | null | undefined): Promise<void> {
  if (value === null || value === undefined) {
    return Promise.resolve()
  }
  if (!Number.isInteger(value) || value < 1 || value > 100) {
    return Promise.reject(new Error('El PREF debe ser un número entre 1 y 100'))
  }
  return Promise.resolve()
}

export interface ContactFormProps {
  /** Instancia del formulario, compartida con el `Modal` (`onOk` → `submit`). */
  form: FormInstance<ContactFormValues>
  /** Valores iniciales (del detalle en edición, vacíos en alta). */
  initialValues: ContactFormValues
  /** Se invoca con los valores ya validados al enviar el formulario. */
  onFinish: (values: ContactFormValues) => void
}

/**
 * Grupo de `TEL`/`EMAIL`/`IMPP`: una fila por valor con su `value`, sus `TYPE` y
 * su `PREF`. `uri` decide el teclado/placeholder del valor.
 */
function LabelledValueList({
  name,
  label,
  placeholder,
  addLabel,
  uri = false,
}: {
  name: 'tels' | 'emails' | 'impps'
  label: string
  placeholder: string
  addLabel: string
  uri?: boolean
}) {
  return (
    <Form.List name={name}>
      {(fields, { add, remove }) => (
        <Flex vertical gap="small">
          {fields.map((field) => (
            <Card
              key={field.key}
              size="small"
              data-testid="contact-labelled-row"
              styles={{ body: { padding: 12 } }}
            >
              <Flex gap="small" align="end" wrap>
                <Form.Item
                  name={[field.name, 'value']}
                  label={label}
                  style={{ flex: '1 1 220px', marginBottom: 0 }}
                  rules={[{ required: true, message: `Introduce un valor` }]}
                >
                  <Input
                    placeholder={placeholder}
                    inputMode={uri ? 'url' : undefined}
                    aria-label={`${label} valor`}
                  />
                </Form.Item>
                <Form.Item
                  name={[field.name, 'types']}
                  label="Tipos"
                  style={{ flex: '1 1 160px', marginBottom: 0 }}
                  rules={[{ validator: validateTypes }]}
                >
                  <Select
                    mode="tags"
                    allowClear
                    tokenSeparators={[',']}
                    options={typeOptions}
                    placeholder="work, home…"
                    aria-label={`Tipos ${field.name + 1}`}
                  />
                </Form.Item>
                <Form.Item
                  name={[field.name, 'pref']}
                  label="PREF"
                  style={{ width: 96, marginBottom: 0 }}
                  rules={[{ validator: validatePref }]}
                >
                  <InputNumber
                    min={1}
                    max={100}
                    style={{ width: '100%' }}
                    aria-label={`PREF ${field.name + 1}`}
                  />
                </Form.Item>
                <Button
                  danger
                  type="text"
                  onClick={() => remove(field.name)}
                  aria-label={`Quitar ${label} ${field.name + 1}`}
                >
                  Quitar
                </Button>
              </Flex>
            </Card>
          ))}
          <Button onClick={() => add({ value: '', types: [], pref: null })} aria-label={addLabel}>
            {addLabel}
          </Button>
        </Flex>
      )}
    </Form.List>
  )
}

/**
 * Claves de {@link ContactFormValues} que son listas simples de texto; es lo
 * único que {@link TagListField} puede editar.
 */
type TagListFieldName = 'nicknames' | 'geos' | 'timezones' | 'titles' | 'roles' | 'urls'

/** Lista de texto simple (por ejemplo `NICKNAME`, `TITLE`, `URL`…). */
function TagListField({
  name,
  label,
  placeholder,
}: {
  name: TagListFieldName
  label: string
  placeholder?: string
}) {
  return (
    <Form.Item name={name} label={label}>
      <Select
        mode="tags"
        allowClear
        tokenSeparators={[',']}
        placeholder={placeholder ?? 'Escribe y pulsa Enter'}
        aria-label={label}
        suffixIcon={null}
      />
    </Form.Item>
  )
}

/**
 * Lista de listas (por ejemplo `ORG`/`CATEGORIES`): cada entrada es una fila con
 * sus componentes. Se implementa con `Form.List` anidados, de modo que un
 * componente pueda contener el separador (`;`/`,`) sin perder información.
 */
function GroupListField({
  name,
  label,
  addLabel,
  entryLabel,
}: {
  name: 'organizations' | 'categories'
  label: string
  addLabel: string
  entryLabel: string
}) {
  return (
    <Form.List name={name}>
      {(fields, { add, remove }) => (
        <div>
          <Typography.Text strong>{label}</Typography.Text>
          <Flex vertical gap="small" style={{ marginTop: 8 }}>
            {fields.map((field) => (
              <Card
                key={field.key}
                size="small"
                title={`${entryLabel} ${field.name + 1}`}
                extra={
                  <Button
                    danger
                    type="link"
                    onClick={() => remove(field.name)}
                    aria-label={`Quitar ${entryLabel.toLowerCase()} ${field.name + 1}`}
                  >
                    Quitar
                  </Button>
                }
                styles={{ body: { padding: 12 } }}
              >
                <Form.List name={[field.name]}>
                  {(subFields, subOps) => (
                    <Flex vertical gap="small">
                      {subFields.map((subField) => (
                        <Space key={subField.key} align="baseline">
                          <Form.Item name={[subField.name]} noStyle>
                            <Input
                              placeholder="Componente"
                              style={{ width: 320, maxWidth: '100%' }}
                              aria-label={`${entryLabel} componente`}
                            />
                          </Form.Item>
                          <Button
                            danger
                            type="text"
                            onClick={() => subOps.remove(subField.name)}
                            aria-label={`Quitar componente de ${entryLabel}`}
                          >
                            Quitar
                          </Button>
                        </Space>
                      ))}
                      <Button size="small" onClick={() => subOps.add('')}>
                        Añadir componente
                      </Button>
                    </Flex>
                  )}
                </Form.List>
              </Card>
            ))}
            <Button onClick={() => add([''])} aria-label={addLabel}>
              {addLabel}
            </Button>
          </Flex>
        </div>
      )}
    </Form.List>
  )
}

/** Lista de direcciones (`ADR`): siete componentes más `LABEL`, `TYPE` y `PREF`. */
function AddressList() {
  return (
    <Form.List name="addresses">
      {(fields, { add, remove }) => (
        <Flex vertical gap="small">
          {fields.map((field) => (
            <Card
              key={field.key}
              size="small"
              title={`Dirección ${field.name + 1}`}
              extra={
                <Button
                  danger
                  type="link"
                  onClick={() => remove(field.name)}
                  aria-label={`Quitar dirección ${field.name + 1}`}
                >
                  Quitar
                </Button>
              }
              styles={{ body: { padding: 12 } }}
            >
              <Flex vertical gap="small">
                <Flex gap="small" wrap>
                  <Form.Item name={[field.name, 'po_box']} label="Apartado" style={{ flex: 1, marginBottom: 0 }}>
                    <Input aria-label="Apartado de correos" />
                  </Form.Item>
                  <Form.Item name={[field.name, 'extended']} label="Extendido" style={{ flex: 1, marginBottom: 0 }}>
                    <Input aria-label="Dirección extendida" />
                  </Form.Item>
                </Flex>
                <Form.Item name={[field.name, 'street']} label="Calle" style={{ marginBottom: 0 }}>
                  <Input aria-label="Calle" />
                </Form.Item>
                <Flex gap="small" wrap>
                  <Form.Item name={[field.name, 'locality']} label="Localidad" style={{ flex: 1, marginBottom: 0 }}>
                    <Input aria-label="Localidad" />
                  </Form.Item>
                  <Form.Item name={[field.name, 'region']} label="Región" style={{ flex: 1, marginBottom: 0 }}>
                    <Input aria-label="Región" />
                  </Form.Item>
                  <Form.Item name={[field.name, 'postal_code']} label="Código postal" style={{ flex: 1, marginBottom: 0 }}>
                    <Input aria-label="Código postal" />
                  </Form.Item>
                </Flex>
                <Form.Item name={[field.name, 'country']} label="País" style={{ marginBottom: 0 }}>
                  <Input aria-label="País" />
                </Form.Item>
                <Form.Item name={[field.name, 'label']} label="Etiqueta (LABEL)" style={{ marginBottom: 0 }}>
                  <Input aria-label="Etiqueta" />
                </Form.Item>
                <Flex gap="small" align="end" wrap>
                  <Form.Item
                    name={[field.name, 'types']}
                    label="Tipos"
                    style={{ flex: '1 1 160px', marginBottom: 0 }}
                    rules={[{ validator: validateTypes }]}
                  >
                    <Select
                      mode="tags"
                      allowClear
                      tokenSeparators={[',']}
                      options={typeOptions}
                      placeholder="work, home…"
                      aria-label={`Tipos de la dirección ${field.name + 1}`}
                    />
                  </Form.Item>
                  <Form.Item
                    name={[field.name, 'pref']}
                    label="PREF"
                    style={{ width: 96, marginBottom: 0 }}
                    rules={[{ validator: validatePref }]}
                  >
                    <InputNumber
                      min={1}
                      max={100}
                      style={{ width: '100%' }}
                      aria-label={`PREF de la dirección ${field.name + 1}`}
                    />
                  </Form.Item>
                </Flex>
              </Flex>
            </Card>
          ))}
          <Button onClick={() => add({ types: [], pref: null })} aria-label="Añadir dirección">
            Añadir dirección
          </Button>
        </Flex>
      )}
    </Form.List>
  )
}

/** Notas (`NOTE`): una fila por nota, en área de texto (pueden ser largas). */
function NotesField() {
  return (
    <Form.List name="notes">
      {(fields, { add, remove }) => (
        <Flex vertical gap="small">
          {fields.map((field) => (
            <Flex key={field.key} gap="small" align="end">
              <Form.Item name={[field.name]} style={{ flex: 1, marginBottom: 0 }}>
                <Input.TextArea rows={2} aria-label="Nota" placeholder="Nota" />
              </Form.Item>
              <Button
                danger
                type="text"
                onClick={() => remove(field.name)}
                aria-label="Quitar nota"
              >
                Quitar
              </Button>
            </Flex>
          ))}
          <Button onClick={() => add('')} aria-label="Añadir nota">
            Añadir nota
          </Button>
        </Flex>
      )}
    </Form.List>
  )
}

/** Una pestaña tal y como la consume `<Tabs>` de Ant Design. */
type TabItem = NonNullable<TabsProps['items']>[number]

/** `name` path que acepta `focusField` para el tipo de valores del formulario. */
type FocusName = Parameters<FormInstance<ContactFormValues>['focusField']>[0]

/**
 * Nodo raíz del `<Form>`: su instancia más el `<form>` del DOM. El `ref` de
 * Ant Design entrega ambos, y el segundo acota las búsquedas al formulario.
 */
type ContactFormNode = FormInstance<ContactFormValues> & { nativeElement?: HTMLElement }

/** Id del texto oculto que describe una pestaña con error. */
function tabErrorDescriptionId(tabKey: ContactFormTabKey): string {
  return `contact-tab-error-desc-${tabKey}`
}

/** Descripción de una pestaña con error (tooltip y texto para lectores). */
function tabErrorDescription(label: string): string {
  return `${label}: esta pestaña tiene errores de validación`
}

/**
 * Etiqueta de una pestaña, con un **punto rojo** cuando tiene algún error.
 *
 * El punto va `aria-hidden` (y `role="presentation"`), de modo que el nombre
 * accesible de la pestaña sigue siendo solo su etiqueta: el punto es una señal
 * **visual** y no entra ni en el nombre ni en la descripción.
 *
 * La señal accesible llega por dos vías que **no** tocan el nombre:
 * un `title` (tooltip nativo al pasar el ratón) y el `aria-describedby` que el
 * componente añade a la pestaña, apuntando al texto `sr-only` con la
 * descripción (ver {@link tabErrorDescriptionId}).
 */
function TabLabel({
  tabKey,
  label,
  hasError,
}: {
  tabKey: ContactFormTabKey
  label: string
  hasError: boolean
}) {
  if (!hasError) {
    return <>{label}</>
  }
  return (
    <span title={tabErrorDescription(label)}>
      {label}
      <span
        role="presentation"
        aria-hidden="true"
        data-testid={`contact-tab-error-${tabKey}`}
        style={{
          display: 'inline-block',
          width: 6,
          height: 6,
          marginInlineStart: 6,
          borderRadius: '50%',
          background: '#ff4d4f',
          verticalAlign: 'middle',
        }}
      />
    </span>
  )
}

/** ¿Los dos conjuntos de pestañas con error contienen exactamente lo mismo? */
function sameTabSet(
  a: ReadonlySet<ContactFormTabKey>,
  b: ReadonlySet<ContactFormTabKey>,
): boolean {
  if (a.size !== b.size) {
    return false
  }
  for (const key of a) {
    if (!b.has(key)) {
      return false
    }
  }
  return true
}

/**
 * Los campos de una pestaña. Son exactamente los mismos bloques que antes de
 * F23 (nada se ha quitado ni reordenado), solo que agrupados en una pestaña.
 */
function TabFields({ tab }: { tab: ContactFormTabKey }) {
  switch (tab) {
    case 'names':
      return (
        <>
          <Flex gap="small" wrap>
            <Form.Item name={['name', 'given']} label="Nombre de pila" style={{ flex: 1, minWidth: 200, marginBottom: 0 }}>
              <Input aria-label="Nombre de pila" />
            </Form.Item>
            <Form.Item name={['name', 'family']} label="Apellidos" style={{ flex: 1, minWidth: 200, marginBottom: 0 }}>
              <Input aria-label="Apellidos" />
            </Form.Item>
          </Flex>
          <Form.Item name={['name', 'prefix']} label="Prefijos" style={{ marginTop: 16 }}>
            <Select mode="tags" allowClear tokenSeparators={[',']} aria-label="Prefijos" suffixIcon={null} />
          </Form.Item>
          <Form.Item name={['name', 'additional']} label="Nombres adicionales">
            <Select mode="tags" allowClear tokenSeparators={[',']} aria-label="Nombres adicionales" suffixIcon={null} />
          </Form.Item>
          <Form.Item name={['name', 'suffix']} label="Sufijos">
            <Select mode="tags" allowClear tokenSeparators={[',']} aria-label="Sufijos" suffixIcon={null} />
          </Form.Item>
          <TagListField name="nicknames" label="Apodos (NICKNAME)" />
          <Flex gap="small" wrap>
            <Form.Item name="birthday" label="Cumpleaños (BDAY)" style={{ flex: 1, minWidth: 200, marginBottom: 0 }}>
              <Input placeholder="1990-01-31" aria-label="Cumpleaños" />
            </Form.Item>
            <Form.Item name="anniversary" label="Aniversario" style={{ flex: 1, minWidth: 200, marginBottom: 0 }}>
              <Input placeholder="2015-06-20" aria-label="Aniversario" />
            </Form.Item>
          </Flex>
          <Flex gap="small" wrap style={{ marginTop: 16 }}>
            <Form.Item name={['gender', 'sex']} label="Género (sexo)" style={{ flex: 1, minWidth: 200, marginBottom: 0 }}>
              <Select
                allowClear
                aria-label="Sexo"
                options={[
                  { value: 'M', label: 'M (masculino)' },
                  { value: 'F', label: 'F (femenino)' },
                  { value: 'O', label: 'O (otro)' },
                  { value: 'N', label: 'N (sin especificar)' },
                  { value: 'U', label: 'U (desconocido)' },
                ]}
              />
            </Form.Item>
            <Form.Item name={['gender', 'identity']} label="Identidad" style={{ flex: 1, minWidth: 200, marginBottom: 0 }}>
              <Input aria-label="Identidad de género" />
            </Form.Item>
          </Flex>
        </>
      )
    case 'contact':
      return (
        <>
          <LabelledValueList name="tels" label="Teléfono" placeholder="+34 600 000 000" addLabel="Añadir teléfono" />
          <LabelledValueList
            name="emails"
            label="Correo"
            placeholder="email@example.com"
            addLabel="Añadir correo"
          />
          <LabelledValueList
            name="impps"
            label="Mensajería (IMPP)"
            placeholder="xmpp:usuario@example.com"
            addLabel="Añadir mensajería"
            uri
          />
        </>
      )
    case 'addresses':
      return (
        <>
          <AddressList />
          <TagListField name="geos" label="Coordenadas (GEO)" placeholder="geo:37.386013,-122.082932" />
          <TagListField name="timezones" label="Zonas horarias (TZ)" placeholder="Europe/Madrid" />
        </>
      )
    case 'work':
      return (
        <>
          <TagListField name="titles" label="Puestos (TITLE)" />
          <TagListField name="roles" label="Roles (ROLE)" />
          <GroupListField
            name="organizations"
            label="Organizaciones (ORG)"
            addLabel="Añadir organización"
            entryLabel="Organización"
          />
        </>
      )
    case 'additional':
      return (
        <>
          <NotesField />
          <GroupListField
            name="categories"
            label="Categorías"
            addLabel="Añadir categoría"
            entryLabel="Categoría"
          />
          <TagListField name="urls" label="Enlaces (URL)" placeholder="https://example.com" />
        </>
      )
  }
}

export function ContactForm({ form, initialValues, onFinish }: ContactFormProps) {
  const [activeTab, setActiveTab] = useState<ContactFormTabKey>(DEFAULT_CONTACT_FORM_TAB)

  // Pestañas con error, en estado. Se recalculan con `onFieldsChange` (no con un
  // `shouldUpdate` que corra en cada tecla) y **solo** se guardan cuando el
  // conjunto cambia: así teclear no re-renderiza las cinco pestañas.
  const [errorTabs, setErrorTabs] = useState<ReadonlySet<ContactFormTabKey>>(
    () => new Set<ContactFormTabKey>(),
  )

  // Campo al que hay que devolver el foco tras el siguiente pintado. Es estado
  // (no `ref`) para que un nuevo valor **siempre** dispare el efecto, incluso si
  // el error está en la pestaña ya activa (que no cambia `activeTab`).
  const [focusRequest, setFocusRequest] = useState<FieldNamePath | null>(null)

  // Nodo raíz del `<Form>`: sirve para acotar las búsquedas en el DOM a **este**
  // formulario en vez de al `document` global (dos formularios con los mismos
  // nombres ya no pueden robarse el foco).
  const formNodeRef = useRef<ContactFormNode>(null)

  // La instancia `form` es única por vista y la comparte el `<Modal>` (su `onOk`
  // llama a `form.submit()`), así que sobrevive al remontaje por `key` del
  // formulario. Su *store* conserva los valores del contacto anterior y Ant
  // Design, al reinicializar, hace `merge(initialValues, store)`: el *store*
  // gana, de modo que `initialValues` por sí solo no reemplaza nada. Hay que
  // resetear explícitamente cuando cambian los valores del contacto para que el
  // formulario muestre los del contacto nuevo, no los del anterior.
  useEffect(() => {
    form.resetFields()
  }, [form, initialValues])

  // Devuelve el foco al campo de un error, con el mismo camino para las dos
  // ramas de `handleFinishFailed` (cambio de pestaña o error en la pestaña ya
  // activa).
  //
  // `form.focusField` (y el `focus()` que expone `getFieldInstance`) **no**
  // enfocan de verdad un `Select` de Ant Design dentro de un `Form.List`: su
  // instancia no mueve el foco al `<input>` interno que el usuario ve. Por eso,
  // tras intentarlo por la instancia, comprobamos dónde quedó
  // `document.activeElement` y, si no está dentro del campo, caemos al DOM de
  // forma determinista: el `Form.Item` de `['tels', 0, 'types']` deja el `id`
  // `tels_0_types` en su control (Ant Design une el *path* con `_`; el `<Form>` no
  // tiene `name`, así que no hay prefijo), y ese input sí acepta `.focus()`.
  //
  // La búsqueda se acota al nodo del formulario (`formNodeRef`), no al
  // `document`. El test que fija `tels_0_types` se pone rojo si Ant Design
  // cambia la convención de ids; si algún día se añadiera `<Form name="…">`,
  // Ant Design prefijaría el id (`<name>_tels_0_types`) y habría que incluirlo
  // aquí.
  //
  // Todo ocurre en el siguiente frame (`requestAnimationFrame`): en el commit en
  // que cambia `activeTab` el panel recién activado todavía no es enfocable (su
  // contenido sigue oculto), así que el foco aplicado allí se pierde; el frame
  // siguiente al cambio ya lo tiene visible. La limpieza cancela el frame si el
  // componente se desmonta o se pide otro foco antes de pintarse.
  useEffect(() => {
    if (focusRequest === null) {
      return
    }
    const name = focusRequest
    const instance = form.getFieldInstance(name as FocusName) as { focus?: () => void } | undefined
    const frame = requestAnimationFrame(() => {
      instance?.focus?.()
      const fieldId = (name as readonly (string | number)[]).join('_')
      const root = formNodeRef.current?.nativeElement ?? null
      const control = root?.querySelector<HTMLElement>(`[id="${fieldId}"]`) ?? null
      // Si el foco ya quedó en el control (o dentro de él), no hay nada que hacer.
      if (control !== null && !control.contains(document.activeElement)) {
        control.focus()
      }
    })
    return () => cancelAnimationFrame(frame)
  }, [focusRequest, form])

  // Señal accesible de las pestañas con error. Ant Design no reenvía props
  // arbitrarias al botón `role="tab"`, así que el `aria-describedby` (que debe
  // vivir en **ese** elemento para describir la pestaña sin tocar su nombre) se
  // sincroniza aquí, acotado al nodo del formulario.
  useEffect(() => {
    const root = formNodeRef.current?.nativeElement
    if (root === null || root === undefined) {
      return
    }
    for (const tab of CONTACT_FORM_TABS) {
      const button = root.querySelector<HTMLElement>(`[data-node-key="${tab.key}"] [role="tab"]`)
      if (button === null) {
        continue
      }
      if (errorTabs.has(tab.key)) {
        button.setAttribute('aria-describedby', tabErrorDescriptionId(tab.key))
      } else {
        button.removeAttribute('aria-describedby')
      }
    }
  }, [errorTabs])

  // Deriva el conjunto de pestañas con error de **todos** los campos que entrega
  // `onFieldsChange` y guarda **solo** si cambia (comparación de conjuntos).
  const handleFieldsChange: NonNullable<FormProps<ContactFormValues>['onFieldsChange']> = (
    _changedFields,
    allFields,
  ) => {
    // `onFieldsChange` entrega los `name` como arrays en tiempo de ejecución,
    // pero el tipo público `FieldData` los ensancha a `NamePath`; se estrechan
    // otra vez al `FieldNamePath` (array) que espera `tabErrors`.
    const next = tabErrors(
      allFields.map((field) => ({
        name: field.name as unknown as FieldNamePath,
        errors: field.errors ?? [],
      })),
    )
    setErrorTabs((prev) => (sameTabSet(prev, next) ? prev : next))
  }

  const handleFinishFailed: NonNullable<FormProps<ContactFormValues>['onFinishFailed']> = (
    errorInfo,
  ) => {
    const first = errorInfo.errorFields[0]
    if (first === undefined) {
      return
    }
    const tab = tabForField(first.name)
    if (tab !== null && tab !== activeTab) {
      setActiveTab(tab)
    }
    // Un único camino de foco para las dos ramas: también cuando el error está
    // en la pestaña ya activa (o en el campo visible), donde antes solo se
    // llamaba a `form.focusField` y un `Select` de un `Form.List` podía no
    // recibir el foco.
    setFocusRequest([...first.name])
  }

  const items: TabItem[] = CONTACT_FORM_TABS.map((tab) => ({
    key: tab.key,
    label: <TabLabel tabKey={tab.key} label={tab.label} hasError={errorTabs.has(tab.key)} />,
    // Todos los paneles se montan desde el principio: así sus campos se
    // registran (la validación cubre hasta los campos en pestañas no visitadas)
    // y los valores de los `Form.List` nunca se desmontan.
    forceRender: true,
    children: <TabFields tab={tab.key} />,
  }))

  return (
    <Form
      ref={formNodeRef}
      form={form}
      layout="vertical"
      onFinish={onFinish}
      onFinishFailed={handleFinishFailed}
      onFieldsChange={handleFieldsChange}
      initialValues={initialValues}
    >
      <Form.Item
        name="fn"
        label="Nombre"
        rules={[{ required: true, message: 'Introduce un nombre' }]}
      >
        <Input placeholder="Nombre completo" />
      </Form.Item>

      <Tabs
        activeKey={activeTab}
        onChange={(key) => setActiveTab(key as ContactFormTabKey)}
        items={items}
      />

      {/* Texto de la descripción accesible: fuera del botón `role="tab"` para no
          alterar su nombre accesible; la pestaña lo enlaza con `aria-describedby`. */}
      {errorTabs.size > 0 && (
        <div>
          {CONTACT_FORM_TABS.map((tab) =>
            errorTabs.has(tab.key) ? (
              <p key={tab.key} id={tabErrorDescriptionId(tab.key)} className="sr-only">
                {tabErrorDescription(tab.label)}
              </p>
            ) : null,
          )}
        </div>
      )}
    </Form>
  )
}

