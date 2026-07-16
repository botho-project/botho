/**
 * @vitest-environment jsdom
 *
 * Smoke coverage for the Tier 0 `/trade` discovery page (#1030): the venue
 * directory, reused peg-health card, and the guided export explainer all
 * render, and the seeded deep-links point at the right venues. The reserve
 * poll is stubbed to fail so the page renders without a live backend (the
 * card degrades to its "unavailable" state, never fabricated values).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { TradePage } from './trade'
import i18n from '../lib/i18n'

beforeEach(() => {
  i18n.changeLanguage('en')
  // No metrics backend in tests — the reserve hook should degrade gracefully.
  vi.stubGlobal(
    'fetch',
    vi.fn(() => Promise.reject(new Error('no network in test'))),
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

function renderTrade() {
  return render(
    <MemoryRouter initialEntries={['/trade']}>
      <TradePage />
    </MemoryRouter>,
  )
}

describe('TradePage (Tier 0)', () => {
  it('renders the hero with an explicit testnet notice', () => {
    renderTrade()
    expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('Trade wrapped BTH')
    expect(screen.getByText(/not valid on mainnet/i)).toBeTruthy()
  })

  it('lists the seeded venues with working deep-links', () => {
    renderTrade()
    expect(screen.getByText('Uniswap v3')).toBeTruthy()
    expect(screen.getByText('Orca')).toBeTruthy()
    // Hyperliquid HIP-1 spot is marked coming soon (no trade link yet).
    expect(screen.getByText('Hyperliquid HIP-1 spot')).toBeTruthy()
    expect(screen.getAllByText(/Coming soon/i).length).toBeGreaterThan(0)

    const uni = screen.getByRole('link', { name: /Trade on Uniswap v3/i })
    expect(uni.getAttribute('href')).toContain('app.uniswap.org')
    expect(uni.getAttribute('href')).toContain(
      '0x49b985ec427ee771a601f11b18f7d4402fa2dd7b',
    )
    expect(uni.getAttribute('target')).toBe('_blank')
  })

  it('surfaces peg health and the guided export explainer', async () => {
    renderTrade()
    expect(screen.getByRole('heading', { name: 'Peg health' })).toBeTruthy()
    expect(screen.getByRole('heading', { name: /How to bridge BTH/i })).toBeTruthy()
    // The Tier 1 extension point renders a disabled export CTA.
    const cta = screen.getByRole('button', { name: 'Export BTH' })
    expect(cta.hasAttribute('disabled')).toBe(true)
    // Reserve card degrades to its unavailable placeholder (no live backend).
    await waitFor(() =>
      expect(screen.getByText(/Reserve proof unavailable/i)).toBeTruthy(),
    )
  })
})
