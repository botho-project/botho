import { defineConfig, devices } from '@playwright/test'

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
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  // Limit workers to avoid network issues with external services
  workers: process.env.CI ? 1 : 2,
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
    video: 'retain-on-failure',
  },

  projects: [
    // Smoke tests - run first, quick sanity check
    {
      name: 'smoke',
      use: {
        ...devices['Desktop Chrome'],
      },
      testMatch: /smoke\.spec\.ts/,
    },

    // Web wallet tests
    {
      name: 'web-wallet',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: 'https://botho.io',
      },
      testMatch: /wallet\/.*\.spec\.ts/,
    },

    // Explorer tests
    {
      name: 'explorer',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: 'https://botho.io',
      },
      testMatch: /explorer\/.*\.spec\.ts/,
    },

    // Faucet tests
    {
      name: 'faucet',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: 'https://faucet.botho.io',
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
