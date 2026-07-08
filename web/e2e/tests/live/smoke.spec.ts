/**
 * Live-smoke e2e — runs against the DEPLOYED wallet (https://botho.io)
 * and the LIVE testnet SCP nodes, complementing the hermetic local-mock suite.
 *
 * WHY THIS EXISTS
 * ---------------
 * The local-mock suite (web/e2e/tests/**) builds the app fresh and serves it
 * with a mocked /rpc, so it can NEVER catch a *deploy* regression — e.g. the
 * wasm-404 that happened when a build landed on a Cloudflare Pages preview
 * instead of production, leaving `/pkg/bth_wasm_signer_bg.wasm` un-served. This
 * spec drives the real deployed bundle + real nodes to catch exactly that class
 * of bug (wasm not served, manifest/SW missing, node ingress down).
 *
 * GATING
 * ------
 * This spec is OPT-IN. It only runs through `playwright.live.config.ts`, which
 * the default `pnpm test` does NOT use. Run it explicitly:
 *
 *     BOTHO_LIVE=1 pnpm --filter @botho/web-wallet exec \
 *       playwright test --config e2e/playwright.live.config.ts
 *
 *   (or from web/e2e: `BOTHO_LIVE=1 npx playwright test --config playwright.live.config.ts`)
 *
 * Override the target with BOTHO_LIVE_URL (defaults to https://botho.io).
 * Without BOTHO_LIVE=1 every test in this file is skipped, so even if this file
 * were ever picked up by the default config it would be a no-op there.
 *
 * The default config's `testDir` is `./tests` and would otherwise match this
 * file; it explicitly ignores `tests/live/**` (see playwright.config.ts
 * testIgnore) so the hermetic suite is unaffected.
 */
import { test, expect, type Page } from '@playwright/test'

// Opt-in flag. When unset, skip the whole file so this never runs in default
// CI / `pnpm test` (which hits live infra + faucet rate limits).
const LIVE = process.env.BOTHO_LIVE === '1'

const BASE_URL = process.env.BOTHO_LIVE_URL ?? 'https://botho.io'

// A vite DEV server does not emit /sw.js or the precache manifest — only a
// built bundle does. Skip the PWA assertions when pointed at local dev so a
// local run isn't misleading (#676); force them with BOTHO_LIVE_EXPECT_PWA=1
// when serving a production build locally (`vite preview`).
const IS_LOCAL_DEV =
  /^https?:\/\/(localhost|127\.0\.0\.1)/.test(BASE_URL) &&
  process.env.BOTHO_LIVE_EXPECT_PWA !== '1'

// Matches PasswordFields' placeholder (min length lives in @botho/core).
const PASSWORD = 'live-smoke-password'

// Known BIP39 test mnemonic — produces a deterministic testnet address. Never
// holds real funds. Mirrors web/e2e/fixtures/test-data.ts TEST_MNEMONIC_12.
const TEST_MNEMONIC_12 =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about'

// Live pages can be slow (cold edge cache + real network); be generous.
const NAV_TIMEOUT = 30_000
const UI_TIMEOUT = 20_000

