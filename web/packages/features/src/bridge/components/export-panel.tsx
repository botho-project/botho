import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Button, Card, CardContent, Input } from '@botho/ui'
import { formatBTH, parseBTH } from '@botho/core'
import {
  AlertCircle,
  ArrowRight,
  ArrowUpRight,
  CheckCircle2,
  Circle,
  Coins,
  Loader2,
  Lock,
  Repeat,
  ShieldCheck,
  Sparkles,
  TriangleAlert,
  Wallet,
} from 'lucide-react'
import type {
  DestinationChain,
  ExportController,
  MintOrder,
  Translate,
} from '../types'
import { isValidDestinationAddress } from '../address'
import {
  MINT_PROGRESSION,
  destTxUrl,
  isTerminalStatus,
  progressionIndex,
} from '../order-status'

/** Step icons for the "how to bridge" flow, in render order. */
const STEP_KEYS = ['factor1', 'lock', 'mint', 'trade'] as const
const STEP_ICON = {
  factor1: Coins,
  lock: Lock,
  mint: Sparkles,
  trade: ArrowRight,
} as const

/** Peg-guarantee callouts. */
const GUARANTEE_KEYS = ['peg', 'exactlyOnce', 'factorOne'] as const

/** Export destinations offered by the chain picker (Hyperliquid = coming-soon). */
const EXPORT_CHAINS: readonly DestinationChain[] = ['ethereum', 'solana'] as const

export interface ExportExplainerProps {
  /** `bridge`-namespace translator supplied by the page. */
  t: Translate
  /**
   * Tier 1 wiring (#1031): wallet + bridge-client injected by the page. When
   * omitted the panel degrades to a "not configured" state, so the discovery
   * page still renders without a wallet.
   */
  controller?: ExportController
  /** Route to the venue directory with `chain` pre-selected (wired by the page). */
  onTradeNow?: (chain: DestinationChain) => void
  className?: string
}

/**
 * Guided BTH→wBTH export explainer (#1030, Tier 0) + the integrated export
 * panel (#1031, Tier 1).
 *
 * The explanatory "how it works" flow and peg guarantees are unchanged; the
 * inert Tier 0 scaffold at the bottom is now the real {@link ExportPanel},
 * wired via the injected `controller`. NO counterparty-chain code lives here —
 * the wallet builds/signs the BTH deposit only.
 */
export function ExportExplainer({ t, controller, onTradeNow, className }: ExportExplainerProps) {
  return (
    <section className={className}>
      <h2 className="font-display text-xl font-semibold text-[--color-light]">
        {t('export.heading')}
      </h2>
      <p className="mt-1 max-w-2xl text-sm text-[--color-dim]">{t('export.intro')}</p>

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
                    {t(`export.steps.${key}.title`)}
                  </div>
                  <p className="mt-1 text-xs text-[--color-dim]">
                    {t(`export.steps.${key}.body`)}
                  </p>
                </CardContent>
              </Card>
            </li>
          )
        })}
      </ol>

      {/* Peg guarantees. */}
      <div className="mt-4 grid gap-4 sm:grid-cols-3">
        {GUARANTEE_KEYS.map((key) => (
          <div
            key={key}
            className="rounded-xl border border-[--color-steel] bg-[--color-abyss]/40 p-4"
          >
            <div className="flex items-center gap-1.5 text-sm font-medium text-[--color-soft]">
              <ShieldCheck className="h-4 w-4 text-emerald-400" />
              {t(`export.guarantees.${key}.title`)}
            </div>
            <p className="mt-1 text-xs text-[--color-dim]">
              {t(`export.guarantees.${key}.body`)}
            </p>
          </div>
        ))}
      </div>

      <ExportPanel t={t} controller={controller} onTradeNow={onTradeNow} className="mt-6" />
    </section>
  )
}

export interface ExportPanelProps {
  /** `bridge`-namespace translator supplied by the page. */
  t: Translate
  /** Tier 1 wiring (#1031); `undefined` renders the "not configured" state. */
  controller?: ExportController
  /** Route to the venue directory with `chain` pre-selected. */
  onTradeNow?: (chain: DestinationChain) => void
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
              <Repeat className="h-5 w-5 text-[--color-pulse]" />
            </div>
            <div>
              <div className="font-display text-base font-semibold text-[--color-light]">
                {t('export.panel.title')}
              </div>
              <p className="mt-1 max-w-xl text-sm text-[--color-dim]">
                {t('export.panel.subtitle')}
              </p>
            </div>
          </div>
          {network === 'testnet' && (
            <span className="shrink-0 rounded-full bg-[--color-warning]/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-[--color-warning]">
              {t('export.panel.testnetBadge')}
            </span>
          )}
        </div>
        <div className="mt-4">{children}</div>
      </CardContent>
    </Card>
  )
}

