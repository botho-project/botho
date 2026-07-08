/**
 * Botho-as-a-Service provisioner core (#502, #458 §3 + §5).
 *
 * Given `{subscriptionId, customerId, region, ...}` this:
 *   1. Enforces the safety caps (region/instance-type allowlist, per-sub cap,
 *      global fleet cap) — FAIL CLOSED before any AWS call.
 *   2. Checks idempotency: D1 first, then reconciles against the EC2
 *      `botho:subscription` tag, so a replayed trigger NEVER launches a second
 *      instance.
 *   3. Launches EC2 `RunInstances` (tagged) with the node-bootstrap user-data.
 *   4. Creates the Cloudflare DNS A record `node-<id>.<domain> -> public IP`.
 *   5. Writes / advances the D1 mapping (provisioning -> running).
 *
 * It is exposed as a plain async function/handler that the Stripe webhook (P7.2
 * / #506) will call from a Queue consumer or Durable Object. The webhook itself
 * (signature verify) is OUT OF SCOPE here (#458 §8).
 *
 * SAFE BY CONSTRUCTION (#458 §5): the caps + idempotency are enforced in *this*
 * code, not merely by IAM. The dedicated SEC hardening pass (#508) tightens the
 * IAM policy and adds the orphan-reconciliation cron on top.
 *
 * Dependency injection: EC2 / DNS / D1 are passed as interfaces, so every test
 * uses in-memory fakes and NO real network call happens in a test code path.
 */

import { buildUserData } from './user-data'
import type { DnsClient } from './cloudflare-dns'
import type { Ec2Client } from './ec2'
import { isLiveInstanceState } from './ec2'
import {
  DEFAULT_FLEET_CAP,
  DEFAULT_NODE_COMPUTE,
  DEFAULT_NODE_DOMAIN,
  isAllowedInstanceType,
  isAllowedRegion,
  MAX_INSTANCES_PER_SUBSCRIPTION,
  nodeHostname,
  nodeRpcUrl,
  TAG_MANAGED_NODE,
  TAG_NODE_ID,
  TAG_SUBSCRIPTION,
  TAG_USER,
  type NodeComputeConfig,
} from './node-config'
import {
  DuplicateSubscriptionError,
  type NodeRecord,
  type NodeStore,
} from './node-store'

/** Input the (future) webhook hands to the provisioner. */
export interface ProvisionRequest {
  /** Stripe subscription id — the idempotency key (#458 §3, §5). */
  subscriptionId: string
  /** Stripe customer id == our user identity (#458 §4). */
  customerId: string
  /** Desired AWS region (re-validated against the allowlist here). */
  region: string
  /**
   * Optional caller-supplied instance type. Ignored unless allowlisted; the
   * provisioner forces `t4g.medium` regardless (defense in depth, #458 §5).
   */
  instanceType?: string
  /** Optional explicit node id; otherwise derived from the subscription id. */
  nodeId?: string
}

/** Everything the provisioner needs, injected for testability. */
export interface ProvisionerDeps {
  ec2: Ec2Client
  dns: DnsClient
  store: NodeStore
  /** Compute shape (AMI/SG/key/type). Defaults to the proven seed/faucet shape. */
  compute?: NodeComputeConfig
  /** Zone for node hostnames. Default: testnet.botho.io. */
  nodeDomain?: string
  /** Global fleet cap circuit-breaker. Default: DEFAULT_FLEET_CAP. */
  fleetCap?: number
  /** Bootstrap binary download URL passed to user-data (BOTHO_BINARY_URL). */
  binaryUrl?: string
  /** Optional sha256 to pin the binary (BOTHO_BINARY_SHA256). */
  binarySha256?: string
  /** URL of node-bootstrap.sh to fetch at boot (BOOTSTRAP_SCRIPT_URL). */
  bootstrapScriptUrl?: string
}

export type ProvisionOutcome =
  | { ok: true; record: NodeRecord; created: boolean }
  | { ok: false; code: ProvisionErrorCode; error: string }

