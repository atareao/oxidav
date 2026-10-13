/**
 * Single source of truth for local-only identifiers used by optimistic rows
 * (records that exist in the UI but have not been persisted yet).
 *
 * A module-level counter guarantees uniqueness even when several ids are minted
 * within the same millisecond, so the value is safe to use as a React `key`.
 */

let counter = 0

/**
 * Mint a locally-unique, non-persisted identifier.
 *
 * @param prefix optional label to make ids self-describing in devtools.
 */
export function tempId(prefix = 'optimistic'): string {
  counter += 1
  return `${prefix}-${Date.now()}-${counter}`
}
