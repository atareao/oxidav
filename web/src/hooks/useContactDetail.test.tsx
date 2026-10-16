/**
 * Tests de `useContactDetail` (F22): solo pide con un objetivo fijado, deriva
 * `loading` de la *request key* y descarta una respuesta obsoleta que llegue
 * después de cambiar de objetivo (el `AbortController` la aborta).
 *
 * El servicio `getContact` se mockea para controlar la resolución de cada
 * petición de forma independiente.
 */

import { act, renderHook, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { getContact } from '../services/contacts'
import type { ContactDetail } from '../types/api'
import { useContactDetail, type ContactDetailTarget } from './useContactDetail'

vi.mock('../services/contacts', () => ({
  getContact: vi.fn(),
}))

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

afterEach(() => {
  vi.clearAllMocks()
})

describe('useContactDetail', () => {
  it('no pide nada con target `null`', () => {
    const { result } = renderHook(() => useContactDetail(null))

    expect(getContact).not.toHaveBeenCalled()
    expect(result.current.loading).toBe(false)
    expect(result.current.detail).toBeNull()
    expect(result.current.error).toBeNull()
  })

  it('pide al fijar un objetivo y deja el detalle al resolver', async () => {
    vi.mocked(getContact).mockResolvedValue(makeDetail({ fn: 'Grace Hopper' }))

    const { result } = renderHook(() => useContactDetail({ collectionId: 'ab-1', uid: 'uid-1' }))

    // Antes de resolver: cargando y sin detalle filtrado.
    expect(result.current.loading).toBe(true)
    expect(result.current.detail).toBeNull()

    await waitFor(() => expect(result.current.loading).toBe(false))
    expect(getContact).toHaveBeenCalledWith('ab-1', 'uid-1', expect.anything())
    expect(result.current.detail?.fn).toBe('Grace Hopper')
    expect(result.current.error).toBeNull()
  })

  it('expone el mensaje cuando la petición falla', async () => {
    vi.mocked(getContact).mockRejectedValue(new Error('no disponible'))

    const { result } = renderHook(() => useContactDetail({ collectionId: 'ab-1', uid: 'uid-1' }))

    await waitFor(() => expect(result.current.error).toBe('no disponible'))
    expect(result.current.loading).toBe(false)
    expect(result.current.detail).toBeNull()
  })

  it('descarta una respuesta obsoleta al cambiar de objetivo', async () => {
    let resolveFirst!: (value: ContactDetail) => void
    let resolveSecond!: (value: ContactDetail) => void
    vi.mocked(getContact).mockImplementation((_collectionId, uid) => {
      if (uid === 'first') {
        return new Promise<ContactDetail>((resolve) => {
          resolveFirst = resolve
        })
      }
      return new Promise<ContactDetail>((resolve) => {
        resolveSecond = resolve
      })
    })

    const { result, rerender } = renderHook(
      (target: ContactDetailTarget) => useContactDetail(target),
      { initialProps: { collectionId: 'ab-1', uid: 'first' } },
    )

    // Cambia de objetivo antes de que la primera petición resuelva: la primera
    // queda abortada.
    rerender({ collectionId: 'ab-1', uid: 'second' })

    // La primera respuesta llega tarde y debe descartarse.
    await act(async () => {
      resolveFirst(makeDetail({ uid: 'first', fn: 'Obsoleto' }))
    })
    expect(result.current.detail).toBeNull()

    await act(async () => {
      resolveSecond(makeDetail({ uid: 'second', fn: 'Vigente' }))
    })

    await waitFor(() => expect(result.current.detail?.fn).toBe('Vigente'))
    expect(result.current.detail?.uid).toBe('second')
    expect(result.current.detail?.fn).not.toBe('Obsoleto')
  })
})
