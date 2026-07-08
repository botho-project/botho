import { describe, it, expect } from 'vitest'
import {
  RPC_PARAM,
  buildWalletRpcLink,
  isValidRpcUrl,
  parseRpcDeepLink,
  rpcLinkHost,
  classifyRpcHost,
} from './custom-rpc-link'

describe('isValidRpcUrl', () => {
  it('accepts https endpoints', () => {
    expect(isValidRpcUrl('https://node-abc.testnet.botho.io/rpc')).toBe(true)
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
    const search = `?${RPC_PARAM}=${encodeURIComponent('https://node-x.testnet.botho.io/rpc')}`
    const parsed = parseRpcDeepLink(search)
    expect(parsed.ok).toBe(true)
    if (parsed.ok === true) {
      expect(parsed.rpcUrl).toBe('https://node-x.testnet.botho.io/rpc')
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
    const rpc = 'https://node-abc.testnet.botho.io/rpc'
    const link = buildWalletRpcLink('/wallet', rpc)
    const search = link.slice(link.indexOf('?'))
    const parsed = parseRpcDeepLink(search)
    expect(parsed.ok).toBe(true)
    if (parsed.ok === true) expect(parsed.rpcUrl).toBe(rpc)
  })
})

describe('rpcLinkHost', () => {
  it('extracts the bare host without the port', () => {
    expect(rpcLinkHost('https://node-abc.testnet.botho.io:8443/rpc')).toBe(
      'node-abc.testnet.botho.io',
    )
  })

  it('returns null for an unparseable url', () => {
    expect(rpcLinkHost('not a url')).toBeNull()
  })
})

describe('classifyRpcHost (#587 trust hint — never an authorization decision)', () => {
  it('treats Botho-operated hosts as known', () => {
    expect(classifyRpcHost('https://seed.botho.io/rpc')).toBe('known')
    expect(classifyRpcHost('https://node-abc.testnet.botho.io/rpc')).toBe('known')
    expect(classifyRpcHost('https://botho.io/rpc')).toBe('known')
  })

  it('treats loopback (dev) as known', () => {
    expect(classifyRpcHost('http://localhost:8787/rpc')).toBe('known')
    expect(classifyRpcHost('http://127.0.0.1/rpc')).toBe('known')
  })

  it('flags arbitrary third-party hosts as unknown', () => {
    expect(classifyRpcHost('https://evil.example/rpc')).toBe('unknown')
    // A look-alike that merely CONTAINS the brand is still unknown.
    expect(classifyRpcHost('https://botho.io.evil.example/rpc')).toBe('unknown')
    expect(classifyRpcHost('https://notbotho.io/rpc')).toBe('unknown')
  })

  it('treats an unparseable url as unknown (most conservative)', () => {
    expect(classifyRpcHost('not a url')).toBe('unknown')
  })
})
