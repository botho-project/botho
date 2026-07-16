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
import type { BridgeNetwork, Venue, VenueDirectory } from './types'

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
