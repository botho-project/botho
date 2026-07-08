import { describe, it, expect } from 'vitest'
import { depsFromEnv, missingProvisionerEnv, type ProvisionerEnv } from './provisioner-env'
import type { D1Like } from './node-store'

const DB = { prepare: () => ({}) } as unknown as D1Like

const FULL: ProvisionerEnv = {
  AWS_ACCESS_KEY_ID: 'AKID',
  AWS_SECRET_ACCESS_KEY: 'SECRET',
  CF_DNS_API_TOKEN: 'token',
  CF_DNS_ZONE_ID: 'zone',
  DB,
}

describe('missingProvisionerEnv', () => {
  it('reports nothing when fully configured', () => {
    expect(missingProvisionerEnv(FULL)).toEqual([])
  })

  it('reports each missing secret/binding', () => {
    expect(missingProvisionerEnv({})).toEqual([
      'AWS_ACCESS_KEY_ID',
      'AWS_SECRET_ACCESS_KEY',
      'CF_DNS_API_TOKEN',
      'CF_DNS_ZONE_ID',
      'DB',
    ])
  })

  it('treats an empty-string secret as missing', () => {
    expect(missingProvisionerEnv({ ...FULL, AWS_SECRET_ACCESS_KEY: '' })).toEqual([
      'AWS_SECRET_ACCESS_KEY',
    ])
  })
})

describe('depsFromEnv', () => {
  it('throws (fail closed) when a required secret is missing', () => {
    expect(() => depsFromEnv({ ...FULL, DB: undefined })).toThrow(/not configured/)
  })

  it('builds deps with the default compute shape + overrides', () => {
    const deps = depsFromEnv(FULL)
    expect(deps.compute?.instanceType).toBe('t4g.medium')
    expect(deps.compute?.amiId).toBe('ami-012798e88aebdba5c')
    expect(deps.nodeDomain).toBe('testnet.botho.io')

    const overridden = depsFromEnv({ ...FULL, NODE_AMI_ID: 'ami-custom', FLEET_CAP: '7' })
    expect(overridden.compute?.amiId).toBe('ami-custom')
    expect(overridden.fleetCap).toBe(7)
  })

  it('never forces a non-allowlisted instance type even via env', () => {
    // There is no env knob for instance type; the compute shape always uses
    // t4g.medium (#458 §5). This documents that invariant.
    const deps = depsFromEnv({ ...FULL, NODE_KEY_NAME: 'k' })
    expect(deps.compute?.instanceType).toBe('t4g.medium')
  })
})
