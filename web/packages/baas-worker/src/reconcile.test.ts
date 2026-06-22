import { describe, it, expect } from 'vitest'
import { reconcileOnce, type ReconcileDeps } from './reconcile'
import type { Ec2Instance } from './ec2'
import { rigHostname, TAG_MANAGED_RIG } from './rig-config'
import {
  FakeDns,
  FakeEc2,
  FakeStore,
  FakeSubscriptionChecker,
} from './test-fakes'

const REGION = 'us-west-2'
const NOW = 1_700_000_000_000

function makeDeps(overrides: Partial<ReconcileDeps> = {}): {
  deps: ReconcileDeps
  ec2: FakeEc2
  dns: FakeDns
  store: FakeStore
  stripe: FakeSubscriptionChecker
} {
  const ec2 = new FakeEc2()
  const dns = new FakeDns()
  const store = new FakeStore()
  const stripe = new FakeSubscriptionChecker()
  const deps: ReconcileDeps = {
    ec2,
    dns,
    store,
    stripe,
    regions: [REGION],
    now: () => NOW,
    ...overrides,
  }
  return { deps, ec2, dns, store, stripe }
}

/** Build a managed-rig EC2 instance fixture. */
function rig(partial: Partial<Ec2Instance> & { instanceId: string }): Ec2Instance {
  return {
    state: 'running',
    subscriptionTag: undefined,
    rigIdTag: undefined,
    publicIp: '1.2.3.4',
    launchTimeMs: NOW - 60_000,
    ...partial,
  }
}

