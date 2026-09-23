// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { FaucetButton } from './faucet-button'
const invoke = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke }))
afterEach(() => { cleanup(); invoke.mockReset() })
it('passes explicit testnet and exact selected endpoint without making a request itself', async () => {
  invoke.mockResolvedValue({ success: false, error: 'fixture endpoint has no faucet' })
  render(<FaucetButton network="botho-testnet" endpoint="https://fixture.invalid/custom/rpc" isUnlocked />)
  fireEvent.click(screen.getByRole('button'))
  await waitFor(() => expect(invoke).toHaveBeenCalledWith('request_faucet', { params: {
    network: 'botho-testnet', endpoint: 'https://fixture.invalid/custom/rpc',
  } }))
})
it('never invokes faucet for mainnet', () => {
  render(<FaucetButton network="botho-mainnet" endpoint="https://fixture.invalid/custom/rpc" isUnlocked />)
  fireEvent.click(screen.getByRole('button'))
  expect(invoke).not.toHaveBeenCalled()
})
