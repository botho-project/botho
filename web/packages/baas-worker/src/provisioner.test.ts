import { describe, it, expect } from 'vitest'
import {
  deriveNodeId,
  provisionNode,
  teardownNode,
  type ProvisionerDeps,
  type ProvisionRequest,
} from './provisioner'
import { TAG_MANAGED_NODE, TAG_SUBSCRIPTION, TAG_USER } from './node-config'
import { FakeDns, FakeEc2, FakeStore, testBase64Decode } from './test-fakes'

function makeDeps(overrides: Partial<ProvisionerDeps> = {}): {
  deps: ProvisionerDeps
  ec2: FakeEc2
  dns: FakeDns
  store: FakeStore
} {
  const ec2 = new FakeEc2()
  const dns = new FakeDns()
  const store = new FakeStore()
  const deps: ProvisionerDeps = {
    ec2,
    dns,
    store,
    binaryUrl: 'https://example.com/botho-aarch64',
    ...overrides,
  }
  return { deps, ec2, dns, store }
}

const REQ: ProvisionRequest = {
  subscriptionId: 'sub_ABC123',
  customerId: 'cus_XYZ',
  region: 'us-west-2',
}

describe('deriveNodeId', () => {
  it('strips the sub_ prefix and lowercases to a DNS-safe label', () => {
    expect(deriveNodeId('sub_ABC123')).toBe('abc123')
  })
  it('is stable for the same subscription (idempotent hostname)', () => {
    expect(deriveNodeId('sub_ABC123')).toBe(deriveNodeId('sub_ABC123'))
  })
  it('falls back to "node" for an empty tail', () => {
    expect(deriveNodeId('sub_')).toBe('node')
  })
})

describe('happy path', () => {
  it('builds a correct run-instances request (tags + user-data) + DNS + D1 row', async () => {
    const { deps, ec2, dns, store } = makeDeps()
    ec2.runPublicIp = '203.0.113.10' // IP available immediately

    const out = await provisionNode(REQ, deps)
    expect(out.ok).toBe(true)
    if (!out.ok) return

    // exactly one launch
    expect(ec2.runCalls).toHaveLength(1)
    const run = ec2.runCalls[0]
    expect(run.instanceType).toBe('t4g.medium')
    expect(run.amiId).toBe('ami-012798e88aebdba5c')
    expect(run.securityGroupId).toBe('sg-0dd3fc95ec3916a4a')
    expect(run.keyName).toBe('botho-nodes')

    // tags (#458 §3 step 1)
    expect(run.tags[TAG_MANAGED_NODE]).toBe('true')
    expect(run.tags[TAG_SUBSCRIPTION]).toBe('sub_ABC123')
    expect(run.tags[TAG_USER]).toBe('cus_XYZ')

    // user-data is base64 of a script exporting the bootstrap params
    const userData = testBase64Decode(run.userDataBase64)
    expect(userData).toContain("export NODE_ID='abc123'")
    expect(userData).toContain("export REGION='us-west-2'")
    expect(userData).toContain("export TIER='t4g.medium'")
    expect(userData).toContain('BOTHO_BINARY_URL')
    expect(userData).toContain('node-bootstrap.sh')

    // DNS A record -> public IP
    expect(dns.upsertCalls).toEqual([
      { name: 'node-abc123.testnet.botho.io', ip: '203.0.113.10' },
    ])

    // D1 row reaches running with the instance id + rpc url
    const row = store.rows.get('sub_ABC123')
    expect(row?.instanceId).toBe('i-fake1')
    expect(row?.state).toBe('running')
    expect(row?.rpcUrl).toBe('https://node-abc123.testnet.botho.io/rpc')
    expect(out.created).toBe(true)
  })

  it('stays in provisioning (no DNS) when no public IP yet', async () => {
    const { deps, dns, store } = makeDeps()
    // ec2.runPublicIp left undefined
    const out = await provisionNode(REQ, deps)
    expect(out.ok).toBe(true)
    expect(dns.upsertCalls).toHaveLength(0)
    expect(store.rows.get('sub_ABC123')?.state).toBe('provisioning')
    expect(store.rows.get('sub_ABC123')?.instanceId).toBe('i-fake1')
  })
})

