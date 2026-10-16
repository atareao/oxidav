/**
 * Typed access to the F12 contact (card-object) REST endpoints
 * (`/api/v1/collections/{id}/contacts`).
 *
 * Mirrors {@link file://./events.ts}: the client never parses or produces
 * `.vcf` — it sends/consumes plain JSON and the backend composes the stored
 * vCard payload.
 */

import type {
  Collection,
  Contact,
  ContactDetail,
  CreateContactInput,
  Page,
  UpdateContactInput,
} from '../types/api'
import { api } from './api'
import { listCollections } from './collections'

/** Optional page window for the (paginated) contacts listing. */
export interface ContactPage {
  limit?: number
  offset?: number
}

function pageQuery(page: ContactPage): string {
  const params = new URLSearchParams()
  if (page.limit !== undefined) {
    params.set('limit', String(page.limit))
  }
  if (page.offset !== undefined) {
    params.set('offset', String(page.offset))
  }
  const query = params.toString()
  return query.length > 0 ? `?${query}` : ''
}

/**
 * List a collection's contacts.
 *
 * The listing honours `?limit=&offset=`. The backend may return either a bare
 * array (like `GET .../events`) or a `Page` envelope; both shapes are unwrapped
 * so the SPA stays compatible with either.
 */
export async function listContacts(
  collectionId: string,
  page: ContactPage = {},
  signal?: AbortSignal,
): Promise<Contact[]> {
  const result = await api.get<Contact[] | Page<Contact>>(
    `/v1/collections/${encodeURIComponent(collectionId)}/contacts${pageQuery(page)}`,
    signal,
  )
  return Array.isArray(result) ? result : result.items
}

/**
 * Page size used when draining a collection's contacts.
 *
 * Matches the backend hard ceiling `MAX_COLLECTION_LIMIT = 200`, so each
 * request fetches as much as the server will allow.
 */
export const CONTACTS_PAGE_SIZE = 200

/** List only the caller's `addressbook` collections (calendars excluded). */
export async function listAddressbookCollections(signal?: AbortSignal): Promise<Collection[]> {
  const items = await listCollections(signal)
  return items.filter((collection) => collection.type === 'addressbook')
}

/** Create a contact in an address book. */
export function createContact(
  collectionId: string,
  input: CreateContactInput,
  signal?: AbortSignal,
): Promise<Contact> {
  return api.post<Contact>(
    `/v1/collections/${encodeURIComponent(collectionId)}/contacts`,
    input,
    signal,
  )
}

/** Partially update a contact (addressed by its `uid`). */
export function updateContact(
  collectionId: string,
  uid: string,
  input: UpdateContactInput,
  signal?: AbortSignal,
): Promise<Contact> {
  return api.patch<Contact>(
    `/v1/collections/${encodeURIComponent(collectionId)}/contacts/${encodeURIComponent(uid)}`,
    input,
    signal,
  )
}

/** Delete a contact (addressed by its `uid`). */
export function deleteContact(
  collectionId: string,
  uid: string,
  signal?: AbortSignal,
): Promise<void> {
  return api.delete(
    `/v1/collections/${encodeURIComponent(collectionId)}/contacts/${encodeURIComponent(uid)}`,
    signal,
  )
}

/**
 * Fetch a contact's full detail (F22): every vCard group, already parsed.
 *
 * The listing endpoint keeps returning the light {@link Contact} DTO; only this
 * detail endpoint exposes photos, addresses, multi-valued `TEL`/`EMAIL`… The
 * client still never parses vCard itself.
 */
export function getContact(
  collectionId: string,
  uid: string,
  signal?: AbortSignal,
): Promise<ContactDetail> {
  return api.get<ContactDetail>(
    `/v1/collections/${encodeURIComponent(collectionId)}/contacts/${encodeURIComponent(uid)}`,
    signal,
  )
}

/**
 * Import a raw `.vcf` file into an address book (F22).
 *
 * The body carries the file's text **verbatim** as `text/vcard` (it may hold
 * several concatenated cards); the server splits and stores each card
 * byte-for-byte, so nothing is lost, unlike the lossy client-side parser. The
 * endpoint answers `201` with the created {@link Contact} DTOs.
 */
export async function importContacts(
  collectionId: string,
  vcardText: string,
  signal?: AbortSignal,
): Promise<Contact[]> {
  return api.postRaw<Contact[]>(
    `/v1/collections/${encodeURIComponent(collectionId)}/contacts/import`,
    vcardText,
    'text/vcard',
    signal,
  )
}
