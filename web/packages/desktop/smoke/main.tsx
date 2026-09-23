import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { invoke } from '@tauri-apps/api/core'
import { SendModal } from '@botho/features/wallet'
import { parseBTH } from '@botho/core'
import { NetworkGraph, type NetworkPeer } from '../src/components/network/network-graph'
import '@botho/ui/styles/theme.css'

const expected = 9007199254740993n
const fee = 1000000000n
const balance = { available: expected * 2n, pending: 0n, total: expected * 2n }
const peers: NetworkPeer[] = [
  { id: 'fixture-a', nodeId: 'fixture-a', isValidator: false, isSelf: true, status: 'online', connectedTo: ['fixture-b'] },
  { id: 'fixture-b', nodeId: 'fixture-b', isValidator: true, isSelf: false, status: 'online', connectedTo: ['fixture-a'] },
]
const errors: string[] = []
window.addEventListener('error', event => errors.push(event.message))
window.addEventListener('unhandledrejection', event => errors.push(String(event.reason)))
// Read-only observations for the native WebDriver; no IPC replacements.
Object.defineProperty(window, '__smokeErrors', { get: () => [...errors] })

function Smoke() {
  const [fixture, setFixture] = useState<{ path: string; recipient: string } | null>(null)
  const [status, setStatus] = useState('ready')
  const [selected, setSelected] = useState('none')
  const [open, setOpen] = useState(false)
  const [result, setResult] = useState('none')
  const [feeAmount, setFeeAmount] = useState('none')
  async function unlock() {
    try {
      const info = await invoke<{ path: string; recipient: string }>('fixture_info')
      const before = await invoke<{ isUnlocked: boolean }>('get_session_status', { network: 'botho-testnet' })
      if (before.isUnlocked) throw new Error('fixture initially unlocked')
      const unlocked = await invoke<{ success: boolean }>('fixture_unlock', { path: info.path })
      const after = await invoke<{ isUnlocked: boolean }>('get_session_status', { network: 'botho-testnet' })
      if (!unlocked.success || !after.isUnlocked) throw new Error('native unlock/session failed')
      setFixture(info)
      setStatus('unlocked')
    } catch (error) { errors.push(String(error)); setStatus('failed') }
  }
  async function lock() {
    if (!await invoke<boolean>('lock_wallet')) throw new Error('native lock failed')
    const after = await invoke<{ isUnlocked: boolean }>('get_session_status', { network: 'botho-testnet' })
    if (after.isUnlocked) throw new Error('native session remained unlocked')
    setStatus('locked')
  }
  return <main style={{ padding: 20 }}>
    <h1 id="theme-probe" className="font-display text-lg font-bold text-[--color-light]">Native fixture smoke</h1>
    <output id="bigint">{parseBTH('9007.199254740993').toString()}</output>
    <output id="remaining">{(balance.available - expected - fee).toString()}</output>
    <button id="unlock" onClick={unlock}>Unlock public fixture</button>
    <button id="lock" onClick={() => void lock().catch(error => errors.push(String(error)))}>Lock fixture</button>
    <button id="prepare" disabled={!fixture} onClick={() => setOpen(true)}>Prepare only</button>
    <output id="status">{status}</output><output id="selected">{selected}</output>
    <output id="recipient">{fixture?.recipient ?? ''}</output>
    <output id="preflight">{result}</output><output id="fee-amount">{feeAmount}</output>
    <div style={{ height: 420 }}><NetworkGraph peers={peers} onPeerSelect={peer => setSelected(peer?.id ?? 'none')} /></div>
    <SendModal isOpen={open} onClose={() => setOpen(false)} balance={balance}
      estimateFee={async amount => { setFeeAmount(amount.toString()); return { fee, clusterFactorDisplay: '1.00x' } }}
      onSend={async data => {
        const amount = await invoke<string>('fixture_preflight', { recipient: data.recipient, amount: data.amount.toString() })
        if (data.amount !== expected || amount !== expected.toString()) throw new Error('amount lost precision')
        setResult(amount)
        setOpen(false)
        // No transaction hash and no broadcast; this callback ends at native pure preflight.
        return { success: true }
      }} />
  </main>
}
createRoot(document.getElementById('root')!).render(<Smoke />)
