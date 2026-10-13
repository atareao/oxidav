/**
 * Tests del callback OIDC (F14).
 *
 * Se mockea `services/auth` para controlar el resultado del intercambio y la
 * ruta de retorno (`consumeReturnTo`), y se monta el componente en un
 * `MemoryRouter` con rutas centinela para verificar a dónde navega tras el éxito
 * y qué muestra ante un error.
 */

import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { consumeReturnTo, handleRedirectCallback } from '../services/auth'
import { Callback } from './Callback'

vi.mock('../services/auth', () => ({
  handleRedirectCallback: vi.fn(),
  consumeReturnTo: vi.fn(),
}))

const handleMock = vi.mocked(handleRedirectCallback)
const consumeMock = vi.mocked(consumeReturnTo)

function renderCallback() {
  return render(
    <MemoryRouter initialEntries={['/callback']}>
      <Routes>
        <Route path="/callback" element={<Callback />} />
        <Route path="/calendar" element={<div>Calendario</div>} />
        <Route path="/settings/profile" element={<div>Perfil</div>} />
      </Routes>
    </MemoryRouter>,
  )
}

describe('Callback', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('navega al returnTo guardado tras el éxito', async () => {
    handleMock.mockResolvedValueOnce(undefined)
    consumeMock.mockReturnValueOnce('/settings/profile')

    renderCallback()

    expect(await screen.findByText('Perfil')).toBeInTheDocument()
  })

  it('navega a /calendar por defecto cuando no hay returnTo', async () => {
    handleMock.mockResolvedValueOnce(undefined)
    consumeMock.mockReturnValueOnce(null)

    renderCallback()

    expect(await screen.findByText('Calendario')).toBeInTheDocument()
  })

  it('muestra un mensaje de error cuando el intercambio falla', async () => {
    handleMock.mockRejectedValueOnce(new Error('state inválido'))

    renderCallback()

    expect(await screen.findByText('state inválido')).toBeInTheDocument()
  })
})