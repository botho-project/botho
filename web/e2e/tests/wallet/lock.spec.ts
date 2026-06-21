/**
 * Lock / unlock e2e (#492, feature #490).
 *
 * The wallet is encrypted by default (#475), so every wallet created via the
 * helpers can be locked. This spec drives the real lock/unlock cycle against the
 * hermetic mock RPC:
 *   - the header Lock button clears the session and shows the Unlock screen;
 *   - unlocking with the correct password restores the SAME wallet (same address);
 *   - a wrong password surfaces an error and keeps the wallet locked.
 */
import { test, expect } from '@playwright/test'
import { TIMEOUTS } from '../../fixtures/test-data'
import {
  createWalletOnDashboard,
  lockWallet,
  unlockWallet,
  readDashboardAddress,
  E2E_PASSWORD,
} from '../../fixtures/wallet-setup'

test.describe('Lock / unlock', () => {
  test('lock clears the session, unlock restores the same wallet', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)
    const addressBefore = await readDashboardAddress(page)
    expect(addressBefore).toBeTruthy()

    // Lock -> Unlock screen.
    await lockWallet(page)
    await expect(page.getByRole('heading', { name: /Unlock Wallet/i })).toBeVisible()
    // The dashboard's Send button is gone while locked.
    await expect(page.getByRole('button', { name: /^Send$/i })).toHaveCount(0)

    // Unlock -> dashboard restored with the same address.
    await unlockWallet(page, E2E_PASSWORD)
    const addressAfter = await readDashboardAddress(page)
    expect(addressAfter).toBe(addressBefore)
  })

  test('a wrong password is rejected and the wallet stays locked', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)
    await lockWallet(page)

    await page.getByPlaceholder('Enter password').fill('definitely-the-wrong-password')
    await page.getByRole('button', { name: /^Unlock$/i }).click()

    // An error is shown and we remain on the Unlock screen.
    await expect(page.getByText(/incorrect|invalid|unlock failed|wrong/i)).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    await expect(page.getByRole('heading', { name: /Unlock Wallet/i })).toBeVisible()
  })
})
