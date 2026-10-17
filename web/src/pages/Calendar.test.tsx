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
import { App, ConfigProvider } from 'antd'
import esES from 'antd/locale/es_ES'
import dayjs from 'dayjs'
import customParseFormat from 'dayjs/plugin/customParseFormat'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { listCalendarCollections } from '../services/collections'
import { createEvent, deleteEvent, getEvent, listEvents, updateEvent } from '../services/events'
import type { CalendarEvent, Collection, EventDetail } from '../types/api'
import { Calendar } from './Calendar'

dayjs.extend(customParseFormat)

vi.mock('../services/collections', () => ({
  listCalendarCollections: vi.fn(),
}))

vi.mock('../services/events', () => ({
  listEvents: vi.fn(),
  createEvent: vi.fn(),
  updateEvent: vi.fn(),
  deleteEvent: vi.fn(),
  getEvent: vi.fn(),
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
    location: null,
    url: null,
    dtstart,
    dtend: dtstart + 3_600_000,
    dtstart_tzid: null,
    dtend_tzid: null,
    rrule: null,
    etag: 'etag-1',
    created_at: 0,
    updated_at: 0,
    ...overrides,
  }
}

/**
 * Detalle completo mínimo (proyección F25) para mockear `getEvent`. Cada test
 * lo completa con `overrides`; las seis propiedades F25 van a "vacío" para que
 * el diálogo no envíe nada que el usuario no haya tocado (regla D2).
 */
function makeDetail(overrides: Partial<EventDetail> = {}): EventDetail {
  return {
    ...makeEvent(),
    categories: [],
    attach: [],
    status: null,
    class: null,
    transp: null,
    duration: null,
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
    <ConfigProvider locale={esES}>
      <App>
        <Calendar />
      </App>
    </ConfigProvider>,
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
    await screen.findByRole('heading', { name: /calendario/i, level: 1 })

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
    await screen.findByRole('heading', { name: /calendario/i, level: 1 })

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

  it('envía el tzid del navegador y mantiene dtstart/dtend en ms al crear (F18)', async () => {
    // No hardcoded zone: compare against whatever the host reports, so the
    // suite is green in any TZ (CI is usually UTC).
    const expectedTz = Intl.DateTimeFormat().resolvedOptions().timeZone

    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([])
    vi.mocked(createEvent).mockResolvedValue(
      makeEvent({ uid: 'nuevo-uid', summary: 'Con zona' }),
    )

    const user = userEvent.setup()
    renderCalendar()
    await screen.findByRole('heading', { name: /calendario/i, level: 1 })

    await user.click(await screen.findByRole('button', { name: /nuevo evento/i }))
    const dialog = await screen.findByRole('dialog')

    // Read the range the form actually shows instead of freezing the clock:
    // the payload's ms must mirror the visible start (and start + 1 h). Query by
    // the input id so the lookup is independent of the (Spanish) placeholder.
    const startInput = dialog.querySelector<HTMLInputElement>('input#range')
    expect(startInput).not.toBeNull()
    const expectedStart = dayjs(startInput?.value, 'DD/MM/YYYY HH:mm:ss').valueOf()
    expect(Number.isNaN(expectedStart)).toBe(false)

    await user.type(within(dialog).getByRole('textbox', { name: 'Título' }), 'Con zona')
    await user.click(within(dialog).getByRole('button', { name: 'Crear' }))

    await waitFor(() => expect(createEvent).toHaveBeenCalledTimes(1))
    const [, input] = vi.mocked(createEvent).mock.calls[0]
    expect(input.tzid).toBe(expectedTz)
    expect(input.dtstart).toBe(expectedStart)
    expect(input.dtend).toBe(expectedStart + 3_600_000)
  })

  it('preserva el ancla al editar un evento con zona (F18)', async () => {
    const dtstart = atToday(10)
    const dtend = dtstart + 3_600_000

    const event = makeDetail({
      summary: 'Editable',
      dtstart,
      dtend,
      dtstart_tzid: 'Europe/Madrid',
      dtend_tzid: 'Europe/Madrid',
    })
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([event])
    vi.mocked(getEvent).mockResolvedValue(event)
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Editable' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Editable/ }))

    const dialog = await screen.findByRole('dialog')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, uid, input] = vi.mocked(updateEvent).mock.calls[0]
    expect(uid).toBe('uid-1')
    // The existing anchor is sent back unchanged — NOT re-derived from the
    // browser zone.
    expect(input.tzid).toBe('Europe/Madrid')
    expect(input.dtstart).toBe(dtstart)
    expect(input.dtend).toBe(dtend)
  })

  it('no inventa un tzid al editar un evento sin zona (F18)', async () => {
    const dtstart = atToday(10)
    const dtend = dtstart + 3_600_000

    const event = makeDetail({ summary: 'Sin zona', dtstart, dtend })
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([event])
    vi.mocked(getEvent).mockResolvedValue(event)
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Sin zona' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Sin zona/ }))

    const dialog = await screen.findByRole('dialog')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect('tzid' in input).toBe(false)
    expect(input.dtstart).toBe(dtstart)
    expect(input.dtend).toBe(dtend)
  })

  it('muestra la zona horaria del evento cuando la tiene (F18)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([
      makeEvent({
        summary: 'Con zona',
        dtstart_tzid: 'Europe/Madrid',
        dtend_tzid: 'Europe/Madrid',
      }),
    ])

    renderCalendar()

    expect(await screen.findByText('Europe/Madrid')).toBeInTheDocument()
  })

  it('renderiza el Alert de error con la prop `title` (sin la deprecada `message`)', async () => {
    vi.mocked(listCalendarCollections).mockRejectedValue(new Error('no disponible'))
    vi.mocked(listEvents).mockResolvedValue([])

    // Ant Design v6 deprecates `Alert.message` in favour of `title`; the legacy
    // prop logs a console warning. Any such warning fails this test.
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    renderCalendar()

    expect(await screen.findByText('No se pudieron cargar los datos')).toBeInTheDocument()
    const deprecatedUsages = consoleError.mock.calls
      .flat()
      .filter((arg) => typeof arg === 'string' && arg.includes('`message` is deprecated'))
    expect(deprecatedUsages).toEqual([])
    consoleError.mockRestore()
  })

  it('recarga los datos al pulsar «Reintentar» en el Alert de error', async () => {
    vi.mocked(listCalendarCollections)
      .mockRejectedValueOnce(new Error('no disponible'))
      .mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([])

    const user = userEvent.setup()
    renderCalendar()

    await screen.findByText('No se pudieron cargar los datos')
    await user.click(screen.getByRole('button', { name: 'Reintentar' }))

    await waitFor(() =>
      expect(screen.queryByText('No se pudieron cargar los datos')).not.toBeInTheDocument(),
    )
    expect(listCalendarCollections).toHaveBeenCalledTimes(2)
  })

  /** Event carrying every editable metadata field (F24/PLAN-028). */
  const richEvent = (): EventDetail =>
    makeDetail({
      summary: 'Con metadatos',
      description: 'Notas',
      location: 'Sala 3',
      url: 'https://ejemplo.org/x',
    })

  it('rellena descripción, ubicación y URL al abrir en edición (F24)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([richEvent()])
    vi.mocked(getEvent).mockResolvedValue(richEvent())

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Con metadatos/ }))

    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByRole('textbox', { name: 'Descripción' })).toHaveValue('Notas')
    expect(within(dialog).getByRole('textbox', { name: 'Ubicación' })).toHaveValue('Sala 3')
    expect(within(dialog).getByRole('textbox', { name: 'URL' })).toHaveValue(
      'https://ejemplo.org/x',
    )
  })

  it('vacía la ubicación y la envía como cadena vacía, sin tocar descripción ni URL (F24/O3)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([richEvent()])
    vi.mocked(getEvent).mockResolvedValue(richEvent())
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Con metadatos' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Con metadatos/ }))

    const dialog = await screen.findByRole('dialog')
    await user.clear(within(dialog).getByRole('textbox', { name: 'Ubicación' }))
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect(input.location).toBe('')
    expect('description' in input).toBe(false)
    expect('url' in input).toBe(false)
  })

  it('vacía la URL y la envía como cadena vacía, sin tocar los demás campos (F24/O3)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([richEvent()])
    vi.mocked(getEvent).mockResolvedValue(richEvent())
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Con metadatos' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Con metadatos/ }))

    const dialog = await screen.findByRole('dialog')
    await user.clear(within(dialog).getByRole('textbox', { name: 'URL' }))
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect(input.url).toBe('')
    expect('description' in input).toBe(false)
    expect('location' in input).toBe(false)
  })

  it('vacía la descripción y la envía como cadena vacía, sin tocar ubicación ni URL (F24/O3)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([richEvent()])
    vi.mocked(getEvent).mockResolvedValue(richEvent())
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Con metadatos' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Con metadatos/ }))

    const dialog = await screen.findByRole('dialog')
    await user.clear(within(dialog).getByRole('textbox', { name: 'Descripción' }))
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect(input.description).toBe('')
    expect('location' in input).toBe(false)
    expect('url' in input).toBe(false)
  })

  it('no envía description/location/url cuando solo cambia el título (D2, F24)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([richEvent()])
    vi.mocked(getEvent).mockResolvedValue(richEvent())
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Otro título' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Con metadatos/ }))

    const dialog = await screen.findByRole('dialog')
    const title = within(dialog).getByRole('textbox', { name: 'Título' })
    await user.clear(title)
    await user.type(title, 'Otro título')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect(input.summary).toBe('Otro título')
    // Fields the user never saw/edited must not be echoed back: an old DAV row
    // may hold a value the column cannot show, and sending it would wipe it.
    expect('description' in input).toBe(false)
    expect('location' in input).toBe(false)
    expect('url' in input).toBe(false)
  })

  it('no permite guardar con una URL que no sea http(s) (F24)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([makeEvent({ summary: 'Sin URL' })])
    vi.mocked(getEvent).mockResolvedValue(makeDetail({ summary: 'Sin URL' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Sin URL/ }))

    const dialog = await screen.findByRole('dialog')
    await user.type(
      within(dialog).getByRole('textbox', { name: 'URL' }),
      'javascript:alert(1)',
    )
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    expect(
      await screen.findByText('La URL debe empezar por http:// o https://'),
    ).toBeInTheDocument()
    expect(updateEvent).not.toHaveBeenCalled()
  })

  it('pinta la ubicación y la URL como enlace seguro en la tarjeta (F24)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([
      makeEvent({ summary: 'Con metadatos', location: 'Sala 3', url: 'https://ejemplo.org/x' }),
    ])

    renderCalendar()

    expect(await screen.findByText('Sala 3')).toBeInTheDocument()
    const link = await screen.findByRole('link', { name: 'https://ejemplo.org/x' })
    expect(link).toHaveAttribute('href', 'https://ejemplo.org/x')
    expect(link).toHaveAttribute('target', '_blank')
    expect(link).toHaveAttribute('rel', 'noopener noreferrer')
  })

  it('permite renombrar un evento con URL heredada no-http sin perderla (F24)', async () => {
    // A native DAV client stored a `URL:mailto:…` the backend deliberately lets
    // through. Changing only the title MUST still be saveable, and the `PATCH`
    // must not echo the URL back (which would destroy it).
    const imported = makeDetail({
      summary: 'Importado',
      location: 'Sala 3',
      url: 'mailto:ana@example.org',
    })
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([imported])
    vi.mocked(getEvent).mockResolvedValue(imported)
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Renombrado' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Importado/ }))

    const dialog = await screen.findByRole('dialog')
    const title = within(dialog).getByRole('textbox', { name: 'Título' })
    await user.clear(title)
    await user.type(title, 'Renombrado')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect(input.summary).toBe('Renombrado')
    expect('url' in input).toBe(false)
    // No validation error was surfaced: the inherited value was accepted as-is.
    expect(screen.queryByText(/La URL/)).not.toBeInTheDocument()
  })

  it('recorta una URL con espacio final antes de enviarla, sin provocar un 400 (F24)', async () => {
    // The server rejects whitespace/control characters in `url`; the front trims
    // (and validates) the value first, so a pasted trailing space is stored
    // without it instead of round-tripping to a `400`.
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([makeEvent({ summary: 'Sin URL' })])
    vi.mocked(getEvent).mockResolvedValue(makeDetail({ summary: 'Sin URL' }))
    vi.mocked(updateEvent).mockResolvedValue(
      makeEvent({ summary: 'Sin URL', url: 'https://ejemplo.org/x' }),
    )

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Sin URL/ }))

    const dialog = await screen.findByRole('dialog')
    await user.type(
      within(dialog).getByRole('textbox', { name: 'URL' }),
      'https://ejemplo.org/x ',
    )
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect(input.url).toBe('https://ejemplo.org/x')
  })

  it('no arrastra la ubicación del evento anterior al editar dos seguidos (D2, F24)', async () => {
    const a = makeEvent({ uid: 'uid-a', summary: 'Evento A', location: 'A' })
    const b = makeEvent({ uid: 'uid-b', summary: 'Evento B', location: 'B' })
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([a, b])
    // The edit dialog fetches the *full* detail per event: answer by uid so each
    // opening gets its own row (never the previous event's).
    vi.mocked(getEvent).mockImplementation((_collectionId, uid) =>
      Promise.resolve(makeDetail(uid === 'uid-a' ? a : b)),
    )
    vi.mocked(updateEvent).mockResolvedValue(b)

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Evento A/ }))

    let dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByRole('textbox', { name: 'Ubicación' })).toHaveValue('A')
    await user.click(within(dialog).getByRole('button', { name: 'Cancelar' }))

    await user.click(await screen.findByRole('button', { name: /· Evento B/ }))
    dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByRole('textbox', { name: 'Ubicación' })).toHaveValue('B')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, uid, input] = vi.mocked(updateEvent).mock.calls[0]
    expect(uid).toBe('uid-b')
    // Neither A's nor B's location was sent, since the user never touched it.
    expect('location' in input).toBe(false)
  })

  it('no pinta enlaces para URLs no http(s) (F24)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([
      makeEvent({ uid: 'uid-js', summary: 'JS', url: 'javascript:alert(1)' }),
      makeEvent({ uid: 'uid-data', summary: 'Data', url: 'data:text/html,x' }),
    ])

    renderCalendar()

    expect(await screen.findByText(/· JS/)).toBeInTheDocument()
    expect(await screen.findByText(/· Data/)).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'javascript:alert(1)' })).not.toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'data:text/html,x' })).not.toBeInTheDocument()
  })

  it('mantiene la URL visible tras guardar aunque se vacíe otro campo (F24)', async () => {
    const event = richEvent()
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([event])
    vi.mocked(getEvent).mockResolvedValue(makeDetail({ ...event }))
    // Realistic API reply: the location was cleared, but the untouched URL stays.
    vi.mocked(updateEvent).mockResolvedValue(
      makeEvent({ summary: 'Con metadatos', description: 'Notas', location: null, url: event.url }),
    )

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Con metadatos/ }))

    const dialog = await screen.findByRole('dialog')
    await user.clear(within(dialog).getByRole('textbox', { name: 'Ubicación' }))
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    expect(
      await screen.findByRole('link', { name: 'https://ejemplo.org/x' }),
    ).toBeInTheDocument()
  })

  it('carga el detalle al abrir en edición y no reenvía las seis si solo cambia el título (F12)', async () => {
    const detailed = makeDetail({
      summary: 'Completo',
      categories: ['trabajo', 'personal'],
      attach: ['https://ejemplo.org/a'],
      status: 'CONFIRMED',
      class: 'PUBLIC',
      transp: 'OPAQUE',
      duration: 'PT2H',
    })
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    // The list row is the light one: it lacks the six F25 properties.
    vi.mocked(listEvents).mockResolvedValue([makeEvent({ summary: 'Completo' })])
    vi.mocked(getEvent).mockResolvedValue(detailed)
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Otro título' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Completo/ }))

    // El diálogo pide el detalle completo con la colección y el uid correctos.
    await waitFor(() =>
      expect(getEvent).toHaveBeenCalledWith(collection.id, 'uid-1', expect.anything()),
    )

    const dialog = await screen.findByRole('dialog')
    // Las seis propiedades se reflejan en el formulario.
    expect(within(dialog).getByText('trabajo')).toBeInTheDocument()
    expect(within(dialog).getByText('personal')).toBeInTheDocument()
    expect(within(dialog).getByLabelText('Adjunto 1')).toHaveValue('https://ejemplo.org/a')
    expect(within(dialog).getByText('Confirmado')).toBeInTheDocument()
    expect(within(dialog).getByText('Público')).toBeInTheDocument()
    expect(within(dialog).getByText('Ocupado (opaco)')).toBeInTheDocument()
    expect(within(dialog).getByLabelText('Dura…')).toBeChecked()
    expect(within(dialog).getByLabelText('Cantidad')).toHaveValue('2')

    const title = within(dialog).getByRole('textbox', { name: 'Título' })
    await user.clear(title)
    await user.type(title, 'Otro título')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect(input.summary).toBe('Otro título')
    // Ninguna de las seis viaja: el usuario no las tocó (regla D2).
    expect('categories' in input).toBe(false)
    expect('attach' in input).toBe(false)
    expect('status' in input).toBe(false)
    expect('class' in input).toBe(false)
    expect('transp' in input).toBe(false)
    expect('duration' in input).toBe(false)
    expect('dtend' in input).toBe(false)
  })

  it('envía DURATION y omite DTEND cuando el radio está en «Dura…» (F13)', async () => {
    const dtstart = atToday(10)
    // `PT2H` decomposes into 2 hours; changing it to 1 hour is an explicit
    // change, so the composed `PT1H` travels instead of the untouched `DTEND`.
    const event = makeDetail({
      summary: 'Duración',
      dtstart,
      dtend: null,
      duration: 'PT2H',
    })
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([makeEvent({ summary: 'Duración', dtstart })])
    vi.mocked(getEvent).mockResolvedValue(event)
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Duración' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Duración/ }))

    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByLabelText('Dura…')).toBeChecked()
    const value = within(dialog).getByLabelText('Cantidad')
    await user.clear(value)
    await user.type(value, '1')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect(input.duration).toBe('PT1H')
    // DURATION and DTEND are exclusive: the latter is never sent alongside.
    expect('dtend' in input).toBe(false)
  })

  it('envía DTEND y omite DURATION cuando el radio está en «Termina a las…» (F13)', async () => {
    const dtstart = atToday(10)
    const event = makeDetail({
      summary: 'Termina',
      dtstart,
      dtend: null,
      duration: 'PT1H',
    })
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([makeEvent({ summary: 'Termina', dtstart })])
    vi.mocked(getEvent).mockResolvedValue(event)
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Termina' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Termina/ }))

    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByLabelText('Dura…')).toBeChecked()
    await user.click(within(dialog).getByLabelText('Termina a las…'))
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    // The one-hour duration became an explicit end (start + 1 h).
    expect(input.dtend).toBe(dtstart + 3_600_000)
    expect('duration' in input).toBe(false)
  })

  it('pinta categorías, adjuntos seguros y un fin calculado con solo DURATION (F14)', async () => {
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([
      makeDetail({
        summary: 'Con todo',
        dtstart: atToday(10),
        dtend: null,
        duration: 'PT90M',
        categories: ['trabajo'],
        attach: ['https://ejemplo.org/a', 'javascript:alert(1)'],
      }),
    ])

    renderCalendar()

    expect(await screen.findByText('trabajo')).toBeInTheDocument()
    const link = await screen.findByRole('link', { name: 'https://ejemplo.org/a' })
    expect(link).toHaveAttribute('href', 'https://ejemplo.org/a')
    expect(link).toHaveAttribute('target', '_blank')
    expect(link).toHaveAttribute('rel', 'noopener noreferrer')
    // No `DTEND`: the card derives 10:00 + 90 min = 11:30 from the `DURATION`.
    expect(screen.getByText(/hasta 11:30/)).toBeInTheDocument()
    // A non-http(s) `ATTACH` is never painted as a link.
    expect(screen.queryByRole('link', { name: 'javascript:alert(1)' })).not.toBeInTheDocument()
  })

  it('no reescribe una DURATION no descomponible al cambiar solo el título (D2)', async () => {
    const event = makeDetail({ summary: 'Larga', dtend: null, duration: 'P1DT2H' })
    vi.mocked(listCalendarCollections).mockResolvedValue([collection])
    vi.mocked(listEvents).mockResolvedValue([makeEvent({ summary: 'Larga' })])
    vi.mocked(getEvent).mockResolvedValue(event)
    vi.mocked(updateEvent).mockResolvedValue(makeEvent({ summary: 'Renombrada' }))

    const user = userEvent.setup()
    renderCalendar()
    await user.click(await screen.findByRole('button', { name: /· Larga/ }))

    const dialog = await screen.findByRole('dialog')
    // The non-decomposable value is shown verbatim, not silently rounded.
    expect(await within(dialog).findByText(/Duración actual: P1DT2H/)).toBeInTheDocument()

    const title = within(dialog).getByRole('textbox', { name: 'Título' })
    await user.clear(title)
    await user.type(title, 'Renombrada')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(updateEvent).toHaveBeenCalledTimes(1))
    const [, , input] = vi.mocked(updateEvent).mock.calls[0]
    expect(input.summary).toBe('Renombrada')
    // Untouched: the raw `P1DT2H` is preserved by not echoing anything back.
    expect('duration' in input).toBe(false)
    expect('dtend' in input).toBe(false)
  })
})
