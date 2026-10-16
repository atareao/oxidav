/**
 * Vista Contactos (F12, detalle completo en F22).
 *
 * Réplica del patrón de la vista Calendario (F11): tabla Antd con búsqueda y
 * orden, `<Drawer>` de detalle, alta/edición en `<Modal>` + `<Form>` y
 * actualizaciones optimistas con React 19 `useOptimistic` (rollback ante error).
 *
 * El `<Drawer>` ya no pinta un `<Descriptions>` con los cuatro campos de la
 * lista: al abrir un contacto pide su detalle completo (`useContactDetail`) y lo
 * organiza en pestañas con `ContactDetailTabs`. La tabla sigue trabajando con el
 * DTO ligero (de ahí salen el título y los botones Editar/Eliminar).
 *
 * Import/export `.vcf`: el export se compone en cliente (solo campos básicos) y
 * la importación envía el fichero **en crudo** al servidor
 * (`POST .../contacts/import`), que lo parsea sin pérdida; el SPA no parsea.
 *
 * Actualizaciones optimistas: cada mutación se ejecuta dentro de un
 * `startTransition`; al terminar la transición la capa optimista se reconcilia
 * automáticamente, por lo que un fallo de la API revierte la UI al estado previo
 * y el error se muestra con `message` de Ant Design.
 */

import {
  Alert,
  App,
  Button,
  Card,
  Drawer,
  Empty,
  Flex,
  Form,
  Input,
  Modal,
  Popconfirm,
  Select,
  Skeleton,
  Space,
  Table,
  Tooltip,
} from 'antd'
import type { TableColumnsType } from 'antd'
import {
  startTransition,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useOptimistic,
  type ChangeEvent,
} from 'react'
import { ContactDetailTabs } from '../components/contact/ContactDetailTabs'
import { useContactDetail } from '../hooks/useContactDetail'
import { useContacts } from '../hooks/useContacts'
import { toErrorMessage } from '../services/api'
import {
  createContact,
  deleteContact,
  importContacts,
  listAddressbookCollections,
  updateContact,
} from '../services/contacts'
import type { Collection, Contact, CreateContactInput, UpdateContactInput } from '../types/api'
import { tempId } from '../utils/tempId'
import { downloadTextFile, MAX_FILE_BYTES, serializeContactsToVcf } from '../utils/vcf'
import { PAGE_GAP, fluidControlStyle } from './pageLayout'

/** Shape of the `Form` values used by the create/edit modal. */
interface ContactFormValues {
  fn: string
  email?: string
  phone?: string
  organization?: string
}

/** Fields the optimistic layer may rewrite on an existing contact. */
type ContactChanges = Pick<Contact, 'fn' | 'email' | 'phone' | 'organization'>

type OptimisticAction =
  | { kind: 'create'; contact: Contact }
  | { kind: 'update'; uid: string; changes: ContactChanges }
  | { kind: 'delete'; uid: string }

/** Pure reducer applied by `useOptimistic` on top of the server-confirmed list. */
function applyOptimistic(contacts: Contact[], action: OptimisticAction): Contact[] {
  switch (action.kind) {
    case 'create':
      return [...contacts, action.contact]
    case 'update':
      return contacts.map((contact) =>
        contact.uid === action.uid ? { ...contact, ...action.changes } : contact,
      )
    case 'delete':
      return contacts.filter((contact) => contact.uid !== action.uid)
  }
}

/** Display name of a contact (`fn`, falling back to its `full_name` alias). */
function displayName(contact: Contact): string {
  const name = contact.fn ?? contact.full_name ?? ''
  return name.trim().length > 0 ? name : '(sin nombre)'
}

/** Trim a form value to `string | undefined` (blank → omitted). */
function optionalValue(value: string | undefined): string | undefined {
  const trimmed = value?.trim() ?? ''
  return trimmed.length > 0 ? trimmed : undefined
}

