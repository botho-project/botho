import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Logo } from '@botho/ui'
import {
  ActionComposer,
  AuditLogView,
  captureOperatorToken,
  importOperatorKey,
  NetworkDashboard,
  TrustDashboard,
  useFleetHistory,
  useFleetStatus,
  useOperatorAuditLog,
  useOperatorQuorumInfo,
  useTrustStatus,
  type SessionSigner,
} from '@botho/features'
import { ArrowLeft, KeyRound } from 'lucide-react'
import { FLEET, METRICS_API_BASE } from '../config/fleet'

/**
 * Operator dashboard — public read surface (#706, P4.1 of the #695 proposal).
 *
 * Two tabs over exclusively public RPC data — reads only, no auth, no write
 * affordances:
 * - Fleet: the same `NetworkDashboard` + fleet hooks as `/network` (one
 *   implementation, re-parented — not a forked copy).
 * - Trust: per-node quorum posture from the promotion gate (#651/#509)
 *   merged with the live `network_getPeers` table (#544).
 *
 * Each tab mounts its own polling hook, so only the visible tab polls.
 */
type OperatorTab = 'fleet' | 'trust' | 'actions' | 'audit'

export function OperatorPage() {
  const [tab, setTab] = useState<OperatorTab>('fleet')
  // Lift any magic-link read token out of the URL fragment into sessionStorage
  // on mount (#707), then strip it from the address bar. Null ⇒ public view.
  const [token] = useState<string | null>(() => captureOperatorToken())
  // The operator signing key is imported into an in-memory, session-only vault
  // (#751, §2). It NEVER touches the server (the Pages host serves static
  // assets only) and is held here for the composer's lifetime.
  const [signer, setSigner] = useState<SessionSigner | null>(null)

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">
              Operator
            </span>
            <span className="font-display text-base font-semibold sm:hidden">Operator</span>
          </Link>
          <nav className="flex items-center gap-4">
            <Link
              to="/network"
              className="text-sm text-ghost hover:text-light transition-colors"
            >
              Network
            </Link>
            <Link
              to="/explorer"
              className="text-sm text-ghost hover:text-light transition-colors"
            >
              Block Explorer
            </Link>
          </nav>
        </div>
      </header>

      <main className="py-6 sm:py-8">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 space-y-4">
          <div role="tablist" aria-label="Operator views" className="flex gap-1">
            <TabButton active={tab === 'fleet'} onClick={() => setTab('fleet')}>
              Fleet
            </TabButton>
            <TabButton active={tab === 'trust'} onClick={() => setTab('trust')}>
              Trust
            </TabButton>
            <TabButton active={tab === 'actions'} onClick={() => setTab('actions')}>
              Actions
            </TabButton>
            <TabButton active={tab === 'audit'} onClick={() => setTab('audit')}>
              Audit
            </TabButton>
          </div>

          {tab === 'fleet' && <FleetTab />}
          {tab === 'trust' && <TrustTab token={token} />}
          {tab === 'actions' && <ActionsTab signer={signer} setSigner={setSigner} />}
          {tab === 'audit' && <AuditTab token={token} />}
        </div>
      </main>
    </div>
  )
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={`rounded px-3 py-1.5 text-sm transition-colors ${
        active
          ? 'bg-[--color-slate] font-medium text-[--color-light]'
          : 'text-ghost hover:text-light'
      }`}
    >
      {children}
    </button>
  )
}

/** Identical wiring to `/network` — the shared hooks ARE the page logic. */
function FleetTab() {
  const { statuses, avgBlockSeconds } = useFleetStatus(FLEET)
  const { history, historyState } = useFleetHistory(FLEET, METRICS_API_BASE)
  return (
    <NetworkDashboard
      nodes={FLEET}
      statuses={statuses}
      avgBlockSeconds={avgBlockSeconds}
      history={history}
      historyState={historyState}
    />
  )
}

/**
 * Trust tab (#706), upgraded for #707: when a valid read token is present it
 * additionally polls `operator_getQuorumInfo` and renders per-peer
 * classification badges + the configured-members panel. Without a token it
 * degrades cleanly to the public read-only view.
 */
function TrustTab({ token }: { token: string | null }) {
  const { statuses } = useTrustStatus(FLEET)
  const { info, mode } = useOperatorQuorumInfo(FLEET, token)
  return (
    <TrustDashboard
      nodes={FLEET}
      statuses={statuses}
      operatorInfo={token ? info : undefined}
      operatorMode={mode}
    />
  )
}

