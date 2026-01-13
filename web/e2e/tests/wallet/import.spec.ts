import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS, TEST_MNEMONIC_12, TEST_MNEMONIC_24 } from '../../fixtures/test-data'

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
    // Enter the test mnemonic
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)
    await mnemonicInput.fill(TEST_MNEMONIC_12)

    // Click import button
    await page.getByRole('button', { name: 'Import Wallet' }).click()

    // Should navigate to dashboard
    await expect(page.getByRole('button', { name: /Send/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Should show Total Balance (dashboard indicator)
    await expect(page.getByText('Total Balance')).toBeVisible()
  })

  test('can import wallet with 24-word mnemonic', async ({ page }) => {
    // Enter the 24-word test mnemonic
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)
    await mnemonicInput.fill(TEST_MNEMONIC_24)

    // Click import button
    await page.getByRole('button', { name: 'Import Wallet' }).click()

    // Should navigate to dashboard
    await expect(page.getByRole('button', { name: /Send/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Should show Total Balance (dashboard indicator)
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

  test('import button disabled for invalid word count', async ({ page }) => {
    // Enter mnemonic with wrong word count
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)
    await mnemonicInput.fill('abandon abandon abandon')

    // Import button should be disabled
    await expect(page.getByRole('button', { name: 'Import Wallet' })).toBeDisabled()

    // Now enter valid 12 words
    await mnemonicInput.fill(TEST_MNEMONIC_12)

    // Import button should be enabled
    await expect(page.getByRole('button', { name: 'Import Wallet' })).toBeEnabled()
  })

  test('can import wallet with password protection', async ({ page }) => {
    // Enter the test mnemonic
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)
    await mnemonicInput.fill(TEST_MNEMONIC_12)

    // Enable password protection
    const passwordSection = page.locator('label').filter({ hasText: 'Protect with password' })
    const passwordCheckbox = passwordSection.locator('input[type="checkbox"]')
    await passwordCheckbox.check()

    // Enter password
    const passwordInput = page.getByPlaceholder('Password (min 4 characters)')
    await passwordInput.fill('testpass123')

    // Confirm password
    const confirmPasswordInput = page.getByPlaceholder('Confirm password')
    await confirmPasswordInput.fill('testpass123')

    // Click import
    await page.getByRole('button', { name: 'Import Wallet' }).click()

    // Should navigate to dashboard
    await expect(page.getByRole('button', { name: /Send/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
  })

  test('imported wallet address is deterministic', async ({ page }) => {
    // Import wallet with test mnemonic
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)
    await mnemonicInput.fill(TEST_MNEMONIC_12)
    await page.getByRole('button', { name: 'Import Wallet' }).click()

    // Wait for dashboard
    await expect(page.getByRole('button', { name: /Send/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Get the displayed address (truncated format in button with code element)
    const addressButton = page.locator('button').filter({ has: page.locator('code.font-mono') }).first()
    const addressText = await addressButton.textContent()

    // Address should start with tbotho (testnet prefix, shown truncated)
    expect(addressText).toContain('tbotho')
  })

  test('imported wallet persists after page reload', async ({ page }) => {
    // Import wallet
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)
    await mnemonicInput.fill(TEST_MNEMONIC_12)
    await page.getByRole('button', { name: 'Import Wallet' }).click()

    // Wait for dashboard
    await expect(page.getByRole('button', { name: /Send/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Get the address
    const addressButton = page.locator('button').filter({ has: page.locator('code.font-mono') }).first()
    const addressBefore = await addressButton.textContent()

    // Reload page
    await page.reload()
    await page.waitForLoadState('networkidle')

    // Should still show dashboard (not setup)
    await expect(page.getByRole('button', { name: /Send/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Address should be the same
    const addressAfter = await addressButton.textContent()
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
