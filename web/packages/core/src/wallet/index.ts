// Wallet-related exports will go here
// For now, we'll add placeholder types

export interface WalletConfig {
  /** Network to connect to */
  network: 'mainnet' | 'testnet'
  /** Storage key prefix */
  storagePrefix: string
}

export const DEFAULT_WALLET_CONFIG: WalletConfig = {
  network: 'mainnet',
  storagePrefix: 'botho-wallet',
}
