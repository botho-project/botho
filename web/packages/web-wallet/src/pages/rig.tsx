import { useState, useEffect, useCallback } from 'react'
import { Link } from 'react-router-dom'
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
  DEFAULT_RIG_REGION,
  RIG_REGIONS,
  RigCheckoutError,
  startRigCheckout,
} from '../lib/rig-checkout'
import {
  createPortalUrl,
  fetchRigStatus,
  tokenFromSearch,
  type RigStatus,
} from '../lib/rig-status'

/**
 * P7.1 — "Get a rig" surface (#458 §2, §4; issue #504).
 *
 * A thin signup page that lets a user buy a managed Botho mining node for
 * $50/mo. It collects the desired AWS region (allowlist, #458 §5), then asks the
 * control-plane Worker (`@botho/baas-worker /checkout`) to create a Stripe
 * Checkout Session and redirects the browser to the Stripe-hosted page.
 *
 * Honest testnet copy (#458 §7): the value prop today is "your own always-on
 * node + the wallet experience + participation in mining," NOT profit. Billing
 * runs in Stripe TEST mode while on testnet.
 *
 * Webhook → provisioning is P7.2 (#506); the rich status page is P6.3. After
 * checkout, Stripe redirects to `/rig/success` (the placeholder below).
 */
export function RigPage() {
  const [region, setRegion] = useState<string>(DEFAULT_RIG_REGION)
  const [email, setEmail] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleGetRig() {
    setError(null)
    setSubmitting(true)
    try {
      const session = await startRigCheckout({
        region,
        email: email.trim() || undefined,
      })
      // Redirect the browser to the Stripe-hosted checkout page.
      window.location.assign(session.url)
    } catch (err) {
      const message =
        err instanceof RigCheckoutError
          ? err.message
          : 'Something went wrong starting checkout. Please try again.'
      setError(message)
      setSubmitting(false)
    }
  }

  return (
    <div className="min-h-screen">
      {/* Header */}
      <header className="fixed top-0 left-0 right-0 z-50 backdrop-blur-md bg-void/80 border-b border-steel">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <Logo size="md" showText={false} />
            <span className="font-display text-lg sm:text-xl font-semibold">Botho</span>
          </Link>
          <Link to="/" className="text-ghost hover:text-light transition-colors flex items-center gap-2">
            <ArrowLeft size={18} />
            Back
          </Link>
        </div>
      </header>

      <main className="pt-28 sm:pt-32 pb-16 px-4 sm:px-6">
        <div className="max-w-3xl mx-auto">
          {/* Hero */}
          <div className="text-center mb-10">
            <div className="inline-flex items-center gap-2 px-3 py-1.5 rounded-full bg-steel/50 border border-muted text-xs sm:text-sm text-ghost mb-6">
              <span className="w-2 h-2 rounded-full bg-warning" />
              Testnet — billing runs in Stripe test mode
            </div>
            <div className="w-14 h-14 rounded-xl bg-pulse/10 flex items-center justify-center mx-auto mb-5">
              <Server className="text-pulse" size={28} />
            </div>
            <h1 className="font-display text-3xl sm:text-4xl md:text-5xl font-bold mb-4">
              Get a Managed Rig
            </h1>
            <p className="text-base sm:text-lg text-ghost max-w-xl mx-auto">
              Your own always-on Botho node in the cloud — joins the testnet,
              participates in mining, and gives you a private RPC endpoint for the
              wallet. <span className="text-light">$50/month.</span>
            </p>
          </div>

          {/* Honest testnet caveat (#458 §7) */}
          <div className="mb-8 p-4 rounded-xl bg-warning/10 border border-warning/30 flex gap-3">
            <AlertCircle className="text-warning shrink-0 mt-0.5" size={20} />
            <p className="text-sm text-ghost">
              <span className="text-light font-medium">Testnet, no profit promised.</span>{' '}
              Rigs mine <span className="text-light">testnet</span> BTH, which has no
              real-world value today. What you get now is your own always-on node,
              the full wallet experience, and a hand in keeping the network running —
              not income. Charges run in Stripe test mode while we validate the
              service.
            </p>
          </div>

          {/* What you get */}
          <div className="grid sm:grid-cols-3 gap-3 sm:gap-4 mb-8">
            {[
              { icon: Cpu, title: 'Always-on node', desc: 'A t4g.medium running RandomX, joined to the testnet.' },
              { icon: Globe, title: 'Private RPC', desc: 'Your own HTTPS endpoint to point the wallet at.' },
              { icon: ShieldCheck, title: 'Non-custodial', desc: 'Keys stay on your device. The node never holds your funds.' },
            ].map((f) => (
              <div key={f.title} className="p-4 rounded-xl bg-slate/50 border border-steel">
                <f.icon className="text-pulse mb-2" size={20} />
                <div className="font-display font-semibold text-sm mb-1">{f.title}</div>
                <div className="text-xs text-ghost">{f.desc}</div>
              </div>
            ))}
          </div>

          {/* Checkout form */}
          <Card className="p-5 sm:p-6">
            <label className="block text-sm font-medium text-light mb-2" htmlFor="rig-region">
              Region
            </label>
            <select
              id="rig-region"
              value={region}
              onChange={(e) => setRegion(e.target.value)}
              disabled={submitting}
              className="w-full mb-1 px-3 py-2.5 rounded-lg bg-void border border-steel text-light focus:outline-none focus:border-pulse disabled:opacity-50"
            >
              {RIG_REGIONS.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.label}
                </option>
              ))}
            </select>
            <p className="text-xs text-ghost mb-5">
              More regions coming soon. Your node will be provisioned here once you
              subscribe.
            </p>

            <label className="block text-sm font-medium text-light mb-2" htmlFor="rig-email">
              Email <span className="text-ghost font-normal">(optional)</span>
            </label>
            <Input
              id="rig-email"
              type="email"
              placeholder="you@example.com"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              disabled={submitting}
              className="mb-1"
            />
            <p className="text-xs text-ghost mb-6">
              Pre-fills Stripe checkout and is where we'll send your node details.
              You can also enter it on the next screen.
            </p>

            {error && (
              <div className="mb-4 p-3 rounded-lg bg-error/10 border border-error/30 flex gap-2 text-sm text-error">
                <AlertCircle size={18} className="shrink-0 mt-0.5" />
                <span>{error}</span>
              </div>
            )}

            <Button
              size="lg"
              className="w-full justify-center"
              onClick={handleGetRig}
              disabled={submitting}
            >
              {submitting ? (
                <>
                  <Loader2 className="animate-spin mr-2" size={18} />
                  Redirecting to Stripe…
                </>
              ) : (
                'Subscribe — $50/mo'
              )}
            </Button>
            <p className="text-xs text-center text-ghost mt-3">
              Secure checkout hosted by Stripe. Cancel anytime.
            </p>
          </Card>
        </div>
      </main>
    </div>
  )
}

