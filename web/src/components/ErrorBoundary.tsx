/**
 * Frontera de error del bundle inicial (F17), sin `antd`.
 *
 * `React.lazy` rechaza su promesa cuando falla la descarga del chunk diferido
 * de la app autenticada (p. ej. un 404 tras un despliegue con hashes nuevos).
 * Sin una frontera, ese rechazo desmonta el árbol y deja la pantalla en blanco.
 * Aquí se captura y se ofrece una acción de recarga para recuperar la app.
 *
 * Es el único componente de clase del proyecto: React solo expone la captura de
 * errores de renderizado (`getDerivedStateFromError`) a las clases.
 */

import { Component, type ErrorInfo, type ReactNode } from 'react'

export interface ErrorBoundaryProps {
  children: ReactNode
}

interface ErrorBoundaryState {
  hasError: boolean
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { hasError: false }

  static getDerivedStateFromError(): ErrorBoundaryState {
    return { hasError: true }
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    // Sin antd ni telemetría en el bundle inicial: se registra en consola para
    // no perder el error.
    console.error('ErrorBoundary capturó un error:', error, info)
  }

  render(): ReactNode {
    if (!this.state.hasError) {
      return this.props.children
    }

    return (
      <div
        role="alert"
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 16,
          minHeight: '100vh',
          padding: 24,
          textAlign: 'center',
        }}
      >
        <p style={{ margin: 0 }}>Algo ha ido mal al cargar la aplicación.</p>
        <button type="button" onClick={() => window.location.reload()}>
          Recargar
        </button>
      </div>
    )
  }
}
