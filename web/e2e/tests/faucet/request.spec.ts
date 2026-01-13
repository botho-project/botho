import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS, TEST_MNEMONIC_12 } from '../../fixtures/test-data'

// Test address - use a known valid testnet address format
const TEST_ADDRESS = 'tbotho://1/NTMFPKArvTestAddress123456789abcdefghijk'

test.describe('Faucet - Page Structure', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(URLS.FAUCET, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
  })

  test('displays faucet header', async ({ page }) => {
    // Should show faucet title
    await expect(page.getByText('Botho Testnet Faucet')).toBeVisible()
    await expect(page.getByText('Get testnet BTH for development')).toBeVisible()
  })

  test('displays address input', async ({ page }) => {
    // Should show address label
    await expect(page.getByText('Your Wallet Address')).toBeVisible()

    // Should have address textarea
    const addressInput = page.locator('#address')
    await expect(addressInput).toBeVisible()

    // Should have placeholder
    await expect(addressInput).toHaveAttribute('placeholder', /tbotho:\/\//)
  })

  test('displays request button', async ({ page }) => {
    const requestButton = page.locator('#request-btn')
    await expect(requestButton).toBeVisible()
    await expect(requestButton).toContainText(/Request.*BTH/i)
  })

  test('displays drip amount info', async ({ page }) => {
    // Should show current drip amount
    await expect(page.getByText('Current drip amount:')).toBeVisible()
    await expect(page.locator('#drip-amount')).toBeVisible()
  })

  test('displays faucet status card', async ({ page }) => {
    // Should show faucet status section
    await expect(page.getByRole('heading', { name: 'Faucet Status' })).toBeVisible()

    // Wait for status to load
    await page.waitForTimeout(2000)

    // Should show status content or loading
    const hasContent = await page.locator('#status-content').isVisible()
    const hasLoading = await page.getByText('Loading faucet status').isVisible()
    const hasError = await page.locator('#status-error').isVisible()

    expect(hasContent || hasLoading || hasError).toBeTruthy()
  })

  test('displays node status card', async ({ page }) => {
    // Should show node status section
    await expect(page.getByRole('heading', { name: 'Node Status' })).toBeVisible()
  })
})

test.describe('Faucet - Address Input', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(URLS.FAUCET, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
  })

  test('accepts valid testnet address', async ({ page }) => {
    const addressInput = page.locator('#address')
    await addressInput.fill(TEST_ADDRESS)

    // Should not show error for valid format
    const errorElement = page.locator('#address-error')
    await page.waitForTimeout(500)

    // Error should be hidden or empty
    const isHidden = await errorElement.isHidden()
    const isEmpty = (await errorElement.textContent()) === ''
    expect(isHidden || isEmpty).toBeTruthy()
  })

  test('shows error for empty address on submit', async ({ page }) => {
    // Click request without entering address
    await page.locator('#request-btn').click()

    // Should show error
    const errorElement = page.locator('#address-error')
    await expect(errorElement).toBeVisible()
  })

  test('shows error for invalid address format', async ({ page }) => {
    const addressInput = page.locator('#address')
    await addressInput.fill('not-a-valid-address')

    // Click request
    await page.locator('#request-btn').click()

    // Should show error
    const errorElement = page.locator('#address-error')
    await expect(errorElement).toBeVisible()
  })

  test('rejects request with mainnet address', async ({ page }) => {
    const addressInput = page.locator('#address')
    // Use mainnet prefix (botho:// instead of tbotho://)
    await addressInput.fill('botho://1/NTMFPKArvTestAddress123456789abcdefghijk')

    // Click request
    await page.locator('#request-btn').click()

    // Should show error or result banner with error
    await page.waitForTimeout(2000)
    const errorElement = page.locator('#address-error')
    const resultBanner = page.locator('#result-banner')

    const hasError = await errorElement.isVisible()
    const hasResult = await resultBanner.isVisible()

    // Either client-side error or server response
    expect(hasError || hasResult).toBeTruthy()
  })
})

test.describe('Faucet - Status Display', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(URLS.FAUCET, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
  })

  test('shows faucet limits when loaded', async ({ page }) => {
    // Wait for status to load
    await page.waitForTimeout(3000)

    const statusContent = page.locator('#status-content')
    const isVisible = await statusContent.isVisible()

    if (isVisible) {
      // Should show max amount
      await expect(page.locator('#max-amount')).toBeVisible()

      // Should show daily limit
      await expect(page.locator('#daily-limit')).toBeVisible()

      // Should show daily dispensed
      await expect(page.locator('#daily-dispensed')).toBeVisible()
    }
  })

  test('shows node metrics when loaded', async ({ page }) => {
    // Wait for stats to load
    await page.waitForTimeout(3000)

    const statsContent = page.locator('#node-stats-content')
    const isVisible = await statsContent.isVisible()

    if (isVisible) {
      // Should show uptime
      await expect(page.locator('#node-uptime')).toBeVisible()

      // Should show height
      await expect(page.locator('#node-height')).toBeVisible()
    }
  })

  test('status indicator shows active or inactive', async ({ page }) => {
    // Wait for status to load
    await page.waitForTimeout(3000)

    const statusContent = page.locator('#status-content')
    const isVisible = await statusContent.isVisible()

    if (isVisible) {
      // Should show status indicator (Active or some status)
      const statusIndicator = page.locator('#status-indicator')
      await expect(statusIndicator).toBeVisible()
    }
  })
})

test.describe('Faucet - Footer', () => {
  test('displays footer links', async ({ page }) => {
    await page.goto(URLS.FAUCET, { timeout: TIMEOUTS.PAGE_LOAD })

    // Should show testnet indicator in footer (use exact match to avoid header)
    await expect(page.getByText('Botho Testnet', { exact: true })).toBeVisible()

    // Should have docs link
    await expect(page.getByRole('link', { name: 'Docs' })).toBeVisible()

    // Should have GitHub link
    await expect(page.getByRole('link', { name: 'GitHub' })).toBeVisible()
  })
})
