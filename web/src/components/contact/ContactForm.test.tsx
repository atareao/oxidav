/**
 * Tests de `ContactForm` (F23 además de FIX-1/FIX-10 de la auditoría de F22).
 *
 * - Validación de `TYPE`/`PREF` (los validadores que evitan un `400`).
 * - El cuerpo del formulario es un `Tabs` con las cinco pestañas del detalle,
 *   «Nombres» activa y «Nombre» (FN) siempre visible fuera de ellas.
 * - Cambiar de pestaña no pierde los `Form.List`.
 * - El primer error lleva a su pestaña y enfoca el campo; un error visible no
 *   cambia de pestaña.
 * - El payload del `PATCH` sigue enviando **solo** los grupos modificados.
 * - Las pestañas con error llevan punto rojo.
 */

import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Form } from 'antd'
import type { FormInstance } from 'antd'
import { useEffect } from 'react'
import { describe, expect, it, vi } from 'vitest'
import type { ContactDetail } from '../../types/api'
import { ContactForm } from './ContactForm'
import {
  buildUpdateInput,
  emptyFormValues,
  groupsFromDetail,
  groupsFromForm,
  initialValuesFromDetail,
  type ContactFormValues,
} from './contactFormValues'

/** Instancia capturada del `<Form>` para disparar el envío desde el test. */
let formInstance: FormInstance<ContactFormValues> | null = null

function Harness({
  initialValues,
  onFinish,
}: {
  initialValues: ContactFormValues
  onFinish: (values: ContactFormValues) => void
}) {
  const [form] = Form.useForm<ContactFormValues>()
  // La instancia se publica **después** del render (en un efecto), no durante
  // él: reasignar una variable de módulo en el cuerpo del componente es un
  // error de `react-hooks/globals`.
  useEffect(() => {
    formInstance = form
  }, [form])
  return (
    <>
      <ContactForm form={form} initialValues={initialValues} onFinish={onFinish} />
      <button type="button" onClick={() => form.submit()}>
        Enviar
      </button>
    </>
  )
}

function valuesWithTelRow(overrides: Partial<ContactFormValues> = {}): ContactFormValues {
  return {
    ...emptyFormValues(),
    fn: 'Grace Hopper',
    tels: [{ value: '+34600000000', types: [], pref: null }],
    ...overrides,
  }
}

/** Fija la fila de teléfono (dentro de `act`, para no ensuciar con warnings). */
function setTelRow(types: string[], pref: number | null) {
  act(() => {
    formInstance?.setFieldsValue({ tels: [{ value: '+34600000000', types, pref }] })
  })
}

async function submitAndExpectBlocked(onFinish: () => void, message: string) {
  const user = userEvent.setup()
  await user.click(screen.getByRole('button', { name: 'Enviar' }))
  expect(await screen.findByText(message)).toBeInTheDocument()
  await waitFor(() => expect(onFinish).not.toHaveBeenCalled())
}

describe('ContactForm: validación de TYPE y PREF', () => {
  it.each([
    ['un `TYPE` con espacio', ['work home']],
    ['un `TYPE` de 33 caracteres', ['a'.repeat(33)]],
  ])('bloquea el submit con %s', async (_label, types) => {
    const onFinish = vi.fn()
    render(<Harness initialValues={valuesWithTelRow()} onFinish={onFinish} />)

    setTelRow(types, null)

    await submitAndExpectBlocked(onFinish, 'Solo letras, números y guiones')
  })

  it('bloquea el submit con más de 8 tipos', async () => {
    const onFinish = vi.fn()
    render(<Harness initialValues={valuesWithTelRow()} onFinish={onFinish} />)

    const nineTypes = Array.from({ length: 9 }, (_value, index) => `t${index}`)
    setTelRow(nineTypes, null)

    await submitAndExpectBlocked(onFinish, 'Máximo 8 tipos')
  })

  it.each([[200], [0]])('bloquea el submit con un PREF fuera de rango (%i)', async (pref) => {
    const onFinish = vi.fn()
    render(<Harness initialValues={valuesWithTelRow()} onFinish={onFinish} />)

    setTelRow([], pref)

    await submitAndExpectBlocked(onFinish, 'El PREF debe ser un número entre 1 y 100')
  })
})

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

