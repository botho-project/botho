/**
 * Build `ReconcileDeps` from the Worker environment for the SEC reconciliation
 * cron (#508, #458 §5). Kept separate from `reconcile.ts` so the sweep logic
 * stays pure/testable with fakes while this thin adapter wires the production
 * clients (the same split as `provisioner-env.ts`).
 *
 * The scheduled handler calls `reconcileOnce(reconcileDepsFromEnv(env))`.
 *
 * SECRETS (Worker secrets, never the repo — #458 §5):
 *   AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, (optional) AWS_SESSION_TOKEN
 *   CF_DNS_API_TOKEN, CF_DNS_ZONE_ID, STRIPE_SECRET_KEY
 * VARS (non-secret): NODE_DOMAIN, RECONCILE_REGIONS, STUCK_PROVISIONING_MS.
 * BINDING: DB (D1).
 */

import { HttpDnsClient } from './cloudflare-dns'
import { HttpEc2Client } from './ec2'
import type { ProvisionerEnv } from './provisioner-env'
import {
  DEFAULT_NODE_DOMAIN,
  REGION_ALLOWLIST,
} from './node-config'
import { DEFAULT_STUCK_PROVISIONING_MS, type ReconcileDeps } from './reconcile'
import { D1NodeStore, type D1Like } from './node-store'
import { HttpSubscriptionChecker } from './stripe-subscriptions'

/** Worker env keys the reconciler needs beyond the provisioner's. */
export interface ReconcileEnv extends ProvisionerEnv {
  /**
   * Stripe secret key (TEST while on testnet) — used to check subscription
   * status. Typed `string` to match `CheckoutEnv` so the combined Worker `Env`
   * can extend both; `missingReconcileEnv` still guards against an empty value.
   */
  STRIPE_SECRET_KEY: string
  /**
   * Comma-separated regions to sweep. Defaults to the provisioner's
   * REGION_ALLOWLIST (the only regions a node can be launched in).
   */
  RECONCILE_REGIONS?: string
  /** Override the stuck-provisioning threshold (ms). */
  STUCK_PROVISIONING_MS?: string
}

/** Required keys for the reconciler; returns the missing ones (fail closed). */
export function missingReconcileEnv(env: ReconcileEnv): string[] {
  const required: (keyof ReconcileEnv)[] = [
    'AWS_ACCESS_KEY_ID',
    'AWS_SECRET_ACCESS_KEY',
    'CF_DNS_API_TOKEN',
    'CF_DNS_ZONE_ID',
    'STRIPE_SECRET_KEY',
    'DB',
  ]
  return required.filter((k) => {
    const v = env[k]
    if (k === 'DB') return v == null
    return typeof v !== 'string' || v.length === 0
  })
}

/** Parse the regions list, falling back to the launch allowlist. */
export function reconcileRegions(env: ReconcileEnv): string[] {
  if (env.RECONCILE_REGIONS && env.RECONCILE_REGIONS.trim().length > 0) {
    return env.RECONCILE_REGIONS.split(',')
      .map((r) => r.trim())
      .filter((r) => r.length > 0)
  }
  return [...REGION_ALLOWLIST]
}

/**
 * Construct production `ReconcileDeps` from the env. Throws if a required
 * secret/binding is missing (call `missingReconcileEnv` first to fail closed).
 * `fetchImpl` is injectable for integration tests.
 */
export function reconcileDepsFromEnv(
  env: ReconcileEnv,
  fetchImpl: typeof fetch = fetch,
): ReconcileDeps {
  const missing = missingReconcileEnv(env)
  if (missing.length > 0) {
    throw new Error(`reconciler not configured; missing: ${missing.join(', ')}`)
  }
  const ec2 = new HttpEc2Client(
    {
      accessKeyId: env.AWS_ACCESS_KEY_ID as string,
      secretAccessKey: env.AWS_SECRET_ACCESS_KEY as string,
      sessionToken: env.AWS_SESSION_TOKEN,
    },
    fetchImpl,
  )
  const dns = new HttpDnsClient(
    env.CF_DNS_API_TOKEN as string,
    env.CF_DNS_ZONE_ID as string,
    fetchImpl,
  )
  const store = new D1NodeStore(env.DB as D1Like)
  const stripe = new HttpSubscriptionChecker(env.STRIPE_SECRET_KEY as string, fetchImpl)

  return {
    ec2,
    dns,
    store,
    stripe,
    regions: reconcileRegions(env),
    nodeDomain: env.NODE_DOMAIN || DEFAULT_NODE_DOMAIN,
    stuckProvisioningMs: env.STUCK_PROVISIONING_MS
      ? Number(env.STUCK_PROVISIONING_MS)
      : DEFAULT_STUCK_PROVISIONING_MS,
  }
}
