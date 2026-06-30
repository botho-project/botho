/**
 * @vitest-environment jsdom
 *
 * Security regression for custom-RPC deep links, against the REAL NetworkProvider (#587).
 *
 * The guarantee under test: opening the wallet with a `?rpc=<https>` deep link
 * must NEVER silently switch the active node. The link is surfaced as a
 * *pending* trust prompt; only an explicit accept applies it, and a decline
 * leaves the prior node intact.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, cleanup, fireEvent, waitFor, act } from '@testing-library/react'
import { NetworkProvider, useNetwork } from './network'
import { buildWalletRpcLink } from '../lib/custom-rpc-link'

// jsdom serves an opaque origin by default, so `localStorage` is unavailable.
// Provide a simple in-memory shim (matching the other context tests) so the
// network context's persistence helpers work under test.
const localStorageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => {
      store[key] = value
    },
    removeItem: (key: string) => {
      delete store[key]
    },
    clear: () => {
      store = {}
    },
  }
})()
Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock })

// jsdom serves an opaque origin by default, so `window.location.search` does not
// update via history and `localStorage` is unavailable. Install a deterministic
// location + history shim so the provider's deep-link effect (which reads
// `location.search`/`location.href` and strips the param via
// `history.replaceState`) behaves like a real same-origin navigation.
let currentUrl = new URL('https://wallet.botho.io/wallet')
Object.defineProperty(window, 'location', {
  configurable: true,
  get: () => currentUrl,
})
Object.defineProperty(window, 'history', {
  configurable: true,
  value: {
    replaceState: (_state: unknown, _title: string, url: string) => {
      currentUrl = new URL(url, currentUrl)
    },
  },
})

/** Probe that renders the network context's trust-gate surface into the DOM. */
function Probe() {
  const {
    ingressId,
    pendingRpcLink,
    customNodeFromLink,
    acceptPendingRpcLink,
    declinePendingRpcLink,
    revertCustomNode,
  } = useNetwork()
  return (
    <div>
      <span data-testid="ingress">{ingressId}</span>
      <span data-testid="pending-host">{pendingRpcLink ? pendingRpcLink.host : 'none'}</span>
      <span data-testid="pending-trust">{pendingRpcLink ? pendingRpcLink.trust : 'none'}</span>
      <span data-testid="from-link">{customNodeFromLink ?? 'none'}</span>
      <button onClick={() => void acceptPendingRpcLink()}>accept</button>
      <button onClick={declinePendingRpcLink}>decline</button>
      <button onClick={revertCustomNode}>revert</button>
    </div>
  )
}

/** Set window.location.search by replacing the wallet path. */
function setSearch(query: string) {
  currentUrl = new URL(`https://wallet.botho.io/wallet${query}`)
}

/** `?rpc=<encoded url>` query string for a given endpoint. */
function rpcQuery(rpcUrl: string): string {
  const link = buildWalletRpcLink('/wallet', rpcUrl)
  return link.slice(link.indexOf('?'))
}

describe('custom-RPC deep link is gated by the NetworkProvider (#587)', () => {
  beforeEach(() => {
    localStorage.clear()
    setSearch('')
    // fetch is only exercised on accept (reachability check); default to online.
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: true,
        json: async () => ({ jsonrpc: '2.0', id: 1, result: { chainHeight: 1, synced: true } }),
      })),
    )
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    localStorage.clear()
    setSearch('')
  })

  it('does NOT switch the active node on load — it raises a pending prompt instead', async () => {
    setSearch(rpcQuery('https://evil.example/rpc'))
    render(
      <NetworkProvider>
        <Probe />
      </NetworkProvider>,
    )

    // The active node is still the default seed — the link did not touch it.
    expect(screen.getByTestId('ingress').textContent).toBe('seed')
    // But the link is surfaced as a pending prompt for the unknown host.
    await waitFor(() => expect(screen.getByTestId('pending-host').textContent).toBe('evil.example'))
    expect(screen.getByTestId('pending-trust').textContent).toBe('unknown')
    expect(screen.getByTestId('from-link').textContent).toBe('none')
    // The param is stripped from the URL so a refresh does not re-arm it.
    expect(window.location.search).toBe('')
  })

  it('decline leaves the prior node intact', async () => {
    setSearch(rpcQuery('https://evil.example/rpc'))
    render(
      <NetworkProvider>
        <Probe />
      </NetworkProvider>,
    )
    await waitFor(() => expect(screen.getByTestId('pending-host').textContent).toBe('evil.example'))

    fireEvent.click(screen.getByText('decline'))

    expect(screen.getByTestId('pending-host').textContent).toBe('none')
    expect(screen.getByTestId('ingress').textContent).toBe('seed')
    expect(screen.getByTestId('from-link').textContent).toBe('none')
    expect(localStorage.getItem('botho_custom_node_from_link')).toBeNull()
  })

  it('accept switches to the custom node and records the "from a link" marker', async () => {
    setSearch(rpcQuery('https://rig-x.testnet.botho.io/rpc'))
    render(
      <NetworkProvider>
        <Probe />
      </NetworkProvider>,
    )
    await waitFor(() =>
      expect(screen.getByTestId('pending-host').textContent).toBe('rig-x.testnet.botho.io'),
    )
    expect(screen.getByTestId('pending-trust').textContent).toBe('known')

    await act(async () => {
      fireEvent.click(screen.getByText('accept'))
    })

    await waitFor(() => expect(screen.getByTestId('ingress').textContent).toBe('custom'))
    expect(screen.getByTestId('from-link').textContent).toBe('rig-x.testnet.botho.io')
    expect(screen.getByTestId('pending-host').textContent).toBe('none')
    expect(localStorage.getItem('botho_custom_node_from_link')).toBe('rig-x.testnet.botho.io')
  })

  it('revert returns to the default node and clears the marker', async () => {
    setSearch(rpcQuery('https://rig-x.testnet.botho.io/rpc'))
    render(
      <NetworkProvider>
        <Probe />
      </NetworkProvider>,
    )
    await waitFor(() =>
      expect(screen.getByTestId('pending-host').textContent).toBe('rig-x.testnet.botho.io'),
    )
    await act(async () => {
      fireEvent.click(screen.getByText('accept'))
    })
    await waitFor(() => expect(screen.getByTestId('ingress').textContent).toBe('custom'))

    fireEvent.click(screen.getByText('revert'))

    expect(screen.getByTestId('ingress').textContent).toBe('seed')
    expect(screen.getByTestId('from-link').textContent).toBe('none')
    expect(localStorage.getItem('botho_custom_node_from_link')).toBeNull()
  })
})
