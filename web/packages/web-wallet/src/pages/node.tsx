import { useState, useEffect, useCallback } from 'react'
import { Link } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Button, Card, Input, Logo } from '@botho/ui'
import {
  Server,
  ArrowLeft,
  Loader2,
  AlertCircle,
  Check,
  Cpu,
  Globe,
  ShieldCheck,
  Copy,
  ExternalLink,
  CreditCard,
} from 'lucide-react'
import {
  DEFAULT_NODE_REGION,
  NODE_REGIONS,
  NodeCheckoutError,
  isRegionAvailable,
  startNodeCheckout,
} from '../lib/node-checkout'
import {
  createPortalUrl,
  fetchNodeStatus,
  fetchSessionStatus,
  sessionIdFromSearch,
  tokenFromSearch,
  NodeStatusError,
  type NodeStatus,
} from '../lib/node-status'
import { LocaleSwitcher } from '../components/LocaleSwitcher'

/**
 * P7.1 — "Host a node" surface (#458 §2, §4; issue #504).
 *
 * A thin signup page that lets a user subscribe to a managed Botho node —
 * **framed as a node-HOSTING service**, not a mining-income product (decision
 * #719; also the regulatory-safe framing — the 2025 SEC staff guidance treats
 * mining as administrative/ministerial, and selling *compute hosting* with
 * *non-custodial* rewards keeps this outside the investment-contract framing).
 * It collects the desired AWS region (allowlist, #458 §5), then asks the
 * control-plane Worker (`@botho/baas-worker /checkout`) to create a Stripe
 * Checkout Session and redirects the browser to the Stripe-hosted page.
 *
 * Honest value prop (#458 §7): we run an always-on Botho node for you so you —
 * and everyone you invite — can transact from the app without anyone managing a
 * server. Its mining participation helps secure the network; it is NOT an income
 * promise (mining self-equilibrates to ~break-even; the managed hosting is the
 * product). Billing runs in Stripe TEST mode while on testnet.
 *
 * Webhook → provisioning is P7.2 (#506); the rich status page is P6.3. After
 * checkout, Stripe redirects to `/node/success` (the placeholder below).
 */
