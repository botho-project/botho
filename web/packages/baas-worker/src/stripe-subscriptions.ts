/**
 * Stripe subscription-status client for the SEC reconciliation sweep (#508,
 * #458 §5).
 *
 * The reconciliation cron needs to answer one question per managed rig: "is the
 * `botho:subscription` tag on this EC2 instance still an ACTIVE Stripe
 * subscription?" If not (cancelled, unpaid, or never existed), the rig is an
 * orphan and must be reaped to stop cost bleed.
 *
 * The reconciler depends on the `SubscriptionChecker` *interface* — never the
 * concrete implementation — so tests use an in-memory fake and NO real Stripe
 * call happens in a test code path (mirrors the EC2/DNS/D1 injectable pattern).
 *
 * Secrets: the Stripe secret key comes from a Worker secret — never the repo.
 */

/**
 * Stripe subscription `status` values. A subscription only entitles a running
 * rig while it is in an ACTIVE state. `trialing` counts as active (the customer
 * is in a paid relationship); everything else means "stop the rig".
 *
 * Reference: Stripe Subscription.status.
 */
export const ACTIVE_SUBSCRIPTION_STATUSES = new Set([
  'active',
  'trialing',
])

/** True if a Stripe subscription status still entitles a running rig. */
export function isActiveSubscriptionStatus(status: string | undefined): boolean {
  return status !== undefined && ACTIVE_SUBSCRIPTION_STATUSES.has(status)
}

/**
 * Injectable Stripe surface for the reconciler. The single method answers
 * "should the rig backed by this subscription keep running?".
 */
export interface SubscriptionChecker {
  /**
   * Return whether `subscriptionId` is an ACTIVE Stripe subscription. A
   * cancelled/unpaid/absent subscription returns false (→ reap the rig). MUST be
   * conservative on transient lookup errors: see `HttpSubscriptionChecker`,
   * which throws so the sweep can SKIP that rig rather than wrongly reaping a
   * paying customer's box on a Stripe hiccup.
   */
  isActive(subscriptionId: string): Promise<boolean>
}

/** Error from the Stripe subscriptions API (transient / non-404). */
export class StripeSubscriptionError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message)
    this.name = 'StripeSubscriptionError'
  }
}

/**
 * Real Stripe subscription checker. `fetchImpl` is injectable (defaults to
 * global fetch); the reconciler's tests use the fake instead, so this code path
 * never runs under test.
 *
 * Behavior:
 *   - 200 + an active status  → true
 *   - 200 + a non-active status (canceled/unpaid/past_due/incomplete...) → false
 *   - 404 (no such subscription) → false (definitely an orphan)
 *   - any other non-2xx → THROW (transient): the sweep treats a throw as
 *     "skip this rig this cycle" so a Stripe outage never reaps paying rigs.
 */
export class HttpSubscriptionChecker implements SubscriptionChecker {
  constructor(
    private readonly stripeSecretKey: string,
    private readonly fetchImpl: typeof fetch = fetch,
  ) {}

  async isActive(subscriptionId: string): Promise<boolean> {
    const resp = await this.fetchImpl(
      `https://api.stripe.com/v1/subscriptions/${encodeURIComponent(subscriptionId)}`,
      {
        method: 'GET',
        headers: {
          Authorization: `Bearer ${this.stripeSecretKey}`,
          'Stripe-Version': '2024-06-20',
        },
      },
    )

    if (resp.status === 404) {
      // The subscription does not exist in Stripe at all → orphan.
      return false
    }

    const json = (await resp.json().catch(() => ({}))) as {
      status?: string
      error?: { message?: string }
    }

    if (!resp.ok) {
      // Transient / auth / rate-limit error — do NOT treat as "inactive".
      throw new StripeSubscriptionError(
        json.error?.message ?? `Stripe returned HTTP ${resp.status}`,
        resp.status,
      )
    }

    return isActiveSubscriptionStatus(json.status)
  }
}
