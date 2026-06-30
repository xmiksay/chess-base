// Trigger a browser file download of text content (issue #120). Kept tiny and
// DOM-only so the stores stay free of browser APIs and remain unit-testable; the
// views call this after a store action returns the PGN text.

/** Download `text` as a file named `filename` via a transient object URL. */
export function downloadText(
  filename: string,
  text: string,
  type = 'application/x-chess-pgn',
): void {
  const url = URL.createObjectURL(new Blob([text], { type }))
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = filename
  document.body.appendChild(anchor)
  anchor.click()
  anchor.remove()
  URL.revokeObjectURL(url)
}
