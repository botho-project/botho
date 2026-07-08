import { describe, it, expect } from 'vitest'
import {
  actionForEventType,
  extractProvisionRequest,
  extractSubscriptionId,
  handleStripeEvent,
  hmacSha256Hex,
  parseSignatureHeader,
  PROVISION_EVENTS,
  TEARDOWN_EVENTS,
  timingSafeEqualHex,
  verifyStripeSignature,
} from './webhook'
import { FakeDns, FakeEc2, FakeStore } from './test-fakes'
import type { ProvisionerDeps } from './provisioner'
import { DEFAULT_NODE_COMPUTE, DEFAULT_INSTANCE_TYPE } from './node-config'

const SECRET = 'whsec_test_secret_value'

function fakeDeps(): ProvisionerDeps & { ec2: FakeEc2; dns: FakeDns; store: FakeStore } {
  const ec2 = new FakeEc2()
  const dns = new FakeDns()
  const store = new FakeStore()
  return {
    ec2,
    dns,
    store,
    compute: { ...DEFAULT_NODE_COMPUTE, instanceType: DEFAULT_INSTANCE_TYPE },
    nodeDomain: 'testnet.botho.io',
    fleetCap: 25,
  }
}

/** Build a valid `Stripe-Signature` header for a body at a given timestamp. */
async function signBody(body: string, ts: number, secret = SECRET): Promise<string> {
  const v1 = await hmacSha256Hex(secret, `${ts}.${body}`)
  return `t=${ts},v1=${v1}`
}

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

describe('parseSignatureHeader', () => {
  it('parses timestamp and multiple v1 candidates', () => {
    const parsed = parseSignatureHeader('t=123,v1=aaa,v1=bbb,v0=ignored')
    expect(parsed.timestamp).toBe(123)
    expect(parsed.v1).toEqual(['aaa', 'bbb'])
  })

  it('returns no timestamp when absent', () => {
    const parsed = parseSignatureHeader('v1=aaa')
    expect(parsed.timestamp).toBeUndefined()
    expect(parsed.v1).toEqual(['aaa'])
  })
})

describe('timingSafeEqualHex', () => {
  it('is true for identical strings', () => {
    expect(timingSafeEqualHex('abcd', 'abcd')).toBe(true)
  })
  it('is false for different strings of equal length', () => {
    expect(timingSafeEqualHex('abcd', 'abce')).toBe(false)
  })
  it('is false for length mismatch', () => {
    expect(timingSafeEqualHex('abcd', 'abcde')).toBe(false)
  })
})

describe('verifyStripeSignature', () => {
  it('accepts a valid signature over the raw body', async () => {
    const body = JSON.stringify({ id: 'evt_1', type: 'invoice.paid' })
    const ts = nowSec()
    const header = await signBody(body, ts)
    const res = await verifyStripeSignature(body, header, SECRET)
    expect(res.ok).toBe(true)
  })

  it('rejects a missing signature header', async () => {
    const res = await verifyStripeSignature('{}', null, SECRET)
    expect(res.ok).toBe(false)
  })

  it('rejects a tampered body (HMAC mismatch)', async () => {
    const body = JSON.stringify({ id: 'evt_1', type: 'invoice.paid' })
    const header = await signBody(body, nowSec())
    // Body changed after signing => signature no longer matches.
    const res = await verifyStripeSignature(body + ' ', header, SECRET)
    expect(res.ok).toBe(false)
  })

  it('rejects a signature made with the wrong secret', async () => {
    const body = '{"type":"invoice.paid"}'
    const header = await signBody(body, nowSec(), 'whsec_wrong')
    const res = await verifyStripeSignature(body, header, SECRET)
    expect(res.ok).toBe(false)
  })

  it('rejects a stale timestamp outside the tolerance window (replay)', async () => {
    const body = '{"type":"invoice.paid"}'
    const old = nowSec() - 10_000
    const header = await signBody(body, old)
    const res = await verifyStripeSignature(body, header, SECRET)
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.reason).toMatch(/tolerance/)
  })

  it('rejects a header with no v1 signature', async () => {
    const res = await verifyStripeSignature('{}', `t=${nowSec()}`, SECRET)
    expect(res.ok).toBe(false)
  })

  it('rejects when the secret is empty', async () => {
    const res = await verifyStripeSignature('{}', `t=${nowSec()},v1=deadbeef`, '')
    expect(res.ok).toBe(false)
  })
})