describe('idempotency (#458 §3, §5)', () => {
  it('a second call with the same subscription does NOT launch a second instance', async () => {
    const { deps, ec2 } = makeDeps()
    ec2.runPublicIp = '203.0.113.10'

    const first = await provisionNode(REQ, deps)
    const second = await provisionNode(REQ, deps)

    expect(first.ok && second.ok).toBe(true)
    expect(ec2.runCalls).toHaveLength(1) // <- the critical assertion
    if (second.ok) expect(second.created).toBe(false)
  })

  it('backfills DNS + running for a row stuck in provisioning WITH an instance id (found live in #721 E2E)', async () => {
    const { deps, ec2, dns, store } = makeDeps()
    // Seed the exact live-E2E shape: launch succeeded but the RunInstances
    // response had no IP, so the row has an instance id and sits in
    // `provisioning`; a later delivery must finish DNS + running, not no-op.
    await store.insertProvisioning({
      user: 'cus_XYZ',
      stripeCustomer: 'cus_XYZ',
      subscriptionId: 'sub_ABC123',
      nodeId: 'abc123',
      region: 'us-west-2',
      rpcUrl: 'https://node-abc123.testnet.botho.io/rpc',
    })
    await store.setInstanceId('sub_ABC123', 'i-stuck')
    ec2.bySubscription.set('sub_ABC123', [
      { instanceId: 'i-stuck', state: 'running', publicIp: '203.0.113.42', subscriptionTag: 'sub_ABC123' },
    ])

    const out = await provisionNode(REQ, deps)
    expect(out.ok).toBe(true)
    expect(ec2.runCalls).toHaveLength(0) // never launches a second box
    if (out.ok) {
      expect(out.record.state).toBe('running')
      expect(out.created).toBe(false)
    }
    expect(dns.records.get('node-abc123.testnet.botho.io')?.content).toBe('203.0.113.42')
    expect(store.rows.get('sub_ABC123')?.state).toBe('running')
  })

  it('stays provisioning (no DNS) when the stuck instance still has no public IP', async () => {
    const { deps, ec2, dns, store } = makeDeps()
    await store.insertProvisioning({
      user: 'cus_XYZ',
      stripeCustomer: 'cus_XYZ',
      subscriptionId: 'sub_ABC123',
      nodeId: 'abc123',
      region: 'us-west-2',
      rpcUrl: 'https://node-abc123.testnet.botho.io/rpc',
    })
    await store.setInstanceId('sub_ABC123', 'i-stuck')
    ec2.bySubscription.set('sub_ABC123', [
      { instanceId: 'i-stuck', state: 'pending', subscriptionTag: 'sub_ABC123' },
    ])

    const out = await provisionNode(REQ, deps)
    expect(out.ok).toBe(true)
    expect(ec2.runCalls).toHaveLength(0)
    expect(dns.records.size).toBe(0)
    expect(store.rows.get('sub_ABC123')?.state).toBe('provisioning')
  })

  it('reconciles a D1 row stuck without an instance id against the EC2 tag', async () => {
    const { deps, ec2, store } = makeDeps()
    // Seed a provisioning row with no instance id (crashed mid-provision).
    await store.insertProvisioning({
      user: 'cus_XYZ',
      stripeCustomer: 'cus_XYZ',
      subscriptionId: 'sub_ABC123',
      nodeId: 'abc123',
      region: 'us-west-2',
      rpcUrl: 'https://node-abc123.testnet.botho.io/rpc',
    })
    // And an EC2 instance already exists carrying the subscription tag.
    ec2.bySubscription.set('sub_ABC123', [
      { instanceId: 'i-orphan', state: 'running', publicIp: '203.0.113.99', subscriptionTag: 'sub_ABC123' },
    ])

    const out = await provisionNode(REQ, deps)
    expect(out.ok).toBe(true)
    expect(ec2.runCalls).toHaveLength(0) // adopted the existing instance
    if (out.ok) {
      expect(out.record.instanceId).toBe('i-orphan')
      expect(out.created).toBe(false)
    }
    expect(store.rows.get('sub_ABC123')?.state).toBe('running')
  })

  it('adopts an orphaned EC2 instance when D1 has no row (post-launch D1 failure)', async () => {
    const { deps, ec2, store } = makeDeps()
    ec2.bySubscription.set('sub_ABC123', [
      { instanceId: 'i-orphan', state: 'running', publicIp: '203.0.113.50', subscriptionTag: 'sub_ABC123' },
    ])

    const out = await provisionNode(REQ, deps)
    expect(out.ok).toBe(true)
    expect(ec2.runCalls).toHaveLength(0) // never launched a duplicate
    expect(store.rows.get('sub_ABC123')?.instanceId).toBe('i-orphan')
    expect(store.rows.get('sub_ABC123')?.state).toBe('running')
  })

  it('re-provisioning a terminated subscription is allowed (state machine)', async () => {
    const { deps, ec2, store } = makeDeps()
    ec2.runPublicIp = '203.0.113.10'
    await provisionNode(REQ, deps)
    // A real teardown terminates the EC2 instance AND marks D1 terminated. The
    // explicit per-sub cap (#508 step 5b) cross-checks EC2, so a terminated D1
    // row alone must not be enough to re-launch while a live box still exists —
    // teardown must have actually removed it. Simulate that here.
    ec2.bySubscription.delete('sub_ABC123')
    await store.setState('sub_ABC123', 'terminated')

    // With both D1 terminated AND the EC2 instance gone, a fresh launch for the
    // same id is allowed (state machine).
    const again = await provisionNode(REQ, deps)
    expect(again.ok).toBe(true)
    expect(ec2.runCalls).toHaveLength(2)
  })
})

