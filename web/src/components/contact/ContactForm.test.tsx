/**
 * Tests de validación de `ContactForm` (FIX-1/FIX-10 de la auditoría de F22).
 *
 * Cubren los validadores que evitan un `400` del servidor: el alfabeto y el
 * número máximo de `TYPE`, y el rango `1..=100` de `PREF`. Se monta el
 * formulario con una fila de teléfono y se fuerza el valor por `setFieldsValue`
 * (la validación corre igual en `submit`) para no depender del widget `Select`.
 */

import { act, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { Form } from 'antd'
import type { FormInstance } from 'antd'
import { useEffect } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { ContactForm } from './ContactForm'
import { emptyFormValues, type ContactFormValues } from './contactFormValues'

/** Instancia capturada del `<Form>` para disparar el envío desde el test. */
let formInstance: FormInstance<ContactFormValues> | null = null

function Harness({ initialValues, onFinish }: { initialValues: ContactFormValues; onFinish: () => void }) {
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
