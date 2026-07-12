/**
 * Compute shape + safety allowlists for Botho-as-a-Service managed nodes
 * (#458 §3, §5).
 *
 * These constants are the "proven recipe" parameters from the live seed/faucet
 * nodes. They are deliberately centralized and server-authoritative so the
 * provisioner (and the SEC hardening pass, #508) have a single source of truth
 * for what a managed node is allowed to be.
 *
 * SAFE BY CONSTRUCTION (#458 §5): the allowlists here are enforced in
 * `provisioner.ts` BEFORE any AWS call, so a crafted/replayed trigger can never
 * launch an off-list region, an off-list instance type, or exceed the fleet cap.
 */

/**
 * AWS regions a managed node may be provisioned in. Kept deliberately small and
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
 * The wider region catalog the UI may offer as "coming soon". A checkout's
 * `preferredRegion` must be on this list; it is NEVER used to provision
 * (only `REGION_ALLOWLIST` regions launch — the provisioner enforces that).
 * The preference is recorded in Stripe metadata purely as demand data for
 * deciding which datacenter to open next.
 */
export const REGION_CATALOG = [
  'us-west-2',
  'us-east-1',
  'ca-central-1',
  'sa-east-1',
  'eu-central-1',
  'eu-west-2',
  'af-south-1',
  'me-south-1',
  'ap-south-1',
  'ap-southeast-1',
  'ap-northeast-1',
  'ap-southeast-2',
] as const
export type CatalogRegion = (typeof REGION_CATALOG)[number]

/** True if `region` is in the coming-soon catalog (demand-capture allowlist). */
export function isCatalogRegion(region: string): region is CatalogRegion {
  return (REGION_CATALOG as readonly string[]).includes(region)
}

/**
 * EC2 instance types a managed node may use. MVP is `t4g.medium`-only (#458 §3,
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
 * counts live managed nodes in D1 and refuses to launch beyond this. Overridable
 * per-environment via the `FLEET_CAP` Worker var; this is the conservative
 * default while on testnet.
 *
 * SEC (#508) tightens this further (IAM-conditioned + cross-checked against EC2
 * tags); here it is the in-code safety net.
 */
export const DEFAULT_FLEET_CAP = 25

/** Hard cap of running instances per active subscription (#458 §5). */
export const MAX_INSTANCES_PER_SUBSCRIPTION = 1

/** The compute shape passed to EC2 `RunInstances` for a managed node (#458 §3). */
export interface NodeComputeConfig {
  /** Ubuntu 24.04 arm64 AMI (matches the live seed/faucet nodes). */
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
export const DEFAULT_NODE_COMPUTE: NodeComputeConfig = {
  amiId: 'ami-012798e88aebdba5c',
  securityGroupId: 'sg-0dd3fc95ec3916a4a',
  keyName: 'botho-nodes',
  instanceType: DEFAULT_INSTANCE_TYPE,
}

/** Zone under which per-node hostnames live: `node-<id>.<NODE_DOMAIN>`. */
export const DEFAULT_NODE_DOMAIN = 'testnet.botho.io'

/**
 * Derive the public hostname for a node from its id. Mirrors the bootstrap
 * script's derivation (`infra/baas/node-bootstrap.sh`): accepts a bare id
 * (`abc123`) or an already-prefixed `node-abc123`.
 */
export function nodeHostname(nodeId: string, domain = DEFAULT_NODE_DOMAIN): string {
  const base = nodeId.startsWith('node-') ? nodeId : `node-${nodeId}`
  return `${base}.${domain}`
}

/** The HTTPS `/rpc` URL a user points the PWA at, given a node hostname. */
export function nodeRpcUrl(hostname: string): string {
  return `https://${hostname}/rpc`
}

/** EC2 resource tag keys for managed nodes (#458 §3 step 1, §5). */
export const TAG_MANAGED_NODE = 'botho:managed-node'
export const TAG_SUBSCRIPTION = 'botho:subscription'
export const TAG_USER = 'botho:user'
export const TAG_NODE_ID = 'botho:node-id'
