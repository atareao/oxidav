/**
 * Tests de la vista Contactos (F12).
 *
 * Los servicios (`services/contacts`, `services/collections`) se mockean para
 * controlar la resolución de la API y verificar el comportamiento optimista: la
 * UI refleja el cambio al instante y lo revierte cuando la API falla.
 *
 * La carga de contactos se pagina del lado del servidor (`useContacts` camina
 * `limit=200&offset=k` hasta agotar), por lo que el mock de `listContacts`
 * devuelve un array por página y expone el mismo `CONTACTS_PAGE_SIZE` que el
 * módulo real (`undefined` haría que el bucle de drenado nunca terminase).
 *
 * La vista consume el `message` *del contexto* vía `App.useApp()` (no el export
 * estático) para heredar el tema activo. Por eso el componente se monta dentro
 * de `<App>` y se espía la instancia de mensaje del contexto, de modo que las
 * aserciones observan las llamadas reales.
 */

import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { App } from 'antd'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  CONTACTS_PAGE_SIZE,
  createContact,
  deleteContact,
  getContact,
  importContacts,
  listAddressbookCollections,
  listContacts,
  updateContact,
} from '../services/contacts'
import type { Collection, Contact, ContactDetail } from '../types/api'
import { MAX_FILE_BYTES, serializeContactsToVcf } from '../utils/vcf'
import { Contacts } from './Contacts'

vi.mock('../services/collections', () => ({
  listCollections: vi.fn(),
  listCalendarCollections: vi.fn(),
}))

vi.mock('../services/contacts', () => ({
  // The real hook drains every page keyed off this constant; a missing export
  // would be `undefined` and the `page.length < PAGE_SIZE` guard would never
  // terminate the loop.
  CONTACTS_PAGE_SIZE: 200,
  listContacts: vi.fn(),
  listAddressbookCollections: vi.fn(),
  createContact: vi.fn(),
  updateContact: vi.fn(),
  deleteContact: vi.fn(),
  // F22: el detalle (para el Drawer) y la importación en servidor. Si faltan,
  // los tests existentes revientan al llamar a `undefined`.
  getContact: vi.fn(),
  importContacts: vi.fn(),
}))

const addressBook: Collection = {
  id: 'ab-1',
  name: 'Personal',
  type: 'addressbook',
  color: null,
  sync_token: 1,
  created_at: 0,
  updated_at: 0,
}

function makeContact(overrides: Partial<Contact> = {}): Contact {
  return {
    id: 'c1',
    uid: 'uid-1',
    collection_id: addressBook.id,
    fn: 'Ada Lovelace',
    email: 'ada@example.com',
    phone: '+34 600 000 000',
    organization: 'Analytical Engines Ltd',
    etag: 'etag-1',
    created_at: 0,
    updated_at: 0,
    ...overrides,
  }
}

/**
 * Detalle completo mínimo (proyección F22) para mockear `getContact`. Cada test
 * lo completa con `overrides`; los arrays van vacíos para que el Drawer no
 * pinte pestañas de más.
 */
