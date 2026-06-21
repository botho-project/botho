/**
 * Receive QR modal smoke (#479, optional).
 *
 * A cheap check that the Receive modal opens and renders the wallet's address +
 * scannable QR. Hermetic mock RPC; no node needed.
 */
import { test, expect } from '@playwright/test'
import { createWalletOnDashboard } from '../../fixtures/wallet-setup'

test.describe('Receive QR modal', () => {
  test('opens and shows the address QR', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)

    await page.getByRole('button', { name: /^Receive$/i }).click()

    await expect(page.getByRole('heading', { name: 'Receive' })).toBeVisible()
    await expect(page.getByLabel('Receiving address QR code')).toBeVisible()
    await expect(page.getByText('Your address')).toBeVisible()
  })
})