/** Shared page chrome (header) for the rig success / status pages. */
function RigPageShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-h-screen flex flex-col">
      <header className="fixed top-0 left-0 right-0 z-50 backdrop-blur-md bg-void/80 border-b border-steel">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <Logo size="md" showText={false} />
            <span className="font-display text-lg sm:text-xl font-semibold">Botho</span>
          </Link>
        </div>
      </header>
      <main className="flex-1 flex items-center justify-center pt-28 px-4 sm:px-6">
        {children}
      </main>
    </div>
  )
}

/**
 * Post-checkout success page (#458 §4). Stripe redirects here after a completed
 * checkout (`/rig/success?session_id=...`). Provisioning is asynchronous (the
 * webhook launches the node), so this confirms the subscription and points the
 * user at the live status page once they have their magic link.
 */
export function RigSuccessPage() {
  return (
    <RigPageShell>
      <Card className="max-w-md w-full p-6 sm:p-8 text-center">
        <div className="w-14 h-14 rounded-full bg-success/10 flex items-center justify-center mx-auto mb-5">
          <Check className="text-success" size={28} />
        </div>
        <h1 className="font-display text-2xl sm:text-3xl font-bold mb-3">
          Subscription started
        </h1>
        <p className="text-sm sm:text-base text-ghost mb-6">
          Thanks! Your managed rig is being provisioned. We'll email you a secure
          link to your node's status page — it shows your private RPC URL, the
          rig's health, and a one-click link to open it in the wallet.
        </p>
        <div className="flex flex-col gap-3">
          <Link to="/wallet">
            <Button size="lg" className="w-full justify-center">
              Open Wallet
            </Button>
          </Link>
          <Link to="/" className="text-sm text-ghost hover:text-light transition-colors">
            Back to home
          </Link>
        </div>
      </Card>
    </RigPageShell>
  )
}

