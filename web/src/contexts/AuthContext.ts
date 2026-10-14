/**
 * Contexto de autenticación de la SPA (F17).
 *
 * Espejo del de `valet/frontend` (paridad de diseño): la sesión vive en una
 * cookie `HttpOnly` que gestiona el backend, así que el cliente **nunca** maneja
 * tokens. Solo expone la identidad resuelta por `GET /api/auth/me` y la
 * operación de cierre de sesión.
 */

import { createContext, useContext } from 'react'

/** Identidad resuelta desde `GET /api/auth/me`. */
export interface AuthUser {
  sub: string
  /** El backend serializa `null` cuando el proveedor no aporta el claim. */
  email?: string | null
  /** El backend serializa `null` cuando el proveedor no aporta el claim. */
  name?: string | null
}

export interface AuthContextValue {
  /** Usuario autenticado o `null` cuando no hay sesión válida. */
  user: AuthUser | null
  /** `true` mientras se resuelve `GET /api/auth/me` al montar. */
  loading: boolean
  /** `true` mientras se está cerrando la sesión (`POST /auth/logout`). */
  loggingOut: boolean
  /** Cierra la sesión local (y la SSO) y navega al proveedor. */
  logout: () => Promise<void>
}

export const AuthContext = createContext<AuthContextValue | null>(null)

export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext)
  if (context === null) {
    throw new Error('useAuth debe usarse dentro de un <AuthProvider>')
  }
  return context
}
