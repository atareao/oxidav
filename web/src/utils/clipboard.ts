/**
 * Best-effort clipboard copy.
 *
 * Prefers the async Clipboard API (`navigator.clipboard.writeText`), which is
 * unavailable in insecure contexts and in jsdom. Falls back to a hidden,
 * off-screen `<textarea>` + `document.execCommand('copy')`, and finally reports
 * failure (rather than throwing) so callers can show a friendly message.
 *
 * Resolves to `true` when the text was copied, `false` otherwise.
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText !== undefined) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    // Permission denied / insecure context: fall through to the legacy path.
  }

  try {
    const textarea = document.createElement('textarea')
    textarea.value = text
    textarea.setAttribute('readonly', '')
    textarea.style.position = 'fixed'
    textarea.style.top = '-9999px'
    textarea.style.opacity = '0'
    document.body.appendChild(textarea)
    textarea.select()
    const ok = document.execCommand('copy')
    document.body.removeChild(textarea)
    return ok
  } catch {
    return false
  }
}
