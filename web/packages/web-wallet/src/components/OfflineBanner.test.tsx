/**
 * @vitest-environment jsdom
 *
 * Tests for the active-node-offline banner logic (#492):
 *   - the pure raw-offline derivation + debounce accumulator
 *   - the OfflineBanner component: shows only AFTER the debounce threshold,
 *     hides when the node is online, and is dismissible.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, cleanup, fireEvent } from '@testing-library/react'
import type { NodeHealth } from '../config/networks'
import {
  advanceDebounce,
  initialDebounceState,
  isActiveNodeOfflineRaw,
  OFFLINE_DEBOUNCE_TICKS,
} from '../lib/active-node-offline'
import { OfflineBanner } from './OfflineBanner'

// --- Mock the two contexts the banner reads --------------------------------
const useNetworkMock = vi.fn()
const useWalletMock = vi.fn()
vi.mock('../contexts/network', () => ({ useNetwork: () => useNetworkMock() }))
vi.mock('../contexts/wallet', () => ({ useWallet: () => useWalletMock() }))

const ONLINE: NodeHealth = { status: 'online', chainHeight: 100, synced: true }
const OFFLINE: NodeHealth = { status: 'offline' }
const CHECKING: NodeHealth = { status: 'checking' }

describe('active-node-offline pure logic', () => {
  it('reports offline for a selected node whose health is offline', () => {
    expect(
      isActiveNodeOfflineRaw({
        ingressId: 'seed',
        nodeHealth: { seed: OFFLINE },
        isConnected: true,
      }),
    ).toBe(true)
  })

  it('reports online for a selected node whose health is online', () => {
    expect(
      isActiveNodeOfflineRaw({
        ingressId: 'seed',
        nodeHealth: { seed: ONLINE },
        isConnected: false,
      }),
    ).toBe(false)
  })

  it('only looks at the SELECTED node, ignoring others', () => {
    // seed2 is offline but seed (selected) is online -> not offline.
    expect(
      isActiveNodeOfflineRaw({
        ingressId: 'seed',
        nodeHealth: { seed: ONLINE, seed2: OFFLINE },
        isConnected: true,
      }),
    ).toBe(false)
  })

  it('defers a not-yet-probed (checking) node to the connection state', () => {
    expect(
      isActiveNodeOfflineRaw({ ingressId: 'seed', nodeHealth: { seed: CHECKING }, isConnected: true }),
    ).toBe(false)
    expect(
      isActiveNodeOfflineRaw({ ingressId: 'seed', nodeHealth: { seed: CHECKING }, isConnected: false }),
    ).toBe(true)
  })

  it('falls back to the connection state for a custom endpoint (no health entry)', () => {
    expect(
      isActiveNodeOfflineRaw({ ingressId: 'custom', nodeHealth: {}, isConnected: false }),
    ).toBe(true)
    expect(
      isActiveNodeOfflineRaw({ ingressId: 'custom', nodeHealth: {}, isConnected: true }),
    ).toBe(false)
  })

  it('debounces transient offline blips: a single tick does not show the banner', () => {
    let s = initialDebounceState()
    expect(s.shown).toBe(false)
    s = advanceDebounce(s, true) // 1 of OFFLINE_DEBOUNCE_TICKS
    expect(OFFLINE_DEBOUNCE_TICKS).toBeGreaterThan(1)
    expect(s.shown).toBe(false)
  })

  it('shows the banner once offline persists for the debounce threshold', () => {
    let s = initialDebounceState()
    for (let i = 0; i < OFFLINE_DEBOUNCE_TICKS; i++) s = advanceDebounce(s, true)
    expect(s.shown).toBe(true)
  })

  it('a single online observation immediately resets + hides (recovery not debounced)', () => {
    let s = initialDebounceState()
    for (let i = 0; i < OFFLINE_DEBOUNCE_TICKS; i++) s = advanceDebounce(s, true)
    expect(s.shown).toBe(true)
    s = advanceDebounce(s, false)
    expect(s.shown).toBe(false)
    expect(s.consecutiveOffline).toBe(0)
  })
})

describe('OfflineBanner component', () => {
  beforeEach(() => {
    cleanup()
    useNetworkMock.mockReset()
    useWalletMock.mockReset()
  })

  function setup(health: Record<string, NodeHealth>, isConnected: boolean, ingressId = 'seed') {
    useNetworkMock.mockReturnValue({ ingressId, nodeHealth: health })
    useWalletMock.mockReturnValue({ isConnected })
  }

  it('is hidden while the active node is online', () => {
    setup({ seed: ONLINE }, true)
    render(<OfflineBanner />)
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('does NOT show on the first offline observation (debounced)', () => {
    setup({ seed: OFFLINE }, true)
    render(<OfflineBanner />)
    // First mount = first observation; below the threshold -> still hidden.
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('shows after the active node stays offline across the debounce threshold', () => {
    setup({ seed: OFFLINE }, true)
    const { rerender } = render(<OfflineBanner />)
    // Re-render once per additional poll tick (nodeHealth identity changes each
    // poll); after OFFLINE_DEBOUNCE_TICKS observations the banner appears.
    for (let i = 1; i < OFFLINE_DEBOUNCE_TICKS; i++) {
      useNetworkMock.mockReturnValue({ ingressId: 'seed', nodeHealth: { seed: { ...OFFLINE } } })
      rerender(<OfflineBanner />)
    }
    expect(screen.getByRole('alert')).toBeTruthy()
    expect(screen.getByText(/unreachable/i)).toBeTruthy()
    expect(screen.getByRole('button', { name: /switch node/i })).toBeTruthy()
  })

  it('can be dismissed', () => {
    setup({ seed: OFFLINE }, true)
    const { rerender } = render(<OfflineBanner />)
    for (let i = 1; i < OFFLINE_DEBOUNCE_TICKS; i++) {
      useNetworkMock.mockReturnValue({ ingressId: 'seed', nodeHealth: { seed: { ...OFFLINE } } })
      rerender(<OfflineBanner />)
    }
    expect(screen.getByRole('alert')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: /dismiss/i }))
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('"Switch node" dispatches the open-network-selector event', () => {
    setup({ seed: OFFLINE }, true)
    const onOpen = vi.fn()
    window.addEventListener('open-network-selector', onOpen)
    const { rerender } = render(<OfflineBanner />)
    for (let i = 1; i < OFFLINE_DEBOUNCE_TICKS; i++) {
      useNetworkMock.mockReturnValue({ ingressId: 'seed', nodeHealth: { seed: { ...OFFLINE } } })
      rerender(<OfflineBanner />)
    }
    fireEvent.click(screen.getByRole('button', { name: /switch node/i }))
    expect(onOpen).toHaveBeenCalled()
    window.removeEventListener('open-network-selector', onOpen)
  })
})
