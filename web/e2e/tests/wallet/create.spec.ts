import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS } from '../../fixtures/test-data'
import { completeCreateWallet, E2E_PASSWORD, readDashboardAddress } from '../../fixtures/wallet-setup'

// Post-#475 the wallet is ENCRYPTED BY DEFAULT: there is no "Protect with
// password" opt-out checkbox and no plaintext create path. Creating a wallet
// REQUIRES a valid password (>= 8 chars, matching confirmation), and the
// "Create Wallet" button stays disabled until the mnemonic is revealed, the
// "I wrote it down" box is ticked, AND the password is valid. These specs drive
// that required-password flow (reusing completeCreateWallet) and keep the still-
// valid coverage: mnemonic reveal, 12-word BIP39 validity, button-gating, the
// password-mismatch error, and persistence across reload.

test.describe('Wallet Creation', () => {
  test.beforeEach(async ({ page, context }) => {
    // Clear storage to ensure fresh state
    await context.clearCookies()
    await page.goto(URLS.WALLET, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.evaluate(() => localStorage.clear())
    await page.reload()
    await page.waitForLoadState('networkidle')
  })

  test('shows create wallet form by default', async ({ page }) => {
    // Should show Create New tab as active
    const createTab = page.getByRole('button', { name: 'Create New' })
    await expect(createTab).toBeVisible()

    // Should show the create wallet heading
    await expect(page.getByRole('heading', { name: /Create New Wallet/i })).toBeVisible()

    // Should show mnemonic area (blurred initially)
    await expect(page.getByText('Click to reveal')).toBeVisible()
  })

  test('can reveal mnemonic phrase', async ({ page }) => {
    // Click to reveal mnemonic
    await page.getByText('Click to reveal').click()

    // Mnemonic should now be visible (12 words)
    // The mnemonic is in a div that becomes unblurred
    const mnemonicArea = page.locator('.font-mono').first()
    await expect(mnemonicArea).toBeVisible()

    // Should contain multiple words (mnemonic)
    const mnemonicText = await mnemonicArea.textContent()
    expect(mnemonicText).toBeTruthy()
    const words = mnemonicText!.trim().split(/\s+/)
    expect(words.length).toBe(12)
  })

  test('create button is disabled until mnemonic confirmed and password valid', async ({ page }) => {
    const createButton = page.getByRole('button', { name: 'Create Wallet' })

    // Initially disabled
    await expect(createButton).toBeDisabled()

    // Reveal mnemonic
    await page.getByText('Click to reveal').click()

    // Still disabled (need to confirm + set a password)
    await expect(createButton).toBeDisabled()

    // Check the confirmation checkbox
    const confirmCheckbox = page.locator('input[type="checkbox"]').first()
    await confirmCheckbox.check()

    // #475: still disabled because no password has been set
    await expect(createButton).toBeDisabled()

    // Fill matching, valid (>= 8 char) password + confirmation
    await page.getByPlaceholder(/^Password \(min/).fill(E2E_PASSWORD)
    await page.getByPlaceholder('Confirm password').fill(E2E_PASSWORD)

    // Now should be enabled
    await expect(createButton).toBeEnabled()
  })

  test('create button stays disabled for a too-short password', async ({ page }) => {
    // Reveal + confirm mnemonic
    await page.getByText('Click to reveal').click()
    await page.locator('input[type="checkbox"]').first().check()

    // A password shorter than the minimum should NOT enable create
    await page.getByPlaceholder(/^Password \(min/).fill('short')
    await page.getByPlaceholder('Confirm password').fill('short')

    // Strength hint surfaces the minimum-length requirement
    await expect(page.getByText(/At least \d+ characters/i)).toBeVisible()

    await expect(page.getByRole('button', { name: 'Create Wallet' })).toBeDisabled()
  })

  test('password mismatch shows error and disables create', async ({ page }) => {
    // Reveal + confirm mnemonic
    await page.getByText('Click to reveal').click()
    await page.locator('input[type="checkbox"]').first().check()

    // Enter mismatched (but individually long-enough) passwords
    await page.getByPlaceholder(/^Password \(min/).fill('e2e-password-123')
    await page.getByPlaceholder('Confirm password').fill('different-password')

    // Should show mismatch error
    await expect(page.getByText(/don't match/i)).toBeVisible()

    // Create button should be disabled while passwords mismatch
    await expect(page.getByRole('button', { name: 'Create Wallet' })).toBeDisabled()
  })

  test('can create wallet with required password', async ({ page }) => {
    // Drive the full required-password create flow.
    await completeCreateWallet(page)

    // Should land on the dashboard - look for Send button + Total Balance.
    await expect(page.getByRole('button', { name: /^Send$/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    await expect(page.getByText('Total Balance')).toBeVisible()
  })

  test('mnemonic is 12 valid BIP39 words', async ({ page }) => {
    // Reveal mnemonic
    await page.getByText('Click to reveal').click()

    // Get mnemonic text
    const mnemonicArea = page.locator('.font-mono').first()
    const mnemonicText = await mnemonicArea.textContent()

    // Should have 12 words
    const words = mnemonicText!.trim().split(/\s+/)
    expect(words.length).toBe(12)

    // Each word should be lowercase alphabetic (BIP39 words are all lowercase)
    for (const word of words) {
      expect(word).toMatch(/^[a-z]+$/)
      expect(word.length).toBeGreaterThanOrEqual(3)
    }
  })

  test('wallet persists after page reload', async ({ page }) => {
    // Create a password-protected wallet through the required-password flow.
    await completeCreateWallet(page)

    // Wait for dashboard
    await expect(page.getByRole('button', { name: /^Send$/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Find the address display (truncated format: tboth...xxxx)
    const addressBefore = await readDashboardAddress(page)
    expect(addressBefore).toBeTruthy()

    // Reload page
    await page.reload()
    await page.waitForLoadState('networkidle')

    // After a full reload the in-memory vault key is dropped, so the wallet is
    // locked: the unlock screen (not the setup screen) proves persistence.
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
})