/**
 * Tier 1 integrated export (#1031).
 *
 * Real, chain-aware BTH→wBTH export: destination picker + address validation,
 * amount + factor-1/fee/net surfacing, order creation, and BTH deposit
 * construction via the wallet's own wasm-signer send path — then live order
 * tracking through the mint state machine to a "Trade wBTH now" hand-off. The
 * wallet NEVER signs on the counterparty chain; wBTH lands in the user's own
 * EVM/SVM wallet.
 */
export function ExportPanel({ t, controller, onTradeNow, className }: ExportPanelProps) {
  return (
    <div className={className}>
      <ExportPanelInner t={t} controller={controller} onTradeNow={onTradeNow} />
    </div>
  )
}

function ExportPanelInner({
  t,
  controller,
  onTradeNow,
}: {
  t: Translate
  controller?: ExportController
  onTradeNow?: (chain: DestinationChain) => void
}) {
  const [chain, setChain] = useState<DestinationChain>('ethereum')
  const [address, setAddress] = useState('')
  const [amount, setAmount] = useState('')
  const [order, setOrder] = useState<MintOrder | null>(null)
  const [depositTx, setDepositTx] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const network = controller?.network
  const client = controller?.client ?? null
  const wallet = controller?.wallet

  // ── Gate states (no endpoint / no wallet / locked) ────────────────────────
  if (!controller || !client) {
    return (
      <PanelShell t={t} network={network}>
        <Notice
          icon={TriangleAlert}
          tone="warning"
          title={t('export.panel.notConfigured.title')}
          body={t('export.panel.notConfigured.body')}
        />
      </PanelShell>
    )
  }

  if (!wallet?.hasWallet) {
    return (
      <PanelShell t={t} network={network}>
        <Notice
          icon={Wallet}
          tone="muted"
          title={t('export.panel.noWallet.title')}
          body={t('export.panel.noWallet.body')}
          action={
            controller.requestWallet && (
              <Button variant="secondary" onClick={controller.requestWallet}>
                {t('export.panel.noWallet.cta')}
              </Button>
            )
          }
        />
      </PanelShell>
    )
  }

  if (wallet.isLocked) {
    return (
      <PanelShell t={t} network={network}>
        <Notice
          icon={Lock}
          tone="muted"
          title={t('export.panel.locked.title')}
          body={t('export.panel.locked.body')}
          action={
            controller.requestWallet && (
              <Button variant="secondary" onClick={controller.requestWallet}>
                {t('export.panel.locked.cta')}
              </Button>
            )
          }
        />
      </PanelShell>
    )
  }

  // ── Tracking an open order ────────────────────────────────────────────────
  if (order) {
    return (
      <PanelShell t={t} network={network}>
        <OrderTracker
          t={t}
          controller={controller}
          order={order}
          setOrder={setOrder}
          depositTx={depositTx}
          setDepositTx={setDepositTx}
          error={error}
          setError={setError}
          onTradeNow={onTradeNow}
          onReset={() => {
            setOrder(null)
            setDepositTx(null)
            setError(null)
            setAmount('')
          }}
        />
      </PanelShell>
    )
  }

  // ── The form ──────────────────────────────────────────────────────────────
  const onSubmit = async () => {
    setError(null)
    let amountPico: bigint
    try {
      amountPico = parseBTH(amount)
    } catch {
      setError(t('export.panel.form.amountRequired'))
      return
    }
    if (amountPico <= 0n) {
      setError(t('export.panel.form.amountRequired'))
      return
    }
    if (wallet.spendableBalance != null && amountPico > wallet.spendableBalance) {
      setError(t('export.panel.form.insufficient'))
      return
    }
    setSubmitting(true)
    let created: MintOrder
    try {
      created = await client.createMintOrder({
        destChain: chain,
        destAddress: address.trim(),
        amount: amountPico.toString(),
      })
      setOrder(created)
    } catch (e) {
      setError(errorMessage(e, t))
      setSubmitting(false)
      return
    }
    // Order is open — build+sign+submit the BTH deposit via the wallet's
    // wasm-signer send path. A failure here keeps the (now-tracked) order so
    // the user can retry the deposit from the tracker.
    try {
      const tx = await controller.submitDeposit({
        depositAddress: created.depositAddress,
        amount: BigInt(created.amount),
        memo: created.memo,
      })
      setDepositTx(tx)
    } catch (e) {
      setError(t('export.panel.order.depositFailed', { error: errorMessage(e, t) }))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <PanelShell t={t} network={network}>
      <ExportForm
        t={t}
        chain={chain}
        setChain={setChain}
        address={address}
        setAddress={setAddress}
        amount={amount}
        setAmount={setAmount}
        spendableBalance={wallet.spendableBalance}
        submitting={submitting}
        error={error}
        onSubmit={onSubmit}
      />
    </PanelShell>
  )
}

/** The destination/address/amount form. */
function ExportForm({
  t,
  chain,
  setChain,
  address,
  setAddress,
  amount,
  setAmount,
  spendableBalance,
  submitting,
  error,
  onSubmit,
}: {
  t: Translate
  chain: DestinationChain
  setChain: (c: DestinationChain) => void
  address: string
  setAddress: (a: string) => void
  amount: string
  setAmount: (a: string) => void
  spendableBalance: bigint | null
  submitting: boolean
  error: string | null
  onSubmit: () => void
}) {
  const trimmedAddress = address.trim()
  const addressValid = isValidDestinationAddress(chain, trimmedAddress)
  const addressInvalid = trimmedAddress.length > 0 && !addressValid

  const amountPico = useMemo(() => {
    if (!amount) return null
    try {
      return parseBTH(amount)
    } catch {
      return null
    }
  }, [amount])

  const insufficient =
    amountPico != null && spendableBalance != null && amountPico > spendableBalance

  const canSubmit =
    !submitting && addressValid && amountPico != null && amountPico > 0n && !insufficient

  const chainLabel = (c: DestinationChain) => t(`export.panel.chains.${c}`)

  return (
    <div className="space-y-4">
      {/* Destination chain picker. */}
      <div>
        <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
          {t('export.panel.form.chainLabel')}
        </label>
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
          {EXPORT_CHAINS.map((c) => (
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
            title={t('export.panel.form.comingSoon')}
            className="flex items-center justify-center gap-1 rounded-lg border border-dashed border-[--color-steel] px-3 py-2 text-sm text-[--color-dim]"
          >
            {t('export.panel.chains.hyperliquid')}
            <span className="text-[10px] uppercase">{t('export.panel.form.comingSoon')}</span>
          </button>
        </div>
        <p className="mt-1 text-xs text-[--color-dim]">{t('export.panel.form.chainHint')}</p>
      </div>

      {/* Destination address. */}
      <div>
        <label className="mb-1.5 block text-sm font-medium text-[--color-ghost]">
          {t('export.panel.form.addressLabel', { chain: chainLabel(chain) })}
        </label>
        <Input
          placeholder={t(`export.panel.form.addressPlaceholder.${chain}`)}
          value={address}
          onChange={(e) => setAddress(e.target.value)}
          className="font-mono text-sm"
        />
        {addressInvalid && (
          <p className="mt-1 flex items-center gap-1 text-xs text-[--color-danger]">
            <AlertCircle className="h-3 w-3 shrink-0" />
            {t('export.panel.form.addressInvalid', { chain: chainLabel(chain) })}
          </p>
        )}
      </div>

      {/* Amount. */}
      <div>
        <div className="mb-1.5 flex items-center justify-between">
          <label className="text-sm font-medium text-[--color-ghost]">
            {t('export.panel.form.amountLabel')}
          </label>
          {spendableBalance != null ? (
            <button
              type="button"
              onClick={() => setAmount(formatBTH(spendableBalance, { separators: false }))}
              className="text-xs text-[--color-pulse] hover:underline"
            >
              {t('export.panel.form.max', { amount: formatBTH(spendableBalance) })}
            </button>
          ) : (
            <span className="text-xs text-[--color-dim]">
              {t('export.panel.form.balanceUnknown')}
            </span>
          )}
        </div>
        <div className="relative">
          <Input
            type="text"
            placeholder="0.00"
            value={amount}
            onChange={(e) => setAmount(e.target.value)}
            className="pr-16 font-mono"
          />
          <span className="absolute right-4 top-1/2 -translate-y-1/2 text-sm font-medium text-[--color-dim]">
            BTH
          </span>
        </div>
        {insufficient && (
          <p className="mt-1 flex items-center gap-1 text-xs text-[--color-danger]">
            <AlertCircle className="h-3 w-3 shrink-0" />
            {t('export.panel.form.insufficient')}
          </p>
        )}
      </div>

      {/* Factor-1 requirement (ADR 0003). */}
      <div className="flex items-start gap-2 rounded-lg border border-[--color-steel] bg-[--color-abyss]/40 px-3 py-2">
        <Coins className="mt-0.5 h-4 w-4 shrink-0 text-[--color-pulse]" />
        <p className="text-xs text-[--color-dim]">{t('export.panel.form.factor1Notice')}</p>
      </div>

      {/* Fee / net preview. */}
      <p className="text-xs text-[--color-dim]">{t('export.panel.form.feePreview')}</p>

      {error && (
        <div className="rounded-lg border border-[--color-danger]/30 bg-[--color-danger]/10 p-3 text-sm text-[--color-danger]">
          {error}
        </div>
      )}

      <Button onClick={onSubmit} disabled={!canSubmit} className="w-full">
        {submitting ? (
          <>
            <Loader2 className="h-4 w-4 animate-spin" />
            {t('export.panel.form.submitting')}
          </>
        ) : (
          <>
            <Repeat className="h-4 w-4" />
            {t('export.panel.form.submit')}
          </>
        )}
      </Button>
    </div>
  )
}

/** Live order tracker: polls status and renders the mint state machine. */
function OrderTracker({
  t,
  controller,
  order,
  setOrder,
  depositTx,
  setDepositTx,
  error,
  setError,
  onTradeNow,
  onReset,
}: {
  t: Translate
  controller: ExportController
  order: MintOrder
  setOrder: (o: MintOrder) => void
  depositTx: string | null
  setDepositTx: (tx: string | null) => void
  error: string | null
  setError: (e: string | null) => void
  onTradeNow?: (chain: DestinationChain) => void
  onReset: () => void
}) {
  const client = controller.client
  const [retrying, setRetrying] = useState(false)
  const orderId = order.id
  const status = order.status
  // Keep the freshest order in a ref so the retry callback isn't a stale closure.
  const orderRef = useRef(order)
  orderRef.current = order

  // Poll status until a terminal state. Mirrors `useReserveProof`'s
  // cancelled-flag cleanup so an unmount/reset can't set state late.
  useEffect(() => {
    if (!client || isTerminalStatus(status)) return
    let cancelled = false
    const poll = async () => {
      try {
        const next = await client.getOrderStatus(orderId)
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

  const retryDeposit = useCallback(async () => {
    setError(null)
    setRetrying(true)
    try {
      const tx = await controller.submitDeposit({
        depositAddress: orderRef.current.depositAddress,
        amount: BigInt(orderRef.current.amount),
        memo: orderRef.current.memo,
      })
      setDepositTx(tx)
    } catch (e) {
      setError(t('export.panel.order.depositFailed', { error: errorMessage(e, t) }))
    } finally {
      setRetrying(false)
    }
  }, [controller, setDepositTx, setError, t])

  const amount = BigInt(order.amount)
  const fee = BigInt(order.fee)
  const net = amount > fee ? amount - fee : 0n
  const currentIndex = progressionIndex(status)
  const isCompleted = status === 'completed'
  const isFailed = status === 'failed'
  const isExpired = status === 'expired'

  return (
    <div className="space-y-4">
      {/* Order summary. */}
      <div className="rounded-lg border border-[--color-steel] bg-[--color-slate]/40 p-3 text-sm">
        <Row
          label={t('export.panel.order.id')}
          value={<span className="font-mono text-xs">{order.id}</span>}
        />
        <Row
          label={t('export.panel.order.destination')}
          value={`${t(`export.panel.chains.${order.destChain}`)} · ${shorten(order.destAddress)}`}
        />
        <Row label={t('export.panel.order.amount')} value={`${formatBTH(amount)} BTH`} />
        <Row label={t('export.panel.order.fee')} value={`${formatBTH(fee)} BTH`} />
        <Row
          label={t('export.panel.order.receive')}
          value={
            <span className="font-semibold text-[--color-pulse]">{formatBTH(net)} wBTH</span>
          }
        />
        <Row
          label={t('export.panel.order.deposit')}
          value={
            depositTx ? (
              <span className="text-emerald-400">
                {t('export.panel.order.depositSubmitted')}
              </span>
            ) : (
              <span className="text-[--color-dim]">—</span>
            )
          }
        />
      </div>

      {/* Deposit-failed banner + retry. */}
      {!depositTx && error && (
        <div className="space-y-2 rounded-lg border border-[--color-danger]/30 bg-[--color-danger]/10 p-3 text-sm text-[--color-danger]">
          <p>{error}</p>
          <Button variant="secondary" onClick={retryDeposit} disabled={retrying}>
            {retrying ? (
              <>
                <Loader2 className="h-4 w-4 animate-spin" />
                {t('export.panel.form.submitting')}
              </>
            ) : (
              t('export.panel.order.retryDeposit')
            )}
          </Button>
        </div>
      )}

      {/* Terminal off-ramps. */}
      {isExpired && (
        <Notice
          icon={TriangleAlert}
          tone="warning"
          title={t('export.panel.status.expired')}
          body={t('export.panel.status.expiredNote')}
        />
      )}
      {isFailed && (
        <Notice
          icon={AlertCircle}
          tone="danger"
          title={t('export.panel.status.failed')}
          body={t('export.panel.status.failedNote', { reason: order.failureReason ?? '' })}
        />
      )}

      {/* State-machine stepper (happy path). */}
      {!isExpired && !isFailed && (
        <ol className="space-y-2">
          {MINT_PROGRESSION.map((step, i) => {
            const done = currentIndex >= 0 && i < currentIndex
            const active = currentIndex >= 0 && i === currentIndex
            return (
              <li key={step} className="flex items-center gap-2 text-sm">
                {done || (isCompleted && i === currentIndex) ? (
                  <CheckCircle2 className="h-4 w-4 shrink-0 text-emerald-400" />
                ) : active ? (
                  <Loader2 className="h-4 w-4 shrink-0 animate-spin text-[--color-pulse]" />
                ) : (
                  <Circle className="h-4 w-4 shrink-0 text-[--color-steel]" />
                )}
                <span className={done || active ? 'text-[--color-light]' : 'text-[--color-dim]'}>
                  {t(`export.panel.status.${step}`)}
                </span>
              </li>
            )
          })}
        </ol>
      )}

      {/* Completed: dest tx link + Trade hand-off. */}
      {isCompleted && (
        <div className="space-y-3 rounded-lg border border-emerald-400/30 bg-emerald-400/5 p-3">
          <div className="flex items-start gap-2">
            <CheckCircle2 className="mt-0.5 h-5 w-5 shrink-0 text-emerald-400" />
            <div>
              <div className="text-sm font-semibold text-[--color-light]">
                {t('export.panel.completed.title')}
              </div>
              <p className="mt-0.5 text-xs text-[--color-dim]">
                {t('export.panel.completed.body', {
                  chain: t(`export.panel.chains.${order.destChain}`),
                })}
              </p>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-3">
            {order.destTx && destTxUrl(order.destChain, order.destTx) && (
              <a
                href={destTxUrl(order.destChain, order.destTx)!}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1 text-sm text-[--color-ghost] transition-colors hover:text-[--color-light]"
              >
                {t('export.panel.completed.viewTx')}
                <ArrowUpRight className="h-3.5 w-3.5" />
              </a>
            )}
            {onTradeNow && (
              <Button onClick={() => onTradeNow(order.destChain)}>
                {t('export.panel.completed.tradeNow')}
                <ArrowRight className="h-4 w-4" />
              </Button>
            )}
          </div>
        </div>
      )}

      <button
        type="button"
        onClick={onReset}
        className="text-xs text-[--color-ghost] transition-colors hover:text-[--color-light]"
      >
        {t('export.panel.order.newExport')}
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

/** `abcdef…wxyz` — compact display of a long counterparty-chain address. */
function shorten(addr: string): string {
  if (addr.length <= 12) return addr
  return `${addr.slice(0, 6)}…${addr.slice(-4)}`
}

/** Extract a human message from a thrown value, falling back to generic copy. */
function errorMessage(e: unknown, t: Translate): string {
  if (e instanceof Error && e.message) return e.message
  return t('export.panel.error.generic')
}