export function NodePage() {
  const { t } = useTranslation('node')
  const [region, setRegion] = useState<string>(DEFAULT_NODE_REGION)
  const [email, setEmail] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleGetNode() {
    setError(null)
    setSubmitting(true)
    try {
      // Coming-soon regions checkout in the default region; the actual
      // preference rides along as demand data (Stripe metadata).
      const available = isRegionAvailable(region)
      const session = await startNodeCheckout({
        region: available ? region : DEFAULT_NODE_REGION,
        preferredRegion: available ? undefined : region,
        email: email.trim() || undefined,
      })
      // Redirect the browser to the Stripe-hosted checkout page.
      window.location.assign(session.url)
    } catch (err) {
      const message =
        err instanceof NodeCheckoutError ? err.message : t('checkout.errorFallback')
      setError(message)
      setSubmitting(false)
    }
  }

  const defaultRegionLabelKey = NODE_REGIONS.find(
    (r) => r.id === DEFAULT_NODE_REGION,
  )?.labelKey

  return (
    <div className="min-h-screen">
      {/* Header */}
      <header className="fixed top-0 left-0 right-0 z-50 backdrop-blur-md bg-void/80 border-b border-steel">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <Logo size="md" showText={false} />
            <span className="font-display text-lg sm:text-xl font-semibold">Botho</span>
          </Link>
          <div className="flex items-center gap-4">
            <LocaleSwitcher className="whitespace-nowrap" />
            <Link to="/" className="text-ghost hover:text-light transition-colors flex items-center gap-2">
              <ArrowLeft size={18} />
              {t('checkout.back')}
            </Link>
          </div>
        </div>
      </header>

      <main className="pt-28 sm:pt-32 pb-16 px-4 sm:px-6">
        <div className="max-w-3xl mx-auto">
          {/* Hero */}
          <div className="text-center mb-10">
            <div className="inline-flex items-center gap-2 px-3 py-1.5 rounded-full bg-steel/50 border border-muted text-xs sm:text-sm text-ghost mb-6">
              <span className="w-2 h-2 rounded-full bg-warning" />
              {t('checkout.testnetBadge')}
            </div>
            <div className="w-14 h-14 rounded-xl bg-pulse/10 flex items-center justify-center mx-auto mb-5">
              <Server className="text-pulse" size={28} />
            </div>
            <h1 className="font-display text-3xl sm:text-4xl md:text-5xl font-bold mb-4">
              {t('checkout.heroTitle')}
            </h1>
            <p className="text-base sm:text-lg text-ghost max-w-xl mx-auto">
              {t('checkout.heroSubtitlePrefix')}
              <span className="text-light whitespace-nowrap">{t('checkout.heroPrice')}</span>
            </p>
          </div>

          {/* Honest value-prop framing (#458 §7; hosting framing, decision #719) */}
          <div className="mb-8 p-4 rounded-xl bg-warning/10 border border-warning/30 flex gap-3">
            <AlertCircle className="text-warning shrink-0 mt-0.5" size={20} />
            <p className="text-sm text-ghost">
              <span className="text-light font-medium">{t('checkout.valueProp.leadStrong')}</span>
              {t('checkout.valueProp.leadBody')}
              <span className="text-light">{t('checkout.valueProp.clarityStrong')}</span>
              {t('checkout.valueProp.clarityBody')}
            </p>
          </div>

          {/* What you get */}
          <div className="grid sm:grid-cols-3 gap-3 sm:gap-4 mb-8">
            {[
              { icon: Cpu, titleKey: 'checkout.cards.managedTitle', descKey: 'checkout.cards.managedDesc' },
              { icon: Globe, titleKey: 'checkout.cards.hubTitle', descKey: 'checkout.cards.hubDesc' },
              { icon: ShieldCheck, titleKey: 'checkout.cards.custodyTitle', descKey: 'checkout.cards.custodyDesc' },
            ].map((f) => (
              <div key={f.titleKey} className="p-4 rounded-xl bg-slate/50 border border-steel">
                <f.icon className="text-pulse mb-2" size={20} />
                <div className="font-display font-semibold text-sm mb-1">{t(f.titleKey)}</div>
                <div className="text-xs text-ghost">{t(f.descKey)}</div>
              </div>
            ))}
          </div>

          {/* Checkout form */}
          <Card className="p-5 sm:p-6">
            <label className="block text-sm font-medium text-light mb-2" htmlFor="node-region">
              {t('checkout.regionLabel')}
            </label>
            <select
              id="node-region"
              value={region}
              onChange={(e) => setRegion(e.target.value)}
              disabled={submitting}
              className="w-full mb-1 px-3 py-2.5 rounded-lg bg-void border border-steel text-light focus:outline-none focus:border-pulse disabled:opacity-50"
            >
              <optgroup label={t('checkout.optgroupAvailable')}>
                {NODE_REGIONS.filter((r) => r.available).map((r) => (
                  <option key={r.id} value={r.id}>
                    {t(r.labelKey)}
                  </option>
                ))}
              </optgroup>
              <optgroup label={t('checkout.optgroupComingSoon')}>
                {NODE_REGIONS.filter((r) => !r.available).map((r) => (
                  <option key={r.id} value={r.id}>
                    {t(r.labelKey)}
                  </option>
                ))}
              </optgroup>
            </select>
            {isRegionAvailable(region) ? (
              <p className="text-xs text-ghost mb-5">{t('checkout.availableHint')}</p>
            ) : (
              <p className="text-xs text-warning mb-5">
                {t('checkout.comingSoonHint', {
                  region: defaultRegionLabelKey ? t(defaultRegionLabelKey) : '',
                })}
              </p>
            )}

            <label className="block text-sm font-medium text-light mb-2" htmlFor="node-email">
              {t('checkout.emailLabel')}{' '}
              <span className="text-ghost font-normal">{t('checkout.emailOptional')}</span>
            </label>
            <Input
              id="node-email"
              type="email"
              placeholder={t('checkout.emailPlaceholder')}
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              disabled={submitting}
              className="mb-1"
            />
            <p className="text-xs text-ghost mb-6">{t('checkout.emailHint')}</p>

            {error && (
              <div className="mb-4 p-3 rounded-lg bg-error/10 border border-error/30 flex gap-2 text-sm text-error">
                <AlertCircle size={18} className="shrink-0 mt-0.5" />
                <span>{error}</span>
              </div>
            )}

            <Button
              size="lg"
              className="w-full justify-center"
              onClick={handleGetNode}
              disabled={submitting}
            >
              {submitting ? (
                <>
                  <Loader2 className="animate-spin mr-2" size={18} />
                  {t('checkout.submitting')}
                </>
              ) : (
                t('checkout.submit')
              )}
            </Button>
            <p className="text-xs text-center text-ghost mt-3">{t('checkout.secureNote')}</p>
          </Card>
        </div>
      </main>
    </div>
  )
}

