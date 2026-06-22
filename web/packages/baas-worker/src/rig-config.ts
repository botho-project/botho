/**
 * Compute shape + safety allowlists for Botho-as-a-Service managed rigs
 * (#458 §3, §5).
 *
 * These constants are the "proven recipe" parameters from the live seed/faucet
 * rigs. They are deliberately centralized and server-authoritative so the
 * provisioner (and the SEC hardening pass, #508) have a single source of truth
 * for what a managed rig is allowed to be.
 *
 * SAFE BY CONSTRUCTION (#458 §5): the allowlists here are enforced in
 * `provisioner.ts` BEFORE any AWS call, so a crafted/replayed trigger can never
 * launch an off-list region, an off-list instance type, or exceed the fleet cap.
 */

/**
 * AWS regions a managed rig may be provisioned in. Kept deliberately small and
 * server-authoritative (#458 §5: "Region allowlist — start: us-west-2 only,
 * expand deliberately"). Fail closed on anything else.
 */
export const REGION_ALLOWLIST = ['us-west-2'] as const
export type AllowedRegion = (typeof REGION_ALLOWLIST)[number]

/** True if `region` is in the server-side region allowlist. */
export function isAllowedRegion(region: string): region is AllowedRegion {
  return (REGION_ALLOWLIST as readonly string[]).includes(region)
}

/**
 * EC2 instance types a managed rig may use. MVP is `t4g.medium`-only (#458 §3,
 * §5): RandomX's ~2GB dataset needs the RAM and this is the proven shape. Any
 * other type is rejected (the provisioner also *forces* this type rather than
 * trusting caller input — defense in depth).
 */
export const INSTANCE_TYPE_ALLOWLIST = ['t4g.medium'] as const
export type AllowedInstanceType = (typeof INSTANCE_TYPE_ALLOWLIST)[number]

/** The single MVP instance type, forced for every launch. */
export const DEFAULT_INSTANCE_TYPE: AllowedInstanceType = 't4g.medium'

/** True if `instanceType` is in the server-side instance-type allowlist. */
export function isAllowedInstanceType(
  instanceType: string,
): instanceType is AllowedInstanceType {
  return (INSTANCE_TYPE_ALLOWLIST as readonly string[]).includes(instanceType)
}

/**
 * Global fleet cap — a circuit breaker against cost-runaway / abuse (#458 §5:
 * "Global fleet cap (e.g. N instances) as a circuit breaker"). The provisioner
 * counts live managed rigs in D1 and refuses to launch beyond this. Overridable
 * per-environment via the `FLEET_CAP` Worker var; this is the conservative
 * default while on testnet.
 *
 * SEC (#508) tightens this further (IAM-conditioned + cross-checked against EC2
 * tags); here it is the in-code safety net.
 */
export const DEFAULT_FLEET_CAP = 25

/** Hard cap of running instances per active subscription (#458 §5). */
export const MAX_INSTANCES_PER_SUBSCRIPTION = 1

/** The compute shape passed to EC2 `RunInstances` for a managed rig (#458 §3). */
export interface RigComputeConfig {
  /** Ubuntu 24.04 arm64 AMI (matches the live seed/faucet rigs). */
  amiId: string
  /** Security group id. */
  securityGroupId: string
  /** EC2 key-pair name (SSH for break-glass only; bootstrap is user-data). */
  keyName: string
  /** Instance type — always `t4g.medium` for the MVP. */
  instanceType: AllowedInstanceType
}

/**
 * The proven seed/faucet compute shape (#458 §1, §3). Non-secret identifiers,
 * safe to keep in the repo; overridable via Worker vars for other accounts.
 */
export const DEFAULT_RIG_COMPUTE: RigComputeConfig = {
  amiId: 'ami-012798e88aebdba5c',
  securityGroupId: 'sg-0dd3fc95ec3916a4a',
  keyName: 'botho-nodes',
  instanceType: DEFAULT_INSTANCE_TYPE,
}

/** Zone under which per-rig hostnames live: `rig-<id>.<RIG_DOMAIN>`. */
export const DEFAULT_RIG_DOMAIN = 'testnet.botho.io'

/**
 * Derive the public hostname for a rig from its id. Mirrors the bootstrap
 * script's derivation (`infra/baas/rig-bootstrap.sh`): accepts a bare id
 * (`abc123`) or an already-prefixed `rig-abc123`.
 */
export function rigHostname(rigId: string, domain = DEFAULT_RIG_DOMAIN): string {
  const base = rigId.startsWith('rig-') ? rigId : `rig-${rigId}`
  return `${base}.${domain}`
}

/** The HTTPS `/rpc` URL a user points the PWA at, given a rig hostname. */
export function rigRpcUrl(hostname: string): string {
  return `https://${hostname}/rpc`
}

/** EC2 resource tag keys for managed rigs (#458 §3 step 1, §5). */
export const TAG_MANAGED_RIG = 'botho:managed-rig'
export const TAG_SUBSCRIPTION = 'botho:subscription'
export const TAG_USER = 'botho:user'
export const TAG_RIG_ID = 'botho:rig-id'
