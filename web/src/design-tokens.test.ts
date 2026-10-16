/**
 * Invariantes del sistema de diseño (F20).
 *
 * F19 dejó en `web/src/index.css` una escala de 14 variables CSS de las que 10
 * no las consumía nadie. Estas pruebas fijan invariantes para que esa deuda no
 * vuelva: ninguna custom property sin consumidor y una sola fuente para la pila
 * de fuentes.
 *
 * Leen los fuentes con `node:fs`, por eso `tsconfig.app.json` declara los tipos
 * de `node`: el único código que los necesita son estas pruebas.
 */

import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

// Vitest runs with the package root as cwd (`npm run test` from `web/`).
// `import.meta.url` is not a `file:` URL under the jsdom environment, so we
// resolve the source directory from the working directory instead.
const SRC_DIR = join(process.cwd(), 'src')
const INDEX_CSS = join(SRC_DIR, 'index.css')
const AUTHENTICATED_APP = join(SRC_DIR, 'AuthenticatedApp.tsx')
const FONT_STACK_MARKER = 'system-ui'

/** Production `.ts`/`.tsx`/`.css` files under a directory (test files excluded). */
function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) return sourceFiles(full)
    if (/\.(ts|tsx|css)$/.test(entry) && !/\.test\.(ts|tsx)$/.test(entry)) return [full]
    return []
  })
}

/**
 * Drop comments so a *commented* usage never counts as a consumer. Without
 * this, a `var(--x)` mentioned in a comment would mask the removal of the real
 * usage.
 */
function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')
}

/** Distinct custom properties declared in a stylesheet (comments stripped). */
function declaredCustomProperties(stylesheet: string): string[] {
  const names = [...stripComments(stylesheet).matchAll(/(?:^|[\s;{])(--[a-z0-9-]+)\s*:/g)].map(
    (m) => m[1],
  )
  return [...new Set(names)]
}

/** True when `source` really uses `var(<name>)`, tolerating fallbacks/spaces. */
function usesCustomProperty(source: string, name: string): boolean {
  return new RegExp(`var\\(\\s*${name}\\s*[,)]`).test(source)
}

describe('design token invariants (F20)', () => {
  const css = readFileSync(INDEX_CSS, 'utf8')
  const declared = declaredCustomProperties(css)

  it('declares no CSS custom property without a consumer', () => {
    const consumerSource = sourceFiles(SRC_DIR)
      .map((file) => stripComments(readFileSync(file, 'utf8')))
      .join('\n')
    const orphans = declared.filter((name) => !usesCustomProperty(consumerSource, name))
    expect(orphans).toEqual([])
  })

  it('keeps the font stack in a single file', () => {
    expect(css).toContain(FONT_STACK_MARKER)
    expect(readFileSync(AUTHENTICATED_APP, 'utf8')).not.toContain(FONT_STACK_MARKER)
  })

  it('has no phantom spacing/radius/mono variables', () => {
    expect(declared).not.toContain('--space-1')
    expect(declared).not.toContain('--space-6')
    expect(declared).not.toContain('--radius-md')
    expect(declared).not.toContain('--font-mono')
  })

  it('ends up with exactly the two surviving variables', () => {
    expect([...declared].sort()).toEqual(['--font-sans', '--line-height-base'].sort())
  })
})
