//! Tests and vector generation; see the parent module.

use super::*;

const VECTORS: &str = include_str!("../../../formal/Botho/Difficulty/Vectors.lean");

/// The lines of the `name` vector block, between `-- BEGIN name` and
/// `-- END name`, that hold a vector (start with `(`).
fn block_lines<'a>(name: &str) -> Vec<&'a str> {
    let begin = format!("-- BEGIN {name}");
    let end = format!("-- END {name}");
    let mut inside = false;
    let mut out = Vec::new();
    for line in VECTORS.lines() {
        let t = line.trim();
        if t == begin {
            inside = true;
        } else if t == end {
            inside = false;
        } else if inside && t.starts_with('(') {
            out.push(t);
        }
    }
    assert!(!out.is_empty(), "no vectors in block {name}");
    out
}

/// Split a vector line such as `(true, 20, [(1, 2), (3, 4)], 40),` into its
/// scalar tokens: `["true", "20", "1", "2", "3", "4", "40"]`.
fn tokens(line: &str) -> Vec<&str> {
    line.split(['(', ')', '[', ']', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

fn num(s: &str) -> u64 {
    s.parse()
        .unwrap_or_else(|e| panic!("bad number {s:?} in Vectors.lean: {e}"))
}

#[test]
fn step_vectors_match_rust() {
    let lines = block_lines("stepVectors");
    for line in &lines {
        let t = tokens(line);
        assert_eq!(t.len(), 4, "malformed step vector {line}");
        let (c, o, tg, want) = (num(t[0]), num(t[1]), num(t[2]), num(t[3]));
        assert_eq!(
            step_rust(c, o, tg),
            want,
            "compute_time_adjusted_difficulty({c}, {o}, {tg}) disagrees with the Lean model"
        );
    }
    assert!(
        lines.len() >= 500,
        "step vector set shrank: {}",
        lines.len()
    );
}

#[test]
fn run_vectors_match_rust() {
    let lines = block_lines("runVectors");
    for line in &lines {
        let t = tokens(line);
        assert!(t.len() >= 2, "malformed run vector {line}");
        let d0 = num(t[0]);
        let want = num(t[t.len() - 1]);
        let gaps: Vec<u64> = t[1..t.len() - 1].iter().map(|s| num(s)).collect();
        assert_eq!(
            run_rust(d0, &gaps),
            want,
            "record_block trajectory from {d0} over {gaps:?} disagrees with the Lean model"
        );
    }
}

#[test]
fn slot_vectors_match_rust() {
    let lines = block_lines("slotVectors");
    for line in &lines {
        let t = tokens(line);
        assert!(
            t.len() >= 3 && t.len() % 2 == 1,
            "malformed slot vector {line}"
        );
        let dynamic = match t[0] {
            "true" => true,
            "false" => false,
            other => panic!("bad bool {other:?}"),
        };
        let cfg = num(t[1]);
        let want = num(t[t.len() - 1]);
        let feed: Vec<(u64, u64)> = t[2..t.len() - 1]
            .chunks(2)
            .map(|p| (num(p[0]), num(p[1])))
            .collect();
        assert_eq!(
            slot_rust(dynamic, cfg, &feed),
            want,
            "current_slot_duration(dynamic={dynamic}, cfg={cfg}, feed={feed:?}) disagrees with the Lean model"
        );
    }
}

/// The #1842 numbers, checked on the Rust controller kernel directly: from
/// `INITIAL_DIFFICULTY`, 8 blocks at the 10/20/40 s pacing tiers reach
/// `MAX_DIFFICULTY` (Lean: `easing_to_max_in_8`, `no_max_in_7`), and 55
/// zero gaps reach the floor (Lean: `time_warp_to_floor_in_55`,
/// `no_floor_in_54`); from `MAX_DIFFICULTY` the floor takes 63.
///
/// The zero-gap counts are about the kernel. Under protocol <= 7.x the node
/// fed it a zero gap for every equal-timestamp block (finding #1848); since
/// 8.0.0 (#1869) the feed floors gaps at 3 s, see
/// `production_feed_floors_gaps_post_1848`.
#[test]
fn headline_counts_match_lean_theorems() {
    use crate::{block::difficulty::MAX_DIFFICULTY, node::minter::INITIAL_DIFFICULTY};
    for gaps in [[10u64; 8], [20; 8], [40; 8], [7200; 8]] {
        assert_eq!(run_rust(INITIAL_DIFFICULTY, &gaps), MAX_DIFFICULTY);
        assert!(run_rust(INITIAL_DIFFICULTY, &gaps[..7]) < MAX_DIFFICULTY);
    }
    let mixed = [10, 40, 33, 20, 12, 40, 10, 25];
    assert_eq!(run_rust(INITIAL_DIFFICULTY, &mixed), MAX_DIFFICULTY);
    for (gap, n) in [(9u64, 10usize), (8, 12), (7, 17), (6, 31)] {
        assert!(run_rust(INITIAL_DIFFICULTY, &vec![gap; n - 1]) < MAX_DIFFICULTY);
        assert_eq!(run_rust(INITIAL_DIFFICULTY, &vec![gap; n]), MAX_DIFFICULTY);
    }
    assert_eq!(run_rust(INITIAL_DIFFICULTY, &[0; 54]), 2);
    assert_eq!(run_rust(INITIAL_DIFFICULTY, &[0; 55]), 1);
    assert_eq!(run_rust(MAX_DIFFICULTY, &[0; 62]), 2);
    assert_eq!(run_rust(MAX_DIFFICULTY, &[0; 63]), 1);
    // At the ceiling, a slow block reaches the `unwrap_or(u64::MAX)` branch
    // (2 * MAX > u64::MAX) and is clamped back (Lean: `step_truncates_at_max`).
    assert!((MAX_DIFFICULTY as u128) * 2 > u64::MAX as u128);
    assert_eq!(step_rust(MAX_DIFFICULTY, 60, 5), MAX_DIFFICULTY);
}

/// The protocol 8.0.0 difficulty feed (#1869): `chain_rust` (stamps through
/// `observed_block_time`, as the node commits blocks) equals the kernel
/// trajectory `run_rust` over the gaps floored at 3 s, so the DRT-tested
/// kernel plus this floor is the production path. Equal and backward stamps
/// no longer give the 7.x zero-gap halving: 55 equal stamps, which took
/// `INITIAL_DIFFICULTY` to the floor under 7.x (Lean
/// `time_warp_to_floor_in_55`), now leave it far above the floor.
#[test]
fn production_feed_floors_gaps_post_1848() {
    use crate::{
        block::difficulty::{MAX_DIFFICULTY, MIN_OBSERVED_BLOCK_TIME_SECS},
        node::minter::INITIAL_DIFFICULTY,
    };
    assert_eq!(MIN_OBSERVED_BLOCK_TIME_SECS, 3);
    let p = 1_700_000_000u64;
    let mut rng = Rng(1848);
    for _ in 0..500 {
        let d0 = match rng.below(3) {
            0 => INITIAL_DIFFICULTY,
            1 => MAX_DIFFICULTY,
            _ => 1 + rng.below(MAX_DIFFICULTY),
        };
        let mut ts = p;
        let stamps: Vec<u64> = (0..rng.below(40))
            .map(|_| {
                ts = match rng.below(5) {
                    0 => ts,
                    1 => ts.saturating_sub(rng.below(50)),
                    2 => ts + rng.pick(&[3, 5, 10, 20, 40, 60]),
                    _ => ts + rng.below(100),
                };
                ts
            })
            .collect();
        let mut parent = p;
        let floored: Vec<u64> = stamps
            .iter()
            .map(|&t| {
                let g = t.saturating_sub(parent).max(MIN_OBSERVED_BLOCK_TIME_SECS);
                parent = t;
                g
            })
            .collect();
        assert_eq!(
            chain_rust(d0, p, &stamps),
            run_rust(d0, &floored),
            "production feed from {d0} over {stamps:?}"
        );
    }
    // Genesis parent (0): no signal, difficulty unchanged for that block.
    assert_eq!(chain_rust(INITIAL_DIFFICULTY, 0, &[p]), INITIAL_DIFFICULTY);
    // The 7.x time warp is gone: 55 equal stamps are 55 floored 3 s gaps.
    let warped = chain_rust(INITIAL_DIFFICULTY, p, &[p; 55]);
    assert_eq!(warped, run_rust(INITIAL_DIFFICULTY, &[3; 55]));
    assert!(
        warped > 1,
        "55 equal stamps must not reach the floor post-#1848"
    );
    assert_eq!(
        run_rust(INITIAL_DIFFICULTY, &[0; 55]),
        1,
        "kernel unchanged"
    );
}

/// Post-fix witness for the Lean counterexample
/// `Botho.Difficulty.forward_date_locks_out_honest` (#1841, finding #1848).
///
/// History: under protocol <= 7.x the validator's lower timestamp bound was
/// the parent (`timestamp >= tip_timestamp`) and the future bound was 2 h, so
/// one tip stamped up to 2 h ahead of the wall clock made every honest
/// `now` stamp invalid (`TimestampBeforeParent`); that is what the Lean
/// theorem proves about the 7.x rule, and what this test used to witness on
/// the real validator. Fixed in protocol 8.0.0 (#1869): the lower bound is
/// `timestamp > MTP(last 11)` and the future bound is 60 s.
///
/// This test now pins the post-fix behaviour on the real validator:
///
/// - a 7.x-style 2 h forward-dated stamp is rejected outright by the 60 s
///   future bound;
/// - a tip forward-dated as far as 8.0.0 allows (`now + 50`) on top of an
///   honest window does not move the median, so the honest minter's stamp
///   `max(now, MTP + 1)` (`node/minter.rs`) is ACCEPTED, even though it is
///   before the parent;
/// - the forward-dated producer's next stamp still passes too: it gains no
///   exclusive right to mint.
#[test]
fn forward_dated_tip_accepts_honest_minting_tx_post_1848() {
    use crate::{
        block::{calculate_block_reward, MintingTx},
        consensus::{TransactionValidator, ValidationError},
    };
    use std::sync::{Arc, RwLock};
    // `consensus::validation::ML_KEM_768_CIPHERTEXT_BYTES` (module-private).
    const ML_KEM_768_CIPHERTEXT_BYTES: usize = 1088;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs();
    // Ten honest blocks 5 s apart ending at `now - 5`, then a tip stamped
    // 50 s ahead (inside the 8.0.0 60 s bound: valid when accepted).
    let tip_timestamp = now + 50;
    let mut recent_timestamps: Vec<u64> = (0..10).map(|i| now - 50 + 5 * i).collect();
    recent_timestamps.push(tip_timestamp);
    let state = ChainState {
        height: 10,
        tip_hash: [7u8; 32],
        tip_timestamp,
        difficulty: u64::MAX,
        total_mined: 1_000_000_000_000,
        current_reward: crate::block::difficulty::INITIAL_REWARD,
        recent_timestamps,
        ..ChainState::default()
    };
    let mtp = state.median_time_past();
    let honest_stamp = now.max(state.next_min_timestamp());
    assert!(mtp < now, "one forward-dated tip cannot move the median");
    assert_eq!(honest_stamp, now);
    assert!(
        honest_stamp < state.tip_timestamp,
        "honest stamp is before the parent"
    );

    let reward = calculate_block_reward(11, state.total_mined);
    let validator = TransactionValidator::new(Arc::new(RwLock::new(state)));
    let tx_at = |timestamp: u64| MintingTx {
        block_height: 11,
        reward,
        minter_view_key: [0u8; 32],
        minter_spend_key: [0u8; 32],
        target_key: [0u8; 32],
        public_key: [0u8; 32],
        kem_ciphertext: Some(vec![0u8; ML_KEM_768_CIPHERTEXT_BYTES]),
        prev_block_hash: [7u8; 32],
        difficulty: u64::MAX,
        nonce: 0,
        timestamp,
    };

    // Honest minter: stamped `max(now, MTP + 1)` -> accepted (difficulty
    // u64::MAX makes any PoW hash valid, so every other check passes too).
    assert_eq!(
        validator.validate_minting_tx(&tx_at(honest_stamp)),
        Ok(()),
        "post-#1848 the honest minting tx must be accepted on a forward-dated tip"
    );
    // The forward-dated producer stamping at its tip still passes as well.
    assert_eq!(
        validator.validate_minting_tx(&tx_at(tip_timestamp + 5)),
        Ok(())
    );
    // The 7.x attack offset (7000 s ahead, inside the old 2 h bound) is now
    // out of bounds.
    assert_eq!(
        validator.validate_minting_tx(&tx_at(now + 7000)),
        Err(ValidationError::TimestampTooFarInFuture)
    );
}

// ---------------------------------------------------------------------------
// Vector generation (ignored; run by hand, see the module docs).
// ---------------------------------------------------------------------------

/// SplitMix64, so the vector set is reproducible without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick(&mut self, xs: &[u64]) -> u64 {
        xs[self.below(xs.len() as u64) as usize]
    }
}

fn gen_step_vectors(rng: &mut Rng) -> Vec<(u64, u64, u64, u64)> {
    use crate::{block::difficulty::MAX_DIFFICULTY, node::minter::INITIAL_DIFFICULTY};
    let cs = [
        0,
        1,
        2,
        3,
        4,
        5,
        INITIAL_DIFFICULTY,
        MAX_DIFFICULTY / 2,
        MAX_DIFFICULTY / 2 + 1,
        (1 << 63) - 1,
        1 << 63,
        MAX_DIFFICULTY - 1,
        MAX_DIFFICULTY,
        MAX_DIFFICULTY + 1,
        u64::MAX - 1,
        u64::MAX,
    ];
    let os = [
        0,
        1,
        2,
        3,
        4,
        5,
        6,
        7,
        8,
        9,
        10,
        11,
        59,
        60,
        61,
        7200,
        u64::MAX / 10_000,
        u64::MAX,
    ];
    let ts = [
        0,
        1,
        3,
        5,
        10,
        40,
        u64::MAX / 12,
        u64::MAX / 12 + 1,
        u64::MAX,
    ];
    let mut out = Vec::new();
    // Every edge `current` x every edge `observed` at the live target.
    for &c in &cs {
        for &o in &os {
            out.push((c, o, 5));
        }
    }
    // Random mixes, including random targets.
    for _ in 0..400 {
        let c = match rng.below(3) {
            0 => rng.pick(&cs),
            1 => rng.next(),
            _ => rng.below(MAX_DIFFICULTY + 2),
        };
        let o = match rng.below(3) {
            0 => rng.pick(&os),
            1 => rng.below(200),
            _ => rng.next(),
        };
        let t = match rng.below(4) {
            0 => rng.pick(&ts),
            1 => rng.below(100),
            2 => rng.next(),
            _ => 5,
        };
        out.push((c, o, t));
    }
    out.into_iter()
        .map(|(c, o, t)| (c, o, t, step_rust(c, o, t)))
        .collect()
}

fn gen_run_vectors(rng: &mut Rng) -> Vec<(u64, Vec<u64>, u64)> {
    use crate::{block::difficulty::MAX_DIFFICULTY, node::minter::INITIAL_DIFFICULTY};
    let mut out = Vec::new();
    for _ in 0..120 {
        let d0 = match rng.below(4) {
            0 => INITIAL_DIFFICULTY,
            1 => MAX_DIFFICULTY,
            2 => 1 + rng.below(1000),
            _ => 1 + rng.below(MAX_DIFFICULTY),
        };
        let n = rng.below(40) as usize;
        let gaps: Vec<u64> = (0..n)
            .map(|_| match rng.below(5) {
                0 => 0,
                1 => rng.pick(&[3, 5, 10, 20, 40]),
                2 => rng.below(12),
                3 => rng.below(100),
                _ => rng.pick(&[60, 61, 7200, u64::MAX]),
            })
            .collect();
        let r = run_rust(d0, &gaps);
        out.push((d0, gaps, r));
    }
    out
}

fn gen_slot_vectors(rng: &mut Rng) -> Vec<(bool, u64, Vec<(u64, u64)>, u64)> {
    let mut out = Vec::new();
    for i in 0..300 {
        let dynamic = i % 10 != 0;
        let cfg = rng.pick(&[1, 3, 20, 40, 7]);
        let n = rng.below(14) as usize;
        let mut ts = 1_700_000_000 + rng.below(1_000_000);
        let mut feed: Vec<(u64, u64)> = Vec::new();
        for _ in 0..n {
            ts = match rng.below(8) {
                0 => ts,                                 // equal to parent
                1 => ts.saturating_sub(rng.below(50)),   // out of order
                2 => ts + 7200,                          // forward-dated (7.x 2 h bound)
                3 => ts + rng.pick(&[3, 5, 10, 20, 40]), // tier pacing
                _ => ts + rng.below(60),
            };
            feed.push((ts, rng.below(30)));
        }
        // Retune the surviving window's tx total to sit on or next to a
        // threshold, so every tier boundary is exercised.
        let keep = n.min(10);
        if keep >= 2 && rng.below(3) != 0 {
            let window = &feed[n - keep..];
            let w = window[keep - 1].0.saturating_sub(window[0].0);
            let base = match rng.below(4) {
                0 => 20 * w,
                1 => 5 * w,
                2 => w,
                _ => w.div_ceil(5),
            };
            let total = (base + rng.below(3)).saturating_sub(1);
            let per = total / keep as u64;
            let rem = total % keep as u64;
            for (j, entry) in feed[n - keep..].iter_mut().enumerate() {
                entry.1 = per + u64::from((j as u64) < rem);
            }
        }
        let r = slot_rust(dynamic, cfg, &feed);
        out.push((dynamic, cfg, feed, r));
    }
    out
}

fn render(
    step: &[(u64, u64, u64, u64)],
    run: &[(u64, Vec<u64>, u64)],
    slot: &[(bool, u64, Vec<(u64, u64)>, u64)],
) -> String {
    fn list<T>(items: &[T], f: impl Fn(&T) -> String) -> String {
        let body: Vec<String> = items.iter().map(|x| format!("  {}", f(x))).collect();
        format!("[\n{}]", body.join(",\n"))
    }
    let join = |xs: &[u64]| xs.iter().map(u64::to_string).collect::<Vec<_>>().join(", ");
    let mut s = String::new();
    s.push_str(HEADER);
    s.push_str("-- BEGIN stepVectors\ndef stepVectors : List (Nat × Nat × Nat × Nat) := ");
    s.push_str(&list(step, |(c, o, t, r)| format!("({c}, {o}, {t}, {r})")));
    s.push_str("\n-- END stepVectors\n\n");
    s.push_str("-- BEGIN runVectors\ndef runVectors : List (Nat × List Nat × Nat) := ");
    s.push_str(&list(run, |(d, g, r)| format!("({d}, [{}], {r})", join(g))));
    s.push_str("\n-- END runVectors\n\n");
    s.push_str(
        "-- BEGIN slotVectors\ndef slotVectors : List (Bool × Nat × List (Nat × Nat) × Nat) := ",
    );
    s.push_str(&list(slot, |(d, c, f, r)| {
        let feed: Vec<String> = f.iter().map(|(t, x)| format!("({t}, {x})")).collect();
        format!("({d}, {c}, [{}], {r})", feed.join(", "))
    }));
    s.push_str("\n-- END slotVectors\n");
    s.push_str(FOOTER);
    s
}

const HEADER: &str = "/-
DRT vectors for the difficulty step, the `record_block` trajectory and the
dynamic slot duration (#1841). GENERATED by the Rust test
`formal_drt_difficulty::tests::regenerate_formal_vectors` (`botho/src/formal_drt_difficulty.rs`)
from the Rust code; do not edit by hand.

Each expected output is the RUST output. The theorems at the end make the
Lean kernel check that the MODEL gives the same output on every vector, and
the Rust tests `step_vectors_match_rust`, `run_vectors_match_rust` and
`slot_vectors_match_rust` re-check the Rust side against this file. Label:
**tested** (evidence that model and code agree, not a proof). These theorems
are deliberately not listed in `Theorems.lean`: they would make the statement
record carry the whole vector table, and they claim nothing beyond agreement
on these inputs. `lake build` and the verify job's kernel replay still check
them.
-/
import Botho.Difficulty.Warp
import Botho.Consensus.SlotDuration

namespace Botho.Vectors

open Botho.Difficulty Botho.Consensus

-- The list literals and the `decide` evaluations below recurse once per
-- vector; the default depth (512) is too small for these tables.
set_option maxRecDepth 200000

";

const FOOTER: &str = "
def checkStep : List (Nat × Nat × Nat × Nat) → Bool
  | [] => true
  | (c, o, t, r) :: vs => step c o t == r && checkStep vs

def checkRun : List (Nat × List Nat × Nat) → Bool
  | [] => true
  | (d, gs, r) :: vs => run d gs == r && checkRun vs

def feedWindow (feed : List (Nat × Nat)) : List RecentBlockInfo :=
  feed.foldl (fun w p => Botho.Consensus.recordBlock w ⟨p.1, p.2⟩) []

def checkSlot : List (Bool × Nat × List (Nat × Nat) × Nat) → Bool
  | [] => true
  | (d, c, f, r) :: vs => slotSecs d c (feedWindow f) == r && checkSlot vs

theorem stepVectors_ok : checkStep stepVectors = true := by decide

theorem runVectors_ok : checkRun runVectors = true := by decide

theorem slotVectors_ok : checkSlot slotVectors = true := by decide

end Botho.Vectors
";

#[test]
#[ignore = "regenerates formal/Botho/Difficulty/Vectors.lean; run by hand"]
fn regenerate_formal_vectors() {
    let path = std::env::var("FORMAL_VECTORS_OUT")
        .expect("set FORMAL_VECTORS_OUT to the Vectors.lean path to write");
    let mut rng = Rng(1841);
    let step = gen_step_vectors(&mut rng);
    let run = gen_run_vectors(&mut rng);
    let slot = gen_slot_vectors(&mut rng);
    std::fs::write(&path, render(&step, &run, &slot)).expect("write vectors");
}
