/**
 * Indicador de carga nativo (sin `antd`) del bundle inicial (F17).
 *
 * Se muestra mientras se resuelve `GET /api/auth/me` y como `fallback` del
 * `Suspense` que carga la app autenticada en diferido. Debe evitar `antd` para
 * no engordar el chunk inicial.
 */

export function AppLoader() {
  return (
    <div
      data-testid="auth-loading"
      role="status"
      aria-label="Cargando…"
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        minHeight: '100vh',
      }}
    >
      <span>Cargando…</span>
    </div>
  )
}