describe('actionForEventType', () => {
  it('maps provision events', () => {
    for (const t of PROVISION_EVENTS) expect(actionForEventType(t)).toBe('provision')
  })
  it('maps teardown events', () => {
    for (const t of TEARDOWN_EVENTS) expect(actionForEventType(t)).toBe('teardown')
  })
  it('ignores unknown events', () => {
    expect(actionForEventType('customer.created')).toBe('ignore')
    expect(actionForEventType('')).toBe('ignore')
  })
})

describe('extractProvisionRequest', () => {
  it('reads subscription/customer/region from checkout.session.completed', () => {
    const req = extractProvisionRequest({
      type: 'checkout.session.completed',
      data: {
        object: {
          subscription: 'sub_abc',
          customer: 'cus_xyz',
          metadata: { region: 'us-west-2' },
        },
      },
    } as never)
    expect(req).toEqual({ subscriptionId: 'sub_abc', customerId: 'cus_xyz', region: 'us-west-2' })
  })

  it('reads region from invoice subscription_details metadata', () => {
    const req = extractProvisionRequest({
      type: 'invoice.paid',
      data: {
        object: {
          subscription: 'sub_abc',
          customer: 'cus_xyz',
          subscription_details: { metadata: { region: 'us-west-2' } },
        },
      },
    } as never)
    expect(req?.region).toBe('us-west-2')
  })

  it('returns undefined when required fields are missing', () => {
    expect(
      extractProvisionRequest({
        type: 'checkout.session.completed',
        data: { object: { customer: 'cus_xyz', metadata: { region: 'us-west-2' } } },
      } as never),
    ).toBeUndefined()
  })

  it('coerces an expanded subscription object to its id', () => {
    const req = extractProvisionRequest({
      type: 'invoice.paid',
      data: {
        object: {
          subscription: { id: 'sub_expanded' },
          customer: { id: 'cus_expanded' },
          metadata: { region: 'us-west-2' },
        },
      },
    } as never)
    expect(req?.subscriptionId).toBe('sub_expanded')
    expect(req?.customerId).toBe('cus_expanded')
  })
})

describe('extractSubscriptionId', () => {
  it('uses the object id for customer.subscription.deleted', () => {
    const id = extractSubscriptionId({
      type: 'customer.subscription.deleted',
      data: { object: { id: 'sub_deleted' } },
    } as never)
    expect(id).toBe('sub_deleted')
  })

  it('uses the subscription field for invoice.payment_failed', () => {
    const id = extractSubscriptionId({
      type: 'invoice.payment_failed',
      data: { object: { subscription: 'sub_failing' } },
    } as never)
    expect(id).toBe('sub_failing')
  })
})

