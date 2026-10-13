/**
 * Login entry point (F14).
 *
 * Reached by {@link RequireAuth} when there is no session. It immediately kicks
 * off the PKCE redirect to Pocket ID via {@link login}; in dev mode `login()`
 * is a no-op, but the guard never sends users here in that mode.
 */

import { App, Result, Spin } from 'antd'
import { useEffect } from 'react'
import { login } from '../services/auth'
import { toErrorMessage } from '../services/api'

export function Login() {
  const { message } = App.useApp()

  useEffect(() => {
    // Surface discovery / redirect failures instead of leaving an unhandled
    // rejection and a spinner that never resolves.
    login().catch((err: unknown) => {
      message.error(toErrorMessage(err))
    })
  }, [message])

  return (
    <Result
      icon={<Spin size="large" />}
      title="Iniciando sesión…"
      subTitle="Redirigiendo al proveedor de identidad (Pocket ID)."
    />
  )
}
