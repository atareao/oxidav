/**
 * Formulario de alta/edición de contactos (F22).
 *
 * Cubre todos los campos **no binarios** del detalle, organizados por las mismas
 * secciones que las pestañas de la vista (`Nombres`, `Contacto`, `Direcciones`,
 * `Laboral`, `Adicional`) con `<Divider>` entre ellas. El cuerpo se hace
 * desplazable desde el `Modal` que lo monta.
 *
 * Los grupos multivalor con `TYPE`/`PREF` (`TEL`/`EMAIL`/`IMPP`) y las
 * direcciones (`ADR`) usan `Form.List` para poder añadir, quitar y editar filas.
 * Los binarios (`PHOTO`/`LOGO`/`SOUND`) **no** aparecen: se muestran y se
 * preservan, no se editan.
 *
 * El componente es **presentacional**: recibe la instancia del formulario y los
 * valores iniciales, y entrega los valores por `onFinish`. Toda la lógica de
 * diff/conversión vive en {@link file://./contactFormValues.ts}.
 */

import { Button, Card, Divider, Flex, Form, Input, InputNumber, Select, Space, Typography } from 'antd'
import type { FormInstance } from 'antd'
import { useEffect } from 'react'
import type { ContactFormValues } from './contactFormValues'

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

export function ContactForm({ form, initialValues, onFinish }: ContactFormProps) {
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

  return (
    <Form form={form} layout="vertical" onFinish={onFinish} initialValues={initialValues}>
      <Form.Item
        name="fn"
        label="Nombre"
        rules={[{ required: true, message: 'Introduce un nombre' }]}
      >
        <Input placeholder="Nombre completo" />
      </Form.Item>

      <Divider titlePlacement="start">
        Nombres
      </Divider>
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

      <Divider titlePlacement="start">
        Contacto
      </Divider>
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

      <Divider titlePlacement="start">
        Direcciones
      </Divider>
      <AddressList />
      <TagListField name="geos" label="Coordenadas (GEO)" placeholder="geo:37.386013,-122.082932" />
      <TagListField name="timezones" label="Zonas horarias (TZ)" placeholder="Europe/Madrid" />

      <Divider titlePlacement="start">
        Laboral
      </Divider>
      <TagListField name="titles" label="Puestos (TITLE)" />
      <TagListField name="roles" label="Roles (ROLE)" />
      <GroupListField
        name="organizations"
        label="Organizaciones (ORG)"
        addLabel="Añadir organización"
        entryLabel="Organización"
      />

      <Divider titlePlacement="start">
        Adicional
      </Divider>
      <NotesField />
      <GroupListField
        name="categories"
        label="Categorías"
        addLabel="Añadir categoría"
        entryLabel="Categoría"
      />
      <TagListField name="urls" label="Enlaces (URL)" placeholder="https://example.com" />
    </Form>
  )
}