describe('safety caps (#458 §5)', () => {
  it('rejects a region not in the allowlist (no launch)', async () => {
    const { deps, ec2 } = makeDeps()
    const out = await provisionNode({ ...REQ, region: 'us-east-1' }, deps)
    expect(out.ok).toBe(false)
    if (!out.ok) expect(out.code).toBe('region_not_allowed')
    expect(ec2.runCalls).toHaveLength(0)
  })

  it('rejects an off-allowlist instance type (no launch)', async () => {
    const { deps, ec2 } = makeDeps()
    const out = await provisionNode({ ...REQ, instanceType: 'c7g.4xlarge' }, deps)
    expect(out.ok).toBe(false)
    if (!out.ok) expect(out.code).toBe('instance_type_not_allowed')
    expect(ec2.runCalls).toHaveLength(0)
  })

  it('forces t4g.medium even if the caller passes the allowed type', async () => {
    const { deps, ec2 } = makeDeps()
    ec2.runPublicIp = '203.0.113.10'
    await provisionNode({ ...REQ, instanceType: 't4g.medium' }, deps)
    expect(ec2.runCalls[0].instanceType).toBe('t4g.medium')
  })

  it('rejects when the global fleet cap is reached (no launch)', async () => {
    const { deps, ec2, store } = makeDeps({ fleetCap: 1 })
    // One active node already fills the cap.
    await store.insertProvisioning({
      user: 'cus_A',
      stripeCustomer: 'cus_A',
      subscriptionId: 'sub_OTHER',
      nodeId: 'other',
      region: 'us-west-2',
      rpcUrl: 'https://node-other.testnet.botho.io/rpc',
    })

    const out = await provisionNode(REQ, deps)
    expect(out.ok).toBe(false)
    if (!out.ok) expect(out.code).toBe('fleet_cap_reached')
    expect(ec2.runCalls).toHaveLength(0)
  })

  it('one instance per subscription: a re-trigger never adds a second (per-sub cap)', async () => {
    const { deps, ec2 } = makeDeps({ fleetCap: 100 })
    ec2.runPublicIp = '203.0.113.10'
    await provisionNode(REQ, deps)
    await provisionNode(REQ, deps)
    await provisionNode(REQ, deps)
    expect(ec2.runCalls).toHaveLength(1)
  })

  it('explicit per-sub cap (#508 step 5b): adopts a live tagged instance instead of launching a second, even when the D1 row has no instance id', async () => {
    const { deps, ec2, store } = makeDeps({ fleetCap: 100 })
    // A provisioning row exists with NO instance id...
    await store.insertProvisioning({
      user: 'cus_XYZ',
      stripeCustomer: 'cus_XYZ',
      subscriptionId: 'sub_ABC123',
      nodeId: 'abc123',
      region: 'us-west-2',
      rpcUrl: 'https://node-abc123.testnet.botho.io/rpc',
    })
    // ...and EC2 already has a LIVE instance carrying this subscription tag.
    ec2.bySubscription.set('sub_ABC123', [
      {
        instanceId: 'i-existing',
        state: 'running',
        publicIp: '203.0.113.77',
        subscriptionTag: 'sub_ABC123',
        nodeIdTag: 'abc123',
      },
    ])

    const out = await provisionNode(REQ, deps)
    expect(out.ok).toBe(true)
    // The explicit MAX_INSTANCES_PER_SUBSCRIPTION cap means NO second launch.
    expect(ec2.runCalls).toHaveLength(0)
    if (out.ok) {
      expect(out.record.instanceId).toBe('i-existing')
      expect(out.created).toBe(false)
    }
    expect(store.rows.get('sub_ABC123')?.state).toBe('running')
  })

  it('rejects a missing subscriptionId / customerId', async () => {
    const { deps } = makeDeps()
    const a = await provisionNode({ ...REQ, subscriptionId: '' }, deps)
    const b = await provisionNode({ ...REQ, customerId: '' }, deps)
    expect(a.ok).toBe(false)
    expect(b.ok).toBe(false)
  })
})

