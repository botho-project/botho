/**
 * EC2 control-plane client for the Botho-as-a-Service provisioner (#458 §3).
 *
 * The provisioner depends on the `Ec2Client` *interface* — never the concrete
 * implementation — so it can be unit-tested with an in-memory fake and NEVER
 * makes a real AWS call in a test code path (#502 test requirement).
 *
 * The real implementation (`HttpEc2Client`) signs requests with SigV4
 * (`aws-sigv4.ts`) and talks to the EC2 query API. Only the three verbs the
 * provisioner/teardown need are implemented:
 *   - RunInstances        (launch a managed rig)
 *   - DescribeInstances   (idempotency reconcile by the botho:subscription tag)
 *   - TerminateInstances  (teardown)
 *
 * Tags applied to every launch (#458 §3 step 1, §5):
 *   botho:managed-rig=true, botho:subscription=<sub>, botho:user=<user>,
 *   botho:rig-id=<rigId>.
 */

import { signAwsRequest, type AwsCredentials } from './aws-sigv4'

/** A live (non-terminated) managed rig instance as seen by EC2. */
export interface Ec2Instance {
  instanceId: string
  /** Lifecycle state: pending | running | stopping | stopped | shutting-down | terminated. */
  state: string
  /** Public IPv4, present once the instance reaches `running`. */
  publicIp?: string
  /** Value of the `botho:subscription` tag, if present. */
  subscriptionTag?: string
}

/** Parameters for launching one managed rig. */
export interface RunInstanceParams {
  region: string
  amiId: string
  instanceType: string
  securityGroupId: string
  keyName: string
  /** Base64-encoded EC2 user-data (the rig bootstrap script + env exports). */
  userDataBase64: string
  /** Resource tags applied at launch (key -> value). */
  tags: Record<string, string>
}

/**
 * Injectable EC2 surface. The provisioner is written against this so tests pass
 * a fake. Implementations MUST be idempotent-friendly: `describeBySubscription`
 * is the reconcile step that defeats duplicate launches on retry (#458 §3, §5).
 */
export interface Ec2Client {
  /** Launch one instance; returns its (initial) instance record. */
  runInstance(params: RunInstanceParams): Promise<Ec2Instance>
  /**
   * List non-terminated instances tagged `botho:subscription=<subscriptionId>`
   * in `region`. Used to reconcile idempotency against AWS itself, not just D1.
   */
  describeBySubscription(region: string, subscriptionId: string): Promise<Ec2Instance[]>
  /** Terminate an instance (teardown). Safe to call if already terminated. */
  terminateInstance(region: string, instanceId: string): Promise<void>
}

const EC2_API_VERSION = '2016-11-15'

function endpointFor(region: string): string {
  return `https://ec2.${region}.amazonaws.com/`
}

/**
 * States we treat as "this subscription already has an instance" for idempotency.
 * Anything not terminated/terminating counts as live so a replay never launches
 * a second box.
 */
const LIVE_STATES = new Set(['pending', 'running', 'stopping', 'stopped', 'rebooting'])

export function isLiveInstanceState(state: string): boolean {
  return LIVE_STATES.has(state)
}

/** Build the form-encoded RunInstances query body (pure; unit-tested). */
export function buildRunInstancesBody(params: RunInstanceParams): URLSearchParams {
  const body = new URLSearchParams()
  body.set('Action', 'RunInstances')
  body.set('Version', EC2_API_VERSION)
  body.set('ImageId', params.amiId)
  body.set('InstanceType', params.instanceType)
  body.set('KeyName', params.keyName)
  body.set('MinCount', '1')
  body.set('MaxCount', '1')
  body.set('SecurityGroupId.1', params.securityGroupId)
  body.set('UserData', params.userDataBase64)

  // Tag the instance at creation time so reconciliation can find it even if D1
  // write or DNS failed mid-provision (#458 §3 step 1, §5).
  body.set('TagSpecification.1.ResourceType', 'instance')
  let i = 1
  for (const [key, value] of Object.entries(params.tags)) {
    body.set(`TagSpecification.1.Tag.${i}.Key`, key)
    body.set(`TagSpecification.1.Tag.${i}.Value`, value)
    i++
  }
  return body
}

/** Extract the first capture of `re` from `xml`, or undefined. */
function pick(xml: string, re: RegExp): string | undefined {
  const m = re.exec(xml)
  return m ? m[1] : undefined
}

