/**
 * Integration tests for the `/webhook` HTTP handler (`handleWebhook` in
 * index.ts) — the signature gate + dispatch wiring (#506, #458 §5).
 *
 * These exercise the FULL request path: raw-body read, signature verification,
 * JSON parse, and dispatch into an injected fake provisioner. No real Stripe /
 * AWS / DNS / D1 is touched.
 */

import { describe, it, expect } from 'vitest'
import { handleWebhook, type Env } from './index'
import { hmacSha256Hex } from './webhook'
import { FakeDns, FakeEc2, FakeStore } from './test-fakes'
import type { ProvisionerDeps } from './provisioner'
import { DEFAULT_RIG_COMPUTE, DEFAULT_INSTANCE_TYPE } from './rig-config'

const SECRET = 'whsec_test_secret_value'

const ENV: Env = {
  // checkout env (unused here but part of the shared Env shape)
  STRIPE_SECRET_KEY: 'sk_test_dummy',
  STRIPE_PRICE_ID: 'price_test',
  CHECKOUT_SUCCESS_URL: 'https://botho.io/rig/success',
  CHECKOUT_CANCEL_URL: 'https://botho.io/rig',
  // webhook secret
  STRIPE_WEBHOOK_SECRET: SECRET,
  // provisioner env so missingProvisionerEnv() passes (DB is the fake below)
  AWS_ACCESS_KEY_ID: 'AKIA_TEST',
  AWS_SECRET_ACCESS_KEY: 'secret',
  CF_DNS_API_TOKEN: 'cf_token',
  CF_DNS_ZONE_ID: 'zone',
  // a non-null DB placeholder; the injected depsFor() ignores it (uses fakes).
  DB: {} as never,
}

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

async function signedRequest(
  bodyObj: unknown,
  opts: { secret?: string; ts?: number; header?: string | null } = {},
): Promise<Request> {
  const body = JSON.stringify(bodyObj)
  const ts = opts.ts ?? nowSec()
  let header = opts.header
  if (header === undefined) {
    const v1 = await hmacSha256Hex(opts.secret ?? SECRET, `${ts}.${body}`)
    header = `t=${ts},v1=${v1}`
  }
  return new Request('https://control.botho.io/webhook', {
    method: 'POST',
    headers: header === null ? {} : { 'Stripe-Signature': header },
    body,
  })
}

function makeDeps(): {
  deps: ProvisionerDeps & { ec2: FakeEc2; dns: FakeDns; store: FakeStore }
  depsFor: () => ProvisionerDeps
} {
  const ec2 = new FakeEc2()
  const dns = new FakeDns()
  const store = new FakeStore()
  const deps: ProvisionerDeps & { ec2: FakeEc2; dns: FakeDns; store: FakeStore } = {
    ec2,
    dns,
    store,
    compute: { ...DEFAULT_RIG_COMPUTE, instanceType: DEFAULT_INSTANCE_TYPE },
    rigDomain: 'testnet.botho.io',
    fleetCap: 25,
  }
  return { deps, depsFor: () => deps }
}

