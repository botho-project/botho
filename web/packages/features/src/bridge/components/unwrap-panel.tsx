import { useEffect, useMemo, useState } from 'react'
import { Button, Card, CardContent, Input } from '@botho/ui'
import { formatBTH, parseBTH } from '@botho/core'
import {
  AlertCircle,
  ArrowUpRight,
  CheckCircle2,
  Circle,
  Copy,
  Check,
  Download,
  ExternalLink,
  Flame,
  Loader2,
  Lock,
  Repeat,
  TriangleAlert,
  Wallet,
} from 'lucide-react'
import type {
  ReleaseOrder,
  SourceChain,
  Translate,
  UnwrapController,
} from '../types'
import { getBurnTarget } from '../venues'
import {
  RELEASE_PROGRESSION,
  isTerminalReleaseStatus,
  releaseProgressionIndex,
  sourceTxUrl,
} from '../release-status'

/** Step icons for the "how unwrap works" flow, in render order. */
const STEP_KEYS = ['destination', 'burn', 'track', 'receive'] as const
const STEP_ICON = {
  destination: Wallet,
  burn: Flame,
  track: Repeat,
  receive: Download,
} as const

/** Unwrap sources offered by the chain picker (Hyperliquid = coming-soon). */
const UNWRAP_CHAINS: readonly SourceChain[] = ['ethereum', 'solana'] as const

export interface UnwrapExplainerProps {
  /** `bridge`-namespace translator supplied by the page. */
  t: Translate
  /**
   * Unwrap wiring (#1032): wallet + release-order client injected by the page.
   * When omitted the panel degrades to a "not configured" tracking state, but
   * the destination + burn guidance still render (they need no backend).
   */
  controller?: UnwrapController
  className?: string
}

/**
 * Guided wBTH→BTH unwrap explainer (#1032) + the integrated {@link UnwrapPanel}.
 *
 * The return leg WITHOUT EVM/Solana signing in the Botho wallet: the wallet
 * provides the Botho release destination + guides the user to burn wBTH in
 * THEIR OWN counterparty wallet, then tracks the release order and confirms the
 * received BTH. NO counterparty-chain code lives here.
 */
export function UnwrapExplainer({ t, controller, className }: UnwrapExplainerProps) {
  return (
    <section className={className}>
      <h2 className="font-display text-xl font-semibold text-[--color-light]">
        {t('unwrap.heading')}
      </h2>
      <p className="mt-1 max-w-2xl text-sm text-[--color-dim]">{t('unwrap.intro')}</p>

      {/* How it works — numbered flow. */}
      <ol className="mt-5 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        {STEP_KEYS.map((key, i) => {
          const Icon = STEP_ICON[key]
          return (
            <li key={key}>
              <Card className="h-full">
                <CardContent className="p-4">
                  <div className="flex items-center gap-2">
                    <span className="flex h-6 w-6 items-center justify-center rounded-full bg-[--color-pulse]/10 text-xs font-semibold text-[--color-pulse]">
                      {i + 1}
                    </span>
                    <Icon className="h-4 w-4 text-[--color-pulse]" />
                  </div>
                  <div className="mt-2.5 text-sm font-semibold text-[--color-light]">
                    {t(`unwrap.steps.${key}.title`)}
                  </div>
                  <p className="mt-1 text-xs text-[--color-dim]">
                    {t(`unwrap.steps.${key}.body`)}
                  </p>
                </CardContent>
              </Card>
            </li>
          )
        })}
      </ol>

      <UnwrapPanel t={t} controller={controller} className="mt-6" />
    </section>
  )
}

export interface UnwrapPanelProps {
  /** `bridge`-namespace translator supplied by the page. */
  t: Translate
  /** Unwrap wiring (#1032); `undefined` still renders destination + guidance. */
  controller?: UnwrapController
  className?: string
}

/** Panel chrome shared by every state — a titled card with a testnet badge. */
function PanelShell({
  t,
  network,
  children,
}: {
  t: Translate
  network?: string
  children: React.ReactNode
}) {
  return (
    <Card>
      <CardContent className="p-5">
        <div className="flex items-start justify-between gap-3">
          <div className="flex items-start gap-3">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-[--color-pulse]/10">
              <Flame className="h-5 w-5 text-[--color-pulse]" />
            </div>
            <div>
              <div className="font-display text-base font-semibold text-[--color-light]">
                {t('unwrap.panel.title')}
              </div>
              <p className="mt-1 max-w-xl text-sm text-[--color-dim]">
                {t('unwrap.panel.subtitle')}
              </p>
            </div>
          </div>
          {network === 'testnet' && (
            <span className="shrink-0 rounded-full bg-[--color-warning]/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-[--color-warning]">
              {t('unwrap.panel.testnetBadge')}
            </span>
          )}
        </div>
        <div className="mt-4">{children}</div>
      </CardContent>
    </Card>
  )
}