export type ProvisionErrorCode =
  | 'region_not_allowed'
  | 'instance_type_not_allowed'
  | 'fleet_cap_reached'
  // `per_subscription_cap` is retained for callers that prefer to treat a
  // would-be-second launch as an error rather than an adopt. The provisioner's
  // own policy is to ADOPT the existing instance (step 5b) — idempotent and
  // safer than failing — so it does not currently return this code, but the
  // explicit `MAX_INSTANCES_PER_SUBSCRIPTION` cap that backs it is now enforced.
  | 'per_subscription_cap'
  | 'invalid_request'
  | 'launch_failed'

/** Derive a short node id from a subscription id when none is supplied. */
export function deriveNodeId(subscriptionId: string): string {
  // Stripe sub ids look like `sub_1Abc...`. Strip the prefix + lowercase to a
  // DNS-safe label. Keep it short and stable so retries derive the same id.
  const tail = subscriptionId.replace(/^sub_/, '').toLowerCase()
  const safe = tail.replace(/[^a-z0-9]/g, '').slice(0, 20)
  return safe.length > 0 ? safe : 'node'
}

/**
 * Provision (or reconcile an existing) managed node for a subscription.
 * Idempotent and fail-closed. Never throws on a cap/validation failure — those
 * return a structured `{ ok: false }` so the caller (webhook/queue) can decide
 * whether to ACK or dead-letter.
 */