describe('handleWebhook', () => {
  it('provisions on a valid signature + checkout.session.completed', async () => {
    const { deps, depsFor } = makeDeps()
    const req = await signedRequest({
      id: 'evt_1',
      type: 'checkout.session.completed',
      data: {
        object: {
          subscription: 'sub_abc',
          customer: 'cus_xyz',
          metadata: { region: 'us-west-2' },
        },
      },
    })
    const res = await handleWebhook(req, ENV, depsFor)
    expect(res.status).toBe(200)
    const json = (await res.json()) as { received: boolean; action: string }
    expect(json.received).toBe(true)
    expect(json.action).toBe('provision')
    expect(deps.ec2.runCalls).toHaveLength(1)
    expect(deps.ec2.runCalls[0].tags['botho:subscription']).toBe('sub_abc')
  })

  it('rejects an invalid signature with 400 and does NOT provision', async () => {
    const { deps, depsFor } = makeDeps()
    const req = await signedRequest(
      {
        type: 'checkout.session.completed',
        data: { object: { subscription: 'sub_bad', customer: 'c', metadata: { region: 'us-west-2' } } },
      },
      { secret: 'whsec_wrong' },
    )
    const res = await handleWebhook(req, ENV, depsFor)
    expect(res.status).toBe(400)
    expect(deps.ec2.runCalls).toHaveLength(0)
  })

  it('rejects a missing signature header with 400 and does NOT provision', async () => {
    const { deps, depsFor } = makeDeps()
    const req = await signedRequest(
      { type: 'checkout.session.completed', data: { object: {} } },
      { header: null },
    )
    const res = await handleWebhook(req, ENV, depsFor)
    expect(res.status).toBe(400)
    expect(deps.ec2.runCalls).toHaveLength(0)
  })

  it('rejects a stale signature (replay) with 400 and does NOT provision', async () => {
    const { deps, depsFor } = makeDeps()
    const req = await signedRequest(
      {
        type: 'checkout.session.completed',
        data: { object: { subscription: 'sub', customer: 'c', metadata: { region: 'us-west-2' } } },
      },
      { ts: nowSec() - 10_000 },
    )
    const res = await handleWebhook(req, ENV, depsFor)
    expect(res.status).toBe(400)
    expect(deps.ec2.runCalls).toHaveLength(0)
  })

  it('tears down on customer.subscription.deleted', async () => {
    const { deps, depsFor } = makeDeps()
    await deps.store.insertProvisioning({
      user: 'cus_xyz',
      stripeCustomer: 'cus_xyz',
      subscriptionId: 'sub_del',
      rigId: 'del',
      region: 'us-west-2',
      rpcUrl: 'https://rig-del.testnet.botho.io/rpc',
    })
    await deps.store.setInstanceId('sub_del', 'i-del')

    const req = await signedRequest({
      type: 'customer.subscription.deleted',
      data: { object: { id: 'sub_del' } },
    })
    const res = await handleWebhook(req, ENV, depsFor)
    expect(res.status).toBe(200)
    expect(deps.ec2.terminateCalls).toEqual([{ region: 'us-west-2', instanceId: 'i-del' }])
  })

  it('is idempotent against duplicate deliveries (no double launch)', async () => {
    const { deps, depsFor } = makeDeps()
    deps.ec2.runPublicIp = '203.0.113.9'
    const event = {
      id: 'evt_dup',
      type: 'checkout.session.completed',
      data: {
        object: { subscription: 'sub_dup', customer: 'cus_xyz', metadata: { region: 'us-west-2' } },
      },
    }
    const res1 = await handleWebhook(await signedRequest(event), ENV, depsFor)
    const res2 = await handleWebhook(await signedRequest(event), ENV, depsFor)
    expect(res1.status).toBe(200)
    expect(res2.status).toBe(200)
    expect(deps.ec2.runCalls).toHaveLength(1)
  })

  it('2xx no-ops on an unknown event type', async () => {
    const { deps, depsFor } = makeDeps()
    const req = await signedRequest({ type: 'customer.created', data: { object: {} } })
    const res = await handleWebhook(req, ENV, depsFor)
    expect(res.status).toBe(200)
    const json = (await res.json()) as { action: string }
    expect(json.action).toBe('ignore')
    expect(deps.ec2.runCalls).toHaveLength(0)
  })

  it('verifies over the RAW body — a re-serialized body fails', async () => {
    const { deps, depsFor } = makeDeps()
    // Sign a compact body, then deliver a pretty-printed body with the SAME
    // signature. If the handler parsed-then-reserialized before verifying, this
    // would wrongly pass; verifying raw bytes makes it fail.
    const obj = {
      type: 'checkout.session.completed',
      data: { object: { subscription: 'sub', customer: 'c', metadata: { region: 'us-west-2' } } },
    }
    const compact = JSON.stringify(obj)
    const ts = nowSec()
    const v1 = await hmacSha256Hex(SECRET, `${ts}.${compact}`)
    const pretty = JSON.stringify(obj, null, 2)
    const req = new Request('https://control.botho.io/webhook', {
      method: 'POST',
      headers: { 'Stripe-Signature': `t=${ts},v1=${v1}` },
      body: pretty,
    })
    const res = await handleWebhook(req, ENV, depsFor)
    expect(res.status).toBe(400)
    expect(deps.ec2.runCalls).toHaveLength(0)
  })

  it('rejects non-POST with 405', async () => {
    const { depsFor } = makeDeps()
    const req = new Request('https://control.botho.io/webhook', { method: 'GET' })
    const res = await handleWebhook(req, ENV, depsFor)
    expect(res.status).toBe(405)
  })

  it('fails closed with 500 when STRIPE_WEBHOOK_SECRET is unset (no provision)', async () => {
    const { deps, depsFor } = makeDeps()
    const badEnv = { ...ENV, STRIPE_WEBHOOK_SECRET: '' }
    const req = await signedRequest({
      type: 'checkout.session.completed',
      data: { object: { subscription: 's', customer: 'c', metadata: { region: 'us-west-2' } } },
    })
    const res = await handleWebhook(req, badEnv, depsFor)
    expect(res.status).toBe(500)
    expect(deps.ec2.runCalls).toHaveLength(0)
  })
})
