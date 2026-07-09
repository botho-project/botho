import { useMemo, useState } from 'react'
import { AlertTriangle, CheckCircle2, Eye, KeyRound, ShieldAlert, XCircle } from 'lucide-react'
import { Card, CardContent } from '@botho/ui'
import type { FleetNode } from '../../network/types'
import {
  buildDryRun,
  buildRealApply,
  MAX_AUTO_MEMBERS_CEILING,
  type ComposeActionRequest,
  type OperatorActionName,
  type SignedActionEnvelope,
} from '../action-envelope'
import {
  fetchNodePeerId,
  submitToFleet,
  type FleetSubmitItem,
  type FleetSubmitOutcome,
  type SubmitResult,
} from '../action-submit'
import type { SessionSigner } from '../key-vault'

/**
 * The action composer (#751, §3/§4/§6/§8.3): compose a v1 quorum-curation
 * action, sign a MANDATORY dry-run, review the node's verdict + the canonical
 * envelope, then sign a SEPARATE real-apply envelope (fresh nonce, finding 1).
 * Applied/refused state comes exclusively from the node's response (anti-#541).
 *
 * State machine:
 *   compose → (sign dryRun, submit) → preview → (sign fresh apply, submit) → result
 * The operator sees the canonical bytes at BOTH the preview and the apply
 * confirmation, before any signature is sent (§8.3 mitigation).
 */

type Phase = 'compose' | 'previewing' | 'preview' | 'applying' | 'result'

export interface ActionComposerProps {
  nodes: FleetNode[]
  /** The unlocked session signer; null ⇒ the composer is disabled. */
  signer: SessionSigner | null
  className?: string
}

interface PreviewState {
  /** The per-node signed dry-run envelopes (for display) + node results. */
  entries: { node: FleetNode; signed: SignedActionEnvelope; result: SubmitResult }[]
  /** The compose request captured at preview time (reused to build the apply). */
  request: Omit<ComposeActionRequest, 'targetNode'>
  /** Resolved targetNode per node id at preview time. */
  targetNodeById: Record<string, string>
}

