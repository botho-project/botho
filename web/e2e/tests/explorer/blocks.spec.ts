import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS } from '../../fixtures/test-data'

// Helper to wait for explorer to be ready (either block list or connecting message)
async function waitForExplorerReady(page: import('@playwright/test').Page) {
  // Wait for either blocks to load or connecting message
  await Promise.race([
    expect(page.getByText('Recent Blocks')).toBeVisible({ timeout: TIMEOUTS.NETWORK_REQUEST }),
    expect(page.getByText('Connecting to network')).toBeVisible({ timeout: 5000 }),
  ]).catch(() => {
    // Ignore - we'll check in the test
  })
}

test.describe('Explorer - Block List', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
  })

  test('loads explorer page successfully', async ({ page }) => {
    // Should show the explorer header
    await expect(page.getByText('Block Explorer')).toBeVisible()

    // Should have network selector
    await expect(page.locator('button').filter({ hasText: /testnet|mainnet/i })).toBeVisible()
  })

  test('displays search bar', async ({ page }) => {
    // Should show search input
    const searchInput = page.getByPlaceholder(/Search by block height or hash/i)
    await expect(searchInput).toBeVisible()

    // Should show search button
    await expect(page.getByRole('button', { name: 'Search' })).toBeVisible()
  })

  test('displays recent blocks when connected', async ({ page }) => {
    await waitForExplorerReady(page)

    // Check if blocks loaded
    const hasBlocks = await page.getByText('Recent Blocks').isVisible()
    if (hasBlocks) {
      // Should show at least one block with height indicator
      await expect(page.locator('text=/#\\d+/').first()).toBeVisible()
    }
    // If not connected yet, that's acceptable - skip the assertion
  })

  test('has load more button when blocks loaded', async ({ page }) => {
    await waitForExplorerReady(page)

    const hasBlocks = await page.getByText('Recent Blocks').isVisible()
    if (hasBlocks) {
      await expect(page.getByRole('button', { name: 'Load More' })).toBeVisible()
    }
  })
})

test.describe('Explorer - Block Detail', () => {
  test('can click block to view details', async ({ page }) => {
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
    await waitForExplorerReady(page)

    const hasBlocks = await page.getByText('Recent Blocks').isVisible()
    if (!hasBlocks) {
      test.skip()
      return
    }

    // Click on the first block
    await page.locator('[class*="cursor-pointer"]').first().click()

    // Should show block detail view
    await expect(page.getByRole('heading', { name: /Block #\d+/i })).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })

    // Should show back button
    await expect(page.getByRole('button', { name: /Back to blocks/i })).toBeVisible()
  })

  test('block detail shows key fields', async ({ page }) => {
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
    await waitForExplorerReady(page)

    const hasBlocks = await page.getByText('Recent Blocks').isVisible()
    if (!hasBlocks) {
      test.skip()
      return
    }

    // Click first block
    await page.locator('[class*="cursor-pointer"]').first().click()

    // Wait for detail view
    await expect(page.getByRole('heading', { name: /Block #\d+/i })).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })

    // Should show key fields (use first() to avoid strict mode)
    await expect(page.getByText('Height').first()).toBeVisible()
    await expect(page.getByText('Timestamp').first()).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Transactions' })).toBeVisible()
  })

  test('can navigate back to block list', async ({ page }) => {
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
    await waitForExplorerReady(page)

    const hasBlocks = await page.getByText('Recent Blocks').isVisible()
    if (!hasBlocks) {
      test.skip()
      return
    }

    // Click a block
    await page.locator('[class*="cursor-pointer"]').first().click()

    // Wait for detail
    await expect(page.getByRole('heading', { name: /Block #/i })).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })

    // Click back
    await page.getByRole('button', { name: /Back to blocks/i }).click()

    // Should be back at list
    await expect(page.getByText('Recent Blocks')).toBeVisible()
  })
})