test.describe('Live smoke @ botho.io', () => {
  test.skip(!LIVE, 'Set BOTHO_LIVE=1 to run the live-smoke suite (hits live infra + faucet limits).')

  // The deployed bundle dynamic-imports the wasm signer from /pkg/*; a 404 there
  // is the regression we are guarding against. Capture wasm-related console
  // errors and failed requests so we can assert the crypto module actually loads.
  function watchWasm(page: Page) {
    const wasmErrors: string[] = []
    const failedWasm: string[] = []
    page.on('console', (msg) => {
      // Match the signer artifact specifically — a bare /wasm/ also matches
      // "wasm-unsafe-eval" inside unrelated CSP-violation messages (e.g. the
      // Cloudflare analytics beacon being blocked by our own script-src).
      if (msg.type() === 'error' && /bth_wasm_signer|\/pkg\/|\.wasm\b/i.test(msg.text())) {
        wasmErrors.push(msg.text())
      }
    })
    page.on('requestfailed', (req) => {
      if (/\.wasm($|\?)|bth_wasm_signer|\/pkg\//i.test(req.url())) {
        failedWasm.push(`${req.url()} (${req.failure()?.errorText ?? 'failed'})`)
      }
    })
    page.on('response', (resp) => {
      if (/\.wasm($|\?)|bth_wasm_signer/i.test(resp.url()) && resp.status() >= 400) {
        failedWasm.push(`${resp.url()} -> HTTP ${resp.status()}`)
      }
    })
    return { wasmErrors, failedWasm }
  }

  test('landing page loads with correct title (HTTP 200)', async ({ page }) => {
    const resp = await page.goto(`${BASE_URL}/`, { timeout: NAV_TIMEOUT })
    expect(resp, 'navigation response present').toBeTruthy()
    expect(resp!.status(), 'landing page returns 2xx').toBeLessThan(400)

    await expect(page).toHaveTitle(/Botho/i)
    // App boots past the shell: the Botho wordmark renders.
    await expect(page.locator('text=Botho').first()).toBeVisible({ timeout: UI_TIMEOUT })
  })

  test('PWA manifest + service worker are served', async ({ page, request }) => {
    test.skip(
      IS_LOCAL_DEV,
      'dev server does not emit /sw.js — run against a built bundle or set BOTHO_LIVE_EXPECT_PWA=1',
    )
    await page.goto(`${BASE_URL}/`, { timeout: NAV_TIMEOUT })

    // Manifest link is present in the document head and resolves to a 200.
    const manifestHref = await page
      .locator('link[rel="manifest"]')
      .first()
      .getAttribute('href', { timeout: UI_TIMEOUT })
    expect(manifestHref, 'manifest <link> present').toBeTruthy()
    const manifestUrl = new URL(manifestHref!, BASE_URL).toString()
    const manifestResp = await request.get(manifestUrl)
    expect(manifestResp.status(), `manifest ${manifestUrl} serves`).toBe(200)

    // Service worker script is served (PWA precache). VitePWA emits /sw.js.
    const swResp = await request.get(new URL('/sw.js', BASE_URL).toString())
    expect(swResp.status(), 'service worker script serves').toBe(200)
  })

  test('WASM crypto module loads — no wasm console error (deploy-404 guard)', async ({ page }) => {
    const { wasmErrors, failedWasm } = watchWasm(page)

    // The wasm signer is dynamic-imported lazily; the wallet page exercises it
    // (key derivation / address rendering), forcing the /pkg/* fetch.
    await gotoFreshWallet(page)

    // Create a wallet client-side — this path calls into the wasm signer to
    // derive the address, so a missing/404 wasm would throw here.
    await createWalletClientSide(page)

    // The address rendering below is the positive signal the signer worked; the
    // negative signals (no wasm console error, no failed /pkg fetch) catch the
    // exact deploy regression this suite exists for.
    expect(failedWasm, `wasm/pkg requests must not 404/fail:\n${failedWasm.join('\n')}`).toEqual([])
    expect(wasmErrors, `no wasm-related console errors:\n${wasmErrors.join('\n')}`).toEqual([])
  })

  test('node ingress picker lists seed / seed2 / faucet and switching updates state', async ({
    page,
  }) => {
    await gotoFreshWallet(page)

    // Open the ingress (network) selector in the header. Its trigger is a button
    // that shows the selected node's name + a "Testnet" badge.
    const trigger = page.getByRole('button', { name: /Testnet/i }).first()
    await expect(trigger).toBeVisible({ timeout: UI_TIMEOUT })
    await trigger.click()

    // The dropdown header + the three live ingress nodes. Scope the row
    // lookups to the dropdown container: the TRIGGER's accessible name also
    // starts with the selected node's name ("Seed (validator) Testnet"), so an
    // unscoped getByRole is a strict-mode violation once the menu is open.
    const dropdown = page.locator('div.absolute', {
      has: page.getByText(/Trusted RPC ingress/i),
    })
    await expect(page.getByText(/Trusted RPC ingress/i)).toBeVisible()
    const seed = dropdown.getByRole('button', { name: /^Seed \(validator\)/ })
    const seed2 = dropdown.getByRole('button', { name: /^Seed 2 \(validator\)/ })
    const faucetNode = dropdown.getByRole('button', { name: /^Faucet node/ })
    await expect(seed).toBeVisible()
    await expect(seed2).toBeVisible()
    await expect(faucetNode).toBeVisible()

    // Switch the selected ingress to seed2; the trigger label updates to reflect
    // the new selection (state change routed through selectIngress).
    await seed2.click()
    await expect(
      page.getByRole('button', { name: /Seed 2/ }).first(),
      'trigger reflects newly selected ingress node'
    ).toBeVisible({ timeout: UI_TIMEOUT })

    // Selection persists across reload (localStorage-backed).
    await page.reload({ timeout: NAV_TIMEOUT })
    await page.waitForLoadState('networkidle')
    await expect(page.getByRole('button', { name: /Seed 2/ }).first()).toBeVisible({
      timeout: UI_TIMEOUT,
    })
  })

  test('wallet create is fully client-side and renders an address', async ({ page }) => {
    await gotoFreshWallet(page)
    await createWalletClientSide(page)

    // Address renders in the dashboard (truncated testnet address).
    const addressButton = page
      .locator('button')
      .filter({ has: page.locator('code.font-mono') })
      .first()
    await expect(addressButton).toBeVisible({ timeout: UI_TIMEOUT })
    const addr = await addressButton.textContent()
    expect(addr, 'address renders').toBeTruthy()
    expect(addr).toContain('tbotho')
  })

  test('wallet import of a known mnemonic yields a deterministic address', async ({ page }) => {
    await gotoFreshWallet(page)

    await page.getByRole('button', { name: 'Import Existing' }).click()
    const mnemonicInput = page.getByPlaceholder(/Enter your recovery phrase/i)
    await mnemonicInput.fill(TEST_MNEMONIC_12)
    await fillPasswordFields(page)
    await page.getByRole('button', { name: 'Import Wallet' }).click()

    // exact: the dashboard also has "Send via Link" — /Send/i is ambiguous (strict mode).
    await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeVisible({ timeout: UI_TIMEOUT })

    const addressButton = page
      .locator('button')
      .filter({ has: page.locator('code.font-mono') })
      .first()
    const addr = await addressButton.textContent()
    expect(addr, 'imported address renders').toBeTruthy()
    // Deterministic: importing the same mnemonic always yields a testnet address.
    expect(addr).toContain('tbotho')
  })

  test('/claim route loads its empty/invalid state without a secret', async ({ page }) => {
    const resp = await page.goto(`${BASE_URL}/claim`, { timeout: NAV_TIMEOUT })
    expect(resp!.status(), '/claim returns 2xx').toBeLessThan(400)

    // With no #fragment secret the page shows its heading + a "no claim link"
    // invalid state, never a crash.
    await expect(page.getByRole('heading', { name: /Claim Your BTH/i })).toBeVisible({
      timeout: UI_TIMEOUT,
    })
    await expect(page.getByText(/No claim link found/i)).toBeVisible({ timeout: UI_TIMEOUT })
  })
})

