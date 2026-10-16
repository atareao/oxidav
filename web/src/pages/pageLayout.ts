import type { CSSProperties } from 'react'

/**
 * Shared page-level layout helpers (F19, aclarado en F20).
 *
 * These are *page layout* concerns — vertical rhythm between top-level blocks
 * and fluid widths for toolbar controls — not design tokens. The design system
 * lives in Ant Design's `theme.token` (`web/src/AuthenticatedApp.tsx`); nothing
 * here duplicates a token from it. In particular `PAGE_GAP` is Ant Design's own
 * semantic gap name (`'middle'`), not a hand-rolled spacing value.
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
