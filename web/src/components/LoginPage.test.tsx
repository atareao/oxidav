/**
 * Tests de la pantalla de login (F17).
 *
 * El botón debe iniciar el flujo OIDC server-side navegando a `/api/auth/login`
 * con `window.location.assign` (jsdom no navega, así que se sustituye
 * `window.location` por un doble). Además debe ser un `<button>` nativo para no
 * arrastrar `antd` al bundle inicial.
 */

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { LoginPage } from './LoginPage'

const originalLocation = window.location

function stubLocation() {
  const location = {
    href: '',
    assign: vi.fn((url: string) => {
      location.href = url
    }),
    replace: vi.fn((url: string) => {
      location.href = url
    }),
  }
  Object.defineProperty(window, 'location', {
    configurable: true,
    writable: true,
    value: location,
  })
  return location
}

afterEach(() => {
  vi.restoreAllMocks()
  Object.defineProperty(window, 'location', {
    configurable: true,
    writable: true,
    value: originalLocation,
  })
})

describe('LoginPage', () => {
  it('navega a /api/auth/login al pulsar el botón de inicio de sesión', async () => {
    const user = userEvent.setup()
    const location = stubLocation()

    render(<LoginPage />)

    await user.click(screen.getByRole('button', { name: /iniciar sesión/i }))

    expect(location.href).toBe('/api/auth/login')
    expect(location.assign).toHaveBeenCalledWith('/api/auth/login')
  })

  it('usa un botón nativo (sin antd) para no engordar el bundle inicial', () => {
    render(<LoginPage />)

    const button = screen.getByRole('button', { name: /iniciar sesión/i })
    expect(button.tagName).toBe('BUTTON')
    expect(button).not.toHaveClass('ant-btn')
  })
})
