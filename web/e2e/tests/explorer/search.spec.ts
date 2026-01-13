import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS, INVALID } from '../../fixtures/test-data'

// Helper to wait for explorer to be ready
async function waitForExplorerReady(page: import('@playwright/test').Page) {
  await Promise.race([
    expect(page.getByText('Recent Blocks')).toBeVisible({ timeout: TIMEOUTS.NETWORK_REQUEST }),
    expect(page.getByText('Connecting to network')).toBeVisible({ timeout: 5000 }),
  ]).catch(() => {})
}

test.describe('Explorer - Search', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
  })

  test('can search by block height', async ({ page }) => {
    await waitForExplorerReady(page)

    const hasBlocks = await page.getByText('Recent Blocks').isVisible()
    if (!hasBlocks) {
      test.skip()
      return
    }

    // Get first block's height
    const firstBlockHeight = await page.locator('text=/#\\d+/').first().textContent()
    const blockHeight = firstBlockHeight?.replace('#', '') || '1'

    // Search
    const searchInput = page.getByPlaceholder(/Search by block height or hash/i)
    await searchInput.fill(blockHeight)
    await page.getByRole('button', { name: 'Search' }).click()

    // Should show block detail
    await expect(page.getByRole('heading', { name: `Block #${blockHeight}` })).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })
  })

  test('can search by pressing Enter', async ({ page }) => {
    await waitForExplorerReady(page)

    const hasBlocks = await page.getByText('Recent Blocks').isVisible()
    if (!hasBlocks) {
      test.skip()
      return
    }

    // Get first block's height
    const firstBlockHeight = await page.locator('text=/#\\d+/').first().textContent()
    const blockHeight = firstBlockHeight?.replace('#', '') || '1'

    // Search using Enter
    const searchInput = page.getByPlaceholder(/Search by block height or hash/i)
    await searchInput.fill(blockHeight)
    await searchInput.press('Enter')

    // Should show block detail
    await expect(page.getByRole('heading', { name: `Block #${blockHeight}` })).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })
  })

  test('shows error for non-existent block height', async ({ page }) => {
    await waitForExplorerReady(page)

    // Search for very large block height
    const searchInput = page.getByPlaceholder(/Search by block height or hash/i)
    await searchInput.fill('99999999')
    await page.getByRole('button', { name: 'Search' }).click()

    // Should show error message
    await expect(page.getByText(/not found/i)).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })
  })

  test('shows error for invalid hash', async ({ page }) => {
    await waitForExplorerReady(page)

    // Search for invalid hash (all zeros)
    const searchInput = page.getByPlaceholder(/Search by block height or hash/i)
    await searchInput.fill(INVALID.TX_HASH)
    await page.getByRole('button', { name: 'Search' }).click()

    // Should show error message
    await expect(page.getByText(/not found/i)).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })
  })

  test('URL updates when viewing block', async ({ page }) => {
    await waitForExplorerReady(page)

    const hasBlocks = await page.getByText('Recent Blocks').isVisible()
    if (!hasBlocks) {
      test.skip()
      return
    }

    // Get first block's height
    const firstBlockHeight = await page.locator('text=/#\\d+/').first().textContent()
    const blockHeight = firstBlockHeight?.replace('#', '') || '1'

    // Search
    const searchInput = page.getByPlaceholder(/Search by block height or hash/i)
    await searchInput.fill(blockHeight)
    await page.getByRole('button', { name: 'Search' }).click()

    // Wait for detail
    await expect(page.getByRole('heading', { name: /Block #/i })).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })

    // URL should contain block path
    expect(page.url()).toContain('/explorer/block/')
  })
})
