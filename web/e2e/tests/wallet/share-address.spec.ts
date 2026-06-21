/**
 * Share my address -> Pay e2e (#479, feature #470).
 *
 * The "Just share my address" Request mode emits an ADDRESS-ONLY `/pay#…` link
 * (no amount, no memo). This spec generates that link, opens it as the payer,
 * enters an amount, and asserts the Pay button is primed.
 *
 * Hermetic mock RPC (no live node / no `tx_submit`): we assert the recipient
 * pre-fill + the payer-entered amount + the primed confirm UI, not on-chain
 * settlement.
 */
import { test, expect } from '@playwright/test'
import { createWalletOnDashboard, openPayLinkAsPayer } from '../../fixtures/wallet-setup'

test.describe('Share my address -> Pay', () => {
  test('address-only link lets a payer enter an amount and pay', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)

    // --- Generate an address-only receive link ----------------------------
    await page.getByRole('button', { name: /^Request$/i }).click()
    await expect(page.getByRole('heading', { name: /Request Payment/i })).toBeVisible()

    // Switch to "Just share my address" mode.
    await page.getByRole('button', { name: /Just share my address/i }).click()

    // The address-only receive link appears under "Receive link".
    const candidates = page.locator('input[readonly]')
    await candidates.first().waitFor({ state: 'visible' })
    let payLink = ''
    const count = await candidates.count()
    for (let i = 0; i < count; i++) {
      const val = await candidates.nth(i).inputValue()
      if (val.includes('/pay#')) {
        payLink = val
        break
      }
    }
    expect(payLink, 'an address-only /pay# link should be generated').toMatch(/\/pay#.+/)

    // --- Open it as the payer (fresh wallet created in-flow after reload) --
    await openPayLinkAsPayer(page, payLink)

    // No amount was requested -> the amount field is empty and the payer is
    // prompted to enter one.
    const amountField = page.getByPlaceholder('0.00')
    await expect(amountField).toHaveValue('')
    await expect(page.getByText(/enter how much to send/i)).toBeVisible()

    // Payer enters an amount; the Pay button is primed with the formatted value
    // (formatBTH renders whole amounts as e.g. "3.00 BTH").
    await amountField.fill('3')
    await expect(page.getByRole('button', { name: /Pay\s+3(\.0+)?\s*BTH/i })).toBeVisible()
  })
})
