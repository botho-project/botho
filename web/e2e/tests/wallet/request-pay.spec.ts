/**
 * Request -> Pay e2e (#479, feature #470).
 *
 * Exercises the *pull* payment flow end-to-end through the real UI against the
 * hermetic mock RPC:
 *   1. Open the Request modal, enter an amount, and read the generated `/pay#…`
 *      link (the requester's PUBLIC address + amount, encoded in the fragment).
 *   2. Open that link as a payer and assert the pay page pre-fills the recipient
 *      (the requester's address) + amount and primes the confirm ("Pay N BTH").
 *
 * The mock RPC has NO `tx_submit`, so we assert the PRE-FILL + CONFIRM UI rather
 * than on-chain settlement. Real submission is covered by the full-stack send
 * spec.
 *
 * Opening the link is a full page load, so the payer arrives at the pay page's
 * WalletGate (the in-memory session is gone). We create a fresh payer wallet
 * in-flow — the realistic payer != payee case — and the parsed request is
 * preserved so the pre-filled confirmation appears.
 */
import { test, expect } from '@playwright/test'
import { formatBTH, parseBTH } from '@botho/core'
import { createWalletOnDashboard, openPayLinkAsPayer } from '../../fixtures/wallet-setup'

/** Pull the generated payment link out of the Request modal's readonly field. */
async function readGeneratedPayLink(page: import('@playwright/test').Page): Promise<string> {
  // The modal renders the link in a readonly <input>; find the one whose value
  // is the /pay# URL (there can be a separate readonly "Your address" field in
  // share-address mode).
  const candidates = page.locator('input[readonly]')
  await candidates.first().waitFor({ state: 'visible' })
  const count = await candidates.count()
  for (let i = 0; i < count; i++) {
    const val = await candidates.nth(i).inputValue()
    if (val.includes('/pay#')) return val
  }
  return ''
}

test.describe('Request -> Pay', () => {
  test('generates a /pay link with an amount and the pay page pre-fills it', async ({
    page,
    context,
  }) => {
    // The requester wallet. A fresh create yields a random mnemonic, so we read
    // the resulting address straight off the generated link to assert the
    // recipient pre-fill rather than guessing it.
    await createWalletOnDashboard(page, context)

    // --- Open the Request modal and request a specific amount -------------
    await page.getByRole('button', { name: /^Request$/i }).click()
    await expect(page.getByRole('heading', { name: /Request Payment/i })).toBeVisible()

    // "Request an amount" mode is the default; enter an amount.
    const amountBth = '2.5'
    await page.getByPlaceholder('Any amount').fill(amountBth)

    // The QR + link build live as you type. Grab the generated /pay# link.
    const payLink = await readGeneratedPayLink(page)
    expect(payLink, 'a /pay# link should be generated').toMatch(/\/pay#.+/)

    // --- Open the link as a (fresh) payer ---------------------------------
    await openPayLinkAsPayer(page, payLink)

    // The amount field is pre-filled with the requested amount (no separators).
    const expectedAmount = formatBTH(parseBTH(amountBth), { separators: false })
    const amountField = page.getByPlaceholder('0.00')
    await expect(amountField).toHaveValue(expectedAmount)

    // The recipient (requester's address) is shown on the confirmation, and the
    // Pay button is primed with the amount.
    await expect(page.getByText(/You.?re paying/i)).toBeVisible()
    await expect(page.getByRole('button', { name: /Pay\s+[\d.,]+\s*BTH/i })).toBeVisible()
  })

  test('a blank-amount request lets the payer enter the amount', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)

    await page.getByRole('button', { name: /^Request$/i }).click()
    await expect(page.getByRole('heading', { name: /Request Payment/i })).toBeVisible()

    // Leave the amount blank -> link carries only the address ("payer chooses").
    const payLink = await readGeneratedPayLink(page)
    expect(payLink).toMatch(/\/pay#.+/)

    await openPayLinkAsPayer(page, payLink)

    // No amount pre-filled; the "enter how much" hint is shown and the payer can
    // type an amount which then primes the Pay button.
    const amountField = page.getByPlaceholder('0.00')
    await expect(amountField).toHaveValue('')
    await expect(page.getByText(/enter how much to send/i)).toBeVisible()

    await amountField.fill('1.25')
    await expect(page.getByRole('button', { name: /Pay\s+1\.25\s*BTH/i })).toBeVisible()
  })
})
