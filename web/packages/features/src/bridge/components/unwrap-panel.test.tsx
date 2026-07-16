/**
 * @vitest-environment jsdom
 *
 * Unwrap panel (#1032). The bridge RELEASE-order API is mocked — there is no
 * running bridge service in unit tests, and (per the epic #1029 custody model)
 * the wBTH BURN happens in the user's OWN counterparty wallet, which is external
 * to this code entirely. So the contract under test is the wiring: the wallet
 * surfaces the Botho release destination + chain-aware burn guidance, opens a
 * release order via the injected client, tracks it to `released`, and confirms
 * the BTH arrived. NO EVM/Solana signing happens here.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { UnwrapPanel } from './unwrap-panel'
import type { ReleaseOrder, Translate, UnwrapController } from '../types'

// Passthrough translator: returns the key so assertions are locale-independent.
const t: Translate = (key) => key

const RELEASE_ADDR = 'tbotho://2/releasedestinationaddress'

const BURN_DETECTED: ReleaseOrder = {
  id: '22222222-2222-2222-2222-222222222222',
  status: 'burn_detected',
  sourceChain: 'ethereum',
  bthAddress: RELEASE_ADDR,
  amount: '5000000000000',
  fee: '100000000',
  tokenAddress: '0x49b985ec427ee771a601f11b18f7d4402fa2dd7b',
  sourceTx: '0xburntxhash',
  destTx: null,
  expiresAt: 1_760_000_000,
  failureReason: null,
}

const RELEASED: ReleaseOrder = {
  ...BURN_DETECTED,
  status: 'released',
  destTx: 'btcreleasetxhash',
}

function makeController(over: Partial<UnwrapController> = {}): UnwrapController {
  return {
    client: {
      createReleaseOrder: vi.fn(async () => BURN_DETECTED),
      getReleaseOrderStatus: vi.fn(async () => BURN_DETECTED),
    },
    network: 'testnet',
    wallet: {
      hasWallet: true,
      isLocked: false,
      releaseAddress: RELEASE_ADDR,
    },
    requestWallet: vi.fn(),
    ...over,
  }
}

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('UnwrapPanel gate states', () => {
  it('prompts to open a wallet when none exists', () => {
    const controller = makeController({
      wallet: { hasWallet: false, isLocked: false, releaseAddress: null },
    })
    render(<UnwrapPanel t={t} controller={controller} />)
    expect(screen.getByText('unwrap.panel.noWallet.title')).toBeTruthy()
  })

  it('prompts to unlock when no release address is available', () => {
    const controller = makeController({
      wallet: { hasWallet: true, isLocked: true, releaseAddress: null },
    })
    render(<UnwrapPanel t={t} controller={controller} />)
    expect(screen.getByText('unwrap.panel.locked.title')).toBeTruthy()
  })
})

describe('UnwrapPanel destination + burn guidance', () => {
  it('shows the Botho release address and the bridgeBurn guidance for the source chain', () => {
    render(<UnwrapPanel t={t} controller={makeController()} />)
    // The release destination is the wallet's own address (reused, not signed).
    const dest = screen.getByDisplayValue(RELEASE_ADDR)
    expect(dest).toBeTruthy()
    // The wBTH token to burn + the burn call are surfaced.
    expect(screen.getByDisplayValue('0x49b985ec427ee771a601f11b18f7d4402fa2dd7b')).toBeTruthy()
    expect(screen.getByText(/bridgeBurn\(/)).toBeTruthy()
    // A deep-link to where the user executes the burn themselves.
    expect(screen.getByText('unwrap.panel.form.openApp')).toBeTruthy()
  })

  it('keeps destination + guidance visible but disables tracking when no client is wired', () => {
    render(<UnwrapPanel t={t} controller={makeController({ client: null })} />)
    // Destination + guidance still render (they need no backend)…
    expect(screen.getByDisplayValue(RELEASE_ADDR)).toBeTruthy()
    expect(screen.getByText(/bridgeBurn\(/)).toBeTruthy()
    // …but the live-tracking action degrades to an explicit notice.
    expect(screen.getByText('unwrap.panel.form.notConfigured.title')).toBeTruthy()
    expect(screen.queryByRole('button', { name: /unwrap\.panel\.form\.track/ })).toBeNull()
  })
})

describe('UnwrapPanel release tracking', () => {
  it('blocks track until an amount is entered', () => {
    render(<UnwrapPanel t={t} controller={makeController()} />)
    const track = screen.getByRole('button', { name: /unwrap\.panel\.form\.track/ })
    expect((track as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(screen.getByPlaceholderText('0.00'), { target: { value: '5' } })
    expect((track as HTMLButtonElement).disabled).toBe(false)
  })

  it('opens a release order with the release address, tracks it to released', async () => {
    vi.useFakeTimers()
    const controller = makeController()
    render(<UnwrapPanel t={t} controller={controller} />)

    fireEvent.change(screen.getByPlaceholderText('0.00'), { target: { value: '5' } })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /unwrap\.panel\.form\.track/ }))
      await vi.advanceTimersByTimeAsync(0)
    })

    // Order was opened with the source chain, the wallet's release address, and
    // the picocredit amount — no counterparty-chain signing involved.
    expect(controller.client!.createReleaseOrder).toHaveBeenCalledWith({
      sourceChain: 'ethereum',
      bthAddress: RELEASE_ADDR,
      amount: '5000000000000',
    })

    // Now tracking: the order id and the burn-detected step are shown.
    expect(screen.getByText(BURN_DETECTED.id)).toBeTruthy()

    // Advance the poll; the next status is `released`.
    ;(controller.client!.getReleaseOrderStatus as ReturnType<typeof vi.fn>).mockResolvedValue(
      RELEASED,
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(8_000)
    })

    // Receive confirmation: released → check your wallet balance.
    expect(screen.getByText('unwrap.panel.released.title')).toBeTruthy()
  })
})
