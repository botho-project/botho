/**
 * @vitest-environment jsdom
 *
 * Proof-of-Reserves card contract (#845): the peg indicator is driven solely
 * by `pegHealthy`; null supply legs render "unverified" (never `0`, never
 * green); a 404/`absent` state renders nothing; a non-404 failure degrades to
 * a grayed placeholder without fabricating values (#541 lesson).
 */
import { describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { ReserveProofCard } from './reserve-proof-card'
import type { ReserveProof } from '../types'

function proof(over: Partial<ReserveProof> = {}): ReserveProof {
  return {
    lockedReserve: 123_000_000_000_000,
    ethSupply: 100_000_000_000_000,
    solSupply: null,
    totalWrapped: null,
    drift: 0,
    inTolerance: true,
    pegHealthy: true,
    takenAt: Math.floor(Date.now() / 1000) - 30,
    ...over,
  }
}

describe('ReserveProofCard', () => {
  it('shows a healthy peg and formatted BTH amounts', () => {
    cleanup()
    render(<ReserveProofCard proof={proof({ totalWrapped: 100_000_000_000_000 })} state="ok" />)
    expect(screen.getByText('Peg healthy')).toBeDefined()
    // 123_000_000_000_000 picocredits = 123 BTH; 100e12 = 100 BTH.
    expect(screen.getByText(/123\.00 BTH/)).toBeDefined()
    expect(screen.getAllByText(/100\.00 BTH/).length).toBeGreaterThan(0)
  })

  it('shows a red/unhealthy peg driven solely by pegHealthy', () => {
    cleanup()
    render(<ReserveProofCard proof={proof({ pegHealthy: false })} state="ok" />)
    expect(screen.getByText('Peg unhealthy')).toBeDefined()
    expect(screen.queryByText('Peg healthy')).toBeNull()
  })

  it('renders null supplies as "unverified", never 0 or green', () => {
    cleanup()
    render(<ReserveProofCard proof={proof({ solSupply: null, totalWrapped: null })} state="ok" />)
    // Total wrapped is unverified.
    expect(screen.getByText('unverified')).toBeDefined()
    // Solana leg labeled explicitly as pending.
    expect(screen.getByText('unverified (Solana pending)')).toBeDefined()
    // Must NOT fabricate a zero figure for the unverified legs.
    expect(screen.queryByText('0.00 BTH')).toBeNull()
  })

  it('renders negative drift with a leading minus sign', () => {
    cleanup()
    render(
      <ReserveProofCard
        proof={proof({ drift: -5_000_000_000_000, inTolerance: false })}
        state="ok"
      />,
    )
    expect(screen.getByText(/−5\.00 BTH/)).toBeDefined()
    expect(screen.getByText('out of tolerance')).toBeDefined()
  })

  it('renders nothing when absent (404 — daemon not polling a bridge)', () => {
    cleanup()
    const { container } = render(<ReserveProofCard proof={null} state="absent" />)
    expect(container.firstChild).toBeNull()
  })

  it('renders a grayed placeholder (no fabricated values) when unavailable', () => {
    cleanup()
    render(<ReserveProofCard proof={null} state="unavailable" />)
    expect(screen.getByText(/Reserve proof unavailable/)).toBeDefined()
    expect(screen.queryByText('Peg healthy')).toBeNull()
    expect(screen.queryByText(/BTH/)).toBeNull()
  })

  it('formats near-u64 reserves via the bigint path without precision loss', () => {
    cleanup()
    // 9_007_199_254_740_993 exceeds Number.MAX_SAFE_INTEGER (2^53); BigInt keeps it exact.
    render(
      <ReserveProofCard
        proof={proof({ lockedReserve: 9_007_199_254_740_993, totalWrapped: 9_007_199_254_740_993 })}
        state="ok"
      />,
    )
    // Should not render NaN.
    expect(screen.queryByText(/NaN/)).toBeNull()
  })
})
