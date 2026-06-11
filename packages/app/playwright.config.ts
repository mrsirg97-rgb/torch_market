import { defineConfig } from '@playwright/test'

// Gate suite: same specs run against localhost AND the deployed preview —
//   BASE_URL=https://torchmarket.dev pnpm exec playwright test
// Phase 1 = no wallet: render, feed connectivity, lifecycle states, console
// hygiene. Wallet-mocked trade flows are phase 2.
export default defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  retries: process.env.CI ? 1 : 0,
  // Auto-start the dev server locally; skip when gating a deployed URL.
  webServer: process.env.BASE_URL
    ? undefined
    : {
        // Local app code against the DEPLOYED api — the exact wiring being
        // shipped. (Local indexer stack not required for the gate.)
        // INDEXER_URL picks the api under test: defaults to the deployed
        // preview (gate posture); point at the local docker stack with
        //   INDEXER_URL=http://127.0.0.1:8081 pnpm test:e2e
        command: `NEXT_PUBLIC_INDEXER_URL=${process.env.INDEXER_URL || 'https://api.torchmarket.dev'} pnpm dev`,
        url: 'http://localhost:3000/markets',
        reuseExistingServer: true,
        timeout: 120_000,
      },
  use: {
    baseURL: process.env.BASE_URL || 'http://localhost:3000',
    trace: 'retain-on-failure',
  },
})
