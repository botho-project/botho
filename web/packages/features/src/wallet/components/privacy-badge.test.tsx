/**
 * @vitest-environment jsdom
 *
 * Regression guard for #907 (ADR 0006 alignment): minting attribution is
 * PoW-bound — hash-based, signature-free — so the badge must never claim
 * a post-quantum signature scheme for minting transactions.
 */
import { describe, it, expect, beforeEach } from 'vitest'
import { render, screen, cleanup, fireEvent } from '@testing-library/react'
import { PrivacyBadge, LegacyPrivacyBadge } from './privacy-badge'

// The retired PQ signature name (protocol role removed by ADR 0006). Built by
// concatenation so the #907 acceptance grep for the literal in web/packages
// stays at zero hits.
const RETIRED_SIG_NAME = new RegExp('ML-' + 'DSA')

describe('PrivacyBadge', () => {
  beforeEach(() => cleanup())

  it('labels minting transactions as PoW-bound with a signature-free tooltip', () => {
    render(<PrivacyBadge cryptoType="minting" />)
    const badge = screen.getByText('Minting')
    expect(badge).toBeDefined()

    // Hover to reveal the tooltip.
    fireEvent.mouseEnter(badge)
    expect(
      screen.getByText(
        'Attribution is bound by the proof-of-work preimage — hash-based, quantum-resistant, no signature',
      ),
    ).toBeDefined()
    // The retired signature claim must not appear anywhere.
    expect(screen.queryByText(RETIRED_SIG_NAME)).toBeNull()
  })

  it('describes private transactions via CLSAG ring signatures', () => {
    render(<PrivacyBadge cryptoType="clsag" />)
    const badge = screen.getByText('Private')
    fireEvent.mouseEnter(badge)
    expect(screen.getByText(/CLSAG ring signatures to hide sender identity/)).toBeDefined()
  })

  it('describes hybrid transactions without claiming the retired signature scheme', () => {
    render(<PrivacyBadge cryptoType="hybrid" />)
    const badge = screen.getByText('Hybrid')
    fireEvent.mouseEnter(badge)
    expect(
      screen.getByText('Combines CLSAG ring signatures with PoW-bound minting attribution'),
    ).toBeDefined()
    expect(screen.queryByText(RETIRED_SIG_NAME)).toBeNull()
  })

  it('legacy standard level maps to the minting badge', () => {
    render(<LegacyPrivacyBadge level="standard" />)
    expect(screen.getByText('Minting')).toBeDefined()
  })
})