describe('launch failure handling', () => {
  it('returns launch_failed (not a throw) when EC2 rejects', async () => {
    const { deps, ec2 } = makeDeps()
    ec2.runInstance = async () => {
      throw new Error('RequestLimitExceeded')
    }
    const out = await provisionNode(REQ, deps)
    expect(out.ok).toBe(false)
    if (!out.ok) expect(out.code).toBe('launch_failed')
    // D1 row exists in provisioning so a retry can reconcile/relaunch.
    expect((deps.store as FakeStore).rows.get('sub_ABC123')?.state).toBe('provisioning')
  })
})

describe('teardownNode', () => {
  it('terminates the instance, deletes DNS, marks terminated', async () => {
    const { deps, ec2, dns, store } = makeDeps()
    ec2.runPublicIp = '203.0.113.10'
    await provisionNode(REQ, deps)

    const out = await teardownNode('sub_ABC123', deps)
    expect(out.ok).toBe(true)
    expect(ec2.terminateCalls).toEqual([{ region: 'us-west-2', instanceId: 'i-fake1' }])
    expect(dns.deleteCalls).toEqual(['node-abc123.testnet.botho.io'])
    expect(store.rows.get('sub_ABC123')?.state).toBe('terminated')
  })

  it('is a no-op for an unknown subscription', async () => {
    const { deps, ec2 } = makeDeps()
    const out = await teardownNode('sub_NOPE', deps)
    expect(out.ok).toBe(true)
    expect(ec2.terminateCalls).toHaveLength(0)
  })
})
