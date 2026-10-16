/**
 * Pestañas del detalle de un contacto (F22).
 *
 * Componente **puro y presentacional**: recibe la proyección completa
 * (`ContactDetail`) ya resuelta por el servidor y la organiza en ocho grupos
 * (las siete categorías del encargo más «Otros»). No hace peticiones ni usa
 * hooks de datos; el `Drawer` de la vista es quien carga y le pasa el detalle.
 *
 * Reglas de presentación fijadas por el contrato 9 del plan y por los tests:
 * - Una pestaña **sin contenido no se renderiza**; si ninguna tuviera contenido
 *   se muestra un `<Empty>`.
 * - Dentro de una pestaña, un apartado sin datos tampoco se pinta (nada de
 *   «NICKNAME: —»).
 * - Los valores multivalor se listan uno por línea con sus `TYPE` como `Tag` y
 *   su `PREF` visible.
 * - `PHOTO`/`LOGO`/`SOUND` se pintan con su `data:` URI o su URI externa.
 * - Nunca se inyecta HTML: todo valor se renderiza **como texto**.
 *
 * Los constructores de bloque son funciones (no componentes) a propósito: así
 * devuelven `null` de verdad cuando no hay datos y `renderBlocks` puede decidir
 * si la pestaña entera sobra. Un elemento JSX sería siempre "verdadero" y
 * ocultaría ese vacío.
 */

import { Descriptions, Empty, Flex, Image, Space, Tabs, Tag, Typography } from 'antd'
import type { TabsProps } from 'antd'
import { Fragment, type ReactNode } from 'react'
import type {
  Address,
  ContactDetail,
  LabelledValue,
  Media,
  RawProperty,
  StructuredName,
} from '../../types/api'
import { formatDateTime } from '../../utils/date'

/** Una pestaña tal y como la consume `<Tabs>` de Ant Design. */
type TabItem = NonNullable<TabsProps['items']>[number]

/** Una fila etiqueta/valor de un `<Descriptions>`. */
interface ScalarRow {
  label: string
  value: string
}

export interface ContactDetailTabsProps {
  /** Proyección completa del contacto, ya derivada por el servidor. */
  detail: ContactDetail
  /**
   * Nombre legible de la libreta del contacto (F22), resuelto por la página a
   * partir de las colecciones ya cargadas. Si no llega, la pestaña «Servidor»
   * cae al identificador opaco de {@link ContactDetail.collection_id}.
   */
  collectionName?: string
}

/** ¿El valor es texto no vacío? (guarda de tipo para filtrar arrays). */
function hasText(value: string | null | undefined): value is string {
  return typeof value === 'string' && value.trim().length > 0
}

/** Bloque titulado; la cabecera da contexto a la sección de la pestaña. */
function Block({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section>
      <Typography.Text strong>{title}</Typography.Text>
      <div style={{ marginTop: 8 }}>{children}</div>
    </section>
  )
}

/** Un bloque con la clave estable con la que se renderiza en la pila. */
interface RenderBlock {
  key: string
  node: ReactNode | null
}

/** Pila vertical de bloques; devuelve `null` si no hay ninguno que pintar. */
function renderBlocks(blocks: RenderBlock[]): ReactNode | null {
  const visible = blocks.filter(
    (block) => block.node !== null && block.node !== undefined && block.node !== false,
  )
  if (visible.length === 0) {
    return null
  }
  return (
    <Flex vertical gap="middle">
      {visible.map((block) => (
        <Fragment key={block.key}>{block.node}</Fragment>
      ))}
    </Flex>
  )
}

/** `<Descriptions>` de filas etiqueta/valor, opcionalmente con título. */
function descriptionsBlock(rows: ScalarRow[], title?: string): ReactNode {
  if (rows.length === 0) {
    return null
  }
  const table = (
    <Descriptions column={1} bordered size="small">
      {rows.map((row, index) => (
        <Descriptions.Item key={`${row.label}-${index}`} label={row.label}>
          {row.value}
        </Descriptions.Item>
      ))}
    </Descriptions>
  )
  return title === undefined ? table : <Block title={title}>{table}</Block>
}

