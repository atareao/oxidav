/**
 * Vista Ajustes y Perfil (F14).
 *
 * Tres bloques independientes:
 *  - **Perfil**: el `sub`/usuario autenticado (claim de Pocket ID) vía
 *    `GET /api/v1/me`.
 *  - **Apariencia**: conmutador claro/oscuro sobre el {@link ThemeProvider}
 *    existente (el modo se persiste en `localStorage`).
 *  - **Colecciones**: tabla con crear, renombrar y borrar (con confirmación)
 *    contra `/api/v1/collections`.
 *
 * Sigue el patrón ya probado de F11–F13: `App.useApp()` para los mensajes
 * (hereda el tema activo), `confirmLoading` + guarda síncrona contra el doble
 * submit, `AbortController`/`signal.aborted` para descartar respuestas obsoletas
 * y refresco optimista de la tabla tras cada mutación.
 */

import {
  Alert,
  App,
  Button,
  Card,
  Descriptions,
  Empty,
  Form,
  Input,
  Modal,
  Popconfirm,
  Segmented,
  Select,
  Space,
  Table,
  Tag,
  Typography,
} from 'antd'
import type { TableColumnsType } from 'antd'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTheme, type ThemeMode } from '../contexts/ThemeContext'
import { getCurrentUser } from '../services/appTokens'
import { toErrorMessage } from '../services/api'
import {
  createCollection,
  deleteCollection,
  listCollections,
  updateCollection,
  type CreateCollectionInput,
} from '../services/collections'
import type { Collection, CollectionType } from '../types/api'

/** Shape of the create-collection `Form`. */
interface CreateCollectionFormValues {
  name: string
  type: CollectionType
}

/** Shape of the rename-collection `Form`. */
interface RenameCollectionFormValues {
  name: string
}

/** Human-readable label for each collection kind. */
const TYPE_LABELS: Record<CollectionType, string> = {
  calendar: 'Calendario',
  addressbook: 'Libreta de direcciones',
}

/** Ant Design tag colour for each collection kind. */
const TYPE_COLORS: Record<CollectionType, string> = {
  calendar: 'blue',
  addressbook: 'green',
}

