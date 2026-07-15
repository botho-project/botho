/**
 * @vitest-environment jsdom
 *
 * Tests for SafeQR's capacity-aware fit guard (#965, #979).
 *
 * The core invariant (#979): the fit-check cap MUST correspond to the EC level
 * the symbol is actually rendered at. The component defaults to level 'M'
 * (version-40 byte capacity 2331), so a payload in the 2332..2953 band — which
 * the old level-L cap (2953) wrongly accepted — must fall back to the copy
 * panel instead of being handed to `qrcode.react` (where it throws at level M).
 */
import { describe, it, expect, afterEach } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'
import {
  SafeQR,
  singleQrByteCap,
  QR_V40_BYTE_CAP,
  SINGLE_QR_BYTE_CAP,
} from './SafeQR'

afterEach(() => cleanup())

/** The rendered QR lives inside an <svg>; the fallback is a role="note" panel. */
function hasQr(container: HTMLElement): boolean {
  return container.querySelector('svg') !== null
}
function hasCopyPanel(): boolean {
  return screen.queryByRole('note') !== null
}

describe('singleQrByteCap — cap matches the rendered EC level', () => {
  it('returns the version-40 byte capacity for each EC level', () => {
    expect(singleQrByteCap('L')).toBe(2953)
    expect(singleQrByteCap('M')).toBe(2331)
    expect(singleQrByteCap('Q')).toBe(1663)
    expect(singleQrByteCap('H')).toBe(1273)
  })

  it('is the single source of truth for the level table', () => {
    for (const level of ['L', 'M', 'Q', 'H'] as const) {
      expect(singleQrByteCap(level)).toBe(QR_V40_BYTE_CAP[level])
    }
  })

  it('keeps the deprecated flat constant pinned to the level-L ceiling only', () => {
    // Back-compat: SINGLE_QR_BYTE_CAP is the L ceiling and must NOT be used as
    // the default-level (M) cap — that mismatch is the #979 drift.
    expect(SINGLE_QR_BYTE_CAP).toBe(QR_V40_BYTE_CAP.L)
    expect(SINGLE_QR_BYTE_CAP).not.toBe(QR_V40_BYTE_CAP.M)
  })
})

describe('SafeQR fit guard — default level M', () => {
  const capM = singleQrByteCap('M') // 2331

  it('renders a QR for a payload exactly at the level-M cap', () => {
    const { container } = render(<SafeQR value={'a'.repeat(capM)} />)
    expect(hasQr(container)).toBe(true)
    expect(hasCopyPanel()).toBe(false)
  })

  it('falls back to the copy panel one byte over the level-M cap', () => {
    // This byte lands in the old 2332..2953 danger band: the level-L cap (2953)
    // would have (wrongly) rendered a QR that throws at level M.
    render(<SafeQR value={'a'.repeat(capM + 1)} />)
    expect(hasCopyPanel()).toBe(true)
  })

  it('rejects a payload sitting in the L-vs-M drift band (e.g. 2600 bytes)', () => {
    render(<SafeQR value={'a'.repeat(2600)} />)
    expect(hasCopyPanel()).toBe(true)
  })
})

describe('SafeQR fit guard — explicit level L widens the cap consistently', () => {
  it('renders a QR at 2600 bytes when level="L" (cap 2953)', () => {
    const { container } = render(<SafeQR value={'a'.repeat(2600)} level="L" />)
    expect(hasQr(container)).toBe(true)
    expect(hasCopyPanel()).toBe(false)
  })

  it('still falls back one byte over the level-L cap', () => {
    render(<SafeQR value={'a'.repeat(singleQrByteCap('L') + 1)} level="L" />)
    expect(hasCopyPanel()).toBe(true)
  })
})

describe('SafeQR edge cases', () => {
  it('shows the copy panel for an empty payload', () => {
    render(<SafeQR value="" />)
    expect(hasCopyPanel()).toBe(true)
  })

  it('renders a QR for a short address-like payload', () => {
    const { container } = render(<SafeQR value="botho://1/abcDEF123" />)
    expect(hasQr(container)).toBe(true)
    expect(hasCopyPanel()).toBe(false)
  })
})
