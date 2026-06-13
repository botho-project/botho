/**
 * Test data for E2E tests.
 *
 * Uses deterministic values where possible for reproducible tests.
 */

// Service URLs
//
// By default the e2e suite runs against locally-served builds (started by the
// Playwright `webServer` config) so it is deterministic and CI-friendly:
//   - Web wallet / explorer: http://localhost:4173  (vite preview of web-wallet)
//   - Faucet static site:    http://localhost:4174  (infra/faucet/web)
//
// To run against the live deployment instead, set the env vars:
//   E2E_WEB_BASE_URL=https://botho.io E2E_FAUCET_BASE_URL=https://faucet.botho.io
const WEB_BASE = process.env.E2E_WEB_BASE_URL ?? 'http://localhost:4173'
const FAUCET_BASE = process.env.E2E_FAUCET_BASE_URL ?? 'http://localhost:4174'

export const URLS = {
  WALLET: `${WEB_BASE}/wallet`,
  EXPLORER: `${WEB_BASE}/explorer`,
  FAUCET: FAUCET_BASE,
  LANDING: WEB_BASE,
} as const

// Standard BIP39 test mnemonics - produce deterministic addresses
// WARNING: Never use these mnemonics for real funds
export const TEST_MNEMONIC_12 =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about'

export const TEST_MNEMONIC_24 =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art'

// Expected addresses from the test mnemonics (testnet)
// These are deterministic based on BIP39/BIP44 derivation
export const EXPECTED_ADDRESSES = {
  // Address derived from TEST_MNEMONIC_12
  MNEMONIC_12: 'tbotho://1/',
  // Address derived from TEST_MNEMONIC_24
  MNEMONIC_24: 'tbotho://1/',
} as const

// Legacy export for backwards compatibility
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