function makeDetail(overrides: Partial<ContactDetail> = {}): ContactDetail {
  return {
    id: 'c1',
    uid: 'uid-1',
    collection_id: addressBook.id,
    fn: 'Ada Lovelace',
    full_name: 'Ada Lovelace',
    etag: 'etag-1',
    created_at: 0,
    updated_at: 0,
    version: '4.0',
    rev: null,
    name: null,
    nicknames: [],
    photos: [],
    birthday: null,
    anniversary: null,
    gender: null,
    tels: [],
    emails: [],
    impps: [],
    addresses: [],
    geos: [],
    timezones: [],
    titles: [],
    roles: [],
    organizations: [],
    logos: [],
    notes: [],
    categories: [],
    urls: [],
    sounds: [],
    others: [],
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
const messageWarning = vi.fn()
const messageInfo = vi.fn()

function renderContacts() {
  vi.spyOn(App, 'useApp').mockReturnValue({
    message: {
      success: messageSuccess,
      error: messageError,
      info: messageInfo,
      warning: messageWarning,
      loading: vi.fn(),
      open: vi.fn(),
      destroy: vi.fn(),
    },
  } as unknown as ReturnType<typeof App.useApp>)

  return render(
    <App>
      <Contacts />
    </App>,
  )
}

// The export test replaces these browser APIs to capture the generated Blob;
// restoring them after every test keeps the global `URL` from leaking mocks.
const originalCreateObjectURL = URL.createObjectURL
const originalRevokeObjectURL = URL.revokeObjectURL

/**
 * El `<Modal>` y el `<Drawer>` de Ant Design conviven en el DOM con
 * `role="dialog"` cuando el primero aún está animando su salida
 * (`ant-zoom-leave`), así que `screen.getByRole('dialog')` se vuelve ambiguo.
 * Estos helpers acotan la consulta al contenedor concreto que necesita cada
 * test en lugar de aflojarla.
 */
function drawer(): HTMLElement {
  const root = document.querySelector<HTMLElement>('.ant-drawer')
  if (root === null) {
    throw new Error('No hay ningún Drawer montado')
  }
  return root
}

/**
 * jsdom no dispara `transitionend`, por lo que la animación de salida del
 * `Modal` (`ant-zoom-leave`) nunca termina y su nodo se queda en el DOM. Este
 * helper emite el evento que un navegador real enviaría para que el `Modal` se
 * desmonte de verdad (igual que un `Drawer`, que sí trae `motionDeadline`).
 */
function finishModalLeave(): void {
  const el = document.querySelector<HTMLElement>('.ant-modal')
  if (el !== null) {
    fireEvent.transitionEnd(el)
  }
}

describe('Contacts', () => {
  beforeEach(() => {
    vi.resetAllMocks()
  })

  afterEach(() => {
    URL.createObjectURL = originalCreateObjectURL
    URL.revokeObjectURL = originalRevokeObjectURL
  })

  it('muestra los contactos cargados al montar', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Alan Turing' })])

    renderContacts()

    expect(await screen.findByText('Alan Turing')).toBeInTheDocument()
    expect(listAddressbookCollections).toHaveBeenCalledTimes(1)
    expect(listContacts).toHaveBeenCalledWith(addressBook.id, expect.any(Object), expect.anything())
  })

  it('carga todas las páginas de contactos (paginación del servidor)', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])

    // A full first page forces the hook to request a second one; the short
    // second page signals exhaustion.
    const firstPage = Array.from({ length: CONTACTS_PAGE_SIZE }, (_value, index) =>
      makeContact({ id: `c-${index}`, uid: `uid-${index}`, fn: `Contacto ${index}` }),
    )
    const lastContact = makeContact({ id: 'c-last', uid: 'uid-last', fn: 'Último Contacto' })
    vi.mocked(listContacts).mockImplementation((_collectionId, page) => {
      const offset = page?.offset ?? 0
      if (offset === 0) {
        return Promise.resolve(firstPage)
      }
      if (offset === CONTACTS_PAGE_SIZE) {
        return Promise.resolve([lastContact])
      }
      return Promise.resolve([])
    })

    const user = userEvent.setup()
    renderContacts()

    await screen.findByRole('heading', { name: /contactos/i, level: 1 })
    await waitFor(() => expect(listContacts).toHaveBeenCalledTimes(2))
    expect(listContacts).toHaveBeenNthCalledWith(
      1,
      addressBook.id,
      { limit: CONTACTS_PAGE_SIZE, offset: 0 },
      expect.anything(),
    )
    expect(listContacts).toHaveBeenNthCalledWith(
      2,
      addressBook.id,
      { limit: CONTACTS_PAGE_SIZE, offset: CONTACTS_PAGE_SIZE },
      expect.anything(),
    )

    // El contacto de la segunda página sí se cargó (se filtra para verlo).
    await user.type(screen.getByRole('searchbox', { name: 'Buscar contactos' }), 'Último')
    expect(await screen.findByText('Último Contacto')).toBeInTheDocument()
  })

  it('muestra el estado vacío cuando la libreta no tiene contactos', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([])

    renderContacts()

    expect(await screen.findByText('No hay contactos')).toBeInTheDocument()
  })

  it('muestra un error de carga si falla la petición de contactos', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockRejectedValue(new Error('no disponible'))

    renderContacts()

    expect(await screen.findByText('No se pudieron cargar los datos')).toBeInTheDocument()
    expect(await screen.findByText('no disponible')).toBeInTheDocument()
    expect(messageError).not.toHaveBeenCalled()
  })

  it('abre el detalle con el teclado al activar el botón Ver', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    vi.mocked(getContact).mockResolvedValue(makeDetail({ uid: 'uid-1' }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    const verButton = screen.getByRole('button', { name: 'Ver Ada Lovelace' })
    verButton.focus()
    expect(verButton).toHaveFocus()

    await user.keyboard('{Enter}')

    const drawer = await screen.findByRole('dialog')
    expect(getContact).toHaveBeenCalledWith(addressBook.id, 'uid-1', expect.anything())
    // El UID aparece primero en la pestaña «Control», ya con el detalle cargado.
    expect(await within(drawer).findByText('uid-1')).toBeInTheDocument()
  })

  it('muestra un esqueleto de carga y luego las pestañas del contacto', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    let resolveDetail!: (value: ContactDetail) => void
    vi.mocked(getContact).mockReturnValue(
      new Promise<ContactDetail>((resolve) => {
        resolveDetail = resolve
      }),
    )

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Ver Ada Lovelace' }))

    // Carga: el Drawer está abierto pero aún no hay pestañas. El Drawer se
    // monta en un portal (document.body), así que el esqueleto se busca dentro.
    const drawer = await screen.findByRole('dialog')
    expect(getContact).toHaveBeenCalledWith(addressBook.id, 'uid-1', expect.anything())
    await waitFor(() => expect(drawer.querySelector('.ant-skeleton')).not.toBeNull())
    expect(within(drawer).queryByRole('tab')).not.toBeInTheDocument()

    await act(async () => {
      resolveDetail(makeDetail({ uid: 'uid-1', fn: 'Ada Lovelace' }))
    })

    // Contenido: la pestaña «Control» (activa por defecto) y el UID completo.
    expect(await within(drawer).findByRole('tab', { name: 'Control' })).toBeInTheDocument()
    expect(await within(drawer).findByText('uid-1')).toBeInTheDocument()
  })

  it('muestra el error del detalle y lo recarga con «Reintentar»', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    vi.mocked(getContact)
      .mockRejectedValueOnce(new Error('detalle no disponible'))
      .mockResolvedValue(makeDetail({ uid: 'uid-1', fn: 'Ada Lovelace' }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Ver Ada Lovelace' }))

    const drawer = await screen.findByRole('dialog')
    expect(
      await within(drawer).findByText('No se pudo cargar el detalle del contacto'),
    ).toBeInTheDocument()
    expect(within(drawer).getByText('detalle no disponible')).toBeInTheDocument()

    await user.click(within(drawer).getByRole('button', { name: 'Reintentar' }))

    expect(await within(drawer).findByText('uid-1')).toBeInTheDocument()
    expect(getContact).toHaveBeenCalledTimes(2)
  })

  it('crea un contacto con estado optimista y lo consolida al resolver la API', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([])
    let resolveCreate!: (value: Contact) => void
    vi.mocked(createContact).mockReturnValue(
      new Promise<Contact>((resolve) => {
        resolveCreate = resolve
      }),
    )

    const user = userEvent.setup()
    renderContacts()
    await screen.findByRole('heading', { name: /contactos/i, level: 1 })

    await user.click(await screen.findByRole('button', { name: /nuevo contacto/i }))
    const dialog = await screen.findByRole('dialog')
    await user.type(within(dialog).getByRole('textbox', { name: 'Nombre' }), 'Grace Hopper')
    await user.click(within(dialog).getByRole('button', { name: 'Crear' }))

    // Optimista: visible antes de que la API responda.
    expect(await screen.findByText('Grace Hopper')).toBeInTheDocument()
    expect(createContact).toHaveBeenCalledTimes(1)
    expect(createContact).toHaveBeenCalledWith(
      addressBook.id,
      expect.objectContaining({ fn: 'Grace Hopper' }),
    )
    expect(screen.getByRole('dialog')).toBeInTheDocument()

    await act(async () => {
      resolveCreate(makeContact({ uid: 'nuevo-uid', fn: 'Grace Hopper' }))
    })

    // La API resolvió: el contacto se consolida con la respuesta del servidor.
    await waitFor(() => expect(messageSuccess).toHaveBeenCalledWith('Contacto creado'))
    expect(screen.getByText('Grace Hopper')).toBeInTheDocument()
  })

  it('revierte el contacto optimista y muestra el error si la API falla al crear', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([])
    let rejectCreate!: (reason: unknown) => void
    vi.mocked(createContact).mockReturnValue(
      new Promise<Contact>((_resolve, reject) => {
        rejectCreate = reject
      }),
    )

    const user = userEvent.setup()
    renderContacts()
    await screen.findByRole('heading', { name: /contactos/i, level: 1 })

    await user.click(await screen.findByRole('button', { name: /nuevo contacto/i }))
    const dialog = await screen.findByRole('dialog')
    await user.type(within(dialog).getByRole('textbox', { name: 'Nombre' }), 'Contacto fallido')
    await user.click(within(dialog).getByRole('button', { name: 'Crear' }))

    // El contacto optimista aparece de inmediato…
    expect(await screen.findByText('Contacto fallido')).toBeInTheDocument()

    await act(async () => {
      rejectCreate(new Error('boom'))
    })

    // …y se revierte al fallar la API, mostrando el error con `message`.
    await waitFor(() => expect(screen.queryByText('Contacto fallido')).not.toBeInTheDocument())
    expect(messageError).toHaveBeenCalledWith('boom')
    expect(messageSuccess).not.toHaveBeenCalled()
  })

  it('revierte la edición optimista si la API falla al actualizar', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    // Editar ahora carga el detalle completo para poblar el formulario.
    vi.mocked(getContact).mockResolvedValue(makeDetail({ uid: 'uid-1', fn: 'Ada Lovelace' }))
    let rejectUpdate!: (reason: unknown) => void
    vi.mocked(updateContact).mockReturnValue(
      new Promise<Contact>((_resolve, reject) => {
        rejectUpdate = reject
      }),
    )

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Editar Ada Lovelace' }))
    const dialog = await screen.findByRole('dialog')
    const nameInput = await within(dialog).findByRole('textbox', { name: 'Nombre' })
    await user.clear(nameInput)
    await user.type(nameInput, 'Ada Byron')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    // Optimista: el nuevo nombre aparece de inmediato…
    expect(await screen.findByText('Ada Byron')).toBeInTheDocument()
    expect(updateContact).toHaveBeenCalledWith(
      addressBook.id,
      'uid-1',
      expect.objectContaining({ fn: 'Ada Byron' }),
    )

    await act(async () => {
      rejectUpdate(new Error('boom'))
    })

    // …y se revierte al fallar la API.
    await waitFor(() => expect(screen.queryByText('Ada Byron')).not.toBeInTheDocument())
    expect(screen.getByText('Ada Lovelace')).toBeInTheDocument()
    expect(messageError).toHaveBeenCalledWith('boom')
    expect(messageSuccess).not.toHaveBeenCalled()
  })

  it('revierte el borrado optimista si la API falla al eliminar', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    let rejectDelete!: (reason: unknown) => void
    vi.mocked(deleteContact).mockReturnValue(
      new Promise<void>((_resolve, reject) => {
        rejectDelete = reject
      }),
    )

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Eliminar Ada Lovelace' }))
    await user.click(await screen.findByRole('button', { name: 'Eliminar' }))

    // Optimista: desaparece de inmediato…
    await waitFor(() => expect(screen.queryByText('Ada Lovelace')).not.toBeInTheDocument())

    await act(async () => {
      rejectDelete(new Error('boom'))
    })

    // …y reaparece al fallar la API.
    expect(await screen.findByText('Ada Lovelace')).toBeInTheDocument()
    expect(deleteContact).toHaveBeenCalledWith(addressBook.id, 'uid-1')
    expect(messageError).toHaveBeenCalledWith('boom')
  })

  it('exporta el listado a un .vcf no vacío', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])

    const createObjectURL = vi.fn<(blob: Blob) => string>(() => 'blob:mock')
    URL.createObjectURL = createObjectURL
    URL.revokeObjectURL = vi.fn()

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: /exportar/i }))

    expect(createObjectURL).toHaveBeenCalledTimes(1)
    const blob = createObjectURL.mock.calls[0]?.[0] as Blob
    expect(blob.size).toBeGreaterThan(0)
    const text = await blob.text()
    expect(text).toContain('BEGIN:VCARD')
    expect(text).toContain('FN:Ada Lovelace')
    expect(messageSuccess).toHaveBeenCalledWith('Contactos exportados')
  })

  it('importa un fichero .vcf enviando el texto en crudo al servidor (sin parseo en cliente)', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([])
    vi.mocked(importContacts).mockResolvedValue([
      makeContact({ uid: 'imported-1', fn: 'Imported Person' }),
    ])

    renderContacts()
    await screen.findByRole('heading', { name: /contactos/i, level: 1 })

    // Texto de una vCard que el parser cliente antiguo no habría conservado
    // (PHOTO embebida y `X-ABLabel`): el servidor lo recibe byte a byte.
    const vcf = [
      'BEGIN:VCARD',
      'VERSION:4.0',
      'UID:vcf-1',
      'FN:Imported Person',
      'EMAIL:imported@example.com',
      'PHOTO:data:image/png;base64,AAAA',
      'X-ABLabel:Amigo',
      'END:VCARD',
      '',
    ].join('\r\n')
    const file = new File([vcf], 'contactos.vcf', { type: 'text/vcard' })

    const input = screen.getByLabelText('Importar archivo .vcf')
    fireEvent.change(input, { target: { files: [file] } })

    await waitFor(() => expect(importContacts).toHaveBeenCalledWith(addressBook.id, vcf))
    // El SPA ya no crea un contacto por ficha: lo hace el servidor de una vez.
    expect(createContact).not.toHaveBeenCalled()
    await waitFor(() => expect(messageSuccess).toHaveBeenCalledWith('1 contacto(s) importado(s)'))
  })

  it('muestra el mensaje del sobre de error si falla la importación en servidor', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([])
    vi.mocked(importContacts).mockRejectedValue(new Error('una tarjeta no es válida'))

    renderContacts()
    await screen.findByRole('heading', { name: /contactos/i, level: 1 })

    const vcf = 'BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Rota\r\nEND:VCARD\r\n'
    const file = new File([vcf], 'contactos.vcf', { type: 'text/vcard' })

    fireEvent.change(screen.getByLabelText('Importar archivo .vcf'), {
      target: { files: [file] },
    })

    await waitFor(() => expect(messageError).toHaveBeenCalledWith('una tarjeta no es válida'))
    expect(messageSuccess).not.toHaveBeenCalled()
  })

  it('explica en un tooltip que el .vcf exportado solo incluye los campos básicos', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.hover(screen.getByRole('button', { name: /exportar/i }))

    expect(
      await screen.findByText(
        'El .vcf exportado incluye solo los campos básicos; el fichero completo vive en el servidor',
      ),
    ).toBeInTheDocument()
  })

  it('muestra el nombre de la libreta (no el id) en la pestaña «Servidor»', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    vi.mocked(getContact).mockResolvedValue(makeDetail({ uid: 'uid-1', collection_id: addressBook.id }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Ver Ada Lovelace' }))
    const drawer = await screen.findByRole('dialog')
    await user.click(await within(drawer).findByRole('tab', { name: 'Servidor' }))

    expect(await within(drawer).findByText(addressBook.name)).toBeInTheDocument()
    expect(within(drawer).queryByText(addressBook.id)).not.toBeInTheDocument()
  })

  it('renderiza el Alert de error con la prop `title` (sin la deprecada `message`)', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockRejectedValue(new Error('no disponible'))

    // Ant Design v6 deprecates `Alert.message` in favour of `title`; the legacy
    // prop logs a console warning. Any such warning fails this test.
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    renderContacts()

    expect(await screen.findByText('No se pudieron cargar los datos')).toBeInTheDocument()
    const deprecatedUsages = consoleError.mock.calls
      .flat()
      .filter((arg) => typeof arg === 'string' && arg.includes('`message` is deprecated'))
    expect(deprecatedUsages).toEqual([])
    consoleError.mockRestore()
  })

  it('recarga los datos al pulsar «Reintentar» en el Alert de error', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts)
      .mockRejectedValueOnce(new Error('no disponible'))
      .mockResolvedValue([makeContact({ fn: 'Grace Hopper' })])

    const user = userEvent.setup()
    renderContacts()

    await screen.findByText('No se pudieron cargar los datos')
    await user.click(screen.getByRole('button', { name: 'Reintentar' }))

    await waitFor(() =>
      expect(screen.queryByText('No se pudieron cargar los datos')).not.toBeInTheDocument(),
    )
    expect(await screen.findByText('Grace Hopper')).toBeInTheDocument()
  })

  it('solo envía los grupos modificados al editar (preserva el resto del payload)', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    // Detalle estilo Apple: varios TEL/EMAIL con TYPE/PREF, notas y propiedades
    // sin modelar conviviendo. Cambiar solo el nombre no debe tocar nada de eso.
    vi.mocked(getContact).mockResolvedValue(
      makeDetail({
        uid: 'uid-1',
        fn: 'Ada Lovelace',
        tels: [
          { value: '+34600000000', types: ['cell'], pref: 1 },
          { value: '+34910000000', types: ['work', 'voice'], pref: null },
        ],
        emails: [{ value: 'ada@example.com', types: ['home'], pref: null }],
        notes: ['Nota que no se toca'],
        others: [{ name: 'X-ABLabel', group: 'item1', parameters: [], value: 'Amigo' }],
      }),
    )
    vi.mocked(updateContact).mockResolvedValue(makeContact({ fn: 'Ada Byron' }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Editar Ada Lovelace' }))
    const dialog = await screen.findByRole('dialog')
    const nameInput = await within(dialog).findByRole('textbox', { name: 'Nombre' })
    await user.clear(nameInput)
    await user.type(nameInput, 'Ada Byron')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    // El cuerpo es **exactamente** `{ fn }`: ni `tels`, ni `emails`, ni `notes`.
    await waitFor(() =>
      expect(updateContact).toHaveBeenCalledWith(addressBook.id, 'uid-1', { fn: 'Ada Byron' }),
    )
    expect(updateContact).toHaveBeenCalledTimes(1)
  })

  it('envía el array completo de teléfonos al cambiar uno', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    vi.mocked(getContact).mockResolvedValue(
      makeDetail({
        uid: 'uid-1',
        fn: 'Ada Lovelace',
        tels: [
          { value: '+111', types: ['home'], pref: null },
          { value: '+222', types: ['work'], pref: 1 },
        ],
      }),
    )
    vi.mocked(updateContact).mockResolvedValue(makeContact({ fn: 'Ada Lovelace' }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Editar Ada Lovelace' }))
    const dialog = await screen.findByRole('dialog')
    await within(dialog).findByRole('textbox', { name: 'Nombre' })

    const firstValue = within(dialog).getByDisplayValue('+111')
    await user.clear(firstValue)
    await user.type(firstValue, '+999')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    // El grupo presente sustituye **todas** sus apariciones: viajan los dos TEL.
    await waitFor(() =>
      expect(updateContact).toHaveBeenCalledWith(addressBook.id, 'uid-1', {
        tels: [
          { value: '+999', types: ['home'], pref: null },
          { value: '+222', types: ['work'], pref: 1 },
        ],
      }),
    )
  })

  it('borra un grupo al vaciar todas sus filas', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    vi.mocked(getContact).mockResolvedValue(
      makeDetail({
        uid: 'uid-1',
        fn: 'Ada Lovelace',
        tels: [{ value: '+111', types: [], pref: null }],
      }),
    )
    vi.mocked(updateContact).mockResolvedValue(makeContact({ fn: 'Ada Lovelace' }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Editar Ada Lovelace' }))
    const dialog = await screen.findByRole('dialog')
    await within(dialog).findByRole('textbox', { name: 'Nombre' })

    // Quitar la única fila deja el grupo vacío → `[]`, que es como se borra.
    await user.click(within(dialog).getByRole('button', { name: 'Quitar Teléfono 1' }))
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() =>
      expect(updateContact).toHaveBeenCalledWith(addressBook.id, 'uid-1', { tels: [] }),
    )
  })

  it('no lanza petición si no hay cambios que guardar', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    vi.mocked(getContact).mockResolvedValue(makeDetail({ uid: 'uid-1', fn: 'Ada Lovelace' }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Editar Ada Lovelace' }))
    const dialog = await screen.findByRole('dialog')
    await within(dialog).findByRole('textbox', { name: 'Nombre' })
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    // Un `PATCH` vacío sería un `400`: se avisa y no se llama a la API.
    await waitFor(() => expect(messageInfo).toHaveBeenCalledWith('No hay cambios que guardar'))
    expect(updateContact).not.toHaveBeenCalled()
    expect(screen.getByRole('dialog')).toBeInTheDocument()
  })

  it('envía un teléfono nuevo bien formado con TYPE y PREF al crear', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([])
    vi.mocked(createContact).mockResolvedValue(makeContact({ fn: 'Grace Hopper' }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByRole('heading', { name: /contactos/i, level: 1 })

    await user.click(await screen.findByRole('button', { name: /nuevo contacto/i }))
    const dialog = await screen.findByRole('dialog')
    await user.type(within(dialog).getByRole('textbox', { name: 'Nombre' }), 'Grace Hopper')

    await user.click(within(dialog).getByRole('button', { name: 'Añadir teléfono' }))
    const row = await screen.findByTestId('contact-labelled-row')
    await user.type(within(row).getByRole('textbox', { name: 'Teléfono valor' }), '+34600000000')
    const types = within(row).getByRole('combobox', { name: 'Tipos 1' })
    await user.click(types)
    await user.type(types, 'work{Enter}')
    await user.type(within(row).getByRole('spinbutton', { name: 'PREF 1' }), '1')

    await user.click(within(dialog).getByRole('button', { name: 'Crear' }))

    await waitFor(() =>
      expect(createContact).toHaveBeenCalledWith(addressBook.id, {
        fn: 'Grace Hopper',
        tels: [{ value: '+34600000000', types: ['work'], pref: 1 }],
      }),
    )
  })

  it('envía los grupos rellenos al crear y nunca los escalares heredados', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([])
    vi.mocked(createContact).mockResolvedValue(makeContact({ fn: 'Grace Hopper' }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByRole('heading', { name: /contactos/i, level: 1 })

    await user.click(await screen.findByRole('button', { name: /nuevo contacto/i }))
    const dialog = await screen.findByRole('dialog')
    await user.type(within(dialog).getByRole('textbox', { name: 'Nombre' }), 'Grace Hopper')

    await user.click(within(dialog).getByRole('button', { name: 'Añadir correo' }))
    const row = await screen.findByTestId('contact-labelled-row')
    await user.type(within(row).getByRole('textbox', { name: 'Correo valor' }), 'grace@example.com')

    await user.click(within(dialog).getByRole('button', { name: 'Crear' }))

    // Exactamente el grupo `emails`; **nunca** `email`/`phone`/`organization`.
    await waitFor(() =>
      expect(createContact).toHaveBeenCalledWith(addressBook.id, {
        fn: 'Grace Hopper',
        emails: [{ value: 'grace@example.com', types: [], pref: null }],
      }),
    )
  })

  it('borra una dirección con «Quitar» enviando las restantes', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])
    vi.mocked(getContact).mockResolvedValue(
      makeDetail({
        uid: 'uid-1',
        fn: 'Ada Lovelace',
        addresses: [
          {
            po_box: null,
            extended: null,
            street: 'Calle 1',
            locality: null,
            region: null,
            postal_code: null,
            country: null,
            label: null,
            types: [],
            pref: null,
          },
          {
            po_box: null,
            extended: null,
            street: 'Calle 2',
            locality: null,
            region: null,
            postal_code: null,
            country: null,
            label: null,
            types: [],
            pref: null,
          },
        ],
      }),
    )
    vi.mocked(updateContact).mockResolvedValue(makeContact({ fn: 'Ada Lovelace' }))

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Editar Ada Lovelace' }))
    const dialog = await screen.findByRole('dialog')
    await within(dialog).findByRole('textbox', { name: 'Nombre' })

    await user.click(within(dialog).getByRole('button', { name: 'Quitar dirección 2' }))
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() =>
      expect(updateContact).toHaveBeenCalledWith(addressBook.id, 'uid-1', {
        addresses: [
          {
            po_box: null,
            extended: null,
            street: 'Calle 1',
            locality: null,
            region: null,
            postal_code: null,
            country: null,
            label: null,
            types: [],
            pref: null,
          },
        ],
      }),
    )
  })

  it('rechaza un fichero de importación demasiado grande sin llamar al servidor', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([])

    renderContacts()
    await screen.findByRole('heading', { name: /contactos/i, level: 1 })

    // No se asigna un buffer de 5 MB real: se falsifica el tamaño del `File`.
    const big = new File(['x'], 'grande.vcf', { type: 'text/vcard' })
    Object.defineProperty(big, 'size', { value: MAX_FILE_BYTES + 1 })

    fireEvent.change(screen.getByLabelText('Importar archivo .vcf'), {
      target: { files: [big] },
    })

    await waitFor(() =>
      expect(messageError).toHaveBeenCalledWith('El archivo es demasiado grande (máximo 5 MB)'),
    )
    expect(importContacts).not.toHaveBeenCalled()
  })

  it('refresca el detalle tras editar (reabrir muestra el valor nuevo)', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([makeContact({ fn: 'Ada Lovelace' })])

    let currentFn = 'Ada Lovelace'
    vi.mocked(getContact).mockImplementation(() =>
      Promise.resolve(makeDetail({ uid: 'uid-1', fn: currentFn, full_name: currentFn })),
    )
    vi.mocked(updateContact).mockImplementation(() => {
      currentFn = 'Ada Byron'
      return Promise.resolve(makeContact({ fn: 'Ada Byron' }))
    })

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    await user.click(screen.getByRole('button', { name: 'Editar Ada Lovelace' }))
    const dialog = await screen.findByRole('dialog')
    const nameInput = await within(dialog).findByRole('textbox', { name: 'Nombre' })
    await user.clear(nameInput)
    await user.type(nameInput, 'Ada Byron')
    await user.click(within(dialog).getByRole('button', { name: 'Guardar' }))

    await waitFor(() => expect(messageSuccess).toHaveBeenCalledWith('Contacto actualizado'))

    // Reabrir el contacto pide el detalle de nuevo y muestra el valor guardado.
    await user.click(await screen.findByRole('button', { name: 'Ver Ada Byron' }))
    // El Modal de la edición anterior todavía anima su salida y conserva
    // `role="dialog"`: se consulta dentro del Drawer, que es el nodo que este
    // test necesita, y el resultado afirma exactamente lo mismo que antes.
    await waitFor(() => expect(drawer()).toBeInTheDocument())
    // El nombre nuevo aparece en el título del Drawer **y** en la fila `FN` del
    // detalle recargado; se acota a la fila del `<Descriptions>` para afirmar
    // que el detalle en sí trae el valor nuevo, no solo la cabecera.
    const fnRow = (await within(drawer()).findByText('FN')).closest('tr')
    expect(fnRow).not.toBeNull()
    expect(fnRow).toHaveTextContent('Ada Byron')
    expect(within(drawer()).queryByText('Ada Lovelace')).not.toBeInTheDocument()
  })

  it('reaplica los valores iniciales al cambiar de contacto en el modal', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([
      makeContact({ id: 'c1', uid: 'uid-1', fn: 'Ada Lovelace' }),
      makeContact({ id: 'c2', uid: 'uid-2', fn: 'Bob Esponja' }),
    ])
    vi.mocked(getContact).mockImplementation((_collectionId, uid) =>
      Promise.resolve(
        makeDetail({
          id: uid === 'uid-1' ? 'c1' : 'c2',
          uid,
          fn: uid === 'uid-1' ? 'Ada Lovelace' : 'Bob Esponja',
          full_name: uid === 'uid-1' ? 'Ada Lovelace' : 'Bob Esponja',
        }),
      ),
    )

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    // Primer contacto.
    await user.click(screen.getByRole('button', { name: 'Editar Ada Lovelace' }))
    let dialog = await screen.findByRole('dialog')
    expect(await within(dialog).findByRole('textbox', { name: 'Nombre' })).toHaveValue('Ada Lovelace')
    await user.click(within(dialog).getByRole('button', { name: 'Cancelar' }))
    // jsdom no dispara `transitionend`, así que el Modal se queda en
    // `ant-zoom-leave` conservando `role="dialog"`. Se completa la animación
    // para que se desmonte de verdad antes de abrir el siguiente contacto (si
    // no, `findByRole('dialog')` sería ambiguo).
    await waitFor(() => expect(screen.getByRole('dialog')).toHaveClass('ant-zoom-leave-active'))
    finishModalLeave()
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())

    // Segundo contacto: el formulario se remonta con los valores del nuevo.
    await user.click(screen.getByRole('button', { name: 'Editar Bob Esponja' }))
    dialog = await screen.findByRole('dialog')
    expect(await within(dialog).findByRole('textbox', { name: 'Nombre' })).toHaveValue('Bob Esponja')
  })
})

