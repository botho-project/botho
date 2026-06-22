//! Higher-level reports: threshold-rule comparison and growth/churn timelines,
//! with human-readable table and JSON (serde) rendering.

use crate::{analysis::ThresholdRule, model::Fbas};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// One row of the threshold-rule comparison: a single `(rule, n)` pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThresholdComparisonRow {
    /// Federation size.
    pub n: usize,
    /// Which threshold rule.
    pub rule: ThresholdRule,
    /// The threshold this rule yields at `n`.
    pub threshold: usize,
    /// Min blocking-set cardinality (failures-to-halt; liveness buffer).
    pub min_blocking_set_cardinality: Option<usize>,
    /// Min splitting-set cardinality (faults-to-fork; safety buffer).
    pub min_splitting_set_cardinality: Option<usize>,
    /// Whether disjoint (non-intersecting) quorums exist (fork risk).
    pub disjoint_quorums_exist: bool,
    /// Liveness margin `m − t + 1`, where `m` is the quorum (top-tier) size and
    /// `t` the threshold: how many of the `m` members may be down while a
    /// quorum can still form. (`= m − t + 1`; clamps at 0.)
    pub liveness_margin: usize,
}

/// Compare the threshold rules over a range of federation sizes.
///
/// For each `n` in `sizes` and each rule, build the symmetric `t`-of-`n` FBAS
/// and record its static-health metrics.
pub fn compare_thresholds(sizes: impl IntoIterator<Item = usize>) -> Vec<ThresholdComparisonRow> {
    let rules = [
        ThresholdRule::BothoBft,
        ThresholdRule::TwoThirds,
        ThresholdRule::Unanimity,
    ];
    let mut rows = Vec::new();
    for n in sizes {
        for rule in rules {
            let t = rule.threshold(n);
            let fbas = Fbas::symmetric(n, t);
            let report = fbas.health_report();
            // For a symmetric top-tier the quorum is the whole tier (size n),
            // so the liveness margin is n − t + 1.
            let liveness_margin = (n + 1).saturating_sub(t);
            rows.push(ThresholdComparisonRow {
                n,
                rule,
                threshold: t,
                min_blocking_set_cardinality: report.min_blocking_set_cardinality,
                min_splitting_set_cardinality: report.min_splitting_set_cardinality,
                disjoint_quorums_exist: !report.quorum_intersection,
                liveness_margin,
            });
        }
    }
    rows
}

/// Render the threshold comparison as a human-readable table.
pub fn render_threshold_table(rows: &[ThresholdComparisonRow]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:>3}  {:<30} {:>5} {:>10} {:>10} {:>8} {:>9}",
        "n", "rule", "thr", "blocking", "splitting", "disjoint", "live_marg"
    );
    let _ = writeln!(out, "{}", "-".repeat(82));
    let mut last_n = None;
    for r in rows {
        if last_n.is_some() && last_n != Some(r.n) {
            let _ = writeln!(out);
        }
        last_n = Some(r.n);
        let _ = writeln!(
            out,
            "{:>3}  {:<30} {:>5} {:>10} {:>10} {:>8} {:>9}",
            r.n,
            r.rule.label(),
            r.threshold,
            opt(r.min_blocking_set_cardinality),
            opt(r.min_splitting_set_cardinality),
            if r.disjoint_quorums_exist {
                "YES(fork)"
            } else {
                "no"
            },
            r.liveness_margin,
        );
    }
    out
}

/// One step in a growth/churn timeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChurnStep {
    /// What happened at this step (e.g. "admit C", "shun A", "initial").
    pub action: String,
    /// Federation size after the action.
    pub n: usize,
    /// Min blocking-set cardinality after the action.
    pub min_blocking_set_cardinality: Option<usize>,
    /// Min splitting-set cardinality after the action.
    pub min_splitting_set_cardinality: Option<usize>,
    /// Whether quorum intersection still holds.
    pub quorum_intersection: bool,
    /// Set `true` when this action *broke* quorum intersection (was holding
    /// before, no longer holds after) — a hard safety regression.
    pub broke_quorum_intersection: bool,
}

