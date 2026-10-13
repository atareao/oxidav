/**
 * Small date helpers for the calendar grid.
 *
 * Everything works in the browser's **local** timezone (no UTC juggling): an
 * event's `dtstart` (epoch ms) maps to the local day cell it belongs to.
 * Implemented with native `Date` + `Intl`, so no date library is needed beyond
 * the `dayjs` already pulled in by Ant Design's `DatePicker`.
 */

const dayFormatter = new Intl.DateTimeFormat('es-ES', {
  weekday: 'long',
  day: 'numeric',
})

const monthFormatter = new Intl.DateTimeFormat('es-ES', {
  month: 'long',
  year: 'numeric',
})

const dateTimeFormatter = new Intl.DateTimeFormat('es-ES', {
  day: '2-digit',
  month: '2-digit',
  year: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
})

/** Stable `YYYY-MM-DD` key for a date in local time. */
export function dateKey(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

/** The first instant of the given month, in local time. */
export function startOfMonth(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), 1)
}

/** Add `months` to a date, clamped to the first of the resulting month. */
export function addMonths(date: Date, months: number): Date {
  return new Date(date.getFullYear(), date.getMonth() + months, 1)
}

/** Epoch ms of the first instant of the month containing `date`. */
export function startOfMonthMs(date: Date): number {
  return startOfMonth(date).getTime()
}

/** Epoch ms of the first instant of the following month. */
export function endOfMonthMs(date: Date): number {
  return addMonths(date, 1).getTime()
}

/**
 * A fixed 6×7 month matrix (Monday-first), starting on the Monday on or before
 * the 1st of `month` and covering 42 consecutive local days.
 */
export function buildMonthGrid(month: Date): Date[] {
  const first = startOfMonth(month)
  const mondayOffset = (first.getDay() + 6) % 7 // Sunday(0) → 6, Monday(1) → 0
  const start = new Date(first.getFullYear(), first.getMonth(), 1 - mondayOffset)
  return Array.from({ length: 42 }, (_, index) => {
    return new Date(start.getFullYear(), start.getMonth(), start.getDate() + index)
  })
}

/** Whether two dates fall in the same month and year. */
export function isSameMonth(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth()
}

/** Human label for a month, e.g. "octubre de 2026". */
export function formatMonthLabel(date: Date): string {
  return monthFormatter.format(date)
}

/** Weekday label for a grid column, e.g. "lun 5". */
export function formatDayLabel(date: Date): string {
  return dayFormatter.format(date)
}

/** Human label for an instant, e.g. "05/10/2026, 09:00". */
export function formatDateTime(ms: number): string {
  return dateTimeFormatter.format(new Date(ms))
}
