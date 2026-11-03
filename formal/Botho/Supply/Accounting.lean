/-
Block value-flow arithmetic (#1839). Status: proven (model).

Uses the existing DRT-tested `splitFees`, `computePoolAccounting` and
`carryoverAfter` models. This is an accounting step, not a proof of CLSAG,
LMDB, input uniqueness, or the correspondence of key images to spent coins.
`unspent` means economic unspent value, NOT the sum of the append-only
output database (spent outputs remain available as ring decoys).
-/
import Botho.Lottery.Pool

namespace Botho.Supply

open Botho.Lottery

/-- Within a percentage, the wide fee-pool quotient is at most the fees. -/
theorem fee_quotient_le (fees permille : Nat) (hp : permille ≤ 1000) :
    fees * permille / 1000 ≤ fees := by
  have h := Nat.div_le_div_right (Nat.mul_le_mul_left fees hp) (c := 1000)
  simpa using h

/-- The `as u64` cast loses no bits for an in-range fee and percentage.
Consensus `lottery_v2::canonical_config` fixes the percentage at 800. -/
theorem splitFees_exact (cfg : FeeConfig) (fees : Nat)
    (hf : fees ≤ U64_MAX) (hp : cfg.poolFractionPermille ≤ 1000) :
    splitFees cfg fees =
      (fees * cfg.poolFractionPermille / 1000,
       fees - fees * cfg.poolFractionPermille / 1000) := by
  have hq := fee_quotient_le fees cfg.poolFractionPermille hp
  have hw : fees * cfg.poolFractionPermille / 1000 < 2 ^ 64 := by
    unfold U64_MAX at hf
    omega
  simp [splitFees, truncU64, Nat.mod_eq_of_lt hw, satSub, hq]

/-- The rounded residue goes to the burn share; no fee units disappear. -/
theorem splitFees_conserves (cfg : FeeConfig) (fees : Nat)
    (hf : fees ≤ U64_MAX) (hp : cfg.poolFractionPermille ≤ 1000) :
    (splitFees cfg fees).1 + (splitFees cfg fees).2 = fees := by
  rw [splitFees_exact cfg fees hf hp]
  have := fee_quotient_le fees cfg.poolFractionPermille hp
  simp only
  omega

/-- Even for arbitrary configurations, the cast cannot silently destroy fees:
the burn is recomputed from the truncated pool. An oversized pool can instead
create value, as the counterexample below demonstrates. -/
theorem splitFees_total (cfg : FeeConfig) (fees : Nat) :
    (splitFees cfg fees).1 + (splitFees cfg fees).2 =
      max fees (splitFees cfg fees).1 := by
  simp only [splitFees, satSub]
  split <;> omega

/-- Integer rounding adds less than one unit to the exact rational burn:
1000 * pool ≤ fees * permille < 1000 * (pool + 1). -/
theorem splitFees_rounding (cfg : FeeConfig) (fees : Nat)
    (hf : fees ≤ U64_MAX) (hp : cfg.poolFractionPermille ≤ 1000) :
    let pool := (splitFees cfg fees).1
    1000 * pool ≤ fees * cfg.poolFractionPermille ∧
      fees * cfg.poolFractionPermille < 1000 * (pool + 1) := by
  rw [splitFees_exact cfg fees hf hp]
  simp only
  omega

/-- The unchecked configuration domain does NOT conserve fees. This is
not a valid canonical consensus configuration. -/
theorem oversized_fraction_creates_value :
    let cfg : FeeConfig := { poolFractionPermille := 2000, draw := DrawConfig.default }
    (splitFees cfg 1).1 + (splitFees cfg 1).2 = 2 := by decide

/-- Arithmetic state corresponding to writer.rs's supply counters plus
the economic unspent total and the lottery carryover. -/
structure State where
  unspent : Nat
  pool : Nat
  mined : Nat
  burned : Nat
  deriving Repr

/-- Scalar projection of a block. `spent` and `outputs` are sums of actual
spent coins and ordinary transaction outputs. `fees` includes all charges
enforced through the consensus fee floor; do not subtract them twice.
Emission and distribution are the values computed by the schedule and draw. -/
structure Flow where
  spent : Nat
  outputs : Nat
  fees : Nat
  reward : Nat
  emission : Nat
  distributed : Nat
  cap : Nat
  deriving Repr

/-- Accounting projection of `store/writer.rs::block_effects` and
`lottery_v2::validate_block`. Coinbase is reward minus lottery emission;
the carryover subtracts the actual payout, including zero when no winners.
`mined` and `burned` are natural additions: their Rust `u128` width is an
explicit theorem premise below, not silently modelled as arbitrary precision. -/
def apply (s : State) (f : Flow) (cfg : FeeConfig) : State :=
  let a := computePoolAccounting f.fees f.emission s.pool f.cap cfg
  { unspent := s.unspent - f.spent + f.outputs + satSub f.reward f.emission + f.distributed
    pool := a.carryoverAfter f.distributed
    mined := s.mined + f.reward
    burned := s.burned + a.feeBurn }

/-- Supply includes carryover; burning is the validated fee burn share. -/
def Conserved (s : State) : Prop := s.unspent + s.pool + s.burned = s.mined

/-- If spending draws from the economic unspent balance and transactions
balance, the available pool fits whenever total mined plus reward fits.
The bound derives from conservation; it does not assume the pool update exact. -/
theorem pool_inflow_fits (s : State) (f : Flow) (cfg : FeeConfig)
    (hs : Conserved s) (hspent : f.spent ≤ s.unspent)
    (htx : f.spent = f.outputs + f.fees) (he : f.emission ≤ f.reward)
    (hf : f.fees ≤ U64_MAX) (hp : cfg.poolFractionPermille ≤ 1000)
    (hwidth : s.mined + f.reward ≤ U128_MAX) :
    s.pool + f.emission + (splitFees cfg f.fees).1 ≤ U128_MAX := by
  have hsplit := splitFees_conserves cfg f.fees hf hp
  unfold Conserved at hs
  omega

/-- The complete scalar step conserves supply under the ledger's balance,
spend and draw premises. The first two concern actual spendability and are
trusted cryptographic/ledger premises, not claims proved by this model. -/
theorem apply_conserves (s : State) (f : Flow) (cfg : FeeConfig)
    (hs : Conserved s) (hspent : f.spent ≤ s.unspent)
    (htx : f.spent = f.outputs + f.fees) (he : f.emission ≤ f.reward)
    (hf : f.fees ≤ U64_MAX) (hp : cfg.poolFractionPermille ≤ 1000)
    (hwidth : s.mined + f.reward ≤ U128_MAX)
    (hd : f.distributed ≤
      (computePoolAccounting f.fees f.emission s.pool f.cap cfg).available) :
    Conserved (apply s f cfg) := by
  have hin := pool_inflow_fits s f cfg hs hspent htx he hf hp hwidth
  have hfirst : s.pool + f.emission ≤ U128_MAX := by omega
  have hav : (computePoolAccounting f.fees f.emission s.pool f.cap cfg).available =
      s.pool + f.emission + (splitFees cfg f.fees).1 := by
    simp [computePoolAccounting, satAdd, hfirst, hin]
  have hsplit := splitFees_conserves cfg f.fees hf hp
  rw [hav] at hd
  unfold Conserved at hs ⊢
  simp only [apply, Accounting.carryoverAfter]
  rw [hav]
  simp only [satSub, ite_eq_left he, ite_eq_left hd, computePoolAccounting]
  omega

/-- Height and reward widths provide the cumulative supply budget used above.
The induction premise is mined ≤ height * u64::MAX; genesis must establish it.
The strict height bound is the checked successor guard in lottery_v2::seed. -/
theorem issuance_step_fits (height mined reward : Nat)
    (hh : height < U64_MAX) (hm : mined ≤ height * U64_MAX)
    (hr : reward ≤ U64_MAX) :
    mined + reward ≤ (height + 1) * U64_MAX ∧ mined + reward ≤ U128_MAX := by
  have hs : mined + reward ≤ (height + 1) * U64_MAX := by
    simp only [Nat.add_mul, Nat.one_mul]
    omega
  have hmax : (height + 1) * U64_MAX ≤ U64_MAX * U64_MAX :=
    Nat.mul_le_mul_right U64_MAX (by omega)
  have hw : U64_MAX * U64_MAX ≤ U128_MAX := by decide
  omega

/-- Conservation also bounds both cumulative Rust counters in the step. -/
theorem counters_fit (s : State) (f : Flow) (cfg : FeeConfig)
    (hs : Conserved s) (hspent : f.spent ≤ s.unspent)
    (htx : f.spent = f.outputs + f.fees)
    (hf : f.fees ≤ U64_MAX) (hp : cfg.poolFractionPermille ≤ 1000)
    (hwidth : s.mined + f.reward ≤ U128_MAX) :
    (apply s f cfg).mined ≤ U128_MAX ∧ (apply s f cfg).burned ≤ U128_MAX := by
  have hsplit := splitFees_conserves cfg f.fees hf hp
  unfold Conserved at hs
  simp only [apply, computePoolAccounting]
  omega

/-- Arbitrary u128 pool states can lose value by saturation. This counterexample
does not satisfy the reachable-state supply budget used by `apply_conserves`. -/
theorem arbitrary_pool_saturates :
    let cfg : FeeConfig := { poolFractionPermille := 800, draw := DrawConfig.default }
    (computePoolAccounting 0 1 U128_MAX 0 cfg).available = U128_MAX ∧
      (computePoolAccounting 0 1 U128_MAX 0 cfg).available < U128_MAX + 1 := by decide

end Botho.Supply
