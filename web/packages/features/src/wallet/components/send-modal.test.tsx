/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup } from '@testing-library/react'
import type { Balance } from '@botho/core'
import { deriveAddress } from '@botho/core'
import { SendModal } from './send-modal'

// A couple of real, valid testnet addresses derived from fixed mnemonics. Using
// the real deriver keeps the test honest about what the production validator
// (`@botho/core`'s isValidAddress) accepts.
const VALID_ADDRESS = deriveAddress(
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
)
const OTHER_ADDRESS = deriveAddress(
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
    estimateFee: vi.fn().mockResolvedValue(4000n),
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