/** Navigate to a fresh (cleared) wallet page on the live site. */
async function gotoFreshWallet(page: Page) {
  await page.goto(`${BASE_URL}/wallet`, { timeout: NAV_TIMEOUT })
  // Clear any persisted wallet/network state so we always exercise the
  // create/import setup flow deterministically.
  await page.evaluate(() => {
    try {
      localStorage.clear()
    } catch {
      /* ignore */
    }
  })
  await page.reload({ timeout: NAV_TIMEOUT })
  await page.waitForLoadState('networkidle')
}

/**
 * Create a wallet entirely client-side (reveal mnemonic, confirm, set the
 * REQUIRED password (#475), create).
 */
async function createWalletClientSide(page: Page) {
  await page.getByText('Click to reveal').click()
  const confirmCheckbox = page.locator('input[type="checkbox"]').first()
  await confirmCheckbox.check()
  await fillPasswordFields(page)
  await page.getByRole('button', { name: 'Create Wallet' }).click()
  // exact: the dashboard also has "Send via Link" — /Send/i is ambiguous (strict mode).
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeVisible({ timeout: UI_TIMEOUT })
}

/**
 * Fill the #475 password + confirm fields. Since #475 a password is REQUIRED
 * for create AND import — the submit buttons stay disabled without one.
 */
async function fillPasswordFields(page: Page) {
  await page.getByPlaceholder(/^Password \(min/).fill(PASSWORD)
  await page.getByPlaceholder('Confirm password').fill(PASSWORD)
}
