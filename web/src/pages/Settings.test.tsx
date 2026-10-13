/**
 * Tests de la vista Ajustes y Perfil (F14).
 *
 * Se mockean por completo los servicios de los que depende la vista
 * (`services/collections` para el CRUD y `services/appTokens` para el perfil)
 * para controlar la resolución de la API y verificar: render del perfil,
 * conmutación de tema con persistencia, y creación/renombrado/borrado de
 * colecciones (borrado con confirmación).
 *
 * Como en F11–F13, la vista consume el `message` *del contexto* vía
 * `App.useApp()`; se espía esa instancia. El modo de tema se persiste en
 * `localStorage`, que se limpia en cada test para aislarlos.
 */

import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { App } from 'antd'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ThemeProvider } from '../contexts/ThemeContext'
import { getCurrentUser } from '../services/appTokens'
import {
  createCollection,
  deleteCollection,
  listCollections,
  updateCollection,
} from '../services/collections'
import type { Collection } from '../types/api'
import { Settings } from './Settings'

vi.mock('../services/collections', () => ({
  listCollections: vi.fn(),
  createCollection: vi.fn(),
  updateCollection: vi.fn(),
  deleteCollection: vi.fn(),
}))

vi.mock('../services/appTokens', () => ({
  getCurrentUser: vi.fn(),
}))

function makeCollection(overrides: Partial<Collection> = {}): Collection {
  return {
    id: 'c1',
    name: 'Personal',
    type: 'calendar',
    color: '#1677ff',
    sync_token: 0,
    created_at: 0,
    updated_at: 0,
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

function renderSettings() {
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

  return render(
    <ThemeProvider>
      <App>
        <Settings />
      </App>
    </ThemeProvider>,
  )
}

describe('Settings', () => {
  beforeEach(() => {
    vi.resetAllMocks()
    localStorage.clear()
  })

  afterEach(() => {
    vi.restoreAllMocks()
    localStorage.clear()
  })

  it('muestra el usuario (claim sub) desde GET /api/v1/me', async () => {
    vi.mocked(listCollections).mockResolvedValue([])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    renderSettings()

    await waitFor(() =>
      expect(screen.getByTestId('profile-user')).toHaveTextContent('alice'),
    )
  })

  it('muestra un error de carga si falla la petición', async () => {
    vi.mocked(listCollections).mockRejectedValue(new Error('no disponible'))
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    renderSettings()

    expect(await screen.findByText('No se pudieron cargar los datos')).toBeInTheDocument()
    expect(await screen.findByText('no disponible')).toBeInTheDocument()
  })

  it('cambia el tema a oscuro y lo persiste', async () => {
    vi.mocked(listCollections).mockResolvedValue([])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    const user = userEvent.setup()
    renderSettings()
    await screen.findByText('No hay colecciones')

    // The radio input itself is `pointer-events: none`; click its label text.
    await user.click(screen.getByText('Oscuro'))

    await waitFor(() => expect(localStorage.getItem('oxidav.theme')).toBe('dark'))
    expect(screen.getByRole('radio', { name: 'Oscuro' })).toBeChecked()
  })

  it('lista las colecciones existentes', async () => {
    vi.mocked(listCollections).mockResolvedValue([
      makeCollection({ id: 'c1', name: 'Personal', type: 'calendar' }),
      makeCollection({ id: 'c2', name: 'Trabajo', type: 'addressbook' }),
    ])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    renderSettings()

    expect(await screen.findByText('Personal')).toBeInTheDocument()
    expect(screen.getByText('Trabajo')).toBeInTheDocument()
    expect(screen.getByText('Calendario')).toBeInTheDocument()
    expect(screen.getByText('Libreta de direcciones')).toBeInTheDocument()
  })

  it('muestra el estado vacío cuando no hay colecciones', async () => {
    vi.mocked(listCollections).mockResolvedValue([])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })

    renderSettings()

    expect(await screen.findByText('No hay colecciones')).toBeInTheDocument()
  })

  it('crea una colección y refresca la tabla', async () => {
    vi.mocked(listCollections).mockResolvedValue([])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })
    vi.mocked(createCollection).mockResolvedValue(makeCollection({ id: 'c-new', name: 'Trabajo' }))

    const user = userEvent.setup()
    renderSettings()
    await screen.findByText('No hay colecciones')

    await user.click(screen.getByRole('button', { name: 'Crear colección' }))
    const dialog = await screen.findByRole('dialog')
    await user.type(within(dialog).getByRole('textbox', { name: 'Nombre' }), 'Trabajo')
    await user.click(within(dialog).getByRole('button', { name: 'Crear' }))

    await waitFor(() =>
      expect(createCollection).toHaveBeenCalledWith({ name: 'Trabajo', type: 'calendar' }),
    )
    await waitFor(() => expect(messageSuccess).toHaveBeenCalledWith('Colección creada'))
    expect(await screen.findByText('Trabajo')).toBeInTheDocument()
  })

  it('renombra una colección tras enviar el modal', async () => {
    vi.mocked(listCollections).mockResolvedValue([makeCollection({ id: 'c1', name: 'Personal' })])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })
    vi.mocked(updateCollection).mockResolvedValue(makeCollection({ id: 'c1', name: 'Familia' }))

    const user = userEvent.setup()
    renderSettings()
    await screen.findByText('Personal')

    await user.click(screen.getByRole('button', { name: 'Renombrar Personal' }))
    const dialog = await screen.findByRole('dialog')
    const input = within(dialog).getByRole('textbox', { name: 'Nombre' })
    await user.clear(input)
    await user.type(input, 'Familia')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateCollection).toHaveBeenCalledWith('c1', { name: 'Familia' }))
    await waitFor(() => expect(messageSuccess).toHaveBeenCalledWith('Colección renombrada'))
    expect(await screen.findByText('Familia')).toBeInTheDocument()
  })

  it('borra una colección tras confirmar en el Popconfirm y refresca la tabla', async () => {
    vi.mocked(listCollections).mockResolvedValue([makeCollection({ id: 'c1', name: 'Personal' })])
    vi.mocked(getCurrentUser).mockResolvedValue({ sub: 'alice' })
    vi.mocked(deleteCollection).mockResolvedValue(undefined)

    const user = userEvent.setup()
    renderSettings()
    await screen.findByText('Personal')

    await user.click(screen.getByRole('button', { name: 'Eliminar Personal' }))
    await user.click(await screen.findByRole('button', { name: 'Eliminar' }))

    await waitFor(() => expect(deleteCollection).toHaveBeenCalledWith('c1'))
    await waitFor(() => expect(screen.queryByText('Personal')).not.toBeInTheDocument())
    expect(messageSuccess).toHaveBeenCalledWith('Colección eliminada')
  })
})
