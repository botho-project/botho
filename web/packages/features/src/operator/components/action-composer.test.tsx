/**
 * @vitest-environment jsdom
 *
 * The composer's contract (#751, §3/§4/§6/§8.3):
 *  - disabled until an operator key is imported;
 *  - a MANDATORY dry-run preview shows the node's verdict AND the canonical
 *    envelope BEFORE any real apply (§4, §8.3);
 *  - the real apply is a SEPARATELY SIGNED dryRun:false envelope with a FRESH
 *    nonce — the UI cannot reuse the dry-run bytes (finding 1);
 *  - applied/refused state comes exclusively from the node response (anti-#541);
 *  - fleet per-node refusal is a first-class partial-failure outcome (§7.3).
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { ActionComposer } from './action-composer'
import { importOperatorKey } from '../key-vault'
import type { SessionSigner } from '../key-vault'
import type { FleetNode } from '../../network/types'
import fixtures from '../fixtures/operator-action-fixtures.json'

const NODES: FleetNode[] = [
  { id: 'a', name: 'Node A', rpcEndpoint: 'https://a.test/rpc' },
  { id: 'b', name: 'Node B', rpcEndpoint: 'https://b.test/rpc' },
]

const PEER_A = '12D3KooWA1111111111111111111111111111111111111111111'
const PEER_B = '12D3KooWB2222222222222222222222222222222222222222222'

function outcomeBody(outcome: string, dryRun: boolean) {
  return {
    outcome,
    dryRun,
    signerKeyId: fixtures.signerKeyId,
    action: 'quorum.set_max_auto_members',
    message: outcome,
    auditTag: outcome,
    authenticated: true,
    resultingQuorum: { mode: 'recommended', members: [], maxAutoMembers: 8 },
    gate: {
      intersectionRefused: outcome !== 'applied',
      curatedMembers: 0,
      autoMembers: 3,
      suppressedPeers: 0,
      maxAutoMembers: 8,
      faultTolerant: true,
      degenerate: false,
    },
  }
}

/**
 * Wire a fetch mock that:
 *  - answers node_getStatus with each node's PeerId,
 *  - records every operator_submitAction body so tests can assert on the
 *    signed envelopes (dryRun flag + nonce), and returns a scripted outcome.
 */
function wireFetch(submitOutcome: (url: string, dryRun: boolean) => unknown) {
  const submitBodies: { url: string; params: { envelope: string; signature: string } }[] = []
  const fetchMock = vi.fn().mockImplementation((url: string, init: { body: string }) => {
    const req = JSON.parse(init.body)
    if (req.method === 'node_getStatus') {
      const peerId = url.includes('a.test') ? PEER_A : PEER_B
      return Promise.resolve({ ok: true, status: 200, json: async () => ({ result: { peerId } }) })
    }
    // operator_submitAction
    const params = req.params as { envelope: string; signature: string }
    submitBodies.push({ url, params })
    const parsed = JSON.parse(params.envelope) as { dryRun: boolean }
    const body = submitOutcome(url, parsed.dryRun)
    return Promise.resolve({ ok: true, status: 200, json: async () => body })
  })
  vi.stubGlobal('fetch', fetchMock)
  return submitBodies
}

async function makeSigner(): Promise<SessionSigner> {
  return importOperatorKey(fixtures.signingKeySeed, 'test-passphrase')
}