export async function provisionNode(
  req: ProvisionRequest,
  deps: ProvisionerDeps,
): Promise<ProvisionOutcome> {
  const compute = deps.compute ?? DEFAULT_NODE_COMPUTE
  const nodeDomain = deps.nodeDomain ?? DEFAULT_NODE_DOMAIN
  const fleetCap = deps.fleetCap ?? DEFAULT_FLEET_CAP

  // --- 0. Basic input validation -------------------------------------------
  if (!req.subscriptionId || typeof req.subscriptionId !== 'string') {
    return { ok: false, code: 'invalid_request', error: 'subscriptionId is required' }
  }
  if (!req.customerId || typeof req.customerId !== 'string') {
    return { ok: false, code: 'invalid_request', error: 'customerId is required' }
  }

  // --- 1. Safety caps: region + instance-type allowlists (#458 §5) ----------
  if (!isAllowedRegion(req.region)) {
    return {
      ok: false,
      code: 'region_not_allowed',
      error: `region "${req.region}" is not in the allowlist`,
    }
  }
  // Force the MVP instance type. If a caller passed an off-list type explicitly,
  // reject it rather than silently overriding (surfaces misconfiguration).
  if (req.instanceType !== undefined && !isAllowedInstanceType(req.instanceType)) {
    return {
      ok: false,
      code: 'instance_type_not_allowed',
      error: `instance type "${req.instanceType}" is not in the allowlist`,
    }
  }
  const instanceType = compute.instanceType // always t4g.medium for the MVP

  // --- 2. Idempotency: D1 first (#458 §3, §5) -------------------------------
  const existing = await deps.store.getBySubscription(req.subscriptionId)
  if (existing && existing.state !== 'terminated') {
    // A row already exists and is live. If it never got an instance id (crashed
    // mid-provision), try to recover the instance from the EC2 tag before
    // deciding to launch — but DO NOT launch a second box.
    if (existing.instanceId) {
      return { ok: true, record: existing, created: false }
    }
    const reconciled = await reconcileFromEc2(req, deps, existing)
    if (reconciled) return { ok: true, record: reconciled, created: false }
    // No D1 instance id AND no EC2 instance found: fall through to launch using
    // the existing row (we'll attach the instance id to it).
  }

  // --- 3. Reconcile against EC2 by tag even with no D1 row (#458 §3) --------
  // Defends against a D1 write that failed AFTER a successful launch.
  const tagged = await deps.ec2.describeBySubscription(req.region, req.subscriptionId)
  const liveTagged = tagged.find((i) => isLiveInstanceState(i.state))
  if (liveTagged && !existing) {
    // An orphaned instance exists but D1 has no row. Adopt it: write the row
    // pointing at the found instance instead of launching a new one.
    const nodeId = req.nodeId ?? deriveNodeId(req.subscriptionId)
    const hostname = nodeHostname(nodeId, nodeDomain)
    const rpcUrl = nodeRpcUrl(hostname)
    const record = await insertRowSafely(deps.store, {
      user: req.customerId,
      stripeCustomer: req.customerId,
      subscriptionId: req.subscriptionId,
      nodeId,
      region: req.region,
      rpcUrl,
    })
    await deps.store.setInstanceId(req.subscriptionId, liveTagged.instanceId)
    if (liveTagged.publicIp) {
      await deps.dns.upsertARecord(hostname, liveTagged.publicIp)
      await deps.store.setState(req.subscriptionId, 'running')
    }
    return {
      ok: true,
      record: { ...record, instanceId: liveTagged.instanceId },
      created: false,
    }
  }

  // --- 4. Global fleet cap circuit breaker (#458 §5) ------------------------
  const active = await deps.store.countActive()
  if (!existing && active >= fleetCap) {
    return {
      ok: false,
      code: 'fleet_cap_reached',
      error: `global fleet cap of ${fleetCap} reached`,
    }
  }

  // --- 5. Insert / reuse the D1 provisioning row ----------------------------
  const nodeId = existing?.nodeId ?? req.nodeId ?? deriveNodeId(req.subscriptionId)
  const hostname = nodeHostname(nodeId, nodeDomain)
  const rpcUrl = nodeRpcUrl(hostname)

  let record = existing
  if (!record) {
    record = await insertRowSafely(deps.store, {
      user: req.customerId,
      stripeCustomer: req.customerId,
      subscriptionId: req.subscriptionId,
      nodeId,
      region: req.region,
      rpcUrl,
    })
  }

  // --- 5b. EXPLICIT per-subscription cap (#508, #458 §5) --------------------
  // The 1-per-subscription guarantee is *structural* (subscription_id is UNIQUE
  // in D1, and steps 2-3 adopt any pre-existing/orphaned instance rather than
  // launching a second). SEC adds the cap as an EXPLICIT, counted defense in
  // depth: immediately before RunInstances we re-count live instances carrying
  // this `botho:subscription` tag in EC2 and refuse to launch if the cap is
  // already met. This wires in `MAX_INSTANCES_PER_SUBSCRIPTION` (previously a
  // dead symbol the #526 Judge flagged) and closes the narrow window where a
  // concurrent in-flight launch could otherwise double-provision.
  const liveForSub = (
    await deps.ec2.describeBySubscription(req.region, req.subscriptionId)
  ).filter((i) => isLiveInstanceState(i.state))
  if (liveForSub.length >= MAX_INSTANCES_PER_SUBSCRIPTION) {
    // Adopt the existing instance instead of launching another.
    const adopt = liveForSub[0]
    await deps.store.setInstanceId(req.subscriptionId, adopt.instanceId)
    let state = record.state
    if (adopt.publicIp) {
      await deps.dns.upsertARecord(hostname, adopt.publicIp)
      await deps.store.setState(req.subscriptionId, 'running')
      state = 'running'
    }
    return {
      ok: true,
      record: { ...record, instanceId: adopt.instanceId, state },
      created: false,
    }
  }

  // --- 6. Launch the instance (tagged) with node-bootstrap user-data ---------
  const userDataBase64 = buildUserData(
    {
      nodeId,
      region: req.region,
      tier: instanceType,
      nodeDomain,
      binaryUrl: deps.binaryUrl,
      binarySha256: deps.binarySha256,
      bootstrapScriptUrl: deps.bootstrapScriptUrl,
    },
    base64Encode,
  )

  let instanceId: string
  let publicIp: string | undefined
  try {
    const launched = await deps.ec2.runInstance({
      region: req.region,
      amiId: compute.amiId,
      instanceType,
      securityGroupId: compute.securityGroupId,
      keyName: compute.keyName,
      userDataBase64,
      tags: {
        [TAG_MANAGED_NODE]: 'true',
        [TAG_SUBSCRIPTION]: req.subscriptionId,
        [TAG_USER]: req.customerId,
        [TAG_NODE_ID]: nodeId,
      },
    })
    instanceId = launched.instanceId
    publicIp = launched.publicIp
  } catch (err) {
    return {
      ok: false,
      code: 'launch_failed',
      error: `RunInstances failed: ${String(err)}`,
    }
  }

  await deps.store.setInstanceId(req.subscriptionId, instanceId)

  // --- 7. DNS + state transition --------------------------------------------
  // The IP is often not yet assigned in the RunInstances response; the caller
  // (queue/DO) re-runs or P6.3 backfills DNS when the IP is known. If we DO have
  // it, create the record and mark running now.
  if (publicIp) {
    await deps.dns.upsertARecord(hostname, publicIp)
    await deps.store.setState(req.subscriptionId, 'running')
  }

  const finalRecord: NodeRecord = {
    ...record,
    instanceId,
    state: publicIp ? 'running' : 'provisioning',
  }
  return { ok: true, record: finalRecord, created: true }
}

