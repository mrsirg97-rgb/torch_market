// [prompt-008 F-2] External-URL scheme guard.
//
// Token social links (twitter / telegram / website) originate from
// attacker-controllable on-chain + indexed metadata and are rendered into
// `<a href>`. React does NOT neutralize `javascript:` / `data:` hrefs — it
// escapes text content, not attribute schemes — so an unguarded runtime href
// executes on click, which is stored XSS on a transaction-signing page. Allow
// ONLY http(s); reject everything else. Mirrors the https-only check already
// used for metadata URIs (hooks/useTokens.ts).

export function isSafeHttpUrl(raw: string | null | undefined): boolean {
  if (!raw) return false
  try {
    const u = new URL(raw)
    return u.protocol === 'http:' || u.protocol === 'https:'
  } catch {
    return false
  }
}

// The URL if it's a safe http(s) link, else null — so callers omit the link
// entirely rather than render an unsafe href.
export function safeExternalUrl(raw: string | null | undefined): string | null {
  return isSafeHttpUrl(raw) ? raw! : null
}
