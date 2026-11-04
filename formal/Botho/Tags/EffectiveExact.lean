/-
Exact claimed wealth (#1876, protocol 9.0.0). Models the fixed
`ValidationReads::effective_cluster_wealth_from_outputs`
(`botho/src/ledger/store/validation.rs`): the output-tag-weighted global
cluster wealth `Σ_out Σ_tag floor(amount·weight/SCALE)·W[c] / Σ_out amount`,
computed in Rust with a 256-bit numerator (`cluster-tax/src/wide.rs`) and
clamped at `u128::MAX` only at the end.

The per-entry `floor(amount·weight/SCALE)` is kept (it fits u128 in Rust), so
the two models agree on every input whose pre-fix numerator does not
saturate. The pre-fix saturating model, `effectiveWealth` in `Inherit.lean`,
is kept unchanged with its theorems. The DRT leg `effective_cluster_wealth`
now compares the Rust function against `effectiveWealthExact`.
Status: proven (model).
-/
import Botho.Tags.Inherit

namespace Botho.Tags

/-- Inner loop: one output's entries, accumulated exactly. -/
def weightedWealthExact (env : FeeEnv) (amount acc : Nat) : TagVec → Nat
  | [] => acc
  | e :: es =>
    weightedWealthExact env amount (acc + (amount * e.weight / SCALE) * env.wealth e.cluster) es

/-- The outer loop: `(total_weighted, total_value)`, both exact. -/
def effectiveAccExact (env : FeeEnv) : Nat × Nat → List Coin → Nat × Nat
  | acc, [] => acc
  | (w, t), o :: os => effectiveAccExact env (weightedWealthExact env o.value w o.tags, t + o.value) os

/-- Protocol 9.0.0 `effective_cluster_wealth_from_outputs`: the exact floor,
clamped once at `u128::MAX`. -/
def effectiveWealthExact (env : FeeEnv) (outs : List Coin) : Nat :=
  let (w, t) := effectiveAccExact env (0, 0) outs
  if t = 0 then 0 else
  let q := w / t
  if q ≤ U128_MAX then q else U128_MAX

/-- A ledger where every cluster holds 1B BTH of cumulative wealth. -/
def whaleEnv : FeeEnv where
  wealth := fun _ => 1000000000000000000000
  isImport := fun _ => false
  curve := id
  implied := fun _ => 0

/-- One 10M BTH output fully tagged to cluster 1. -/
def whaleOutputs : List Coin := [⟨10000000000000000000, [⟨1, SCALE⟩]⟩]

/-- The #1876 whale output: the exact claimed wealth is the cluster's wealth;
the pre-fix model returned `u128::MAX / 10^19`. -/
theorem whale_claimed_wealth_exact :
    effectiveWealthExact whaleEnv whaleOutputs = 1000000000000000000000 ∧
      effectiveWealth whaleEnv whaleOutputs = 34028236692093846346 := by
  decide

end Botho.Tags
