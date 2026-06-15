import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS, INVALID } from '../../fixtures/test-data'

// Helper to wait for the explorer to finish connecting. The search bar is only
// rendered once connected to the node, so waiting for it is a reliable signal
// that the explorer is ready for search interactions.
async function waitForExplorerReady(page: import('@playwright/test').Page) {
  await expect(page.getByPlaceholder(/Search by block height or hash/i)).toBeVisible({
    timeout: TIMEOUTS.WALLET_SYNC,
  })
}

test.describe('Explorer - Search', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')
  })

  test('can search by block height', async ({ page }) => {
    await waitForExplorerReady(page)

    // The e2e RPC mock always serves a recent-block list, so this is deterministic.
    await expect(page.getByText('Recent Blocks')).toBeVisible()

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

    // The e2e RPC mock always serves a recent-block list, so this is deterministic.
    await expect(page.getByText('Recent Blocks')).toBeVisible()

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

    // The e2e RPC mock always serves a recent-block list, so this is deterministic.
    await expect(page.getByText('Recent Blocks')).toBeVisible()

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