/** Shared page chrome (header) for the node success / status pages. */
function NodePageShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-h-screen flex flex-col">
      <header className="fixed top-0 left-0 right-0 z-50 backdrop-blur-md bg-void/80 border-b border-steel">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <Logo size="md" showText={false} />
            <span className="font-display text-lg sm:text-xl font-semibold">Botho</span>
          </Link>
          <LocaleSwitcher className="whitespace-nowrap" />
        </div>
      </header>
      <main className="flex-1 flex items-center justify-center pt-28 px-4 sm:px-6">
        {children}
      </main>
    </div>
  )
}

/** Delay (ms) between `/session-status` polls while provisioning is pending. */
const SESSION_POLL_INTERVAL_MS = 3000
/** Stop polling after this many attempts (~1 min at 3s) — the email is the fallback. */
const SESSION_POLL_MAX_ATTEMPTS = 20

/** Exchange state for the success page's `session_id` → status-link poll. */
type SessionExchangeState =
  | { kind: 'pending' }
  | { kind: 'ready'; statusUrl: string }
  | { kind: 'error' }
  | { kind: 'no-session' }

/**
 * Post-checkout success page (#458 §4, #805 part 1). Stripe redirects here after
 * a completed checkout (`/node/success?session_id=...`). Provisioning is
 * asynchronous (the webhook launches the node), so this page exchanges the
 * `session_id` for a magic-link status URL via the control-plane Worker, polling
 * while provisioning lands and rendering:
 *   - pending: a spinner while the node comes up,
 *   - ready:   a "View your node status" link,
 *   - error:   a fallback if the session can't be confirmed,
 *   - no-session: the plain confirmation when no `session_id` is present.
 */
