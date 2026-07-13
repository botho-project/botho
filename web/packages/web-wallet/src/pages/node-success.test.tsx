/**
 * @vitest-environment jsdom
 *
 * Success-page (`NodeSuccessPage`) state coverage (#805 part 1). The page
 * exchanges Stripe's `session_id` for a magic-link status URL via the
 * control-plane Worker, polling while provisioning lands. These tests assert the
 * pending → ready transition, the terminal-error state, the no-session
 * fallback, and the poll-exhausted fallback (#809 — must NOT promise an email,
 * which is env-gated), with `fetchSessionStatus` mocked (no network).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { NodeStatusError, type SessionStatus } from '../lib/node-status'

const fetchSessionStatusMock = vi.fn<(id: string) => Promise<SessionStatus>>()
vi.mock('../lib/node-status', async () => {
  const actual = await vi.importActual<typeof import('../lib/node-status')>('../lib/node-status')
  return {
    ...actual,
    fetchSessionStatus: (id: string) => fetchSessionStatusMock(id),
  }
})

// Imported AFTER the mock is registered.
import { NodeSuccessPage } from './node'
import i18n from '../lib/i18n'

function renderSuccess() {
  return render(
    <MemoryRouter initialEntries={['/node/success']}>
      <NodeSuccessPage />
    </MemoryRouter>,
  )
}

describe('NodeSuccessPage', () => {
  beforeEach(async () => {
    fetchSessionStatusMock.mockReset()
    await i18n.changeLanguage('en')
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
    window.history.replaceState({}, '', '/node/success')
  })

  it('shows the no-session fallback when no session_id is present', () => {
    window.history.replaceState({}, '', '/node/success')
    renderSuccess()
    expect(screen.getByRole('heading', { name: 'Subscription started' })).toBeTruthy()
    // No session_id → the plain email-fallback copy, no spinner poll.
    expect(fetchSessionStatusMock).not.toHaveBeenCalled()
    expect(screen.getByText(/Check your email/i)).toBeTruthy()
  })

  it('renders a "View your node status" link once the exchange is ready', async () => {
    window.history.replaceState({}, '', '/node/success?session_id=cs_test_abc')
    fetchSessionStatusMock.mockResolvedValue({
      kind: 'ready',
      statusUrl: 'https://botho.io/node/status?token=cus_A.1.sig',
    })
    renderSuccess()
    const link = (await screen.findByText('View your node status')).closest('a')
    expect(link?.getAttribute('href')).toBe('https://botho.io/node/status?token=cus_A.1.sig')
  })

  it('shows the pending spinner while provisioning, then the ready link', async () => {
    window.history.replaceState({}, '', '/node/success?session_id=cs_test_abc')
    fetchSessionStatusMock
      .mockResolvedValueOnce({ kind: 'pending' })
      .mockResolvedValue({
        kind: 'ready',
        statusUrl: 'https://botho.io/node/status?token=t',
      })
    renderSuccess()
    // First poll → pending copy.
    expect(await screen.findByText(/Setting up your node/i)).toBeTruthy()
    // Second poll (after the interval) → ready link.
    await waitFor(() => expect(screen.getByText('View your node status')).toBeTruthy(), {
      timeout: 6000,
    })
  })

  it('shows the still-provisioning fallback (no email promise) when polling exhausts', async () => {
    vi.useFakeTimers()
    window.history.replaceState({}, '', '/node/success?session_id=cs_test_slow')
    // Provisioning never lands within the attempt cap.
    fetchSessionStatusMock.mockResolvedValue({ kind: 'pending' })
    renderSuccess()
    // Drain the initial poll plus every retry interval up to the cap (20 × 3s).
    for (let i = 0; i < 20; i++) {
      await vi.advanceTimersByTimeAsync(3000)
    }
    expect(fetchSessionStatusMock).toHaveBeenCalledTimes(20)
    // Poll-exhaustion copy asks the user to refresh — it must NOT promise an
    // email (the status email is env-gated; #809).
    expect(screen.getByText(/still being set up/i)).toBeTruthy()
    expect(screen.queryByText(/check your email/i)).toBeNull()
  })

  it('shows the terminal error state on a 401 (stops polling)', async () => {
    window.history.replaceState({}, '', '/node/success?session_id=cs_bad')
    fetchSessionStatusMock.mockRejectedValue(new NodeStatusError('expired', 401))
    renderSuccess()
    expect(await screen.findByText(/couldn't confirm this checkout/i)).toBeTruthy()
    // Only one attempt — a terminal 401 does not retry.
    expect(fetchSessionStatusMock).toHaveBeenCalledTimes(1)
  })
})
