/-
Cluster-tag vectors and the wallet's tag merge (#1840, epic #1704).

What this module mirrors, and what it does not:

| Lean (`Botho.Tags.*`) | Rust | Role | Status |
|---|---|---|---|
| `Entry`, `TagVec` | `ClusterTagEntry`, `ClusterTagVector::entries`, `transaction/types/src/cluster_tags.rs:51,113` | data | - |
| `fromPairs` | `ClusterTagVector::from_pairs`, `cluster_tags.rs:306` | wallet | tested (through `mergeWeighted`) |
| `mergeWeighted` | `ClusterTagVector::merge_weighted`, `cluster_tags.rs:355` | wallet, and the node's mempool (`botho/src/mempool.rs:1062`) | tested (DRT leg `merge_weighted`) |

`merge_weighted` is **not a consensus rule**. The stock wallets call it to
choose output tags (`botho/src/wallet.rs:357`, `botho-wallet/src/transaction.rs:261`,
decay rate `DEFAULT_CLUSTER_DECAY_RATE = 50_000`), and the mempool calls it
with rate 0 over a transaction's *output* tags to compare them with the ring
(`validate_clsag_inputs`). Block acceptance never calls it: a modified wallet
may write any output tags that pass C6 (`Botho.Tags.Inherit`). So the
theorems here say what an honest wallet does, and nothing about what a block
may contain.

Not mirrored (sim or research only; `git grep` finds no caller under
`botho/src`, `transaction`, `botho-wallet/src` or `core`): the
`cluster-tax` kernels `block_decay.rs` (`decay_factor`,
`BlockAwareTagVector::mix`), `age_decay.rs`, `entropy_decay.rs`, and the
floating-point `TagVector::mix` / `apply_decay` (`cluster-tax/src/tag.rs`),
which only the wallet's fee estimate uses. `merge_weighted_with_and_decay`
(`cluster_tags.rs:427`) has no production caller either. `decay_state` is not
modelled: `merge_weighted` returns `None` for it and no check reads it.

Integers. Weights are `u32` and values `u64` in Rust; the model computes on
`Nat` and states each width. The products `value * weight` are taken in
`u128` in Rust and cannot overflow there (`< 2^96`). The Rust operations that
can overflow are mirrored exactly:

