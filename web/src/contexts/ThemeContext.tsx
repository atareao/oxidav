import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react'

export type ThemeMode = 'light' | 'dark'

export interface ThemeContextValue {
  mode: ThemeMode
  /** Switch directly to a given mode (used by the Settings view). */
  setMode: (mode: ThemeMode) => void
  /** Flip between light and dark (used by the header toggle). */
  toggle: () => void
}

/**
 * `localStorage` key holding the last chosen theme.
 *
 * The theme is a non-sensitive UI preference, so persisting it across sessions
 * is safe and expected. (There are no client-side tokens to guard against since
 * F17: the backend owns the `HttpOnly` session cookie and the SPA never sees
 * any credential.)
 */
const STORAGE_KEY = 'oxidav.theme'

/** Read the persisted mode, defaulting to `light` on any failure. */
function readStoredMode(): ThemeMode {
  try {
    return localStorage.getItem(STORAGE_KEY) === 'dark' ? 'dark' : 'light'
  } catch {
    // Storage can throw when disabled (private mode / hardened browsers).
    return 'light'
  }
}

/** Best-effort persistence: a storage failure must never break theming. */
function persistMode(mode: ThemeMode): void {
  try {
    localStorage.setItem(STORAGE_KEY, mode)
  } catch {
    // Ignore: theming still works in-memory for this session.
  }
}

// eslint-disable-next-line react-refresh/only-export-components
export const ThemeContext = createContext<ThemeContextValue | null>(null)

export interface ThemeProviderProps {
  children: ReactNode
}

/**
 * Provides the light/dark mode state to the application.
 *
 * The chosen mode is persisted to `localStorage` and restored on the next load,
 * so the preference survives reloads and new sessions.
 */
export function ThemeProvider({ children }: ThemeProviderProps) {
  const [mode, setModeState] = useState<ThemeMode>(readStoredMode)

  /**
   * Push the *real app mode* to the document so the browser paints native
   * chrome (scrollbars, form controls, canvas background) in the same scheme
   * the user picked here — instead of following `prefers-color-scheme` from
   * the operating system.
   *
   * `web/src/index.css` keeps a `color-scheme: light` default for the login
   * screen, which renders *outside* this provider; this effect overrides it
   * whenever the provider is mounted.
   */
  useEffect(() => {
    document.documentElement.style.colorScheme = mode
  }, [mode])

  const setMode = useCallback((next: ThemeMode) => {
    setModeState(next)
    persistMode(next)
  }, [])

  const toggle = useCallback(() => {
    setModeState((current) => {
      const next: ThemeMode = current === 'light' ? 'dark' : 'light'
      persistMode(next)
      return next
    })
  }, [])

  const value = useMemo<ThemeContextValue>(
    () => ({ mode, setMode, toggle }),
    [mode, setMode, toggle],
  )

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}

// eslint-disable-next-line react-refresh/only-export-components
export function useTheme(): ThemeContextValue {
  const context = useContext(ThemeContext)
  if (context === null) {
    throw new Error('useTheme must be used within a ThemeProvider')
  }
  return context
}
