import { test, expect } from '@playwright/test'

// Deployment gate (phase 1, walletless). Catches cross-layer wiring drift:
// the page must render real data AND the rooms feed must actually flow.

test('markets page renders without console errors', async ({ page }) => {
  const errors: string[] = []
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text())
  })
  await page.goto('/markets')
  await expect(page).toHaveTitle(/torch/i)
  // Hydration settled; either market cards or an honest empty state.
  await page.waitForLoadState('networkidle')
  expect(errors, `console errors:\n${errors.join('\n')}`).toHaveLength(0)
})

test('feed connects and joins the all room', async ({ page }) => {
  const sent: string[] = []
  page.on('websocket', (ws) => {
    if (!ws.url().includes('/events')) return
    ws.on('framesent', (f) => sent.push(String(f.payload)))
  })
  await page.goto('/markets')
  await page.waitForTimeout(3000)
  expect(
    sent.some((p) => p.includes('"subscribe":"all"')),
    `frames sent: ${sent.join(' | ')}`,
  ).toBeTruthy()
})

test('market detail page subscribes to its room', async ({ page }) => {
  await page.goto('/markets')
  await page.waitForLoadState('networkidle')
  const firstCard = page.locator('a[href*="/markets/"]').first()
  const count = await firstCard.count()
  test.skip(count === 0, 'no markets yet (fresh world) — rerun after first market')

  const sent: string[] = []
  page.on('websocket', (ws) => {
    if (!ws.url().includes('/events')) return
    ws.on('framesent', (f) => sent.push(String(f.payload)))
  })
  await firstCard.click()
  await page.waitForTimeout(3000)
  expect(sent.some((p) => p.includes('"market":'))).toBeTruthy()
})

test('api surface answers through the public edge', async ({ request, baseURL }) => {
  // When gating the deployed preview, hit the real api host; locally, skip.
  test.skip(!baseURL?.includes('torchmarket.dev'), 'public-edge check only')
  const api = 'https://api.torchmarket.dev'
  for (const path of ['/health', '/api/markets?limit=1']) {
    const res = await request.get(api + path)
    expect(res.status(), path).toBe(200)
  }
  // /metrics must NOT be publicly reachable (edge allowlist).
  const metrics = await request.get(api + '/metrics', { maxRedirects: 0 })
  expect(metrics.status(), '/metrics hidden at edge').not.toBe(200)
})