describe('ContactForm: pestañas (F23)', () => {
  it('escenario 1: se abre por pestañas, con «Nombres» activa y el Nombre (FN) fuera', () => {
    render(<Harness initialValues={valuesWithTelRow()} onFinish={vi.fn()} />)

    for (const label of ['Nombres', 'Contacto', 'Direcciones', 'Laboral', 'Adicional']) {
      expect(screen.getByRole('tab', { name: label })).toBeInTheDocument()
    }
    expect(screen.getByRole('tab', { name: 'Nombres' })).toHaveAttribute('aria-selected', 'true')

    const fn = screen.getByLabelText('Nombre')
    expect(fn).toBeInTheDocument()
    // Fuera de las pestañas: no vive dentro de ningún `tabpanel`.
    expect(fn.closest('[role="tabpanel"]')).toBeNull()
  })

  it('escenario 2: cambiar de pestaña no pierde filas ni valores de los Form.List', async () => {
    const user = userEvent.setup()
    const onFinish = vi.fn<(values: ContactFormValues) => void>()
    render(<Harness initialValues={{ ...emptyFormValues(), fn: 'Grace Hopper' }} onFinish={onFinish} />)

    // «Contacto»: añade un teléfono.
    await user.click(screen.getByRole('tab', { name: 'Contacto' }))
    await user.click(screen.getByRole('button', { name: 'Añadir teléfono' }))
    await user.type(screen.getByLabelText('Teléfono valor'), '+34600111222')

    // «Direcciones»: añade una dirección.
    await user.click(screen.getByRole('tab', { name: 'Direcciones' }))
    await user.click(screen.getByRole('button', { name: 'Añadir dirección' }))
    await user.type(screen.getByLabelText('Calle'), 'Calle Mayor 1')

    // Sale a «Nombres» y vuelve: los valores siguen ahí.
    await user.click(screen.getByRole('tab', { name: 'Nombres' }))
    await user.click(screen.getByRole('tab', { name: 'Contacto' }))
    expect(screen.getByLabelText('Teléfono valor')).toHaveValue('+34600111222')
    await user.click(screen.getByRole('tab', { name: 'Direcciones' }))
    expect(screen.getByLabelText('Calle')).toHaveValue('Calle Mayor 1')

    // Y al enviar, viajan en el payload.
    await user.click(screen.getByRole('button', { name: 'Enviar' }))
    await waitFor(() => expect(onFinish).toHaveBeenCalledTimes(1))
    const values = onFinish.mock.calls[0][0]
    expect(values.tels).toEqual([{ value: '+34600111222', types: [], pref: null }])
    expect(values.addresses?.[0]?.street).toBe('Calle Mayor 1')
  })

  it('escenario 3: un error en otra pestaña activa esa pestaña y enfoca el campo, sin enviar', async () => {
    const user = userEvent.setup()
    const onFinish = vi.fn()
    render(<Harness initialValues={valuesWithTelRow()} onFinish={onFinish} />)

    // Estando en «Nombres», un TYPE inválido en el teléfono de «Contacto».
    setTelRow(['work home'], null)

    await user.click(screen.getByRole('button', { name: 'Enviar' }))

    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Contacto' })).toHaveAttribute('aria-selected', 'true'),
    )
    await waitFor(() => expect(screen.getByLabelText('Tipos 1')).toHaveFocus())
    expect(onFinish).not.toHaveBeenCalled()
  })

  it('escenario 4: el error visible (Nombre) no cambia la pestaña activa', async () => {
    const user = userEvent.setup()
    const onFinish = vi.fn()
    render(<Harness initialValues={{ ...emptyFormValues(), fn: '' }} onFinish={onFinish} />)

    await user.click(screen.getByRole('tab', { name: 'Contacto' }))
    await user.click(screen.getByRole('button', { name: 'Enviar' }))

    expect(await screen.findByText('Introduce un nombre')).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Contacto' })).toHaveAttribute('aria-selected', 'true')
    await waitFor(() => expect(onFinish).not.toHaveBeenCalled())
  })

  it('escenario 6: las pestañas con error llevan punto rojo', async () => {
    const user = userEvent.setup()
    render(
      <Harness
        initialValues={{
          ...emptyFormValues(),
          fn: 'Grace Hopper',
          tels: [{ value: '+34600000000', types: [], pref: null }],
          addresses: [{ types: [], pref: null }],
        }}
        onFinish={vi.fn()}
      />,
    )

    act(() => {
      formInstance?.setFieldsValue({
        tels: [{ value: '+34600000000', types: ['work home'], pref: null }],
        addresses: [{ types: ['bad type'], pref: null }],
      })
    })

    await user.click(screen.getByRole('button', { name: 'Enviar' }))

    expect(await screen.findByTestId('contact-tab-error-contact')).toBeInTheDocument()
    expect(screen.getByTestId('contact-tab-error-addresses')).toBeInTheDocument()
    // Las que no tienen error quedan limpias.
    expect(screen.queryByTestId('contact-tab-error-names')).toBeNull()
    expect(screen.queryByTestId('contact-tab-error-work')).toBeNull()
    expect(screen.queryByTestId('contact-tab-error-additional')).toBeNull()
  })
})

