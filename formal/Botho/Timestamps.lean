import Botho.Basic

/-!
Protocol 8.0.0 timestamp rules, retained in 9.0.0 (#1941).
Status: proven (model); the zero-skew claim is refuted in the timestamp-rule
model. The `timestamps` DRT leg (#1942, #1943) calls five pure helpers and the
production scalar lower/future predicates: full-u64 ledger inputs, minting
only at validator now ≤ U-60, generated windows of length 0..32 and pinned
boundaries. Above U-60 the minting subrequest returns an explicit domain error;
helper and saturated ledger observations remain available. This finite testing
does not prove Rust equivalence. Clock acquisition/failure, full validators,
SCP scheduling and physical-clock realizability remain outside DRT coverage.

`ledger/mod.rs::median_time_past_of`, `next_min_timestamp`, and
`rolled_timestamps` supply the window rules; `node/minter.rs::minting_timestamp`
supplies the honest stamp. `ledger/store/validation.rs` C2b supplies only the
scalar timestamp conjunction modeled here. Header equality, errors, clock-read
order, PoW and complete ledger/SCP/physical-clock realizability are excluded.
`consensus/validation.rs` uses ordinary now+60: intrinsic validation has only
the upper bound; full validation checks it before MTP. Agreement is restricted
to now ≤ U-60. `block.rs::difficulty::observed_block_time` supplies the gap.
-/
namespace Botho.Timestamps

/-- Timestamps are naturals with explicit u64 bounds at the public claims. -/
def U : Nat := 2^64 - 1
def Bounded (w : List Nat) : Prop := ∀ t ∈ w, t ≤ U

/-- Oldest first: discard any prefix before the last eleven entries. -/
def tail11 (w : List Nat) : List Nat := w.drop (w.length - 11)
def slots (w : List Nat) : List Nat :=
  List.replicate (11 - (tail11 w).length) 0 ++ tail11 w

def mtp (w : List Nat) : Nat := ((slots w).mergeSort).getD 5 0
def roll (w : List Nat) (t : Nat) : List Nat := w.drop (w.length - 10) ++ [t]
def futureCap (v : Nat) : Nat := Botho.satAdd U v 60
def nextMin (w : List Nat) : Nat := Botho.satAdd U (mtp w) 1
def honestStamp (w : List Nat) (n : Nat) : Nat := max n (nextMin w)
def accepts (w : List Nat) (t v : Nat) : Prop := mtp w < t ∧ t ≤ futureCap v
instance (w : List Nat) (t v : Nat) : Decidable (accepts w t v) :=
  inferInstanceAs (Decidable (_ ∧ _))

def observedGap (parent stamp : Nat) : Option Nat :=
  if parent = 0 then none else some (max (stamp - parent) 3)

private theorem slots_length (w : List Nat) : (slots w).length = 11 := by
  simp only [slots, tail11, List.length_append, List.length_replicate, List.length_drop]
  omega

private theorem median_index (w : List Nat) : 5 < ((slots w).mergeSort).length := by
  rw [List.length_mergeSort, slots_length]; decide

private theorem getD_mem (l : List Nat) (i : Nat) (hi : i < l.length) :
    l.getD i 0 ∈ l := by
  simp [List.getD_eq_getElem?_getD, List.getElem?_eq_getElem hi]

/-- Rank characterization, proved for every sorted finite list and valid index. -/
private theorem rank (l : List Nat) (hs : l.Pairwise (· ≤ ·)) (i k : Nat)
    (hi : i < l.length) :
    k ≤ l.getD i 0 ↔ l.countP (fun x => x < k) ≤ i := by
  induction l generalizing i with
  | nil => simp at hi
  | cons a l ih =>
    obtain ⟨ha, hs⟩ := List.pairwise_cons.mp hs
    by_cases hak : a < k
    · cases i with
      | zero => simp [hak]
      | succ i =>
        have hi' : i < l.length := by simpa using hi
        have hr := ih hs i hi'
        simpa [List.countP_cons, hak] using hr
    · have hc : l.countP (fun x => x < k) = 0 := List.countP_eq_zero.mpr (by
        intro x hx
        have := ha x hx
        simp only [decide_eq_true_eq]
        omega)
      have hm : k ≤ (a :: l).getD i 0 := by
        have hx := getD_mem (a :: l) i hi
        rcases List.mem_cons.mp hx with he | he
        · omega
        · have := ha _ he; omega
      simp only [List.countP_cons, show decide (a < k) = false by simp [hak],
        Bool.false_eq_true, ite_false, hc, Nat.zero_add, Nat.zero_le, iff_true]
      exact hm

private theorem sorted (w : List Nat) : ((slots w).mergeSort).Pairwise (· ≤ ·) := by
  have h := List.pairwise_mergeSort (le := fun a b : Nat => a ≤ b)
    (by intros; simp_all; omega) (by intros; simp; omega) (slots w)
  simpa using h

private theorem median_rank (w : List Nat) (k : Nat) :
    k ≤ mtp w ↔ (slots w).countP (fun x => x < k) ≤ 5 := by
  have h := rank ((slots w).mergeSort) (sorted w) 5 k (median_index w)
  rw [(List.mergeSort_perm (slots w) _).countP_eq] at h
  exact h

private theorem slots_roll (w : List Nat) (t : Nat) :
    slots (roll w t) = (slots w).drop 1 ++ [t] := by
  have small (l : List Nat) (h : l.length ≤ 11) :
      slots l = List.replicate (11 - l.length) 0 ++ l := by
    simp [slots, tail11, Nat.sub_eq_zero_of_le h]
  by_cases h : w.length ≤ 10
  · have h10 : w.length - 10 = 0 := by omega
    have hr : (roll w t).length ≤ 11 := by simp [roll, h10]; omega
    rw [small w (by omega), small (roll w t) hr]
    simp only [roll, h10, List.drop_zero, List.length_append,
      List.length_cons, List.length_nil]
    rw [List.drop_append_of_le_length (by simp; omega)]
    simp only [List.drop_replicate, List.append_assoc]
    congr 2
  · have hl : (w.drop (w.length - 10)).length = 10 := by simp; omega
    have hr : (roll w t).length = 11 := by simp [roll, hl]
    rw [small (roll w t) (by omega)]
    have ht : w.length - (w.length - 11) = 11 := by omega
    simp only [Nat.sub_self, List.replicate_zero, List.nil_append,
      slots, tail11, List.length_drop, ht, List.drop_drop, roll]
    simp only [List.length_append, hl, List.length_singleton]
    change w.drop (w.length - 10) ++ [t] = w.drop (w.length - 11 + 1) ++ [t]
    rw [show w.length - 10 = w.length - 11 + 1 by omega]

/-- The six zero pads force the fixed median to zero for at most five entries. -/
theorem short_window_zero (w : List Nat) (_hw : Bounded w) (hl : w.length ≤ 5) :
    mtp w = 0 := by
  have hr := median_rank w 1
  have hd : w.length - 11 = 0 := by omega
  simp only [slots, tail11, hd, List.drop_zero, List.countP_append,
    List.countP_replicate] at hr
  simp only [show decide (0 < 1) = true from rfl, ite_true] at hr
  omega

/-- Arbitrary bounded windows, with no sortedness or reachability premise. -/
theorem rolling_mtp_monotone (w : List Nat) (t : Nat) (_hw : Bounded w)
    (_ht : t ≤ U) (ha : mtp w < t) : mtp w ≤ mtp (roll w t) := by
  apply (median_rank _ _).mpr
  have hc := (median_rank w (mtp w)).mp (Nat.le_refl _)
  have hd := (List.drop_sublist 1 (slots w)).countP_le (p := fun x => x < mtp w)
  rw [slots_roll]
  simp only [List.countP_append, List.countP_cons, List.countP_nil]
  have h : ¬ t < mtp w := by omega
  simp only [h, decide_false, Bool.false_eq_true, ite_false, Nat.add_zero]
  omega

private theorem mtp_bounded (w : List Nat) (hw : Bounded w) : mtp w ≤ U := by
  have hm := (List.mergeSort_perm (slots w) _).mem_iff.mp
    (getD_mem ((slots w).mergeSort) 5 (median_index w))
  rcases List.mem_append.mp hm with hm | hm
  · have hz := (List.mem_replicate.mp hm).2
    change mtp w = 0 at hz
    rw [hz]; omega
  · exact hw _ (List.mem_of_mem_drop hm)

/-- Exact headroom, including saturation at the maximum u64 timestamp. -/
theorem honest_stamp_headroom_iff (w : List Nat) (n v : Nat) (hw : Bounded w)
    (_hn : n ≤ U) (_hv : v ≤ U) :
    accepts w (honestStamp w n) v ↔ (mtp w < futureCap v ∧ n ≤ futureCap v) := by
  have hm := mtp_bounded w hw
  unfold accepts honestStamp nextMin futureCap Botho.satAdd
  omega

/-- Every step checks bounds and the timestamp predicate at its supplied clock.
The earlier stamps are not asserted to have been honestly generated. -/
def acceptedHistory : List Nat → List (Nat × Nat) → Prop
  | w, [] => Bounded w
  | w, (t, v) :: rest =>
    Bounded w ∧ t ≤ U ∧ v ≤ U ∧ accepts w t v ∧ acceptedHistory (roll w t) rest

def replay (w : List Nat) (history : List (Nat × Nat)) : List Nat :=
  history.foldl (fun window step => roll window step.1) w

def zeroSkewHistory : List (Nat × Nat) := List.replicate 6 (1060, 1000)
def reachedWindow : List Nat := replay [] zeroSkewHistory

/-- Typed refutation of skew-only acceptance in the scalar timestamp-rule model.
Six accepted transitions reach MTP 1060; equal next clocks cannot accept 1061.
No claim is made about full ledger/SCP, PoW, pacing or physical-clock realizability. -/
theorem zero_skew_counterexample :
    acceptedHistory [] zeroSkewHistory ∧
    reachedWindow = List.replicate 6 1060 ∧ Bounded reachedWindow ∧
    mtp reachedWindow = 1060 ∧
    (let minterNow := 1000; let validatorNow := 1000;
      minterNow ≤ U ∧ validatorNow ≤ U ∧
      max minterNow validatorNow - min minterNow validatorNow = 0 ∧
      max minterNow validatorNow - min minterNow validatorNow < 60 ∧
      honestStamp reachedWindow minterNow = 1061 ∧
      ¬ accepts reachedWindow (honestStamp reachedWindow minterNow) validatorNow) := by
  simp +decide [acceptedHistory, zeroSkewHistory, reachedWindow, replay, Bounded,
    accepts, honestStamp, nextMin, futureCap, Botho.satAdd, roll, mtp, slots, tail11,
    List.mergeSort, U]

/-- The floor concerns Some results only; a zero parent remains None. -/
theorem some_observed_gap_floor (parent stamp gap : Nat) (_hp : parent ≤ U)
    (_ht : stamp ≤ U) (hg : observedGap parent stamp = some gap) : 3 ≤ gap := by
  unfold observedGap at hg
  split at hg
  · contradiction
  · simp only [Option.some.injEq] at hg
    omega

-- Kernel examples supplement the universal proofs: lengths 0, 1, 5, 6, 10, 11, 13.
example : mtp [] = 0 ∧ mtp [42] = 0 ∧ mtp [9, 0, 9, 1, 2] = 0 ∧
    mtp [9, 1, 9, 2, 8, 3] = 1 ∧
    mtp [9, 1, 9, 2, 8, 3, 7, 4, 6, 5] = 5 ∧
    mtp [9, 1, 9, 2, 8, 3, 7, 4, 6, 5, 0] = 5 ∧
    mtp [U, U, 9, 1, 9, 2, 8, 3, 7, 4, 6, 5, 0] = 5 := by
  simp +decide [mtp, slots, tail11, List.mergeSort, U]

example : tail11 [99, 98, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] =
    [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] ∧
    roll [99, 98, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] 12 =
    [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12] := by decide

example (t : Nat) : observedGap 0 t = none := by simp [observedGap]
example : observedGap 10 9 = some 3 ∧ observedGap 10 10 = some 3 ∧
    observedGap 10 15 = some 5 := by decide

-- Ordinary addition agrees through U-60. Above that, only the ledger caps.
example : (U - 60) + 60 = U ∧ futureCap (U - 60) = U ∧
    U < (U - 59) + 60 ∧ futureCap (U - 59) = U ∧
    futureCap (U - 1) = U ∧ futureCap U = U := by decide

example : mtp (List.replicate 11 (U - 1)) = U - 1 ∧
    honestStamp (List.replicate 11 (U - 1)) (U - 1) = U ∧
    accepts (List.replicate 11 (U - 1)) U (U - 60) := by
  simp +decide [mtp, slots, tail11, List.mergeSort, U, honestStamp, nextMin,
    accepts, Botho.satAdd]

-- At maximum MTP, saturation cannot satisfy the strict lower bound.
example : mtp (List.replicate 11 U) = U ∧ nextMin (List.replicate 11 U) = U := by
  simp +decide [mtp, slots, tail11, List.mergeSort, U, nextMin, Botho.satAdd]
example (n : Nat) (hn : n ≤ U) :
    honestStamp (List.replicate 11 U) n = U ∧
    ¬ accepts (List.replicate 11 U) (honestStamp (List.replicate 11 U) n) U := by
  have hm : mtp (List.replicate 11 U) = U := by
    simp +decide [mtp, slots, tail11, List.mergeSort, U]
  simp only [honestStamp, nextMin, Botho.satAdd, hm, accepts]
  omega

end Botho.Timestamps
