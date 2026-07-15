/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup } from '@testing-library/react'
import type { Balance } from '@botho/core'
import { deriveDefaultSubaddressPublicKeys, formatAddress } from '@botho/core'
import { SendModal } from './send-modal'

// A couple of real, valid testnet v2 addresses derived from fixed mnemonics.
// Using the real classical deriver + shared v2 codec keeps the test honest about
// what the production validator (`@botho/core`'s isValidAddress) accepts. The
// post-quantum bytes are deterministic placeholders of the correct v2 lengths
// (the real ML-KEM/ML-DSA derivation lives in @botho/wasm-signer, which needs
// wasm); `isValidAddress` only length-checks those fields.
function testAddress(mnemonic: string): string {
  const { viewPublic, spendPublic } = deriveDefaultSubaddressPublicKeys(mnemonic, 0)
  return formatAddress(viewPublic, spendPublic, new Uint8Array(1184), new Uint8Array(1952), 'testnet')
}
const VALID_ADDRESS = testAddress(
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
)
const OTHER_ADDRESS = testAddress(
  'legal winner thank year wave sausage worth useful legal winner thank yellow',
)

const BALANCE: Balance = {
  available: 1_000_000_000_000_000n,
  pending: 0n,
  total: 1_000_000_000_000_000n,
}

function setup(overrides: Partial<React.ComponentProps<typeof SendModal>> = {}) {
  const onSend = vi.fn().mockResolvedValue({ success: true, txHash: '0xabc' })
  const props: React.ComponentProps<typeof SendModal> = {
    isOpen: true,
    onClose: vi.fn(),
    balance: BALANCE,
    estimateFee: vi.fn().mockResolvedValue({ fee: 4000n, clusterFactorDisplay: '1.00x' }),
    onSend,
    ...overrides,
  }
  render(<SendModal {...props} />)
  return { onSend, props }
}