export function NodeSuccessPage() {
  const { t } = useTranslation('node')
  const sessionId =
    typeof window !== 'undefined' ? sessionIdFromSearch(window.location.search) : null
  const [state, setState] = useState<SessionExchangeState>(
    sessionId ? { kind: 'pending' } : { kind: 'no-session' },
  )

  useEffect(() => {
    if (!sessionId) return
    let cancelled = false
    let attempts = 0
    let timer: ReturnType<typeof setTimeout> | undefined

    const poll = async () => {
      attempts += 1
      try {
        const result = await fetchSessionStatus(sessionId)
        if (cancelled) return
        if (result.kind === 'ready') {
          setState({ kind: 'ready', statusUrl: result.statusUrl })
          return
        }
        // pending → keep polling until we hit the attempt cap.
        if (attempts >= SESSION_POLL_MAX_ATTEMPTS) {
          setState({ kind: 'no-session' })
          return
        }
        timer = setTimeout(() => void poll(), SESSION_POLL_INTERVAL_MS)
      } catch (err) {
        if (cancelled) return
        // A terminal 401 (unknown/expired session) stops polling with an error.
        if (err instanceof NodeStatusError && err.status === 401) {
          setState({ kind: 'error' })
          return
        }
        // Transient error: retry until the cap, then fall back to the email note.
        if (attempts >= SESSION_POLL_MAX_ATTEMPTS) {
          setState({ kind: 'no-session' })
          return
        }
        timer = setTimeout(() => void poll(), SESSION_POLL_INTERVAL_MS)
      }
    }

    void poll()
    return () => {
      cancelled = true
      if (timer) clearTimeout(timer)
    }
  }, [sessionId])

  return (
    <NodePageShell>
      <Card className="max-w-md w-full p-6 sm:p-8 text-center">
        <div className="w-14 h-14 rounded-full bg-success/10 flex items-center justify-center mx-auto mb-5">
          <Check className="text-success" size={28} />
        </div>
        <h1 className="font-display text-2xl sm:text-3xl font-bold mb-3">
          {t('success.title')}
        </h1>
        <p className="text-sm sm:text-base text-ghost mb-6">{t('success.body')}</p>

        {state.kind === 'pending' && (
          <div className="flex items-center justify-center gap-2 text-ghost mb-6">
            <Loader2 className="animate-spin" size={18} />
            <span className="text-sm">{t('success.provisioning')}</span>
          </div>
        )}

        {state.kind === 'ready' && (
          <a href={state.statusUrl} className="block mb-6">
            <Button size="lg" className="w-full justify-center gap-2">
              <ExternalLink size={18} />
              {t('success.viewStatus')}
            </Button>
          </a>
        )}

        {state.kind === 'error' && (
          <div className="mb-6 p-3 rounded-lg bg-warning/10 border border-warning/30 flex gap-2 text-sm text-warning text-left">
            <AlertCircle size={18} className="shrink-0 mt-0.5" />
            <span>{t('success.linkError')}</span>
          </div>
        )}

        {state.kind === 'no-session' && (
          <p className="text-sm text-ghost mb-6">{t('success.noSession')}</p>
        )}

        <div className="flex flex-col gap-3">
          <Link to="/wallet">
            <Button size="lg" variant="ghost" className="w-full justify-center">
              {t('success.openWallet')}
            </Button>
          </Link>
          <Link to="/" className="text-sm text-ghost hover:text-light transition-colors">
            {t('success.backToHome')}
          </Link>
        </div>
      </Card>
    </NodePageShell>
  )
}

/** i18n translator bound to the `node` namespace (from `useTranslation('node')`). */
type NodeT = ReturnType<typeof useTranslation<'node'>>['t']

/** Colored dot + label for a node's lifecycle state. */
function StateBadge({ state, t }: { state: NodeStatus['state']; t: NodeT }) {
  const map: Record<NodeStatus['state'], { labelKey: string; cls: string }> = {
    provisioning: { labelKey: 'status.state.provisioning', cls: 'bg-warning/20 text-warning' },
    running: { labelKey: 'status.state.running', cls: 'bg-success/20 text-success' },
    suspended: { labelKey: 'status.state.suspended', cls: 'bg-warning/20 text-warning' },
    terminated: { labelKey: 'status.state.terminated', cls: 'bg-danger/20 text-danger' },
  }
  const { labelKey, cls } = map[state]
  return <span className={`px-2 py-0.5 rounded text-xs font-medium ${cls}`}>{t(labelKey)}</span>
}

/** One-line health summary from node_getStatus. */
function healthSummary(health: NodeStatus['health'], t: NodeT): string {
  if (health.status === 'unknown') return t('status.health.notReporting')
  if (health.status === 'offline') return t('status.health.unreachable')
  const h =
    health.chainHeight != null
      ? t('status.health.height', { height: health.chainHeight })
      : t('status.health.online')
  const sync = health.synced ? t('status.health.synced') : `${Math.round(health.syncProgress ?? 0)}%`
  return `${h} · ${sync}`
}

/**
 * Node status page (P6.3, #458 §3 step 5 / §4 / §6). Reached via a magic link
 * (`/node/status?token=...`) — the MVP identity model (no password, the signed
 * link is the credential). Shows the node's RPC URL, state, and live health, plus
 * an "Open in wallet" deep link (pre-selects the node as the custom RPC ingress)
 * and a "Manage Subscription" button (Stripe Customer Portal).
 */
