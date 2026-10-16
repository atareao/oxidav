/**
 * Vista Sincronización y dispositivos (F13).
 *
 * Reúne lo que un cliente DAV nativo necesita para conectar: las URLs DAV
 * absolutas copiables, instrucciones breves para iOS/DAVx⁵/Thunderbird y el
 * gestor de *app tokens* (crear, listar y revocar).
 *
 * Sigue el patrón ya probado de F11/F12: `App.useApp()` para los mensajes
 * (hereda el tema activo), `confirmLoading` + guarda síncrona contra el doble
 * submit, `AbortController` para cancelar la carga al desmontar y refresco
 * optimista de la tabla tras crear o revocar.
 *
 * El secreto en claro se muestra **una única vez**, en un diálogo no
 * persistente que se limpia al cerrarlo: nunca se guarda en estado global,
 * `localStorage` ni logs.
 */

import {
  Alert,
  App,
  Button,
  Card,
  Descriptions,
  Empty,
  Flex,
  Form,
  Input,
  Modal,
  Popconfirm,
  Space,
  Table,
  Typography,
} from 'antd'
import type { TableColumnsType } from 'antd'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  createAppToken,
  getCurrentUser,
  listAppTokens,
  revokeAppToken,
} from '../services/appTokens'
import { toErrorMessage } from '../services/api'
import type { AppToken, CreatedAppToken } from '../types/api'
import { copyToClipboard } from '../utils/clipboard'
import { PAGE_GAP } from './pageLayout'

/** Shape of the create-token `Form`. */
interface CreateTokenFormValues {
  name: string
}

const dateTimeFormatter = new Intl.DateTimeFormat('es-ES', {
  dateStyle: 'medium',
  timeStyle: 'short',
})

/** Render an ISO-8601 timestamp for the table (`null` → an em dash). */
function formatTimestamp(value: string | null): string {
  if (value === null) {
    return '—'
  }
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '—' : dateTimeFormatter.format(date)
}

/**
 * Compose the absolute DAV URLs from the current origin.
 *
 * The path segment is the Pocket ID `sub`/username; when it is not (yet) known
 * a neutral `<usuario>` placeholder is shown rather than guessing, so nobody
 * copies a wrong URL.
 */
function useDavUrls(username: string | null): { caldav: string; carddav: string } {
  return useMemo(() => {
    const origin = window.location.origin
    const segment = username === null ? '<usuario>' : encodeURIComponent(username)
    return {
      caldav: `${origin}/dav/calendars/${segment}/`,
      carddav: `${origin}/dav/addressbooks/${segment}/`,
    }
  }, [username])
}

