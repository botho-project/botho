//! Bridge-import cluster tagging (ADR 0007).
//!
//! Unwrapping wBTH → BTH mints BTH into a **block-epoch import cluster**
//! instead of returning it at factor-1 (background). Every unwrap whose output
//! lands in block-height range `[mK, (m+1)K)` joins one shared cluster origin
//!
//! ```text
//! c_import(m) = H("bridge-import" ‖ m),   m = ⌊height / K⌋
//! ```
//!
//! The import cluster accumulates wealth (the sum of the unwrap amounts tagged
//! to it) exactly like any domestic cluster, and the production
//! [`crate::ClusterFactorCurve`] maps that wealth to a factor. The effective
//! factor of an import cluster is then clamped to a floor `F`:
//!
//! ```text
//! import_factor(m) = max(F, ClusterFactorCurve(Σ unwrap amounts in epoch m))
//! ```
//!
//! This module is the single, consensus-critical source of the two ratified
//! constants (`K`, `F`), the epoch key, the deterministic import-cluster-id
//! derivation, and the floor clamp. Both the ledger (consensus enforcement) and
//! the bridge service (tag assignment on release) depend on it, so the id
//! derivation and clamp can never drift between the two.
//!
//! # Why the floor is a **separate** clamp from the ring-centroid floor
//!
//! The consensus fee floor already raises a spender-claimed factor to the
//! ring-centroid-implied factor via `max(claimed, ring_centroid)` (audit
//! cycle-6 H2). The import floor is a DIFFERENT `max`: it clamps *the import
//! cluster's own curve-derived factor* up to `F`. Because both are `max`
//! operations against independent lower bounds, composing them is just a wider
//! `max` — there is no double-floor: `max(claimed, ring_centroid, F_import)`
//! collapses to a single dominating bound. The import floor only ever RAISES a
//! factor, and only for wealth that actually traces to an import cluster; a
//! purely-domestic coin never touches it.
//!
//! # Determinism
//!
//! CONSENSUS-CRITICAL. `import_epoch` is integer division; `import_cluster_id`
//! is SHA-256 over a fixed-format preimage (mirroring how
//! `MintingTx::to_tx_output` derives a mint `ClusterId` from a hash — first 8
//! little-endian bytes); the floor clamp is a `max` on `u64` FACTOR_SCALE
//! units. No floats, bit-identical on every platform.

use sha2::{Digest, Sha256};

use crate::{ClusterFactorCurve, ClusterId};

/// Epoch length `K`, in blocks (ADR 0007, ratified 2026-07-14 via #937/#940).
///
/// `K = 17_280` blocks = 1 day at the 5 s reference block time
/// (`86_400 s / 5 s`). All unwraps whose output is created in block-height
/// range `[mK, (m+1)K)` share the single import cluster `c_import(m)`, so
/// intra-epoch splitting piles into one accumulating pool (Sybil resistance).
///
/// A future maintainer preferring minimal co-location collateral over
/// operational legibility may lower this to `10_080` (~14 h); the calibration
/// sim (`simulation::bridge_import_sweep`) regenerates the numbers. Changing it
/// is a consensus change (new import-cluster ids ⇒ protocol bump + reset).
pub const BRIDGE_IMPORT_EPOCH_BLOCKS: u64 = 17_280;

/// Import-factor floor `F`, in [`ClusterFactorCurve::FACTOR_SCALE`] units
/// (1000 = 1.0×), ratified 2026-07-14 (#937/#940).
///
/// `F = 1_500` = 1.5×. This is the residual anti-hoarding premium a split-gamer
/// cannot erode (the best factor reachable by diluting across epochs is `F`,
/// not 1×) and the minimum toll for entering via the bridge rather than earning
/// domestically. It clears the ~1.27× a genuine ~1000-BTH retail import already
/// prices on the raw curve (so the floor binds) at a ~0.20 %/yr transient
/// onboarding toll that blends off in ≈9 domestic spends.
pub const BRIDGE_IMPORT_FACTOR_FLOOR: u64 = 1_500;

/// Domain-separation tag for the import-cluster-id hash preimage.
const BRIDGE_IMPORT_DOMAIN: &[u8] = b"bridge-import";

/// The block-height epoch `m = ⌊height / K⌋` an unwrap at `height` belongs to.
///
/// Integer division; deterministic. All unwraps in `[mK, (m+1)K)` map to the
/// same `m` and therefore the same import cluster.
#[inline]
pub fn import_epoch(height: u64) -> u64 {
    height / BRIDGE_IMPORT_EPOCH_BLOCKS
}

/// The canonical import cluster id for epoch `m`:
/// `c_import(m) = H("bridge-import" ‖ m)`.
///
/// The preimage is `BRIDGE_IMPORT_DOMAIN ‖ m.to_le_bytes()`; the id is the
/// first 8 little-endian bytes of the SHA-256 digest, folded into the `u64`
/// cluster-id space — the identical convention `MintingTx::to_tx_output` uses
/// to derive a mint cluster id from a tx hash. A bridge-import cluster is
/// simply a third way to create a cluster origin (alongside minting), with no
/// new machinery beyond the tag.
pub fn import_cluster_id(epoch: u64) -> ClusterId {
    let mut hasher = Sha256::new();
    hasher.update(BRIDGE_IMPORT_DOMAIN);
    hasher.update(epoch.to_le_bytes());
    let digest = hasher.finalize();
    let id = u64::from_le_bytes(digest[0..8].try_into().unwrap());
    ClusterId::new(id)
}

