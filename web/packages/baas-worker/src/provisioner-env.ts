/**
 * Build a `ProvisionerDeps` from the Worker environment (Worker secrets + the D1
 * binding). Kept separate from `provisioner.ts` so the core flow stays pure and
 * testable with fakes, while this thin adapter wires the production clients.
 *
 * The Stripe webhook (P7.2 / #506) calls `provisionNode(req, depsFromEnv(env))`
 * from its Queue consumer / Durable Object — it never re-implements the wiring.
 *
 * SECRETS (Worker secrets, never the repo — #458 §2, §5):
 *   AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, (optional) AWS_SESSION_TOKEN
 *   CF_DNS_API_TOKEN, CF_DNS_ZONE_ID
 *   (optional) BOTHO_BINARY_URL, BOTHO_BINARY_SHA256, BOOTSTRAP_SCRIPT_URL
 * VARS (non-secret): NODE_DOMAIN, FLEET_CAP, NODE_AMI_ID, NODE_SECURITY_GROUP_ID,
 *   NODE_KEY_NAME.
 * BINDING: DB (D1).
 */

import { HttpDnsClient } from './cloudflare-dns'
import { HttpEc2Client } from './ec2'
import type { ProvisionerDeps } from './provisioner'
import {
  DEFAULT_FLEET_CAP,
  DEFAULT_NODE_COMPUTE,
  DEFAULT_NODE_DOMAIN,
  DEFAULT_INSTANCE_TYPE,
} from './node-config'
import { D1NodeStore, type D1Like } from './node-store'
import { boundFetch } from './bound-fetch'

/** Worker env keys the provisioner needs. All secrets come from Worker secrets. */
export interface ProvisionerEnv {
  // --- AWS provisioner creds (dedicated, tightly-scoped IAM user — #458 §5) --
  AWS_ACCESS_KEY_ID?: string
  AWS_SECRET_ACCESS_KEY?: string
  AWS_SESSION_TOKEN?: string

  // --- Cloudflare DNS (Zone:DNS:Edit token) ---------------------------------
  CF_DNS_API_TOKEN?: string
  CF_DNS_ZONE_ID?: string

  // --- non-secret compute overrides (default to the proven seed/faucet shape)-
  NODE_AMI_ID?: string
  NODE_SECURITY_GROUP_ID?: string
  NODE_KEY_NAME?: string
  NODE_DOMAIN?: string
  FLEET_CAP?: string

  // --- bootstrap binary plumbing (passed to user-data) ----------------------
  BOTHO_BINARY_URL?: string
  BOTHO_BINARY_SHA256?: string
  BOOTSTRAP_SCRIPT_URL?: string

  // --- D1 binding -----------------------------------------------------------
  DB?: D1Like
}

/** Required keys for the provisioner to function; returns the missing ones. */
export function missingProvisionerEnv(env: ProvisionerEnv): string[] {
  const required: (keyof ProvisionerEnv)[] = [
    'AWS_ACCESS_KEY_ID',
    'AWS_SECRET_ACCESS_KEY',
    'CF_DNS_API_TOKEN',
    'CF_DNS_ZONE_ID',
    'DB',
  ]
  return required.filter((k) => {
    const v = env[k]
    if (k === 'DB') return v == null
    return typeof v !== 'string' || v.length === 0
  })
}

/**
 * Construct production `ProvisionerDeps` from the env. Throws if a required
 * secret/binding is missing (call `missingProvisionerEnv` first to fail closed
 * with a clean error). `fetchImpl` is injectable for integration tests.
 */
export function depsFromEnv(
  env: ProvisionerEnv,
  fetchImpl: typeof fetch = boundFetch,
): ProvisionerDeps {
  const missing = missingProvisionerEnv(env)
  if (missing.length > 0) {
    throw new Error(`provisioner not configured; missing: ${missing.join(', ')}`)
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

  return {
    ec2,
    dns,
    store,
    compute: {
      amiId: env.NODE_AMI_ID || DEFAULT_NODE_COMPUTE.amiId,
      securityGroupId: env.NODE_SECURITY_GROUP_ID || DEFAULT_NODE_COMPUTE.securityGroupId,
      keyName: env.NODE_KEY_NAME || DEFAULT_NODE_COMPUTE.keyName,
      instanceType: DEFAULT_INSTANCE_TYPE,
    },
    nodeDomain: env.NODE_DOMAIN || DEFAULT_NODE_DOMAIN,
    fleetCap: env.FLEET_CAP ? Number(env.FLEET_CAP) : DEFAULT_FLEET_CAP,
    binaryUrl: env.BOTHO_BINARY_URL,
    binarySha256: env.BOTHO_BINARY_SHA256,
    bootstrapScriptUrl: env.BOOTSTRAP_SCRIPT_URL,
  }
}
