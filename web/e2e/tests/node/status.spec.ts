import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS } from '../../fixtures/test-data'
import {
  BAAS_STATUS_RE,
  BAAS_PORTAL_RE,
  fulfillJson,
  MOCK_STRIPE_PORTAL_URL,
  PROVISIONING_NODE,
  RUNNING_NODE,
} from '../../fixtures/node'

/**
 * E2E coverage for the magic-link node status page (`/node/status?token=…`,
 * #458 §3 step 5 / §4 / §6).
 *
 * The Worker `/status` + `/portal` endpoints are mocked at the network layer
 * (fixtures/node.ts) so these specs are hermetic. They verify the browser-side
 * contract: a valid token renders the node's RPC URL / state / health / deep
 * links, the magic-link failure modes (missing token, 401 expired, 404 no node
 * yet) show specific messages, and "Manage subscription" redirects to the Stripe
 * customer portal URL the Worker returns.
 */
test.describe('Node status page', () => {
  test('without a token, tells the user the link is missing its credential', async ({ page }) => {
    await page.goto(URLS.NODE_STATUS, { timeout: TIMEOUTS.PAGE_LOAD })
    await expect(page.getByText(/missing its access token/i)).toBeVisible()
  })

  test('renders RPC endpoint, region, running state, and health for a valid token', async ({
    page,
  }) => {
    await page.route(BAAS_STATUS_RE, (route) => fulfillJson(route, 200, RUNNING_NODE))
    await page.goto(`${URLS.NODE_STATUS}?token=valid-token`, { timeout: TIMEOUTS.PAGE_LOAD })

    await expect(page.getByRole('heading', { name: /Your managed node/i })).toBeVisible()
    await expect(page.getByText(RUNNING_NODE.rpcUrl)).toBeVisible()
    await expect(page.getByText('us-west-2')).toBeVisible()
    await expect(page.getByText('Running')).toBeVisible()
    // healthSummary() renders "height <n> · synced" for an online, synced node.
    await expect(page.getByText(/height 4321 · synced/i)).toBeVisible()

    // "Open in wallet" is the deep link that pre-selects this node as custom RPC.
    const openLink = page.getByRole('link', { name: /Open in wallet/i })
    await expect(openLink).toHaveAttribute('href', RUNNING_NODE.walletDeepLink)
    await expect(page.getByRole('button', { name: /Manage subscription/i })).toBeVisible()
  })

  test('shows a provisioning node as still booting (health not yet reporting)', async ({ page }) => {
    await page.route(BAAS_STATUS_RE, (route) => fulfillJson(route, 200, PROVISIONING_NODE))
    await page.goto(`${URLS.NODE_STATUS}?token=valid-token`, { timeout: TIMEOUTS.PAGE_LOAD })

    await expect(page.getByText('Provisioning')).toBeVisible()
    await expect(page.getByText(/Not yet reporting/i)).toBeVisible()
  })

  test('surfaces an expired/invalid link (401)', async ({ page }) => {
    await page.route(BAAS_STATUS_RE, (route) =>
      fulfillJson(route, 401, { error: 'unauthorized' }),
    )
    await page.goto(`${URLS.NODE_STATUS}?token=stale`, { timeout: TIMEOUTS.PAGE_LOAD })

    await expect(page.getByText(/invalid or has expired/i)).toBeVisible()
  })

  test('tells a paid-but-not-yet-provisioned user no node exists yet (404)', async ({ page }) => {
    await page.route(BAAS_STATUS_RE, (route) => fulfillJson(route, 404, { error: 'no node' }))
    await page.goto(`${URLS.NODE_STATUS}?token=valid-token`, { timeout: TIMEOUTS.PAGE_LOAD })

    await expect(page.getByText(/No node found for this account yet/i)).toBeVisible()
  })

  test('"Manage subscription" opens the Stripe customer portal URL', async ({ page }) => {
    await page.route(BAAS_STATUS_RE, (route) => fulfillJson(route, 200, RUNNING_NODE))
    await page.route(BAAS_PORTAL_RE, (route) =>
      fulfillJson(route, 200, { url: MOCK_STRIPE_PORTAL_URL }),
    )
    await page.goto(`${URLS.NODE_STATUS}?token=valid-token`, { timeout: TIMEOUTS.PAGE_LOAD })

    await page.getByRole('button', { name: /Manage subscription/i }).click()
    await page.waitForURL(MOCK_STRIPE_PORTAL_URL, { timeout: TIMEOUTS.NETWORK_REQUEST })
  })
})
