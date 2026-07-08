/**
 * Integration tests for the scheduled (cron) handler `handleScheduled`
 * (index.ts) — the SEC reconciliation sweep wiring (#508, #458 §5).
 *
 * Exercises the full handler path with injected in-memory fakes: env-gating
 * (fail closed when unconfigured) and dispatch into `reconcileOnce`. No real
 * AWS / Stripe / DNS / D1 is touched.
 */

import { describe, it, expect } from 'vitest'
import { handleScheduled, type Env } from './index'
import {
  FakeDns,
  FakeEc2,
  FakeStore,
  FakeSubscriptionChecker,
} from './test-fakes'
import type { ReconcileDeps } from './reconcile'

const ENV: Env = {
  STRIPE_SECRET_KEY: 'sk_test_dummy',
  STRIPE_PRICE_ID: 'price_test',
  CHECKOUT_SUCCESS_URL: 'https://botho.io/node/success',
  CHECKOUT_CANCEL_URL: 'https://botho.io/node',
  AWS_ACCESS_KEY_ID: 'AKIA_TEST',
  AWS_SECRET_ACCESS_KEY: 'secret',
  CF_DNS_API_TOKEN: 'cf_token',
  CF_DNS_ZONE_ID: 'zone',
  DB: {} as never,
}

function makeDeps(): {
  deps: ReconcileDeps & {
    ec2: FakeEc2
    dns: FakeDns
    store: FakeStore
    stripe: FakeSubscriptionChecker
  }
  depsFor: () => ReconcileDeps
} {
  const ec2 = new FakeEc2()
  const dns = new FakeDns()
  const store = new FakeStore()
  const stripe = new FakeSubscriptionChecker()
  const deps = { ec2, dns, store, stripe, regions: ['us-west-2'] }
  return { deps, depsFor: () => deps }
}

describe('handleScheduled (cron reconciliation)', () => {
  it('reaps an orphan and leaves an active node (full sweep wiring)', async () => {
    const { deps, depsFor } = makeDeps()
    deps.ec2.managedByRegion.set('us-west-2', [
      {
        instanceId: 'i-active',
        state: 'running',
        subscriptionTag: 'sub_ok',
        nodeIdTag: 'ok1',
        publicIp: '1.1.1.1',
      },
      {
        instanceId: 'i-orphan',
        state: 'running',
        subscriptionTag: 'sub_dead',
        nodeIdTag: 'dead1',
        publicIp: '2.2.2.2',
      },
    ])
    deps.stripe.active.add('sub_ok')

    await handleScheduled(ENV, depsFor)

    expect(deps.ec2.terminateCalls).toEqual([
      { region: 'us-west-2', instanceId: 'i-orphan' },
    ])
  })

  it('fails closed (no-op) when the reconciler env is unconfigured', async () => {
    const { deps, depsFor } = makeDeps()
    deps.ec2.managedByRegion.set('us-west-2', [
      { instanceId: 'i-x', state: 'running', subscriptionTag: 'sub_dead' },
    ])
    // Strip a required secret so missingReconcileEnv() trips.
    const badEnv: Env = { ...ENV, AWS_ACCESS_KEY_ID: '' }

    await handleScheduled(badEnv, depsFor)

    // The sweep never ran — nothing terminated.
    expect(deps.ec2.terminateCalls).toEqual([])
  })

  it('does not throw when a sweep step errors (logs + continues)', async () => {
    const { deps, depsFor } = makeDeps()
    deps.ec2.describeManagedNodesError = 'boom'
    await expect(handleScheduled(ENV, depsFor)).resolves.toBeUndefined()
  })
})