/** Lista de valores etiquetados: valor + `TYPE`s + `PREF`, uno por línea. */
function labelledValueBlock(title: string, values: LabelledValue[]): ReactNode {
  if (values.length === 0) {
    return null
  }
  return (
    <Block title={title}>
      <Flex vertical gap="small">
        {values.map((item, index) => (
          <Space key={`${item.value}-${index}`} wrap>
            <Typography.Text>{item.value}</Typography.Text>
            {item.types.map((type) => (
              <Tag key={type}>{type}</Tag>
            ))}
            {item.pref !== null ? <Tag>PREF {item.pref}</Tag> : null}
          </Space>
        ))}
      </Flex>
    </Block>
  )
}

/** Lista simple de textos, uno por línea. */
function stringBlock(title: string, values: string[]): ReactNode {
  const present = values.filter(hasText)
  if (present.length === 0) {
    return null
  }
  return (
    <Block title={title}>
      <Flex vertical gap="small">
        {present.map((value, index) => (
          <Typography.Text key={`${value}-${index}`}>{value}</Typography.Text>
        ))}
      </Flex>
    </Block>
  )
}

/** Lista de grupos (arrays de arrays): un `Tag` por elemento, agrupado por línea. */
function groupBlock(title: string, groups: string[][]): ReactNode {
  const present = groups.map((group) => group.filter(hasText)).filter((group) => group.length > 0)
  if (present.length === 0) {
    return null
  }
  return (
    <Block title={title}>
      <Flex vertical gap="small">
        {present.map((group, index) => (
          <Space key={index} wrap>
            {group.map((value, position) => (
              <Tag key={`${value}-${position}`}>{value}</Tag>
            ))}
          </Space>
        ))}
      </Flex>
    </Block>
  )
}

/** Los componentes no vacíos de una dirección. */
function addressLines(address: Address): string[] {
  return [
    address.po_box,
    address.extended,
    address.street,
    address.locality,
    address.region,
    address.postal_code,
    address.country,
  ].filter(hasText)
}

/** ¿La dirección tiene algo que mostrar? */
function hasAddressContent(address: Address): boolean {
  return (
    addressLines(address).length > 0 ||
    hasText(address.label) ||
    address.types.length > 0 ||
    address.pref !== null
  )
}

/** Direcciones: cada una en un bloque legible con sus `TYPE`/`PREF`. */
function addressBlock(addresses: Address[]): ReactNode {
  const present = addresses.filter(hasAddressContent)
  if (present.length === 0) {
    return null
  }
  return (
    <Block title="Direcciones">
      <Flex vertical gap="middle">
        {present.map((address, index) => (
          <div key={`${addressLines(address).join('|')}#${index}`}>
            {address.types.length > 0 || address.pref !== null ? (
              <Space wrap>
                {address.types.map((type) => (
                  <Tag key={type}>{type}</Tag>
                ))}
                {address.pref !== null ? <Tag>PREF {address.pref}</Tag> : null}
              </Space>
            ) : null}
            {hasText(address.label) ? <Typography.Text>{address.label}</Typography.Text> : null}
            <Flex vertical>
              {addressLines(address).map((line, position) => (
                <Typography.Text key={`${line}-${position}`}>{line}</Typography.Text>
              ))}
            </Flex>
          </div>
        ))}
      </Flex>
    </Block>
  )
}

/** Fuente de una media: el `data:` URI o la URI externa según la variante. */
function mediaSrc(media: Media): string {
  return media.kind === 'data' ? media.data_uri : media.uri
}

/**
 * `PHOTO`/`LOGO`/`SOUND`: una imagen por valor. `referrerPolicy="no-referrer"`
 * evita filtrar la URL de origen y `onError` deja que una imagen rota no rompa
 * la vista (la damos por no renderizable y seguimos).
 */
