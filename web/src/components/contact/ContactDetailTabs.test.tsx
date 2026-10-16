/**
 * Tests de `ContactDetailTabs` (F22): es un componente puro y presentacional, así
 * que no se mockea nada; se le pasa un `ContactDetail` construido a mano y se
 * comprueba la organización en pestañas.
 *
 * Cubren las reglas del contrato 9 del plan: una pestaña vacía no se renderiza,
 * los multivalor exponen `TYPE`/`PREF`, las fotos se pintan con su `data:`/URI,
 * «Otros» muestra las propiedades sin modelar y ningún valor se inyecta como
 * HTML (todo se pinta como texto).
 */

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import type { ContactDetail } from '../../types/api'
import { formatDateTime } from '../../utils/date'
import { ContactDetailTabs } from './ContactDetailTabs'

/** Detalle mínimo por defecto; cada test lo completa con `overrides`. */
function makeDetail(overrides: Partial<ContactDetail> = {}): ContactDetail {
  return {
    id: 'c1',
    uid: 'uid-1',
    collection_id: 'ab-1',
    fn: 'Ada Lovelace',
    full_name: 'Ada Lovelace',
    etag: 'etag-1',
    created_at: 0,
    updated_at: 0,
    version: null,
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

/** Tarjeta completa que toca todas las pestañas. */
function fullDetail(): ContactDetail {
  return makeDetail({
    version: '4.0',
    rev: '2026-01-01T00:00:00Z',
    name: {
      family: 'Lovelace',
      given: 'Ada',
      additional: ['Augusta'],
      prefix: ['Countess'],
      suffix: [],
    },
    nicknames: ['Ada'],
    photos: [{ kind: 'data', media_type: 'image/png', data_uri: 'data:image/png;base64,AAAA' }],
    birthday: '1815-12-10',
    anniversary: '1835-07-08',
    gender: { sex: 'F', identity: 'mujer' },
    tels: [
      { value: '+34 600 000 001', types: ['cell'], pref: 1 },
      { value: '+34 600 000 002', types: ['work', 'voice'], pref: null },
      { value: '+34 600 000 003', types: [], pref: null },
    ],
    emails: [{ value: 'ada@example.com', types: ['work'], pref: 1 }],
    impps: [{ value: 'ada@matrix', types: ['work'], pref: null }],
    addresses: [
      {
        po_box: null,
        extended: null,
        street: 'Calle Mayor 1',
        locality: 'Madrid',
        region: 'Madrid',
        postal_code: '28001',
        country: 'España',
        label: 'Casa',
        types: ['home'],
        pref: 1,
      },
      {
        po_box: null,
        extended: 'Piso 2',
        street: 'Gran Vía 2',
        locality: 'Madrid',
        region: null,
        postal_code: '28002',
        country: 'España',
        label: null,
        types: [],
        pref: null,
      },
    ],
    geos: ['geo:40.4168,-3.7038'],
    timezones: ['Europe/Madrid'],
    titles: ['Matemática'],
    roles: ['Programadora'],
    organizations: [['Analytical Engines', 'Investigación']],
    logos: [{ kind: 'uri', uri: 'https://example.com/logo.png' }],
    notes: ['Nota personal'],
    categories: [['amigos', 'vip']],
    urls: ['https://example.com'],
    sounds: [{ kind: 'data', media_type: 'audio/mpeg', data_uri: 'data:audio/mpeg;base64,BBBB' }],
    others: [{ name: 'X-ABLabel', group: 'item1', parameters: [['TYPE', 'work']], value: 'Trabajo' }],
  })
}

describe('ContactDetailTabs', () => {
  it('muestra cada pestaña con su contenido en una tarjeta completa', async () => {
    const user = userEvent.setup()
    render(<ContactDetailTabs detail={fullDetail()} />)

    // Control es la pestaña activa por defecto.
    expect(screen.getByRole('tab', { name: 'Control' })).toBeInTheDocument()
    expect(screen.getByText('Ada Lovelace')).toBeInTheDocument()
    expect(screen.getByText('4.0')).toBeInTheDocument()

    await user.click(screen.getByRole('tab', { name: 'Nombres' }))
    expect(await screen.findByText('Lovelace')).toBeInTheDocument()
    expect(await screen.findByText('Augusta')).toBeInTheDocument()
    expect(await screen.findByText('1815-12-10')).toBeInTheDocument()

    await user.click(screen.getByRole('tab', { name: 'Contacto' }))
    expect(await screen.findByText('+34 600 000 001')).toBeInTheDocument()

    await user.click(screen.getByRole('tab', { name: 'Direcciones' }))
    expect(await screen.findByText('Calle Mayor 1')).toBeInTheDocument()

    await user.click(screen.getByRole('tab', { name: 'Laboral' }))
    expect(await screen.findByText('Matemática')).toBeInTheDocument()

    await user.click(screen.getByRole('tab', { name: 'Adicional' }))
    expect(await screen.findByText('Nota personal')).toBeInTheDocument()

    await user.click(screen.getByRole('tab', { name: 'Servidor' }))
    expect(await screen.findByText('etag-1')).toBeInTheDocument()

    await user.click(screen.getByRole('tab', { name: 'Otros' }))
    expect(await screen.findByText('X-ABLabel')).toBeInTheDocument()
  })

  it('no renderiza las pestañas sin contenido (detalle mínimo)', () => {
    // Solo fn/uid/etag: el resto de la proyección va vacío o nulo.
    render(<ContactDetailTabs detail={makeDetail()} />)

    expect(screen.getByRole('tab', { name: 'Control' })).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Servidor' })).toBeInTheDocument()

    for (const label of ['Nombres', 'Contacto', 'Direcciones', 'Laboral', 'Adicional', 'Otros']) {
      expect(screen.queryByRole('tab', { name: label })).not.toBeInTheDocument()
    }
  })

  it('cambia de pestaña al pulsarla y muestra su contenido', async () => {
    const user = userEvent.setup()
    render(
      <ContactDetailTabs detail={makeDetail({ tels: [{ value: '+34 600 000 000', types: [], pref: null }] })} />,
    )

    // La pestaña activa por defecto no es Contacto.
    expect(screen.queryByText('+34 600 000 000')).not.toBeInTheDocument()

    await user.click(screen.getByRole('tab', { name: 'Contacto' }))
    expect(await screen.findByText('+34 600 000 000')).toBeInTheDocument()
  })

  it('muestra los TYPE y el PREF de un teléfono', async () => {
    const user = userEvent.setup()
    render(
      <ContactDetailTabs
        detail={makeDetail({ tels: [{ value: '+34 600 000 000', types: ['cell', 'voice'], pref: 1 }] })}
      />,
    )

    await user.click(screen.getByRole('tab', { name: 'Contacto' }))

    expect(await screen.findByText('cell')).toBeInTheDocument()
    expect(screen.getByText('voice')).toBeInTheDocument()
    expect(screen.getByText('PREF 1')).toBeInTheDocument()
  })

  it('pinta la foto con su `data_uri` y el logo con su URI externa', async () => {
    const user = userEvent.setup()
    const detail = fullDetail()
    const { container } = render(<ContactDetailTabs detail={detail} />)

    await user.click(screen.getByRole('tab', { name: 'Nombres' }))
    const photo = container.querySelector('img[src^="data:image/png"]')
    expect(photo).not.toBeNull()
    expect(photo).toHaveAttribute('src', 'data:image/png;base64,AAAA')

    await user.click(screen.getByRole('tab', { name: 'Laboral' }))
    const logo = container.querySelector('img[src="https://example.com/logo.png"]')
    expect(logo).not.toBeNull()
  })

  it('muestra nombre, grupo y valor de las propiedades de «Otros»', async () => {
    const user = userEvent.setup()
    render(
      <ContactDetailTabs
        detail={makeDetail({
          others: [
            { name: 'X-ABLabel', group: 'item1', parameters: [['TYPE', 'work']], value: 'Trabajo' },
          ],
        })}
      />,
    )

    await user.click(screen.getByRole('tab', { name: 'Otros' }))

    expect(await screen.findByText('X-ABLabel')).toBeInTheDocument()
    expect(screen.getByText('item1')).toBeInTheDocument()
    expect(screen.getByText('Trabajo')).toBeInTheDocument()
  })

  it('pinta los valores maliciosos como texto y no inyecta HTML', async () => {
    const user = userEvent.setup()
    const payload = '<script>alert(1)</script>'
    const imgPayload = '<img src=x onerror=alert(1)>'

    const { container } = render(
      <ContactDetailTabs
        detail={makeDetail({
          fn: payload,
          others: [{ name: 'X-EVIL', group: null, parameters: [], value: imgPayload }],
        })}
      />,
    )

    // El `<script>` aparece literal en el DOM…
    expect(screen.getByText(payload)).toBeInTheDocument()
    // …pero no se ha creado ningún nodo `<script>` real.
    expect(container.querySelector('script')).toBeNull()

    await user.click(screen.getByRole('tab', { name: 'Otros' }))
    const evil = await screen.findByText(imgPayload)
    expect(evil).toBeInTheDocument()
    expect(container.querySelector('img[src="x"]')).toBeNull()
  })

  it('formatea `created_at`/`updated_at` (epoch ms) como el resto de la app', async () => {
    const user = userEvent.setup()
    const instant = Date.UTC(2026, 0, 2, 10, 30)
    render(<ContactDetailTabs detail={makeDetail({ created_at: instant, updated_at: instant })} />)

    await user.click(screen.getByRole('tab', { name: 'Servidor' }))

    expect(await screen.findAllByText(formatDateTime(instant))).toHaveLength(2)
  })

  it('muestra un `<Empty>` cuando no hay ninguna pestaña con contenido', () => {
    // Escenario defensivo: sin uid, sin etag, sin colección y sin datos.
    render(
      <ContactDetailTabs
        detail={makeDetail({ fn: null, uid: '', etag: '', collection_id: '', created_at: 0, updated_at: 0 })}
      />,
    )

    expect(screen.getByText('Este contacto no tiene datos que mostrar')).toBeInTheDocument()
    expect(screen.queryByRole('tab')).not.toBeInTheDocument()
  })
})
