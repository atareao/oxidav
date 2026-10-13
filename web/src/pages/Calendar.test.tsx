/**
 * Tests de la vista Calendario (F11).
 *
 * Los servicios (`services/collections`, `services/events`) se mockean para
 * controlar la resolución de la API y verificar el comportamiento optimista:
 * la UI refleja el cambio al instante y lo revierte cuando la API falla.
 *
 * La vista consume el `message` *del contexto* vía `App.useApp()` (no el
 * export estático) para heredar el tema activo. Por eso el componente se
 * monta dentro de `<App>` y se espía la instancia de mensaje del contexto, de
 * modo que las aserciones observan las llamadas reales.
 */

import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { App } from 'antd'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { listCalendarCollections } from '../services/collections'
import { createEvent, deleteEvent, listEvents } from '../services/events'
import type { CalendarEvent, Collection } from '../types/api'
import { Calendar } from './Calendar'

vi.mock('../services/collections', () => ({
  listCalendarCollections: vi.fn(),
}))

vi.mock('../services/events', () => ({
  listEvents: vi.fn(),
  createEvent: vi.fn(),
  updateEvent: vi.fn(),
  deleteEvent: vi.fn(),
}))

const collection: Collection = {
  id: 'cal-1',
  name: 'Personal',
  type: 'calendar',
  color: null,
  sync_token: 1,
  created_at: 0,
  updated_at: 0,
}

/** Epoch ms of `hour:00` local time on the current day (always inside the grid). */
function atToday(hour: number): number {
  const now = new Date()
  return new Date(now.getFullYear(), now.getMonth(), now.getDate(), hour, 0, 0, 0).getTime()
}

function makeEvent(overrides: Partial<CalendarEvent> = {}): CalendarEvent {
  const dtstart = atToday(10)
  return {
    id: 'e1',
    uid: 'uid-1',
    collection_id: collection.id,
    summary: 'Evento',
    description: null,
    dtstart,
    dtend: dtstart + 3_600_000,
    rrule: null,
    etag: 'etag-1',
    created_at: 0,
    updated_at: 0,
    ...overrides,
  }
}

/**
 * Espías sobre la API de mensajes que `App.useApp()` publica en su contexto:
 * la misma instancia que consume la vista (en lugar del export estático
 * `message`). Mockear el hook mantiene ese contrato bajo test sin ensuciar el
 * DOM con toasts reales.
 */
const messageSuccess = vi.fn()
const messageError = vi.fn()

function renderCalendar() {
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
      <Calendar />
    </App>,
  )
}

describe('Calendar', () => {
  beforeEach(() => {
    vi.resetAllMocks()
  })

  it('muestra los eventos cargados al montar', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([makeEvent({ summary: 'Reunión de equipo' })])

    renderCalendar()

    expect(await screen.findByText(/Reunión de equipo/)).toBeInTheDocument()
    expect(listCalendarCollections).toHaveBeenCalledTimes(1)
    expect(listEvents).toHaveBeenCalledWith(collection.id, expect.any(Object), expect.anything())
  })

  it('crea un evento con estado optimista y lo consolida al resolver la API', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([])
    let resolveCreate!: (value: CalendarEvent) => void
    vi.mocked(createEvent).mockReturnValue(
      new Promise<CalendarEvent>((resolve) => {
        resolveCreate = resolve
      }),
    )

    const user = userEvent.setup()
    renderCalendar()
    await screen.findByRole('heading', { name: /calendario/i, level: 2 })

    await user.click(await screen.findByRole('button', { name: /nuevo evento/i }))
    const dialog = await screen.findByRole('dialog')
    await user.type(
      within(dialog).getByRole('textbox', { name: 'Título' }),
      'Sesión de planificación',
    )
    await user.click(within(dialog).getByRole('button', { name: 'Crear' }))

    // Optimista: visible antes de que la API responda.
    expect(await screen.findByText(/Sesión de planificación/)).toBeInTheDocument()
    expect(createEvent).toHaveBeenCalledTimes(1)
    expect(screen.getByRole('dialog')).toBeInTheDocument()

    await act(async () => {
      resolveCreate(makeEvent({ uid: 'nuevo-uid', summary: 'Sesión de planificación' }))
    })

    // La API resolvió: el evento se consolida con la respuesta del servidor y
    // se confirma con el mensaje del contexto de <App>.
    await waitFor(() => expect(messageSuccess).toHaveBeenCalledWith('Evento creado'))
    expect(screen.getByText(/Sesión de planificación/)).toBeInTheDocument()
  })

  it('revierte el evento optimista y muestra el error si la API falla al crear', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([])
    let rejectCreate!: (reason: unknown) => void
    vi.mocked(createEvent).mockReturnValue(
      new Promise<CalendarEvent>((_resolve, reject) => {
        rejectCreate = reject
      }),
    )

    const user = userEvent.setup()
    renderCalendar()
    await screen.findByRole('heading', { name: /calendario/i, level: 2 })

    await user.click(await screen.findByRole('button', { name: /nuevo evento/i }))
    const dialog = await screen.findByRole('dialog')
    await user.type(within(dialog).getByRole('textbox', { name: 'Título' }), 'Evento fallido')
    await user.click(within(dialog).getByRole('button', { name: 'Crear' }))

    // El evento optimista aparece de inmediato…
    expect(await screen.findByText(/Evento fallido/)).toBeInTheDocument()

    await act(async () => {
      rejectCreate(new Error('boom'))
    })

    // …y se revierte al fallar la API, mostrando el error en el Alert inline
    // (superficie de error única: sin toast duplicado).
    await waitFor(() => expect(screen.queryByText(/Evento fallido/)).not.toBeInTheDocument())
    expect(await screen.findByText('boom')).toBeInTheDocument()
    expect(messageError).not.toHaveBeenCalled()
    expect(messageSuccess).not.toHaveBeenCalled()
  })

  it('elimina un evento tras confirmar', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([makeEvent({ summary: 'Reunión' })])
    vi.mocked(deleteEvent).mockResolvedValue(undefined)

    const user = userEvent.setup()
    renderCalendar()
    await screen.findByText(/Reunión/)

    await user.click(screen.getByRole('button', { name: /eliminar reunión/i }))
    await user.click(await screen.findByRole('button', { name: 'Eliminar' }))

    await waitFor(() => expect(screen.queryByText(/Reunión/)).not.toBeInTheDocument())
    expect(deleteEvent).toHaveBeenCalledWith(collection.id, 'uid-1')
  })
})
