import { parseAddress } from '@botho/core'

// Exact IDs emitted by node_getStatus; unknown is not a third spending network.
export type WalletNetwork = 'botho-mainnet' | 'botho-testnet'
export function walletNetwork(id: unknown): WalletNetwork | null {
  return id === 'botho-mainnet' || id === 'botho-testnet' ? id : null
}
export function validWalletAddress(address: string, network: WalletNetwork | null): boolean {
  if (!network) return false
  try {
    const parsed = parseAddress(address)
    return address.startsWith(network === 'botho-mainnet' ? 'botho://2/' : 'tbotho://2/') && !!parsed
  } catch { return false }
}
export function addressCacheKey(network: WalletNetwork): string {
  return `botho-wallet-address:${network}`
}
