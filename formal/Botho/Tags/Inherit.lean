/-
What consensus checks about cluster tags (#1840, epic #1704).

**Scope: protocol ≤ 7.x (pre-fix, #1857/#1858; fixed by #1870/#1863).**
`inheritOk` and the theorems below model the 7.x tag rule: C6's
conservation-of-mass check alone. Protocol 8.0.0 (#1870) adds Rule 1 (every
output `is_valid`, in `validate_ordinary_body` before C6) and Rule 2 (an
output may name a cluster no ring member carries only if it is the including
block's epoch import cluster, inside `check_cluster_tag_inheritance`). The
statements are unchanged: they are about `inheritOk`, so they stay true, and
they are the machine-checked record of #1857. `inheritOk₈` (end of this
module) models the 8.0.0 rules, and the DRT leg `cluster_tag_inheritance`
compares against it.

Consensus applies no tag mixing and no decay. In 7.x the only tag rule a
block had to pass was C6, `check_cluster_tag_inheritance`; the ledger then
reads output tags for the fee floor and writes them into the per-cluster
wealth index. This module mirrors those four pieces, and keeps them apart
from the wallet's `merge_weighted` (`Botho.Tags.Vector`), which a modified
wallet need not run.

| Lean (`Botho.Tags.*`) | Rust | Status |
|---|---|---|
| `inheritOk`, `inputMass`, `outputMass`, `inputClusters` | the C6 mass check of `check_cluster_tag_inheritance` (`botho/src/ledger/store.rs:242`); in 7.x the whole tag rule | tested, as the last step of `inheritOk₈` |
| `inheritOk₈`, `tagsValid`, `ruleTwoOk` | 8.0.0: `first_malformed_output_tags` (Rule 1, `store.rs:332`) then `check_cluster_tag_inheritance` (Rule 2 via `check_output_clusters_on_ring`, `store.rs:353`, then C6), reached through `validate_ordinary_body` and `ValidationReads::verify_cluster_tag_inheritance(tx, height)` (`store/validation.rs:103`) | tested (DRT leg `cluster_tag_inheritance`); no recorded theorems yet |
| `wealthCredit` | `update_cluster_wealth_for_output`, `botho/src/ledger/store/writer.rs:469` | tested (end to end, `cluster_tag_invariant_tests.rs`) |
| `effectiveWealth` | `ValidationReads::effective_cluster_wealth_from_outputs`, `store/validation.rs:272` | tested (DRT leg `effective_cluster_wealth`) |
| `importFloor` | `ValidationReads::import_floor_factor_from_outputs`, `store/validation.rs:305` | tested (DRT leg `import_floor_factor`) |
| `demurrageFactor` | the factor composition in `ValidationReads::consensus_fee_floor`, `store/validation.rs:161` (`claimed.max(implied)` in `ring_centroid_floored_factor`, then `.max(import_floor)`) | model; the curve and the ring-centroid factor are parameters |

The fee curve (`ClusterFactorCurve::factor`) and the ring-centroid implied
factor (`ring_centroid_implied_factor`) are not modelled here (#1711 models
demurrage). They enter as fields of `FeeEnv`, so a theorem about
`demurrageFactor` holds for every curve and every implied factor; where it
needs a property of them, that is a hypothesis, labeled **trusted**.

Integers. Values are `u64`, weights `u32`, all sums `u128`. C6 accumulates
with plain `+=` on `u128`, which cannot overflow for fewer than `2^52` tag
entries; the model uses `Nat`. The fee-side functions saturate, and the
model mirrors each saturation.

Results (#1840; about 7.x C6, see the scope note above):

1. **Mass bound: refuted (7.x; fixed by #1870 Rules 1 and 2).** C6 bounds only the clusters present on the
   ring members (`inputClusters`); for those it is a per-cluster
   conservation-of-mass bound (`inheritOk_bound`, proven). Any other
   cluster is unconstrained (`inheritOk_cons_fresh`), so an output may carry
   any weight, beyond 100% and with any number of entries
   (`inherit_admits_fresh_mass_above_one`). The writer credits every
   named cluster (`wealthCredit`), so a spender can raise a cluster's
   recorded wealth by `k * value * 4294.967` with `k` entries of weight
   `u32::MAX` (`fresh_cluster_credit_unbounded`).
2. **#1687 tag drop: refuted (unchanged in 8.0.0: an empty vector passes
   Rule 1, and Rule 2 trivially).** Untagged outputs always pass C6
   (`inheritOk_of_untagged`). The ADR-0007 import floor is read from
   output tags, so it is 1x for such a spend (`importFloor_of_untagged`),
   and the factor the fee floor charges no longer mentions the import tag
   (`tag_drop_demurrage_factor`). With a fee curve and a ring factor below
   `F` (trusted hypotheses; measured in the end-to-end test), the fully
   import-tagged coin is spent below `F` while the same spend keeping its
   tag pays at least `F` (`tag_drop_escapes_import_floor`), and the
   invariant "every accepted spend of a fully import-tagged ring pays at
   least `F`" is false (`import_floor_invariant_refuted`).
-/
import Botho.Tags.Vector

namespace Botho.Tags

/-- A ring member or an output: its value (`amount`, `u64`) and tags. -/
structure Coin where
  value : Nat
  tags : TagVec
  deriving DecidableEq, Repr

/-- The mass a coin of value `v` puts on cluster `c`:
`Σ (v as u128) * (weight as u128) / TAG_WEIGHT_SCALE` over the entries of
cluster `c` (duplicates summed, each entry floored on its own). -/
def massOver (c v : Nat) : TagVec → Nat
  | [] => 0
  | e :: es => (if e.cluster = c then v * e.weight / SCALE else 0) + massOver c v es

/-- `member_mass[c]` / the output accumulation for one coin. -/
def coinMass (c : Nat) (x : Coin) : Nat := massOver c x.value x.tags

/-- `ring_max[c]`: the largest member mass on cluster `c` in one ring. -/
def ringMax (c : Nat) : List Coin → Nat
  | [] => 0
  | x :: xs => max (coinMass c x) (ringMax c xs)

/-- `input_masses[c]`: the sum over rings of the ring maxima. -/
def inputMass (c : Nat) (rings : List (List Coin)) : Nat := sumN (rings.map (ringMax c))

/-- `output_masses[c]`. -/
def outputMass (c : Nat) (outs : List Coin) : Nat := sumN (outs.map (coinMass c))

/-- The keys of `input_masses`: every cluster an entry of a ring member
names (with repeats; the check below does not depend on order or
multiplicity). -/
def inputClusters (rings : List (List Coin)) : List Nat :=
  rings.flatMap fun ring => ring.flatMap fun x => x.tags.map Entry.cluster

/-- `CONSENSUS_TAG_DECAY_RATE` (`store.rs:178`). -/
def CONSENSUS_TAG_DECAY_RATE : Nat := 0

/-- `expected = input_mass * (SCALE - CONSENSUS_TAG_DECAY_RATE) / SCALE`. -/
def expected (m : Nat) : Nat := m * (SCALE - CONSENSUS_TAG_DECAY_RATE) / SCALE

/-- `tolerance = (input_mass / 1000).max(1)`. -/
def tolerance (m : Nat) : Nat := max (m / 1000) 1

/-- The per-cluster test of C6 (the negation of the `return Err` branch). -/
def clusterOk (rings : List (List Coin)) (outs : List Coin) (c : Nat) : Bool :=
  decide (outputMass c outs ≤ expected (inputMass c rings) + tolerance (inputMass c rings))

/-- Mirrors the 7.x `check_cluster_tag_inheritance`: `true` iff it returned
`Ok`. Since 8.0.0 this is only its C6 mass check, which runs after Rule 2
(`inheritOk₈`). -/
def inheritOk (rings : List (List Coin)) (outs : List Coin) : Bool :=
  (inputClusters rings).all (clusterOk rings outs)

/-- Mirrors `update_cluster_wealth_for_output` (`writer.rs:469`) over a
transaction's outputs: the increment of cluster `c`'s recorded wealth
(`Σ amount * weight / SCALE` over its entries; the `u128` accumulator
saturates only at `2^128`). Every named cluster is credited, whether or not
the inputs carried it. -/
def wealthCredit (c : Nat) (outs : List Coin) : Nat := outputMass c outs

/-! ## Fee-floor inputs read from output tags -/

/-- `ClusterFactorCurve::FACTOR_SCALE`: factor 1x. -/
def FACTOR_SCALE : Nat := 1000

/-- `BRIDGE_IMPORT_FACTOR_FLOOR` (`cluster-tax/src/bridge_import.rs:72`):
ADR 0007's `F = 1.5x`. -/
def BRIDGE_IMPORT_FACTOR_FLOOR : Nat := 1500

/-- The ledger state and the kernels the fee floor reads that this module
does not model. -/
structure FeeEnv where
  /-- `get_cluster_wealth` (`u128`). -/
  wealth : Nat → Nat
  /-- `is_bridge_import_cluster`. -/
  isImport : Nat → Bool
  /-- `FeeConfig::cluster_factor` (`ClusterFactorCurve::factor`), #1711. -/
  curve : Nat → Nat
  /-- The ring-centroid implied factor of `ring_centroid_floored_factor`
  (`ring_centroid_implied_factor` over the resolved ring members), #1711. -/
  implied : List (List Coin) → Nat

/-- Inner loop of `effective_cluster_wealth_from_outputs`: one output's
entries, `total_weighted = total_weighted.saturating_add(
value_fraction.saturating_mul(global_wealth))`. -/
def weightedWealth (env : FeeEnv) (amount acc : Nat) : TagVec → Nat
  | [] => acc
  | e :: es =>
    weightedWealth env amount
      (satAdd U128_MAX acc (satMul U128_MAX (amount * e.weight / SCALE) (env.wealth e.cluster))) es

/-- The outer loop: `(total_weighted, total_value)`, both saturating
`u128`. -/
def effectiveAcc (env : FeeEnv) : Nat × Nat → List Coin → Nat × Nat
  | acc, [] => acc
  | (w, t), o :: os => effectiveAcc env (weightedWealth env o.value w o.tags, satAdd U128_MAX t o.value) os

/-- Mirrors `effective_cluster_wealth_from_outputs` (`validation.rs:272`). -/
def effectiveWealth (env : FeeEnv) (outs : List Coin) : Nat :=
  let (w, t) := effectiveAcc env (0, 0) outs
  if t = 0 then 0 else w / t

/-- The spender-declared factor `claimed_factor`. -/
def claimedFactor (env : FeeEnv) (outs : List Coin) : Nat := env.curve (effectiveWealth env outs)

/-- Inner loop of `import_floor_factor_from_outputs`: import-tagged value,
`amount.saturating_mul(weight) / SCALE`, summed saturating. -/
def importTagged (env : FeeEnv) (amount acc : Nat) : TagVec → Nat
  | [] => acc
  | e :: es =>
    importTagged env amount
      (if env.isImport e.cluster then satAdd U128_MAX acc (satMul U128_MAX amount e.weight / SCALE)
       else acc) es

/-- The outer loop: `(import_value, total_value)`. -/
def importAcc (env : FeeEnv) : Nat × Nat → List Coin → Nat × Nat
  | acc, [] => acc
  | (i, t), o :: os => importAcc env (importTagged env o.value i o.tags, satAdd U128_MAX t o.value) os

/-- Mirrors `import_floor_factor_from_outputs` (`validation.rs:305`): the
value-weighted blend of `F` on the import-tagged value (clamped to the
total) and 1x on the rest, `as u64`. -/
def importFloor (env : FeeEnv) (outs : List Coin) : Nat :=
  let (i, t) := importAcc env (0, 0) outs
  if t = 0 then FACTOR_SCALE
  else
    let i := min i t
    truncU64 (satAdd U128_MAX (satMul U128_MAX BRIDGE_IMPORT_FACTOR_FLOOR i)
      (satMul U128_MAX FACTOR_SCALE (t - i)) / t)

/-- The factor the consensus fee floor charges demurrage at:
`max(max(claimed, implied), import_floor)` (`consensus_fee_floor`,
`validation.rs:161`). -/
def demurrageFactor (env : FeeEnv) (rings : List (List Coin)) (outs : List Coin) : Nat :=
  max (max (claimedFactor env outs) (env.implied rings)) (importFloor env outs)

/-! ## Lemmas -/

theorem expected_eq (m : Nat) : expected m = m := by
  simp [expected, CONSENSUS_TAG_DECAY_RATE, SCALE]

theorem massOver_nil_tags (c v : Nat) : massOver c v [] = 0 := rfl

theorem outputMass_cons (c : Nat) (x : Coin) (outs : List Coin) :
    outputMass c (x :: outs) = coinMass c x + outputMass c outs := rfl

theorem massOver_fresh (c v : Nat) (tags : TagVec) (h : ∀ e ∈ tags, e.cluster ≠ c) :
    massOver c v tags = 0 := by
  induction tags with
  | nil => rfl
  | cons e es ih =>
    have he := h e (List.mem_cons_self ..)
    simp only [massOver, he, ite_false, Nat.zero_add]
    exact ih (fun e' h' => h e' (List.mem_cons_of_mem _ h'))

theorem outputMass_untagged (c : Nat) (outs : List Coin) (h : ∀ x ∈ outs, x.tags = []) :
    outputMass c outs = 0 := by
  induction outs with
  | nil => rfl
  | cons x xs ih =>
    rw [outputMass_cons, ih (fun y hy => h y (List.mem_cons_of_mem _ hy))]
    simp [coinMass, h x (List.mem_cons_self ..), massOver]

theorem all_congr_mem {f g : Nat → Bool} (l : List Nat) (h : ∀ c ∈ l, f c = g c) :
    l.all f = l.all g := by
  induction l with
  | nil => rfl
  | cons x xs ih =>
    simp only [List.all_cons, h x (List.mem_cons_self ..),
      ih (fun c hc => h c (List.mem_cons_of_mem _ hc))]

/-! ## Theorems: C6 -/

/-- **What C6 does bound** (proven (model)): on every cluster carried by a
ring member, the outputs' mass is at most the sum of the per-ring maxima
plus the tolerance (decay rate 0). -/
theorem inheritOk_bound (rings : List (List Coin)) (outs : List Coin)
    (h : inheritOk rings outs = true) (c : Nat) (hc : c ∈ inputClusters rings) :
    outputMass c outs ≤ inputMass c rings + tolerance (inputMass c rings) := by
  unfold inheritOk at h
  have := List.all_eq_true.mp h c hc
  simp only [clusterOk, decide_eq_true_eq, expected_eq] at this
  exact this

/-- **C6 ignores clusters absent from the ring** (proven (model)): adding an
output whose entries all name clusters no ring member carries never changes
the verdict. -/
theorem inheritOk_cons_fresh (rings : List (List Coin)) (outs : List Coin) (x : Coin)
    (hx : ∀ e ∈ x.tags, e.cluster ∉ inputClusters rings) :
    inheritOk rings (x :: outs) = inheritOk rings outs := by
  unfold inheritOk
  apply all_congr_mem
  intro c hc
  have h0 : coinMass c x = 0 :=
    massOver_fresh c x.value x.tags (fun e he hce => hx e he (hce ▸ hc))
  simp only [clusterOk, outputMass_cons, h0, Nat.zero_add]

/-- **#1687: untagged outputs always pass C6** (proven (model)): a spender
may drop every tag of every input in one transaction. -/
theorem inheritOk_of_untagged (rings : List (List Coin)) (outs : List Coin)
    (h : ∀ x ∈ outs, x.tags = []) : inheritOk rings outs = true := by
  unfold inheritOk
  apply List.all_eq_true.mpr
  intro c _
  simp only [clusterOk, outputMass_untagged c outs h, Nat.zero_le, decide_true]

/-- **Invariant 1 (total mass at most 100%) is refuted** for 7.x C6 (proven
(model); #1857, fixed by #1870: Rule 1 rejects this output): for every set
of rings and every cluster `c` no ring member carries, an
output of any value `a` tagged `c` at weight `u32::MAX` (429,497%) passes
C6, and the writer credits `c` with at least 4,294 times `a`. -/
theorem inherit_admits_fresh_mass_above_one (rings : List (List Coin)) (c a : Nat)
    (hc : c ∉ inputClusters rings) :
    inheritOk rings [⟨a, [⟨c, U32_MAX⟩]⟩] = true ∧
    SCALE < mass [⟨c, U32_MAX⟩] ∧
    4294 * a ≤ wealthCredit c [⟨a, [⟨c, U32_MAX⟩]⟩] := by
  refine ⟨?_, by simp only [mass, massP, ite_true, Nat.add_zero]; decide, ?_⟩
  · rw [inheritOk_cons_fresh rings [] _ (by intro e he; simp at he; subst he; exact hc)]
    exact inheritOk_of_untagged rings [] (by simp)
  · have hw : wealthCredit c [⟨a, [⟨c, U32_MAX⟩]⟩] = a * U32_MAX / SCALE := by
      show massOver c a [⟨c, U32_MAX⟩] + 0 = _
      rw [Nat.add_zero, massOver, massOver, Nat.add_zero, ite_eq_left rfl]
    rw [hw]
    apply (Nat.le_div_iff_mul_le (by decide : 0 < SCALE)).mpr
    simp only [SCALE, U32_MAX]
    omega

/-- **Unbounded wealth credit** under 7.x C6 (proven (model); #1857, fixed
by #1870: Rules 1 and 2 reject such outputs): `k` entries of weight
`u32::MAX` for a cluster no ring member carries pass C6, and credit that
cluster with `k * (a * u32::MAX / SCALE)`. Only the transaction size
bounds `k`. -/
theorem fresh_cluster_credit_unbounded (rings : List (List Coin)) (c a k : Nat)
    (hc : c ∉ inputClusters rings) :
    inheritOk rings [⟨a, List.replicate k ⟨c, U32_MAX⟩⟩] = true ∧
    wealthCredit c [⟨a, List.replicate k ⟨c, U32_MAX⟩⟩] = k * (a * U32_MAX / SCALE) := by
  refine ⟨?_, ?_⟩
  · rw [inheritOk_cons_fresh rings [] _ (by
      intro e he; rw [List.eq_of_mem_replicate he]; exact hc)]
    exact inheritOk_of_untagged rings [] (by simp)
  · show massOver c a (List.replicate k ⟨c, U32_MAX⟩) + 0 = _
    rw [Nat.add_zero]
    induction k with
    | zero => exact (Nat.zero_mul _).symm
    | succ k ih =>
      rw [List.replicate_succ, massOver, ite_eq_left rfl, ih, Nat.succ_mul, Nat.add_comm]

/-! ## Theorems: the #1687 tag drop -/

theorem importTagged_nil (env : FeeEnv) (amount acc : Nat) : importTagged env amount acc [] = acc :=
  rfl

theorem weightedWealth_nil (env : FeeEnv) (amount acc : Nat) :
    weightedWealth env amount acc [] = acc := rfl

theorem importAcc_untagged (env : FeeEnv) (i t : Nat) (outs : List Coin)
    (h : ∀ x ∈ outs, x.tags = []) (ht : t + sumN (outs.map Coin.value) ≤ U128_MAX) :
    importAcc env (i, t) outs = (i, t + sumN (outs.map Coin.value)) := by
  induction outs generalizing t with
  | nil => simp [importAcc, sumN]
  | cons x xs ih =>
    have hx := h x (List.mem_cons_self ..)
    simp only [List.map_cons, sumN] at ht ⊢
    have hs : satAdd U128_MAX t x.value = t + x.value := by unfold satAdd; split <;> omega
    simp only [importAcc, hx, importTagged_nil, hs]
    rw [ih (t + x.value) (fun y hy => h y (List.mem_cons_of_mem _ hy)) (by omega)]
    congr 1; omega

theorem effectiveAcc_untagged (env : FeeEnv) (w t : Nat) (outs : List Coin)
    (h : ∀ x ∈ outs, x.tags = []) (ht : t + sumN (outs.map Coin.value) ≤ U128_MAX) :
    effectiveAcc env (w, t) outs = (w, t + sumN (outs.map Coin.value)) := by
  induction outs generalizing t with
  | nil => simp [effectiveAcc, sumN]
  | cons x xs ih =>
    have hx := h x (List.mem_cons_self ..)
    simp only [List.map_cons, sumN] at ht ⊢
    have hs : satAdd U128_MAX t x.value = t + x.value := by unfold satAdd; split <;> omega
    simp only [effectiveAcc, hx, weightedWealth_nil, hs]
    rw [ih (t + x.value) (fun y hy => h y (List.mem_cons_of_mem _ hy)) (by omega)]
    congr 1; omega

/-- **#1687: the import floor of an untagged spend is 1x** (proven (model)),
whatever its inputs were. The hypothesis on the output sum holds for every
transaction that balances (`verify_ring_signatures` rejects an output sum
above `u64::MAX`). -/
theorem importFloor_of_untagged (env : FeeEnv) (outs : List Coin)
    (h : ∀ x ∈ outs, x.tags = []) (hs : sumN (outs.map Coin.value) ≤ U64_MAX) :
    importFloor env outs = FACTOR_SCALE := by
  have hU : sumN (outs.map Coin.value) ≤ U128_MAX := by
    simp only [U64_MAX, U128_MAX] at hs ⊢; omega
  unfold importFloor
  rw [importAcc_untagged env 0 0 outs h (by omega)]
  simp only [Nat.zero_add]
  split
  · rfl
  · rename_i ht
    have hpos : 0 < sumN (outs.map Coin.value) := Nat.pos_of_ne_zero ht
    simp only [Nat.zero_min, Nat.sub_zero, satMul, Nat.mul_zero, Nat.zero_le, ite_true,
      FACTOR_SCALE]
    have hm : 1000 * sumN (outs.map Coin.value) ≤ U128_MAX := by
      simp only [U64_MAX, U128_MAX] at hs ⊢; omega
    simp only [hm, ite_true, satAdd, Nat.zero_add]
    rw [Nat.mul_div_cancel _ hpos]
    decide

/-- An untagged spend declares effective wealth 0, so its claimed factor is
the curve at 0 (proven (model)). -/
theorem claimedFactor_of_untagged (env : FeeEnv) (outs : List Coin)
    (h : ∀ x ∈ outs, x.tags = []) (hs : sumN (outs.map Coin.value) ≤ U64_MAX) :
    claimedFactor env outs = env.curve 0 := by
  have hU : sumN (outs.map Coin.value) ≤ U128_MAX := by
    simp only [U64_MAX, U128_MAX] at hs ⊢; omega
  unfold claimedFactor effectiveWealth
  rw [effectiveAcc_untagged env 0 0 outs h (by omega)]
  simp only [Nat.zero_add, Nat.zero_div]
  split <;> rfl

/-- **#1687: the factor a tag-dropping spend is charged at** (proven
(model)): `max(curve(0), implied(ring), 1x)`. The import floor `F` does not
appear, whatever the inputs' tags were. -/
theorem tag_drop_demurrage_factor (env : FeeEnv) (rings : List (List Coin)) (outs : List Coin)
    (h : ∀ x ∈ outs, x.tags = []) (hs : sumN (outs.map Coin.value) ≤ U64_MAX) :
    inheritOk rings outs = true ∧
    demurrageFactor env rings outs = max (max (env.curve 0) (env.implied rings)) FACTOR_SCALE := by
  refine ⟨inheritOk_of_untagged rings outs h, ?_⟩
  unfold demurrageFactor
  rw [claimedFactor_of_untagged env outs h hs, importFloor_of_untagged env outs h hs]

/-- The import floor of one output fully tagged to an import cluster is `F`
(proven (model)). -/
theorem importFloor_fully_tagged (env : FeeEnv) (c v : Nat) (hc : env.isImport c = true)
    (hv : 0 < v) (hv' : v ≤ U64_MAX) :
    importFloor env [⟨v, [⟨c, SCALE⟩]⟩] = BRIDGE_IMPORT_FACTOR_FLOOR := by
  simp only [U64_MAX] at hv'
  have h1 : satMul U128_MAX v SCALE = v * SCALE := by
    unfold satMul U128_MAX SCALE; split <;> omega
  have h2 : satAdd U128_MAX 0 (v * SCALE / SCALE) = v := by
    rw [Nat.mul_div_cancel _ (by decide : 0 < SCALE)]; unfold satAdd U128_MAX; split <;> omega
  have h3 : satAdd U128_MAX 0 v = v := by unfold satAdd U128_MAX; split <;> omega
  have h4 : satMul U128_MAX BRIDGE_IMPORT_FACTOR_FLOOR v = 1500 * v := by
    unfold satMul U128_MAX BRIDGE_IMPORT_FACTOR_FLOOR; split <;> omega
  have h5 : satMul U128_MAX FACTOR_SCALE 0 = 0 := by unfold satMul; simp
  have h6 : satAdd U128_MAX (1500 * v) 0 = 1500 * v := by unfold satAdd U128_MAX; split <;> omega
  simp only [importFloor, importAcc, importTagged, hc, ite_true, h1, h2, h3]
  have hv0 : v ≠ 0 := by omega
  simp only [hv0, ite_false, Nat.min_self, Nat.sub_self, h4, h5, h6]
  rw [Nat.mul_div_cancel _ hv]
  decide

/-- **#1687 counterexample** (trusted: the hypotheses `hcurve` and
`himplied` are about the fee curve and the ring-centroid factor, which this
module takes as parameters; the end-to-end test
`import_tag_drop_is_accepted_and_pays_less_than_the_import_floor` measures
them on a real ledger). A coin of value `v` fully tagged to an import
cluster, spent alone: keeping the tag, the spend is charged at least `F`;
dropping it, the spend still passes C6 and is charged below `F`. -/
theorem tag_drop_escapes_import_floor (env : FeeEnv) (c v : Nat)
    (hc : env.isImport c = true) (hv : 0 < v) (hv' : v ≤ U64_MAX)
    (hcurve : env.curve 0 < BRIDGE_IMPORT_FACTOR_FLOOR)
    (himplied : env.implied [[⟨v, [⟨c, SCALE⟩]⟩]] < BRIDGE_IMPORT_FACTOR_FLOOR) :
    inheritOk [[⟨v, [⟨c, SCALE⟩]⟩]] [⟨v, [⟨c, SCALE⟩]⟩] = true ∧
    BRIDGE_IMPORT_FACTOR_FLOOR ≤ demurrageFactor env [[⟨v, [⟨c, SCALE⟩]⟩]] [⟨v, [⟨c, SCALE⟩]⟩] ∧
    inheritOk [[⟨v, [⟨c, SCALE⟩]⟩]] [⟨v, []⟩] = true ∧
    demurrageFactor env [[⟨v, [⟨c, SCALE⟩]⟩]] [⟨v, []⟩] < BRIDGE_IMPORT_FACTOR_FLOOR := by
  have hdrop := tag_drop_demurrage_factor env [[⟨v, [⟨c, SCALE⟩]⟩]] [⟨v, []⟩]
    (by intro x hx; simp at hx; subst hx; rfl) (by simp [sumN]; omega)
  refine ⟨?_, ?_, hdrop.1, ?_⟩
  · unfold inheritOk
    apply List.all_eq_true.mpr
    intro d hd
    simp [inputClusters] at hd
    subst hd
    simp only [clusterOk, decide_eq_true_eq, expected_eq, outputMass, inputMass, List.map, sumN,
      ringMax, Nat.add_zero]
    omega
  · unfold demurrageFactor
    rw [importFloor_fully_tagged env c v hc hv hv']
    omega
  · rw [hdrop.2]
    simp only [FACTOR_SCALE, BRIDGE_IMPORT_FACTOR_FLOOR] at hcurve himplied ⊢
    omega

/-- A concrete fee environment: the curve is 1x everywhere (as the real
curve is at wealth 0) and the ring factor is 1x; cluster 7 is an import
cluster. -/
def flatEnv : FeeEnv :=
  { wealth := fun _ => 0, isImport := fun c => c == 7, curve := fun _ => FACTOR_SCALE,
    implied := fun _ => FACTOR_SCALE }

/-- ADR 0007's invariant, stated against what consensus checks: every
spend C6 accepts whose ring is fully import-tagged is charged at least `F`. -/
def ImportFloorHonoured (env : FeeEnv) : Prop :=
  ∀ (rings : List (List Coin)) (outs : List Coin), inheritOk rings outs = true →
    (∀ ring ∈ rings, ∀ x ∈ ring, x.tags.all (fun e => env.isImport e.cluster) = true ∧
      massP env.isImport x.tags = SCALE) →
    BRIDGE_IMPORT_FACTOR_FLOOR ≤ demurrageFactor env rings outs

/-- **#1687: ADR 0007's invariant is false** (proven (model)) in the flat
environment: a one-member ring fully tagged to import cluster 7, spent to
an untagged output, is accepted and charged at 1x. -/
theorem import_floor_invariant_refuted : ¬ ImportFloorHonoured flatEnv := by
  intro hinv
  have := hinv [[⟨10 ^ 12, [⟨7, SCALE⟩]⟩]] [⟨10 ^ 12, []⟩]
    (inheritOk_of_untagged _ _ (by intro x hx; simp at hx; subst hx; rfl))
    (by intro ring hr x hx; simp at hr; subst hr; simp at hx; subst hx; decide)
  rw [(tag_drop_demurrage_factor flatEnv _ _ (by intro x hx; simp at hx; subst hx; rfl)
    (by simp [sumN, U64_MAX])).2] at this
  simp [flatEnv, FACTOR_SCALE, BRIDGE_IMPORT_FACTOR_FLOOR] at this

/-! ## Protocol 8.0.0: Rules 1 and 2 (#1857, #1870; DRT model, no recorded theorems yet)

New definitions, so the records of the 7.x theorems above keep their
hashes. -/

/-- Each weight at most the previous one (`entry.weight > prev_weight`
rejects; the first entry is compared with `u32::MAX`, which a `u32` never
exceeds). -/
def sortedDesc : TagVec → Bool
  | [] => true
  | [_] => true
  | a :: b :: rest => decide (b.weight ≤ a.weight) && sortedDesc (b :: rest)

/-- Mirrors `ClusterTagVector::is_valid` (`cluster_tags.rs:550`): at most
`MAX_CLUSTER_TAGS` entries, every weight at least `MIN_STORED_WEIGHT`,
sorted by weight descending, distinct cluster ids, total at most `SCALE`.
Rust sums with `u32::saturating_add`; for weights below `2^32` the saturated
sum is at most `SCALE` iff the exact sum is, so the model uses `mass`. -/
def tagsValid (t : TagVec) : Bool :=
  decide (t.length ≤ MAX_CLUSTER_TAGS) &&
  t.all (fun e => decide (MIN_STORED_WEIGHT ≤ e.weight)) &&
  sortedDesc t &&
  decide (t.map Entry.cluster).Nodup &&
  decide (mass t ≤ SCALE)

/-- Rule 1, `first_malformed_output_tags`: the index of the first output
whose tags fail `is_valid`. -/
def firstMalformed (outs : List Coin) : Option Nat :=
  outs.findIdx? fun o => !tagsValid o.tags

/-- Rule 2 for one entry: its cluster is the import cluster `imp` or is
named by some entry of some ring member (presence, not mass). -/
def onRingOrImport (rings : List (List Coin)) (imp c : Nat) : Bool :=
  c == imp || (inputClusters rings).contains c

/-- Rule 2, `check_output_clusters_on_ring`: the first `(output index,
cluster)` that names a cluster neither on the ring nor `imp`, scanning
outputs and then entries in order. -/
def firstOffRing (rings : List (List Coin)) (outs : List Coin) (imp : Nat) : Option (Nat × Nat) :=
  let rec go : Nat → List Coin → Option (Nat × Nat)
    | _, [] => none
    | i, o :: os =>
      match o.tags.find? (fun e => !onRingOrImport rings imp e.cluster) with
      | some e => some (i, e.cluster)
      | none => go (i + 1) os
  go 0 outs

/-- Rule 2 holds. -/
def ruleTwoOk (rings : List (List Coin)) (outs : List Coin) (imp : Nat) : Bool :=
  outs.all fun o => o.tags.all fun e => onRingOrImport rings imp e.cluster

/-- The 8.0.0 tag verdict of one transfer transaction in a block whose epoch
import cluster is `imp` (`import_cluster_id_for_height(height)`, not
modelled: a parameter): Rule 1 on every output, then Rule 2, then the C6
mass check `inheritOk`, unchanged. -/
def inheritOk₈ (rings : List (List Coin)) (outs : List Coin) (imp : Nat) : Bool :=
  outs.all (fun o => tagsValid o.tags) && ruleTwoOk rings outs imp && inheritOk rings outs

end Botho.Tags