describe('reconcileOnce', () => {
  it('leaves an instance whose subscription is ACTIVE strictly alone', async () => {
    const { deps, ec2, stripe } = makeDeps()
    ec2.managedByRegion.set(REGION, [
      rig({ instanceId: 'i-active', subscriptionTag: 'sub_active', rigIdTag: 'active1' }),
    ])
    stripe.active.add('sub_active')

    const report = await reconcileOnce(deps)

    expect(ec2.terminateCalls).toEqual([])
    expect(report.reaped).toBe(0)
    expect(report.items[0].disposition).toBe('active')
    expect(report.items[0].reaped).toBe(false)
  })

  it('terminates an instance whose subscription is CANCELLED/absent in Stripe', async () => {
    const { deps, ec2, dns, store } = makeDeps()
    // Seed D1 so we can assert it is marked terminated.
    await store.insertProvisioning({
      user: 'cus_1',
      stripeCustomer: 'cus_1',
      subscriptionId: 'sub_dead',
      rigId: 'dead1',
      region: REGION,
      rpcUrl: 'https://rig-dead1.testnet.botho.io/rpc',
    })
    await store.setInstanceId('sub_dead', 'i-dead')
    ec2.managedByRegion.set(REGION, [
      rig({ instanceId: 'i-dead', subscriptionTag: 'sub_dead', rigIdTag: 'dead1' }),
    ])
    // stripe.active is empty → sub_dead is NOT active.

    const report = await reconcileOnce(deps)

    expect(ec2.terminateCalls).toEqual([{ region: REGION, instanceId: 'i-dead' }])
    expect(dns.deleteCalls).toContain(rigHostname('dead1'))
    expect((await store.getBySubscription('sub_dead'))?.state).toBe('terminated')
    expect(report.reaped).toBe(1)
    expect(report.items[0].disposition).toBe('orphan_cancelled')
  })

  it('terminates a STUCK-PROVISIONING rig past the threshold (inactive sub)', async () => {
    const { deps, ec2 } = makeDeps({ stuckProvisioningMs: 30 * 60 * 1000 })
    ec2.managedByRegion.set(REGION, [
      rig({
        instanceId: 'i-stuck',
        subscriptionTag: 'sub_stuck',
        rigIdTag: 'stuck1',
        state: 'pending', // never reached running
        publicIp: undefined,
        launchTimeMs: NOW - 60 * 60 * 1000, // launched an hour ago
      }),
    ])
    // sub_stuck not active

    const report = await reconcileOnce(deps)

    expect(ec2.terminateCalls).toEqual([{ region: REGION, instanceId: 'i-stuck' }])
    expect(report.items[0].disposition).toBe('orphan_stuck_provisioning')
    expect(report.reaped).toBe(1)
  })

  it('terminates a managed rig with NO subscription tag (orphan by definition)', async () => {
    const { deps, ec2, stripe } = makeDeps()
    ec2.managedByRegion.set(REGION, [
      rig({ instanceId: 'i-notag', subscriptionTag: undefined, rigIdTag: 'notag1' }),
    ])

    const report = await reconcileOnce(deps)

    expect(ec2.terminateCalls).toEqual([{ region: REGION, instanceId: 'i-notag' }])
    expect(report.items[0].disposition).toBe('orphan_no_subscription_tag')
    // Stripe is never consulted for a tag-less rig.
    expect(stripe.calls).toEqual([])
  })

  it('SKIPS (does not reap) a rig when the Stripe lookup errors transiently', async () => {
    const { deps, ec2, stripe } = makeDeps()
    ec2.managedByRegion.set(REGION, [
      rig({ instanceId: 'i-hiccup', subscriptionTag: 'sub_hiccup', rigIdTag: 'hiccup1' }),
    ])
    stripe.throwFor.add('sub_hiccup')

    const report = await reconcileOnce(deps)

    // Never reap a (possibly paying) rig on a Stripe outage.
    expect(ec2.terminateCalls).toEqual([])
    expect(report.reaped).toBe(0)
    expect(report.skipped).toBe(1)
    expect(report.items[0].disposition).toBe('skipped_stripe_error')
  })

  it('NEVER touches non-managed-rig instances (only describeManagedRigs is consulted)', async () => {
    const { deps, ec2 } = makeDeps()
    // describeManagedRigs is the ONLY listing the sweep uses, and it is filtered
    // by botho:managed-rig=true. Seed-node-like instances are simply never
    // returned by it, so they can never be terminated. Assert the sweep only
    // ever terminates what describeManagedRigs yields.
    ec2.managedByRegion.set(REGION, [
      rig({ instanceId: 'i-managed-dead', subscriptionTag: 'sub_dead', rigIdTag: 'd1' }),
    ])
    // A "seed node" the test deliberately does NOT put in managedByRegion:
    ec2.bySubscription.set('seed', [
      rig({ instanceId: 'i-seed-node', subscriptionTag: undefined }),
    ])

    const report = await reconcileOnce(deps)

    const terminated = ec2.terminateCalls.map((c) => c.instanceId)
    expect(terminated).toEqual(['i-managed-dead'])
    expect(terminated).not.toContain('i-seed-node')
    expect(report.scanned).toBe(1)
  })

  it('ignores instances already terminating/terminated', async () => {
    const { deps, ec2 } = makeDeps()
    ec2.managedByRegion.set(REGION, [
      rig({ instanceId: 'i-gone', subscriptionTag: 'sub_x', state: 'terminated' }),
      rig({ instanceId: 'i-going', subscriptionTag: 'sub_y', state: 'shutting-down' }),
    ])

    const report = await reconcileOnce(deps)

    expect(ec2.terminateCalls).toEqual([])
    expect(report.items.every((i) => i.disposition === 'skipped_not_live')).toBe(true)
  })

  it('continues the sweep when one region cannot be listed', async () => {
    const { deps, ec2, stripe } = makeDeps({ regions: ['us-west-2', 'us-east-1'] })
    // us-west-2 listing throws; us-east-1 still gets swept.
    ec2.describeManagedRigsError = undefined
    const original = ec2.describeManagedRigs.bind(ec2)
    ec2.describeManagedRigs = async (region: string) => {
      if (region === 'us-west-2') throw new Error('throttled')
      return original(region)
    }
    ec2.managedByRegion.set('us-east-1', [
      rig({ instanceId: 'i-east-dead', subscriptionTag: 'sub_dead', rigIdTag: 'e1' }),
    ])

    const report = await reconcileOnce(deps)

    expect(ec2.terminateCalls).toEqual([
      { region: 'us-east-1', instanceId: 'i-east-dead' },
    ])
    expect(report.scanned).toBe(1)
    void stripe
  })

  it('reaps an active-then-cancelled rig only after Stripe reports inactive', async () => {
    const { deps, ec2, stripe } = makeDeps()
    ec2.managedByRegion.set(REGION, [
      rig({ instanceId: 'i-a', subscriptionTag: 'sub_a', rigIdTag: 'a1' }),
      rig({ instanceId: 'i-b', subscriptionTag: 'sub_b', rigIdTag: 'b1' }),
    ])
    stripe.active.add('sub_a') // a stays, b reaped

    const report = await reconcileOnce(deps)

    expect(ec2.terminateCalls).toEqual([{ region: REGION, instanceId: 'i-b' }])
    expect(report.reaped).toBe(1)
    const dispById = Object.fromEntries(report.items.map((i) => [i.instanceId, i.disposition]))
    expect(dispById['i-a']).toBe('active')
    expect(dispById['i-b']).toBe('orphan_cancelled')
  })
})

describe('reconcile filter discipline', () => {
  it('the sweep listing is gated on the botho:managed-rig tag', () => {
    // Documents the structural guarantee asserted in the EC2 client + sweep.
    expect(TAG_MANAGED_RIG).toBe('botho:managed-rig')
  })
})