describe('vcf helpers', () => {
  it('escapa los caracteres especiales al serializar', () => {
    const content = serializeContactsToVcf([
      {
        uid: 'u1',
        fn: 'Ada Lovelace',
        email: 'ada@example.com',
        phone: '+34 600 000 000',
        organization: 'Engines; Ltd, Co',
      },
    ])

    expect(content).toContain('BEGIN:VCARD')
    expect(content).toContain('VERSION:3.0')
    expect(content).toContain('FN:Ada Lovelace')
    expect(content).toContain('EMAIL:ada@example.com')
    // `;` y `,` se escapan (RFC 6350 §3.4) para no romper el valor estructurado.
    expect(content).toContain('ORG:Engines\\; Ltd\\, Co')
    expect(content).toContain('END:VCARD')
  })

  it('duplica las barras invertidas y escapa los saltos de línea', () => {
    const content = serializeContactsToVcf([
      {
        uid: 'win-1',
        fn: 'DOM\\user',
        email: null,
        phone: null,
        // Nueva línea real → `\n` literal en la salida.
        organization: 'C:\\Users\nada',
      },
    ])

    // `\` → `\\` para que el valor no se confunda con una secuencia de escape.
    expect(content).toContain('FN:DOM\\\\user')
    expect(content).toContain('ORG:C:\\\\Users\\nada')
  })

  it('pliega las líneas a 75 octetos (RFC 6350) sin cortar el texto', () => {
    const encoder = new TextEncoder()
    // Mezcla de caracteres de 1 y 2 octetos para forzar el plegado por bytes.
    const longName = `${'Ü'.repeat(60)}${'A'.repeat(60)}`

    const content = serializeContactsToVcf([
      { uid: 'fold-1', fn: longName, email: null, phone: null, organization: null },
    ])

    const physicalLines = content.split('\r\n')
    for (const line of physicalLines) {
      expect(encoder.encode(line).length).toBeLessThanOrEqual(75)
    }
    // Existe al menos una línea de continuación (prefijada con un espacio).
    expect(physicalLines.some((line) => line.startsWith(' '))).toBe(true)
    // Al desplegar (quitar CRLF + espacio) el valor se reconstruye sin pérdida.
    const unfolded = content.replace(/\r\n[ \t]/g, '')
    expect(unfolded).toContain(`FN:${longName}`)
  })
})
