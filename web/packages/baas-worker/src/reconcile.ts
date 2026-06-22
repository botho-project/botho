/**
 * SEC reconciliation sweep (#508, #458 §5) — the cost-bleed safety net.
 *
 * Reliable de-provisioning on cancel/payment-failure is critical: a rig left
 * running after a subscription ends bleeds AWS cost indefinitely. The webhook
 * (#506) tears down on the cancel/payment-failed events, but webhooks can be
 * missed, mis-delivered, or race a crashed Queue consumer. This scheduled sweep
 * is the belt-and-suspenders backstop:
 *
 *   1. List EVERY EC2 instance tagged `botho:managed-rig=true` (the same tag IAM
 *      uses to gate TerminateInstances — so the sweep can only ever see/act on
 *      managed rigs, NEVER the seed/seed2/faucet nodes).
 *   2. For each, read its `botho:subscription` tag and cross-check it against
 *      Stripe: is that subscription still ACTIVE?
 *   3. Reap orphans — terminate the instance, delete its DNS record, mark the D1
 *      row `terminated`:
 *        - subscription cancelled / unpaid / absent in Stripe → orphan
 *        - "stuck provisioning": never reached `running` within a threshold AND
 *          (defensively) carries no/active subscription tag → orphan
 *   4. Leave instances whose subscription is still active strictly alone.
 *   5. On a TRANSIENT Stripe lookup error, SKIP that instance this cycle (never
 *      reap a paying customer's rig because Stripe was briefly unreachable).
 *
 * Everything is injectable (EC2 / DNS / D1 / Stripe are interfaces), so the
 * sweep is unit-tested with in-memory fakes and NO real AWS/Stripe/DNS/D1 call
 * happens in a test code path.
 */

import type { DnsClient } from './cloudflare-dns'
import type { Ec2Client, Ec2Instance } from './ec2'
import { isLiveInstanceState } from './ec2'
import {
  DEFAULT_RIG_DOMAIN,
  rigHostname,
  TAG_MANAGED_RIG,
} from './rig-config'
import type { RigStore } from './rig-store'
import {
  StripeSubscriptionError,
  type SubscriptionChecker,
} from './stripe-subscriptions'

/** Default age after which a not-yet-`running` rig is considered stuck. */
export const DEFAULT_STUCK_PROVISIONING_MS = 30 * 60 * 1000 // 30 minutes

/** Why a given instance was reaped (or left), for observability + tests. */
export type ReconcileDisposition =
  | 'active' // subscription active → left running
  | 'orphan_cancelled' // subscription cancelled/unpaid/absent → reaped
  | 'orphan_no_subscription_tag' // managed rig with no sub tag → reaped
  | 'orphan_stuck_provisioning' // never reached running past threshold → reaped
  | 'skipped_stripe_error' // transient Stripe error → skipped this cycle
  | 'skipped_not_live' // already terminating/terminated → ignored

/** Per-instance outcome of one sweep. */
export interface ReconcileItem {
  instanceId: string
  region: string
  subscriptionId?: string
  disposition: ReconcileDisposition
  reaped: boolean
}

/** Aggregate result of one full sweep. */
export interface ReconcileReport {
  scanned: number
  reaped: number
  skipped: number
  items: ReconcileItem[]
}

/** Everything the reconciler needs, injected for testability. */
export interface ReconcileDeps {
  ec2: Ec2Client
  dns: DnsClient
  store: RigStore
  stripe: SubscriptionChecker
  /** Regions to sweep. The provisioner only launches in the allowlist. */
  regions: string[]
  /** Zone for rig hostnames (DNS cleanup). Default: testnet.botho.io. */
  rigDomain?: string
  /** Age (ms) after which a non-`running` rig is reaped as stuck. */
  stuckProvisioningMs?: number
  /** Injectable clock (epoch ms) so the stuck threshold is deterministic in tests. */
  now?: () => number
}

/**
 * Run one reconciliation sweep across all configured regions. Never throws on a
 * single-instance failure: each instance is handled independently and its
 * outcome recorded, so one bad row can't abort the whole sweep.
 */
