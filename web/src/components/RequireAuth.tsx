/**
 * Route guard for the SPA (F14).
 *
 * Wraps the protected routes and redirects to `/login` when there is no
 * session. In dev mode (`VITE_OIDC_AUTHORITY` unset) {@link isAuthenticated}
 * always returns `true`, so the F11–F13 views keep rendering unchanged.
 *
 * When the access token is expired but a `refresh_token` is still available the
 * guard first tries a silent {@link refresh} before falling back to a full
 * login round-trip. While that refresh is in flight the guard renders nothing,
 * so a valid session never flashes the login redirect.
 */

import { useEffect, useState } from 'react'
import type { ReactNode } from 'react'
import { Navigate, useLocation } from 'react-router-dom'
import { hasRefreshToken, isAuthenticated, refresh } from '../services/auth'

export interface RequireAuthProps {
  children: ReactNode
}

type AuthStatus = 'checking' | 'authenticated' | 'anonymous'

function initialStatus(): AuthStatus {
  if (isAuthenticated()) {
    return 'authenticated'
  }
  // Expired/none present: only worth a network round-trip if we can refresh.
  return hasRefreshToken() ? 'checking' : 'anonymous'
}

export function RequireAuth({ children }: RequireAuthProps) {
  const location = useLocation()
  const [status, setStatus] = useState<AuthStatus>(initialStatus)

  useEffect(() => {
    if (status !== 'checking') {
      return
    }
    let active = true
    refresh()
      .then(() => {
        if (active) {
          setStatus('authenticated')
        }
      })
      .catch(() => {
        if (active) {
          setStatus('anonymous')
        }
      })
    return () => {
      active = false
    }
  }, [status])

  if (status === 'authenticated') {
    return <>{children}</>
  }
  if (status === 'checking') {
    return null
  }

  return (
    <Navigate
      to="/login"
      replace
      state={{ from: location.pathname + location.search + location.hash }}
    />
  )
}