/**
 * Actions tab (#751): import the operator signing key into an in-memory,
 * passphrase-encrypted session vault (§2), then compose signed quorum-curation
 * actions with a mandatory dry-run preview (§4). The key never leaves the
 * browser (§8.3).
 *
 * MAINNET HARDENING (§8.3, §9): for testnet the residual risk of a malicious
 * Pages bundle prompting the operator for a signature is ACCEPTED. Before
 * mainnet the operator page must ship with Subresource Integrity (SRI) and/or
 * be self-hosted so the operator controls the exact bundle they run. Tracked as
 * follow-up issue #757 (the mainnet-hardening list from §9).
 */
function ActionsTab({
  signer,
  setSigner,
}: {
  signer: SessionSigner | null
  setSigner: (s: SessionSigner | null) => void
}) {
  return (
    <div className="space-y-4">
      <KeyImportPanel signer={signer} setSigner={setSigner} />
      <ActionComposer nodes={FLEET} signer={signer} />
    </div>
  )
}

function KeyImportPanel({
  signer,
  setSigner,
}: {
  signer: SessionSigner | null
  setSigner: (s: SessionSigner | null) => void
}) {
  const [secret, setSecret] = useState('')
  const [pass, setPass] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  async function onImport() {
    setError(null)
    setBusy(true)
    try {
      const s = await importOperatorKey(secret, pass)
      setSigner(s)
      // Clear the raw secret + passphrase from the form inputs immediately.
      setSecret('')
      setPass('')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'import failed')
    } finally {
      setBusy(false)
    }
  }

  function onLock() {
    signer?.wipe()
    setSigner(null)
  }

  if (signer) {
    return (
      <div className="flex items-center justify-between rounded border border-steel bg-abyss/50 px-4 py-3">
        <div className="flex items-center gap-2 text-sm text-light">
          <KeyRound className="h-4 w-4 text-emerald-400" />
          Operator key loaded · signerKeyId <span className="font-mono">{signer.signerKeyId}</span>
        </div>
        <button
          type="button"
          onClick={onLock}
          className="rounded border border-steel px-3 py-1.5 text-xs text-ghost"
        >
          Lock / forget key
        </button>
      </div>
    )
  }

  return (
    <div className="space-y-3 rounded border border-steel bg-abyss/50 p-4">
      <div className="flex items-center gap-2 text-sm font-medium text-light">
        <KeyRound className="h-4 w-4" />
        Import operator signing key (encrypted in-browser, session only)
      </div>
      {error && (
        <div role="alert" className="text-xs text-red-300">
          {error}
        </div>
      )}
      <input
        aria-label="operator secret key hex"
        value={secret}
        onChange={(e) => setSecret(e.target.value)}
        placeholder="Ed25519 secret scalar (64 hex chars)"
        className="w-full rounded border border-steel bg-abyss px-3 py-2 font-mono text-sm text-light"
      />
      <input
        aria-label="operator key passphrase"
        type="password"
        value={pass}
        onChange={(e) => setPass(e.target.value)}
        placeholder="Passphrase (required — encrypts the key in memory)"
        className="w-full rounded border border-steel bg-abyss px-3 py-2 text-sm text-light"
      />
      <button
        type="button"
        onClick={onImport}
        disabled={busy}
        className="rounded bg-[--color-slate] px-4 py-2 text-sm font-medium text-light disabled:opacity-50"
      >
        {busy ? 'Encrypting…' : 'Import key'}
      </button>
      <p className="text-xs text-ghost">
        The key is encrypted (AES-256-GCM + PBKDF2) and held in memory for this tab only. It is
        never sent to a server. Mainnet hardening (SRI / self-hosting the operator page) is tracked
        as a follow-up.
      </p>
    </div>
  )
}

/**
 * Audit tab (#751, §6): renders each node's persisted audit log EXCLUSIVELY
 * from the node's stored entries (#750; anti-#541).
 */
function AuditTab({ token }: { token: string | null }) {
  const { logs } = useOperatorAuditLog(FLEET, token)
  return (
    <div className="space-y-6">
      {FLEET.map((node) => {
        const result = logs[node.id]
        const entries = result?.status === 'ok' ? result.data : []
        const unavailable = !token || (result !== undefined && result.status !== 'ok')
        return (
          <div key={node.id} className="space-y-2">
            <h3 className="text-sm font-medium text-light">{node.name}</h3>
            <AuditLogView entries={entries} unavailable={unavailable} />
          </div>
        )
      })}
    </div>
  )
}
