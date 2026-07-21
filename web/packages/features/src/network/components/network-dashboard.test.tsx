/**
 * @vitest-environment jsdom
 *
 * The dashboard's contract (#698): unreachable nodes render an EXPLICIT error
 * card (never stale or fabricated values — the #541 lesson), lagging nodes
 * are highlighted against the fleet consensus height, and a missing history
 * backend degrades to an informational state without breaking the live grid.
 */
import { describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { NetworkDashboard } from './network-dashboard'
import type { FleetNode, FleetNodeStatus } from '../types'

const NODES: FleetNode[] = [
  { id: 'seed', name: 'Seed (validator)', rpcEndpoint: 'https://seed.test/rpc' },
  { id: 'eu', name: 'EU seed (Frankfurt)', rpcEndpoint: 'https://eu.test/rpc' },
  { id: 'ap', name: 'AP seed (Singapore)', rpcEndpoint: 'https://ap.test/rpc' },
]

function live(id: string, over: Partial<FleetNodeStatus> = {}): FleetNodeStatus {
  return {
    nodeId: id,
    reachable: true,
    polledAt: Date.now(),
    chainHeight: 221,
    peerCount: 4,
    scpPeerCount: 4,
    mempoolSize: 0,
    mintingActive: false,
    nodeVersion: '0.3.1',
    slotStalled: false,
    ...over,
  }
}

function renderDash(statuses: Record<string, FleetNodeStatus>) {
  cleanup()
  return render(
    <NetworkDashboard
      nodes={NODES}
      statuses={statuses}
      avgBlockSeconds={20}
      history={{}}
      historyState="unavailable"
    />,
  )
}

describe('NetworkDashboard', () => {
  it('renders live stats per node and the fleet summary', () => {
    renderDash({ seed: live('seed'), eu: live('eu'), ap: live('ap') })
    expect(screen.getByText('Consensus height')).toBeDefined()
    expect(screen.getAllByText('221')).toHaveLength(4) // summary + 3 node cards
    expect(screen.getByText('3/3')).toBeDefined() // nodes in sync
  })

  it('renders an explicit error card for an unreachable node — no stale values', () => {
    renderDash({
      seed: live('seed'),
      eu: { nodeId: 'eu', reachable: false, polledAt: Date.now() },
      ap: live('ap'),
    })
    expect(screen.getByText('Unreachable')).toBeDefined()
    expect(screen.getByText(/1 unreachable/)).toBeDefined()
    // The unreachable card must not show any height number.
    expect(screen.getAllByText('221')).toHaveLength(3) // summary + 2 live cards only
  })

  it('highlights a lagging node against the consensus height', () => {
    renderDash({
      seed: live('seed', { chainHeight: 221 }),
      eu: live('eu', { chainHeight: 210 }),
      ap: live('ap', { chainHeight: 221 }),
    })
    expect(screen.getByText(/11 blocks behind/)).toBeDefined()
    expect(screen.getByText('2/3')).toBeDefined()
  })

  it('flags peer-isolated relays and does not let their stale height poison consensus', () => {
    // The live eu/ap drift: one connected validator at 202, two isolated
    // relays stuck on the old pre-reset chain at 3233 with zero peers.
    renderDash({
      seed: live('seed', { chainHeight: 202, peerCount: 2 }),
      eu: live('eu', { chainHeight: 3233, peerCount: 0 }),
      ap: live('ap', { chainHeight: 3233, peerCount: 0 }),
    })
    // Consensus is the connected validator's height, not the isolated relays'.
    expect(screen.getAllByText('202')).toHaveLength(2) // summary + seed card
    // Both isolated relays are called out; the validator is NOT "behind".
    expect(screen.getAllByText(/isolated — 0 peers/)).toHaveLength(2)
    expect(screen.queryByText(/blocks behind/)).toBeNull()
    expect(screen.getByText(/2 isolated/)).toBeDefined()
    expect(screen.getByText('1/3')).toBeDefined() // only the validator in sync
  })

  it('surfaces a stalled SCP slot as a warning badge', () => {
    renderDash({
      seed: live('seed', { slotStalled: true }),
      eu: live('eu'),
      ap: live('ap'),
    })
    expect(screen.getByText('SCP slot stalled')).toBeDefined()
  })

  it('shows checking state for nodes whose first poll is in flight', () => {
    renderDash({ seed: live('seed') })
    expect(screen.getAllByText('Checking…')).toHaveLength(2)
  })

  it('degrades history to an informational state when the metrics API is unreachable', () => {
    renderDash({ seed: live('seed'), eu: live('eu'), ap: live('ap') })
    expect(screen.getAllByText(/History unavailable/)).toHaveLength(2)
    // The live grid is unaffected.
    expect(screen.getByText('3/3')).toBeDefined()
  })

  it('omits the reserve card entirely when reserveState is undefined', () => {
    renderDash({ seed: live('seed'), eu: live('eu'), ap: live('ap') })
    expect(screen.queryByText('Proof of Reserves')).toBeNull()
  })

  it('renders the reserve card with an ok proof', () => {
    cleanup()
    render(
      <NetworkDashboard
        nodes={NODES}
        statuses={{ seed: live('seed'), eu: live('eu'), ap: live('ap') }}
        avgBlockSeconds={20}
        history={{}}
        historyState="unavailable"
        reserve={{
          lockedReserve: 123_000_000_000_000,
          ethSupply: 100_000_000_000_000,
          solSupply: null,
          totalWrapped: null,
          drift: 0,
          inTolerance: true,
          pegHealthy: true,
          takenAt: Math.floor(Date.now() / 1000) - 30,
        }}
        reserveState="ok"
      />,
    )
    expect(screen.getByText('Proof of Reserves')).toBeDefined()
    expect(screen.getByText('Peg healthy')).toBeDefined()
    // The live grid is unaffected.
    expect(screen.getByText('3/3')).toBeDefined()
  })

  it('hides the reserve card on the absent (404) state without breaking the grid', () => {
    cleanup()
    render(
      <NetworkDashboard
        nodes={NODES}
        statuses={{ seed: live('seed'), eu: live('eu'), ap: live('ap') }}
        avgBlockSeconds={20}
        history={{}}
        historyState="unavailable"
        reserve={null}
        reserveState="absent"
      />,
    )
    expect(screen.queryByText('Proof of Reserves')).toBeNull()
    expect(screen.getByText('3/3')).toBeDefined()
  })
})
