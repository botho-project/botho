/**
 * wBTH venue directory (#1030).
 *
 * Config-driven so the `/trade` page renders from data, and so mainnet
 * addresses can be swapped in later WITHOUT touching component code. The
 * testnet set holds the seeded pairs deployed during the bridge rollout
 * (Sepolia Uniswap v3, Solana-devnet Orca, HyperEVM PeerToken); the `mainnet`
 * set is intentionally empty until a production peg is stood up.
 *
 * IMPORTANT: these are TESTNET addresses — they are NOT valid on mainnet. The
 * page labels this clearly; keep the labeling if you edit this file.
 *
 * Explorer/deep-link hosts:
 * - Sepolia:       https://sepolia.etherscan.io   (used repo-wide, e.g.
 *                  contracts/ethereum/scripts/live-defi-roundtrip.ts)
 * - Solana devnet: https://explorer.solana.com (with `?cluster=devnet`)
 * - HyperEVM test: https://testnet.purrsec.com
 */
import type { BridgeNetwork, SourceChain, Venue, VenueDirectory } from './types'

// --- Ethereum Sepolia (Uniswap v3 wBTH/WETH) --------------------------------
const WBTH_SEPOLIA = '0x49b985ec427ee771a601f11b18f7d4402fa2dd7b'
const UNIV3_WBTH_WETH_POOL = '0x16C4fDbe2b7497EA67f1DC8205dd2F5B31458D53'

// --- Solana devnet (Orca wBTH/WSOL) -----------------------------------------
const WBTH_SOLANA_MINT = 'F7LsiATxVQxnDEBWemfuq1BgFDYbuzqMMJ5eZjaB7LFX'
const ORCA_WBTH_WSOL_POOL = '9Yog17D3nt1v9cREJBh1Ddeo6fRGuS48hbQcaH9WH1JS'

// --- Hyperliquid HyperEVM (PeerToken; HIP-1 spot pending #877) ---------------
const WBTH_HYPEREVM_PEERTOKEN = '0x230f154Ae33A53dcFFEDedB2d92cc1F32BcE7610'

const TESTNET_VENUES: Venue[] = [
  {
    id: 'uniswap-sepolia',
    chain: 'ethereum',
    chainLabel: 'Ethereum Sepolia',
    venueName: 'Uniswap v3',
    pairLabel: 'wBTH / WETH',
    tokenAddress: WBTH_SEPOLIA,
    poolAddress: UNIV3_WBTH_WETH_POOL,
    // Uniswap app deep-link, pre-selecting wBTH as the output token on Sepolia.
    tradeUrl: `https://app.uniswap.org/swap?chain=sepolia&outputCurrency=${WBTH_SEPOLIA}`,
    explorerUrl: `https://sepolia.etherscan.io/address/${UNIV3_WBTH_WETH_POOL}`,
    status: 'live',
  },
  {
    id: 'orca-solana-devnet',
    chain: 'solana',
    chainLabel: 'Solana devnet',
    venueName: 'Orca',
    pairLabel: 'wBTH / WSOL',
    tokenAddress: WBTH_SOLANA_MINT,
    poolAddress: ORCA_WBTH_WSOL_POOL,
    tradeUrl: 'https://www.orca.so/',
    explorerUrl: `https://explorer.solana.com/address/${ORCA_WBTH_WSOL_POOL}?cluster=devnet`,
    status: 'live',
  },
  {
    id: 'hyperliquid-hyperevm',
    chain: 'hyperliquid',
    chainLabel: 'Hyperliquid HyperEVM',
    venueName: 'Hyperliquid HIP-1 spot',
    pairLabel: 'wBTH spot',
    tokenAddress: WBTH_HYPEREVM_PEERTOKEN,
    // No trade link yet — HIP-1 spot market is pending (#877).
    explorerUrl: `https://testnet.purrsec.com/address/${WBTH_HYPEREVM_PEERTOKEN}`,
    status: 'coming-soon',
  },
]

/**
 * Venue sets keyed by network. Mainnet is empty until a production peg exists —
 * swap real addresses in here (mirroring the testnet shape) when it does.
 */
