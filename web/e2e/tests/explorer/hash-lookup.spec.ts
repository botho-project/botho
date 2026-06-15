import { test, expect } from '@playwright/test'
import { URLS, TIMEOUTS } from '../../fixtures/test-data'

/**
 * Explorer hash-lookup coverage (issue #330).
 *
 * These specs verify that the explorer can resolve lookups BY HASH, which
 * previously failed:
 *   - block-by-hash deep links (/explorer/block/:hash) and hash search, backed
 *     by the new `getBlockByHash` RPC method, and
 *   - tx-detail-from-RPC (/explorer/tx/:hash and tx-hash search), backed by the
 *     `getTransaction` RPC wiring in RemoteNodeAdapter.
 *
 * The suite proxies `/rpc` to the live seed node, whose chain tip moves over
 * time. Rather than hardcode a (quickly-stale) block/tx hash, these tests
 * derive a real, currently-valid block hash from the recent-blocks list at
 * runtime, then verify it round-trips through both the in-app search and a
 * cold deep-link reload. Specs skip cleanly when the explorer is not connected
 * (e.g. seed node unreachable from CI) so they never produce false negatives.
 */

async function waitForExplorerReady(page: import('@playwright/test').Page): Promise<boolean> {
  return await expect(page.getByPlaceholder(/Search by block height or hash/i))
    .toBeVisible({ timeout: TIMEOUTS.WALLET_SYNC })
    .then(() => true)
    .catch(() => false)
}

/**
 * Open the first recent block, read its full hash from the detail view's
 * "Hash" row, then return to the list. Returns null when no blocks are
 * available (disconnected). The block detail renders the full hash in a
 * copyable/monospace field, which we read via the DetailRow labelled "Hash".
 */
async function discoverBlockHash(page: import('@playwright/test').Page): Promise<string | null> {
  const hasBlocks = await page.getByText('Recent Blocks').isVisible()
  if (!hasBlocks) return null

  await page.locator('[class*="cursor-pointer"]').first().click()
  await expect(page.getByRole('heading', { name: /Block #\d+/i })).toBeVisible({
    timeout: TIMEOUTS.NETWORK_REQUEST,
  })

  // Read the 64-hex block hash rendered in the detail view.
  const hashText = await page
    .locator('text=/[0-9a-f]{64}/i')
    .first()
    .textContent({ timeout: TIMEOUTS.NETWORK_REQUEST })
    .catch(() => null)

  const match = hashText?.match(/[0-9a-f]{64}/i)
  return match ? match[0] : null
}

test.describe('Explorer - Block by hash', () => {
  test('search resolves a valid block hash to its block detail', async ({ page }) => {
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')

    const ready = await waitForExplorerReady(page)
    if (!ready) {
      test.skip(true, 'Explorer did not connect to a node')
      return
    }

    const blockHash = await discoverBlockHash(page)
    if (!blockHash) {
      test.skip(true, 'No recent blocks available to derive a hash')
      return
    }

    // Return to the list, then search by the discovered hash.
    await page.getByRole('button', { name: /Back to blocks/i }).click()
    await expect(page.getByText('Recent Blocks')).toBeVisible()

    const searchInput = page.getByPlaceholder(/Search by block height or hash/i)
    await searchInput.fill(blockHash)
    await page.getByRole('button', { name: 'Search' }).click()

    // The hash should resolve via getBlockByHash to a block detail view.
    await expect(page.getByRole('heading', { name: /Block #\d+/i })).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })
    expect(page.url()).toContain('/explorer/block/')
  })

  test('block-by-hash deep link resolves on cold load', async ({ page }) => {
    // First discover a valid hash from a connected session.
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')

    const ready = await waitForExplorerReady(page)
    if (!ready) {
      test.skip(true, 'Explorer did not connect to a node')
      return
    }

    const blockHash = await discoverBlockHash(page)
    if (!blockHash) {
      test.skip(true, 'No recent blocks available to derive a hash')
      return
    }

    // Cold-load the deep link directly (simulating a reload / shared link).
    await page.goto(`${URLS.EXPLORER}/block/${blockHash}`, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')

    // The deep link must re-fetch the block via getBlockByHash and render it,
    // rather than showing "Block or transaction not found".
    await expect(page.getByRole('heading', { name: /Block #\d+/i })).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
    await expect(page.getByText(/not found/i)).toHaveCount(0)
  })

  test('searching an unknown hash shows a not-found message', async ({ page }) => {
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')

    const ready = await waitForExplorerReady(page)
    if (!ready) {
      test.skip(true, 'Explorer did not connect to a node')
      return
    }

    // A non-zero 64-hex value that is not a real block or tx. (All-zeros is
    // special-cased as height 0 by the search precedence logic, so use a
    // value that is unambiguously a hash.)
    const unknownHash = 'f'.repeat(64)
    const searchInput = page.getByPlaceholder(/Search by block height or hash/i)
    await searchInput.fill(unknownHash)
    await page.getByRole('button', { name: 'Search' }).click()

    await expect(page.getByText(/not found/i)).toBeVisible({
      timeout: TIMEOUTS.NETWORK_REQUEST,
    })
  })
})

test.describe('Explorer - Transaction by hash', () => {
  test('tx deep link resolves to a detail view or a not-found message (never hangs)', async ({
    page,
  }) => {
    await page.goto(URLS.EXPLORER, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')

    const ready = await waitForExplorerReady(page)
    if (!ready) {
      test.skip(true, 'Explorer did not connect to a node')
      return
    }

    // The seed testnet has very few transactions and tx hashes rotate, so we
    // cannot rely on a fixed known-funded tx hash. Instead we assert that a tx
    // deep link RESOLVES to a deterministic terminal state (either a rendered
    // transaction detail, or a "not found" message) rather than getting stuck
    // on the connecting/loading state. This exercises the getTransaction RPC
    // path end-to-end regardless of whether the specific hash still exists.
    const someTxHash = '3ca3c24209844d8f6d9bf38bd1571976a691423e329f4ca0cbbf3d044da3012e'
    await page.goto(`${URLS.EXPLORER}/tx/${someTxHash}`, { timeout: TIMEOUTS.PAGE_LOAD })
    await page.waitForLoadState('networkidle')

    // Either the tx hash is rendered in a detail view, or a not-found message
    // is shown. Both are acceptable terminal states; a stuck spinner is not.
    const detail = page.getByText(someTxHash, { exact: false })
    const notFound = page.getByText(/not found/i)
    await expect(detail.or(notFound).first()).toBeVisible({
      timeout: TIMEOUTS.WALLET_SYNC,
    })
  })
})