- `inputs.iter().map(|(_, v)| *v).sum()` (`u64`) panics on overflow under
  `overflow-checks = true` (release, #663): `none`;
- `existing.1.saturating_add(contribution as u64)`: `satAdd U64_MAX`;
- `w * (retention as u64)` (`u64`) panics on overflow: `none`;
- `decayed as u32` keeps the low 32 bits: `% 2 ^ 32`.

Theorems (status: proven (model); the DRT leg `merge_weighted` ties the model
to the Rust function):

- `mergeWeighted_sub_convex`: for every predicate `p` on cluster ids (one
  cluster, or all of them), the merged weight on `p` times `SCALE * T` is at
  most `retention * Σ value_i * weight_i(p)`. The merge is a value-weighted
  combination that rounds down, prunes and truncates: never more than the
  convex combination, scaled by the retention.
- `mergeWeighted_mass_le_scale`: inputs of total weight at most 100% merge
  to at most 100%.
- `mergeWeighted_isSome`: such inputs, with a total value that fits a
  `u64` (every balancing transaction's does), never make it panic.
- `decayEntry_le`, `decayEntry_antitone`: decay never raises a weight below
  `2^32`, and a higher rate never gives a larger weight.

Refuted (named counterexamples):

- `mergeWeighted_truncation_halves_mass`: exact convexity fails, and the
  deficit is not bounded by the pruning threshold. Two equal inputs, each
  100% spread over 16 distinct clusters, merge to 50%: the 16-entry
  truncation drops half the mass.
- `mergeWeighted_overflow_panics`: one input whose vector repeats a cluster
  4,295 times at weight `u32::MAX` makes `w * retention` overflow `u64`, so
  the function panics (4,294 entries do not: `mergeWeighted_no_panic_4294`).
  Consensus does not bound the entries of an output (`is_valid` has no
  caller), and the mempool merges output tags, so a validly signed
  transaction reaches this panic on the node.
- `mergeWeighted_decay_not_antitone`: on a vector whose accumulated weight
  reaches `2^32` (duplicate entries), rate 0 wraps the weight to 0 while
  rate 1 keeps it: more decay, more weight.
-/
namespace Botho.Tags

/-! ## Fixed-width helpers

Local copies of the `Botho.Lottery.Arith` helpers (same definitions), so the
statement records of this module do not depend on another model's files. -/

/-- `u32::MAX`. -/
def U32_MAX : Nat := 2 ^ 32 - 1

/-- `u64::MAX`. -/
def U64_MAX : Nat := 2 ^ 64 - 1

/-- `u128::MAX`. -/
def U128_MAX : Nat := 2 ^ 128 - 1

/-- Rust `a.saturating_sub(b)`. -/
def satSub (a b : Nat) : Nat :=
  if b ≤ a then a - b else 0

/-- Rust `a.saturating_add(b)` on the width whose maximum is `max`. -/
def satAdd (max a b : Nat) : Nat :=
  if a + b ≤ max then a + b else max

/-- Rust `a.saturating_mul(b)` on the width whose maximum is `max`. -/
def satMul (max a b : Nat) : Nat :=
  if a * b ≤ max then a * b else max

/-- Rust `x as u64` from a wider unsigned integer: the low 64 bits. -/
def truncU64 (x : Nat) : Nat :=
  x % 2 ^ 64

/-- Sum of a list of naturals, by structural recursion. -/
def sumN : List Nat → Nat
  | [] => 0
  | x :: xs => x + sumN xs

/-- `TAG_WEIGHT_SCALE` (`cluster_tags.rs:33`): 100% in parts per million. -/
def SCALE : Nat := 1000000

/-- `MIN_STORED_WEIGHT` (`cluster_tags.rs:30`). -/
def MIN_STORED_WEIGHT : Nat := 1000

/-- `MAX_CLUSTER_TAGS` (`cluster_tags.rs:26`). -/
def MAX_CLUSTER_TAGS : Nat := 16

/-- `ClusterTagEntry`: a cluster id (`u64`) and a weight (`u32`, parts per
`SCALE`). -/
structure Entry where
  cluster : Nat
  weight : Nat
  deriving DecidableEq, Repr

/-- `ClusterTagVector::entries`, in order. Nothing forces the Rust invariants
(sorted, distinct, at most 16, total at most `SCALE`) on a vector read from
the chain: the node never calls `is_valid` (#1840). -/
abbrev TagVec := List Entry

/-- The weight a vector puts on the clusters satisfying `p`, summing
duplicate entries. `massP (· == c)` is cluster `c`'s weight;
`mass` is the total. -/
def massP (p : Nat → Bool) : TagVec → Nat
  | [] => 0
  | e :: es => (if p e.cluster then e.weight else 0) + massP p es

/-- `ClusterTagVector::total_weight` without the `u32` saturation. -/
def mass (t : TagVec) : Nat := massP (fun _ => true) t

/-! ## `from_pairs` -/

/-- One step of a stable sort by weight, descending: insert `x` after every
entry of weight `≥ x.weight`. -/
def insertDesc (x : Entry) : TagVec → TagVec
  | [] => [x]
  | y :: ys => if y.weight < x.weight then x :: y :: ys else y :: insertDesc x ys

/-- Insertion sort, left to right. -/
def sortDescAux : TagVec → TagVec → TagVec
  | acc, [] => acc
  | acc, x :: xs => sortDescAux (insertDesc x acc) xs

/-- `entries.sort_by_key(|e| Reverse(e.weight))`: Rust's `sort_by_key` is
stable, so entries of equal weight keep their order, as here. -/
def sortDesc (l : TagVec) : TagVec := sortDescAux [] l

/-- Mirrors `ClusterTagVector::from_pairs` (`cluster_tags.rs:306`): drop
weights below `MIN_STORED_WEIGHT`, sort by weight descending (stable), keep
the first `MAX_CLUSTER_TAGS`. -/
def fromPairs (ps : TagVec) : TagVec :=
  (sortDesc (ps.filter fun e => decide (MIN_STORED_WEIGHT ≤ e.weight))).take MAX_CLUSTER_TAGS

/-! ## `merge_weighted` -/

/-- The accumulator step: add `x` to the first entry of cluster `c`
(saturating `u64`), or append `⟨c, x⟩` (`Vec::iter_mut().find`, `push`). -/
def addTo (c x : Nat) : TagVec → TagVec
  | [] => [⟨c, x⟩]
  | e :: es =>
    if e.cluster = c then ⟨e.cluster, satAdd U64_MAX e.weight x⟩ :: es
    else e :: addTo c x es

/-- One input's contributions: `((value as u128) * (weight as u128)) /
(total_value as u128)`, then `as u64` (lossless: it is at most the weight). -/
def accEntries (T v : Nat) (acc : TagVec) : TagVec → TagVec
  | [] => acc
  | e :: es => accEntries T v (addTo e.cluster (truncU64 (v * e.weight / T)) acc) es

/-- All inputs' contributions, in input order. -/
def accumulate (T : Nat) (acc : TagVec) : List (TagVec × Nat) → TagVec
  | [] => acc
  | (tags, v) :: rest => accumulate T (accEntries T v acc tags) rest

/-- `TAG_WEIGHT_SCALE.saturating_sub(decay_rate)` (`u32`). -/
def retention (rate : Nat) : Nat := satSub SCALE rate

/-- The decay of one accumulated weight: `((w * retention) / SCALE) as u32`
(the product is checked separately, see `overflows`). -/
def decayEntry (r : Nat) (e : Entry) : Entry :=
  ⟨e.cluster, (e.weight * r / SCALE) % 2 ^ 32⟩

/-- Whether `w * retention` overflows `u64` for some accumulated weight (a
panic under `overflow-checks = true`). -/
def overflows (r : Nat) : TagVec → Bool
  | [] => false
  | e :: es => decide (U64_MAX < e.weight * r) || overflows r es

/-- `inputs.iter().map(|(_, v)| *v).sum()` without the overflow check. -/
def totalValue (inputs : List (TagVec × Nat)) : Nat := sumN (inputs.map Prod.snd)

/-- `Σ value_i * weight_i(p)`: `T` times the exact value-weighted
combination on `p`. -/
def weightedP (p : Nat → Bool) : List (TagVec × Nat) → Nat
  | [] => 0
  | (tags, v) :: rest => v * massP p tags + weightedP p rest

/-- Mirrors `ClusterTagVector::merge_weighted` (`cluster_tags.rs:355`).
`none` is a Rust panic (overflow of the `u64` value sum, or of
`w * retention`). Weights must be below `2^32` and values below `2^64`, as
the Rust types force. -/
def mergeWeighted (inputs : List (TagVec × Nat)) (rate : Nat) : Option TagVec :=
  let T := totalValue inputs
  if U64_MAX < T then none
  else if T = 0 then some []
  else
    let acc := accumulate T [] inputs
    let r := retention rate
    if overflows r acc then none
    else some (fromPairs (acc.map (decayEntry r)))

/-! ## Lemmas -/

theorem massP_cons (p : Nat → Bool) (e : Entry) (es : TagVec) :
    massP p (e :: es) = (if p e.cluster then e.weight else 0) + massP p es := rfl

theorem massP_insertDesc (p : Nat → Bool) (x : Entry) (l : TagVec) :
    massP p (insertDesc x l) = massP p [x] + massP p l := by
  induction l with
  | nil => rfl
  | cons y ys ih =>
    unfold insertDesc
    split
    · simp only [massP]; omega
    · simp only [ih, massP]; omega

theorem massP_sortDescAux (p : Nat → Bool) (acc l : TagVec) :
    massP p (sortDescAux acc l) = massP p acc + massP p l := by
  induction l generalizing acc with
  | nil => simp [sortDescAux, massP]
  | cons x xs ih =>
    simp only [sortDescAux, ih, massP_insertDesc, massP]; omega

theorem massP_sortDesc (p : Nat → Bool) (l : TagVec) : massP p (sortDesc l) = massP p l := by
  simp [sortDesc, massP_sortDescAux, massP]

theorem massP_filter_le (p : Nat → Bool) (q : Entry → Bool) (l : TagVec) :
    massP p (l.filter q) ≤ massP p l := by
  induction l with
  | nil => simp [massP]
  | cons x xs ih =>
    simp only [List.filter_cons]
    split
    · simp only [massP_cons]; omega
    · simp only [massP_cons]; omega

theorem massP_take_le (p : Nat → Bool) (n : Nat) (l : TagVec) :
    massP p (l.take n) ≤ massP p l := by
  induction l generalizing n with
  | nil => simp [massP]
  | cons x xs ih =>
    cases n with
    | zero => simp [massP]
    | succ n => simp only [List.take_succ_cons, massP_cons]; have := ih n; omega

theorem massP_fromPairs_le (p : Nat → Bool) (ps : TagVec) :
    massP p (fromPairs ps) ≤ massP p ps := by
  unfold fromPairs
  have h1 := massP_take_le p MAX_CLUSTER_TAGS
    (sortDesc (ps.filter fun e => decide (MIN_STORED_WEIGHT ≤ e.weight)))
  rw [massP_sortDesc] at h1
  have h2 := massP_filter_le p (fun e => decide (MIN_STORED_WEIGHT ≤ e.weight)) ps
  omega

theorem decay_weight_le (r w : Nat) : (w * r / SCALE) % 2 ^ 32 * SCALE ≤ r * w := by
  have h1 : (w * r / SCALE) % 2 ^ 32 ≤ w * r / SCALE := Nat.mod_le _ _
  have h2 : w * r / SCALE * SCALE ≤ w * r := Nat.div_mul_le_self _ _
  have h3 : (w * r / SCALE) % 2 ^ 32 * SCALE ≤ w * r / SCALE * SCALE :=
    Nat.mul_le_mul_right _ h1
  rw [Nat.mul_comm r w]; omega

theorem massP_map_decay (p : Nat → Bool) (r : Nat) (acc : TagVec) :
    massP p (acc.map (decayEntry r)) * SCALE ≤ r * massP p acc := by
  induction acc with
  | nil => simp [massP]
  | cons e es ih =>
    simp only [List.map_cons, massP_cons, decayEntry]
    rw [Nat.add_mul, Nat.mul_add]
    have h := decay_weight_le r e.weight
    split
    · omega
    · simp only [Nat.zero_mul, Nat.mul_zero]; omega

theorem satAdd_le_add (m a b : Nat) : satAdd m a b ≤ a + b := by
  unfold satAdd; split <;> omega

theorem massP_addTo (p : Nat → Bool) (c x : Nat) (acc : TagVec) :
    massP p (addTo c x acc) ≤ massP p acc + (if p c then x else 0) := by
  induction acc with
  | nil => simp [addTo, massP]
  | cons e es ih =>
    unfold addTo
    split
    · rename_i h
      subst h
      simp only [massP_cons]
      have := satAdd_le_add U64_MAX e.weight x
      split <;> omega
    · simp only [massP_cons]; omega

theorem truncU64_le (x : Nat) : truncU64 x ≤ x := Nat.mod_le _ _

theorem contribution_mul_le (T v w : Nat) : truncU64 (v * w / T) * T ≤ v * w := by
  have h1 := truncU64_le (v * w / T)
  have h2 : v * w / T * T ≤ v * w := Nat.div_mul_le_self _ _
  have h3 : truncU64 (v * w / T) * T ≤ v * w / T * T := Nat.mul_le_mul_right _ h1
  omega

theorem massP_accEntries (p : Nat → Bool) (T v : Nat) (acc tags : TagVec) :
    massP p (accEntries T v acc tags) * T ≤ massP p acc * T + v * massP p tags := by
  induction tags generalizing acc with
  | nil => simp [accEntries, massP]
  | cons e es ih =>
    simp only [accEntries, massP_cons]
    have h1 := ih (addTo e.cluster (truncU64 (v * e.weight / T)) acc)
    have h2 := massP_addTo p e.cluster (truncU64 (v * e.weight / T)) acc
    have h3 : massP p (addTo e.cluster (truncU64 (v * e.weight / T)) acc) * T ≤
        (massP p acc + (if p e.cluster then truncU64 (v * e.weight / T) else 0)) * T :=
      Nat.mul_le_mul_right _ h2
    have h4 := contribution_mul_le T v e.weight
    rw [Nat.add_mul] at h3
    rw [Nat.mul_add]
    split
    · rename_i hp; simp only [hp, ite_true] at h3; omega
    · rename_i hp; simp only [hp] at h3; simp at h3; omega

theorem massP_accumulate (p : Nat → Bool) (T : Nat) (acc : TagVec) (inputs : List (TagVec × Nat)) :
    massP p (accumulate T acc inputs) * T ≤ massP p acc * T + weightedP p inputs := by
  induction inputs generalizing acc with
  | nil => simp [accumulate, weightedP]
  | cons x rest ih =>
    obtain ⟨tags, v⟩ := x
    simp only [accumulate, weightedP]
    have h1 := ih (accEntries T v acc tags)
    have h2 := massP_accEntries p T v acc tags
    omega

/-! ## Theorems -/

/-- **Sub-convexity** (proven (model)): for every predicate `p` on cluster
ids, `merge_weighted`'s weight on `p`, times `SCALE * T`, is at most
`retention * Σ value_i * weight_i(p)`. With `p = (· == c)` this is "cluster
`c`'s merged weight is at most its value-weighted average, decayed"; with
`p = fun _ => true` it is the same for the total. -/
theorem mergeWeighted_sub_convex (p : Nat → Bool) (inputs : List (TagVec × Nat))
    (rate : Nat) (out : TagVec) (h : mergeWeighted inputs rate = some out) :
    massP p out * SCALE * totalValue inputs ≤ retention rate * weightedP p inputs := by
  unfold mergeWeighted at h
  simp only at h
  split at h
  · cases h
  · split at h
    · rename_i hT; cases h; simp [massP, hT]
    · split at h
      · cases h
      · cases h
        have h1 := massP_fromPairs_le p
          ((accumulate (totalValue inputs) [] inputs).map (decayEntry (retention rate)))
        have h2 := massP_map_decay p (retention rate) (accumulate (totalValue inputs) [] inputs)
        have h3 := massP_accumulate p (totalValue inputs) [] inputs
        simp only [massP, Nat.zero_mul, Nat.zero_add] at h3
        have h4 : massP p (fromPairs ((accumulate (totalValue inputs) [] inputs).map
            (decayEntry (retention rate)))) * SCALE ≤
            retention rate * massP p (accumulate (totalValue inputs) [] inputs) :=
          Nat.le_trans (Nat.mul_le_mul_right _ h1) h2
        have h5 := Nat.mul_le_mul_right (totalValue inputs) h4
        rw [Nat.mul_assoc (retention rate)] at h5
        exact Nat.le_trans h5 (Nat.mul_le_mul_left _ h3)

theorem retention_le (rate : Nat) : retention rate ≤ SCALE := by
  unfold retention satSub; split <;> omega

theorem weightedP_le (p : Nat → Bool) (inputs : List (TagVec × Nat))
    (h : ∀ x ∈ inputs, massP p x.1 ≤ SCALE) :
    weightedP p inputs ≤ SCALE * totalValue inputs := by
  induction inputs with
  | nil => simp [weightedP, totalValue, sumN]
  | cons x rest ih =>
    obtain ⟨tags, v⟩ := x
    have hx : massP p tags ≤ SCALE := h (tags, v) (List.mem_cons_self ..)
    have hr := ih (fun y hy => h y (List.mem_cons_of_mem _ hy))
    simp only [weightedP, totalValue, List.map_cons, sumN] at hr ⊢
    have : v * massP p tags ≤ v * SCALE := Nat.mul_le_mul_left _ hx
    rw [Nat.mul_add]; rw [Nat.mul_comm v SCALE] at this; omega

theorem mergeWeighted_zero (inputs : List (TagVec × Nat)) (rate : Nat)
    (h0 : totalValue inputs = 0) : mergeWeighted inputs rate = some [] := by
  unfold mergeWeighted; simp [h0, U64_MAX]

/-- **Mass bound for the wallet** (proven (model)): if every input's weight
on `p` is at most 100%, so is the merged vector's. Consensus does not
require it (`Botho.Tags.inherit_admits_fresh_mass_above_one`). -/
theorem mergeWeighted_mass_le_scale (p : Nat → Bool) (inputs : List (TagVec × Nat))
    (rate : Nat) (out : TagVec) (hin : ∀ x ∈ inputs, massP p x.1 ≤ SCALE)
    (h : mergeWeighted inputs rate = some out) : massP p out ≤ SCALE := by
  by_cases hT : totalValue inputs = 0
  · rw [mergeWeighted_zero inputs rate hT] at h; cases h; simp [massP]
  · have h1 := mergeWeighted_sub_convex p inputs rate out h
    have h2 := weightedP_le p inputs hin
    have h3 := retention_le rate
    have h4 : retention rate * weightedP p inputs ≤ SCALE * (SCALE * totalValue inputs) :=
      Nat.mul_le_mul h3 h2
    have h5 : massP p out * SCALE * totalValue inputs ≤ SCALE * SCALE * totalValue inputs := by
      rw [Nat.mul_assoc SCALE SCALE]; omega
    have hpos : 0 < totalValue inputs := Nat.pos_of_ne_zero hT
    have h6 := Nat.le_of_mul_le_mul_right h5 hpos
    have h7 : massP p out * SCALE ≤ SCALE * SCALE := h6
    exact Nat.le_of_mul_le_mul_right h7 (by decide)

theorem weight_le_mass (e : Entry) (l : TagVec) (h : e ∈ l) : e.weight ≤ mass l := by
  induction l with
  | nil => cases h
  | cons x xs ih =>
    unfold mass at *
    simp only [massP_cons, ite_true]
    cases h with
    | head => omega
    | tail _ h' => have := ih h'; omega

theorem overflows_false (r : Nat) (l : TagVec) (h : ∀ e ∈ l, e.weight * r ≤ U64_MAX) :
    overflows r l = false := by
  induction l with
  | nil => rfl
  | cons x xs ih =>
    simp only [overflows, Bool.or_eq_false_iff, decide_eq_false_iff_not, Nat.not_lt]
    exact ⟨h x (List.mem_cons_self ..), ih (fun e he => h e (List.mem_cons_of_mem _ he))⟩

/-- **No panic for well-formed inputs** (proven (model)): if the values sum
to at most `u64::MAX` (true of every transaction that balances, since
`verify_ring_signatures` rejects a larger output sum) and every input has
total weight at most 100%, `merge_weighted` does not panic. -/
theorem mergeWeighted_isSome (inputs : List (TagVec × Nat)) (rate : Nat)
    (hT : totalValue inputs ≤ U64_MAX) (hin : ∀ x ∈ inputs, mass x.1 ≤ SCALE) :
    (mergeWeighted inputs rate).isSome = true := by
  by_cases h0 : totalValue inputs = 0
  · rw [mergeWeighted_zero inputs rate h0]; rfl
  · have hpos : 0 < totalValue inputs := Nat.pos_of_ne_zero h0
    have hacc := massP_accumulate (fun _ => true) (totalValue inputs) [] inputs
    simp only [massP, Nat.zero_mul, Nat.zero_add] at hacc
    have hw := weightedP_le (fun _ => true) inputs hin
    have hm : mass (accumulate (totalValue inputs) [] inputs) ≤ SCALE := by
      have : mass (accumulate (totalValue inputs) [] inputs) * totalValue inputs ≤
          SCALE * totalValue inputs := Nat.le_trans hacc hw
      exact Nat.le_of_mul_le_mul_right this hpos
    have hov : overflows (retention rate) (accumulate (totalValue inputs) [] inputs) = false := by
      apply overflows_false
      intro e he
      have h1 := weight_le_mass e _ he
      have h2 := retention_le rate
      have h3 : e.weight * retention rate ≤ SCALE * SCALE := Nat.mul_le_mul (by omega) h2
      have : SCALE * SCALE ≤ U64_MAX := by decide
      omega
    unfold mergeWeighted
    simp only
    have hT' : ¬ U64_MAX < totalValue inputs := by omega
    simp [hT', h0, hov]

/-- **Decay never raises a weight** (proven (model)). -/
theorem decayEntry_le (rate : Nat) (e : Entry) :
    (decayEntry (retention rate) e).weight ≤ e.weight := by
  simp only [decayEntry]
  have h1 : e.weight * retention rate ≤ e.weight * SCALE :=
    Nat.mul_le_mul_left _ (retention_le rate)
  have h2 : e.weight * retention rate / SCALE ≤ e.weight * SCALE / SCALE := Nat.div_le_div_right h1
  rw [Nat.mul_div_cancel _ (by decide : 0 < SCALE)] at h2
  exact Nat.le_trans (Nat.mod_le _ _) h2

theorem retention_antitone {r₁ r₂ : Nat} (h : r₁ ≤ r₂) : retention r₂ ≤ retention r₁ := by
  unfold retention satSub; split <;> split <;> omega

/-- **A higher rate never gives a larger weight**, for weights below `2^32`
(proven (model)). Fails for larger accumulated weights:
`mergeWeighted_decay_not_antitone`. -/
theorem decayEntry_antitone (e : Entry) (h : e.weight ≤ U32_MAX) {r₁ r₂ : Nat} (hr : r₁ ≤ r₂) :
    (decayEntry (retention r₂) e).weight ≤ (decayEntry (retention r₁) e).weight := by
  simp only [decayEntry]
  have hlt : ∀ r, r ≤ SCALE → e.weight * r / SCALE < 2 ^ 32 := by
    intro r hr'
    have h1 : e.weight * r ≤ e.weight * SCALE := Nat.mul_le_mul_left _ hr'
    have h2 : e.weight * r / SCALE ≤ e.weight * SCALE / SCALE := Nat.div_le_div_right h1
    rw [Nat.mul_div_cancel _ (by decide : 0 < SCALE)] at h2
    unfold U32_MAX at h; omega
  rw [Nat.mod_eq_of_lt (hlt _ (retention_le r₂)), Nat.mod_eq_of_lt (hlt _ (retention_le r₁))]
  exact Nat.div_le_div_right (Nat.mul_le_mul_left _ (retention_antitone hr))

/-! ## Counterexamples -/

/-- Sixteen distinct clusters `base, ..., base + 15`, each at 6.25%: a valid
vector of total weight exactly 100%. -/
def spread16 (base : Nat) : TagVec :=
  (List.range 16).map fun i => ⟨base + i, 62500⟩

/-- Two equal-value inputs, each 100% over its own 16 clusters. -/
def truncationExample : List (TagVec × Nat) := [(spread16 0, 1), (spread16 16, 1)]

/-- **Exact convexity is refuted** (proven (model)): both inputs carry 100%
and the value-weighted combination is 100%, but `merge_weighted` (rate 0,
as the mempool calls it) returns 50%: the 32 contributions of 3.125% each
are truncated to 16. So the deficit is not bounded by the pruning
threshold. -/
theorem mergeWeighted_truncation_halves_mass :
    (∀ x ∈ truncationExample, mass x.1 = SCALE) ∧
    (mergeWeighted truncationExample 0).map mass = some (SCALE / 2) := by
  decide

/-- One input repeating cluster 7 `k` times at weight `u32::MAX`, value 1. -/
def repeatedMax (k : Nat) : List (TagVec × Nat) := [(List.replicate k ⟨7, U32_MAX⟩, 1)]

theorem satAdd_satAdd (m a b c : Nat) :
    satAdd m (satAdd m a b) c = satAdd m a (b + c) := by
  unfold satAdd; split <;> split <;> split <;> omega

theorem accEntries_repeatedMax (k w : Nat) (hw : w ≤ U64_MAX) :
    accEntries 1 1 [⟨7, w⟩] (List.replicate k ⟨7, U32_MAX⟩) =
      [⟨7, satAdd U64_MAX w (k * U32_MAX)⟩] := by
  induction k generalizing w with
  | zero => simp [accEntries, satAdd]; omega
  | succ k ih =>
    have hx : truncU64 (1 * U32_MAX / 1) = U32_MAX := by decide
    have hs : satAdd U64_MAX w U32_MAX ≤ U64_MAX := by unfold satAdd; split <;> omega
    simp only [List.replicate_succ, accEntries, hx, addTo, ite_true]
    rw [ih _ hs]
    rw [satAdd_satAdd, Nat.succ_mul, Nat.add_comm U32_MAX]

theorem accumulate_repeatedMax (k : Nat) :
    accumulate 1 [] (repeatedMax (k + 1)) = [⟨7, satAdd U64_MAX U32_MAX (k * U32_MAX)⟩] := by
  have hx : truncU64 (1 * U32_MAX / 1) = U32_MAX := by decide
  simp only [repeatedMax, accumulate, List.replicate_succ, accEntries, hx, addTo]
  exact accEntries_repeatedMax k U32_MAX (by decide)

theorem totalValue_repeatedMax (k : Nat) : totalValue (repeatedMax k) = 1 := rfl

/-- **`merge_weighted` panics on 4,295 repeated `u32::MAX` entries**
(proven (model)): the accumulated weight `4295 * (2^32 - 1)` times the
retention `10^6` exceeds `u64::MAX`. The node's mempool merges a
transaction's output tags with rate 0, and consensus does not bound an
output's entries, so this input is reachable from a signed transaction
(#1840; end to end in `botho/src/ledger/cluster_tag_invariant_tests.rs`). -/
theorem mergeWeighted_overflow_panics : mergeWeighted (repeatedMax 4295) 0 = none := by
  unfold mergeWeighted
  simp only [totalValue_repeatedMax, accumulate_repeatedMax 4294]
  decide

/-- The threshold of `mergeWeighted_overflow_panics` is exact
(proven (model)). -/
theorem mergeWeighted_no_panic_4294 : (mergeWeighted (repeatedMax 4294) 0).isSome = true := by
  unfold mergeWeighted
  simp only [totalValue_repeatedMax, accumulate_repeatedMax 4293]
  decide

/-- An accumulated weight of exactly `2^32`, from two duplicate entries. -/
def wrapExample : List (TagVec × Nat) := [([⟨5, 2 ^ 31⟩, ⟨5, 2 ^ 31⟩], 1)]

/-- **Decay is not antitone in the rate on malformed vectors**
(proven (model)): at rate 0 the `as u32` cast wraps cluster 5's weight
`2^32` to 0 and the entry is pruned; at rate 1 it is kept at 4,294,962,999. -/
theorem mergeWeighted_decay_not_antitone :
    mergeWeighted wrapExample 0 = some [] ∧
    mergeWeighted wrapExample 1 = some [⟨5, 4294963001⟩] := by
  decide

end Botho.Tags
