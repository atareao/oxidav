/**
 * Login entry point (F14).
 *
 * Reached by {@link RequireAuth} when there is no session. It immediately kicks
 * off the PKCE redirect to Pocket ID via {@link login}; in dev mode `login()`
 * is a no-op, but the guard never sends users here in that mode.
 */

import { App, Result, Spin } from 'antd'
import { useEffect } from 'react'
import { useLocation } from 'react-router-dom'
import { login } from '../services/auth'
import { toErrorMessage } from '../services/api'

export function Login() {
  const { message } = App.useApp()
  const location = useLocation()

  useEffect(() => {
    // Recover the route the guard wanted before bouncing us here (carried in
    // Router state), so the callback can return the user to it.
    const from = (location.state as { from?: unknown } | null)?.from
    // Surface discovery / redirect failures instead of leaving an unhandled
    // rejection and a spinner that never resolves.
    login(typeof from === 'string' ? from : undefined).catch((err: unknown) => {
      message.error(toErrorMessage(err))
    })
  }, [message, location.state])

  return (
    <Result
      icon={<Spin size="large" />}
      title="Iniciando sesión…"
      subTitle="Redirigiendo al proveedor de identidad (Pocket ID)."
    />
  )
}