function mediaBlock(title: string, media: Media[]): ReactNode {
  if (media.length === 0) {
    return null
  }
  return (
    <Block title={title}>
      <Space wrap>
        {media.map((item, index) => (
          <Image
            key={`${mediaSrc(item)}#${index}`}
            src={mediaSrc(item)}
            alt={`${title} ${index + 1}`}
            width={120}
            preview={false}
            referrerPolicy="no-referrer"
            onError={() => undefined}
          />
        ))}
      </Space>
    </Block>
  )
}

/** «Otros»: propiedades no modeladas con nombre, grupo, parámetros y valor. */
function othersBlock(others: RawProperty[]): ReactNode {
  if (others.length === 0) {
    return null
  }
  return (
    <Block title="Propiedades sin modelar">
      <Flex vertical gap="middle">
        {others.map((property, index) => (
          <div key={`${property.name}#${property.group ?? ''}#${index}`}>
            <Space wrap>
              <Typography.Text strong>{property.name}</Typography.Text>
              {hasText(property.group) ? <Tag>{property.group}</Tag> : null}
            </Space>
            {property.parameters.length > 0 ? (
              <Typography.Text type="secondary">
                {property.parameters.map(([name, value]) => `${name}=${value}`).join(', ')}
              </Typography.Text>
            ) : null}
            <div>
              <Typography.Text>{property.value}</Typography.Text>
            </div>
          </div>
        ))}
      </Flex>
    </Block>
  )
}

/** Filas del `N` estructurado, solo con los componentes presentes. */
function structuredNameRows(name: StructuredName | null): ScalarRow[] {
  if (name === null) {
    return []
  }
  const rows: ScalarRow[] = []
  if (hasText(name.family)) {
    rows.push({ label: 'Familia', value: name.family })
  }
  if (hasText(name.given)) {
    rows.push({ label: 'Nombre', value: name.given })
  }
  if (name.additional.length > 0) {
    rows.push({ label: 'Adicional', value: name.additional.join(', ') })
  }
  if (name.prefix.length > 0) {
    rows.push({ label: 'Prefijo', value: name.prefix.join(', ') })
  }
  if (name.suffix.length > 0) {
    rows.push({ label: 'Sufijo', value: name.suffix.join(', ') })
  }
  return rows
}

