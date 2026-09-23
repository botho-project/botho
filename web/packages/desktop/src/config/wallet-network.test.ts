import { expect, it } from 'vitest'
import { formatAddress } from '@botho/core'
import { walletNetwork, validWalletAddress, addressCacheKey } from './wallet-network'
it('recognizes exact node networks and complete matching v2 watch addresses', () => {
  const address = formatAddress(new Uint8Array(32), new Uint8Array(32), new Uint8Array(1184), new Uint8Array(1952), 'testnet')
  expect(walletNetwork('botho-testnet')).toBe('botho-testnet')
  expect(walletNetwork('botho-mainnet')).toBe('botho-mainnet')
  for (const v of ['', undefined, null, 42, 'testnet', 'botho-testnet-extra']) expect(walletNetwork(v)).toBeNull()
  expect(validWalletAddress(address, 'botho-testnet')).toBe(true)
  expect(validWalletAddress(address, 'botho-mainnet')).toBe(false)
  expect(validWalletAddress(address, null)).toBe(false)
  for (const v of ['cad:a:b', 'tbotho://1/abc', 'tbotho://2/abc']) expect(validWalletAddress(v, 'botho-testnet')).toBe(false)
  expect(addressCacheKey('botho-testnet')).not.toBe(addressCacheKey('botho-mainnet'))
})
