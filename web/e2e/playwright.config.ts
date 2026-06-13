import { defineConfig, devices } from '@playwright/test'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

/**
 * Base URLs for the apps under test.
 *
 * By default the suite spins up local servers (see `webServer` below) so the
 * tests are deterministic and do not depend on the live deployment being up.
 * Set E2E_WEB_BASE_URL / E2E_FAUCET_BASE_URL to point at a remote deployment
 * (e.g. https://botho.io); in that case the local servers are not started.
 */
const WEB_BASE_URL = process.env.E2E_WEB_BASE_URL ?? 'http://localhost:4173'
const FAUCET_BASE_URL = process.env.E2E_FAUCET_BASE_URL ?? 'http://localhost:4174'
const useLocalServers = !process.env.E2E_WEB_BASE_URL && !process.env.E2E_FAUCET_BASE_URL

/**
 * Playwright E2E test configuration for Botho web services.
 *
 * Projects:
 * - smoke: Quick sanity checks across all services
 * - web-wallet: Wallet creation, import, balance flows
 * - explorer: Block and transaction viewing
 * - faucet: Testnet coin requests
 * - integration: Cross-service flows (requires others to pass first)
 */
export default defineConfig({
  testDir: './tests',
  // Run serially by default: the wallet/explorer specs share a single
  // vite-preview /rpc proxy to a live seed node, and running many browser
  // contexts in parallel against it caused intermittent "connecting" flakes.
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  // A couple of retries absorb transient slowness from the shared public RPC.
  retries: process.env.CI ? 2 : 1,
  workers: 1,
  timeout: 60_000, // 60s default timeout for blockchain operations

  outputDir: '../test-results/artifacts',

  reporter: [
    ['list'],
    ['html', { open: 'never', outputFolder: '../test-results/report' }],
    ['json', { outputFile: '../test-results/results.json' }],
  ],

  use: {
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    // Video recording requires Playwright's ffmpeg build. Default to capturing
    // on failure; set E2E_VIDEO=off for environments where ffmpeg is unavailable.
    video: (process.env.E2E_VIDEO as 'off' | 'retain-on-failure') || 'retain-on-failure',
    // Browser channel. Defaults to Playwright's bundled chromium (works in CI
    // after `npx playwright install chromium`). For local runs where the
    // bundled browser cannot be downloaded, set E2E_BROWSER_CHANNEL=chrome to
    // use the system-installed Google Chrome instead.
    channel: process.env.E2E_BROWSER_CHANNEL || undefined,
  },

  // Start local servers for the web wallet/explorer and the faucet site unless
  // the suite is pointed at a remote deployment via env vars.
  webServer: useLocalServers
    ? [
        {
          // Build the web wallet with the RPC endpoint pointed at the
          // same-origin /rpc proxy (configured in the wallet's vite preview
          // config to forward to https://seed.botho.io), then serve
          // landing/wallet/explorer as a SPA via vite preview on port 4173.
          // This lets the explorer perform a real RPC read in e2e without
          // depending on cross-origin CORS.
          command:
            'VITE_RPC_ENDPOINT=/rpc pnpm --filter @botho/web-wallet build && pnpm --filter @botho/web-wallet preview --port 4173 --strictPort',
          cwd: path.resolve(__dirname, '..'),
          url: 'http://localhost:4173/',
          reuseExistingServer: !process.env.CI,
          timeout: 180_000,
        },
        {
          command: 'node e2e/serve-faucet.mjs',
          cwd: path.resolve(__dirname, '..'),
          url: 'http://localhost:4174/',
          reuseExistingServer: !process.env.CI,
          timeout: 30_000,
        },
      ]
    : undefined,

  projects: [
    // Smoke tests - run first, quick sanity check
    {
      name: 'smoke',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: WEB_BASE_URL,
      },
      testMatch: /smoke\.spec\.ts/,
    },

    // Web wallet tests
    {
      name: 'web-wallet',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: WEB_BASE_URL,
      },
      testMatch: /wallet\/.*\.spec\.ts/,
    },

    // Explorer tests
    {
      name: 'explorer',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: WEB_BASE_URL,
      },
      testMatch: /explorer\/.*\.spec\.ts/,
    },

    // Faucet tests
    {
      name: 'faucet',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: FAUCET_BASE_URL,
      },
      testMatch: /faucet\/.*\.spec\.ts/,
    },

    // Integration tests - run after component tests pass
    {
      name: 'integration',
      use: {
        ...devices['Desktop Chrome'],
      },
      testMatch: /integration\/.*\.spec\.ts/,
      dependencies: ['smoke'],
    },
  ],
})
