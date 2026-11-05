/-
The protocol 9.0.0 demurrage kernel (#1875): one floor of the full rational
charge. Status: model; tested (DRT leg `demurrage_charge`); no recorded
theorems yet (new statements need their records on the base branch first).

`Botho.Demurrage.charge` in `Counterexamples.lean` is the LEGACY pre-9.0.0
kernel (time fraction floored to 1e6 units of a year before the value is
applied). Its theorems (`six_block_dead_zone`, `churn_counterexample`,
`charge_exceeds_value`, `saturation_lowers_charge`) are unchanged: they are
the machine-checked record of the #1875 finding against that kernel.
-/
import Botho.Lottery.Arith

namespace Botho.Demurrage
open Botho.Lottery

/-- `cluster-tax/src/demurrage.rs::demurrage_charge` from protocol 9.0.0:

    floor(value × rate × (clamp(factor, 1000, 6000) − 1000) × elapsed
          / (10_000 × 5_000 × year))

clamped to `u64::MAX`. This is the specification itself, on `Nat`, with no
intermediate rounding: the Rust kernel's u128 decomposition (quotient and
remainder of `value × rate × p` by the denominator, with `elapsed` split at
2^32) is not mirrored, so the DRT tests that the decomposition is exact.
The Rust zero guards (`rate = 0`, `elapsed = 0`, `year = 0`, progressivity
0) all agree with this formula: a zero factor in the numerator gives 0, and
`Nat` division by zero is 0. -/
def exactCharge (value factor elapsed rate year : Nat) : Nat :=
  let progressivity := clampN 1000 6000 factor - 1000
  minN (value * rate * progressivity * elapsed / (10000 * 5000 * year)) U64_MAX

end Botho.Demurrage
