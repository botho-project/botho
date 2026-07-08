import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS } from '../../fixtures/test-data'
import {
  BAAS_CHECKOUT_RE,
  captureCheckout,
  fulfillJson,
  MOCK_STRIPE_CHECKOUT_URL,
} from '../../fixtures/node'

/**
 * E2E coverage for the "Host a Node" checkout surface (`/node`, #458 §2/§4).
 *
 * The Worker `/checkout` endpoint is mocked at the network layer (see
 * fixtures/node.ts) so these specs are hermetic — no Stripe, no AWS. They verify
 * the browser-side contract: the page renders, the button POSTs the right body
 * to `/checkout`, redirects to the returned Stripe URL, surfaces Worker/network
 * errors, and manages its own loading/disabled state.
 *
 * The Worker-side behaviour (region allowlist, Stripe params, the
 * subscription-mode fix in PR #723) is covered by baas-worker unit tests; this
 * suite is the front-of-house integration layer.
 */
test.describe('Node checkout', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(URLS.NODE, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
  })

  test('renders the host-a-node page with region + subscribe CTA', async ({ page }) => {
    await expect(
      page.getByRole('heading', { name: /Host a Node for Your Community/i }),
    ).toBeVisible()
    // The honest "not an income scheme" caveat must be present (#458 §7).
    await expect(page.getByText(/A hosting service, not an income scheme/i)).toBeVisible()
    await expect(page.getByLabel('Region')).toBeVisible()
    await expect(page.getByRole('button', { name: /Subscribe/i })).toBeEnabled()
  })

  test('subscribing POSTs {region} and redirects to the Stripe URL', async ({ page }) => {
    const getBody = captureCheckout(page)

    await page.getByRole('button', { name: /Subscribe/i }).click()

    // The frontend redirects via window.location.assign; the mock points that at
    // the in-app success route so navigation resolves.
    await page.waitForURL(MOCK_STRIPE_CHECKOUT_URL, { timeout: TIMEOUTS.NETWORK_REQUEST })
    await expect(page.getByRole('heading', { name: /Subscription started/i })).toBeVisible()

    // The Worker must receive exactly the region (no email when the field is blank).
    expect(getBody()).toEqual({ region: 'us-west-2' })
  })

  test('includes the email in the checkout request when provided', async ({ page }) => {
    const getBody = captureCheckout(page)

    await page.getByLabel(/Email/i).fill('villager@example.com')
    await page.getByRole('button', { name: /Subscribe/i }).click()

    await page.waitForURL(MOCK_STRIPE_CHECKOUT_URL, { timeout: TIMEOUTS.NETWORK_REQUEST })
    expect(getBody()).toEqual({ region: 'us-west-2', email: 'villager@example.com' })
  })

  test('surfaces a Worker error and re-enables the button', async ({ page }) => {
    // Mirror the real 502 shape the Worker returns on a Stripe failure.
    await page.route(BAAS_CHECKOUT_RE, (route) =>
      fulfillJson(route, 502, { error: 'could not create checkout session' }),
    )

    const button = page.getByRole('button', { name: /Subscribe/i })
    await button.click()

    await expect(page.getByText(/could not create checkout session/i)).toBeVisible()
    // Still on the node page (no redirect) and the button is usable again.
    expect(new URL(page.url()).pathname).toBe('/node')
    await expect(button).toBeEnabled()
  })

  test('shows a friendly message when the checkout service is unreachable', async ({ page }) => {
    await page.route(BAAS_CHECKOUT_RE, (route) => route.abort())

    await page.getByRole('button', { name: /Subscribe/i }).click()

    await expect(page.getByText(/Could not reach the checkout service/i)).toBeVisible()
    await expect(page.getByRole('button', { name: /Subscribe/i })).toBeEnabled()
  })

  test('disables the button and shows a redirecting state while submitting', async ({ page }) => {
    // Delay the checkout response so the in-flight state is observable.
    captureCheckout(page, { delayMs: 1500 })

    await page.getByRole('button', { name: /Subscribe/i }).click()

    // While the request is in flight the button is disabled and shows the
    // redirecting label.
    await expect(page.getByRole('button', { name: /Redirecting to Stripe/i })).toBeVisible()
    await expect(page.getByRole('button', { name: /Redirecting to Stripe/i })).toBeDisabled()

    // Eventually completes the redirect.
    await page.waitForURL(MOCK_STRIPE_CHECKOUT_URL, { timeout: TIMEOUTS.NETWORK_REQUEST })
  })
})