export function Sync() {
  // Context-aware message API: unlike the static `message` export it inherits
  // the active Ant Design theme (light/dark) from the closest `<App>` provider.
  const { message } = App.useApp()

  const [username, setUsername] = useState<string | null>(null)
  const [tokens, setTokens] = useState<AppToken[]>([])
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
  // The created token is held *only* while its reveal dialog is open.
  const [created, setCreated] = useState<CreatedAppToken | null>(null)

  const [form] = Form.useForm<CreateTokenFormValues>()

  const requestKey = `app-tokens#${reloadToken}`
  useEffect(() => {
    const controller = new AbortController()
    // Resolve the tokens and the username together: both feed the same view and
    // a single request key keeps the loading/error handling simple.
    Promise.all([listAppTokens(controller.signal), getCurrentUser(controller.signal)])
      .then(([list, me]) => {
        // Stale-response guard: a newer request (or unmount) aborted us while
        // this response was in flight, so it must not overwrite fresher state.
        if (controller.signal.aborted) {
          return
        }
        setTokens(list)
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

  const { caldav, carddav } = useDavUrls(username)

  const handleCopy = useCallback(
    async (text: string) => {
      const ok = await copyToClipboard(text)
      if (ok) {
        message.success('Copiado al portapapeles')
      } else {
        message.error('No se pudo copiar al portapapeles')
      }
    },
    [message],
  )

  const handleCreate = useCallback(
    (values: CreateTokenFormValues) => {
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
      createAppToken(name)
        .then((createdToken) => {
          // Optimistic refresh: fold the new (secret-free) metadata into the
          // table immediately, without a second round-trip.
          setTokens((previous) => [
            {
              id: createdToken.id,
              name: createdToken.name,
              created_at: createdToken.created_at,
              last_used_at: null,
            },
            ...previous,
          ])
          setCreateOpen(false)
          form.resetFields()
          // Reveal-once dialog; cleared as soon as it is dismissed.
          setCreated(createdToken)
          message.success('Token creado')
        })
        .catch((err: unknown) => {
          message.error(toErrorMessage(err))
        })
        .finally(() => {
          creatingRef.current = false
          setCreating(false)
        })
    },
    [form, message],
  )

  const handleRevoke = useCallback(
    (token: AppToken) => {
      revokeAppToken(token.id)
        .then(() => {
          // Optimistic refresh: drop the row once the server confirms.
          setTokens((previous) => previous.filter((item) => item.id !== token.id))
          message.success('Token revocado')
        })
        .catch((err: unknown) => {
          message.error(toErrorMessage(err))
        })
    },
    [message],
  )

  const columns: TableColumnsType<AppToken> = useMemo(
    () => [
      {
        title: 'Nombre',
        dataIndex: 'name',
        key: 'name',
        sorter: (a, b) => a.name.localeCompare(b.name),
      },
      {
        title: 'Creado',
        dataIndex: 'created_at',
        key: 'created_at',
        sorter: (a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime(),
        render: (value: string) => formatTimestamp(value),
      },
      {
        title: 'Último uso',
        dataIndex: 'last_used_at',
        key: 'last_used_at',
        render: (value: string | null) => formatTimestamp(value),
      },
      {
        title: 'Acciones',
        key: 'actions',
        render: (_value, token) => (
          <Popconfirm
            title="Revocar token"
            description="Los dispositivos que lo usen dejarán de sincronizar. ¿Continuar?"
            okText="Revocar"
            cancelText="Cancelar"
            okButtonProps={{ danger: true }}
            onConfirm={() => handleRevoke(token)}
          >
            <Button danger type="link" size="small" aria-label={`Revocar ${token.name}`}>
              Revocar
            </Button>
          </Popconfirm>
        ),
      },
    ],
    [handleRevoke],
  )

  return (
    <>
      <h1 className="sr-only">Sincronización y dispositivos</h1>

      <Flex vertical gap={PAGE_GAP}>
        {error !== null ? (
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
          />
        ) : null}

        <Card title="URLs DAV">
          <Typography.Paragraph type="secondary">
            Configura estos servidores en tus clientes CalDAV/CardDAV. La autenticación es
            <Typography.Text strong> Basic Auth</Typography.Text>: usa tu usuario y un token de
            aplicación (sección «Tokens de aplicación») como contraseña.
          </Typography.Paragraph>
          <Descriptions column={1} bordered size="small">
            <Descriptions.Item label="CalDAV">
              <Space wrap>
                <Typography.Text code data-testid="caldav-url">
                  {caldav}
                </Typography.Text>
                <Button
                  size="small"
                  aria-label="Copiar URL CalDAV"
                  onClick={() => {
                    void handleCopy(caldav)
                  }}
                >
                  Copiar
                </Button>
              </Space>
            </Descriptions.Item>
            <Descriptions.Item label="CardDAV">
              <Space wrap>
                <Typography.Text code data-testid="carddav-url">
                  {carddav}
                </Typography.Text>
                <Button
                  size="small"
                  aria-label="Copiar URL CardDAV"
                  onClick={() => {
                    void handleCopy(carddav)
                  }}
                >
                  Copiar
                </Button>
              </Space>
            </Descriptions.Item>
          </Descriptions>
        </Card>

        <Card title="Cómo conectar tus dispositivos">
          <Typography.Paragraph>
            <Typography.Text strong>iPhone / iPad (iOS):</Typography.Text> Ajustes → Apps → Calendario →
            Cuentas → Añadir cuenta → Otra → «Añadir cuenta CalDAV». Introduce la URL CalDAV, tu usuario
            y el token como contraseña. Para contactos, repite con «Añadir cuenta CardDAV».
          </Typography.Paragraph>
          <Typography.Paragraph>
            <Typography.Text strong>Android (DAVx⁵):</Typography.Text> Añade una cuenta, elige «Iniciar
            sesión con URL y usuario», pega la URL CalDAV o CardDAV, tu usuario y el token.
          </Typography.Paragraph>
          <Typography.Paragraph style={{ marginBottom: 0 }}>
            <Typography.Text strong>Thunderbird:</Typography.Text> Añade un calendario o una libreta de
            direcciones «en la red», introduce la URL correspondiente y selecciona autenticación básica
            con tu usuario y el token.
          </Typography.Paragraph>
        </Card>

        <Card
          title="Tokens de aplicación"
          extra={
            <Button type="primary" onClick={() => setCreateOpen(true)}>
              Crear token
            </Button>
          }
        >
          <Table<AppToken>
            rowKey="id"
            columns={columns}
            dataSource={tokens}
            loading={loading}
            pagination={{ pageSize: 20, showSizeChanger: false }}
            locale={{
              emptyText: <Empty description="No hay tokens de aplicación" />,
            }}
            scroll={{ x: 'max-content' }}
          />
        </Card>
      </Flex>

      <Modal
        title="Crear token de aplicación"
        open={createOpen}
        onOk={() => form.submit()}
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
        <Form form={form} layout="vertical" onFinish={handleCreate}>
          <Form.Item
            name="name"
            label="Nombre"
            rules={[{ required: true, message: 'Introduce un nombre' }]}
          >
            <Input placeholder="p. ej. iPhone de Ada" />
          </Form.Item>
        </Form>
      </Modal>

      {/*
        The reveal dialog is *unmounted* (not merely hidden) the moment it is
        dismissed. `destroyOnHidden` alone is not enough: while `open` goes
        false the dialog plays its leave transition and `rc-dialog`'s memoized
        children keep the previous subtree mounted, so the plaintext secret
        would linger in the DOM until the animation ends. Unmounting the whole
        `<Modal>` guarantees the secret leaves the DOM at once — in a real
        browser and under jsdom alike.
      */}
      {created !== null ? (
        <Modal
          title="Token creado"
          open
          onCancel={() => setCreated(null)}
          mask={{ closable: false }}
          keyboard={false}
          footer={[
            <Button
              key="copy"
              onClick={() => {
                void handleCopy(created.token)
              }}
            >
              Copiar token
            </Button>,
            <Button key="close" type="primary" onClick={() => setCreated(null)}>
              Cerrar
            </Button>,
          ]}
          destroyOnHidden
        >
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 16 }}
            title="Este token no volverá a mostrarse"
            description="Cópialo ahora y guárdalo en un lugar seguro. Si lo pierdes, revócalo y crea otro."
          />
          <Typography.Paragraph type="secondary">
            Token de «{created.name}»
          </Typography.Paragraph>
          <Input
            readOnly
            aria-label="Token de aplicación"
            value={created.token}
            onFocus={(event) => event.target.select()}
          />
        </Modal>
      ) : null}
    </>
  )
}