export function Contacts() {
  // Context-aware message API: unlike the static `message` export it inherits
  // the active Ant Design theme (light/dark) from the closest `<App>` provider.
  const { message } = App.useApp()

  const [selectedCollectionId, setSelectedCollectionId] = useState<string | null>(null)
  const [collections, setCollections] = useState<Collection[]>([])
  const [collectionsError, setCollectionsError] = useState<string | null>(null)
  const [collectionsKey, setCollectionsKey] = useState<string | null>(null)
  const [collectionsReloadToken, setCollectionsReloadToken] = useState(0)

  const [search, setSearch] = useState('')
  const [detail, setDetail] = useState<Contact | null>(null)
  const [modalOpen, setModalOpen] = useState(false)
  const [editing, setEditing] = useState<Contact | null>(null)
  // `saving` drives the Modal's `confirmLoading` (disables OK); the ref is the
  // synchronous guard that drops the *second* of two submits fired in the same
  // tick, before React has re-rendered the disabled button.
  const [saving, setSaving] = useState(false)
  const savingRef = useRef(false)

  // Import is a bulk operation with its own in-flight flag/guard.
  const [importing, setImporting] = useState(false)
  const importingRef = useRef(false)
  const fileInputRef = useRef<HTMLInputElement>(null)

  const [form] = Form.useForm<ContactFormValues>()

  // Load (only) the address-book collections. Loading state is *derived* from a
  // request key instead of being written synchronously inside the effect, which
  // the `react-hooks/set-state-in-effect` rule forbids.
  const collectionsRequestKey = `addressbook-collections#${collectionsReloadToken}`
  useEffect(() => {
    const controller = new AbortController()
    listAddressbookCollections(controller.signal)
      .then((items) => {
        // Stale-response guard: a newer request (or unmount) aborted us while
        // this response was in flight, so it must not overwrite fresher state.
        if (controller.signal.aborted) {
          return
        }
        setCollections(items)
        setCollectionsError(null)
        setCollectionsKey(collectionsRequestKey)
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          setCollectionsError(toErrorMessage(error))
          setCollectionsKey(collectionsRequestKey)
        }
      })
    return () => {
      controller.abort()
    }
  }, [collectionsRequestKey])

  const collectionsLoading = collectionsKey !== collectionsRequestKey

  // Derive the selected collection instead of storing it via an effect: if the
  // stored id is gone (or none was picked yet), fall back to the first one.
  const effectiveCollectionId = useMemo(() => {
    if (
      selectedCollectionId !== null &&
      collections.some((item) => item.id === selectedCollectionId)
    ) {
      return selectedCollectionId
    }
    return collections[0]?.id ?? null
  }, [selectedCollectionId, collections])

  const {
    contacts,
    setContacts,
    loading: contactsLoading,
    error: contactsError,
    reload: reloadContacts,
  } = useContacts(effectiveCollectionId)

  const [optimisticContacts, addOptimistic] = useOptimistic(contacts, applyOptimistic)

  // El `Drawer` se abre con el DTO ligero de la tabla (`detail`), pero el
  // contenido de las pestañas sale de una petición aparte al detalle completo.
  // El objetivo del hook es `null` mientras no haya contacto o libreta.
  const detailTarget = useMemo(() => {
    if (detail === null || effectiveCollectionId === null) {
      return null
    }
    return { collectionId: effectiveCollectionId, uid: detail.uid }
  }, [detail, effectiveCollectionId])

  const {
    detail: fullDetail,
    loading: detailLoading,
    error: detailError,
    reload: reloadDetail,
  } = useContactDetail(detailTarget)

  // Nombre legible de la libreta para la pestaña «Servidor»; las colecciones ya
  // están cargadas para el `<Select>`.
  const detailCollectionName = useMemo(() => {
    const collectionId = fullDetail?.collection_id ?? detail?.collection_id
    if (collectionId === undefined || collectionId === null) {
      return undefined
    }
    return collections.find((collection) => collection.id === collectionId)?.name
  }, [collections, fullDetail, detail])

  const filteredContacts = useMemo(() => {
    const query = search.trim().toLowerCase()
    if (query.length === 0) {
      return optimisticContacts
    }
    return optimisticContacts.filter((contact) =>
      [contact.fn, contact.full_name, contact.email, contact.phone, contact.organization].some(
        (value) => (value ?? '').toLowerCase().includes(query),
      ),
    )
  }, [optimisticContacts, search])

  const closeModal = useCallback(() => {
    setModalOpen(false)
    setEditing(null)
  }, [])

  const openCreate = useCallback(() => {
    setEditing(null)
    setModalOpen(true)
  }, [])

  const openEdit = useCallback((contact: Contact) => {
    setDetail(null)
    setEditing(contact)
    setModalOpen(true)
  }, [])

  const handleSubmit = useCallback(
    (values: ContactFormValues) => {
      // Drop submits while one is already in flight (double-click / Enter spam).
      if (savingRef.current) {
        return
      }
      const collectionId = effectiveCollectionId
      if (collectionId === null) {
        return
      }
      const email = optionalValue(values.email)
      const phone = optionalValue(values.phone)
      const organization = optionalValue(values.organization)
      savingRef.current = true
      setSaving(true)

      // Always clear the in-flight flag, on success or failure.
      const finish = () => {
        savingRef.current = false
        setSaving(false)
      }

      if (editing === null) {
        const optimisticId = tempId('contact')
        startTransition(async () => {
          addOptimistic({
            kind: 'create',
            contact: {
              id: optimisticId,
              uid: optimisticId,
              collection_id: collectionId,
              fn: values.fn,
              email: email ?? null,
              phone: phone ?? null,
              organization: organization ?? null,
              etag: '',
              created_at: Date.now(),
              updated_at: Date.now(),
            },
          })
          try {
            const input: CreateContactInput = { fn: values.fn }
            if (email !== undefined) {
              input.email = email
            }
            if (phone !== undefined) {
              input.phone = phone
            }
            if (organization !== undefined) {
              input.organization = organization
            }
            const created = await createContact(collectionId, input)
            setContacts((previous) => [...previous, created])
            closeModal()
            message.success('Contacto creado')
          } catch (error) {
            // Rollback happens automatically when the transition ends; surface
            // the failure with the context message API.
            message.error(toErrorMessage(error))
          } finally {
            finish()
          }
        })
        return
      }

      const target = editing
      startTransition(async () => {
        addOptimistic({
          kind: 'update',
          uid: target.uid,
          changes: {
            fn: values.fn,
            email: email ?? null,
            phone: phone ?? null,
            organization: organization ?? null,
          },
        })
        try {
          const input: UpdateContactInput = { fn: values.fn }
          if (email !== undefined) {
            input.email = email
          }
          if (phone !== undefined) {
            input.phone = phone
          }
          if (organization !== undefined) {
            input.organization = organization
          }
          const updated = await updateContact(collectionId, target.uid, input)
          setContacts((previous) =>
            previous.map((contact) => (contact.uid === target.uid ? updated : contact)),
          )
          closeModal()
          message.success('Contacto actualizado')
        } catch (error) {
          message.error(toErrorMessage(error))
        } finally {
          finish()
        }
      })
    },
    [addOptimistic, closeModal, editing, effectiveCollectionId, message, setContacts],
  )

  const handleDelete = useCallback(
    (contact: Contact) => {
      const collectionId = effectiveCollectionId
      if (collectionId === null) {
        return
      }
      setDetail(null)
      startTransition(async () => {
        addOptimistic({ kind: 'delete', uid: contact.uid })
        try {
          await deleteContact(collectionId, contact.uid)
          setContacts((previous) => previous.filter((item) => item.uid !== contact.uid))
          message.success('Contacto eliminado')
        } catch (error) {
          message.error(toErrorMessage(error))
        }
      })
    },
    [addOptimistic, effectiveCollectionId, message, setContacts],
  )

  const handleExport = useCallback(() => {
    const content = serializeContactsToVcf(optimisticContacts)
    if (content.length === 0) {
      message.warning('No hay contactos para exportar')
      return
    }
    downloadTextFile('contactos.vcf', content)
    message.success('Contactos exportados')
  }, [message, optimisticContacts])

  const handleImportClick = useCallback(() => {
    fileInputRef.current?.click()
  }, [])

  const handleFileChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0]
      // Allow re-selecting the same file later.
      event.target.value = ''
      if (file === undefined) {
        return
      }
      const collectionId = effectiveCollectionId
      if (collectionId === null) {
        message.error('Selecciona una libreta de direcciones')
        return
      }
      if (importingRef.current) {
        return
      }
      // Reject oversized uploads before reading them into memory.
      if (file.size > MAX_FILE_BYTES) {
        message.error('El archivo es demasiado grande (máximo 5 MB)')
        return
      }
      importingRef.current = true
      setImporting(true)

      void (async () => {
        try {
          const text = await file.text()
          // La importación pasa al servidor (F22): se envía el texto **en
          // crudo** como `text/vcard` y es el backend quien parsea y almacena
          // cada tarjeta verbatim. El SPA no parsea vCard. La validación es
          // atómica (o entran todas las tarjetas o ninguna), así que la
          // respuesta trae todas las creadas o el sobre de error.
          const created = await importContacts(collectionId, text)
          if (created.length === 0) {
            message.warning('El archivo no contiene contactos válidos')
            return
          }
          setContacts((previous) => [...previous, ...created])
          message.success(`${created.length} contacto(s) importado(s)`)
        } catch (error) {
          message.error(toErrorMessage(error))
        } finally {
          importingRef.current = false
          setImporting(false)
        }
      })()
    },
    [effectiveCollectionId, message, setContacts],
  )

  const columns: TableColumnsType<Contact> = useMemo(
    () => [
      {
        title: 'Nombre',
        dataIndex: 'fn',
        key: 'fn',
        sorter: (a, b) => displayName(a).localeCompare(displayName(b)),
        render: (_value, contact) => displayName(contact),
      },
      {
        title: 'Email',
        dataIndex: 'email',
        key: 'email',
        sorter: (a, b) => (a.email ?? '').localeCompare(b.email ?? ''),
        render: (value: string | null) => value ?? '—',
      },
      {
        title: 'Teléfono',
        dataIndex: 'phone',
        key: 'phone',
        sorter: (a, b) => (a.phone ?? '').localeCompare(b.phone ?? ''),
        render: (value: string | null) => value ?? '—',
      },
      {
        title: 'Organización',
        dataIndex: 'organization',
        key: 'organization',
        sorter: (a, b) => (a.organization ?? '').localeCompare(b.organization ?? ''),
        render: (value: string | null) => value ?? '—',
      },
      {
        title: 'Acciones',
        key: 'actions',
        render: (_value, contact) => (
          <Space onClick={(event) => event.stopPropagation()}>
            <Button
              type="link"
              size="small"
              aria-label={`Ver ${displayName(contact)}`}
              onClick={() => setDetail(contact)}
            >
              Ver
            </Button>
            <Button
              type="link"
              size="small"
              aria-label={`Editar ${displayName(contact)}`}
              onClick={() => openEdit(contact)}
            >
              Editar
            </Button>
            <Popconfirm
              title="Eliminar contacto"
              description="¿Seguro que quieres eliminar este contacto?"
              okText="Eliminar"
              cancelText="Cancelar"
              okButtonProps={{ danger: true }}
              onConfirm={() => handleDelete(contact)}
            >
              <Button danger type="link" size="small" aria-label={`Eliminar ${displayName(contact)}`}>
                Eliminar
              </Button>
            </Popconfirm>
          </Space>
        ),
      },
    ],
    [handleDelete, openEdit],
  )

  const loading = collectionsLoading || (effectiveCollectionId !== null && contactsLoading)
  const loadError = collectionsError ?? (effectiveCollectionId !== null ? contactsError : null)

  const formInitialValues: ContactFormValues = useMemo(() => {
    if (editing !== null) {
      return {
        fn: editing.fn ?? editing.full_name ?? '',
        email: editing.email ?? '',
        phone: editing.phone ?? '',
        organization: editing.organization ?? '',
      }
    }
    return { fn: '', email: '', phone: '', organization: '' }
  }, [editing])

  return (
    <>
      <h1 className="sr-only">Contactos</h1>

      <Flex vertical gap={PAGE_GAP}>
        {loadError !== null ? (
          <Alert
            type="error"
            showIcon
            title="No se pudieron cargar los datos"
            description={loadError}
            action={
              <Button
                size="small"
                onClick={() => {
                  setCollectionsReloadToken((token) => token + 1)
                  reloadContacts()
                }}
              >
                Reintentar
              </Button>
            }
          />
        ) : null}

        <Card
          title="Libreta de direcciones"
          extra={
            <Space wrap>
              <Select
                aria-label="Libreta"
                style={fluidControlStyle}
                placeholder="Selecciona una libreta"
                value={effectiveCollectionId ?? undefined}
                onChange={(value: string) => setSelectedCollectionId(value)}
                loading={collectionsLoading}
                disabled={collections.length === 0}
                options={collections.map((collection) => ({
                  value: collection.id,
                  label: collection.name,
                }))}
              />
              <Input.Search
                allowClear
                aria-label="Buscar contactos"
                placeholder="Buscar contactos"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                style={fluidControlStyle}
              />
              <Tooltip title="El .vcf exportado incluye solo los campos básicos; el fichero completo vive en el servidor">
                <Button onClick={handleExport} disabled={optimisticContacts.length === 0}>
                  Exportar .vcf
                </Button>
              </Tooltip>
              <Tooltip title="Importa uno o varios contactos desde un fichero .vcf">
                <Button onClick={handleImportClick} loading={importing} disabled={collections.length === 0}>
                  Importar .vcf
                </Button>
              </Tooltip>
              <Button type="primary" onClick={openCreate} disabled={effectiveCollectionId === null}>
                Nuevo contacto
              </Button>
            </Space>
          }
        >
          <input
            ref={fileInputRef}
            type="file"
            accept=".vcf,text/vcard,text/x-vcard"
            aria-label="Importar archivo .vcf"
            style={{ display: 'none' }}
            onChange={handleFileChange}
          />
          {collections.length === 0 && !loading ? (
            <Empty description="No hay libretas de direcciones disponibles" />
          ) : (
            <Table<Contact>
              rowKey="uid"
              columns={columns}
              dataSource={filteredContacts}
              loading={loading}
              pagination={{ pageSize: 20, showSizeChanger: false }}
              locale={{
                emptyText: <Empty description="No hay contactos" />,
              }}
              onRow={(contact) => ({
                onClick: () => setDetail(contact),
                style: { cursor: 'pointer' },
              })}
              scroll={{ x: 'max-content' }}
            />
          )}
        </Card>
      </Flex>

      <Drawer
        title={detail === null ? 'Contacto' : displayName(detail)}
        placement="right"
        width={420}
        open={detail !== null}
        onClose={() => setDetail(null)}
        extra={
          detail !== null ? (
            <Space>
              <Button onClick={() => openEdit(detail)}>Editar</Button>
              <Popconfirm
                title="Eliminar contacto"
                description="¿Seguro que quieres eliminar este contacto?"
                okText="Eliminar"
                cancelText="Cancelar"
                okButtonProps={{ danger: true }}
                onConfirm={() => handleDelete(detail)}
              >
                <Button danger>Eliminar</Button>
              </Popconfirm>
            </Space>
          ) : null
        }
      >
        {detail !== null ? (
          detailError !== null ? (
            <Alert
              type="error"
              showIcon
              title="No se pudo cargar el detalle del contacto"
              description={detailError}
              action={
                <Button size="small" onClick={reloadDetail}>
                  Reintentar
                </Button>
              }
            />
          ) : detailLoading || fullDetail === null ? (
            <Skeleton active paragraph={{ rows: 6 }} />
          ) : (
            <ContactDetailTabs detail={fullDetail} collectionName={detailCollectionName} />
          )
        ) : null}
      </Drawer>

      <Modal
        title={editing === null ? 'Nuevo contacto' : 'Editar contacto'}
        open={modalOpen}
        onOk={() => form.submit()}
        okText={editing === null ? 'Crear' : 'Guardar'}
        cancelText="Cancelar"
        confirmLoading={saving}
        maskClosable={!saving}
        keyboard={!saving}
        onCancel={() => {
          if (!saving) {
            closeModal()
          }
        }}
        destroyOnHidden
      >
        <Form
          form={form}
          layout="vertical"
          onFinish={handleSubmit}
          initialValues={formInitialValues}
        >
          <Form.Item
            name="fn"
            label="Nombre"
            rules={[{ required: true, message: 'Introduce un nombre' }]}
          >
            <Input placeholder="Nombre completo" />
          </Form.Item>
          <Form.Item
            name="email"
            label="Email"
            rules={[{ type: 'email', message: 'Introduce un email válido' }]}
          >
            <Input placeholder="email@example.com" />
          </Form.Item>
          <Form.Item name="phone" label="Teléfono">
            <Input placeholder="+34 600 000 000" />
          </Form.Item>
          <Form.Item name="organization" label="Organización">
            <Input placeholder="Empresa" />
          </Form.Item>
        </Form>
      </Modal>
    </>
  )
}
