/**
 * Shared wallet-bootstrap helpers for the e2e specs (#479).
 *
 * Post-#475 the wallet is ENCRYPTED BY DEFAULT: creating or importing a wallet
 * REQUIRES a password (>= MIN_PASSWORD_LENGTH chars), there is no plaintext
 * opt-out, and the "Create Wallet" / "Import Wallet" buttons stay disabled until
 * a valid, matching password is entered. Every flow that needs an unlocked
 * wallet on the dashboard goes through here so the password handling lives in one
 * place and the request→pay / share-address / contacts specs can focus on the
 * feature under test.
 *
 * These specs run against the HERMETIC mock RPC (e2e/serve-rpc-mock.mjs) the rest
 * of the default suite uses — no live node or faucet. The mock answers the
 * connect handshake + reads deterministically; it has NO `tx_submit`, so these
 * specs assert the PRE-FILL + CONFIRM UI (recipient/amount populated, pay button
 * primed) rather than on-chain settlement. The full-stack send path (real node)
 * is covered separately by tests/fullstack/send.spec.ts.
 */
import type { Page, BrowserContext } from '@playwright/test'
import { URLS, TIMEOUTS } from './test-data'

/** A password that satisfies the #475 minimum (>= 8 chars). */
export const E2E_PASSWORD = 'e2e-password-123'

/** Reset the wallet origin to a clean, no-wallet state. */
export async function resetWalletStorage(page: Page, context: BrowserContext): Promise<void> {
  await context.clearCookies()
  await page.goto(URLS.WALLET, { timeout: TIMEOUTS.PAGE_LOAD })
  await page.evaluate(() => localStorage.clear())
  await page.reload()
  await page.waitForLoadState('networkidle')
}

/**
 * Create a fresh wallet through the real UI (reveal mnemonic -> confirm ->
 * password) and land on the dashboard. The wallet's keys/contacts live under the
 * resulting in-session vault key, so contact add/edit/delete works afterwards.
 *
 * Assumes the page is already on a clean /wallet (call {@link resetWalletStorage}
 * first, or use {@link createWalletOnDashboard}).
 */
