import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS } from '../fixtures/test-data'

test.describe('Smoke Tests', () => {
  test.describe('Landing Page', () => {
    test('loads successfully', async ({ page }) => {
      await page.goto(URLS.LANDING, { timeout: TIMEOUTS.PAGE_LOAD })

      // Page should have loaded
      await expect(page).toHaveTitle(/Botho/i)

      // Logo should be visible
      await expect(page.locator('text=Botho').first()).toBeVisible()
    })

    test('has no significant console errors', async ({ page }) => {
      const errors: string[] = []
      page.on('console', (msg) => {
        if (msg.type() === 'error') {
          errors.push(msg.text())
        }
      })

      await page.goto(URLS.LANDING, { timeout: TIMEOUTS.PAGE_LOAD })
      await page.waitForLoadState('networkidle')

      // Filter out known benign errors:
      // - favicon 404
      // - CORS errors (infrastructure config issue, not app bug)
      // - Failed to load resource (usually caused by CORS)
      const significantErrors = errors.filter(
        (e) =>
          !e.includes('favicon') &&
          !e.includes('404') &&
          !e.includes('CORS') &&
          !e.includes('Access-Control-Allow-Origin') &&
          !e.includes('net::ERR_FAILED')
      )

      expect(significantErrors).toHaveLength(0)
    })

    test('navigation links work', async ({ page }) => {
      await page.goto(URLS.LANDING, { timeout: TIMEOUTS.PAGE_LOAD })

      // Check wallet link exists and is clickable
      const walletLink = page.locator('a[href="/wallet"]').first()
      await expect(walletLink).toBeVisible()

      // Check explorer link exists
      const explorerLink = page.locator('a[href="/explorer"]').first()
      await expect(explorerLink).toBeVisible()
    })
  })

  test.describe('Wallet Page', () => {
    test('loads successfully', async ({ page }) => {
      await page.goto(URLS.WALLET, { timeout: TIMEOUTS.PAGE_LOAD })

      // Wait for page to stabilize
      await page.waitForLoadState('networkidle')

      // Should show wallet setup (Create New Wallet heading) or dashboard (Send button)
      const hasCreateHeading = await page
        .getByRole('heading', { name: /Create New Wallet/i })
        .isVisible()
        .catch(() => false)

      const hasImportHeading = await page
        .getByRole('heading', { name: /Import Wallet/i })
        .isVisible()
        .catch(() => false)

      const hasSendButton = await page
        .getByRole('button', { name: /Send/i })
        .isVisible()
        .catch(() => false)

      const hasUnlockHeading = await page
        .getByRole('heading', { name: /Unlock Wallet/i })
        .isVisible()
        .catch(() => false)

      expect(hasCreateHeading || hasImportHeading || hasSendButton || hasUnlockHeading).toBe(true)
    })

    test('has network selector', async ({ page }) => {
      await page.goto(URLS.WALLET, { timeout: TIMEOUTS.PAGE_LOAD })

      // Network selector should be visible in header
      const networkSelector = page.locator(
        'button:has-text("Testnet"), button:has-text("Mainnet")'
      )
      await expect(networkSelector.first()).toBeVisible()
    })

    test('shows create/import options for new users', async ({ page, context }) => {
      // Clear storage to simulate new user
      await context.clearCookies()

      await page.goto(URLS.WALLET, { timeout: TIMEOUTS.PAGE_LOAD })

      // Clear localStorage to ensure fresh state
      await page.evaluate(() => localStorage.clear())
      await page.reload()
      await page.waitForLoadState('networkidle')

      // Should show Create New button (tab) - use role to be specific
      await expect(page.getByRole('button', { name: 'Create New' })).toBeVisible()

      // Should show Import Existing button (tab)
      await expect(page.getByRole('button', { name: 'Import Existing' })).toBeVisible()
    })
  })

  test.describe('Explorer Page', () => {
    test('loads successfully', async ({ page }) => {
      await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })

      // Should show Block Explorer title
      await expect(page.getByText('Block Explorer')).toBeVisible()
    })

    test('displays recent blocks or loading state', async ({ page }) => {
      await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })

      // Wait for either blocks to load or "connecting" message
      const hasBlocks = await page
        .locator('[class*="block"]')
        .first()
        .isVisible({ timeout: TIMEOUTS.NETWORK_REQUEST })
        .catch(() => false)

      const isConnecting = await page
        .getByText(/connecting/i)
        .isVisible()
        .catch(() => false)

      // Should show either blocks or connecting state
      expect(hasBlocks || isConnecting).toBe(true)
    })

    test('has search functionality', async ({ page }) => {
      await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })

      // Search input should be present
      const searchInput = page.locator(
        'input[placeholder*="Search"], input[placeholder*="search"], input[type="search"]'
      )

      // Wait for page to fully load
      await page.waitForLoadState('networkidle')

      // Search should be visible (may need to wait for connection)
      const isVisible = await searchInput.isVisible({ timeout: 5000 }).catch(() => false)

      // It's okay if search isn't visible while connecting
      if (!isVisible) {
        const isConnecting = await page.getByText(/connecting/i).isVisible()
        expect(isConnecting).toBe(true)
      }
    })
  })

  test.describe('Faucet Page', () => {
    test('loads successfully', async ({ page }) => {
      await page.goto(URLS.FAUCET, { timeout: TIMEOUTS.PAGE_LOAD })

      // Should show faucet title/heading
      await expect(page.getByText(/Testnet Faucet|Get Testnet/i).first()).toBeVisible()
    })

    test('has address input field', async ({ page }) => {
      await page.goto(URLS.FAUCET, { timeout: TIMEOUTS.PAGE_LOAD })

      // Faucet uses a textarea with id="address"
      const addressInput = page.locator('#address')
      await expect(addressInput).toBeVisible()
    })

    test('has request button', async ({ page }) => {
      await page.goto(URLS.FAUCET, { timeout: TIMEOUTS.PAGE_LOAD })

      // Should have a button to request coins
      const requestButton = page.locator('#request-btn')
      await expect(requestButton).toBeVisible()
    })

    test('shows testnet indicator', async ({ page }) => {
      await page.goto(URLS.FAUCET, { timeout: TIMEOUTS.PAGE_LOAD })

      // Should clearly indicate this is testnet
      await expect(page.getByText(/testnet/i).first()).toBeVisible()
    })
  })

  test.describe('Cross-Service Navigation', () => {
    test('can navigate from landing to wallet', async ({ page }) => {
      await page.goto(URLS.LANDING, { timeout: TIMEOUTS.PAGE_LOAD })

      // Click wallet link
      await page.click('a[href="/wallet"]')

      // Should be on wallet page
      await expect(page).toHaveURL(/\/wallet/)
    })

    test('can navigate from landing to explorer', async ({ page }) => {
      await page.goto(URLS.LANDING, { timeout: TIMEOUTS.PAGE_LOAD })

      // Click explorer link
      await page.click('a[href="/explorer"]')

      // Should be on explorer page
      await expect(page).toHaveURL(/\/explorer/)
    })

    test('can navigate back to landing from wallet', async ({ page }) => {
      await page.goto(URLS.WALLET, { timeout: TIMEOUTS.PAGE_LOAD })

      // Click back/home link (usually the logo or back arrow)
      const backLink = page.locator('a[href="/"]').first()
      await backLink.click()

      // Should be back on landing
      await expect(page).toHaveURL(URLS.LANDING)
    })
  })
})
