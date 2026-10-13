/**
 * Typed access to the F11 calendar-event REST endpoints
 * (`/api/v1/collections/{id}/events`).
 *
 * The client never parses or produces `.ics`: it sends/consumes plain JSON and
 * the backend composes the stored iCalendar payload.
 */

import type { CalendarEvent, CreateEventInput, UpdateEventInput } from '../types/api'
import { api } from './api'

/** Optional `[start, end)` window, in Unix epoch milliseconds. */
export interface EventRange {
  start: number
  end: number
}

function rangeQuery(range?: EventRange): string {
  if (range === undefined) {
    return ''
  }
  const params = new URLSearchParams({
    start: String(range.start),
    end: String(range.end),
  })
  return `?${params.toString()}`
}

/**
 * List a calendar collection's events, optionally within a time window.
 *
 * NOTE: the backend `GET .../events` handler currently ignores the `start` /
 * `end` query parameters and returns the whole collection — the SPA therefore
 * also filters client-side for display. The parameters are sent anyway so the
 * call sites are ready for server-side range filtering once it lands.
 */
export function listEvents(
  collectionId: string,
  range?: EventRange,
  signal?: AbortSignal,
): Promise<CalendarEvent[]> {
  return api.get<CalendarEvent[]>(
    `/v1/collections/${encodeURIComponent(collectionId)}/events${rangeQuery(range)}`,
    signal,
  )
}

/** Create an event in a calendar collection. */
export function createEvent(
  collectionId: string,
  input: CreateEventInput,
  signal?: AbortSignal,
): Promise<CalendarEvent> {
  return api.post<CalendarEvent>(
    `/v1/collections/${encodeURIComponent(collectionId)}/events`,
    input,
    signal,
  )
}

/** Partially update an event (addressed by its `uid`). */
export function updateEvent(
  collectionId: string,
  uid: string,
  input: UpdateEventInput,
  signal?: AbortSignal,
): Promise<CalendarEvent> {
  return api.patch<CalendarEvent>(
    `/v1/collections/${encodeURIComponent(collectionId)}/events/${encodeURIComponent(uid)}`,
    input,
    signal,
  )
}

/** Delete an event (addressed by its `uid`). */
export function deleteEvent(
  collectionId: string,
  uid: string,
  signal?: AbortSignal,
): Promise<void> {
  return api.delete(
    `/v1/collections/${encodeURIComponent(collectionId)}/events/${encodeURIComponent(uid)}`,
    signal,
  )
}
