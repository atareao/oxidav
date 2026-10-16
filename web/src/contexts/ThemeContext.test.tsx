/**
 * Tests de `ThemeContext` (F19, Paso 7).
 *
 * El `color-scheme` del documento debe seguir el **modo real de la app**
 * (claro/oscuro elegido en el conmutador) y **no** el del sistema operativo.
 * Antes de F19, `web/src/index.css` declaraba `color-scheme: light dark`, así
 * que el navegador decidía en función de `prefers-color-scheme`, ignorando el
 * conmutador interno.
 */

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { ThemeProvider, useTheme } from './ThemeContext'

/** Small consumer exposing the mode and the toggle to the test. */
function Probe() {
  const { mode, toggle } = useTheme()
  return (
    <div>
      <span data-testid="mode">{mode}</span>
      <button type="button" onClick={toggle}>
        cambiar tema
      </button>
    </div>
  )
}

function renderProbe() {
  return render(
    <ThemeProvider>
      <Probe />
    </ThemeProvider>,
  )
}

/** Force the simulated OS colour preference reported by `matchMedia`. */
function setSystemDark(dark: boolean): void {
  window.matchMedia = (query: string): MediaQueryList =>
    ({
      matches: dark && query.includes('prefers-color-scheme: dark'),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList
}

describe('ThemeContext document color-scheme', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  afterEach(() => {
    // Do not leak the pushed value into other suites.
    document.documentElement.style.removeProperty('color-scheme')
  })

  it('pushes the app light mode as the document color-scheme', () => {
    renderProbe()
    expect(document.documentElement.style.colorScheme).toBe('light')
  })

  it('follows the app toggle instead of the operating system', async () => {
    // The OS says "dark"; the app still defaults to light.
    setSystemDark(true)
    const user = userEvent.setup()
    renderProbe()

    expect(screen.getByTestId('mode')).toHaveTextContent('light')
    expect(document.documentElement.style.colorScheme).toBe('light')

    await user.click(screen.getByRole('button', { name: /cambiar tema/i }))

    expect(screen.getByTestId('mode')).toHaveTextContent('dark')
    expect(document.documentElement.style.colorScheme).toBe('dark')
  })

  it('restores a stored dark mode on mount', () => {
    // OS says "light" but the persisted app preference is dark.
    setSystemDark(false)
    localStorage.setItem('oxidav.theme', 'dark')
    renderProbe()

    expect(document.documentElement.style.colorScheme).toBe('dark')
  })
})
