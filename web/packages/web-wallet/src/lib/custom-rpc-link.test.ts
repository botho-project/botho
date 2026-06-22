import { describe, it, expect } from 'vitest'
import {
  RPC_PARAM,
  buildWalletRpcLink,
  isValidRpcUrl,
  parseRpcDeepLink,
} from './custom-rpc-link'

describe('isValidRpcUrl', () => {
  it('accepts https endpoints', () => {
    expect(isValidRpcUrl('https://rig-abc.testnet.botho.io/rpc')).toBe(true)
  })

  it('accepts http only for localhost / 127.0.0.1 (dev)', () => {
    expect(isValidRpcUrl('http://localhost:8787/rpc')).toBe(true)
    expect(isValidRpcUrl('http://127.0.0.1/rpc')).toBe(true)
  })

  it('rejects plain http on a non-loopback host', () => {
    expect(isValidRpcUrl('http://evil.example/rpc')).toBe(false)
  })

  it('rejects non-URLs and non-http(s) schemes', () => {
    expect(isValidRpcUrl('not a url')).toBe(false)
    expect(isValidRpcUrl('javascript:alert(1)')).toBe(false)
    expect(isValidRpcUrl('ftp://host/rpc')).toBe(false)
  })
})

describe('parseRpcDeepLink', () => {
  it('returns absent when there is no rpc param', () => {
    expect(parseRpcDeepLink('').ok).toBe('absent')
    expect(parseRpcDeepLink('?foo=bar').ok).toBe('absent')
    expect(parseRpcDeepLink('?rpc=').ok).toBe('absent')
  })

  it('parses a valid url-encoded https endpoint', () => {
    const search = `?${RPC_PARAM}=${encodeURIComponent('https://rig-x.testnet.botho.io/rpc')}`
    const parsed = parseRpcDeepLink(search)
    expect(parsed.ok).toBe(true)
    if (parsed.ok === true) {
      expect(parsed.rpcUrl).toBe('https://rig-x.testnet.botho.io/rpc')
    }
  })

  it('rejects a present-but-invalid rpc param', () => {
    const parsed = parseRpcDeepLink(`?${RPC_PARAM}=${encodeURIComponent('http://evil/rpc')}`)
    expect(parsed.ok).toBe(false)
  })

  it('preserves other params and still finds rpc', () => {
    const search = `?session_id=abc&${RPC_PARAM}=${encodeURIComponent('https://n/rpc')}`
    const parsed = parseRpcDeepLink(search)
    expect(parsed.ok).toBe(true)
  })
})

describe('buildWalletRpcLink', () => {
  it('appends an encoded rpc param', () => {
    expect(buildWalletRpcLink('/wallet', 'https://n/rpc')).toBe(
      '/wallet?rpc=https%3A%2F%2Fn%2Frpc',
    )
  })

  it('uses & when the path already has a query string', () => {
    expect(buildWalletRpcLink('/wallet?x=1', 'https://n/rpc')).toBe(
      '/wallet?x=1&rpc=https%3A%2F%2Fn%2Frpc',
    )
  })

  it('round-trips through parseRpcDeepLink', () => {
    const rpc = 'https://rig-abc.testnet.botho.io/rpc'
    const link = buildWalletRpcLink('/wallet', rpc)
    const search = link.slice(link.indexOf('?'))
    const parsed = parseRpcDeepLink(search)
    expect(parsed.ok).toBe(true)
    if (parsed.ok === true) expect(parsed.rpcUrl).toBe(rpc)
  })
})
