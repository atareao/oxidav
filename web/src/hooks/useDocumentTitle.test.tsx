/**
 * Tests del título del documento por ruta (F19, Bloque D).
 *
 * `titleForPath` es la tabla de mapeo ruta → título y `useDocumentTitle` aplica
 * ese valor a `document.title` restaurando el título base (`OxiDav`, el de
 * `web/index.html`) al desmontar.
 */

import { renderHook } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { BASE_TITLE, titleForPath, useDocumentTitle } from './useDocumentTitle'

afterEach(() => {
  document.title = BASE_TITLE
})

describe('titleForPath', () => {
  it('mapea cada ruta conocida a su título con el sufijo de marca', () => {
    expect(titleForPath('/calendar')).toBe('Calendario · OxiDav')
    expect(titleForPath('/contacts')).toBe('Contactos · OxiDav')
    expect(titleForPath('/sync')).toBe('Sincronización · OxiDav')
    expect(titleForPath('/settings')).toBe('Ajustes · OxiDav')
  })

  it('mapea rutas hijas por su segmento raíz', () => {
    expect(titleForPath('/contacts/42')).toBe('Contactos · OxiDav')
  })

  it('cae al título base para cualquier otra ruta', () => {
    expect(titleForPath('/')).toBe(BASE_TITLE)
    expect(titleForPath('/unknown')).toBe(BASE_TITLE)
  })
})

describe('useDocumentTitle', () => {
  it('aplica el título y lo restaura al título base al desmontar', () => {
    document.title = BASE_TITLE
    const { unmount } = renderHook(() => useDocumentTitle('Calendario · OxiDav'))

    expect(document.title).toBe('Calendario · OxiDav')

    unmount()

    expect(document.title).toBe(BASE_TITLE)
  })

  it('refresca el título cuando cambia la ruta', () => {
    const { rerender } = renderHook(({ title }: { title: string }) => useDocumentTitle(title), {
      initialProps: { title: 'Calendario · OxiDav' },
    })

    expect(document.title).toBe('Calendario · OxiDav')

    rerender({ title: 'Ajustes · OxiDav' })

    expect(document.title).toBe('Ajustes · OxiDav')
  })
})