/**
 * Integrated wBTH→BTH unwrap (#1032).
 *
 * Provides the Botho release destination + chain-aware burn guidance, then (when
 * a release-order endpoint is configured) tracks the release state machine to
 * `released` and confirms the BTH arrived. The wallet NEVER signs on the
 * counterparty chain — the user burns wBTH in their own wallet.
 */
export function UnwrapPanel({ t, controller, className }: UnwrapPanelProps) {
  return (
    <div className={className}>
      <UnwrapPanelInner t={t} controller={controller} />
    </div>
  )
}

function UnwrapPanelInner({
  t,
  controller,
}: {
  t: Translate
  controller?: UnwrapController
}) {
  const [chain, setChain] = useState<SourceChain>('ethereum')
  const [amount, setAmount] = useState('')
  const [order, setOrder] = useState<ReleaseOrder | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const network = controller?.network
  const client = controller?.client ?? null
  const wallet = controller?.wallet

  // ── Gate states (no wallet / locked → no release address) ─────────────────
  if (!controller || !wallet?.hasWallet) {
    return (
      <PanelShell t={t} network={network}>
        <Notice
          icon={Wallet}
          tone="muted"
          title={t('unwrap.panel.noWallet.title')}
          body={t('unwrap.panel.noWallet.body')}
          action={
            controller?.requestWallet && (
              <Button variant="secondary" onClick={controller.requestWallet}>
                {t('unwrap.panel.noWallet.cta')}
              </Button>
            )
          }
        />
      </PanelShell>
    )
  }

  if (!wallet.releaseAddress) {
    return (
      <PanelShell t={t} network={network}>
        <Notice
          icon={Lock}
          tone="muted"
          title={t('unwrap.panel.locked.title')}
          body={t('unwrap.panel.locked.body')}
          action={
            controller.requestWallet && (
              <Button variant="secondary" onClick={controller.requestWallet}>
                {t('unwrap.panel.locked.cta')}
              </Button>
            )
          }
        />
      </PanelShell>
    )
  }

  // ── Tracking an open release order ────────────────────────────────────────
  if (order) {
    return (
      <PanelShell t={t} network={network}>
        <ReleaseTracker
          t={t}
          controller={controller}
          order={order}
          setOrder={setOrder}
          onReset={() => {
            setOrder(null)
            setError(null)
            setAmount('')
          }}
        />
      </PanelShell>
    )
  }

  // ── The form ──────────────────────────────────────────────────────────────
  const onTrack = async () => {
    if (!client) return
    setError(null)
    let amountPico: bigint
    try {
      amountPico = parseBTH(amount)
    } catch {
      setError(t('unwrap.panel.form.amountRequired'))
      return
    }
    if (amountPico <= 0n) {
      setError(t('unwrap.panel.form.amountRequired'))
      return
    }
    setSubmitting(true)
    try {
      const created = await client.createReleaseOrder({
        sourceChain: chain,
        bthAddress: wallet.releaseAddress!,
        amount: amountPico.toString(),
      })
      setOrder(created)
    } catch (e) {
      setError(errorMessage(e, t))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <PanelShell t={t} network={network}>
      <UnwrapForm
        t={t}
        chain={chain}
        setChain={setChain}
        amount={amount}
        setAmount={setAmount}
        releaseAddress={wallet.releaseAddress}
        hasClient={client != null}
        submitting={submitting}
        error={error}
        onTrack={onTrack}
      />
    </PanelShell>
  )
}

/** Source picker + release destination + amount + burn guidance. */
function UnwrapForm({
  t,
  chain,
  setChain,
  amount,
  setAmount,
  releaseAddress,
  hasClient,
  submitting,
  error,
  onTrack,
}: {
  t: Translate
  chain: SourceChain
  setChain: (c: SourceChain) => void
  amount: string
  setAmount: (a: string) => void
  releaseAddress: string
  hasClient: boolean
  submitting: boolean
  error: string | null
  onTrack: () => void
}) {
  const target = getBurnTarget(chain)

  const amountPico = useMemo(() => {
    if (!amount) return null
    try {
      const v = parseBTH(amount)
      return v > 0n ? v : null
    } catch {
      return null
    }
  }, [amount])

  const canTrack = hasClient && !submitting && amountPico != null

  const chainLabel = (c: SourceChain) => t(`unwrap.panel.chains.${c}`)

  return (
    <div className="space-y-4">
      {/* Source chain picker. */}
      <div>
        <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
          {t('unwrap.panel.form.chainLabel')}
        </label>
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
          {UNWRAP_CHAINS.map((c) => (
            <button
              key={c}
              type="button"
              onClick={() => setChain(c)}
              aria-pressed={chain === c}
              className={`rounded-lg border px-3 py-2 text-sm font-medium transition-colors ${
                chain === c
                  ? 'border-[--color-pulse] bg-[--color-pulse]/10 text-[--color-light]'
                  : 'border-[--color-steel] text-[--color-soft] hover:border-[--color-pulse]/50'
              }`}
            >
              {chainLabel(c)}
            </button>
          ))}
          {/* Hyperliquid — discovery-only until HIP-1 spot lands (#877). */}
          <button
            type="button"
            disabled
            title={t('unwrap.panel.form.comingSoon')}
            className="flex items-center justify-center gap-1 rounded-lg border border-dashed border-[--color-steel] px-3 py-2 text-sm text-[--color-dim]"
          >
            {t('unwrap.panel.chains.hyperliquid')}
            <span className="text-[10px] uppercase">{t('unwrap.panel.form.comingSoon')}</span>
          </button>
        </div>
      </div>

      {/* Release destination — the wallet's own Botho receive address. */}
      <div>
        <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
          {t('unwrap.panel.form.destinationLabel')}
        </label>
        <div className="flex gap-2">
          <input
            readOnly
            value={releaseAddress}
            onFocus={(e) => e.currentTarget.select()}
            className="min-w-0 flex-1 rounded-lg border border-[--color-steel] bg-[--color-abyss] px-3 py-2 font-mono text-xs text-[--color-light]"
          />
          <CopyButton t={t} value={releaseAddress} ariaLabel={t('unwrap.panel.form.copy')} />
        </div>
        <p className="mt-1 text-xs text-[--color-dim]">{t('unwrap.panel.form.destinationHint')}</p>
      </div>

      {/* Amount to burn. */}
      <div>
        <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
          {t('unwrap.panel.form.amountLabel')}
        </label>
        <div className="relative">
          <Input
            type="text"
            placeholder="0.00"
            value={amount}
            onChange={(e) => setAmount(e.target.value)}
            className="pr-16 font-mono"
          />
          <span className="absolute right-4 top-1/2 -translate-y-1/2 text-sm font-medium text-[--color-dim]">
            wBTH
          </span>
        </div>
        <p className="mt-1 text-xs text-[--color-dim]">{t('unwrap.panel.form.amountHint')}</p>
      </div>

      {/* Burn guidance — the user's own-wallet action. */}
      {target && (
        <div className="space-y-3 rounded-lg border border-[--color-steel] bg-[--color-abyss]/40 p-4">
          <div className="flex items-center gap-2">
            <Flame className="h-4 w-4 shrink-0 text-[--color-pulse]" />
            <div className="text-sm font-semibold text-[--color-light]">
              {t('unwrap.panel.form.burnHeading', { chain: chainLabel(chain) })}
            </div>
          </div>
          <p className="text-xs text-[--color-dim]">{t('unwrap.panel.form.burnIntro')}</p>

          <BurnRow label={t('unwrap.panel.form.tokenLabel')} value={target.tokenAddress} t={t} />
          <div>
            <div className="mb-1 text-xs text-[--color-dim]">{t('unwrap.panel.form.callLabel')}</div>
            <code className="block overflow-x-auto rounded-md border border-[--color-steel] bg-[--color-slate]/40 px-3 py-2 font-mono text-xs text-[--color-soft]">
              bridgeBurn({amountPico != null ? amountPico.toString() : t('unwrap.panel.form.amountToken')}, &quot;{shorten(releaseAddress)}&quot;)
            </code>
            <p className="mt-1 text-xs text-[--color-dim]">{t('unwrap.panel.form.callHint')}</p>
          </div>

          <a
            href={target.appUrl}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-1.5 text-sm text-[--color-pulse] transition-colors hover:text-[--color-light]"
          >
            {t('unwrap.panel.form.openApp', { chain: chainLabel(chain) })}
            <ExternalLink className="h-3.5 w-3.5" />
          </a>
        </div>
      )}

      {error && (
        <div className="rounded-lg border border-[--color-danger]/30 bg-[--color-danger]/10 p-3 text-sm text-[--color-danger]">
          {error}
        </div>
      )}

      {/* Track action (client-gated) OR the inert "tracking not wired" notice. */}
      {hasClient ? (
        <div className="space-y-1.5">
          <Button onClick={onTrack} disabled={!canTrack} className="w-full">
            {submitting ? (
              <>
                <Loader2 className="h-4 w-4 animate-spin" />
                {t('unwrap.panel.form.submitting')}
              </>
            ) : (
              <>
                <Repeat className="h-4 w-4" />
                {t('unwrap.panel.form.track')}
              </>
            )}
          </Button>
          <p className="text-center text-xs text-[--color-dim]">
            {t('unwrap.panel.form.trackHint')}
          </p>
        </div>
      ) : (
        <Notice
          icon={TriangleAlert}
          tone="warning"
          title={t('unwrap.panel.form.notConfigured.title')}
          body={t('unwrap.panel.form.notConfigured.body')}
        />
      )}
    </div>
  )
}

/** Live release tracker: polls status and renders the release state machine. */
function ReleaseTracker({
  t,
  controller,
  order,
  setOrder,
  onReset,
}: {
  t: Translate
  controller: UnwrapController
  order: ReleaseOrder
  setOrder: (o: ReleaseOrder) => void
  onReset: () => void
}) {
  const client = controller.client
  const orderId = order.id
  const status = order.status

  // Poll status until a terminal state. Mirrors the export tracker's
  // cancelled-flag cleanup so an unmount/reset can't set state late.
  useEffect(() => {
    if (!client || isTerminalReleaseStatus(status)) return
    let cancelled = false
    const poll = async () => {
      try {
        const next = await client.getReleaseOrderStatus(orderId)
        if (!cancelled) setOrder(next)
      } catch {
        // Transient fetch failure — keep the last known state and retry.
      }
    }
    const id = setInterval(poll, 8_000)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [client, orderId, status, setOrder])

  const amount = BigInt(order.amount)
  const fee = BigInt(order.fee)
  const net = amount > fee ? amount - fee : 0n
  const currentIndex = releaseProgressionIndex(status)
  const isReleased = status === 'released'
  const isFailed = status === 'failed'
  const isExpired = status === 'expired'

  return (
    <div className="space-y-4">
      {/* Order summary. */}
      <div className="rounded-lg border border-[--color-steel] bg-[--color-slate]/40 p-3 text-sm">
        <Row
          label={t('unwrap.panel.order.id')}
          value={<span className="font-mono text-xs">{order.id}</span>}
        />
        <Row
          label={t('unwrap.panel.order.source')}
          value={t(`unwrap.panel.chains.${order.sourceChain}`)}
        />
        <Row
          label={t('unwrap.panel.order.destination')}
          value={<span className="font-mono text-xs">{shorten(order.bthAddress)}</span>}
        />
        <Row label={t('unwrap.panel.order.amount')} value={`${formatBTH(amount)} wBTH`} />
        <Row label={t('unwrap.panel.order.fee')} value={`${formatBTH(fee)} BTH`} />
        <Row
          label={t('unwrap.panel.order.receive')}
          value={<span className="font-semibold text-[--color-pulse]">{formatBTH(net)} BTH</span>}
        />
      </div>

      {/* Terminal off-ramps. */}
      {isExpired && (
        <Notice
          icon={TriangleAlert}
          tone="warning"
          title={t('unwrap.panel.status.expired')}
          body={t('unwrap.panel.status.expiredNote')}
        />
      )}
      {isFailed && (
        <Notice
          icon={AlertCircle}
          tone="danger"
          title={t('unwrap.panel.status.failed')}
          body={t('unwrap.panel.status.failedNote', { reason: order.failureReason ?? '' })}
        />
      )}

      {/* State-machine stepper (happy path). */}
      {!isExpired && !isFailed && (
        <ol className="space-y-2">
          {RELEASE_PROGRESSION.map((step, i) => {
            const done = currentIndex >= 0 && i < currentIndex
            const active = currentIndex >= 0 && i === currentIndex
            return (
              <li key={step} className="flex items-center gap-2 text-sm">
                {done || (isReleased && i === currentIndex) ? (
                  <CheckCircle2 className="h-4 w-4 shrink-0 text-emerald-400" />
                ) : active ? (
                  <Loader2 className="h-4 w-4 shrink-0 animate-spin text-[--color-pulse]" />
                ) : (
                  <Circle className="h-4 w-4 shrink-0 text-[--color-steel]" />
                )}
                <span className={done || active ? 'text-[--color-light]' : 'text-[--color-dim]'}>
                  {t(`unwrap.panel.status.${step}`)}
                </span>
              </li>
            )
          })}
        </ol>
      )}

      {/* Burn tx link (once detected). */}
      {order.sourceTx && sourceTxUrl(order.sourceChain, order.sourceTx) && (
        <a
          href={sourceTxUrl(order.sourceChain, order.sourceTx)!}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1 text-sm text-[--color-ghost] transition-colors hover:text-[--color-light]"
        >
          {t('unwrap.panel.order.viewBurnTx')}
          <ArrowUpRight className="h-3.5 w-3.5" />
        </a>
      )}

      {/* Released: confirm the BTH arrived — the wallet scans owned outputs. */}
      {isReleased && (
        <div className="space-y-2 rounded-lg border border-emerald-400/30 bg-emerald-400/5 p-3">
          <div className="flex items-start gap-2">
            <CheckCircle2 className="mt-0.5 h-5 w-5 shrink-0 text-emerald-400" />
            <div>
              <div className="text-sm font-semibold text-[--color-light]">
                {t('unwrap.panel.released.title')}
              </div>
              <p className="mt-0.5 text-xs text-[--color-dim]">
                {t('unwrap.panel.released.body')}
              </p>
            </div>
          </div>
          {order.destTx && (
            <div className="pl-7 font-mono text-xs text-[--color-dim]">
              {t('unwrap.panel.released.releaseTx', { tx: shorten(order.destTx) })}
            </div>
          )}
        </div>
      )}

      <button
        type="button"
        onClick={onReset}
        className="text-xs text-[--color-ghost] transition-colors hover:text-[--color-light]"
      >
        {t('unwrap.panel.order.newUnwrap')}
      </button>
    </div>
  )
}

// ── small presentational helpers ────────────────────────────────────────────

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-2 py-0.5">
      <span className="text-[--color-dim]">{label}</span>
      <span className="text-[--color-soft]">{value}</span>
    </div>
  )
}

