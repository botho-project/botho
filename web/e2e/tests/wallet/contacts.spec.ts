/**
 * Contacts e2e (#479, feature #472).
 *
 * Covers the /contacts manager and the Send-form contact picker against the
 * hermetic mock RPC:
 *   - add / edit / delete a contact
 *   - search filters by name + address
 *   - an unnamed ("previously paid"-style) entry can be labeled
 *   - the Send modal's contact picker filters and fills the recipient
 *
 * NOTE on auto-record: addresses are auto-recorded into the address book inside
 * the real `send()` path, AFTER a successful on-chain `tx_submit`. The hermetic
 * mock RPC has no `tx_submit`, so a real spend can't complete here; the
 * auto-record-on-send behavior is covered by unit tests (wallet context /
 * AddressBook). This spec validates the same downstream UX — an address that
 * lands in the book unnamed can be labeled — by adding it via the UI, plus the
 * full add/edit/delete/search CRUD and the Send picker.
 */
import { test, expect } from '@playwright/test'
import { deriveAddress } from '@botho/core'
import { TEST_MNEMONIC_12, TEST_MNEMONIC_24, TIMEOUTS } from '../../fixtures/test-data'
import { createWalletOnDashboard } from '../../fixtures/wallet-setup'

// Two valid, deterministic testnet addresses to use as contacts. Deriving them
// (rather than hardcoding) keeps the spec correct if the address format changes.
const ALICE_ADDRESS = deriveAddress(TEST_MNEMONIC_12, 'testnet')
const BOB_ADDRESS = deriveAddress(TEST_MNEMONIC_24, 'testnet')

/** Open /contacts on an already-unlocked wallet (same SPA, in-session vault). */
async function gotoContacts(page: import('@playwright/test').Page): Promise<void> {
  await page.getByRole('link', { name: /Contacts/i }).click()
  await expect(page.getByRole('heading', { name: 'Contacts' })).toBeVisible({
    timeout: TIMEOUTS.WALLET_SYNC,
  })
}

/** Add a contact via the editor modal. Leave name blank for an unnamed entry. */
async function addContact(
  page: import('@playwright/test').Page,
  { name, address, notes }: { name?: string; address: string; notes?: string },
): Promise<void> {
  await page.getByRole('button', { name: /^Add$/i }).click()
  await expect(page.getByRole('heading', { name: 'Add contact' })).toBeVisible()
  if (name) await page.getByPlaceholder('e.g. Alice').fill(name)
  await page.getByPlaceholder('tbotho://1/…').fill(address)
  if (notes) await page.getByPlaceholder(/Anything to remember/i).fill(notes)
  await page.getByRole('button', { name: 'Add contact' }).click()
  await expect(page.getByRole('heading', { name: 'Add contact' })).toBeHidden()
}

test.describe('Contacts', () => {
  test('add, edit, delete, and search contacts', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)
    await gotoContacts(page)

    // Empty state.
    await expect(page.getByText(/No contacts yet/i)).toBeVisible()

    // --- Add two contacts -------------------------------------------------
    await addContact(page, { name: 'Alice', address: ALICE_ADDRESS, notes: 'coffee fund' })
    await addContact(page, { name: 'Bob', address: BOB_ADDRESS })

    await expect(page.getByText('Alice', { exact: true })).toBeVisible()
    await expect(page.getByText('Bob', { exact: true })).toBeVisible()

    // --- Search filters by name -------------------------------------------
    const search = page.getByPlaceholder(/Search by name or address/i)
    await search.fill('alice')
    await expect(page.getByText('Alice', { exact: true })).toBeVisible()
    await expect(page.getByText('Bob', { exact: true })).toBeHidden()

    // --- Search filters by address ----------------------------------------
    await search.fill(BOB_ADDRESS.slice(-8))
    await expect(page.getByText('Bob', { exact: true })).toBeVisible()
    await expect(page.getByText('Alice', { exact: true })).toBeHidden()
    await search.fill('')

    // --- Edit a contact (rename + notes) ----------------------------------
    await page.getByText('Alice', { exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Edit contact' })).toBeVisible()
    const nameField = page.getByPlaceholder('e.g. Alice')
    await nameField.fill('Alice Cooper')
    await page.getByRole('button', { name: 'Save changes' }).click()
    await expect(page.getByRole('heading', { name: 'Edit contact' })).toBeHidden()
    await expect(page.getByText('Alice Cooper', { exact: true })).toBeVisible()

    // --- Delete a contact -------------------------------------------------
    await page.getByText('Bob', { exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Edit contact' })).toBeVisible()
    await page.getByRole('button', { name: /Delete contact/i }).click()
    // Confirm the destructive action.
    await page.getByRole('button', { name: /^Delete$/i }).click()
    await expect(page.getByRole('heading', { name: 'Edit contact' })).toBeHidden()
    await expect(page.getByText('Bob', { exact: true })).toBeHidden()
    await expect(page.getByText('Alice Cooper', { exact: true })).toBeVisible()
  })

  test('an unnamed contact can be labeled', async ({ page, context }) => {
    await createWalletOnDashboard(page, context)
    await gotoContacts(page)

    // Add an UNNAMED entry — this mirrors the auto-recorded "previously paid"
    // state a recipient lands in after a real send (see the file header note).
    await addContact(page, { address: ALICE_ADDRESS })

    // It renders as the "Unnamed — tap to label" affordance.
    const unnamed = page.getByText(/Unnamed — tap to label/i)
    await expect(unnamed).toBeVisible()

    // Tap to label it.
    await unnamed.click()
    await expect(page.getByRole('heading', { name: 'Edit contact' })).toBeVisible()
    await page.getByPlaceholder('e.g. Alice').fill('Labeled Later')
    await page.getByRole('button', { name: 'Save changes' }).click()
    await expect(page.getByRole('heading', { name: 'Edit contact' })).toBeHidden()

    await expect(page.getByText('Labeled Later', { exact: true })).toBeVisible()
    await expect(page.getByText(/Unnamed — tap to label/i)).toBeHidden()
  })

  test('the Send modal contact picker filters and fills the recipient', async ({
    page,
    context,
  }) => {
    await createWalletOnDashboard(page, context)

    // Seed two named contacts via the contacts page, then return to the wallet.
    await gotoContacts(page)
    await addContact(page, { name: 'Alice', address: ALICE_ADDRESS })
    await addContact(page, { name: 'Bob', address: BOB_ADDRESS })
    await page.getByRole('link', { name: /Botho Wallet|Wallet/ }).first().click()
    await page.getByRole('button', { name: /^Send$/i }).waitFor({ state: 'visible' })

    // Open the Send modal and focus the recipient field to reveal the picker.
    await page.getByRole('button', { name: /^Send$/i }).click()
    await expect(page.getByRole('heading', { name: /Send BTH/i })).toBeVisible()

    const recipient = page.getByPlaceholder(/search contacts/i)
    await recipient.click()
    await recipient.fill('alice')

    // The picker filters to Alice; clicking her fills the recipient with her
    // full address.
    const pickerAlice = page.getByRole('button', { name: /Alice/ }).last()
    await expect(pickerAlice).toBeVisible()
    await pickerAlice.click()

    await expect(recipient).toHaveValue(ALICE_ADDRESS)
    // The matched-contact name is surfaced inline once the address is filled.
    await expect(page.getByText('Alice', { exact: true })).toBeVisible()
  })
})
