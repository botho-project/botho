import { describe, it, expect } from 'vitest'
import { buildUserData, renderUserDataScript } from './user-data'
import { testBase64, testBase64Decode } from './test-fakes'

const PARAMS = {
  rigId: 'abc123',
  region: 'us-west-2',
  tier: 't4g.medium',
  rigDomain: 'testnet.botho.io',
  binaryUrl: 'https://example.com/botho-aarch64',
  binarySha256: 'deadbeef',
}

describe('renderUserDataScript', () => {
  const script = renderUserDataScript(PARAMS)

  it('exports the bootstrap parameters the rig-bootstrap.sh contract expects', () => {
    expect(script).toContain("export RIG_ID='abc123'")
    expect(script).toContain("export RIG_DOMAIN='testnet.botho.io'")
    expect(script).toContain("export REGION='us-west-2'")
    expect(script).toContain("export TIER='t4g.medium'")
    expect(script).toContain('export NETWORK=testnet')
    expect(script).toContain("export BOTHO_BINARY_URL='https://example.com/botho-aarch64'")
    expect(script).toContain("export BOTHO_BINARY_SHA256='deadbeef'")
  })

  it('fetches + execs the bootstrap script', () => {
    expect(script).toContain('rig-bootstrap.sh')
    expect(script).toContain('curl -fsSL')
    expect(script).toContain('exec /root/rig-bootstrap.sh')
  })

  it('omits the binary exports when not provided', () => {
    const s = renderUserDataScript({ ...PARAMS, binaryUrl: undefined, binarySha256: undefined })
    expect(s).not.toContain('BOTHO_BINARY_URL')
    expect(s).not.toContain('BOTHO_BINARY_SHA256')
  })

  it('shell-quotes safely against injection in the rig id', () => {
    const s = renderUserDataScript({ ...PARAMS, rigId: "x'; rm -rf /; '" })
    // The single quote is escaped POSIX-style; no unescaped break-out.
    expect(s).toContain(`export RIG_ID='x'\\''; rm -rf /; '\\'''`)
  })

  it('honors a custom bootstrap script url', () => {
    const s = renderUserDataScript({ ...PARAMS, bootstrapScriptUrl: 'https://r2.example/boot.sh' })
    expect(s).toContain("BOOTSTRAP_URL='https://r2.example/boot.sh'")
  })
})

describe('buildUserData', () => {
  it('base64-encodes the rendered script', () => {
    const b64 = buildUserData(PARAMS, testBase64)
    const decoded = testBase64Decode(b64)
    expect(decoded).toBe(renderUserDataScript(PARAMS))
  })
})
