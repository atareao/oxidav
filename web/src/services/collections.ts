/**
 * Typed access to the collections REST endpoints (`/api/v1/collections`).
 */

import type { Collection, CollectionType, Page } from '../types/api'
import { api } from './api'

/** Request body for creating a collection. */
export interface CreateCollectionInput {
  name: string
  type: CollectionType
  color?: string
}

/**
 * Request body for a partial update of a collection (`PATCH`).
 *
 * Only `name` and `color` are mutable: the backend rejects any attempt to
 * change `type`, so it is deliberately absent from this input.
 */
export interface UpdateCollectionInput {
  name?: string
  color?: string
}

/**
 * List every collection owned by the caller.
 *
 * The backend paginates (`Page<CollectionDto>`); the SPA uses the default page
 * and simply unwraps `items`.
 */
export async function listCollections(signal?: AbortSignal): Promise<Collection[]> {
  const page = await api.get<Page<Collection>>('/v1/collections', signal)
  return page.items
}

/** List only the caller's `calendar` collections (address books excluded). */
export async function listCalendarCollections(signal?: AbortSignal): Promise<Collection[]> {
  const items = await listCollections(signal)
  return items.filter((collection) => collection.type === 'calendar')
}

/** Create a collection. */
export function createCollection(
  input: CreateCollectionInput,
  signal?: AbortSignal,
): Promise<Collection> {
  return api.post<Collection>('/v1/collections', input, signal)
}

/**
 * Partially update a collection (rename and/or recolour).
 *
 * The backend returns the updated {@link Collection}; the immutable `type` is
 * never sent (it would be rejected with a `400`).
 */
export function updateCollection(
  id: string,
  input: UpdateCollectionInput,
  signal?: AbortSignal,
): Promise<Collection> {
  return api.patch<Collection>(`/v1/collections/${encodeURIComponent(id)}`, input, signal)
}

/** Delete a collection and (server-side) cascade its objects. */
export function deleteCollection(id: string, signal?: AbortSignal): Promise<void> {
  return api.delete(`/v1/collections/${encodeURIComponent(id)}`, signal)
}
