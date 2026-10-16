import type { CSSProperties } from 'react'

/**
 * Shared page-level layout tokens (F19 · paso 18).
 *
 * Every page stacks its top-level blocks with the same vertical rhythm and
 * sizes its toolbar controls with the same fluid width, so the four views are
 * visually consistent and none of them overflows a narrow (320 px) viewport
 * while staying comfortable on desktop.
 */

/** Vertical gap (16 px) between the top-level blocks of a page. */
export const PAGE_GAP = 'middle'

/**
 * Fluid width for toolbar controls (collection selectors, search boxes):
 * 150 px on a phone, up to 240 px on a desktop, never wider than half the
 * viewport — replaces the previous hard-coded `minWidth`/`width` values.
 */
export const fluidControlStyle: CSSProperties = {
  width: 'clamp(150px, 50vw, 240px)',
}
