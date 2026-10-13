/**
 * OIDC redirect callback (F14).
 *
 * Mounted at `/callback`: exchanges the authorization `code` for tokens, then
 * leaves the OAuth query parameters behind and navigates back into the app.
 * Failures render a friendly message instead of raw details.
 */

import { Result, Spin } from 'antd'
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { handleRedirectCallback, consumeReturnTo } from '../services/auth'

export function Callback() {
  const navigate = useNavigate()
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let active = true
    handleRedirectCallback()
      .then(() => {
        if (active) {
          navigate(consumeReturnTo() ?? '/calendar', { replace: true })
        }
      })
      .catch((cause: unknown) => {
        if (active) {
          setError(cause instanceof Error ? cause.message : 'No se pudo completar el inicio de sesión')
        }
      })
    return () => {
      active = false
    }
  }, [navigate])

  if (error !== null) {
    return <Result status="error" title="No se pudo iniciar sesión" subTitle={error} />
  }

  return <Result icon={<Spin size="large" />} title="Completando el inicio de sesión…" />
}