/** A copyable long value (token address / burn address). */
function BurnRow({ label, value, t }: { label: string; value: string; t: Translate }) {
  return (
    <div>
      <div className="mb-1 text-xs text-[--color-dim]">{label}</div>
      <div className="flex gap-2">
        <input
          readOnly
          value={value}
          onFocus={(e) => e.currentTarget.select()}
          className="min-w-0 flex-1 rounded-md border border-[--color-steel] bg-[--color-slate]/40 px-3 py-2 font-mono text-xs text-[--color-soft]"
        />
        <CopyButton t={t} value={value} ariaLabel={t('unwrap.panel.form.copy')} />
      </div>
    </div>
  )
}

/** Clipboard-copy button with a transient "copied" checkmark. */
function CopyButton({ t, value, ariaLabel }: { t: Translate; value: string; ariaLabel: string }) {
  const [copied, setCopied] = useState(false)
  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(value)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard may be unavailable; the value is still selectable in the field.
    }
  }
  return (
    <Button variant="secondary" size="sm" onClick={onCopy} aria-label={ariaLabel}>
      {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
      <span className="ml-1 hidden sm:inline">
        {copied ? t('unwrap.panel.form.copied') : t('unwrap.panel.form.copy')}
      </span>
    </Button>
  )
}

function Notice({
  icon: Icon,
  tone,
  title,
  body,
  action,
}: {
  icon: typeof TriangleAlert
  tone: 'warning' | 'danger' | 'muted'
  title: string
  body: string
  action?: React.ReactNode
}) {
  const toneClass =
    tone === 'warning'
      ? 'border-[--color-warning]/30 bg-[--color-warning]/5 text-[--color-warning]'
      : tone === 'danger'
        ? 'border-[--color-danger]/30 bg-[--color-danger]/10 text-[--color-danger]'
        : 'border-[--color-steel] bg-[--color-abyss]/40 text-[--color-soft]'
  return (
    <div className={`flex flex-col gap-3 rounded-xl border px-4 py-3 ${toneClass}`}>
      <div className="flex items-start gap-2">
        <Icon className="mt-0.5 h-4 w-4 shrink-0" />
        <div>
          <div className="text-sm font-medium">{title}</div>
          <p className="mt-1 text-xs text-[--color-dim]">{body}</p>
        </div>
      </div>
      {action}
    </div>
  )
}

/** `abcdef…wxyz` — compact display of a long address / tx hash. */
function shorten(addr: string): string {
  if (addr.length <= 16) return addr
  return `${addr.slice(0, 8)}…${addr.slice(-6)}`
}

/** Extract a human message from a thrown value, falling back to generic copy. */
function errorMessage(e: unknown, t: Translate): string {
  if (e instanceof Error && e.message) return e.message
  return t('unwrap.panel.error.generic')
}