export function Settings() {
  // Context-aware message API: unlike the static `message` export it inherits
  // the active Ant Design theme (light/dark) from the closest `<App>` provider.
  const { message } = App.useApp()
  const { mode, setMode } = useTheme()

  const [username, setUsername] = useState<string | null>(null)
  const [collections, setCollections] = useState<Collection[]>([])
  const [error, setError] = useState<string | null>(null)
  // Loading is *derived* from a request key instead of being written
  // synchronously inside the effect (forbidden by `react-hooks/set-state-in-effect`).
  const [loadedKey, setLoadedKey] = useState<string | null>(null)
  const [reloadToken, setReloadToken] = useState(0)

  const [createOpen, setCreateOpen] = useState(false)
  // `creating` drives the Modal's `confirmLoading`; the ref is the synchronous
  // guard that drops the second of two submits fired in the same tick.
  const [creating, setCreating] = useState(false)
  const creatingRef = useRef(false)

  const [renameOpen, setRenameOpen] = useState(false)
  const [editing, setEditing] = useState<Collection | null>(null)
  const [renaming, setRenaming] = useState(false)
  const renamingRef = useRef(false)

  // Synchronous re-entry guard: a second Popconfirm confirm fired in the same
  // tick (double-click) must not launch a parallel DELETE.
  const deletingRef = useRef(false)

  const [createForm] = Form.useForm<CreateCollectionFormValues>()
  const [renameForm] = Form.useForm<RenameCollectionFormValues>()

  const requestKey = `settings#${reloadToken}`
  useEffect(() => {
    const controller = new AbortController()
    // Profile and collections feed the same view; one request key keeps the
    // loading/error handling simple.
    Promise.all([listCollections(controller.signal), getCurrentUser(controller.signal)])
      .then(([items, me]) => {
        // Stale-response guard: a newer request (or unmount) aborted us while
        // this response was in flight, so it must not overwrite fresher state.
        if (controller.signal.aborted) {
          return
        }
        setCollections(items)
        setUsername(me.sub.length > 0 ? me.sub : null)
        setError(null)
        setLoadedKey(requestKey)
      })
      .catch((err: unknown) => {
        if (!controller.signal.aborted) {
          setError(toErrorMessage(err))
          setLoadedKey(requestKey)
        }
      })
    return () => {
      controller.abort()
    }
  }, [requestKey])

  const loading = loadedKey !== requestKey

  const openCreate = useCallback(() => {
    createForm.setFieldsValue({ name: '', type: 'calendar' })
    setCreateOpen(true)
  }, [createForm])

  const handleCreate = useCallback(
    (values: CreateCollectionFormValues) => {
      // Drop submits while one is already in flight (double-click / Enter spam).
      if (creatingRef.current) {
        return
      }
      const name = values.name.trim()
      if (name.length === 0) {
        return
      }
      creatingRef.current = true
      setCreating(true)
      const input: CreateCollectionInput = { name, type: values.type }
      createCollection(input)
        .then((created) => {
          // Optimistic refresh: fold the new collection into the table without
          // a second round-trip.
          setCollections((previous) => [created, ...previous])
          setCreateOpen(false)
          createForm.resetFields()
          message.success('Colección creada')
        })
        .catch((err: unknown) => {
          message.error(toErrorMessage(err))
        })
        .finally(() => {
          creatingRef.current = false
          setCreating(false)
        })
    },
    [createForm, message],
  )

  const openRename = useCallback(
    (collection: Collection) => {
      setEditing(collection)
      renameForm.setFieldsValue({ name: collection.name })
      setRenameOpen(true)
    },
    [renameForm],
  )

  const handleRename = useCallback(
    (values: RenameCollectionFormValues) => {
      const target = editing
      if (target === null || renamingRef.current) {
        return
      }
      const name = values.name.trim()
      if (name.length === 0) {
        return
      }
      renamingRef.current = true
      setRenaming(true)
      updateCollection(target.id, { name })
        .then((updated) => {
          setCollections((previous) =>
            previous.map((item) => (item.id === updated.id ? updated : item)),
          )
          setRenameOpen(false)
          setEditing(null)
          renameForm.resetFields()
          message.success('Colección renombrada')
        })
        .catch((err: unknown) => {
          message.error(toErrorMessage(err))
        })
        .finally(() => {
          renamingRef.current = false
          setRenaming(false)
        })
    },
    [editing, message, renameForm],
  )

  const handleDelete = useCallback(
    (collection: Collection) => {
      if (deletingRef.current) {
        return
      }
      deletingRef.current = true
      deleteCollection(collection.id)
        .then(() => {
          // Optimistic refresh: drop the row once the server confirms.
          setCollections((previous) => previous.filter((item) => item.id !== collection.id))
          message.success('Colección eliminada')
        })
        .catch((err: unknown) => {
          message.error(toErrorMessage(err))
        })
        .finally(() => {
          deletingRef.current = false
        })
    },
    [message],
  )

  const typeOptions = useMemo(
    () => [
      { value: 'calendar' as const, label: TYPE_LABELS.calendar },
      { value: 'addressbook' as const, label: TYPE_LABELS.addressbook },
    ],
    [],
  )

  const columns: TableColumnsType<Collection> = useMemo(
    () => [
      {
        title: 'Nombre',
        dataIndex: 'name',
        key: 'name',
        sorter: (a, b) => a.name.localeCompare(b.name),
      },
      {
        title: 'Tipo',
        dataIndex: 'type',
        key: 'type',
        render: (type: CollectionType) => (
          <Tag color={TYPE_COLORS[type]}>{TYPE_LABELS[type]}</Tag>
        ),
      },
      {
        title: 'Acciones',
        key: 'actions',
        render: (_value, collection) => (
          <Space>
            <Button
              type="link"
              size="small"
              aria-label={`Renombrar ${collection.name}`}
              onClick={() => openRename(collection)}
            >
              Renombrar
            </Button>
            <Popconfirm
              title="Eliminar colección"
              description="Se eliminarán también todos sus objetos. ¿Continuar?"
              okText="Eliminar"
              cancelText="Cancelar"
              okButtonProps={{ danger: true }}
              onConfirm={() => handleDelete(collection)}
            >
              <Button danger type="link" size="small" aria-label={`Eliminar ${collection.name}`}>
                Eliminar
              </Button>
            </Popconfirm>
          </Space>
        ),
      },
    ],
    [handleDelete, openRename],
  )

  const handleModeChange = useCallback(
    (value: ThemeMode) => {
      setMode(value)
    },
    [setMode],
  )

  return (
    <>
      <h1 className="sr-only">Ajustes</h1>

      {error !== null && !loading ? (
        <Alert
          type="error"
          showIcon
          title="No se pudieron cargar los datos"
          description={error}
          action={
            <Button size="small" onClick={() => setReloadToken((token) => token + 1)}>
              Reintentar
            </Button>
          }
          style={{ marginBottom: 16 }}
        />
      ) : null}

      <Card title="Perfil" style={{ marginBottom: 16 }}>
        <Descriptions column={1} size="small">
          <Descriptions.Item label="Usuario">
            <Typography.Text code data-testid="profile-user">
              {loading ? '…' : (username ?? '—')}
            </Typography.Text>
          </Descriptions.Item>
        </Descriptions>
      </Card>

      <Card title="Apariencia" style={{ marginBottom: 16 }}>
        <Space align="center" wrap>
          <Typography.Text>Tema</Typography.Text>
          <Segmented<ThemeMode>
            aria-label="Tema"
            value={mode}
            onChange={handleModeChange}
            options={[
              { label: 'Claro', value: 'light' },
              { label: 'Oscuro', value: 'dark' },
            ]}
          />
        </Space>
      </Card>

      <Card
        title="Colecciones"
        extra={
          <Button type="primary" onClick={openCreate}>
            Crear colección
          </Button>
        }
      >
        <Table<Collection>
          rowKey="id"
          columns={columns}
          dataSource={collections}
          loading={loading}
          pagination={{ pageSize: 20, showSizeChanger: false }}
          locale={{
            emptyText: <Empty description="No hay colecciones" />,
          }}
          scroll={{ x: 'max-content' }}
        />
      </Card>

      <Modal
        title="Crear colección"
        open={createOpen}
        onOk={() => createForm.submit()}
        okText="Crear"
        cancelText="Cancelar"
        confirmLoading={creating}
        mask={{ closable: !creating }}
        keyboard={!creating}
        onCancel={() => {
          if (!creating) {
            setCreateOpen(false)
          }
        }}
        destroyOnHidden
      >
        <Form form={createForm} layout="vertical" onFinish={handleCreate}>
          <Form.Item
            name="name"
            label="Nombre"
            rules={[{ required: true, message: 'Introduce un nombre' }]}
          >
            <Input placeholder="p. ej. Personal" />
          </Form.Item>
          <Form.Item
            name="type"
            label="Tipo"
            rules={[{ required: true, message: 'Selecciona un tipo' }]}
          >
            <Select options={typeOptions} />
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        title="Renombrar colección"
        open={renameOpen}
        onOk={() => renameForm.submit()}
        okText="Guardar"
        cancelText="Cancelar"
        confirmLoading={renaming}
        mask={{ closable: !renaming }}
        keyboard={!renaming}
        onCancel={() => {
          if (!renaming) {
            setRenameOpen(false)
            setEditing(null)
          }
        }}
        destroyOnHidden
      >
        <Form form={renameForm} layout="vertical" onFinish={handleRename}>
          <Form.Item
            name="name"
            label="Nombre"
            rules={[{ required: true, message: 'Introduce un nombre' }]}
          >
            <Input placeholder="Nuevo nombre" />
          </Form.Item>
        </Form>
      </Modal>
    </>
  )
}
