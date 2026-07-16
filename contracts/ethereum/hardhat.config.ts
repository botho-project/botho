// Auto-load contracts/ethereum/.env (git-ignored) so PRIVATE_KEY,
// SEPOLIA_RPC_URL, ETHERSCAN_API_KEY and the WBTH_*_SAFE / BRIDGE_SAFE_OWNER_*
// addresses resolve without a manual `source .env` (#1011). Secrets are only
// read from this git-ignored file — never printed, never committed.
import "dotenv/config";
import { HardhatUserConfig } from "hardhat/config";
import "@nomicfoundation/hardhat-toolbox";

const config: HardhatUserConfig = {
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
      chainId: 31337,
    },
    localhost: {
      url: "http://127.0.0.1:8545",
    },
    // Local Sepolia FORK for dry-running deploys with no real testnet ETH
    // (#1011/#992). Point BRIDGE_FORK_RPC_URL at an `anvil --fork-url <sepolia>`
    // node; the deployer is any key funded on the fork via anvil_setBalance.
    fork: {
      url: process.env.BRIDGE_FORK_RPC_URL || "http://127.0.0.1:8545",
      accounts: process.env.PRIVATE_KEY ? [process.env.PRIVATE_KEY] : [],
    },
    sepolia: {
      url: process.env.SEPOLIA_RPC_URL || "",
      accounts: process.env.PRIVATE_KEY ? [process.env.PRIVATE_KEY] : [],
    },
    mainnet: {
      url: process.env.MAINNET_RPC_URL || "",
      accounts: process.env.PRIVATE_KEY ? [process.env.PRIVATE_KEY] : [],
    },
  },
  // Etherscan source verification (#1013). Key is read from the git-ignored
  // .env (ETHERSCAN_API_KEY) via dotenv above — never committed.
  etherscan: {
    // Etherscan API V2 — a single multichain key (chainid selects the
    // explorer). V1 per-network keys are deprecated.
    apiKey: process.env.ETHERSCAN_API_KEY || "",
  },
};

export default config;
