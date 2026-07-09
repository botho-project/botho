import { CheckCircle2, FileClock, XCircle } from 'lucide-react'
import { Card, CardContent } from '@botho/ui'
import type { AuditEntry } from '../audit'

/**
 * Audit-log view (#751, §6, anti-#541): renders EXCLUSIVELY the node's stored
 * audit entries (#750's `OperatorAuditEntry`). It never fabricates, infers, or
 * optimistically shows an outcome — an empty list is an empty list, and a
 * failed fetch renders an explicit unavailable state, not a blank success.
 */

export interface AuditLogViewProps {
  /** The node's stored entries (newest first, as the node serves them). */
  entries: AuditEntry[]
  /** True when the audit fetch failed / is unauthorized — show, don't fake. */
  unavailable?: boolean
  className?: string
}

export function AuditLogView({ entries, unavailable, className }: AuditLogViewProps) {
  if (unavailable) {
    return (
      <Card className={className}>
        <CardContent className="flex items-center gap-3 py-6 text-ghost">
          <FileClock className="h-5 w-5 shrink-0" />
          Audit log unavailable (unreachable or unauthorized). No entries are shown rather than
          fabricated ones.
        </CardContent>
      </Card>
    )
  }

  if (entries.length === 0) {
    return (
      <Card className={className}>
        <CardContent className="flex items-center gap-3 py-6 text-ghost">
          <FileClock className="h-5 w-5 shrink-0" />
          No operator actions recorded on this node yet.
        </CardContent>
      </Card>
    )
  }

  return (
    <div className={`space-y-2 ${className ?? ''}`} data-testid="audit-log-view">
      {entries.map((e, i) => (
        <AuditRow key={`${e.envelopeHash}-${e.ts}-${i}`} entry={e} />
      ))}
    </div>
  )
}

function AuditRow({ entry }: { entry: AuditEntry }) {
  const applied = entry.outcome === 'applied'
  const when = entry.ts ? new Date(entry.ts * 1000).toISOString() : 'unknown time'
  return (
    <Card>
      <CardContent className="space-y-1 py-3">
        <div className="flex items-center justify-between">
          <span className="font-mono text-sm text-light">{entry.action}</span>
          <OutcomeBadge outcome={entry.outcome} applied={applied} dryRun={entry.dryRun} />
        </div>
        <div className="flex flex-wrap gap-x-4 gap-y-0.5 text-xs text-ghost">
          <span>{when}</span>
          <span>signer {entry.signerKeyId || '—'}</span>
          {entry.dryRun && <span className="text-amber-400">dry-run</span>}
          <span className="font-mono">env {entry.envelopeHash.slice(0, 12)}…</span>
        </div>
        <pre className="overflow-x-auto rounded bg-abyss p-1.5 font-mono text-[11px] text-ghost">
          {JSON.stringify(entry.params)}
        </pre>
      </CardContent>
    </Card>
  )
}

function OutcomeBadge({
  outcome,
  applied,
  dryRun,
}: {
  outcome: string
  applied: boolean
  dryRun: boolean
}) {
  if (applied && !dryRun) {
    return (
      <span className="flex items-center gap-1 text-xs text-emerald-300">
        <CheckCircle2 className="h-3.5 w-3.5" />
        applied
      </span>
    )
  }
  if (applied && dryRun) {
    // A dry-run that the gate WOULD accept: never shown as "applied" (nothing
    // was installed). Label it as a preview so it is unmistakable (anti-#541).
    return (
      <span className="flex items-center gap-1 text-xs text-amber-300">
        dry-run (would apply)
      </span>
    )
  }
  return (
    <span className="flex items-center gap-1 text-xs text-red-300">
      <XCircle className="h-3.5 w-3.5" />
      {outcome}
    </span>
  )
}