function getRecipientInput(): HTMLInputElement {
  return screen.getByPlaceholderText(/tbotho:\/\//i) as HTMLInputElement
}

function getAmountInput(): HTMLInputElement {
  return screen.getByPlaceholderText('0.00') as HTMLInputElement
}

function getSubmitButton(): HTMLButtonElement {
  return screen.getByRole('button', { name: /Send Transaction/i }) as HTMLButtonElement
}

describe('SendModal address validation', () => {
  beforeEach(() => cleanup())

  it('does not show an error for an empty recipient, but keeps submit disabled', () => {
    setup()
    expect(screen.queryByText('Invalid Botho address')).toBeNull()
    expect(getSubmitButton().disabled).toBe(true)
  })

  it('shows an inline error and disables submit for a malformed address', () => {
    setup()
    fireEvent.change(getRecipientInput(), { target: { value: 'not-an-address' } })
    fireEvent.change(getAmountInput(), { target: { value: '1' } })

    expect(screen.getByText('Invalid Botho address')).toBeDefined()
    expect(getSubmitButton().disabled).toBe(true)
  })

  it('does not block a valid address', async () => {
    const { onSend } = setup()
    fireEvent.change(getRecipientInput(), { target: { value: VALID_ADDRESS } })
    fireEvent.change(getAmountInput(), { target: { value: '1' } })

    expect(screen.queryByText('Invalid Botho address')).toBeNull()
    const submit = getSubmitButton()
    expect(submit.disabled).toBe(false)

    fireEvent.click(submit)
    // onSend is invoked with the valid recipient.
    expect(onSend).toHaveBeenCalledTimes(1)
    expect(onSend.mock.calls[0][0].recipient).toBe(VALID_ADDRESS)
  })

  it('clicking submit with an invalid address surfaces the error and does not call onSend', () => {
    const { onSend } = setup()
    fireEvent.change(getRecipientInput(), { target: { value: 'garbage' } })
    fireEvent.change(getAmountInput(), { target: { value: '1' } })

    // Button is disabled, but guard handleSend defensively too.
    fireEvent.click(getSubmitButton())
    expect(onSend).not.toHaveBeenCalled()
  })
})

describe('SendModal self-send guard', () => {
  beforeEach(() => cleanup())

  it('warns and blocks when recipient equals the user own address', () => {
    const { onSend } = setup({ ownAddress: VALID_ADDRESS })
    fireEvent.change(getRecipientInput(), { target: { value: VALID_ADDRESS } })
    fireEvent.change(getAmountInput(), { target: { value: '1' } })

    expect(screen.getByText(/your own address/i)).toBeDefined()
    expect(getSubmitButton().disabled).toBe(true)

    fireEvent.click(getSubmitButton())
    expect(onSend).not.toHaveBeenCalled()
  })

  it('does not flag a self-send for a different recipient', () => {
    setup({ ownAddress: VALID_ADDRESS })
    fireEvent.change(getRecipientInput(), { target: { value: OTHER_ADDRESS } })
    fireEvent.change(getAmountInput(), { target: { value: '1' } })

    expect(screen.queryByText(/your own address/i)).toBeNull()
    expect(getSubmitButton().disabled).toBe(false)
  })

  it('does not run the self-send check when ownAddress is not provided', () => {
    setup()
    fireEvent.change(getRecipientInput(), { target: { value: VALID_ADDRESS } })
    fireEvent.change(getAmountInput(), { target: { value: '1' } })

    expect(screen.queryByText(/your own address/i)).toBeNull()
    expect(getSubmitButton().disabled).toBe(false)
  })
})

describe('SendModal dismissal (#655)', () => {
  beforeEach(() => cleanup())

  function getBackdrop(): HTMLElement {
    return screen.getByRole('dialog')
  }

  it('closes on Escape', () => {
    const { props } = setup()
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(props.onClose).toHaveBeenCalledTimes(1)
  })

  it('closes on backdrop click', () => {
    const { props } = setup()
    const backdrop = getBackdrop()
    fireEvent.mouseDown(backdrop)
    fireEvent.click(backdrop)
    expect(props.onClose).toHaveBeenCalledTimes(1)
  })

  it('does not close on a click inside the panel', () => {
    const { props } = setup()
    const input = getAmountInput()
    fireEvent.mouseDown(input)
    fireEvent.click(input)
    expect(props.onClose).not.toHaveBeenCalled()
  })

  it('does not close when a text-selection drag ends over the backdrop', () => {
    const { props } = setup()
    fireEvent.mouseDown(getAmountInput())
    fireEvent.click(getBackdrop())
    expect(props.onClose).not.toHaveBeenCalled()
  })

  it('suppresses backdrop and Escape dismissal while a send is in flight', () => {
    const { props } = setup({ isSending: true })
    const backdrop = getBackdrop()
    fireEvent.mouseDown(backdrop)
    fireEvent.click(backdrop)
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(props.onClose).not.toHaveBeenCalled()
  })
})

describe('SendModal cluster fee factor display (#635)', () => {
  beforeEach(() => cleanup())

  it('shows the progressive-rate row and multiplier when the factor is above 1.00x', async () => {
    setup({
      estimateFee: vi
        .fn()
        .mockResolvedValue({ fee: 20240n, clusterFactorDisplay: '1.85x' }),
    })
    // Entering an amount triggers the estimateFee effect, which resolves with
    // the above-base factor.
    fireEvent.change(getAmountInput(), { target: { value: '10' } })

    // The multiplier and its "Progressive rate" label render...
    expect(await screen.findByText('1.85x')).toBeDefined()
    expect(screen.getByText('Progressive rate')).toBeDefined()
    // ...along with the one-line why-explanation.
    expect(screen.getByText(/progressive fee/i)).toBeDefined()
    // ...and the base-rate copy is NOT shown.
    expect(screen.queryByText(/no cluster-wealth premium/i)).toBeNull()
  })

  it('shows the base-rate copy and no progressive row at 1.00x', async () => {
    // The default setup mock resolves with clusterFactorDisplay: '1.00x'.
    setup()
    fireEvent.change(getAmountInput(), { target: { value: '10' } })

    // Give the estimateFee effect a chance to resolve, then assert the base-rate
    // copy is present and the progressive row is absent.
    expect(await screen.findByText(/no cluster-wealth premium/i)).toBeDefined()
    expect(screen.queryByText('Progressive rate')).toBeNull()
  })
})
