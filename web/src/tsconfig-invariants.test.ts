/**
 * Invariantes de la separación de proyectos de TypeScript (F21).
 *
 * F20 tuvo que añadir los tipos de `node` a `tsconfig.app.json` para un único
 * fichero de pruebas, de modo que el código de aplicación veía `process`, `fs`,
 * `path` y los globales de Vitest sin necesitarlos. F21 saca esas dos cosas del
 * proyecto de la aplicación y las pone en un proyecto propio de tests
 * (`tsconfig.test.json`).
 *
 * Estas pruebas fijan la frontera: la app no ve Node ni los globales de test,
 * el proyecto de test es el dueño de ambos, y ningún fichero de test se cuela
 * en el proyecto de app (ni al revés). Leen las configuraciones con el propio
 * parser de TypeScript —`ts.readConfigFile` + `ts.parseJsonConfigFileContent`—
 * porque los `tsconfig` llevan comentarios JSONC y no son `JSON.parse`-ables.
 */

import { existsSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import * as ts from 'typescript'
import { beforeAll, describe, expect, it } from 'vitest'

// Vitest runs with the package root as cwd (`npm run test` from `web/`), the
// same assumption `design-tokens.test.ts` makes. Everything is resolved from
// there so the suite does not depend on `import.meta.url` (not a `file:` URL
// under jsdom).
const PROJECT_ROOT = process.cwd()

const APP_CONFIG = 'tsconfig.app.json'
const TEST_CONFIG = 'tsconfig.test.json'
const SOLUTION_CONFIG = 'tsconfig.json'

const MAIN_ENTRY = toPosix(join(PROJECT_ROOT, 'src', 'main.tsx'))
const APP_ENTRY = toPosix(join(PROJECT_ROOT, 'src', 'App.tsx'))
const TEST_SETUP = toPosix(join(PROJECT_ROOT, 'src', 'test', 'setup.ts'))
const TEST_DIR = toPosix(join(PROJECT_ROOT, 'src', 'test'))

/** Normalize separators so comparisons are OS-independent. */
function toPosix(path: string): string {
  return path.replaceAll('\\', '/')
}

/** The two things this feature cares about from a parsed project. */
interface ProjectConfig {
  types: string[]
  fileNames: string[]
}

/**
 * Parsed configs, keyed by file name: the helper is called a dozen times across
 * the suite and each `tsconfig` must be read and parsed exactly once.
 */
const configCache = new Map<string, ProjectConfig | null>()

/**
 * Parse a `tsconfig` with the TypeScript parser (JSONC-aware, resolves
 * `extends`). Returns `null` when the file does not exist yet, so callers
 * assert on a readable message instead of blowing up with `undefined`. Throws
 * when the file is readable but the parser reports diagnostics (a broken
 * `extends`, an unreadable referenced file, no inputs…), so those failures
 * cannot slip through as "parseable".
 */
function readProjectConfig(fileName: string): ProjectConfig | null {
  if (configCache.has(fileName)) return configCache.get(fileName) ?? null

  const configPath = join(PROJECT_ROOT, fileName)
  const read = ts.readConfigFile(configPath, ts.sys.readFile)
  if (read.error !== undefined || read.config === undefined) {
    configCache.set(fileName, null)
    return null
  }

  const parsed = ts.parseJsonConfigFileContent(read.config, ts.sys, PROJECT_ROOT)
  if (parsed.errors.length > 0) {
    const details = parsed.errors
      .map((error) => ts.flattenDiagnosticMessageText(error.messageText, ' '))
      .join('; ')
    throw new Error(`${fileName} must parse without errors: ${details}`)
  }

  const config: ProjectConfig = {
    types: parsed.options.types ?? [],
    fileNames: parsed.fileNames.map(toPosix),
  }
  configCache.set(fileName, config)
  return config
}

/** Every `*.test.ts` / `*.test.tsx` file that really exists under `src/`. */
function testFilesOnDisk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) return testFilesOnDisk(full)
    return /\.test\.tsx?$/.test(entry) ? [toPosix(full)] : []
  })
}

/** True when a compiled file name belongs to the test surface, not to the app. */
function isTestFile(fileName: string): boolean {
  return /\.test\.tsx?$/.test(fileName) || fileName.startsWith(`${TEST_DIR}/`)
}

describe('tsconfig project invariants (F21)', () => {
  beforeAll(() => {
    expect(
      existsSync(join(PROJECT_ROOT, SOLUTION_CONFIG)),
      `expected to run from web/: ${SOLUTION_CONFIG} not found under ${PROJECT_ROOT}`,
    ).toBe(true)
  })

  it('I1: the app project does not see Node types', () => {
    const app = readProjectConfig(APP_CONFIG)
    expect(app, `${APP_CONFIG} must exist and be parseable`).not.toBeNull()
    expect(app?.types).not.toContain('node')
  })

  it('I2: the app project does not see the test globals', () => {
    const app = readProjectConfig(APP_CONFIG)
    expect(app, `${APP_CONFIG} must exist and be parseable`).not.toBeNull()
    expect(app?.types).not.toContain('vitest/globals')
  })

  it('I3: the app project keeps vite/client', () => {
    const app = readProjectConfig(APP_CONFIG)
    expect(app, `${APP_CONFIG} must exist and be parseable`).not.toBeNull()
    expect(app?.types).toContain('vite/client')
  })

  it('I4: app and test file sets do not overlap', () => {
    const app = readProjectConfig(APP_CONFIG)
    expect(app, `${APP_CONFIG} must exist and be parseable`).not.toBeNull()

    const leaked = app?.fileNames.filter(isTestFile) ?? []
    expect(leaked, `${APP_CONFIG} must not include test files`).toEqual([])

    const test = readProjectConfig(TEST_CONFIG)
    expect(test, `${TEST_CONFIG} must exist and be parseable`).not.toBeNull()

    const missing = testFilesOnDisk(join(PROJECT_ROOT, 'src')).filter(
      (file) => !(test?.fileNames ?? []).includes(file),
    )
    expect(missing, `${TEST_CONFIG} must own every test file under src/`).toEqual([])
  })

  it('I5: the test project owns its setup file', () => {
    const test = readProjectConfig(TEST_CONFIG)
    expect(test, `${TEST_CONFIG} must exist and be parseable`).not.toBeNull()
    expect(test?.fileNames).toContain(TEST_SETUP)
  })

  it('I6: the app project still compiles the application entry points', () => {
    const app = readProjectConfig(APP_CONFIG)
    expect(app, `${APP_CONFIG} must exist and be parseable`).not.toBeNull()
    expect(app?.fileNames).toContain(MAIN_ENTRY)
    expect(app?.fileNames).toContain(APP_ENTRY)
  })

  it('I7: the solution config references the test project', () => {
    const read = ts.readConfigFile(join(PROJECT_ROOT, SOLUTION_CONFIG), ts.sys.readFile)
    expect(read.error, `${SOLUTION_CONFIG} must be parseable`).toBeUndefined()
    const references: Array<{ path: string }> = read.config?.references ?? []
    expect(references.map((reference) => reference.path)).toContain(`./${TEST_CONFIG}`)
  })

  it('I8: the test project owns Node types and the test globals', () => {
    const test = readProjectConfig(TEST_CONFIG)
    expect(test, `${TEST_CONFIG} must exist and be parseable`).not.toBeNull()
    expect(test?.types).toContain('node')
    expect(test?.types).toContain('vitest/globals')
  })
})
