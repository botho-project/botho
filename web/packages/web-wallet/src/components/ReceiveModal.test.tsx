/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'
import { ReceiveModal } from './ReceiveModal'

const TEST_ADDRESS = 'btho1qexampleaddress000000000000000000000000000deadbeef'

// Mock the wallet context so the component is self-contained.
const useWalletMock = vi.fn()
vi.mock('../contexts/wallet', () => ({
  useWallet: () => useWalletMock(),
}))

describe('ReceiveModal', () => {
  beforeEach(() => {
    cleanup()
    useWalletMock.mockReset()
    useWalletMock.mockReturnValue({ address: TEST_ADDRESS })
  })

  it('renders nothing when closed', () => {
    const { container } = render(<ReceiveModal isOpen={false} onClose={() => {}} />)
    expect(container.childElementCount).toBe(0)
  })

  it('shows the address and a scannable QR when open', () => {
    render(<ReceiveModal isOpen onClose={() => {}} />)

    // The full raw address is shown in the read-only field.
    const field = screen.getByDisplayValue(TEST_ADDRESS)
    expect(field).toBeDefined()

    // qrcode.react renders an <svg>; assert one is present (the QR).
    const qr = document.querySelector('svg[aria-label="Receiving address QR code"]')
    expect(qr).not.toBeNull()
  })

  it('encodes the raw address in the QR (not a /pay link)', () => {
    render(<ReceiveModal isOpen onClose={() => {}} />)
    const field = screen.getByDisplayValue(TEST_ADDRESS) as HTMLInputElement
    // Raw address — distinct from RequestModal's /pay#… share link.
    expect(field.value).toBe(TEST_ADDRESS)
    expect(field.value).not.toContain('/pay')
  })

  it('prompts to unlock when there is no wallet', () => {
    useWalletMock.mockReturnValue({ address: null })
    render(<ReceiveModal isOpen onClose={() => {}} />)
    expect(screen.getByText(/Unlock or create a wallet/i)).toBeDefined()
  })
})