export async function completeCreateWallet(page: Page, password = E2E_PASSWORD): Promise<void> {
  // Reveal the mnemonic + tick the "I wrote it down" confirmation.
  await page.getByText('Click to reveal').click()
  await page.locator('input[type="checkbox"]').first().check()

  // #475: a password is mandatory. Fill both password + confirm fields.
  await page.getByPlaceholder(/^Password \(min/).fill(password)
  await page.getByPlaceholder('Confirm password').fill(password)

  await page.getByRole('button', { name: 'Create Wallet' }).click()

  // Dashboard up.
  await page.getByRole('button', { name: /^Send$/i }).waitFor({ state: 'visible', timeout: TIMEOUTS.WALLET_SYNC })
}

/** Reset storage and create a fresh password-protected wallet on the dashboard. */
export async function createWalletOnDashboard(
  page: Page,
  context: BrowserContext,
  password = E2E_PASSWORD,
): Promise<void> {
  await resetWalletStorage(page, context)
  await completeCreateWallet(page, password)
}

/**
 * Import a known mnemonic through the real UI with a password, landing on the
 * dashboard. Useful when a deterministic address is needed (e.g. self-pay
 * checks). Assumes the page is on a clean /wallet.
 */
export async function completeImportWallet(
  page: Page,
  mnemonic: string,
  password = E2E_PASSWORD,
): Promise<void> {
  await page.getByRole('button', { name: 'Import Existing' }).click()
  await page.getByPlaceholder(/Enter your recovery phrase/i).fill(mnemonic)
  await page.getByPlaceholder(/^Password \(min/).fill(password)
  await page.getByPlaceholder('Confirm password').fill(password)
  await page.getByRole('button', { name: 'Import Wallet' }).click()
  await page.getByRole('button', { name: /^Send$/i }).waitFor({ state: 'visible', timeout: TIMEOUTS.WALLET_SYNC })
}

/**
 * Open a `/pay#…` link as the payer and get to the pay confirmation.
 *
 * Navigating to the link is a FULL page load, so the in-memory session vault key
 * is dropped and the pay page shows its WalletGate (no unlocked wallet in this
 * fresh document). The gate lets the visitor unlock / create / import in-flow,
 * with the parsed request preserved; once a wallet is ready the pre-filled pay
 * confirmation appears. We drive the gate here so the specs can focus on the
 * pre-fill assertions.
 *
 * By default we CREATE a fresh payer wallet in-flow (the realistic payer ≠ payee
 * case, and the gate's create path renders deterministically). Pass
 * `unlockPassword` to instead unlock the SAME wallet (payer == payee / self-pay).
 */
export async function openPayLinkAsPayer(
  page: Page,
  payLink: string,
  opts: { unlockPassword?: string } = {},
): Promise<void> {
  const payPath = payLink.slice(payLink.indexOf('/pay#'))
  await page.goto(payPath, { timeout: TIMEOUTS.PAGE_LOAD })
  await page.waitForLoadState('networkidle')

  await page.getByRole('heading', { name: /Send a Payment/i }).waitFor({
    state: 'visible',
    timeout: TIMEOUTS.WALLET_SYNC,
  })

  // Already at the pay confirmation (e.g. wallet still unlocked)? Done.
  const amountField = page.getByPlaceholder('0.00')
  if (await amountField.isVisible().catch(() => false)) return

  // Prefer unlocking the existing wallet when a password is supplied AND the
  // unlock field is present.
  const unlockField = page.getByPlaceholder('Enter password')
  if (opts.unlockPassword && (await unlockField.isVisible().catch(() => false))) {
    await unlockField.fill(opts.unlockPassword)
    await page.getByRole('button', { name: /^Unlock$/i }).click()
  } else {
    // Otherwise create a fresh payer wallet in-flow via the gate's create path.
    await page.getByText('Click to reveal').click()
    await page
      .getByRole('checkbox', { name: /written down my recovery phrase/i })
      .check()
    await page.getByRole('button', { name: /Create .* Continue/i }).click()
  }

  await amountField.waitFor({ state: 'visible', timeout: TIMEOUTS.WALLET_SYNC })
}

/** Read the wallet's own address off the dashboard balance card (truncated). */
export async function readDashboardAddress(page: Page): Promise<string> {
  const addressButton = page.locator('button').filter({ has: page.locator('code.font-mono') }).first()
  return (await addressButton.textContent())?.trim() ?? ''
}

/**
 * Open the Send modal from the dashboard. Assumes an unlocked wallet (the
 * dashboard's "Send" button is visible).
 */
export async function openSendModal(page: Page): Promise<void> {
  await page.getByRole('button', { name: /^Send$/i }).click()
  await page.getByRole('heading', { name: /Send BTH/i }).waitFor({
    state: 'visible',
    timeout: TIMEOUTS.WALLET_SYNC,
  })
}

/**
 * Lock the wallet from the dashboard header (#490). Clicks the header Lock
 * button (the icon-only button, disabled for plaintext wallets — but every
 * wallet created via the helpers is encrypted) and waits for the Unlock screen.
 */
export async function lockWallet(page: Page): Promise<void> {
  // The header has an icon-only Lock button (title "Lock wallet"). Click it and
  // wait for the unlock view to replace the dashboard.
  await page.getByRole('button', { name: 'Lock wallet' }).first().click()
  await page.getByRole('heading', { name: /Unlock Wallet/i }).waitFor({
    state: 'visible',
    timeout: TIMEOUTS.WALLET_SYNC,
  })
}

/** Unlock the wallet from the Unlock screen with `password` (#490). */
export async function unlockWallet(page: Page, password = E2E_PASSWORD): Promise<void> {
  await page.getByPlaceholder('Enter password').fill(password)
  await page.getByRole('button', { name: /^Unlock$/i }).click()
  await page.getByRole('button', { name: /^Send$/i }).waitFor({
    state: 'visible',
    timeout: TIMEOUTS.WALLET_SYNC,
  })
}

/**
 * Change the wallet's password via the dashboard "Change password" flow (#489).
 * Opens the password settings modal in change mode, fills current + new + confirm
 * and submits. Resolves once the modal closes (success).
 */
export async function changeWalletPassword(
  page: Page,
  oldPassword: string,
  newPassword: string,
): Promise<void> {
  // The dashboard "Change password" trigger and the modal's submit button share
  // the same label, so scope the modal interactions to the modal overlay.
  await page.getByRole('button', { name: /Change password/i }).first().click()
  const modal = page.locator('div.fixed.inset-0').filter({
    has: page.getByRole('heading', { name: /Change password/i }),
  })
  await modal.getByRole('heading', { name: /Change password/i }).waitFor({ state: 'visible' })
  await modal.getByPlaceholder('Current password').fill(oldPassword)
  await modal.getByPlaceholder(/^Password \(min/).fill(newPassword)
  await modal.getByPlaceholder('Confirm password').fill(newPassword)
  await modal.getByRole('button', { name: /^Change password$/i }).click()
}
