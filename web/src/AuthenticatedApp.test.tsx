/**
 * Tests de la aplicación autenticada (F17): enrutado y tema.
 *
 * Estas pruebas vivían en `App.test.tsx`; al introducir el guard `AuthGate`
 * (que solo monta la app autenticada cuando hay sesión) se movieron aquí, donde
 * se ejerce el enrutado real sin tener que simular la sesión.
 */

import { theme } from 'antd'
import { render, screen, waitFor, within } from '@testing-library/react'
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

function lastTheme() {
  const calls = configProviderSpy.mock.calls
  const lastProps = calls.at(-1)?.[0] as
    | ComponentProps<typeof import('antd').ConfigProvider>
    | undefined
  return lastProps?.theme
}

function lastAlgorithm() {
  return lastTheme()?.algorithm
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
    expect(await screen.findByRole('heading', { name: /calendario/i, level: 1 })).toBeInTheDocument()
  })

  it.each([
    ['/calendar', /calendario/i],
    ['/contacts', /contactos/i],
    ['/sync', /sincronización/i],
    ['/settings', /ajustes/i],
  ])('renders the %s route with its title', async (path, heading) => {
    renderAt(path)
    expect(await screen.findByRole('heading', { name: heading, level: 1 })).toBeInTheDocument()
  })

  it('navigates between sections using the sidebar menu', async () => {
    const user = userEvent.setup()
    renderAt('/calendar')

    await user.click(await screen.findByRole('menuitem', { name: /contactos/i }))

    expect(await screen.findByRole('heading', { name: /contactos/i, level: 1 })).toBeInTheDocument()
  })
})

describe('AuthenticatedApp theme', () => {
  beforeEach(() => {
    configProviderSpy.mockClear()
    // The chosen theme is persisted in `localStorage`; clear it so each test
    // starts from the documented default (`light`) regardless of execution order.
    localStorage.clear()
  })

  it('starts with the light (default) algorithm applied via ConfigProvider', async () => {
    renderAt('/calendar')
    await screen.findByRole('heading', { name: /calendario/i, level: 1 })

    expect(lastAlgorithm()).toBe(theme.defaultAlgorithm)
  })

  it('applies the dark algorithm after toggling the theme', async () => {
    const user = userEvent.setup()
    renderAt('/calendar')

    const toggle = await screen.findByRole('button', { name: /cambiar tema/i })
    // The toggle is icon-only; Ant Design exposes each icon as
    // `<span role="img" aria-label="moon|sun">`, so we assert on that instead
    // of the old emoji text.
    expect(within(toggle).getByRole('img', { name: 'moon' })).toBeInTheDocument()
    expect(lastAlgorithm()).not.toBe(theme.darkAlgorithm)

    await user.click(toggle)

    expect(within(toggle).getByRole('img', { name: 'sun' })).toBeInTheDocument()
    await waitFor(() => expect(lastAlgorithm()).toBe(theme.darkAlgorithm))
  })

  it('applies a non-empty design token set via ConfigProvider', async () => {
    renderAt('/calendar')
    await screen.findByRole('heading', { name: /calendario/i, level: 1 })

    const applied = lastTheme()
    expect(applied?.token).toBeDefined()

    const token = applied?.token ?? {}
    expect(Object.keys(token).length).toBeGreaterThan(0)
    expect(token.borderRadius).toBeGreaterThan(0)
    expect(token.fontSize).toBeGreaterThan(0)
    expect(token.controlHeight).toBeGreaterThan(0)
    expect(typeof token.fontFamily).toBe('string')
    expect((token.fontFamily ?? '').length).toBeGreaterThan(0)

    expect(applied?.components).toBeDefined()
    expect(Object.keys(applied?.components ?? {}).length).toBeGreaterThan(0)
  })

  it('keeps the light/dark algorithm switch while applying tokens', async () => {
    const user = userEvent.setup()
    renderAt('/calendar')

    const toggle = await screen.findByRole('button', { name: /cambiar tema/i })
    expect(lastAlgorithm()).toBe(theme.defaultAlgorithm)
    expect(Object.keys(lastTheme()?.token ?? {}).length).toBeGreaterThan(0)

    await user.click(toggle)

    await waitFor(() => expect(lastAlgorithm()).toBe(theme.darkAlgorithm))
    expect(Object.keys(lastTheme()?.token ?? {}).length).toBeGreaterThan(0)
  })
})
