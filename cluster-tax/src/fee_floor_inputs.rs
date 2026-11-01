//! Pure mirrors of the ledger's DB-dependent fee-floor resolution (#1697).
//!
//! [`crate::consensus_fee_model`] (#1695) is the pure *arithmetic* half of the
//! node's consensus fee floor. It takes three inputs the ledger resolves from
//! its UTXO / cluster-wealth / bridge-import databases first:
//!
//! - the **claimed wealth** — `effective_cluster_wealth_from_outputs`
//!   (`botho/src/ledger/store/validation.rs`): the output-tag-weighted global
//!   cluster wealth, `Σ_out Σ_tag (amount·weight/SCALE)·W[c] / Σ_out amount`;
//! - the **ring-centroid floor** — `ring_centroid_floored_factor`: each ring
//!   member's wealth `Σ_tag weight·W[c]/SCALE`, value-weighted into
//!   [`crate::ring_centroid_implied_factor`];
//! - the **bridge-import floor** — `import_floor_factor_from_outputs`: a
//!   value-weighted blend of `F` on output value tagged to a *recorded* import
//!   cluster and `1×` on the rest.
//!
//! And the per-cluster wealth `W[c]` those read is the cumulative sum the block
//! writer adds for **every output ever created** with a tag on `c`
//! (`update_cluster_wealth_for_output`, `botho/src/ledger/store/writer.rs`):
//! `W[c] += amount·weight/SCALE`, never decremented.
//!
//! This module restates those four ledger functions over plain data (a
//! closure for `W[c]` and for import-set membership) so a simulation can run
//! the node's exact semantics without a database. Nothing here is consensus
//! code: the node does not call it. The golden test
//! `fee_floor_inputs_match_ledger_for_sampled_inputs` in
//! `botho/src/ledger/store.rs` drives the real ledger and these mirrors over
//! the same sampled inputs and fails CI on any drift.

use crate::{
    demurrage::ring_centroid_implied_factor, fee_curve::ClusterFactorCurve,
    BRIDGE_IMPORT_FACTOR_FLOOR, TAG_WEIGHT_SCALE,
};

/// The wallet's per-transfer cluster-tag decay, parts per `TAG_WEIGHT_SCALE`
/// (50,000 = 5%).
///
/// Mirrors `botho::wallet::DEFAULT_CLUSTER_DECAY_RATE` (and the CLI wallet's
/// copy in `botho-wallet/src/transaction.rs`): every ordinary transfer's
/// outputs carry `ClusterTagVector::merge_weighted(inputs, 5%)`. This is
/// wallet policy, not consensus — consensus accepts any non-inflating output
/// tags (`check_cluster_tag_inheritance`, decay rate 0). The drift guard
/// `wallet_decay_rate_matches_cluster_tax_mirror` in `botho/src/wallet.rs`
/// asserts equality.
pub const WALLET_CLUSTER_DECAY_RATE: u32 = 50_000;

/// One cluster-tag entry: `(cluster id, weight in TAG_WEIGHT_SCALE units)`.
pub type TagPair = (u64, u32);

/// One transaction output as the fee-floor resolution sees it.
#[derive(Clone, Copy, Debug)]
pub struct FloorOutput<'a> {
    /// Output amount, picocredits.
    pub amount: u64,
    /// The output's cluster tags.
    pub tags: &'a [TagPair],
}

/// The per-cluster wealth contributions the block writer adds for one output
/// (`update_cluster_wealth_for_output`): `amount·weight/SCALE` per tag, in
/// u128, skipping zero contributions. Apply with `saturating_add`.
pub fn output_wealth_contributions(amount: u64, tags: &[TagPair]) -> Vec<(u64, u128)> {
    tags.iter()
        .filter_map(|&(cluster, weight)| {
            let contribution = (amount as u128) * (weight as u128) / (TAG_WEIGHT_SCALE as u128);
            (contribution > 0).then_some((cluster, contribution))
        })
        .collect()
}

