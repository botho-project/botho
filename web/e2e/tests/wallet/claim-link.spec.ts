/**
 * Claim-link (send-via-link -> claim) e2e (#492, feature #460/#474).
 *
 * Two halves of the bearer-link round-trip, both hermetic against the mock RPC:
 *
 *   SENDER: creating a claim link now requires an ENCRYPTED wallet (#474) — the
 *   bearer secret can only be persisted under a vault key. Wallets created via
 *   the helpers are encrypted by default (#475), so the "Send via Link" modal
 *   opens with the create action primed for a valid amount. The mock RPC has NO
 *   `tx_submit` and the e2e preview build ships without the wasm signer, so —
 *   like the request->pay spec — we assert up to the PRIMED/CONFIRM state rather
 *   than on-chain funding.
 *
 *   RECIPIENT: a link built in the sender's exact wire format (`buildClaimLink`)
 *   is opened at `/claim#…`. We assert the claim page RECOGNISES the bearer
 *   secret (reads the link, strips the secret from the address bar so it does not
 *   linger in history) and that a malformed fragment is rejected as invalid. The
 *   on-chain sweep/settlement is covered by the full-stack path, not the mock.
 */
import { test, expect } from '@playwright/test'
import { buildClaimLink, createClaimLinkMnemonic, parseBTH } from '@botho/core'
import { URLS, TIMEOUTS } from '../../fixtures/test-data'
import { createWalletOnDashboard } from '../../fixtures/wallet-setup'

test.describe('Claim link', () => {
  test('send-via-link modal primes the create action for an encrypted wallet', async ({
    page,
    context,
  }) => {
    await createWalletOnDashboard(page, context)

    await page.getByRole('button', { name: /Send via Link/i }).click()
    await expect(page.getByRole('heading', { name: /Send via Link/i })).toBeVisible()

    // Bearer warning is shown ("share it like cash").
    await expect(page.getByText(/Anyone with this link can claim/i)).toBeVisible()

    // With no amount the create button is disabled; a valid amount primes it.
    const createBtn = page.getByRole('button', { name: /Create Claim Link/i })
    await expect(createBtn).toBeDisabled()

    await page.getByPlaceholder('0.00').fill('1.0')
    await expect(createBtn).toBeEnabled()
  })

  test('claim page recognises a valid bearer link and strips the secret', async ({ page }) => {
    // Build a link in the sender's exact wire format, then open it as a claimer.
    const ephMnemonic = createClaimLinkMnemonic()
    const link = buildClaimLink('http://localhost:4173', ephMnemonic, parseBTH('2'))
    const claimPath = link.slice(link.indexOf('/claim#'))

    await page.goto(`${URLS.LANDING.replace(/\/$/, '')}${claimPath}`, {
      timeout: TIMEOUTS.PAGE_LOAD,
    })

    // The claim view renders and recognises the link (no malformed-fragment
    // error). The page reads the secret on mount.
    await expect(page.getByRole('heading', { name: /Claim Your BTH/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    await expect(page.getByText(/not valid|Unsupported claim-link|missing its secret/i)).toHaveCount(
      0,
    )

    // The bearer secret is stripped from the address bar so it does not linger
    // in history (a security-critical claim-page behavior).
    await expect.poll(() => page.evaluate(() => window.location.hash)).toBe('')
  })

  test('claim page rejects a malformed link', async ({ page }) => {
    await page.goto(`${URLS.LANDING.replace(/\/$/, '')}/claim#not-a-real-claim-link`, {
      timeout: TIMEOUTS.PAGE_LOAD,
    })

    await expect(page.getByRole('heading', { name: /Claim Your BTH/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    // A malformed fragment surfaces the invalid-link error.
    await expect(page.getByText(/not valid|Unsupported claim-link|valid base58/i)).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
  })
})
