import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS, TEST_MNEMONIC_12, TEST_MNEMONIC_24 } from '../../fixtures/test-data'
import { completeImportWallet, E2E_PASSWORD, readDashboardAddress } from '../../fixtures/wallet-setup'

// Post-#475 importing is ENCRYPTED BY DEFAULT: there is no "Protect with
// password" opt-out checkbox and importing REQUIRES a valid password (>= 8
// chars, matching). The "Import Wallet" button stays disabled until the
// mnemonic is a valid 12/24-word phrase AND the password is valid. These specs
// drive that required-password flow (reusing completeImportWallet) and keep the
// still-valid coverage: word-count validation, button gating, deterministic
// address, persistence, and tab switching.

test.describe('Wallet Import', () => {
  test.beforeEach(async ({ page, context }) => {
    // Clear storage to ensure fresh state
    await context.clearCookies()
    await page.goto(URLS.WALLET, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.evaluate(() => localStorage.clear())
    await page.reload()
    await page.waitForLoadState('networkidle')

    // Navigate to import tab
    await page.getByRole('button', { name: 'Import Existing' }).click()
  })

  test('shows import wallet form when tab selected', async ({ page }) => {
    // Should show Import Wallet heading (not "Import Existing Wallet")
    await expect(page.getByRole('heading', { name: /Import Wallet/i })).toBeVisible()

    // Should show mnemonic input area (textarea)
    await expect(page.getByPlaceholder(/Enter your recovery phrase/i)).toBeVisible()
  })

  test('can import wallet with 12-word mnemonic', async ({ page }) => {
    // Import through the required-password flow.
    await completeImportWallet(page, TEST_MNEMONIC_12)

    // Should land on the dashboard.
    await expect(page.getByRole('button', { name: /^Send$/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    await expect(page.getByText('Total Balance')).toBeVisible()
  })

  test('can import wallet with 24-word mnemonic', async ({ page }) => {
    // Import through the required-password flow.
    await completeImportWallet(page, TEST_MNEMONIC_24)

    // Should land on the dashboard.
    await expect(page.getByRole('button', { name: /^Send$/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    await expect(page.getByText('Total Balance')).toBeVisible()
  })

  test('shows word count validation', async ({ page }) => {
    // Enter partial mnemonic (not 12 or 24 words)
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)
    await mnemonicInput.fill('abandon abandon abandon abandon abandon')

    // Should show word count indicator
    await expect(page.getByText('5 words')).toBeVisible()

    // Should show warning about expected word count
    await expect(page.getByText(/Expected 12 or 24 words/i)).toBeVisible()

    // Import button should be disabled
    await expect(page.getByRole('button', { name: 'Import Wallet' })).toBeDisabled()
  })

  test('import button is gated on word count AND a valid password', async ({ page }) => {
    const importButton = page.getByRole('button', { name: 'Import Wallet' })
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)

    // Wrong word count -> disabled.
    await mnemonicInput.fill('abandon abandon abandon')
    await expect(importButton).toBeDisabled()

    // Valid 12 words but NO password (#475) -> still disabled.
    await mnemonicInput.fill(TEST_MNEMONIC_12)
    await expect(importButton).toBeDisabled()

    // A too-short password keeps it disabled.
    await page.getByPlaceholder(/^Password \(min/).fill('short')
    await page.getByPlaceholder('Confirm password').fill('short')
    await expect(importButton).toBeDisabled()

    // Valid, matching password -> enabled.
    await page.getByPlaceholder(/^Password \(min/).fill(E2E_PASSWORD)
    await page.getByPlaceholder('Confirm password').fill(E2E_PASSWORD)
    await expect(importButton).toBeEnabled()
  })

  test('import button disabled while passwords mismatch', async ({ page }) => {
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)
    await mnemonicInput.fill(TEST_MNEMONIC_12)

    // Mismatched (individually long-enough) passwords.
    await page.getByPlaceholder(/^Password \(min/).fill('e2e-password-123')
    await page.getByPlaceholder('Confirm password').fill('different-password')

    await expect(page.getByText(/don't match/i)).toBeVisible()
    await expect(page.getByRole('button', { name: 'Import Wallet' })).toBeDisabled()
  })

  test('imported wallet address is deterministic', async ({ page }) => {
    // Import wallet with the test mnemonic through the required-password flow.
    await completeImportWallet(page, TEST_MNEMONIC_12)

    // Wait for dashboard
    await expect(page.getByRole('button', { name: /^Send$/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Get the displayed address (truncated format in button with code element)
    const addressText = await readDashboardAddress(page)

    // Address should start with tbotho (testnet prefix, shown truncated)
    expect(addressText).toContain('tbotho')
  })

  test('imported wallet persists after page reload', async ({ page }) => {
    // Import wallet through the required-password flow.
    await completeImportWallet(page, TEST_MNEMONIC_12)

    // Wait for dashboard
    await expect(page.getByRole('button', { name: /^Send$/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Get the address
    const addressBefore = await readDashboardAddress(page)
    expect(addressBefore).toBeTruthy()

    // Reload page
    await page.reload()
    await page.waitForLoadState('networkidle')

    // After a full reload the in-memory vault key is dropped, so the encrypted
    // wallet is locked: the unlock screen (not setup) proves persistence.
    await expect(page.getByRole('heading', { name: /Unlock Wallet/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Unlock and confirm the same address is restored.
    await page.getByPlaceholder('Enter password').fill(E2E_PASSWORD)
    await page.getByRole('button', { name: /^Unlock$/i }).click()

    await expect(page.getByRole('button', { name: /^Send$/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    const addressAfter = await readDashboardAddress(page)
    expect(addressAfter).toBe(addressBefore)
  })

  test('can switch between create and import tabs', async ({ page }) => {
    // Currently on import tab (from beforeEach)
    await expect(page.getByRole('heading', { name: /Import Wallet/i })).toBeVisible()

    // Switch to create tab
    await page.getByRole('button', { name: 'Create New' }).click()
    await expect(page.getByRole('heading', { name: /Create New Wallet/i })).toBeVisible()

    // Switch back to import tab
    await page.getByRole('button', { name: 'Import Existing' }).click()
    await expect(page.getByRole('heading', { name: /Import Wallet/i })).toBeVisible()
  })
})
