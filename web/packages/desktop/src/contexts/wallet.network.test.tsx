// @vitest-environment jsdom
import { act, cleanup, render, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { formatAddress } from '@botho/core'
import { WalletProvider, useWallet } from './wallet'
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), connection: {} as Record<string, unknown> }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('./connection', () => ({ useConnection: () => mocks.connection }))
// Format-only watch fixture; native tests use actually derived PQ keys.
const address = formatAddress(new Uint8Array(32), new Uint8Array(32), new Uint8Array(1184), new Uint8Array(1952), 'testnet')
const mainAddress = address.replace('tbotho://', 'botho://')
let wallet: ReturnType<typeof useWallet>
function Capture() { wallet = useWallet(); return <output>{wallet.address}</output> }
function tree() { return <WalletProvider><Capture /></WalletProvider> }
function select(network: string) {
  mocks.connection = { connectedNode: { id: `https://${network}.invalid/custom/rpc`, networkId: network },
    endpoint: `https://${network}.invalid/custom/rpc`, adapter: {
      getBalance: vi.fn().mockResolvedValue({ available: 0n, pending: 0n, total: 0n }),
      getTransactionHistory: vi.fn().mockResolvedValue([]), onTransaction: vi.fn(() => () => {}),
    } }
}
beforeEach(() => {
  const values = new Map<string, string>()
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key), clear: () => values.clear(),
  })
  mocks.invoke.mockReset(); select('botho-testnet')
  vi.stubGlobal('fetch', vi.fn(() => { throw new Error('No network permitted') }))
  mocks.invoke.mockImplementation(async (command: string) => {
    if (command === 'get_session_status') return { isUnlocked: false }
    if (command === 'wallet_file_exists') return { exists: false, path: '/fixture' }
    return true
  })
})
afterEach(() => { cleanup(); vi.unstubAllGlobals() })
it('invalidates only legacy public cache and refuses unknown-network unlock', async () => {
  localStorage.setItem('botho-wallet-address', 'cad:short:display')
  localStorage.setItem('unrelated-wallet-material', 'untouched')
  select('unrecognized'); render(tree())
  expect(wallet.address).toBeNull()
  expect(localStorage.getItem('botho-wallet-address')).toBeNull()
  expect(localStorage.getItem('unrelated-wallet-material')).toBe('untouched')
  await act(async () => { expect((await wallet.unlockWallet('fixture')).success).toBe(false) })
  expect(mocks.invoke.mock.calls.some(([command]) => command === 'unlock_wallet')).toBe(false)
})
it('drops an old-network unlock completion after selection changes', async () => {
  let finish!: (value: unknown) => void
  mocks.invoke.mockImplementation(async (command: string) => {
    if (command === 'unlock_wallet') return new Promise(resolve => { finish = resolve })
    if (command === 'get_session_status') return { isUnlocked: false }
    return { exists: false, path: '/fixture' }
  })
  const view = render(tree())
  let pending!: ReturnType<typeof wallet.unlockWallet>
  act(() => { pending = wallet.unlockWallet('fixture', '/fixture') })
  localStorage.setItem('botho-wallet-address:botho-mainnet', mainAddress)
  select('botho-mainnet'); view.rerender(tree())
  await act(async () => { finish({ success: true, address }); await pending })
  expect(wallet.address).toBe(mainAddress)
  expect(wallet.isUnlocked).toBe(false)
  expect(mocks.invoke).toHaveBeenCalledWith('unlock_wallet', { params: { network: 'botho-testnet', password: 'fixture', path: '/fixture' } })
})
it('passes exact endpoint to native balance and drops its stale completion', async () => {
  let finish!: (value: unknown) => void
  mocks.invoke.mockImplementation(async (command: string) => {
    if (command === 'get_session_status') return { isUnlocked: true, address }
    if (command === 'get_balance') return new Promise(resolve => { finish = resolve })
    return { exists: false, path: '/fixture' }
  })
  const view = render(tree())
  await waitFor(() => expect(finish).toBeDefined())
  expect(mocks.invoke).toHaveBeenCalledWith('get_balance', { params: { network: 'botho-testnet', endpoint: 'https://botho-testnet.invalid/custom/rpc' } })
  select('botho-mainnet'); view.rerender(tree())
  await act(async () => { finish({ success: true, balance: '999' }) })
  expect(wallet.balance).toBeNull()
  expect(wallet.address).not.toBe(address)
})
