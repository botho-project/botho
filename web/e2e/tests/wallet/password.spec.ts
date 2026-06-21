/**
 * Change-password e2e (#492, feature #489).
 *
 * Wallets are encrypted by default (#475), so the dashboard offers "Change
 * password" (password ROTATION) rather than a plaintext->encrypted upgrade. This
 * spec drives the real rotation flow against the hermetic mock RPC:
 *   - a wrong CURRENT password is rejected with an inline error (modal stays open);
 *   - a correct rotation completes (modal closes); afterwards the NEW password
 *     unlocks the wallet and the OLD password no longer does.
 */
import { test, expect } from '@playwright/test'
import { TIMEOUTS } from '../../fixtures/test-data'
import {
  createWalletOnDashboard,
  changeWalletPassword,
  lockWallet,
  unlockWallet,
  readDashboardAddress,
  E2E_PASSWORD,
} from '../../fixtures/wallet-setup'

const NEW_PASSWORD = 'rotated-password-456'

test.describe('Change password', () => {
  test('rejects a wrong current password', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)

    // Trigger and submit share the "Change password" label; scope to the modal.
    await page.getByRole('button', { name: /Change password/i }).first().click()
    const modal = page.locator('div.fixed.inset-0').filter({
      has: page.getByRole('heading', { name: /Change password/i }),
    })
    await expect(modal.getByRole('heading', { name: /Change password/i })).toBeVisible()

    await modal.getByPlaceholder('Current password').fill('the-wrong-current-password')
    await modal.getByPlaceholder(/^Password \(min/).fill(NEW_PASSWORD)
    await modal.getByPlaceholder('Confirm password').fill(NEW_PASSWORD)
    await modal.getByRole('button', { name: /^Change password$/i }).click()

    // The modal surfaces the wrong-current-password error and stays open.
    await expect(page.getByText(/incorrect current password/i)).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    await expect(modal.getByRole('heading', { name: /Change password/i })).toBeVisible()
  })

  test('rotates the password: new password unlocks, old no longer does', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)
    const addressBefore = await readDashboardAddress(page)

    // Rotate E2E_PASSWORD -> NEW_PASSWORD; the modal closes on success.
    await changeWalletPassword(page, E2E_PASSWORD, NEW_PASSWORD)
    await expect(page.getByRole('heading', { name: /Change password/i })).toBeHidden({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Lock, then confirm the OLD password is now rejected.
    await lockWallet(page)
    await page.getByPlaceholder('Enter password').fill(E2E_PASSWORD)
    await page.getByRole('button', { name: /^Unlock$/i }).click()
    await expect(page.getByText(/incorrect|invalid|unlock failed|wrong/i)).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    await expect(page.getByRole('heading', { name: /Unlock Wallet/i })).toBeVisible()

    // The NEW password unlocks the SAME wallet.
    await unlockWallet(page, NEW_PASSWORD)
    expect(await readDashboardAddress(page)).toBe(addressBefore)
  })
})