export const VENUES: VenueDirectory = {
  testnet: TESTNET_VENUES,
  mainnet: [],
}

/** The network Tier 0 surfaces. Bridge is testnet-only today. */
export const ACTIVE_BRIDGE_NETWORK: BridgeNetwork = 'testnet'

/** Venues for a given network (defaults to the active/testnet set). */
export function getVenues(network: BridgeNetwork = ACTIVE_BRIDGE_NETWORK): Venue[] {
  return VENUES[network]
}

// ─── Unwrap: burn targets (#1032) ───────────────────────────────────────────

/**
 * Where + how the user burns wBTH to redeem native BTH. The Botho wallet never
 * signs this call — the user executes `bridgeBurn(amount, bthAddress)` in THEIR
 * OWN counterparty wallet (MetaMask / Phantom / Anchor client), pasting the
 * wallet-provided BTH release address as `bthAddress`. This config drives the
 * unwrap panel's burn guidance (token to burn + a deep-link to where the user
 * executes it), so it stays config-driven and mainnet-swappable like `VENUES`.
 *
 * `bridgeBurn` is the ONLY burn path on both chains:
 * - Ethereum: `function bridgeBurn(uint256 amount, string bthAddress)`
 *   (`contracts/ethereum/contracts/WrappedBTH.sol`).
 * - Solana:   the `bridgeBurn(amount, bthAddress)` Anchor instruction
 *   (`contracts/solana/tests/wbth.ts`).
 */
export interface BurnTarget {
  /** Source chain the user burns on. */
  chain: SourceChain
  /** Human chain + network, e.g. "Ethereum Sepolia". */
  chainLabel: string
  /** wBTH token/mint address the user burns. */
  tokenAddress: string
  /**
   * The exact burn call the user makes, e.g. `bridgeBurn(amount, bthAddress)`.
   * `bthAddress` is the wallet-provided BTH release destination; `amount` is in
   * picocredits (u64), matching the token's on-chain decimals.
   */
  burnCall: string
  /**
   * Deep-link to where the user executes the burn: the block-explorer
   * write-contract tab on Ethereum, or the token/program page on Solana (Solana
   * explorers have no generic write UI — the burn is an Anchor instruction).
   */
  appUrl: string
  /** Block-explorer address page for the wBTH token. */
  explorerUrl: string
}

const TESTNET_BURN_TARGETS: BurnTarget[] = [
  {
    chain: 'ethereum',
    chainLabel: 'Ethereum Sepolia',
    tokenAddress: WBTH_SEPOLIA,
    burnCall: 'bridgeBurn(amount, bthAddress)',
    // Etherscan "Write Contract" tab, where the user calls bridgeBurn directly.
    appUrl: `https://sepolia.etherscan.io/address/${WBTH_SEPOLIA}#writeContract`,
    explorerUrl: `https://sepolia.etherscan.io/address/${WBTH_SEPOLIA}`,
  },
  {
    chain: 'solana',
    chainLabel: 'Solana devnet',
    tokenAddress: WBTH_SOLANA_MINT,
    burnCall: 'bridgeBurn(amount, bthAddress)',
    appUrl: `https://explorer.solana.com/address/${WBTH_SOLANA_MINT}?cluster=devnet`,
    explorerUrl: `https://explorer.solana.com/address/${WBTH_SOLANA_MINT}?cluster=devnet`,
  },
]

/** Burn targets keyed by network. Mainnet is empty until a production peg exists. */
export const BURN_TARGETS: Record<BridgeNetwork, BurnTarget[]> = {
  testnet: TESTNET_BURN_TARGETS,
  mainnet: [],
}

/** Burn targets for a given network (defaults to the active/testnet set). */
export function getBurnTargets(
  network: BridgeNetwork = ACTIVE_BRIDGE_NETWORK,
): BurnTarget[] {
  return BURN_TARGETS[network]
}

/** The burn target for a specific source chain, or `undefined` if unsupported. */
export function getBurnTarget(
  chain: SourceChain,
  network: BridgeNetwork = ACTIVE_BRIDGE_NETWORK,
): BurnTarget | undefined {
  return BURN_TARGETS[network].find((b) => b.chain === chain)
}
