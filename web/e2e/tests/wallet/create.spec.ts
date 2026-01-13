import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS } from '../../fixtures/test-data'

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

  test('create button is disabled until mnemonic revealed and confirmed', async ({ page }) => {
    const createButton = page.getByRole('button', { name: 'Create Wallet' })

    // Initially disabled
    await expect(createButton).toBeDisabled()

    // Reveal mnemonic
    await page.getByText('Click to reveal').click()

    // Still disabled (need to confirm)
    await expect(createButton).toBeDisabled()

    // Check the confirmation checkbox
    const confirmCheckbox = page.locator('input[type="checkbox"]').first()
    await confirmCheckbox.check()

    // Now should be enabled
    await expect(createButton).toBeEnabled()
  })

  test('can create wallet without password', async ({ page }) => {
    // Reveal mnemonic
    await page.getByText('Click to reveal').click()

    // Check confirmation
    const confirmCheckbox = page.locator('input[type="checkbox"]').first()
    await confirmCheckbox.check()

    // Click create
    await page.getByRole('button', { name: 'Create Wallet' }).click()

    // Should navigate to dashboard - look for Send button
    await expect(page.getByRole('button', { name: /Send/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Should show Total Balance text (dashboard indicator)
    await expect(page.getByText('Total Balance')).toBeVisible()
  })

  test('can create wallet with password protection', async ({ page }) => {
    // Reveal mnemonic
    await page.getByText('Click to reveal').click()

    // Check confirmation
    const confirmCheckbox = page.locator('input[type="checkbox"]').first()
    await confirmCheckbox.check()

    // Enable password protection - find checkbox near "Protect with password" text
    const passwordSection = page.locator('label').filter({ hasText: 'Protect with password' })
    const passwordCheckbox = passwordSection.locator('input[type="checkbox"]')
    await passwordCheckbox.check()

    // Enter password
    const passwordInput = page.getByPlaceholder('Password (min 4 characters)')
    await passwordInput.fill('testpass123')

    // Confirm password
    const confirmPasswordInput = page.getByPlaceholder('Confirm password')
    await confirmPasswordInput.fill('testpass123')

    // Click create
    await page.getByRole('button', { name: 'Create Wallet' }).click()

    // Should navigate to dashboard
    await expect(page.getByRole('button', { name: /Send/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
  })

  test('password mismatch shows error', async ({ page }) => {
    // Reveal mnemonic
    await page.getByText('Click to reveal').click()

    // Check confirmation
    const confirmCheckbox = page.locator('input[type="checkbox"]').first()
    await confirmCheckbox.check()

    // Enable password protection
    const passwordSection = page.locator('label').filter({ hasText: 'Protect with password' })
    const passwordCheckbox = passwordSection.locator('input[type="checkbox"]')
    await passwordCheckbox.check()

    // Enter mismatched passwords
    const passwordInput = page.getByPlaceholder('Password (min 4 characters)')
    await passwordInput.fill('testpass123')

    const confirmPasswordInput = page.getByPlaceholder('Confirm password')
    await confirmPasswordInput.fill('differentpass')

    // Should show mismatch error
    await expect(page.getByText(/don't match/i)).toBeVisible()

    // Create button should be disabled
    await expect(page.getByRole('button', { name: 'Create Wallet' })).toBeDisabled()
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
    // Create wallet
    await page.getByText('Click to reveal').click()
    const confirmCheckbox = page.locator('input[type="checkbox"]').first()
    await confirmCheckbox.check()
    await page.getByRole('button', { name: 'Create Wallet' }).click()

    // Wait for dashboard
    await expect(page.getByRole('button', { name: /Send/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })

    // Find the address display (truncated format: tboth...xxxx)
    // The address is in a button with the copy functionality
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
})
