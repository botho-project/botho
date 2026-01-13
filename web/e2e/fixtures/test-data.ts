/**
 * Test data for E2E tests.
 *
 * Uses deterministic values where possible for reproducible tests.
 */

// Service URLs
export const URLS = {
  WALLET: 'https://botho.io/wallet',
  EXPLORER: 'https://botho.io/explorer',
  FAUCET: 'https://faucet.botho.io',
  LANDING: 'https://botho.io',
} as const

// Standard BIP39 test mnemonic - produces deterministic addresses
// WARNING: Never use this mnemonic for real funds
export const TEST_MNEMONIC_12 =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about'

// Expected address from the test mnemonic (testnet)
export const EXPECTED_ADDRESS_PREFIX = 'tbotho://1/'

// Known testnet data for explorer tests
// These should be updated periodically to reference recent blocks/transactions
export const KNOWN_DATA = {
  // A known transaction hash from testnet faucet
  TX_HASH: '3ca3c24209844d8f6d9bf38bd1571976a691423e329f4ca0cbbf3d044da3012e',
  // A known block height that exists
  BLOCK_HEIGHT: 896,
  // Block hash for the known height
  BLOCK_HASH: '0e8a92f9fdd7adfb59689f0b62dc4b7867c3dfa790a69a5562e53ce5f3280b03',
} as const

// Invalid inputs for error handling tests
export const INVALID = {
  ADDRESS: 'not-a-valid-address',
  MNEMONIC: 'invalid mnemonic words that will not work',
  TX_HASH: '0000000000000000000000000000000000000000000000000000000000000000',
} as const

// Timeouts for various operations
export const TIMEOUTS = {
  PAGE_LOAD: 10_000,
  NETWORK_REQUEST: 15_000,
  BLOCK_CONFIRMATION: 45_000,
  WALLET_SYNC: 30_000,
} as const
