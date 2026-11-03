/- Canonical row ordering for LotteryV2 (#1850).
The sort is a specification of LMDB iteration order, not added Rust code.
Its correspondence to LMDB's default memcmp comparator is trusted; the
existing lottery_candidates DRT leg tests the production reader over LMDB. -/
import Botho.Lottery.Drawing

namespace Botho.Lottery

private theorem bytesLt_iff_lt (a b : Bytes) : bytesLt a b = true ↔ a < b := by
  induction a generalizing b with
  | nil => cases b <;> simp [bytesLt]
  | cons x xs ih =>
    cases b with
    | nil => simp [bytesLt]
    | cons y ys =>
      simp only [bytesLt, List.cons_lt_cons_iff]
      by_cases hxy : x < y
      · simp [hxy]
      · by_cases hyx : y < x
        · have hne : x ≠ y := by intro he; subst he; exact UInt8.lt_irrefl _ hyx
          simp [hxy, hyx, hne]
        · have he : x = y := UInt8.le_antisymm (UInt8.not_lt.mp hyx) (UInt8.not_lt.mp hxy)
          simp [he, ih]

/-- The modeled byte comparator is irreflexive, including on 36-byte keys. -/
theorem bytesLt_irrefl (a : Bytes) : bytesLt a a = false := by
  apply Bool.eq_false_iff.mpr
  intro h
  exact List.lt_irrefl a ((bytesLt_iff_lt a a).mp h)

/-- Lexicographic byte comparison is transitive (no length restriction needed). -/
theorem bytesLt_trans (a b c : Bytes) (hab : bytesLt a b = true)
    (hbc : bytesLt b c = true) : bytesLt a c = true := by
  exact (bytesLt_iff_lt a c).mpr (List.lt_trans
    ((bytesLt_iff_lt a b).mp hab) ((bytesLt_iff_lt b c).mp hbc))

/-- Trichotomy holds on all byte strings, and hence equal-length 36-byte IDs. -/
theorem bytesLt_trichotomy (a b : Bytes) :
    bytesLt a b = true ∨ a = b ∨ bytesLt b a = true := by
  by_cases hab : bytesLt a b = true
  · exact Or.inl hab
  · by_cases hba : bytesLt b a = true
    · exact Or.inr (Or.inr hba)
    · exact Or.inr (Or.inl (List.le_antisymm
        (List.not_lt.mp (by simpa only [bytesLt_iff_lt] using hba))
        (List.not_lt.mp (by simpa only [bytesLt_iff_lt] using hab))))

/-- Trusted specification of LMDB's default key order (memcmp, shorter first).
Stable merge sort is weakly increasing; unique IDs make the order strict.
This function does not change the existing executable drawing or DRT dispatch. -/
def sortById (rows : List Utxo) : List Utxo :=
  rows.mergeSort (fun a b => !bytesLt b.id a.id)

private theorem keyLe_trans (a b c : Utxo)
    (hab : (!bytesLt b.id a.id) = true) (hbc : (!bytesLt c.id b.id) = true) :
    (!bytesLt c.id a.id) = true := by
  simp only [Bool.not_eq_true', Bool.eq_false_iff, ne_eq, bytesLt_iff_lt] at hab hbc ⊢
  exact List.not_lt.mpr (List.le_trans (List.not_lt.mp hab) (List.not_lt.mp hbc))

private theorem keyLe_total (a b : Utxo) :
    ((!bytesLt b.id a.id) || (!bytesLt a.id b.id)) = true := by
  simp only [Bool.or_eq_true, Bool.not_eq_true', Bool.eq_false_iff, ne_eq, bytesLt_iff_lt]
  exact (List.le_total a.id b.id).elim
    (fun h => Or.inl (List.not_lt.mpr h)) (fun h => Or.inr (List.not_lt.mpr h))

/-- Unique keys make the canonical result strictly ordered, as required by
lotteryCandidates. This proves the sort specification, not the LMDB tie. -/
theorem sortById_sorted (rows : List Utxo) (hn : (rows.map Utxo.id).Nodup) :
    (sortById rows).Pairwise (fun a b => bytesLt a.id b.id = true) := by
  have hp := List.mergeSort_perm rows (fun a b => !bytesLt b.id a.id)
  have hd : ((sortById rows).map Utxo.id).Nodup := (hp.map Utxo.id).symm.nodup hn
  have hne := List.pairwise_map.mp hd
  have hs := List.pairwise_mergeSort keyLe_trans keyLe_total rows
  apply (hs.and hne).imp
  intro a b h
  rcases bytesLt_trichotomy a.id b.id with hab | he | hba
  · exact hab
  · exact False.elim (h.2 he)
  · have hf := h.1
    simp only [hba, Bool.not_true, Bool.false_eq_true] at hf

/-- Strictly ordered input already has the canonical order assumed by
lotteryCandidates, so adding the sort leaves that input unchanged. -/
theorem sortById_of_sorted (rows : List Utxo)
    (h : rows.Pairwise (fun a b => bytesLt a.id b.id = true)) : sortById rows = rows := by
  apply List.mergeSort_of_pairwise
  apply h.imp
  intro a b hab
  simp only [Bool.not_eq_true', Bool.eq_false_iff]
  intro hba
  have := bytesLt_trans a.id b.id a.id hab hba
  simp [bytesLt_irrefl] at this

private theorem id_injective_on_rows (rows : List Utxo)
    (hn : (rows.map Utxo.id).Nodup) (a b : Utxo)
    (ha : a ∈ rows) (hb : b ∈ rows) (he : a.id = b.id) : a = b := by
  induction rows with
  | nil => simp at ha
  | cons x xs ih =>
    simp only [List.map_cons, List.nodup_cons] at hn
    simp only [List.mem_cons] at ha hb
    rcases ha with rfl | ha
    · rcases hb with rfl | hb
      · rfl
      · exact False.elim (hn.1 (he ▸ List.mem_map.mpr ⟨b, hb, rfl⟩))
    · rcases hb with rfl | hb
      · exact False.elim (hn.1 (he.symm ▸ List.mem_map.mpr ⟨a, ha, rfl⟩))
      · exact ih hn.2 ha hb

/-- A permutation of uniquely keyed rows has exactly the same canonical list.
Uniqueness matters: equal keys carrying different row data would be tied. -/
theorem sortById_perm (l₁ l₂ : List Utxo) (hp : l₁.Perm l₂)
    (hn : (l₁.map Utxo.id).Nodup) : sortById l₁ = sortById l₂ := by
  apply List.Perm.eq_of_pairwise (le := fun a b => (!bytesLt b.id a.id) = true)
  · intro a b ha hb hab hba
    have ha' : a ∈ l₁ := (List.mem_mergeSort).mp ha
    have hb' : b ∈ l₁ := hp.mem_iff.mpr ((List.mem_mergeSort).mp hb)
    apply id_injective_on_rows l₁ hn a b ha' hb'
    simp only [Bool.not_eq_true', Bool.eq_false_iff, ne_eq, bytesLt_iff_lt] at hab hba
    exact List.le_antisymm (List.not_lt.mp hab) (List.not_lt.mp hba)
  · exact List.pairwise_mergeSort keyLe_trans keyLe_total l₁
  · exact List.pairwise_mergeSort keyLe_trans keyLe_total l₂
  · exact (List.mergeSort_perm l₁ _).trans (hp.trans (List.mergeSort_perm l₂ _).symm)

/-- With unique LMDB keys, every enumeration of the same rows yields the
same drawing after canonical ordering, including error outcomes. -/
theorem drawing_perm_invariant (P : Prims) (r : Reads) (b : DrawBlock)
    (l₁ l₂ : List Utxo) (hp : l₁.Perm l₂) (hn : (l₁.map Utxo.id).Nodup) :
    drawing P { r with rows := sortById l₁ } b =
      drawing P { r with rows := sortById l₂ } b := by
  rw [sortById_perm l₁ l₂ hp hn]

/-- Constant-zero primitives expose deterministic zero rolls. This is an
explicit witness, not an assumption that production SHA-256 is constant. -/
def orderWitnessPrims : Prims :=
  { sha256 := fun _ => zeros 32, powHash := fun _ => zeros 32 }

/-- Distinct, valid-width, eligible witness rows at height 720. -/
def orderWitnessA : Utxo :=
  { id := zeros 36, amount := 1000000, createdAt := 0, clusterFactor := FACTOR_SCALE }

def orderWitnessB : Utxo :=
  { id := 1 :: zeros 35, amount := 1000000, createdAt := 0, clusterFactor := FACTOR_SCALE }

/-- Zero rolls select in input order. With pool 3, equal splitting pays 1
each and addToLast pays the remainder to the second winner: reversing the
rows changes which ID gets 2. Canonical ordering is therefore load-bearing. -/
theorem draw_order_dependent :
    (drawEligible orderWitnessPrims [orderWitnessA.toCandidate, orderWitnessB.toCandidate]
      3 720 (zeros 32) DrawConfig.default).map
        (fun r => r.winners.map (fun w => (w.utxoId, w.payout))) =
      some [(orderWitnessA.id, 1), (orderWitnessB.id, 2)] ∧
    (drawEligible orderWitnessPrims [orderWitnessB.toCandidate, orderWitnessA.toCandidate]
      3 720 (zeros 32) DrawConfig.default).map
        (fun r => r.winners.map (fun w => (w.utxoId, w.payout))) =
      some [(orderWitnessB.id, 1), (orderWitnessA.id, 2)] ∧
    orderWitnessA.id ≠ orderWitnessB.id ∧
    orderWitnessA.toCandidate.isEligible 720 DrawConfig.default = true ∧
    orderWitnessB.toCandidate.isEligible 720 DrawConfig.default = true := by
  decide

/-- At the actual 10,000-candidate cap, the final available slot goes to
the next eligible row. Reversing the two remaining rows changes that ID. -/
theorem scan_cap_order_dependent :
    (scan MAX_LOTTERY_CANDIDATES 720 DrawConfig.default [orderWitnessA, orderWitnessB]
      (MAX_LOTTERY_CANDIDATES - 1)).map Candidate.id = [orderWitnessA.id] ∧
    (scan MAX_LOTTERY_CANDIDATES 720 DrawConfig.default [orderWitnessB, orderWitnessA]
      (MAX_LOTTERY_CANDIDATES - 1)).map Candidate.id = [orderWitnessB.id] ∧
    orderWitnessA.id ≠ orderWitnessB.id := by
  decide

end Botho.Lottery