describe('handleStripeEvent', () => {
  it('provisions on checkout.session.completed', async () => {
    const deps = fakeDeps()
    const handled = await handleStripeEvent(
      {
        type: 'checkout.session.completed',
        data: {
          object: {
            subscription: 'sub_abc',
            customer: 'cus_xyz',
            metadata: { region: 'us-west-2' },
          },
        },
      } as never,
      deps,
    )
    expect(handled.action).toBe('provision')
    expect(deps.ec2.runCalls).toHaveLength(1)
    // The right args reached the provisioner -> the EC2 launch carries the tags.
    expect(deps.ec2.runCalls[0].tags['botho:subscription']).toBe('sub_abc')
    expect(deps.ec2.runCalls[0].tags['botho:user']).toBe('cus_xyz')
    expect(deps.ec2.runCalls[0].region).toBe('us-west-2')
  })

  it('provisions on invoice.paid', async () => {
    const deps = fakeDeps()
    await handleStripeEvent(
      {
        type: 'invoice.paid',
        data: {
          object: {
            subscription: 'sub_renew',
            customer: 'cus_xyz',
            subscription_details: { metadata: { region: 'us-west-2' } },
          },
        },
      } as never,
      deps,
    )
    expect(deps.ec2.runCalls).toHaveLength(1)
    expect(deps.ec2.runCalls[0].tags['botho:subscription']).toBe('sub_renew')
  })

  it('tears down on customer.subscription.deleted', async () => {
    const deps = fakeDeps()
    // Seed an existing running node so teardown has something to terminate.
    await deps.store.insertProvisioning({
      user: 'cus_xyz',
      stripeCustomer: 'cus_xyz',
      subscriptionId: 'sub_del',
      nodeId: 'del',
      region: 'us-west-2',
      rpcUrl: 'https://node-del.testnet.botho.io/rpc',
    })
    await deps.store.setInstanceId('sub_del', 'i-123')

    const handled = await handleStripeEvent(
      { type: 'customer.subscription.deleted', data: { object: { id: 'sub_del' } } } as never,
      deps,
    )
    expect(handled.action).toBe('teardown')
    expect(deps.ec2.terminateCalls).toEqual([{ region: 'us-west-2', instanceId: 'i-123' }])
    const row = await deps.store.getBySubscription('sub_del')
    expect(row?.state).toBe('terminated')
  })

  it('tears down on invoice.payment_failed', async () => {
    const deps = fakeDeps()
    await deps.store.insertProvisioning({
      user: 'cus_xyz',
      stripeCustomer: 'cus_xyz',
      subscriptionId: 'sub_pf',
      nodeId: 'pf',
      region: 'us-west-2',
      rpcUrl: 'https://node-pf.testnet.botho.io/rpc',
    })
    await deps.store.setInstanceId('sub_pf', 'i-pf')

    await handleStripeEvent(
      { type: 'invoice.payment_failed', data: { object: { subscription: 'sub_pf' } } } as never,
      deps,
    )
    expect(deps.ec2.terminateCalls).toEqual([{ region: 'us-west-2', instanceId: 'i-pf' }])
  })

  it('is idempotent: a duplicate delivery does not double-provision', async () => {
    const deps = fakeDeps()
    deps.ec2.runPublicIp = '203.0.113.7' // makes the row go straight to running
    const event = {
      type: 'checkout.session.completed',
      data: {
        object: {
          subscription: 'sub_dup',
          customer: 'cus_xyz',
          metadata: { region: 'us-west-2' },
        },
      },
    } as never

    const first = await handleStripeEvent(event, deps)
    const second = await handleStripeEvent(event, deps)

    expect(deps.ec2.runCalls).toHaveLength(1) // only ONE instance ever launched
    if (first.action === 'provision') expect(first.outcome.ok).toBe(true)
    if (second.action === 'provision') {
      expect(second.outcome.ok).toBe(true)
      if (second.outcome.ok) expect(second.outcome.created).toBe(false)
    }
  })

  it('no-ops on an unknown event type', async () => {
    const deps = fakeDeps()
    const handled = await handleStripeEvent(
      { type: 'customer.created', data: { object: {} } } as never,
      deps,
    )
    expect(handled.action).toBe('ignore')
    expect(deps.ec2.runCalls).toHaveLength(0)
    expect(deps.ec2.terminateCalls).toHaveLength(0)
  })

  it('no-ops a provision event missing required fields (no launch)', async () => {
    const deps = fakeDeps()
    const handled = await handleStripeEvent(
      {
        type: 'checkout.session.completed',
        data: { object: { customer: 'cus_xyz', metadata: { region: 'us-west-2' } } },
      } as never,
      deps,
    )
    expect(handled.action).toBe('ignore')
    expect(deps.ec2.runCalls).toHaveLength(0)
  })
})
