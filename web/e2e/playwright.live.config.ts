import { defineConfig, devices } from '@playwright/test'

/**
 * Live-smoke Playwright config — runs the deployed wallet (wallet.botho.io) +
 * the LIVE testnet, NOT a local build/mock.
 *
 * This is SEPARATE from playwright.config.ts on purpose:
 *   - It spins up NO local servers (no serve-node / serve-rpc-mock / preview):
 *     it points at the real deployed site + real SCP nodes.
 *   - It only runs the gated spec under tests/live/** (which itself no-ops
 *     unless BOTHO_LIVE=1), so it is fully OPT-IN.
 *   - The default `pnpm test` uses playwright.config.ts and never references
 *     this file, so the hermetic local-mock suite is completely unaffected.
 *
 * Run it explicitly (from web/, the @botho/web-wallet package, etc.):
 *
 *     BOTHO_LIVE=1 npx playwright test --config e2e/playwright.live.config.ts
 *
 * Override the target deployment with BOTHO_LIVE_URL (default
 * https://wallet.botho.io). This config is intentionally NOT wired into CI: it
 * hits live infrastructure and the faucet's rate limits.
 */
const BASE_URL = process.env.BOTHO_LIVE_URL ?? 'https://wallet.botho.io'

export default defineConfig({
  testDir: './tests/live',
  testMatch: /.*\.spec\.ts/,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  // Live infra is occasionally slow / cold; a retry absorbs transient flakes.
  retries: 1,
  workers: 1,
  timeout: 90_000,

  outputDir: '../test-results/live-artifacts',

  reporter: [
    ['list'],
    ['html', { open: 'never', outputFolder: '../test-results/live-report' }],
  ],

  use: {
    baseURL: BASE_URL,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    video: (process.env.E2E_VIDEO as 'off' | 'retain-on-failure') || 'off',
    // Browser channel. Defaults to Playwright's bundled chromium (which uses
    // chrome-headless-shell for headless runs). Set E2E_BROWSER_CHANNEL=chrome
    // to use the system-installed Google Chrome, or E2E_BROWSER_CHANNEL=chromium
    // to launch the full bundled Chromium build instead of the smaller
    // chrome-headless-shell — handy where only the full chromium binary is
    // available.
    channel: process.env.E2E_BROWSER_CHANNEL || undefined,
  },

  // No webServer: we target the live deployment + live nodes directly.

  projects: [
    {
      name: 'live-smoke',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
})