/**
 * Teardown: terminate the instance, delete the DNS record, mark D1 `terminated`.
 * Callable by SEC (#508) / P7.2 even before the trigger wiring lands (#458 §3).
 * Idempotent and best-effort — missing instance/record is not an error.
 */
export async function teardownNode(
  subscriptionId: string,
  deps: ProvisionerDeps,
): Promise<{ ok: boolean; error?: string }> {
  const nodeDomain = deps.nodeDomain ?? DEFAULT_NODE_DOMAIN
  const record = await deps.store.getBySubscription(subscriptionId)
  if (!record) return { ok: true }

  try {
    if (record.instanceId) {
      await deps.ec2.terminateInstance(record.region, record.instanceId)
    }
    await deps.dns.deleteARecord(nodeHostname(record.nodeId, nodeDomain))
    await deps.store.setState(subscriptionId, 'terminated')
    return { ok: true }
  } catch (err) {
    return { ok: false, error: String(err) }
  }
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

/** Insert a row, tolerating a concurrent insert (treat dup as "already there"). */
async function insertRowSafely(
  store: NodeStore,
  rec: Parameters<NodeStore['insertProvisioning']>[0],
): Promise<NodeRecord> {
  try {
    return await store.insertProvisioning(rec)
  } catch (err) {
    if (err instanceof DuplicateSubscriptionError) {
      const found = await store.getBySubscription(rec.subscriptionId)
      if (found) return found
    }
    throw err
  }
}

/**
 * Recover an in-flight provision whose D1 row has no instance id by looking up
 * the EC2 `botho:subscription` tag. Returns the updated record if an instance is
 * found, else undefined (caller proceeds to launch).
 */
async function reconcileFromEc2(
  req: ProvisionRequest,
  deps: ProvisionerDeps,
  existing: NodeRecord,
): Promise<NodeRecord | undefined> {
  const nodeDomain = deps.nodeDomain ?? DEFAULT_NODE_DOMAIN
  const tagged = await deps.ec2.describeBySubscription(req.region, req.subscriptionId)
  const live = tagged.find((i) => isLiveInstanceState(i.state))
  if (!live) return undefined

  await deps.store.setInstanceId(req.subscriptionId, live.instanceId)
  let state = existing.state
  if (live.publicIp) {
    await deps.dns.upsertARecord(nodeHostname(existing.nodeId, nodeDomain), live.publicIp)
    await deps.store.setState(req.subscriptionId, 'running')
    state = 'running'
  }
  return { ...existing, instanceId: live.instanceId, state }
}

/** Base64 encode a UTF-8 string (Worker-safe; no Node Buffer). */
export function base64Encode(s: string): string {
  // btoa operates on Latin-1; encode UTF-8 bytes first to be safe.
  const bytes = new TextEncoder().encode(s)
  let binary = ''
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i])
  return btoa(binary)
}
