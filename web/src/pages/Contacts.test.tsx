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
  listAddressbookCollections,
  listContacts,
  updateContact,
} from '../services/contacts'
import type { Collection, Contact } from '../types/api'
import { parseVcf, serializeContactsToVcf } from '../utils/vcf'
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
 * Espías sobre la API de mensajes que `App.useApp()` publica en su contexto:
 * la misma instancia que consume la vista (en lugar del export estático
 * `message`). Mockear el hook mantiene ese contrato bajo test sin ensuciar el
 * DOM con toasts reales.
 */
const messageSuccess = vi.fn()
const messageError = vi.fn()
const messageWarning = vi.fn()

function renderContacts() {
  vi.spyOn(App, 'useApp').mockReturnValue({
    message: {
      success: messageSuccess,
      error: messageError,
      info: vi.fn(),
      warning: messageWarning,
      loading: vi.fn(),
      open: vi.fn(),
      destroy: vi.fn(),
    },
  } as unknown as ReturnType<typeof App.useApp>)

  render(
    <App>
      <Contacts />
    </App>,
  )
}

// The export test replaces these browser APIs to capture the generated Blob;
// restoring them after every test keeps the global `URL` from leaking mocks.
const originalCreateObjectURL = URL.createObjectURL
const originalRevokeObjectURL = URL.revokeObjectURL

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

    const user = userEvent.setup()
    renderContacts()
    await screen.findByText('Ada Lovelace')

    const verButton = screen.getByRole('button', { name: 'Ver Ada Lovelace' })
    verButton.focus()
    expect(verButton).toHaveFocus()

    await user.keyboard('{Enter}')

    const drawer = await screen.findByRole('dialog')
    // El UID sólo aparece en el Drawer de detalle (no en la tabla).
    expect(within(drawer).getByText('uid-1')).toBeInTheDocument()
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
    const nameInput = within(dialog).getByRole('textbox', { name: 'Nombre' })
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

  it('importa un fichero .vcf creando un contacto por ficha', async () => {
    vi.mocked(listAddressbookCollections).mockResolvedValue([addressBook])
    vi.mocked(listContacts).mockResolvedValue([])
    vi.mocked(createContact).mockResolvedValue(makeContact({ uid: 'imported-1', fn: 'Imported Person' }))

    renderContacts()
    await screen.findByRole('heading', { name: /contactos/i, level: 1 })

    const vcf = serializeContactsToVcf([
      {
        uid: 'vcf-1',
        fn: 'Imported Person',
        email: 'imported@example.com',
        phone: null,
        organization: null,
      },
    ])
    const file = new File([vcf], 'contactos.vcf', { type: 'text/vcard' })

    const input = screen.getByLabelText('Importar archivo .vcf')
    fireEvent.change(input, { target: { files: [file] } })

    await waitFor(() =>
      expect(createContact).toHaveBeenCalledWith(
        addressBook.id,
        expect.objectContaining({ fn: 'Imported Person', email: 'imported@example.com' }),
      ),
    )
    await waitFor(() => expect(messageSuccess).toHaveBeenCalledWith('1 contacto(s) importado(s)'))
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
})

describe('vcf helpers', () => {
  it('redondea la serialización y el parseo', () => {
    const content = serializeContactsToVcf([
      {
        uid: 'u1',
        fn: 'Ada Lovelace',
        email: 'ada@example.com',
        phone: '+34 600 000 000',
        organization: 'Engines; Ltd, Co',
      },
    ])

    const parsed = parseVcf(content)
    expect(parsed).toHaveLength(1)
    expect(parsed[0]?.fn).toBe('Ada Lovelace')
    expect(parsed[0]?.email).toBe('ada@example.com')
    expect(parsed[0]?.organization).toBe('Engines; Ltd, Co')
  })

  it('redondea el .vcf conservando barras invertidas (rutas Windows y DOM\\user)', () => {
    const source = [
      {
        uid: 'win-1',
        fn: 'DOM\\user',
        email: 'dom@example.com',
        phone: null,
        organization: 'C:\\new',
      },
      {
        uid: 'win-2',
        fn: 'Ada Lovelace',
        email: null,
        phone: null,
        organization: 'C:\\Users\\ada',
      },
    ]

    const parsed = parseVcf(serializeContactsToVcf(source))

    expect(parsed).toHaveLength(2)
    expect(parsed[0]?.fn).toBe('DOM\\user')
    expect(parsed[0]?.organization).toBe('C:\\new')
    expect(parsed[1]?.organization).toBe('C:\\Users\\ada')
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
    // El valor se reconstruye sin pérdida al desplegar.
    expect(parseVcf(content)[0]?.fn).toBe(longName)
  })
})