/** Fill the compose form for a set_max_auto_members action on both nodes. */
function fillMaxAuto() {
  fireEvent.change(screen.getByLabelText('Action'), {
    target: { value: 'quorum.set_max_auto_members' },
  })
  fireEvent.change(screen.getByLabelText(/maxAutoMembers/), { target: { value: '8' } })
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('ActionComposer', () => {
  it('is disabled until a key is imported', () => {
    render(<ActionComposer nodes={NODES} signer={null} />)
    expect(screen.getByText(/Import the operator signing key/)).toBeTruthy()
  })

  it('dry-run preview shows the canonical envelope AND the node verdict before apply', async () => {
    wireFetch((_url, dryRun) => ({ result: outcomeBody('applied', dryRun) }))
    const signer = await makeSigner()
    render(<ActionComposer nodes={NODES} signer={signer} />)
    fillMaxAuto()
    fireEvent.click(screen.getByText('Dry-run preview'))

    // The canonical envelope for each node is displayed (§8.3).
    await waitFor(() => expect(screen.getByTestId('canonical-a')).toBeTruthy())
    const canonicalA = screen.getByTestId('canonical-a').textContent ?? ''
    expect(canonicalA).toContain('"dryRun":true')
    expect(canonicalA).toContain(`"targetNode":"${PEER_A}"`)
    // The node's verdict is shown before any real apply.
    expect(screen.getAllByText('would apply').length).toBeGreaterThan(0)
    // The apply button exists but nothing has been applied yet.
    expect(screen.getByText(/Sign & apply/)).toBeTruthy()
    expect(screen.queryByTestId('apply-result')).toBeNull()
  })

  it('the real apply is a separately-signed dryRun:false envelope with a FRESH nonce (finding 1)', async () => {
    const bodies = wireFetch((_url, dryRun) => ({ result: outcomeBody('applied', dryRun) }))
    const signer = await makeSigner()
    render(<ActionComposer nodes={NODES} signer={signer} />)
    fillMaxAuto()
    fireEvent.click(screen.getByText('Dry-run preview'))
    await waitFor(() => expect(screen.getByTestId('canonical-a')).toBeTruthy())

    fireEvent.click(screen.getByText(/Sign & apply/))
    await waitFor(() => expect(screen.getByTestId('apply-result')).toBeTruthy())

    // Two dry-run envelopes (one per node) + two apply envelopes were submitted.
    const dryRuns = bodies.filter((b) => JSON.parse(b.params.envelope).dryRun === true)
    const applies = bodies.filter((b) => JSON.parse(b.params.envelope).dryRun === false)
    expect(dryRuns).toHaveLength(2)
    expect(applies).toHaveLength(2)

    // For node A: the apply envelope is DIFFERENT bytes, DIFFERENT signature,
    // and a DIFFERENT nonce than the dry-run — the UI never flipped a flag.
    const dryA = dryRuns.find((b) => b.url.includes('a.test'))!
    const applyA = applies.find((b) => b.url.includes('a.test'))!
    expect(applyA.params.envelope).not.toBe(dryA.params.envelope)
    expect(applyA.params.signature).not.toBe(dryA.params.signature)
    const dryNonce = JSON.parse(dryA.params.envelope).nonce
    const applyNonce = JSON.parse(applyA.params.envelope).nonce
    expect(applyNonce).not.toBe(dryNonce)
  })

  it('renders applied EXCLUSIVELY from the node response (anti-#541)', async () => {
    wireFetch((_url, dryRun) => ({ result: outcomeBody('applied', dryRun) }))
    const signer = await makeSigner()
    render(<ActionComposer nodes={NODES} signer={signer} />)
    fillMaxAuto()
    fireEvent.click(screen.getByText('Dry-run preview'))
    await waitFor(() => expect(screen.getByTestId('canonical-a')).toBeTruthy())
    fireEvent.click(screen.getByText(/Sign & apply/))
    await waitFor(() => expect(screen.getByTestId('apply-result')).toBeTruthy())
    // Both nodes report applied.
    expect(screen.getAllByText('applied').length).toBe(2)
  })

  it('fleet per-node refusal is a first-class partial-failure outcome (§7.3)', async () => {
    // Node A applies; node B refuses (gate_refused).
    wireFetch((url, dryRun) => {
      if (url.includes('b.test')) {
        return { error: { code: -32024, data: outcomeBody('gate_refused', dryRun) } }
      }
      return { result: outcomeBody('applied', dryRun) }
    })
    const signer = await makeSigner()
    render(<ActionComposer nodes={NODES} signer={signer} />)
    fillMaxAuto()
    fireEvent.click(screen.getByText('Dry-run preview'))
    await waitFor(() => expect(screen.getByTestId('canonical-a')).toBeTruthy())
    fireEvent.click(screen.getByText(/Sign & apply/))
    await waitFor(() => expect(screen.getByTestId('apply-result')).toBeTruthy())

    // The partial-failure banner is shown.
    expect(screen.getByTestId('partial-failure')).toBeTruthy()
    expect(screen.getByText('applied')).toBeTruthy()
    expect(screen.getByText('gate_refused')).toBeTruthy()
  })
})