/** Colored dot + label for a rig's lifecycle state. */
function StateBadge({ state }: { state: RigStatus['state'] }) {
  const map: Record<RigStatus['state'], { label: string; cls: string }> = {
    provisioning: { label: 'Provisioning', cls: 'bg-warning/20 text-warning' },
    running: { label: 'Running', cls: 'bg-success/20 text-success' },
    suspended: { label: 'Suspended', cls: 'bg-warning/20 text-warning' },
    terminated: { label: 'Terminated', cls: 'bg-danger/20 text-danger' },
  }
  const { label, cls } = map[state]
  return <span className={`px-2 py-0.5 rounded text-xs font-medium ${cls}`}>{label}</span>
}

/** One-line health summary from node_getStatus. */
function healthSummary(health: RigStatus['health']): string {
  if (health.status === 'unknown') return 'Not yet reporting'
  if (health.status === 'offline') return 'Unreachable'
  const h = health.chainHeight != null ? `height ${health.chainHeight}` : 'online'
  const sync = health.synced ? 'synced' : `${Math.round(health.syncProgress ?? 0)}%`
  return `${h} · ${sync}`
}

/**
 * Rig status page (P6.3, #458 §3 step 5 / §4 / §6). Reached via a magic link
 * (`/rig/status?token=...`) — the MVP identity model (no password, the signed
 * link is the credential). Shows the rig's RPC URL, state, and live health, plus
 * an "Open in wallet" deep link (pre-selects the rig as the custom RPC ingress)
 * and a "Manage Subscription" button (Stripe Customer Portal).
 */
export function RigStatusPage() {
  const [status, setStatus] = useState<RigStatus | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [portalBusy, setPortalBusy] = useState(false)

  const token =
    typeof window !== 'undefined' ? tokenFromSearch(window.location.search) : null

  const load = useCallback(async () => {
    if (!token) {
      setError('This status link is missing its access token.')
      setLoading(false)
      return
    }
    setLoading(true)
    setError(null)
    try {
      setStatus(await fetchRigStatus(token))
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not load your rig status.')
    } finally {
      setLoading(false)
    }
  }, [token])

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
      setError(err instanceof Error ? err.message : 'Could not open the billing portal.')
      setPortalBusy(false)
    }
  }

  return (
    <RigPageShell>
      <Card className="max-w-lg w-full p-6 sm:p-8">
        <div className="flex items-center gap-3 mb-6">
          <div className="w-11 h-11 rounded-lg bg-pulse/10 flex items-center justify-center">
            <Server className="text-pulse" size={22} />
          </div>
          <div>
            <h1 className="font-display text-xl sm:text-2xl font-bold">Your managed rig</h1>
            <p className="text-xs text-ghost">Botho-as-a-Service · testnet</p>
          </div>
        </div>

        {loading && (
          <div className="flex items-center gap-2 text-ghost py-8 justify-center">
            <Loader2 className="animate-spin" size={18} />
            Loading your rig…
          </div>
        )}

        {!loading && error && (
          <div className="p-4 rounded-xl bg-error/10 border border-error/30 flex gap-3 text-sm text-error">
            <AlertCircle size={18} className="shrink-0 mt-0.5" />
            <div>
              <p>{error}</p>
              {token && (
                <button onClick={load} className="underline mt-2 text-light">
                  Try again
                </button>
              )}
            </div>
          </div>
        )}

        {!loading && status && (
          <div className="flex flex-col gap-5">
            <div className="flex items-center justify-between">
              <span className="text-sm text-ghost">Status</span>
              <div className="flex items-center gap-2">
                <StateBadge state={status.state} />
                <span className="text-xs text-ghost">{healthSummary(status.health)}</span>
              </div>
            </div>

            <div>
              <span className="text-sm text-ghost block mb-1.5">RPC endpoint</span>
              <div className="flex gap-2">
                <code className="flex-1 px-3 py-2 rounded-lg bg-void border border-steel text-xs sm:text-sm text-light break-all">
                  {status.rpcUrl}
                </code>
                <Button size="sm" variant="ghost" onClick={handleCopy} aria-label="Copy RPC URL">
                  {copied ? <Check size={16} className="text-success" /> : <Copy size={16} />}
                </Button>
              </div>
            </div>

            <div className="flex items-center justify-between text-sm">
              <span className="text-ghost">Region</span>
              <span className="text-light">{status.region}</span>
            </div>

            <div className="flex flex-col gap-3 pt-1">
              {/* Deep link: opens the wallet with this rig pre-selected as the
                  custom RPC ingress (#458 §3 step 5). */}
              <a href={status.walletDeepLink}>
                <Button size="lg" className="w-full justify-center gap-2">
                  <ExternalLink size={18} />
                  Open in wallet
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
                Manage subscription
              </Button>
            </div>
          </div>
        )}
      </Card>
    </RigPageShell>
  )
}
