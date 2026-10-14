/**
 * Pantalla mostrada cuando no hay sesión válida (F17).
 *
 * Inicia el flujo OIDC server-side navegando al endpoint `login` del backend
 * (`window.location.assign`, no `fetch`: es un redirect del navegador).
 *
 * Usa un `<button>` nativo y estilos en línea para que la pantalla de login no
 * arrastre `antd` al bundle inicial: la app autenticada (antd, router y vistas)
 * se carga en diferido desde `App.tsx`.
 */

import { API_BASE_URL } from '../services/api'

export function LoginPage() {
  const handleLogin = () => {
    // Se deriva de la misma base que usa el cliente HTTP (`API_BASE_URL`) en vez
    // de duplicar el literal `/api`: si se configurara una base externa
    // (`VITE_API_BASE_URL`), el botón apuntaría al origen equivocado.
    window.location.assign(`${API_BASE_URL}/auth/login`)
  }

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 24,
        minHeight: '100vh',
        padding: 24,
        textAlign: 'center',
      }}
    >
      <h1 style={{ margin: 0, fontSize: 40, fontWeight: 600 }}>OxiDav</h1>
      <p style={{ margin: 0, opacity: 0.75 }}>
        Inicia sesión para gestionar tus calendarios y contactos.
      </p>
      <button
        type="button"
        onClick={handleLogin}
        style={{
          padding: '10px 24px',
          font: 'inherit',
          fontSize: 16,
          fontWeight: 500,
          cursor: 'pointer',
        }}
      >
        Iniciar sesión
      </button>
    </div>
  )
}