export function ActionComposer({ nodes, signer, className }: ActionComposerProps) {
  const [action, setAction] = useState<OperatorActionName>('quorum.pin_member')
  const [peerId, setPeerId] = useState('')
  const [maxAuto, setMaxAuto] = useState('8')
  const [ack, setAck] = useState(false)
  const [selectedNodeIds, setSelectedNodeIds] = useState<string[]>(nodes.map((n) => n.id))
  const [phase, setPhase] = useState<Phase>('compose')
  const [error, setError] = useState<string | null>(null)
  const [preview, setPreview] = useState<PreviewState | null>(null)
  const [result, setResult] = useState<FleetSubmitOutcome | null>(null)

  const disabled = signer === null

  const selectedNodes = useMemo(
    () => nodes.filter((n) => selectedNodeIds.includes(n.id)),
    [nodes, selectedNodeIds],
  )

  function buildRequestBase(): Omit<ComposeActionRequest, 'targetNode'> {
    const params =
      action === 'quorum.set_max_auto_members'
        ? { value: Number.parseInt(maxAuto, 10) }
        : { peerId: peerId.trim() }
    return {
      action,
      params,
      signerKeyId: signer!.signerKeyId,
      acknowledgeDegenerate: ack ? true : undefined,
    }
  }

  /**
   * Step 1: sign a dryRun:true envelope PER node (resolving each node's PeerId
   * live), submit, and show the operator the node's verdict + the canonical
   * bytes before any real apply.
   */
  async function onPreview() {
    setError(null)
    if (!signer) {
      setError('Import the operator key first.')
      return
    }
    if (selectedNodes.length === 0) {
      setError('Select at least one target node.')
      return
    }
    setPhase('previewing')
    try {
      const base = buildRequestBase()
      const targetNodeById: Record<string, string> = {}
      const entries: PreviewState['entries'] = []
      for (const node of selectedNodes) {
        const target = await fetchNodePeerId(node)
        if (!target) {
          entries.push({
            node,
            // Cannot sign for a node whose PeerId is unconfirmed.
            signed: {
              canonical: '',
              signature: '',
              envelopeHash: '',
              fields: {} as SignedActionEnvelope['fields'],
            },
            result: { status: 'unreachable', message: 'could not resolve node PeerId' },
          })
          continue
        }
        targetNodeById[node.id] = target
        const signed = buildDryRun({ ...base, targetNode: target }, signer.sign)
        const result = (await submitToFleet([{ node, signed }])).entries[0].result
        entries.push({ node, signed, result })
      }
      setPreview({ entries, request: base, targetNodeById })
      setPhase('preview')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'preview failed')
      setPhase('compose')
    }
  }

  /**
   * Step 2: sign a SEPARATE dryRun:false envelope with a FRESH nonce per node
   * (finding 1 — the UI never reuses the dry-run bytes), submit, and render the
   * node-confirmed outcome.
   */
  async function onApply() {
    if (!signer || !preview) return
    setPhase('applying')
    setError(null)
    try {
      const items: FleetSubmitItem[] = []
      for (const { node } of preview.entries) {
        const target = preview.targetNodeById[node.id]
        if (!target) continue // node we could not target at preview time
        const signed = buildRealApply({ ...preview.request, targetNode: target }, signer.sign)
        items.push({ node, signed })
      }
      const outcome = await submitToFleet(items)
      setResult(outcome)
      setPhase('result')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'apply failed')
      setPhase('preview')
    }
  }

  function reset() {
    setPreview(null)
    setResult(null)
    setError(null)
    setPhase('compose')
  }

  if (disabled) {
    return (
      <Card className={className}>
        <CardContent className="flex items-center gap-3 py-6 text-ghost">
          <KeyRound className="h-5 w-5 shrink-0" />
          <span>Import the operator signing key to compose actions.</span>
        </CardContent>
      </Card>
    )
  }

  return (
    <div className={`space-y-4 ${className ?? ''}`} data-testid="action-composer">
      {error && (
        <div
          role="alert"
          className="flex items-center gap-2 rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-sm text-red-300"
        >
          <AlertTriangle className="h-4 w-4 shrink-0" />
          {error}
        </div>
      )}

      {(phase === 'compose' || phase === 'previewing') && (
        <ComposeForm
          action={action}
          setAction={setAction}
          peerId={peerId}
          setPeerId={setPeerId}
          maxAuto={maxAuto}
          setMaxAuto={setMaxAuto}
          ack={ack}
          setAck={setAck}
          nodes={nodes}
          selectedNodeIds={selectedNodeIds}
          setSelectedNodeIds={setSelectedNodeIds}
          busy={phase === 'previewing'}
          onPreview={onPreview}
        />
      )}

      {(phase === 'preview' || phase === 'applying') && preview && (
        <PreviewPanel
          preview={preview}
          busy={phase === 'applying'}
          onApply={onApply}
          onCancel={reset}
        />
      )}

      {phase === 'result' && result && <ResultPanel result={result} onDone={reset} />}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Compose form
// ---------------------------------------------------------------------------

interface ComposeFormProps {
  action: OperatorActionName
  setAction: (a: OperatorActionName) => void
  peerId: string
  setPeerId: (s: string) => void
  maxAuto: string
  setMaxAuto: (s: string) => void
  ack: boolean
  setAck: (b: boolean) => void
  nodes: FleetNode[]
  selectedNodeIds: string[]
  setSelectedNodeIds: (ids: string[]) => void
  busy: boolean
  onPreview: () => void
}

function ComposeForm(props: ComposeFormProps) {
  const isPeerAction = props.action !== 'quorum.set_max_auto_members'
  const toggleNode = (id: string) => {
    props.setSelectedNodeIds(
      props.selectedNodeIds.includes(id)
        ? props.selectedNodeIds.filter((x) => x !== id)
        : [...props.selectedNodeIds, id],
    )
  }
  return (
    <Card>
      <CardContent className="space-y-4 py-5">
        <div className="space-y-1">
          <label htmlFor="op-action" className="text-sm text-ghost">
            Action
          </label>
          <select
            id="op-action"
            value={props.action}
            onChange={(e) => props.setAction(e.target.value as OperatorActionName)}
            className="w-full rounded border border-steel bg-abyss px-3 py-2 text-sm text-light"
          >
            <option value="quorum.pin_member">quorum.pin_member</option>
            <option value="quorum.unpin_member">quorum.unpin_member</option>
            <option value="quorum.set_max_auto_members">quorum.set_max_auto_members</option>
          </select>
        </div>

        {isPeerAction ? (
          <div className="space-y-1">
            <label htmlFor="op-peer" className="text-sm text-ghost">
              Member PeerId (base58)
            </label>
            <input
              id="op-peer"
              value={props.peerId}
              onChange={(e) => props.setPeerId(e.target.value)}
              placeholder="12D3Koo..."
              className="w-full rounded border border-steel bg-abyss px-3 py-2 font-mono text-sm text-light"
            />
          </div>
        ) : (
          <div className="space-y-1">
            <label htmlFor="op-max" className="text-sm text-ghost">
              maxAutoMembers (0..{MAX_AUTO_MEMBERS_CEILING})
            </label>
            <input
              id="op-max"
              type="number"
              min={0}
              max={MAX_AUTO_MEMBERS_CEILING}
              value={props.maxAuto}
              onChange={(e) => props.setMaxAuto(e.target.value)}
              className="w-full rounded border border-steel bg-abyss px-3 py-2 text-sm text-light"
            />
          </div>
        )}

        <label className="flex items-center gap-2 text-sm text-ghost">
          <input
            type="checkbox"
            checked={props.ack}
            onChange={(e) => props.setAck(e.target.checked)}
          />
          acknowledgeDegenerate (required only to shrink below the 4-node BFT floor)
        </label>

        <div className="space-y-1">
          <span className="text-sm text-ghost">Target nodes (one signed envelope per node)</span>
          <div className="flex flex-wrap gap-2">
            {props.nodes.map((n) => (
              <label
                key={n.id}
                className="flex items-center gap-1.5 rounded border border-steel px-2 py-1 text-xs text-light"
              >
                <input
                  type="checkbox"
                  checked={props.selectedNodeIds.includes(n.id)}
                  onChange={() => toggleNode(n.id)}
                />
                {n.name}
              </label>
            ))}
          </div>
        </div>

        <button
          type="button"
          onClick={props.onPreview}
          disabled={props.busy}
          className="flex items-center gap-2 rounded bg-[--color-slate] px-4 py-2 text-sm font-medium text-light disabled:opacity-50"
        >
          <Eye className="h-4 w-4" />
          {props.busy ? 'Signing dry-run…' : 'Dry-run preview'}
        </button>
      </CardContent>
    </Card>
  )
}

// ---------------------------------------------------------------------------
// Preview panel — shows canonical bytes + node verdict before real apply
// ---------------------------------------------------------------------------

function PreviewPanel({
  preview,
  busy,
  onApply,
  onCancel,
}: {
  preview: PreviewState
  busy: boolean
  onApply: () => void
  onCancel: () => void
}) {
  const anyTargetable = preview.entries.some((e) => e.signed.canonical !== '')
  return (
    <Card>
      <CardContent className="space-y-4 py-5">
        <div className="flex items-center gap-2 text-sm font-medium text-light">
          <Eye className="h-4 w-4" />
          Dry-run preview — review before signing the real apply
        </div>
        <p className="text-xs text-ghost">
          The real apply is a SEPARATELY SIGNED envelope with a fresh nonce. Review the exact
          canonical bytes below (what you will sign) and each node&apos;s verdict.
        </p>

        {preview.entries.map(({ node, signed, result }) => (
          <div key={node.id} className="space-y-2 rounded border border-steel p-3">
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium text-light">{node.name}</span>
              <VerdictBadge result={result} />
            </div>
            {signed.canonical ? (
              <pre
                data-testid={`canonical-${node.id}`}
                className="overflow-x-auto rounded bg-abyss p-2 font-mono text-[11px] text-ghost"
              >
                {signed.canonical}
              </pre>
            ) : (
              <span className="text-xs text-red-300">
                Not targetable: {result.status === 'unreachable' ? result.message : result.status}
              </span>
            )}
            {result.status === 'ok' && <QuorumPreview result={result} />}
          </div>
        ))}

        <div className="flex gap-2">
          <button
            type="button"
            onClick={onApply}
            disabled={busy || !anyTargetable}
            className="flex items-center gap-2 rounded bg-emerald-600/80 px-4 py-2 text-sm font-medium text-white disabled:opacity-50"
          >
            <CheckCircle2 className="h-4 w-4" />
            {busy ? 'Signing apply…' : 'Sign & apply (fresh signature)'}
          </button>
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            className="rounded border border-steel px-4 py-2 text-sm text-ghost disabled:opacity-50"
          >
            Cancel
          </button>
        </div>
      </CardContent>
    </Card>
  )
}

function QuorumPreview({ result }: { result: Extract<SubmitResult, { status: 'ok' }> }) {
  const q = result.outcome.resultingQuorum
  const g = result.outcome.gate
  return (
    <div className="text-xs text-ghost">
      {q && (
        <div>
          Resulting quorum: mode {q.mode}, {q.members.length} curated member(s), cap{' '}
          {q.maxAutoMembers}
        </div>
      )}
      {g && (
        <div className="flex items-center gap-1">
          {g.degenerate && <ShieldAlert className="h-3 w-3 text-amber-400" />}
          {g.intersectionRefused ? 'intersection REFUSED' : 'intersection ok'} · fault-tolerant:{' '}
          {String(g.faultTolerant)}
        </div>
      )}
    </div>
  )
}

function VerdictBadge({ result }: { result: SubmitResult }) {
  if (result.status !== 'ok') {
    return (
      <span className="rounded bg-steel/40 px-2 py-0.5 text-[11px] text-ghost">
        {result.status}
      </span>
    )
  }
  const cls = result.outcome.outcome
  if (cls === 'applied') {
    return (
      <span className="rounded bg-emerald-600/30 px-2 py-0.5 text-[11px] text-emerald-300">
        would apply
      </span>
    )
  }
  return (
    <span className="rounded bg-red-500/20 px-2 py-0.5 text-[11px] text-red-300">
      {result.outcome.auditTag}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Result panel — node-confirmed apply outcomes ONLY (anti-#541)
// ---------------------------------------------------------------------------

function ResultPanel({ result, onDone }: { result: FleetSubmitOutcome; onDone: () => void }) {
  return (
    <Card>
      <CardContent className="space-y-3 py-5" data-testid="apply-result">
        <div className="text-sm font-medium text-light">Apply result (from node responses)</div>

        {result.partial && (
          <div
            role="alert"
            data-testid="partial-failure"
            className="flex items-center gap-2 rounded border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm text-amber-300"
          >
            <AlertTriangle className="h-4 w-4 shrink-0" />
            Partial failure: {result.appliedNodeIds.length} node(s) applied,{' '}
            {result.refusedNodeIds.length} refused, {result.inconclusiveNodeIds.length} inconclusive.
          </div>
        )}

        {result.entries.map((e) => (
          <div
            key={e.nodeId}
            className="flex items-center justify-between rounded border border-steel px-3 py-2"
          >
            <span className="text-sm text-light">{e.nodeName}</span>
            <ResultBadge result={e.result} />
          </div>
        ))}

        <button
          type="button"
          onClick={onDone}
          className="rounded border border-steel px-4 py-2 text-sm text-ghost"
        >
          Compose another
        </button>
      </CardContent>
    </Card>
  )
}

function ResultBadge({ result }: { result: SubmitResult }) {
  if (result.status !== 'ok') {
    return (
      <span className="flex items-center gap-1 text-xs text-ghost">
        <XCircle className="h-3.5 w-3.5" />
        {result.status}
        {result.status !== 'not-enabled' ? '' : ''}
      </span>
    )
  }
  const cls = result.outcome.outcome
  if (cls === 'applied' && !result.outcome.dryRun) {
    return (
      <span className="flex items-center gap-1 text-xs text-emerald-300">
        <CheckCircle2 className="h-3.5 w-3.5" />
        applied
      </span>
    )
  }
  return (
    <span className="flex items-center gap-1 text-xs text-red-300">
      <XCircle className="h-3.5 w-3.5" />
      {result.outcome.auditTag}
    </span>
  )
}
