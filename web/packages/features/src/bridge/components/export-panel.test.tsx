/**
 * @vitest-environment jsdom
 *
 * Integrated export panel (#1031). The bridge order API and the wallet send
 * path are BOTH mocked — there is no running bridge service or wasm signer in
 * unit tests — so the contract under test is the wiring: the form opens an
 * order, the BTH deposit is submitted via the injected `submitDeposit` (the
 * page wires this to the wallet's real wasm-signer `send()`), the order is
 * tracked to `completed`, and the "Trade wBTH now" hand-off fires.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { ExportPanel } from './export-panel'
import type { ExportController, MintOrder, Translate } from '../types'

// Passthrough translator: returns the key so assertions are locale-independent.
const t: Translate = (key) => key

const EVM_ADDR = '0x49b985ec427ee771a601f11b18f7d4402fa2dd7b'

const AWAITING: MintOrder = {
  id: '11111111-1111-1111-1111-111111111111',
  status: 'awaiting_deposit',
  destChain: 'ethereum',
  destAddress: EVM_ADDR,
  amount: '5000000000000',
  fee: '100000000',
  depositAddress: 'tbotho://1/reservedeposit',
  memo: 'deadbeefdeadbeefdeadbeefdeadbeef',
  destTx: null,
  expiresAt: 1_760_000_000,
  failureReason: null,
}

const COMPLETED: MintOrder = {
  ...AWAITING,
  status: 'completed',
  destTx: '0xminttxhash',
}

function makeController(over: Partial<ExportController> = {}): ExportController {
  return {
    client: {
      createMintOrder: vi.fn(async () => AWAITING),
      getOrderStatus: vi.fn(async () => AWAITING),
    },
    network: 'testnet',
    wallet: {
      hasWallet: true,
      isLocked: false,
      spendableBalance: 100n * 10n ** 12n, // 100 BTH in picocredits
    },
    submitDeposit: vi.fn(async () => '0xdeposittx'),
    requestWallet: vi.fn(),
    ...over,
  }
}

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('ExportPanel gate states', () => {
  it('renders a "not configured" notice when no client is wired', () => {
    render(<ExportPanel t={t} controller={makeController({ client: null })} />)
    expect(screen.getByText('export.panel.notConfigured.title')).toBeTruthy()
  })

  it('prompts to unlock when the wallet is locked', () => {
    const controller = makeController({
      wallet: { hasWallet: true, isLocked: true, spendableBalance: null },
    })
    render(<ExportPanel t={t} controller={controller} />)
    expect(screen.getByText('export.panel.locked.title')).toBeTruthy()
  })

  it('prompts to open a wallet when none exists', () => {
    const controller = makeController({
      wallet: { hasWallet: false, isLocked: false, spendableBalance: null },
    })
    render(<ExportPanel t={t} controller={controller} />)
    expect(screen.getByText('export.panel.noWallet.title')).toBeTruthy()
  })
})

describe('ExportPanel export flow', () => {
  it('blocks submit on an invalid destination address', () => {
    render(<ExportPanel t={t} controller={makeController()} />)
    const addr = screen.getByPlaceholderText('export.panel.form.addressPlaceholder.ethereum')
    fireEvent.change(addr, { target: { value: '0xnothex' } })
    fireEvent.change(screen.getByPlaceholderText('0.00'), { target: { value: '5' } })
    expect(screen.getByText('export.panel.form.addressInvalid')).toBeTruthy()
    const submit = screen.getByRole('button', { name: /export\.panel\.form\.submit/ })
    expect((submit as HTMLButtonElement).disabled).toBe(true)
  })

  it('flags an amount over the spendable balance', () => {
    render(<ExportPanel t={t} controller={makeController()} />)
    fireEvent.change(
      screen.getByPlaceholderText('export.panel.form.addressPlaceholder.ethereum'),
      { target: { value: EVM_ADDR } },
    )
    // 250 BTH > 100 BTH spendable.
    fireEvent.change(screen.getByPlaceholderText('0.00'), { target: { value: '250' } })
    expect(screen.getByText('export.panel.form.insufficient')).toBeTruthy()
  })

  it('opens an order, submits the BTH deposit via the wallet send path, tracks to completed, and hands off to Trade', async () => {
    vi.useFakeTimers()
    const controller = makeController()
    const onTradeNow = vi.fn()
    render(<ExportPanel t={t} controller={controller} onTradeNow={onTradeNow} />)

    fireEvent.change(
      screen.getByPlaceholderText('export.panel.form.addressPlaceholder.ethereum'),
      { target: { value: EVM_ADDR } },
    )
    fireEvent.change(screen.getByPlaceholderText('0.00'), { target: { value: '5' } })

    // Submit: create order + submit deposit (both async microtasks).
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /export\.panel\.form\.submit/ }))
      await vi.advanceTimersByTimeAsync(0)
    })

    // Order was opened with the typed destination + picocredit amount.
    expect(controller.client!.createMintOrder).toHaveBeenCalledWith({
      destChain: 'ethereum',
      destAddress: EVM_ADDR,
      amount: '5000000000000',
    })
    // The BTH deposit reuses the injected wallet send path with the order's
    // deposit address, u64 amount (as bigint), and the order memo.
    expect(controller.submitDeposit).toHaveBeenCalledWith({
      depositAddress: AWAITING.depositAddress,
      amount: 5_000_000_000_000n,
      memo: AWAITING.memo,
    })

    // Now tracking: the order id and the awaiting-deposit step are shown.
    expect(screen.getByText(AWAITING.id)).toBeTruthy()
    expect(screen.getByText('export.panel.order.depositSubmitted')).toBeTruthy()

    // Advance the poll; the next status is `completed`.
    ;(controller.client!.getOrderStatus as ReturnType<typeof vi.fn>).mockResolvedValue(COMPLETED)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(8_000)
    })

    const trade = screen.getByRole('button', { name: /export\.panel\.completed\.tradeNow/ })
    expect(trade).toBeTruthy()
    fireEvent.click(trade)
    expect(onTradeNow).toHaveBeenCalledWith('ethereum')
  })
})
