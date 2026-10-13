/**
 * Tests de la vista Sincronización y dispositivos (F13).
 *
 * El servicio `services/appTokens` se mockea por completo (incluido
 * `getCurrentUser`, del que depende la URL DAV) para controlar la resolución de
 * la API y verificar: composición de las URLs DAV, listado de tokens, creación
 * con revelado **único** del secreto y revocación con confirmación.
 *
 * Como en F11/F12, la vista consume el `message` *del contexto* vía
 * `App.useApp()`; se espía esa instancia para observar las llamadas reales. La
 * API global que se reemplaza (`navigator.clipboard`) se restaura en
 * `afterEach` (junto con `vi.restoreAllMocks()`).
 */

import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { App } from 'antd'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  createAppToken,
  getCurrentUser,
  listAppTokens,
  revokeAppToken,
} from '../services/appTokens'
import type { AppToken, CreatedAppToken } from '../types/api'
import { Sync } from './Sync'

vi.mock('../services/appTokens', () => ({
  listAppTokens: vi.fn(),
  createAppToken: vi.fn(),
  revokeAppToken: vi.fn(),
  getCurrentUser: vi.fn(),
}))

function makeToken(overrides: Partial<AppToken> = {}): AppToken {
  return {
    id: 't1',
    name: 'Móvil',
    created_at: '2026-01-01T10:00:00Z',
    last_used_at: null,
    ...overrides,
  }
}

/**
 * Espías sobre la API de mensajes que `App.useApp()` publica en su contexto:
 * la misma instancia que consume la vista (en lugar del export estático
 * `message`).
 */
const messageSuccess = vi.fn()
const messageError = vi.fn()

function renderSync() {
  vi.spyOn(App, 'useApp').mockReturnValue({
    message: {
      success: messageSuccess,
      error: messageError,
      info: vi.fn(),
      warning: vi.fn(),
      loading: vi.fn(),
      open: vi.fn(),
      destroy: vi.fn(),
    },
  } as unknown as ReturnType<typeof App.useApp>)

  render(
    <App>
      <Sync />
    </App>,
  )
}

// Capture the browser API this suite replaces so it does not leak across tests.
const originalClipboardDescriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')

let clipboardWriteText: ReturnType<typeof vi.fn>

/**
 * `userEvent.setup()` installs its own `navigator.clipboard` stub (for
 * copy/paste support), shadowing the one installed in `beforeEach`. Re-install
 * our spy *after* setup so copy assertions observe the real calls.
 */
function setupUser(): ReturnType<typeof userEvent.setup> {
  const user = userEvent.setup()
  Object.defineProperty(navigator, 'clipboard', {
    value: { writeText: clipboardWriteText },
    configurable: true,
  })
  return user
}