export function ContactDetailTabs({ detail, collectionName }: ContactDetailTabsProps) {
  const items: TabItem[] = []

  // ── Control ───────────────────────────────────────────────────────────────
  const controlRows: ScalarRow[] = []
  if (hasText(detail.fn)) {
    controlRows.push({ label: 'FN', value: detail.fn })
  }
  if (hasText(detail.version)) {
    controlRows.push({ label: 'VERSION', value: detail.version })
  }
  if (hasText(detail.uid)) {
    controlRows.push({ label: 'UID', value: detail.uid })
  }
  if (hasText(detail.rev)) {
    controlRows.push({ label: 'REV', value: detail.rev })
  }
  const control = renderBlocks([{ key: 'control', node: descriptionsBlock(controlRows) }])
  if (control !== null) {
    items.push({ key: 'control', label: 'Control', children: control })
  }

  // ── Nombres ───────────────────────────────────────────────────────────────
  const genderRows: ScalarRow[] = []
  if (detail.gender !== null) {
    if (hasText(detail.gender.sex)) {
      genderRows.push({ label: 'Sexo', value: detail.gender.sex })
    }
    if (hasText(detail.gender.identity)) {
      genderRows.push({ label: 'Identidad', value: detail.gender.identity })
    }
  }
  const dateRows: ScalarRow[] = []
  if (hasText(detail.birthday)) {
    dateRows.push({ label: 'Cumpleaños', value: detail.birthday })
  }
  if (hasText(detail.anniversary)) {
    dateRows.push({ label: 'Aniversario', value: detail.anniversary })
  }
  const names = renderBlocks([
    { key: 'name', node: descriptionsBlock(structuredNameRows(detail.name), 'Nombre') },
    { key: 'nicknames', node: stringBlock('Apodos', detail.nicknames) },
    { key: 'photos', node: mediaBlock('Fotos', detail.photos) },
    { key: 'dates', node: descriptionsBlock(dateRows, 'Fechas') },
    { key: 'gender', node: descriptionsBlock(genderRows, 'Género') },
  ])
  if (names !== null) {
    items.push({ key: 'names', label: 'Nombres', children: names })
  }

  // ── Contacto ──────────────────────────────────────────────────────────────
  const contact = renderBlocks([
    { key: 'tels', node: labelledValueBlock('Teléfonos', detail.tels) },
    { key: 'emails', node: labelledValueBlock('Correos', detail.emails) },
    { key: 'impps', node: labelledValueBlock('Mensajería (IMPP)', detail.impps) },
  ])
  if (contact !== null) {
    items.push({ key: 'contact', label: 'Contacto', children: contact })
  }

  // ── Direcciones ───────────────────────────────────────────────────────────
  const addresses = renderBlocks([
    { key: 'addresses', node: addressBlock(detail.addresses) },
    { key: 'geos', node: stringBlock('Coordenadas (GEO)', detail.geos) },
    { key: 'timezones', node: stringBlock('Zonas horarias (TZ)', detail.timezones) },
  ])
  if (addresses !== null) {
    items.push({ key: 'addresses', label: 'Direcciones', children: addresses })
  }

  // ── Laboral ───────────────────────────────────────────────────────────────
  const work = renderBlocks([
    { key: 'titles', node: stringBlock('Puestos (TITLE)', detail.titles) },
    { key: 'roles', node: stringBlock('Roles (ROLE)', detail.roles) },
    { key: 'organizations', node: groupBlock('Organizaciones (ORG)', detail.organizations) },
    { key: 'logos', node: mediaBlock('Logos', detail.logos) },
  ])
  if (work !== null) {
    items.push({ key: 'work', label: 'Laboral', children: work })
  }

  // ── Adicional ─────────────────────────────────────────────────────────────
  const additional = renderBlocks([
    { key: 'notes', node: stringBlock('Notas', detail.notes) },
    { key: 'categories', node: groupBlock('Categorías', detail.categories) },
    { key: 'urls', node: stringBlock('Enlaces (URL)', detail.urls) },
    { key: 'sounds', node: mediaBlock('Sonidos', detail.sounds) },
  ])
  if (additional !== null) {
    items.push({ key: 'additional', label: 'Adicional', children: additional })
  }

  // ── Servidor ──────────────────────────────────────────────────────────────
  const serverRows: ScalarRow[] = []
  if (hasText(detail.etag)) {
    serverRows.push({ label: 'ETag', value: detail.etag })
  }
  if (hasText(detail.collection_id)) {
    // El nombre legible manda cuando la página lo conoce; si no, el id opaco.
    serverRows.push({ label: 'Colección', value: collectionName ?? detail.collection_id })
  }
  // `0` es un centinela de "sin marca de tiempo" (epoch 1970 no es real aquí);
  // solo se pinta la fecha cuando hay un instante de verdad.
  if (detail.created_at > 0) {
    serverRows.push({ label: 'Creado', value: formatDateTime(detail.created_at) })
  }
  if (detail.updated_at > 0) {
    serverRows.push({ label: 'Actualizado', value: formatDateTime(detail.updated_at) })
  }
  const server = renderBlocks([{ key: 'server', node: descriptionsBlock(serverRows) }])
  if (server !== null) {
    items.push({ key: 'server', label: 'Servidor', children: server })
  }

  // ── Otros ─────────────────────────────────────────────────────────────────
  const others = renderBlocks([{ key: 'others', node: othersBlock(detail.others) }])
  if (others !== null) {
    items.push({ key: 'others', label: 'Otros', children: others })
  }

  if (items.length === 0) {
    return <Empty description="Este contacto no tiene datos que mostrar" />
  }

  return <Tabs items={items} destroyOnHidden />
}
