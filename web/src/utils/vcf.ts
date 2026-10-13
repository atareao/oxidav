/**
 * Client-side vCard helpers for the Contacts view (F12).
 *
 * Export is generated locally from the contact DTOs (no server round-trip); the
 * backend remains the source of truth for the stored `raw_payload`, but the SPA
 * needs to offer a portable `.vcf` download of the current listing. Import reads
 * a `.vcf` file and yields field tuples that the page turns into `POST` calls.
 *
 * Scope is deliberately small (vCard 3.0, the fields the DTO exposes: `FN`,
 * `EMAIL`, `TEL`, `ORG`, `UID`): it is a convenience interchange format, not a
 * full RFC 6350 parser.
 */

import type { Contact, CreateContactInput } from '../types/api'

const CRLF = '\r\n'

/**
 * Maximum number of octets per physical vCard line, excluding the line break
 * (RFC 6350 §3.2). A continuation line is prefixed with a single space, which
 * counts towards its own budget.
 */
const MAX_LINE_OCTETS = 75

/** Largest `.vcf` accepted by the import flow (defence against memory abuse). */
export const MAX_FILE_BYTES = 5 * 1024 * 1024

/** UTF-8 encoded length, in octets, of a single Unicode code point. */
function utf8ByteLength(char: string): number {
  const code = char.codePointAt(0) ?? 0
  if (code <= 0x7f) {
    return 1
  }
  if (code <= 0x7ff) {
    return 2
  }
  if (code <= 0xffff) {
    return 3
  }
  return 4
}

/**
 * Fold a logical vCard line into physical lines of at most
 * {@link MAX_LINE_OCTETS} octets, joined by `CRLF` plus a single leading space
 * on every continuation line (RFC 6350 §3.2). The split is byte-aware so
 * multi-byte UTF-8 sequences are never cut in half.
 */
function foldLine(line: string): string {
  const physical: string[] = []
  let current = ''
  let octets = 0

  for (const char of line) {
    const charOctets = utf8ByteLength(char)
    if (octets + charOctets > MAX_LINE_OCTETS) {
      physical.push(current)
      // Continuation line starts with one space, which consumes an octet.
      current = ' '
      octets = 1
    }
    current += char
    octets += charOctets
  }
  physical.push(current)
  return physical.join(CRLF)
}

/** The subset of a contact used when composing a vCard. */
export type VCardSource = Pick<
  Contact,
  'uid' | 'fn' | 'full_name' | 'email' | 'phone' | 'organization'
>

/** A parsed contact, ready to be sent as {@link CreateContactInput}. */
export interface ParsedVCard {
  fn: string
  email: string | null
  phone: string | null
  organization: string | null
  uid: string | null
}

/** Escape a value for a vCard property (RFC 6350 §3.4). */
function escapeValue(value: string): string {
  return value
    .replace(/\\/g, '\\\\')
    .replace(/\r\n|\r|\n/g, '\\n')
    .replace(/,/g, '\\,')
    .replace(/;/g, '\\;')
}

/**
 * Reverse {@link escapeValue} in a **single pass**.
 *
 * A chained implementation is order-dependent and corrupts any value holding a
 * literal backslash followed by an escaped character (`C:\new` exports as
 * `C:\\new` and would unfold back to a newline). Matching the whole escape
 * (`\` + one of `\ n N , ;`) at once keeps the round-trip lossless.
 */
function unescapeValue(value: string): string {
  return value.replace(/\\([\\nN,;])/g, (_match, escaped: string) =>
    escaped === 'n' || escaped === 'N' ? '\n' : escaped,
  )
}

/**
 * Serialise contacts to a single vCard 3.0 document (CRLF-separated).
 *
 * Returns `''` for an empty list so callers can detect "nothing to export".
 */
export function serializeContactsToVcf(contacts: readonly VCardSource[]): string {
  return contacts
    .map((contact) => {
      const lines: string[] = [
        'BEGIN:VCARD',
        'VERSION:3.0',
        // UID is escaped too (defence in depth): a `uid` carrying a comma,
        // semicolon, newline or backslash must not inject extra content.
        `UID:${escapeValue(contact.uid)}`,
      ]
      const name = contact.fn ?? contact.full_name ?? ''
      if (name.trim().length > 0) {
        lines.push(`FN:${escapeValue(name)}`)
      }
      if (contact.email !== null && contact.email !== '') {
        lines.push(`EMAIL:${escapeValue(contact.email)}`)
      }
      if (contact.phone !== null && contact.phone !== '') {
        lines.push(`TEL:${escapeValue(contact.phone)}`)
      }
      if (contact.organization !== null && contact.organization !== '') {
        lines.push(`ORG:${escapeValue(contact.organization)}`)
      }
      lines.push('END:VCARD')
      return lines.map(foldLine).join(CRLF)
    })
    .join(CRLF)
}

/**
 * Parse a vCard document into field tuples.
 *
 * Unfolds continuation lines (a CRLF/LF followed by a space or tab) and keeps
 * the first value seen for multi-valued properties (`EMAIL`, `TEL`).
 */
export function parseVcf(text: string): ParsedVCard[] {
  const unfolded = text.replace(/\r\n[ \t]/g, '').replace(/\n[ \t]/g, '')
  const cards: ParsedVCard[] = []
  let current: ParsedVCard | null = null

  for (const rawLine of unfolded.split(/\r\n|\n|\r/)) {
    const line = rawLine.trim()
    if (line.length === 0) {
      continue
    }
    const upper = line.toUpperCase()
    if (upper === 'BEGIN:VCARD') {
      current = { fn: '', email: null, phone: null, organization: null, uid: null }
      continue
    }
    if (upper === 'END:VCARD') {
      if (current !== null && current.fn.trim().length > 0) {
        cards.push(current)
      }
      current = null
      continue
    }
    if (current === null) {
      continue
    }

    const separator = line.indexOf(':')
    if (separator === -1) {
      continue
    }
    const property = line.slice(0, separator).split(';')[0]?.toUpperCase() ?? ''
    const value = unescapeValue(line.slice(separator + 1))

    switch (property) {
      case 'FN':
        current.fn = value
        break
      case 'EMAIL':
        current.email ??= value
        break
      case 'TEL':
        current.phone ??= value
        break
      case 'ORG':
        current.organization ??= value
        break
      case 'UID':
        current.uid = value
        break
      default:
        break
    }
  }

  return cards
}

/** Convert a parsed card into the API create payload (blank fields omitted). */
export function vcardToCreateInput(card: ParsedVCard): CreateContactInput {
  const input: CreateContactInput = { fn: card.fn.trim() }
  if (card.email !== null && card.email !== '') {
    input.email = card.email
  }
  if (card.phone !== null && card.phone !== '') {
    input.phone = card.phone
  }
  if (card.organization !== null && card.organization !== '') {
    input.organization = card.organization
  }
  if (card.uid !== null && card.uid !== '') {
    input.uid = card.uid
  }
  return input
}

/** Trigger a client-side text download (e.g. the exported `.vcf`). */
export function downloadTextFile(
  filename: string,
  content: string,
  mime = 'text/vcard;charset=utf-8',
): void {
  // `URL.createObjectURL` is absent in some minimal environments (jsdom).
  if (typeof URL.createObjectURL !== 'function') {
    return
  }
  const blob = new Blob([content], { type: mime })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = filename
  document.body.appendChild(link)
  link.click()
  link.remove()
  URL.revokeObjectURL(url)
}
