/**
 * Tests del servicio de contactos (F22): `getContact` construye la URL de
 * detalle y `importContacts` envía el `.vcf` **en crudo** con `text/vcard`.
 *
 * Se mockea `fetch` global (como en `services/api.test.ts`) y se ejercita el
 * cliente real, de modo que se comprueba también el manejo del sobre de error
 * (`ApiError`) compartido por todo el SPA.
 */

import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from './api'
import { getContact, importContacts } from './contacts'
import type { Contact, ContactDetail } from '../types/api'

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    json: async () => body,
  } as unknown as Response
}

function errorResponse(status: number, code = 'not_found', message = 'No existe'): Response {
  return jsonResponse({ error: { code, message } }, status)
}

function lastCall(fetchMock: ReturnType<typeof vi.fn>): [string, RequestInit] {
  return fetchMock.mock.calls[0] as [string, RequestInit]
}

const detail: ContactDetail = {
  id: 'c1',
  uid: 'ada@example.com',
  collection_id: 'ab-1',
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
}

function makeContact(overrides: Partial<Contact> = {}): Contact {
  return {
    id: 'c1',
    uid: 'uid@example.com',
    collection_id: 'ab-1',
    fn: 'Ada Lovelace',
    email: null,
    phone: null,
    organization: null,
    etag: 'etag-1',
    created_at: 0,
    updated_at: 0,
    ...overrides,
  }
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('getContact', () => {
  it('pide el detalle a la URL del contacto y devuelve el DTO', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(detail))
    vi.stubGlobal('fetch', fetchMock)

    const result = await getContact('ab-1', 'ada@example.com')

    const [url, init] = lastCall(fetchMock)
    expect(url).toBe('/api/v1/collections/ab-1/contacts/ada%40example.com')
    expect(init).toMatchObject({ method: 'GET', credentials: 'include' })
    expect(result).toEqual(detail)
  })

  it('propaga el mensaje del sobre de error del backend', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(errorResponse(404, 'not_found', 'No existe')))

    const error = await getContact('ab-1', 'ghost@example.com').catch((e: unknown) => e)

    expect(error).toBeInstanceOf(ApiError)
    expect((error as ApiError).status).toBe(404)
    expect((error as ApiError).code).toBe('not_found')
    expect((error as ApiError).message).toBe('No existe')
  })
})

describe('importContacts', () => {
  it('envía el texto .vcf en crudo con Content-Type text/vcard y devuelve las tarjetas', async () => {
    const created = [makeContact({ uid: 'imported-1', fn: 'Imported Person' })]
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(created, 201))
    vi.stubGlobal('fetch', fetchMock)

    const text = 'BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Imported Person\r\nEND:VCARD\r\n'
    const result = await importContacts('ab-1', text)

    const [url, init] = lastCall(fetchMock)
    expect(url).toBe('/api/v1/collections/ab-1/contacts/import')
    expect(init.method).toBe('POST')
    expect(new Headers(init.headers).get('Content-Type')).toBe('text/vcard')
    // El cuerpo viaja tal cual, sin serializar como JSON (sin comillas).
    expect(init.body).toBe(text)
    expect(result).toEqual(created)
  })

  it('propaga el mensaje del sobre de error del backend', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(errorResponse(400, 'validation', 'vCard inválida')),
    )

    const error = await importContacts('ab-1', 'not a card').catch((e: unknown) => e)

    expect(error).toBeInstanceOf(ApiError)
    expect((error as ApiError).message).toBe('vCard inválida')
  })
})
