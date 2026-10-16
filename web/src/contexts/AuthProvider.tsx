/**
 * Proveedor de sesión de la SPA (F17).
 *
 * Espejo del `AuthProvider` de `valet/frontend` adaptado al stack de OxiDav:
 * resuelve la sesión al montar con `GET /api/auth/me`, reacciona al evento
 * global de 401 que emite el cliente HTTP y expone `logout()`.
 *
 * No hay tokens en el cliente: la cookie de sesión viaja sola con
 * `credentials: "include"`. Ver `plans/PLAN-018-oidc-valet-parity-COMPLETED.md`.
 */

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { api, UNAUTHORIZED_EVENT } from '../services/api'
import { AuthContext, type AuthContextValue, type AuthUser } from './AuthContext'

/**
 * Base pública de la app. `vite.config.ts` usa `base: './'`, que Vite normaliza
 * a una base relativa: en ese caso queremos navegar a la raíz absoluta `/`, no
 * resolver una ruta relativa al path actual. Si algún día se despliega bajo un
 * subpath con un `base` real (p. ej. `/oxidav/`), se conserva.
 */
const APP_BASE_URL =
  import.meta.env.BASE_URL && import.meta.env.BASE_URL !== './' ? import.meta.env.BASE_URL : '/'

/**
 * El `end_session_url` del proveedor solo se sigue si es una URL absoluta
 * `https:` con host explícito y **sin credenciales embebidas**. Cualquier otro
 * valor (`http:`, `javascript:`, `data:`, una URL relativa o *protocol-relative*
 * `//host/...`, o la cadena vacía) se descarta para evitar un *open redirect* y
 * se cae a {@link APP_BASE_URL}.
 *
 * La comprobación se hace sobre la URL **parseada**, nunca sobre la cadena
 * literal: un valor como `https://legit-idp.example@evil.example/` tiene
 * protocolo `https:` pero su host real es `evil.example`, porque lo que precede
 * al `@` es *userinfo*, no el host. Como a este redirect se le adjunta el
 * `id_token_hint`, aceptarlo entregaría el token de cierre de sesión a un host
 * ajeno. Por eso se exige además `hostname` no vacío (descarta `https://`) y
 * `username`/`password` vacíos (descarta el userinfo malicioso).
 */
function isSafeEndSessionUrl(url: string | null | undefined): url is string {
  if (!url) return false
  try {
    const parsed = new URL(url)
    return (
      parsed.protocol === 'https:' &&
      parsed.hostname.length > 0 &&
      parsed.username === '' &&
      parsed.password === ''
    )
  } catch {
    return false
  }
}

export interface AuthProviderProps {
  children: ReactNode
}

/**
 * Resuelve la sesión OIDC al montar, expone el usuario y reacciona a los 401
 * globales emitidos por el cliente HTTP.
 *
 * Mientras `me` está en curso `loading` es `true`; ante un 401 (o cualquier
 * fallo de red) el usuario queda como no autenticado.
 */
export function AuthProvider({ children }: AuthProviderProps) {
  const [user, setUser] = useState<AuthUser | null>(null)
  const [loading, setLoading] = useState(true)
  const [loggingOut, setLoggingOut] = useState(false)
  const meRequestRef = useRef<Promise<AuthUser> | null>(null)

  useEffect(() => {
    // La petición se cachea para que el doble montaje de StrictMode no lance dos
    // `GET /api/auth/me`: ambos efectos se suscriben a la misma promesa.
    // A propósito NO se reintenta: un fallo aquí deja al usuario sin sesión y el
    // login es una navegación completa a `/api/auth/login` que remonta todo el
    // árbol (nuevo `AuthProvider`), así que la promesa cacheada nunca queda
    // obsoleta entre reintentos.
    if (meRequestRef.current === null) {
      meRequestRef.current = api.getMe()
    }

    let active = true
    meRequestRef.current
      .then((me) => {
        if (active) setUser(me)
      })
      .catch(() => {
        if (active) setUser(null)
      })
      .finally(() => {
        if (active) setLoading(false)
      })

    return () => {
      active = false
    }
  }, [])

  useEffect(() => {
    const onUnauthorized = () => {
      setUser(null)
      setLoading(false)
    }
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized)
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized)
  }, [])

  const logout = useCallback(async () => {
    setLoggingOut(true)
    let endSessionUrl: string | null
    try {
      const result = await api.logout()
      endSessionUrl = result.end_session_url
    } catch {
      // Si el cierre remoto falla, al menos liberamos la sesión local.
      endSessionUrl = null
    } finally {
      setLoggingOut(false)
    }

    setUser(null)
    window.location.assign(isSafeEndSessionUrl(endSessionUrl) ? endSessionUrl : APP_BASE_URL)
  }, [])

  const value = useMemo<AuthContextValue>(
    () => ({ user, loading, loggingOut, logout }),
    [user, loading, loggingOut, logout],
  )

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>
}
