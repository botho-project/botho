// Copyright (c) 2024 Botho Foundation

//! Canonical hardcoded bootstrap-seed list (single source of truth).
//!
//! This module is the one place that defines the *hardcoded fallback* seed
//! multiaddrs for each network. Both [`crate::config::NetworkConfig`] and
//! [`crate::network::DnsSeedDiscovery`] delegate here so the two layers never
//! drift apart.
//!
//! ## Bootstrap order (see PLAN.md "Network Bootstrap Strategy")
//!
//! 1. Explicitly configured `bootstrap_peers` in `config.toml` (if non-empty).
//! 2. DNS TXT-record discovery (`seeds.botho.io` / `seeds.testnet.botho.io`).
//! 3. The hardcoded fallback list defined here.
//!
//! ## Multi-seed / regional scaffolding
//!
//! PLAN.md calls for >= 3 geographically diverse seeds + DNS failover. The
//! regional hostnames below (`us`, `eu`, `ap`) are *scaffolding*: they are
//! emitted only when [`include_regional_seeds`] is true, which currently
//! requires the `BOTHO_REGIONAL_SEEDS=1` environment variable. They stay OFF
//! by default because the regional DNS records are not yet provisioned —
//! dialing unresolvable hostnames just wastes connection attempts.
//!
//! Operator activation (NOT done by this code):
//!   1. Launch the regional seed hosts and point DNS at them.
//!   2. Either set `BOTHO_REGIONAL_SEEDS=1`, or (preferred) publish the seeds
//!      as `seeds.testnet.botho.io` TXT records so DNS discovery picks them up
//!      with zero client changes.

use bth_transaction_types::constants::Network;

/// Primary live testnet seed (gossip on the testnet port; peer ID resolved
/// dynamically so a host re-key does not require a client release).
const TESTNET_PRIMARY_SEED: &str = "/dns4/seed.botho.io/tcp/17100";

/// Primary live mainnet seed (with pinned peer ID).
const MAINNET_PRIMARY_SEED: &str =
    "/dns4/seed.botho.io/tcp/7100/p2p/12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ";

/// Regional testnet seeds (>= 3 regions per PLAN.md). NOT yet live — gated
/// behind [`include_regional_seeds`]. Peer IDs are resolved dynamically.
const TESTNET_REGIONAL_SEEDS: &[&str] = &[
    "/dns4/us.seed.botho.io/tcp/17100",
    "/dns4/eu.seed.botho.io/tcp/17100",
    "/dns4/ap.seed.botho.io/tcp/17100",
];

/// Regional mainnet seeds (>= 3 regions per PLAN.md). NOT yet live — gated
/// behind [`include_regional_seeds`].
const MAINNET_REGIONAL_SEEDS: &[&str] = &[
    "/dns4/us.seed.botho.io/tcp/7100",
    "/dns4/eu.seed.botho.io/tcp/7100",
    "/dns4/ap.seed.botho.io/tcp/7100",
];

/// Whether to include the (not-yet-live) regional seed scaffolding.
///
/// Returns true only when `BOTHO_REGIONAL_SEEDS` is set to a truthy value
/// (`1`, `true`, `yes`). Keeping this opt-in avoids dialing unresolvable
/// hostnames before the regional infra exists.
pub fn include_regional_seeds() -> bool {
    match std::env::var("BOTHO_REGIONAL_SEEDS") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            v == "1" || v == "true" || v == "yes"
        }
        Err(_) => false,
    }
}

/// Hardcoded fallback bootstrap seeds for a network.
///
/// Always includes the primary live seed. Includes the regional scaffolding
/// only when [`include_regional_seeds`] is true.
pub fn fallback_seeds(network: Network) -> Vec<String> {
    let (primary, regional): (&str, &[&str]) = match network {
        Network::Mainnet => (MAINNET_PRIMARY_SEED, MAINNET_REGIONAL_SEEDS),
        Network::Testnet => (TESTNET_PRIMARY_SEED, TESTNET_REGIONAL_SEEDS),
    };

    let mut seeds = vec![primary.to_string()];
    if include_regional_seeds() {
        seeds.extend(regional.iter().map(|s| s.to_string()));
    }
    seeds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_seed_always_present() {
        // Regardless of env, the primary live seed is always first.
        let testnet = fallback_seeds(Network::Testnet);
        assert!(!testnet.is_empty());
        assert!(testnet[0].contains("seed.botho.io"));
        assert!(testnet[0].contains("17100"));

        let mainnet = fallback_seeds(Network::Mainnet);
        assert!(mainnet[0].contains("seed.botho.io"));
        assert!(mainnet[0].contains("7100"));
    }

    #[test]
    fn regional_seeds_cover_three_regions() {
        // The scaffolding must define >= 3 regions for both networks
        // (PLAN.md "min 3 geographic regions").
        assert!(TESTNET_REGIONAL_SEEDS.len() >= 3);
        assert!(MAINNET_REGIONAL_SEEDS.len() >= 3);

        let regions = ["us.", "eu.", "ap."];
        for r in regions {
            assert!(
                TESTNET_REGIONAL_SEEDS.iter().any(|s| s.contains(r)),
                "missing testnet region {r}"
            );
            assert!(
                MAINNET_REGIONAL_SEEDS.iter().any(|s| s.contains(r)),
                "missing mainnet region {r}"
            );
        }
    }

    #[test]
    fn regional_seeds_off_by_default() {
        // This test does not set BOTHO_REGIONAL_SEEDS. In a clean environment
        // only the primary seed is returned. (We avoid mutating the process
        // env here to stay robust against parallel test execution.)
        if std::env::var("BOTHO_REGIONAL_SEEDS").is_err() {
            assert_eq!(fallback_seeds(Network::Testnet).len(), 1);
            assert_eq!(fallback_seeds(Network::Mainnet).len(), 1);
        }
    }
}
