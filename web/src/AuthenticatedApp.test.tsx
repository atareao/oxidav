/**
 * Tests de la aplicación autenticada (F17): enrutado y tema.
 *
 * Estas pruebas vivían en `App.test.tsx`; al introducir el guard `AuthGate`
 * (que solo monta la app autenticada cuando hay sesión) se movieron aquí, donde
 * se ejerce el enrutado real sin tener que simular la sesión.
 */

import { theme } from 'antd'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { ComponentProps } from 'react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import AuthenticatedApp from './AuthenticatedApp'

// Capture every props object Ant Design's ConfigProvider receives so we can
// assert the *real* algorithm passed by the app (not just the button label).
const { configProviderSpy } = vi.hoisted(() => ({ configProviderSpy: vi.fn() }))

vi.mock('antd', async (importOriginal) => {
  const actual = await importOriginal<typeof import('antd')>()
  return {
    ...actual,
    ConfigProvider: (props: ComponentProps<typeof actual.ConfigProvider>) => {
      configProviderSpy(props)
      return <actual.ConfigProvider {...props} />
    },
  }
})

function lastAlgorithm() {
  const calls = configProviderSpy.mock.calls
  const lastProps = calls.at(-1)?.[0] as
    | ComponentProps<typeof import('antd').ConfigProvider>
    | undefined
  return lastProps?.theme?.algorithm
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AuthenticatedApp />
    </MemoryRouter>,
  )
}

describe('AuthenticatedApp routing', () => {
  beforeEach(() => {
    configProviderSpy.mockClear()
  })

  it('redirects "/" to the calendar view', async () => {
    renderAt('/')
    expect(await screen.findByRole('heading', { name: /calendario/i, level: 2 })).toBeInTheDocument()
  })

  it.each([
    ['/calendar', /calendario/i],
    ['/contacts', /contactos/i],
    ['/sync', /sincronización/i],
    ['/settings', /ajustes/i],
  ])('renders the %s route with its title', async (path, heading) => {
    renderAt(path)
    expect(await screen.findByRole('heading', { name: heading, level: 2 })).toBeInTheDocument()
  })

  it('navigates between sections using the sidebar menu', async () => {
    const user = userEvent.setup()
    renderAt('/calendar')

    await user.click(await screen.findByRole('menuitem', { name: /contactos/i }))

    expect(await screen.findByRole('heading', { name: /contactos/i, level: 2 })).toBeInTheDocument()
  })
})

describe('AuthenticatedApp theme', () => {
  beforeEach(() => {
    configProviderSpy.mockClear()
  })

  it('starts with the light (default) algorithm applied via ConfigProvider', async () => {
    renderAt('/calendar')
    await screen.findByRole('heading', { name: /calendario/i, level: 2 })

    expect(lastAlgorithm()).toBe(theme.defaultAlgorithm)
  })

  it('applies the dark algorithm after toggling the theme', async () => {
    const user = userEvent.setup()
    renderAt('/calendar')

    const toggle = await screen.findByRole('button', { name: /cambiar tema/i })
    expect(toggle).toHaveTextContent('🌙')
    expect(lastAlgorithm()).not.toBe(theme.darkAlgorithm)

    await user.click(toggle)

    expect(toggle).toHaveTextContent('☀️')
    await waitFor(() => expect(lastAlgorithm()).toBe(theme.darkAlgorithm))
  })
})
