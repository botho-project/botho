// Auto-load contracts/ethereum/.env (git-ignored) so PRIVATE_KEY,
// SEPOLIA_RPC_URL, ETHERSCAN_API_KEY and the WBTH_*_SAFE / BRIDGE_SAFE_OWNER_*
// addresses resolve without a manual `source .env` (#1011). Secrets are only
// read from this git-ignored file — never printed, never committed.
import "dotenv/config";
import { configVariable, type HardhatUserConfig } from "hardhat/config";
import hardhatToolboxMochaEthers from "@nomicfoundation/hardhat-toolbox-mocha-ethers";

// Hardhat 3 config (#1174): ESM-first, explicit `plugins` array, and every
// network declares its `type` ("edr-simulated" in-process vs "http" remote).
const config: HardhatUserConfig = {
  plugins: [hardhatToolboxMochaEthers],
  solidity: {
    version: "0.8.20",
    settings: {
      optimizer: {
        enabled: true,
        runs: 200,
      },
    },
  },
  networks: {
    hardhat: {
      type: "edr-simulated",
      chainId: 31337,
      // Hardhat 2 sent gas = blockGasLimit on the in-process network; Hardhat
      // 3 estimates instead. For SafeStub.execTransaction the outer tx
      // "succeeds" (ExecutionFailure event) even when the inner call runs out
      // of gas, so minimal-gas estimation silently starves the inner
      // bridgeMint. Pin a fixed gas allowance like HH2 (kept under EDR's
      // 16,777,216 per-transaction gas cap, EIP-7825).
      gas: 16_000_000,
    },
    localhost: {
      type: "http",
      url: "http://127.0.0.1:8545",
    },
    // Local Sepolia FORK for dry-running deploys with no real testnet ETH
    // (#1011/#992). Point BRIDGE_FORK_RPC_URL at an `anvil --fork-url <sepolia>`
    // node; the deployer is any key funded on the fork via anvil_setBalance.
    fork: {
      type: "http",
      url: process.env.BRIDGE_FORK_RPC_URL || "http://127.0.0.1:8545",
      accounts: process.env.PRIVATE_KEY ? [process.env.PRIVATE_KEY] : [],
    },
    // Hardhat 3 rejects empty-string URLs at config-load time, so unset RPC
    // URLs fall back to configVariable(): resolution is lazy and only fails
    // when the network is actually connected to.
    sepolia: {
      type: "http",
      url: process.env.SEPOLIA_RPC_URL || configVariable("SEPOLIA_RPC_URL"),
      accounts: process.env.PRIVATE_KEY ? [process.env.PRIVATE_KEY] : [],
    },
    mainnet: {
      type: "http",
      url: process.env.MAINNET_RPC_URL || configVariable("MAINNET_RPC_URL"),
      accounts: process.env.PRIVATE_KEY ? [process.env.PRIVATE_KEY] : [],
    },
  },
  // Etherscan source verification (#1013). Key is read from the git-ignored
  // .env (ETHERSCAN_API_KEY) via dotenv above — never committed. hardhat-verify
  // v3 nests the etherscan block under `verify`.
  verify: {
    etherscan: {
      // Etherscan API V2 — a single multichain key (chainid selects the
      // explorer). V1 per-network keys are deprecated.
      apiKey: process.env.ETHERSCAN_API_KEY || "",
    },
  },
};

export default config;