/// Simulate a sequence of curated admissions / reactive-shuns over a symmetric
/// federation, recording how the safety/liveness buffers evolve and flagging
/// any action that breaks quorum intersection.
///
/// `actions` is a list of [`ChurnAction`]s applied in order, starting from a
/// symmetric Botho-BFT federation of size `initial_n`.
pub fn simulate_churn(initial_n: usize, actions: &[ChurnAction]) -> Vec<ChurnStep> {
    let mut fbas = Fbas::symmetric_botho(initial_n);
    let mut steps = Vec::new();

    let record = |action: String, fbas: &Fbas, prev_qi: bool| -> ChurnStep {
        let report = fbas.health_report();
        let qi = report.quorum_intersection;
        ChurnStep {
            action,
            n: fbas.len(),
            min_blocking_set_cardinality: report.min_blocking_set_cardinality,
            min_splitting_set_cardinality: report.min_splitting_set_cardinality,
            quorum_intersection: qi,
            broke_quorum_intersection: prev_qi && !qi,
        }
    };

    let mut prev_qi = fbas.health_report().quorum_intersection;
    steps.push(record("initial".to_string(), &fbas, prev_qi));
    prev_qi = steps.last().unwrap().quorum_intersection;

    for action in actions {
        let label = match action {
            ChurnAction::AdmitSymmetric => {
                let idx = fbas.admit_symmetric();
                format!("admit {}", fbas.nodes[idx].id)
            }
            ChurnAction::ShunSymmetric(idx) => {
                let id = fbas
                    .nodes
                    .get(*idx)
                    .map(|n| n.id.clone())
                    .unwrap_or_else(|| format!("#{idx}"));
                fbas.shun_symmetric(*idx);
                format!("shun {id}")
            }
        };
        let step = record(label, &fbas, prev_qi);
        prev_qi = step.quorum_intersection;
        steps.push(step);
    }
    steps
}

/// A curated growth/churn action over a symmetric federation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChurnAction {
    /// Admit one validator (re-derives the symmetric Botho-BFT threshold).
    AdmitSymmetric,
    /// Reactively shun the node at the given index.
    ShunSymmetric(usize),
}

/// Render a churn timeline as a human-readable table.
pub fn render_churn_table(steps: &[ChurnStep]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<14} {:>3} {:>10} {:>10} {:>7} {:>8}",
        "action", "n", "blocking", "splitting", "qi", "flag"
    );
    let _ = writeln!(out, "{}", "-".repeat(56));
    for s in steps {
        let _ = writeln!(
            out,
            "{:<14} {:>3} {:>10} {:>10} {:>7} {:>8}",
            s.action,
            s.n,
            opt(s.min_blocking_set_cardinality),
            opt(s.min_splitting_set_cardinality),
            if s.quorum_intersection {
                "ok"
            } else {
                "BROKEN"
            },
            if s.broke_quorum_intersection {
                "FORK!"
            } else {
                ""
            },
        );
    }
    out
}

fn opt(v: Option<usize>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_comparison_flags_unanimity_below_4() {
        let rows = compare_thresholds(2..=4);
        // Botho rule at n=3 is unanimity -> min blocking 1.
        let r = rows
            .iter()
            .find(|r| r.n == 3 && r.rule == ThresholdRule::BothoBft)
            .unwrap();
        assert_eq!(r.threshold, 3);
        assert_eq!(r.min_blocking_set_cardinality, Some(1));
        assert!(!r.disjoint_quorums_exist);
    }

    #[test]
    fn churn_grows_buffers() {
        // 3 -> admit -> 4: blocking buffer goes 1 -> 2.
        let steps = simulate_churn(3, &[ChurnAction::AdmitSymmetric]);
        assert_eq!(steps[0].min_blocking_set_cardinality, Some(1)); // 3-of-3
        assert_eq!(steps[1].min_blocking_set_cardinality, Some(2)); // 3-of-4
        assert!(steps.iter().all(|s| !s.broke_quorum_intersection));
    }

    #[test]
    fn render_smoke() {
        let rows = compare_thresholds(2..=5);
        let table = render_threshold_table(&rows);
        assert!(table.contains("botho_bft"));
        let steps = simulate_churn(4, &[ChurnAction::ShunSymmetric(0)]);
        let t = render_churn_table(&steps);
        assert!(t.contains("shun"));
    }
}