export async function reconcileOnce(deps: ReconcileDeps): Promise<ReconcileReport> {
  const rigDomain = deps.rigDomain ?? DEFAULT_RIG_DOMAIN
  const stuckMs = deps.stuckProvisioningMs ?? DEFAULT_STUCK_PROVISIONING_MS
  const now = deps.now ? deps.now() : Date.now()

  const items: ReconcileItem[] = []

  for (const region of deps.regions) {
    let instances: Ec2Instance[]
    try {
      instances = await deps.ec2.describeManagedRigs(region)
    } catch (err) {
      // Can't list this region this cycle — skip it entirely (next cycle retries).
      console.error('reconcile: describeManagedRigs failed', region, String(err))
      continue
    }

    for (const inst of instances) {
      const item = await reconcileInstance(inst, region, deps, {
        rigDomain,
        stuckMs,
        now,
      })
      items.push(item)
    }
  }

  const reaped = items.filter((i) => i.reaped).length
  const skipped = items.filter(
    (i) => i.disposition === 'skipped_stripe_error' || i.disposition === 'skipped_not_live',
  ).length
  return { scanned: items.length, reaped, skipped, items }
}

/** Classify + (if needed) reap a single managed-rig instance. */
async function reconcileInstance(
  inst: Ec2Instance,
  region: string,
  deps: ReconcileDeps,
  cfg: { rigDomain: string; stuckMs: number; now: number },
): Promise<ReconcileItem> {
  const subscriptionId = inst.subscriptionTag

  // Already terminating/terminated: nothing to do.
  if (!isLiveInstanceState(inst.state)) {
    return {
      instanceId: inst.instanceId,
      region,
      subscriptionId,
      disposition: 'skipped_not_live',
      reaped: false,
    }
  }

  // A managed rig with NO subscription tag can never map to a paying customer —
  // it is an orphan by definition (e.g. a launch that lost its tag, or a manual
  // box that wrongly carries botho:managed-rig). Reap it.
  if (!subscriptionId) {
    await reap(inst, region, deps, cfg.rigDomain)
    return {
      instanceId: inst.instanceId,
      region,
      disposition: 'orphan_no_subscription_tag',
      reaped: true,
    }
  }

  // Cross-check the subscription against Stripe.
  let active: boolean
  try {
    active = await deps.stripe.isActive(subscriptionId)
  } catch (err) {
    // Transient Stripe error → SKIP (never reap a paying rig on a Stripe hiccup).
    if (!(err instanceof StripeSubscriptionError)) {
      console.error('reconcile: unexpected Stripe error', subscriptionId, String(err))
    }
    return {
      instanceId: inst.instanceId,
      region,
      subscriptionId,
      disposition: 'skipped_stripe_error',
      reaped: false,
    }
  }

  if (active) {
    // Paying customer — leave it strictly alone, even if "stuck" (a slow boot
    // for an active subscription is not an orphan; teardown only on non-active).
    return {
      instanceId: inst.instanceId,
      region,
      subscriptionId,
      disposition: 'active',
      reaped: false,
    }
  }

  // Subscription is NOT active (cancelled / unpaid / absent). Reap.
  await reap(inst, region, deps, cfg.rigDomain)

  // Distinguish a stuck-provisioning reap (never reached running past the
  // threshold) from a plain cancelled one, for observability.
  const stuck =
    inst.state !== 'running' &&
    inst.launchTimeMs !== undefined &&
    cfg.now - inst.launchTimeMs > cfg.stuckMs
  return {
    instanceId: inst.instanceId,
    region,
    subscriptionId,
    disposition: stuck ? 'orphan_stuck_provisioning' : 'orphan_cancelled',
    reaped: true,
  }
}

/**
 * Terminate an instance, delete its DNS record, and mark the D1 row terminated.
 * Best-effort and independent: a failure in one step is logged but does not
 * abort the others or the sweep. The terminate is the cost-critical step.
 */
async function reap(
  inst: Ec2Instance,
  region: string,
  deps: ReconcileDeps,
  rigDomain: string,
): Promise<void> {
  // 1. Terminate (the cost-bleed stopper). IAM restricts TerminateInstances to
  //    botho:managed-rig=true resources, so this can never hit a seed/faucet node.
  try {
    await deps.ec2.terminateInstance(region, inst.instanceId)
  } catch (err) {
    console.error('reconcile: terminate failed', inst.instanceId, String(err))
  }

  // 2. Delete the DNS record if we can derive the hostname from the rig-id tag.
  if (inst.rigIdTag) {
    try {
      await deps.dns.deleteARecord(rigHostname(inst.rigIdTag, rigDomain))
    } catch (err) {
      console.error('reconcile: dns delete failed', inst.rigIdTag, String(err))
    }
  }

  // 3. Mark the D1 row terminated (idempotent; no-op if the row is absent).
  if (inst.subscriptionTag) {
    try {
      await deps.store.setState(inst.subscriptionTag, 'terminated')
    } catch (err) {
      console.error('reconcile: D1 setState failed', inst.subscriptionTag, String(err))
    }
  }
}

/** The managed-rig tag the sweep filters on (re-exported for callers/docs). */
export const MANAGED_RIG_TAG = TAG_MANAGED_RIG
