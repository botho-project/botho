/**
 * @vitest-environment jsdom
 *
 * The audit view's contract (#751, §6, anti-#541): it renders EXCLUSIVELY the
 * node's stored entries. An empty list shows "no actions", a failed fetch shows
 * an explicit unavailable state — never a fabricated success.
 */
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { AuditLogView } from './audit-log-view'
import type { AuditEntry } from '../audit'

afterEach(cleanup)

const applied: AuditEntry = {
  ts: 1_800_000_000,
  signerKeyId: 'c5e21ab1c9f6022d',
  envelopeHash: 'deadbeef'.repeat(8),
  action: 'quorum.pin_member',
  params: { peerId: '12D3KooWfake' },
  dryRun: false,
  outcome: 'applied',
  newQuorum: { mode: 'recommended', members: ['12D3KooWfake'], maxAutoMembers: 8 },
}

const refused: AuditEntry = {
  ts: 1_800_000_100,
  signerKeyId: 'c5e21ab1c9f6022d',
  envelopeHash: 'cafe'.repeat(16),
  action: 'quorum.unpin_member',
  params: { peerId: '12D3KooWfake' },
  dryRun: false,
  outcome: 'gate_refused',
}

describe('AuditLogView', () => {
  it('renders stored entries with their node-reported outcomes', () => {
    render(<AuditLogView entries={[refused, applied]} />)
    expect(screen.getByTestId('audit-log-view')).toBeTruthy()
    expect(screen.getByText('applied')).toBeTruthy()
    expect(screen.getByText('gate_refused')).toBeTruthy()
    expect(screen.getByText('quorum.pin_member')).toBeTruthy()
  })

  it('shows an explicit empty state when there are no entries', () => {
    render(<AuditLogView entries={[]} />)
    expect(screen.getByText(/No operator actions recorded/)).toBeTruthy()
    expect(screen.queryByTestId('audit-log-view')).toBeNull()
  })

  it('shows an explicit unavailable state on fetch failure (never fabricated entries)', () => {
    render(<AuditLogView entries={[]} unavailable />)
    expect(screen.getByText(/Audit log unavailable/)).toBeTruthy()
    expect(screen.queryByTestId('audit-log-view')).toBeNull()
  })

  it('does NOT show a dry-run entry as applied', () => {
    render(<AuditLogView entries={[{ ...applied, dryRun: true }]} />)
    // The applied badge is only for real applies; a dry-run renders its outcome tag.
    expect(screen.queryByText('applied')).toBeNull()
  })
})
