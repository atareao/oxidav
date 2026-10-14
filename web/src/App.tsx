/**
 * Raíz de la SPA (F17).
 *
 * `App` monta el proveedor de sesión y el guard `AuthGate`. Deliberadamente no
 * importa `antd` ni el router: la app autenticada (antd, router y vistas) se
 * carga en diferido (`React.lazy`) solo cuando hay sesión, para mantener mínimo
 * el bundle inicial.
 */

import { lazy, Suspense } from 'react'
import { AppLoader } from './components/AppLoader'
import { ErrorBoundary } from './components/ErrorBoundary'
import { LoginPage } from './components/LoginPage'
import { AuthProvider } from './contexts/AuthProvider'
import { useAuth } from './contexts/AuthContext'

const AuthenticatedApp = lazy(() => import('./AuthenticatedApp'))

/**
 * Guard de sesión: mientras se resuelve `/api/auth/me` muestra carga; sin
 * sesión, la pantalla de login; con sesión, la aplicación autenticada.
 */
function AuthGate() {
  const { user, loading } = useAuth()

  if (loading) return <AppLoader />
  if (!user) return <LoginPage />

  return (
    // `ErrorBoundary` envuelve al `Suspense`: si falla la descarga del chunk de
    // la app autenticada (`React.lazy` rechaza), se muestra un aviso recuperable
    // en lugar de dejar la pantalla en blanco.
    <ErrorBoundary>
      <Suspense fallback={<AppLoader />}>
        <AuthenticatedApp />
      </Suspense>
    </ErrorBoundary>
  )
}

export default function App() {
  return (
    <AuthProvider>
      <AuthGate />
    </AuthProvider>
  )
}