/// `effective_cluster_wealth_from_outputs`: the output-tag-weighted global
/// cluster wealth (the claimed class). Background value contributes zero.
pub fn effective_cluster_wealth_from_outputs(
    outputs: &[FloorOutput<'_>],
    wealth: &dyn Fn(u64) -> u128,
) -> u128 {
    let mut total_weighted_wealth: u128 = 0;
    let mut total_value: u128 = 0;
    for output in outputs {
        total_value = total_value.saturating_add(output.amount as u128);
        for &(cluster, weight) in output.tags {
            let value_fraction =
                (output.amount as u128 * weight as u128) / (TAG_WEIGHT_SCALE as u128);
            total_weighted_wealth = total_weighted_wealth
                .saturating_add(value_fraction.saturating_mul(wealth(cluster)));
        }
    }
    if total_value == 0 {
        return 0;
    }
    total_weighted_wealth / total_value
}

/// `import_floor_factor_from_outputs` with an explicit floor `F` (FACTOR_SCALE
/// units), so a calibration sweep can vary `F`. The ledger always uses
/// [`BRIDGE_IMPORT_FACTOR_FLOOR`]; see [`import_floor_factor_from_outputs`].
pub fn import_floor_factor_from_outputs_at(
    outputs: &[FloorOutput<'_>],
    is_import: &dyn Fn(u64) -> bool,
    floor: u64,
) -> u64 {
    let background = ClusterFactorCurve::FACTOR_SCALE as u128;
    let floor = floor as u128;
    let mut total_value: u128 = 0;
    let mut import_value: u128 = 0;
    for output in outputs {
        let amount = output.amount as u128;
        total_value = total_value.saturating_add(amount);
        for &(cluster, weight) in output.tags {
            if is_import(cluster) {
                let tagged = amount.saturating_mul(weight as u128) / (TAG_WEIGHT_SCALE as u128);
                import_value = import_value.saturating_add(tagged);
            }
        }
    }
    if total_value == 0 {
        return ClusterFactorCurve::FACTOR_SCALE;
    }
    let import_value = import_value.min(total_value);
    let background_value = total_value - import_value;
    ((floor
        .saturating_mul(import_value)
        .saturating_add(background.saturating_mul(background_value)))
        / total_value) as u64
}

/// `import_floor_factor_from_outputs` at the ratified floor `F`.
pub fn import_floor_factor_from_outputs(
    outputs: &[FloorOutput<'_>],
    is_import: &dyn Fn(u64) -> bool,
) -> u64 {
    import_floor_factor_from_outputs_at(outputs, is_import, BRIDGE_IMPORT_FACTOR_FLOOR)
}

/// One ring member's effective wealth, as `ring_centroid_floored_factor`
/// resolves it: `Σ_tag weight·W[c]/SCALE` (saturating).
pub fn ring_member_wealth(tags: &[TagPair], wealth: &dyn Fn(u64) -> u128) -> u128 {
    tags.iter().fold(0u128, |acc, &(cluster, weight)| {
        acc.saturating_add(
            (weight as u128).saturating_mul(wealth(cluster)) / TAG_WEIGHT_SCALE as u128,
        )
    })
}

/// The ring-centroid-implied factor over `(value, tags)` ring members — the
/// `ring_centroid_implied_factor` half of `ring_centroid_floored_factor`
/// (the ledger then takes `max(claimed, implied)`).
pub fn ring_centroid_factor(
    ring: &[(u64, &[TagPair])],
    wealth: &dyn Fn(u64) -> u128,
    curve: &ClusterFactorCurve,
) -> u64 {
    let members: Vec<(u64, u128)> = ring
        .iter()
        .map(|&(value, tags)| (value, ring_member_wealth(tags, wealth)))
        .collect();
    ring_centroid_implied_factor(&members, curve)
}

/// Everything `Ledger::consensus_fee_floor` resolves from its databases before
/// the pure fee arithmetic, plus the composed demurrage factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedFeeFloor {
    /// Output-tag effective cluster wealth (the claimed class), picocredits.
    pub claimed_wealth: u128,
    /// `curve(claimed_wealth)`, FACTOR_SCALE.
    pub claimed_factor: u64,
    /// Ring-centroid-implied factor, FACTOR_SCALE.
    pub ring_floor: u64,
    /// Bridge-import floor blend on the output tags, FACTOR_SCALE.
    pub import_floor: u64,
    /// The composed input-class floor `max(ring_floor, import_floor)` —
    /// [`crate::ConsensusFeeInputs::floor`].
    pub floor: u64,
    /// The demurrage factor the ledger charges at:
    /// `max(claimed_factor, ring_floor, import_floor)`.
    pub demurrage_factor: u64,
}

/// Resolve the fee-floor inputs exactly as `Ledger::consensus_fee_floor` does,
/// with an explicit import floor `F` (the ledger uses
/// [`BRIDGE_IMPORT_FACTOR_FLOOR`]).
pub fn resolve_fee_floor_at(
    outputs: &[FloorOutput<'_>],
    ring: &[(u64, &[TagPair])],
    wealth: &dyn Fn(u64) -> u128,
    is_import: &dyn Fn(u64) -> bool,
    import_floor_f: u64,
) -> ResolvedFeeFloor {
    let curve = ClusterFactorCurve::default_params();
    let claimed_wealth = effective_cluster_wealth_from_outputs(outputs, wealth);
    let claimed_factor = curve.factor(claimed_wealth);
    let ring_floor = ring_centroid_factor(ring, wealth, &curve);
    let import_floor = import_floor_factor_from_outputs_at(outputs, is_import, import_floor_f);
    let floor = ring_floor.max(import_floor);
    ResolvedFeeFloor {
        claimed_wealth,
        claimed_factor,
        ring_floor,
        import_floor,
        floor,
        demurrage_factor: claimed_factor.max(floor),
    }
}

/// [`resolve_fee_floor_at`] at the ratified floor `F`.
pub fn resolve_fee_floor(
    outputs: &[FloorOutput<'_>],
    ring: &[(u64, &[TagPair])],
    wealth: &dyn Fn(u64) -> u128,
    is_import: &dyn Fn(u64) -> bool,
) -> ResolvedFeeFloor {
    resolve_fee_floor_at(outputs, ring, wealth, is_import, BRIDGE_IMPORT_FACTOR_FLOOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fee_curve::{FeeConfig, PICO_PER_BTH};

    #[test]
    fn claimed_factor_matches_fee_config_cluster_factor() {
        // The ledger computes the claimed factor via FeeConfig::default().
        let w = |c: u64| if c == 9 { 250_000 * PICO_PER_BTH } else { 0 };
        let tags = [(9u64, 600_000u32)];
        let outs = [FloorOutput {
            amount: 10 * PICO_PER_BTH as u64,
            tags: &tags,
        }];
        let r = resolve_fee_floor(&outs, &[], &w, &|_| false);
        assert_eq!(
            r.claimed_factor,
            FeeConfig::default().cluster_factor(r.claimed_wealth)
        );
        assert_eq!(r.claimed_wealth, 150_000 * PICO_PER_BTH);
    }

    #[test]
    fn import_floor_blends_f_on_import_value_only() {
        let tags = [(1u64, 500_000u32)];
        let outs = [FloorOutput {
            amount: 1_000,
            tags: &tags,
        }];
        // 50% import-tagged: (F + 1x) / 2.
        let f = import_floor_factor_from_outputs(&outs, &|c| c == 1);
        assert_eq!(f, (BRIDGE_IMPORT_FACTOR_FLOOR + 1_000) / 2);
        // Not a recorded import cluster: 1x.
        assert_eq!(import_floor_factor_from_outputs(&outs, &|_| false), 1_000);
        // No outputs: 1x.
        assert_eq!(import_floor_factor_from_outputs(&[], &|_| true), 1_000);
    }

    #[test]
    fn writer_contributions_skip_zero() {
        let tags = [(1u64, 1u32), (2u64, 500_000u32)];
        assert_eq!(output_wealth_contributions(10, &tags), vec![(2, 5)]);
    }
}
