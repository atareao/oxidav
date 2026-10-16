/**
 * Shared Suspense fallback for lazy-loaded routes.
 *
 * Used both at the top level (`App.tsx`, for the layout-less `/login` and
 * `/callback` routes) and inside `AppLayout` (so the header shell stays
 * visible while a protected page chunk loads). Announced to assistive tech via
 * `role="status"` + `aria-label`.
 */

import { Spin } from 'antd'

export function RouteFallback() {
  return (
    <div
      role="status"
      aria-label="Cargando…"
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        minHeight: '100vh',
      }}
    >
      <Spin size="large" />
    </div>
  )
}
