/**
 * Send-flow e2e (#492, feature #491 address validation).
 *
 * Drives the Send modal through the real UI against the hermetic mock RPC:
 *   - an INVALID recipient address is blocked with an inline error and keeps the
 *     Send button disabled;
 *   - a VALID recipient + amount primes the confirm (Send button enabled) and the
 *     entered values stick.
 *
 * The mock RPC has NO `tx_submit`, so — like the request->pay spec — we assert up
 * to the PRIMED/CONFIRM state rather than on-chain settlement. The real submit
 * path is covered by tests/fullstack/send.spec.ts against a live local node.
 */
import { test, expect } from '@playwright/test'
import { deriveAddress } from '@botho/core'
import { TEST_MNEMONIC_24 } from '../../fixtures/test-data'
import { createWalletOnDashboard, openSendModal } from '../../fixtures/wallet-setup'

// A valid, deterministic testnet recipient (derived, so it stays correct if the
// address format changes).
const VALID_RECIPIENT = deriveAddress(TEST_MNEMONIC_24, 'testnet')

test.describe('Send flow', () => {
  test('blocks an invalid recipient address with an inline error', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)
    await openSendModal(page)

    const recipient = page.getByPlaceholder(/tbotho:\/\/1\/\.\.\. or search contacts/i)
    await recipient.fill('not-a-valid-address')

    // Inline validation error appears for a malformed address.
    await expect(page.getByText(/Invalid Botho address/i)).toBeVisible()

    // The Send button stays disabled while the recipient is invalid.
    await expect(page.getByRole('button', { name: /Send Transaction/i })).toBeDisabled()
  })

  test('a valid recipient + amount primes the confirm', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)
    await openSendModal(page)

    const recipient = page.getByPlaceholder(/tbotho:\/\/1\/\.\.\. or search contacts/i)
    await recipient.fill(VALID_RECIPIENT)

    // No inline error for a valid address.
    await expect(page.getByText(/Invalid Botho address/i)).toHaveCount(0)

    // Enter an amount.
    await page.getByPlaceholder('0.00').first().fill('1.5')

    // The recipient + amount stick, and the Send button is primed (enabled).
    await expect(recipient).toHaveValue(VALID_RECIPIENT)
    await expect(page.getByPlaceholder('0.00').first()).toHaveValue('1.5')
    await expect(page.getByRole('button', { name: /Send Transaction/i })).toBeEnabled()
  })
})
