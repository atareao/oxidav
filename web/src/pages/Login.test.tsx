/**
 * Tests del punto de entrada de login (F14).
 *
 * Se mockea `services/auth` (`login`) para verificar que la vista recupera la
 * ruta de origen del `location.state` (poblado por `RequireAuth`) y la reenvía
 * al flujo PKCE; cuando no hay `from`, delega en `login(undefined)`.
 */

import { render, waitFor } from '@testing-library/react'
import { App } from 'antd'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { login } from '../services/auth'
import { Login } from './Login'

vi.mock('../services/auth', () => ({
  login: vi.fn(),
}))

const loginMock = vi.mocked(login)

function renderLogin(state?: unknown) {
  return render(
    <App>
      <MemoryRouter initialEntries={[{ pathname: '/login', state }]}>
        <Routes>
          <Route path="/login" element={<Login />} />
        </Routes>
      </MemoryRouter>
    </App>,
  )
}

describe('Login', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    loginMock.mockResolvedValue(undefined)
  })

  it('pasa el `from` de location.state a login()', async () => {
    renderLogin({ from: '/settings?tab=profile#tokens' })

    await waitFor(() =>
      expect(loginMock).toHaveBeenCalledWith('/settings?tab=profile#tokens'),
    )
  })

  it('llama a login() sin argumento cuando no hay `from`', async () => {
    renderLogin()

    await waitFor(() => expect(loginMock).toHaveBeenCalledWith(undefined))
  })
})