describe('Sync', () => {
  beforeEach(() => {
    vi.resetAllMocks()
    clipboardWriteText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: clipboardWriteText },
      configurable: true,
    })
  })

  afterEach(() => {
    vi.restoreAllMocks()
    if (originalClipboardDescriptor === undefined) {
      Reflect.deleteProperty(navigator, 'clipboard')
    } else {
      Object.defineProperty(navigator, 'clipboard', originalClipboardDescriptor)
    }
  })

  it('compone y muestra las URLs DAV absolutas desde el origen con el usuario', async () => {
    vi.mocked(listAppTokens).mockResolvedValue([])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    renderSync()

    const origin = window.location.origin
    expect(await screen.findByTestId('caldav-url')).toHaveTextContent(
      `${origin}/dav/calendars/alice/`,
    )
    expect(screen.getByTestId('carddav-url')).toHaveTextContent(
      `${origin}/dav/addressbooks/alice/`,
    )
  })

  it('copia la URL CalDAV al portapapeles', async () => {
    vi.mocked(listAppTokens).mockResolvedValue([])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    const user = setupUser()
    renderSync()
    await screen.findByTestId('caldav-url')

    await user.click(screen.getByRole('button', { name: 'Copiar URL CalDAV' }))

    expect(clipboardWriteText).toHaveBeenCalledWith(`${window.location.origin}/dav/calendars/alice/`)
  })

  it('lista los tokens existentes en la tabla', async () => {
    vi.mocked(listAppTokens).mockResolvedValue([
      makeToken({ id: 't1', name: 'Móvil', last_used_at: null }),
      makeToken({ id: 't2', name: 'Portátil', last_used_at: '2026-02-03T09:30:00Z' }),
    ])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    renderSync()

    expect(await screen.findByText('Móvil')).toBeInTheDocument()
    expect(screen.getByText('Portátil')).toBeInTheDocument()
    expect(listAppTokens).toHaveBeenCalledTimes(1)
  })

  it('muestra el estado vacío cuando no hay tokens', async () => {
    vi.mocked(listAppTokens).mockResolvedValue([])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    renderSync()

    expect(await screen.findByText('No hay tokens de aplicación')).toBeInTheDocument()
  })

  it('muestra un error de carga si falla la petición', async () => {
    vi.mocked(listAppTokens).mockRejectedValue(new Error('no disponible'))
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    renderSync()

    expect(await screen.findByText('No se pudieron cargar los datos')).toBeInTheDocument()
    expect(await screen.findByText('no disponible')).toBeInTheDocument()
  })

  it('crea un token y muestra el secreto una sola vez', async () => {
    vi.mocked(listAppTokens).mockResolvedValue([])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })
    const created: CreatedAppToken = {
      id: 't-nuevo',
      name: 'Móvil',
      token: 'oxidav_t-nuevo_s3cret',
      created_at: '2026-03-01T12:00:00Z',
    }
    vi.mocked(createAppToken).mockResolvedValue(created)

    const user = setupUser()
    renderSync()
    await screen.findByRole('heading', { name: /sincronización/i, level: 2 })

    await user.click(screen.getByRole('button', { name: 'Crear token' }))
    const createDialog = await screen.findByRole('dialog')
    await user.type(within(createDialog).getByRole('textbox', { name: 'Nombre' }), 'Móvil')
    await user.click(within(createDialog).getByRole('button', { name: 'Crear' }))

    expect(createAppToken).toHaveBeenCalledTimes(1)
    expect(createAppToken).toHaveBeenCalledWith('Móvil')

    // El token en claro se muestra en el diálogo de revelado único.
    const tokenInput = await screen.findByLabelText('Token de aplicación')
    expect(tokenInput).toHaveValue(created.token)
    expect(screen.getByText('Este token no volverá a mostrarse')).toBeInTheDocument()

    // Se puede copiar el token.
    await user.click(screen.getByRole('button', { name: 'Copiar token' }))
    expect(clipboardWriteText).toHaveBeenCalledWith(created.token)

    // Al cerrar, el secreto desaparece y no vuelve a mostrarse.
    await user.click(screen.getByRole('button', { name: 'Cerrar' }))
    await waitFor(() =>
      expect(screen.queryByDisplayValue(created.token)).not.toBeInTheDocument(),
    )
    expect(screen.queryByText(created.token)).not.toBeInTheDocument()
    expect(messageSuccess).toHaveBeenCalledWith('Token creado')
  })

  it('revoca un token tras confirmar en el Popconfirm y refresca la tabla', async () => {
    vi.mocked(listAppTokens).mockResolvedValue([makeToken({ id: 't1', name: 'Móvil' })])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })
    vi.mocked(revokeAppToken).mockResolvedValue(undefined)

    const user = setupUser()
    renderSync()
    await screen.findByText('Móvil')

    await user.click(screen.getByRole('button', { name: 'Revocar Móvil' }))
    await user.click(await screen.findByRole('button', { name: 'Revocar' }))

    expect(revokeAppToken).toHaveBeenCalledWith('t1')
    await waitFor(() => expect(screen.queryByText('Móvil')).not.toBeInTheDocument())
    expect(messageSuccess).toHaveBeenCalledWith('Token revocado')
  })
})