/**
 * Parse the EC2 RunInstances XML response for the launched instance id + state.
 * Minimal, dependency-free extraction (the Worker runtime has no DOMParser for
 * XML). Exported for unit testing against captured fixtures.
 */
export function parseRunInstancesResponse(xml: string): Ec2Instance {
  const instanceId = pick(xml, /<instanceId>([^<]+)<\/instanceId>/)
  if (!instanceId) {
    throw new Ec2Error('RunInstances response missing instanceId', 200, xml)
  }
  const state = pick(xml, /<instanceState>[\s\S]*?<name>([^<]+)<\/name>/) ?? 'pending'
  const publicIp = pick(xml, /<ipAddress>([^<]+)<\/ipAddress>/)
  return { instanceId, state, publicIp }
}

/**
 * Parse a DescribeInstances XML response into the instances it lists, capturing
 * each instance's `botho:subscription` tag for the idempotency reconcile.
 */
export function parseDescribeInstancesResponse(xml: string): Ec2Instance[] {
  const out: Ec2Instance[] = []
  // Each instance contains exactly one <instanceId>. Slice the document from one
  // <instanceId> to the next so a block captures that instance's nested fields
  // (state, ipAddress, tagSet) without the brittle <item>-splitting that nested
  // tagSet <item> elements would break.
  const idRe = /<instanceId>([^<]+)<\/instanceId>/g
  const starts: { id: string; index: number }[] = []
  let m: RegExpExecArray | null
  while ((m = idRe.exec(xml)) !== null) {
    starts.push({ id: m[1], index: m.index })
  }
  for (let i = 0; i < starts.length; i++) {
    const begin = starts[i].index
    const end = i + 1 < starts.length ? starts[i + 1].index : xml.length
    const block = xml.slice(begin, end)
    const state = pick(block, /<instanceState>[\s\S]*?<name>([^<]+)<\/name>/) ?? 'unknown'
    const publicIp = pick(block, /<ipAddress>([^<]+)<\/ipAddress>/)
    const subscriptionTag = pick(
      block,
      /<key>botho:subscription<\/key>\s*<value>([^<]+)<\/value>/,
    )
    out.push({ instanceId: starts[i].id, state, publicIp, subscriptionTag })
  }
  return out
}

/** Error from the EC2 API (non-2xx or unparsable response). */
export class Ec2Error extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly body?: string,
  ) {
    super(message)
    this.name = 'Ec2Error'
  }
}

/**
 * Real EC2 client. Signs each request with SigV4 and POSTs to the regional
 * endpoint. `fetchImpl` is injectable (defaults to global fetch); tests use the
 * fake `Ec2Client` instead, so this code path never runs under test.
 */
export class HttpEc2Client implements Ec2Client {
  constructor(
    private readonly credentials: AwsCredentials,
    private readonly fetchImpl: typeof fetch = fetch,
  ) {}

  private async send(region: string, body: URLSearchParams): Promise<string> {
    const signed = await signAwsRequest({
      endpoint: endpointFor(region),
      region,
      service: 'ec2',
      body: body.toString(),
      credentials: this.credentials,
    })
    const resp = await this.fetchImpl(signed.url, {
      method: 'POST',
      headers: signed.headers,
      body: signed.body,
    })
    const text = await resp.text()
    if (!resp.ok) {
      const code = pick(text, /<Code>([^<]+)<\/Code>/) ?? `HTTP ${resp.status}`
      const msg = pick(text, /<Message>([^<]+)<\/Message>/) ?? 'EC2 request failed'
      throw new Ec2Error(`${code}: ${msg}`, resp.status, text)
    }
    return text
  }

  async runInstance(params: RunInstanceParams): Promise<Ec2Instance> {
    const xml = await this.send(params.region, buildRunInstancesBody(params))
    return parseRunInstancesResponse(xml)
  }

  async describeBySubscription(
    region: string,
    subscriptionId: string,
  ): Promise<Ec2Instance[]> {
    const body = new URLSearchParams()
    body.set('Action', 'DescribeInstances')
    body.set('Version', EC2_API_VERSION)
    body.set('Filter.1.Name', 'tag:botho:subscription')
    body.set('Filter.1.Value.1', subscriptionId)
    const xml = await this.send(region, body)
    return parseDescribeInstancesResponse(xml)
  }

  async terminateInstance(region: string, instanceId: string): Promise<void> {
    const body = new URLSearchParams()
    body.set('Action', 'TerminateInstances')
    body.set('Version', EC2_API_VERSION)
    body.set('InstanceId.1', instanceId)
    await this.send(region, body)
  }
}