describe('ContactForm: payload del PATCH (F23)', () => {
  it('escenario 5a: modificar solo un teléfono produce exactamente el grupo `tels`', async () => {
    const user = userEvent.setup()
    const detail = makeDetail({
      name: { family: 'Lovelace', given: 'Ada', additional: [], prefix: [], suffix: [] },
      tels: [{ value: '+34600000000', types: ['work'], pref: 1 }],
      emails: [{ value: 'ada@example.com', types: [], pref: null }],
      addresses: [{ po_box: null, extended: null, street: 'Calle Mayor 1', locality: null, region: null, postal_code: null, country: null, label: null, types: [], pref: null }],
      notes: ['Una nota'],
      organizations: [['Analytical Engines']],
      categories: [['amigos']],
      urls: ['https://example.com'],
    })
    const initialValues = initialValuesFromDetail(detail)
    const initialGroups = groupsFromDetail(detail)

    const onFinish = vi.fn<(values: ContactFormValues) => void>()
    render(<Harness initialValues={initialValues} onFinish={onFinish} />)

    // Cambia **solo** el valor del teléfono existente.
    act(() => {
      formInstance?.setFieldsValue({ tels: [{ value: '+34600999888', types: ['work'], pref: 1 }] })
    })

    await user.click(screen.getByRole('button', { name: 'Enviar' }))
    await waitFor(() => expect(onFinish).toHaveBeenCalledTimes(1))

    const input = buildUpdateInput(initialGroups, groupsFromForm(onFinish.mock.calls[0][0]))
    expect(input).toEqual({ tels: [{ value: '+34600999888', types: ['work'], pref: 1 }] })
  })

  it('escenario 5b: sin tocar nada, el payload es vacío', () => {
    const detail = makeDetail({
      tels: [{ value: '+34600000000', types: ['work'], pref: 1 }],
      notes: ['Una nota'],
      urls: ['https://example.com'],
    })
    const values = initialValuesFromDetail(detail)
    expect(buildUpdateInput(groupsFromDetail(detail), groupsFromForm(values))).toEqual({})
  })
})

