/// <reference types="vite/client" />
/// <reference types="vite-plugin-pwa/client" />

interface ImportMetaEnv {
  /** Custom RPC endpoint URL (overrides default testnet endpoint) */
  readonly VITE_RPC_ENDPOINT?: string
  /** Custom faucet endpoint URL (overrides default testnet faucet) */
  readonly VITE_FAUCET_ENDPOINT?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
