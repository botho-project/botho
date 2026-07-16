import { describe, expect, it } from 'vitest'
import {
  RELEASE_PROGRESSION,
  isTerminalReleaseStatus,
  releaseProgressionIndex,
  sourceTxUrl,
} from './release-status'

describe('release-status', () => {
  it('orders the happy-path progression to match the Rust burn-side state machine', () => {
    expect([...RELEASE_PROGRESSION]).toEqual([
      'burn_detected',
      'burn_confirmed',
      'release_pending',
      'released',
    ])
  })

  it('treats released/expired/failed as terminal, others as non-terminal', () => {
    expect(isTerminalReleaseStatus('released')).toBe(true)
    expect(isTerminalReleaseStatus('expired')).toBe(true)
    expect(isTerminalReleaseStatus('failed')).toBe(true)
    expect(isTerminalReleaseStatus('burn_detected')).toBe(false)
    expect(isTerminalReleaseStatus('release_pending')).toBe(false)
  })

  it('indexes progression states and returns -1 for off-path terminals', () => {
    expect(releaseProgressionIndex('burn_detected')).toBe(0)
    expect(releaseProgressionIndex('released')).toBe(3)
    expect(releaseProgressionIndex('expired')).toBe(-1)
    expect(releaseProgressionIndex('failed')).toBe(-1)
  })

  it('builds testnet burn-tx explorer URLs (and null for unknown chains)', () => {
    expect(sourceTxUrl('ethereum', '0xabc')).toBe('https://sepolia.etherscan.io/tx/0xabc')
    expect(sourceTxUrl('solana', 'sig123')).toBe(
      'https://explorer.solana.com/tx/sig123?cluster=devnet',
    )
    // @ts-expect-error — exhaustiveness guard: unknown chains yield no link.
    expect(sourceTxUrl('dogecoin', 'x')).toBeNull()
  })
})
