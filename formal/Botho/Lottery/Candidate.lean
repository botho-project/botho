/-
Lottery candidates: eligibility, the eligible count ρ, the Sybil reward cap
and the selection weights (#1707).

Mirrors `cluster-tax/src/lottery.rs`. Status: tested (DRT legs
`is_eligible`, `count_eligible`, `sybil_reward_cap`; the weights through
`draw_winners_with_seed`).

Not modeled: `SelectionMode::EntropyWeighted`. It is research-only, reads a
floating-point entropy (`TagVector::cluster_entropy`, a `log2`), and the
source says it must not be used in consensus. `SelectionMode::Hybrid` is
modeled from its quantized parameter (`alpha_bps`, see `SelectionMode.hybrid`).
-/
import Botho.Lottery.Arith

namespace Botho.Lottery

/-- `CIRCULATION_WINDOW_BLOCKS` (`cluster-tax/src/lottery.rs`). -/
def CIRCULATION_WINDOW_BLOCKS : Nat := 10000

/-- `LOTTERY_BASE_FEE_PICO` (`cluster-tax/src/lottery.rs`): 0.25 BTH. -/
def LOTTERY_BASE_FEE_PICO : Nat := 250000000000

/-- `FACTOR_SCALE` (`cluster-tax/src/lottery.rs`): 1000 = factor 1.0. -/
def FACTOR_SCALE : Nat := 1000

/-- `MAX_FACTOR_SCALED` (`cluster-tax/src/lottery.rs`): factor 6.0. -/
def MAX_FACTOR_SCALED : Nat := 6 * FACTOR_SCALE

/-- `SelectionMode` (`cluster-tax/src/lottery.rs`), without the research-only
`EntropyWeighted`. -/
inductive SelectionMode where
  /-- Path C default: every eligible output has weight 1. -/
  | uniform
  /-- Weight = value. -/
  | valueWeighted
  /-- `Hybrid { alpha }`, given by `alphaBps`, the value Rust computes as
  `(alpha.clamp(0.0, 1.0) * 10_000.0).round() as u128`. So
  `alphaBps ≤ 10000`. The `f64` to basis-point step is not modeled: it is a
  property of a configuration constant, identical on every node. -/
  | hybrid (alphaBps : Nat)
  /-- Weight = value × (6000 − factor + 1000). -/
  | clusterWeighted
  deriving Repr, DecidableEq

/-- `LotteryDrawConfig` (`cluster-tax/src/lottery.rs`), without the unused
`pool_fraction: f64` (the node splits fees with
`LotteryFeeConfig::pool_fraction_permille`, see `Pool.lean`). Numbers are
`u64`, `winnersPerDraw` is a `usize`. -/
structure DrawConfig where
  winnersPerDraw : Nat
  minUtxoAge : Nat
  minUtxoValue : Nat
  circulationWindow : Nat
  mode : SelectionMode
  deriving Repr

/-- `LotteryDrawConfig::default()` (`cluster-tax/src/lottery.rs`): 4 winners,
age 720, value 1e6, window 10,000, uniform. -/
def DrawConfig.default : DrawConfig :=
  { winnersPerDraw := 4, minUtxoAge := 720, minUtxoValue := 1000000,
    circulationWindow := CIRCULATION_WINDOW_BLOCKS, mode := .uniform }

/-- `LotteryCandidate` (`cluster-tax/src/lottery.rs`), without the
research-only `tag_entropy`. `id` is 36 bytes; the numbers are `u64`. -/
structure Candidate where
  id : Bytes
  value : Nat
  clusterFactor : Nat
  creationBlock : Nat
  deriving Repr

/-- Mirrors `LotteryCandidate::is_eligible` (`cluster-tax/src/lottery.rs:187`):
mature (`age ≥ min_utxo_age`), inside the circulation window
(`age ≤ circulation_window`) and above the dust floor. -/
def Candidate.isEligible (c : Candidate) (currentBlock : Nat)
    (cfg : DrawConfig) : Bool :=
  let age := satSub currentBlock c.creationBlock
  decide (cfg.minUtxoAge ≤ age) && decide (age ≤ cfg.circulationWindow) &&
    decide (cfg.minUtxoValue ≤ c.value)

/-- Mirrors `count_eligible` (`cluster-tax/src/lottery.rs:248`): ρ, the number
of eligible candidates. -/
def countEligible : List Candidate → Nat → DrawConfig → Nat
  | [], _, _ => 0
  | c :: cs, h, cfg =>
    (if c.isEligible h cfg then 1 else 0) + countEligible cs h cfg

/-- Mirrors `sybil_reward_cap` (`cluster-tax/src/lottery.rs:272`):
`(eligible_count as u128).saturating_mul(LOTTERY_BASE_FEE_PICO)`. -/
def sybilRewardCap (eligibleCount : Nat) : Nat :=
  satMul U128_MAX eligibleCount LOTTERY_BASE_FEE_PICO

/-- Mirrors `LotteryCandidate::weight` (`cluster-tax/src/lottery.rs:203`) for
the integer modes. The result is a `u128`; none of the products can overflow
it (`value < 2^64`, the factors are below 2^14, `alphaBps ≤ 10000`). -/
def Candidate.weight (c : Candidate) (mode : SelectionMode)
    (maxValue : Nat) : Nat :=
  match mode with
  | .uniform => 1
  | .valueWeighted => c.value
  | .hybrid alphaBps =>
    alphaBps * (if maxValue < 1 then 1 else maxValue) + (10000 - alphaBps) * c.value
  | .clusterWeighted =>
    let factor := clampN FACTOR_SCALE MAX_FACTOR_SCALED c.clusterFactor
    c.value * (MAX_FACTOR_SCALED - factor + FACTOR_SCALE)

end Botho.Lottery
