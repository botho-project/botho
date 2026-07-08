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

// Local JSON-RPC mock for the explorer/wallet. Pointing the vite-preview `/rpc`
// proxy here (instead of the live seed node) makes the explorer specs hermetic:
// the connect handshake (node_getStatus) and block reads resolve deterministically
// from fixed fixtures, eliminating the "Connecting to network..." flake (#334).
const RPC_MOCK_PORT = 4175
const RPC_MOCK_URL = `http://localhost:${RPC_MOCK_PORT}`

// Full-stack mode (#372 layer b): run the wallet against a REAL local botho node
// instead of the static mock, so the send-modal flow builds+signs+submits a tx
// the node actually accepts and mines. Enabled with E2E_FULLSTACK=1; the node is
// launched by `serve-node.mjs` and the preview `/rpc` proxy is pointed at it.
// The mock does not implement `tx_submit`, so a real node is mandatory here.
const FULLSTACK = process.env.E2E_FULLSTACK === '1'
const NODE_RPC_PORT = Number(process.env.E2E_NODE_RPC_PORT ?? 17599)
const NODE_RPC_URL = process.env.E2E_RPC_PROXY_TARGET ?? `http://127.0.0.1:${NODE_RPC_PORT}`
// In full-stack mode the preview proxy targets the real node; otherwise the mock.
const PROXY_TARGET = FULLSTACK ? NODE_RPC_URL : RPC_MOCK_URL

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
  // The live-smoke suite (tests/live/**) targets the DEPLOYED wallet + live
  // testnet and is run via a SEPARATE config (playwright.live.config.ts), gated
  // behind BOTHO_LIVE=1. Exclude it here so the default `pnpm test` (hermetic,
  // local-mock) never picks it up — including via the `smoke` project's
  // `/smoke\.spec\.ts/` testMatch, which would otherwise match it.
  testIgnore: ['**/tests/live/**'],
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
        FULLSTACK
          ? {
              // Real local botho node (solo minting) for the full-stack send
              // flow. Replaces the mock; the mock has no `tx_submit`. Pre-mines
              // enough blocks for a CLSAG decoy ring before reporting ready, so
              // the wallet can actually build + submit a transaction.
              command: 'node e2e/serve-node.mjs',
              cwd: path.resolve(__dirname, '..'),
              // serve-node exposes a GET health endpoint that returns 200 only
              // once the node has mined enough blocks; the RPC itself only
              // answers POST /rpc and would never satisfy Playwright's GET probe.
              url: `http://127.0.0.1:${process.env.E2E_NODE_HEALTH_PORT ?? 17600}`,
              reuseExistingServer: !process.env.CI,
              timeout: 300_000,
            }
          : {
              // Local JSON-RPC mock the wallet/explorer talk to via the
              // same-origin /rpc proxy. Started before the preview server so the
              // proxy target is up by the time the explorer connects. Returns
              // fixed node_getStatus / getChainInfo / getBlockByHeight payloads
              // so connect + block reads are deterministic (no live-node dep).
              command: 'node e2e/serve-rpc-mock.mjs',
              cwd: path.resolve(__dirname, '..'),
              url: RPC_MOCK_URL,
              reuseExistingServer: !process.env.CI,
              timeout: 30_000,
            },
        {
          // Build the web wallet with the RPC endpoint pointed at the
          // same-origin /rpc proxy, then serve landing/wallet/explorer as a SPA
          // via vite preview on port 4173. E2E_RPC_PROXY_TARGET points the
          // preview's /rpc proxy at PROXY_TARGET — the local mock by default, or
          // the real local node in full-stack mode (E2E_FULLSTACK=1) — so the
          // wallet performs real RPC against the chosen backend without
          // cross-origin CORS or live-node flake.
          command:
            'VITE_RPC_ENDPOINT=/rpc pnpm --filter @botho/web-wallet build && E2E_RPC_PROXY_TARGET=' +
            PROXY_TARGET +
            ' pnpm --filter @botho/web-wallet preview --port 4173 --strictPort',
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

    // Botho-as-a-Service "Host a node" checkout + status flows (#458). The
    // Worker (/checkout, /status, /portal) is mocked via page.route (see
    // fixtures/node.ts) so these stay hermetic — no Stripe, no AWS.
    {
      name: 'node',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: WEB_BASE_URL,
      },
      testMatch: /node\/.*\.spec\.ts/,
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

    // Full-stack send flow (#372 layer b) - drives the browser wallet against a
    // REAL local node (E2E_FULLSTACK=1). Gated to its own project so the default
    // suite (mock-backed) is unaffected; run with:
    //   E2E_FULLSTACK=1 pnpm test:e2e --project=fullstack
    {
      name: 'fullstack',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: WEB_BASE_URL,
      },
      testMatch: /fullstack\/.*\.spec\.ts/,
    },
  ],
})