/// The import cluster id for an unwrap whose output is created at `height`:
/// `import_cluster_id(import_epoch(height))`.
#[inline]
pub fn import_cluster_id_for_height(height: u64) -> ClusterId {
    import_cluster_id(import_epoch(height))
}

/// Apply the import-factor floor to a factor known to belong to an import
/// cluster.
///
/// `curve_factor` is the import cluster's curve-derived factor
/// (`ClusterFactorCurve::factor(import_cluster_wealth)`), in FACTOR_SCALE
/// units. Returns `max(curve_factor, F)`. Never lowers a factor; a flood epoch
/// whose curve factor already exceeds `F` is unchanged.
#[inline]
pub fn apply_import_floor(curve_factor: u64) -> u64 {
    curve_factor.max(BRIDGE_IMPORT_FACTOR_FLOOR)
}

/// The effective factor of an import cluster with the given accumulated wealth:
/// `max(F, ClusterFactorCurve(wealth))`.
///
/// This is the consensus-canonical import factor — the identical expression the
/// calibration sim (`simulation::bridge_import_sweep::import_factor`) models,
/// computed here from the real curve so the ledger and the sim agree.
pub fn import_cluster_factor(import_cluster_wealth_pico: u128, curve: &ClusterFactorCurve) -> u64 {
    apply_import_floor(curve.factor(import_cluster_wealth_pico))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PICO_PER_BTH;

    #[test]
    fn epoch_boundaries_are_k_blocks() {
        assert_eq!(import_epoch(0), 0);
        assert_eq!(import_epoch(BRIDGE_IMPORT_EPOCH_BLOCKS - 1), 0);
        assert_eq!(import_epoch(BRIDGE_IMPORT_EPOCH_BLOCKS), 1);
        assert_eq!(import_epoch(2 * BRIDGE_IMPORT_EPOCH_BLOCKS + 5), 2);
    }

    #[test]
    fn constants_match_ratified_values() {
        // ADR 0007: K = 17,280 blocks (1 day @ 5s), F = 1.5x.
        assert_eq!(BRIDGE_IMPORT_EPOCH_BLOCKS, 17_280);
        assert_eq!(BRIDGE_IMPORT_FACTOR_FLOOR, 1_500);
        assert_eq!(
            BRIDGE_IMPORT_FACTOR_FLOOR,
            ClusterFactorCurve::FACTOR_SCALE * 3 / 2
        );
    }

    #[test]
    fn import_cluster_id_is_deterministic_and_epoch_distinct() {
        // Deterministic: the same epoch always yields the same id.
        assert_eq!(import_cluster_id(7), import_cluster_id(7));
        // Distinct across epochs (defeats the drip-split: each epoch is its own
        // pool, but two unwraps in the SAME epoch share one).
        assert_ne!(import_cluster_id(7), import_cluster_id(8));
        assert_ne!(import_cluster_id(0), import_cluster_id(1));
    }

    #[test]
    fn two_unwraps_same_epoch_share_one_cluster() {
        // Heights inside the same epoch window map to the same cluster id.
        let h1 = 3 * BRIDGE_IMPORT_EPOCH_BLOCKS + 1;
        let h2 = 3 * BRIDGE_IMPORT_EPOCH_BLOCKS + BRIDGE_IMPORT_EPOCH_BLOCKS - 1;
        assert_eq!(
            import_cluster_id_for_height(h1),
            import_cluster_id_for_height(h2)
        );
    }

    #[test]
    fn unwraps_in_different_epochs_form_distinct_clusters() {
        let h1 = 3 * BRIDGE_IMPORT_EPOCH_BLOCKS + 1;
        let h2 = 4 * BRIDGE_IMPORT_EPOCH_BLOCKS + 1;
        assert_ne!(
            import_cluster_id_for_height(h1),
            import_cluster_id_for_height(h2)
        );
    }

    #[test]
    fn floor_binds_small_import_and_yields_to_flood() {
        let curve = ClusterFactorCurve::default_params();

        // A small import sits below the curve knee → floored to exactly F.
        let small = 1_000u128 * PICO_PER_BTH;
        assert_eq!(
            import_cluster_factor(small, &curve),
            BRIDGE_IMPORT_FACTOR_FLOOR
        );

        // A flood import saturates the curve well above F → floor does not bind.
        let flood = 10_000_000u128 * PICO_PER_BTH;
        let flood_factor = import_cluster_factor(flood, &curve);
        assert!(
            flood_factor > 5_000,
            "flood import must price near 6x, got {flood_factor}"
        );
        assert_eq!(
            flood_factor,
            curve.factor(flood),
            "floor must not alter a flood factor"
        );
    }

    #[test]
    fn apply_import_floor_never_lowers() {
        assert_eq!(apply_import_floor(0), BRIDGE_IMPORT_FACTOR_FLOOR);
        assert_eq!(apply_import_floor(1_000), BRIDGE_IMPORT_FACTOR_FLOOR);
        assert_eq!(
            apply_import_floor(BRIDGE_IMPORT_FACTOR_FLOOR),
            BRIDGE_IMPORT_FACTOR_FLOOR
        );
        assert_eq!(apply_import_floor(6_000), 6_000);
    }
}