export function NodeStatusPage() {
  const { t } = useTranslation('node')
  const [status, setStatus] = useState<NodeStatus | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [portalBusy, setPortalBusy] = useState(false)

  const token =
    typeof window !== 'undefined' ? tokenFromSearch(window.location.search) : null

  const load = useCallback(async () => {
    if (!token) {
      setError(t('status.missingToken'))
      setLoading(false)
      return
    }
    setLoading(true)
    setError(null)
    try {
      setStatus(await fetchNodeStatus(token))
    } catch (err) {
      setError(err instanceof Error ? err.message : t('status.loadError'))
    } finally {
      setLoading(false)
    }
  }, [token, t])

  useEffect(() => {
    void load()
  }, [load])

  async function handleCopy() {
    if (!status) return
    try {
      await navigator.clipboard.writeText(status.rpcUrl)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // clipboard unavailable — ignore.
    }
  }

  async function handleManage() {
    if (!token) return
    setPortalBusy(true)
    try {
      const url = await createPortalUrl(token)
      window.location.assign(url)
    } catch (err) {
      setError(err instanceof Error ? err.message : t('status.portalError'))
      setPortalBusy(false)
    }
  }

  return (
    <NodePageShell>
      <Card className="max-w-lg w-full p-6 sm:p-8">
        <div className="flex items-center gap-3 mb-6">
          <div className="w-11 h-11 rounded-lg bg-pulse/10 flex items-center justify-center">
            <Server className="text-pulse" size={22} />
          </div>
          <div>
            <h1 className="font-display text-xl sm:text-2xl font-bold">{t('status.title')}</h1>
            <p className="text-xs text-ghost">{t('status.subtitle')}</p>
          </div>
        </div>

        {loading && (
          <div className="flex items-center gap-2 text-ghost py-8 justify-center">
            <Loader2 className="animate-spin" size={18} />
            {t('status.loading')}
          </div>
        )}

        {!loading && error && (
          <div className="p-4 rounded-xl bg-error/10 border border-error/30 flex gap-3 text-sm text-error">
            <AlertCircle size={18} className="shrink-0 mt-0.5" />
            <div>
              <p>{error}</p>
              {token && (
                <button onClick={load} className="underline mt-2 text-light">
                  {t('status.tryAgain')}
                </button>
              )}
            </div>
          </div>
        )}

        {!loading && status && (
          <div className="flex flex-col gap-5">
            <div className="flex items-center justify-between">
              <span className="text-sm text-ghost">{t('status.statusLabel')}</span>
              <div className="flex items-center gap-2">
                <StateBadge state={status.state} t={t} />
                <span className="text-xs text-ghost">{healthSummary(status.health, t)}</span>
              </div>
            </div>

            <div>
              <span className="text-sm text-ghost block mb-1.5">{t('status.rpcLabel')}</span>
              <div className="flex gap-2">
                <code className="flex-1 px-3 py-2 rounded-lg bg-void border border-steel text-xs sm:text-sm text-light break-all">
                  {status.rpcUrl}
                </code>
                <Button size="sm" variant="ghost" onClick={handleCopy} aria-label={t('status.copyRpcAria')}>
                  {copied ? <Check size={16} className="text-success" /> : <Copy size={16} />}
                </Button>
              </div>
            </div>

            <div className="flex items-center justify-between text-sm">
              <span className="text-ghost">{t('status.regionLabel')}</span>
              <span className="text-light">{status.region}</span>
            </div>

            <div className="flex flex-col gap-3 pt-1">
              {/* Deep link: opens the wallet with this node pre-selected as the
                  custom RPC ingress (#458 §3 step 5). */}
              <a href={status.walletDeepLink}>
                <Button size="lg" className="w-full justify-center gap-2">
                  <ExternalLink size={18} />
                  {t('status.openInWallet')}
                </Button>
              </a>
              <Button
                size="lg"
                variant="ghost"
                className="w-full justify-center gap-2"
                onClick={handleManage}
                disabled={portalBusy}
              >
                {portalBusy ? (
                  <Loader2 className="animate-spin" size={18} />
                ) : (
                  <CreditCard size={18} />
                )}
                {t('status.manageSubscription')}
              </Button>
            </div>
          </div>
        )}
      </Card>
    </NodePageShell>
  )
}