describe('ContactForm: accesibilidad de la pestaña con error (auditoría F23)', () => {
  it('hallazgo 1: describe la pestaña con `aria-describedby` sin tocar su nombre accesible', async () => {
    const user = userEvent.setup()
    render(
      <Harness
        initialValues={{
          ...emptyFormValues(),
          fn: 'Grace Hopper',
          tels: [{ value: '+34600000000', types: [], pref: null }],
        }}
        onFinish={vi.fn()}
      />,
    )

    act(() => {
      formInstance?.setFieldsValue({
        tels: [{ value: '+34600000000', types: ['work home'], pref: null }],
      })
    })
    await user.click(screen.getByRole('button', { name: 'Enviar' }))

    // El nombre accesible sigue siendo exactamente «Contacto» (consulta exacta,
    // la misma que usan `Contacts.test.tsx` y el resto de escenarios).
    const tab = await screen.findByRole('tab', { name: 'Contacto' })

    // La descripción llega **aparte**, vía `aria-describedby`, no por el nombre.
    expect(tab).toHaveAttribute('aria-describedby', 'contact-tab-error-desc-contact')
    const description = document.getElementById('contact-tab-error-desc-contact')
    expect(description).not.toBeNull()
    expect(description).toHaveClass('sr-only')
    expect(description).toHaveTextContent(/error/i)
    expect(tab).toHaveAccessibleDescription(/error/i)

    // El `title` da el tooltip: vive en la etiqueta, dentro de la pestaña.
    expect(within(tab).getByTitle(/error/i)).toBeInTheDocument()

    // Una pestaña sin error no lleva ni descripción ni tooltip.
    const names = screen.getByRole('tab', { name: 'Nombres' })
    expect(names).not.toHaveAttribute('aria-describedby')
    expect(within(names).queryByTitle(/error/i)).toBeNull()
  })

  it('hallazgo 2: teclear en «Nombre» no recalcula los errores de las pestañas', async () => {
    const user = userEvent.setup()
    render(<Harness initialValues={valuesWithTelRow()} onFinish={vi.fn()} />)

    // En el diseño anterior un `shouldUpdate={() => true}` llamaba a
    // `form.getFieldsError()` en **cada** tecla. Ahora la tabla de errores se
    // deriva de `onFieldsChange`, así que teclear no debe consultarla.
    const spy = vi.spyOn(formInstance!, 'getFieldsError')
    const input = screen.getByLabelText('Nombre')
    await user.type(input, 'abc')
    expect(spy).not.toHaveBeenCalled()
    spy.mockRestore()
  })

  it('hallazgo 2 (guard): el punto aparece, se corrige y vuelve a aparecer', async () => {
    const user = userEvent.setup()
    render(<Harness initialValues={valuesWithTelRow()} onFinish={vi.fn()} />)

    // Aparece.
    setTelRow(['work home'], null)
    await user.click(screen.getByRole('button', { name: 'Enviar' }))
    expect(await screen.findByTestId('contact-tab-error-contact')).toBeInTheDocument()

    // Se corrige (un submit válido limpia los errores → el conjunto cambia).
    setTelRow(['work'], null)
    await user.click(screen.getByRole('button', { name: 'Enviar' }))
    await waitFor(() => expect(screen.queryByTestId('contact-tab-error-contact')).toBeNull())

    // Vuelve a aparecer (el guard no se «traga» la transición de vuelta).
    setTelRow(['bad type'], null)
    await user.click(screen.getByRole('button', { name: 'Enviar' }))
    expect(await screen.findByTestId('contact-tab-error-contact')).toBeInTheDocument()
  })

  it('hallazgo 3: fija la convención de ids de Ant Design (`tels_0_types`)', () => {
    const { container } = render(<Harness initialValues={valuesWithTelRow()} onFinish={vi.fn()} />)
    // `['tels', 0, 'types']` → `id="tels_0_types"`. Si Ant Design cambiara la
    // convención, este test se pone rojo en vez de perder el foco en silencio.
    const control = container.querySelector('#tels_0_types')
    expect(control).not.toBeNull()
    expect(control).toBeInstanceOf(HTMLInputElement)
  })

  it('hallazgo 4: un error en la pestaña ya activa enfoca el campo sin cambiar de pestaña', async () => {
    const user = userEvent.setup()
    render(<Harness initialValues={valuesWithTelRow()} onFinish={vi.fn()} />)

    // «Contacto» ya está activa antes de enviar.
    await user.click(screen.getByRole('tab', { name: 'Contacto' }))
    setTelRow(['work home'], null)
    await user.click(screen.getByRole('button', { name: 'Enviar' }))

    expect(screen.getByRole('tab', { name: 'Contacto' })).toHaveAttribute('aria-selected', 'true')
    // El camino de foco «bueno» se aplica también aquí (no solo al cambiar de pestaña).
    await waitFor(() => expect(screen.getByLabelText('Tipos 1')).toHaveFocus())
  })
})
