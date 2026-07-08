/**
 * @vitest-environment jsdom
 *
 * The trust view's contract (#706): unreachable nodes render an EXPLICIT
 * error card (never stale or fabricated values — the #541 lesson), gate
 * fields the node omits render as absent ("—") rather than zero, and
 * `quorumGateIntersectionRefused` / `quorumDegenerate` surface as prominent
 * warning banners (#509 warn-don't-refuse).
 */
import { describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { TrustDashboard } from './trust-dashboard'
import type { FleetNode } from '../../network/types'
import type {
  NodeTrustStatus,
  OperatorFetchResult,
  OperatorQuorumInfo,
  TrustPeer,
} from '../types'

const NODES: FleetNode[] = [
  { id: 'seed', name: 'Seed (validator)', rpcEndpoint: 'https://seed.test/rpc' },
  { id: 'eu', name: 'EU seed (Frankfurt)', rpcEndpoint: 'https://eu.test/rpc' },
  { id: 'ap', name: 'AP seed (Singapore)', rpcEndpoint: 'https://ap.test/rpc' },
]

const PEERS: TrustPeer[] = [
  {
    peerId: '12D3KooWJ5U2gk6Pe9ehZb6aHng2zu7RnUwAKzEYxHbaM6VRo592',
    address: null,
    protocolVersion: '4.0.0 (block v5)',
    versionWarning: false,
    lastSeenSecs: 44,
  },
  {
    peerId: '12D3KooWRubuvzRNxbxHH5BdzgxQNqMoWyQtdxKXUdNWJt5huTpk',
    address: null,
    protocolVersion: null,
    versionWarning: true,
    lastSeenSecs: 0,
  },
]

function live(id: string, over: Partial<NodeTrustStatus> = {}): NodeTrustStatus {
  return {
    nodeId: id,
    reachable: true,
    polledAt: Date.now(),
    quorumFaultTolerant: true,
    quorumDegenerate: false,
    quorumCuratedMembers: 0,
    quorumAutoMembers: 3,
    quorumGateSuppressedPeers: 0,
    quorumGateMaxAutoMembers: 8,
    quorumGateIntersectionRefused: false,
    scpPeerCount: 3,
    peers: PEERS,
    ...over,
  }
}

function renderDash(statuses: Record<string, NodeTrustStatus>) {
  cleanup()
  return render(<TrustDashboard nodes={NODES} statuses={statuses} />)
}

describe('TrustDashboard', () => {
  it('renders per-node gate counts, posture badges, and the peer table', () => {
    renderDash({ seed: live('seed'), eu: live('eu'), ap: live('ap') })
    expect(screen.getAllByText('Curated')).toHaveLength(3)
    expect(screen.getAllByText('Auto-promoted')).toHaveLength(3)
    expect(screen.getAllByText('Suppressed')).toHaveLength(3)
    expect(screen.getAllByText('Auto cap')).toHaveLength(3)
    expect(screen.getAllByText('BFT fault tolerant')).toHaveLength(3)
    expect(screen.getAllByText('Connected peers (2)')).toHaveLength(3)
    expect(
      screen.getAllByText('12D3KooWJ5U2gk6Pe9ehZb6aHng2zu7RnUwAKzEYxHbaM6VRo592'),
    ).toHaveLength(3)
    // A peer with an outdated protocol version is flagged.
    expect(screen.getAllByText('outdated')).toHaveLength(3)
    // No warning banners in the healthy state.
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('renders an explicit error card for an unreachable node — no stale values', () => {
    renderDash({
      seed: live('seed'),
      eu: { nodeId: 'eu', reachable: false, polledAt: Date.now() },
      ap: live('ap'),
    })
    expect(screen.getByText('Unreachable')).toBeDefined()
    expect(screen.getByText(/1 of 3 nodes unreachable/)).toBeDefined()
    // Only the two live cards show gate counts.
    expect(screen.getAllByText('Curated')).toHaveLength(2)
  })

  it('shows checking state for nodes whose first poll is in flight', () => {
    renderDash({ seed: live('seed') })
    expect(screen.getAllByText('Checking…')).toHaveLength(2)
    // Nodes never polled don't count as unreachable.
    expect(screen.queryByText(/unreachable/)).toBeNull()
  })

  it('surfaces quorumGateIntersectionRefused as a prominent warning', () => {
    renderDash({
      seed: live('seed', { quorumGateIntersectionRefused: true }),
      eu: live('eu'),
      ap: live('ap'),
    })
    const banner = screen.getByRole('alert')
    expect(banner.textContent).toContain(
      'Quorum intersection check refused the latest candidate',
    )
    expect(banner.textContent).toContain('Seed (validator)')
    expect(screen.getByText('intersection check refused last candidate')).toBeDefined()
  })

  it('surfaces quorumDegenerate as a prominent warning', () => {
    renderDash({
      seed: live('seed'),
      eu: live('eu', { quorumDegenerate: true, quorumFaultTolerant: false }),
      ap: live('ap'),
    })
    const banner = screen.getByRole('alert')
    expect(banner.textContent).toContain('Degenerate quorum — zero fault tolerance')
    expect(banner.textContent).toContain('EU seed (Frankfurt)')
    expect(screen.getByText(/degenerate quorum — zero fault/)).toBeDefined()
  })

  it('renders absent gate fields as "—", never zero (anti-#541)', () => {
    renderDash({
      seed: live('seed', {
        quorumCuratedMembers: undefined,
        quorumAutoMembers: undefined,
        quorumGateSuppressedPeers: undefined,
        quorumGateMaxAutoMembers: undefined,
        quorumGateIntersectionRefused: undefined,
        scpPeerCount: undefined,
      }),
    })
    const card = screen.getByTestId('trust-card-seed')
    // All five stats render the absent marker; no fabricated zeros.
    expect(card.textContent?.match(/—/g)?.length).toBeGreaterThanOrEqual(5)
  })

  it('renders an explicit unavailable state when the peer list call failed', () => {
    renderDash({ seed: live('seed', { peers: undefined }) })
    expect(screen.getByText(/Peer list unavailable/)).toBeDefined()
  })

  it('distinguishes a genuinely empty peer list from an unavailable one', () => {
    renderDash({ seed: live('seed', { peers: [] }) })
    expect(screen.getByText('No connected peers')).toBeDefined()
    expect(screen.queryByText(/Peer list unavailable/)).toBeNull()
  })

  it('renders a null peer protocol version as "—"', () => {
    renderDash({ seed: live('seed') })
    const card = screen.getByTestId('trust-card-seed')
    expect(card.textContent).toContain('—')
  })
})

describe('TrustDashboard operator view (#707)', () => {
  const okInfo = (over: Partial<OperatorQuorumInfo> = {}): OperatorFetchResult<OperatorQuorumInfo> => ({
    status: 'ok',
    data: {
      mode: 'recommended',
      faultModel: 'crash',
      threshold: 2,
      members: ['12D3KooWCuratedMemberAAAA'],
      minPeers: 1,
      maxAutoMembers: 8,
      perPeer: {
        curated: ['12D3KooWCuratedMemberAAAA'],
        auto: ['12D3KooWAutoPeerBBBB'],
        suppressed: ['12D3KooWSuppressedCCCC'],
      },
      ...over,
    },
  })

  it('shows NO operator panels or banner in the public view (no token)', () => {
    cleanup()
    render(<TrustDashboard nodes={NODES} statuses={{ seed: live('seed') }} operatorMode="disabled" />)
    expect(screen.queryByText('Operator detail')).toBeNull()
    expect(screen.queryByText(/Operator view/)).toBeNull()
  })

  it('renders the configured-members panel and per-peer badges with a valid token', () => {
    cleanup()
    render(
      <TrustDashboard
        nodes={NODES}
        statuses={{ seed: live('seed') }}
        operatorInfo={{ seed: okInfo() }}
        operatorMode="active"
      />,
    )
    expect(screen.getByText('Operator detail')).toBeDefined()
    // Active-session banner.
    expect(screen.getByText(/Operator view/)).toBeDefined()
    // Configured members panel.
    expect(screen.getByText('Configured members (1)')).toBeDefined()
    expect(screen.getByText('12D3KooWCuratedMemberAAAA')).toBeDefined()
    // Per-peer classification badges (curated / auto / suppressed).
    expect(screen.getByText('curated')).toBeDefined()
    expect(screen.getByText('auto')).toBeDefined()
    expect(screen.getByText('suppressed')).toBeDefined()
  })

  it('renders "no gate evaluation yet" for perPeer:absent (anti-#541)', () => {
    cleanup()
    render(
      <TrustDashboard
        nodes={NODES}
        statuses={{ seed: live('seed') }}
        operatorInfo={{ seed: okInfo({ perPeer: undefined }) }}
        operatorMode="active"
      />,
    )
    expect(screen.getByText(/no gate evaluation yet/)).toBeDefined()
    // No fabricated classification badges.
    expect(screen.queryByText('curated')).toBeNull()
  })

  it('degrades to the public view with an expired-link banner on unauthorized', () => {
    cleanup()
    render(
      <TrustDashboard
        nodes={NODES}
        statuses={{ seed: live('seed') }}
        operatorInfo={{ seed: { status: 'unauthorized' } }}
        operatorMode="unauthorized"
      />,
    )
    // Both the fleet banner and the per-node panel flag the expired link.
    expect(screen.getAllByText(/Operator link expired or invalid/).length).toBeGreaterThan(0)
    expect(screen.getByText(/showing the public read-only view/)).toBeDefined()
    // No operator detail data is shown.
    expect(screen.queryByText('Configured members (1)')).toBeNull()
  })

  it('explains a fleet with no operator surface (not-enabled)', () => {
    cleanup()
    render(
      <TrustDashboard
        nodes={NODES}
        statuses={{ seed: live('seed') }}
        operatorInfo={{ seed: { status: 'not-enabled' } }}
        operatorMode="not-enabled"
      />,
    )
    expect(screen.getByText(/Operator reads are not enabled/)).toBeDefined()
  })
})
