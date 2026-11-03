/-
Finite ideal-roll expectation for LotteryV2 (#1851, part C2).

The roll loop is factored without changing `Draw.lean`. Uniform selection
is counted over finite paths of successive in-range rolls. Status: proven
(model). Hash uniformity and independence are separate, explicit trusted
premises; they are not axioms of this library.
-/
import Botho.Lottery.Sybil

namespace Botho.Lottery

/-- The existing winner loop with its roll source supplied explicitly. -/
def pickWinnersF (roll : Nat → Nat → Nat) (payoutPerWinner : Nat) :
    Nat → Nat → List Slot → List Winner
  | _, 0, _ => []
  | i, left + 1, slots =>
    let remaining := remainingTotal slots
    if remaining = 0 then []
    else
      match selectAt (roll i remaining) 0 slots with
      | (some c, slots') =>
        { utxoId := c.id, payout := payoutPerWinner } ::
          pickWinnersF roll payoutPerWinner (i + 1) left slots'
      | (none, slots') => pickWinnersF roll payoutPerWinner (i + 1) left slots'

/-- The parameterized loop is exactly the existing hash/modulo loop. -/
theorem pickWinners_eq_pickWinnersF (P : Prims) (seed : Bytes) (q i n : Nat)
    (slots : List Slot) :
    pickWinners P seed q i n slots =
      pickWinnersF (fun j m => verifiableRandomU128 P seed j % m) q i n slots := by
  induction n generalizing i slots with
  | zero => rfl
  | succ n ih =>
    simp only [pickWinners, pickWinnersF]
    split
    · rfl
    · cases h : selectAt (verifiableRandomU128 P seed i % remainingTotal slots) 0 slots with
      | mk c ss => cases c <;> simp [ih]

namespace Ideal

/-- One branch per candidate position, retaining the order of the others.
Repeated IDs still represent distinct tickets, just as in the roll loop. -/
def choices {α : Type} : List α → List (α × List α)
  | [] => []
  | a :: as => (a, as) :: (choices as).map (fun (b, bs) => (b, a :: bs))

/-- Removing the zero-based index selected by a uniform roll. -/
def pickAt {α : Type} : Nat → List α → Option α × List α
  | _, [] => (none, [])
  | 0, a :: as => (some a, as)
  | r + 1, a :: as =>
    let p := pickAt r as
    (p.1, a :: p.2)

/-- Finite Nat summation, used for both event counts and payout masses. -/
def sum {α : Type} (f : α → Nat) : List α → Nat
  | [] => 0
  | a :: as => f a + sum f as

/-- Number of ordered, without-replacement roll paths of length `t`.
Its factors are exactly `n, n-1, ..., n-t+1`; no probabilities are rounded. -/
def mass : Nat → Nat → Nat
  | _, 0 => 1
  | 0, _ + 1 => 0
  | n + 1, t + 1 => (n + 1) * mass n t

/-- Total payout over every removal path, with the payout at each draw
specified by `payments`. The final payment can carry the pool remainder. -/
def payoutMass {α : Type} (owns : α → Bool) : List Nat → List α → Nat
  | [], _ => 0
  | p :: ps, cs =>
    sum (fun (c, rest) =>
      (if owns c then p else 0) * mass rest.length ps.length +
        payoutMass owns ps rest) (choices cs)

private theorem sum_map {α β : Type} (f : β → Nat) (g : α → β) (xs : List α) :
    sum f (xs.map g) = sum (fun x => f (g x)) xs := by
  induction xs with
  | nil => rfl
  | cons x xs ih => simp [sum, ih]

private theorem sum_add {α : Type} (f g : α → Nat) (xs : List α) :
    sum (fun x => f x + g x) xs = sum f xs + sum g xs := by
  induction xs with
  | nil => rfl
  | cons x xs ih => simp only [sum, ih]; omega

private theorem sum_mul {α : Type} (f : α → Nat) (k : Nat) (xs : List α) :
    sum (fun x => f x * k) xs = sum f xs * k := by
  induction xs with
  | nil => simp [sum]
  | cons x xs ih => simp [sum, ih, Nat.add_mul]

private theorem sum_const {α : Type} (k : Nat) (xs : List α) :
    sum (fun _ => k) xs = xs.length * k := by
  induction xs with
  | nil => simp [sum]
  | cons x xs ih => simp [sum, ih, Nat.add_mul, Nat.add_comm]

private theorem sum_congr {α : Type} {f g : α → Nat} (xs : List α)
    (h : ∀ x ∈ xs, f x = g x) : sum f xs = sum g xs := by
  induction xs with
  | nil => rfl
  | cons x xs ih =>
    simp only [sum]
    rw [h x (by simp), ih (fun y hy => h y (by simp [hy]))]

private theorem choices_length {α : Type} (cs : List α) :
    (choices cs).length = cs.length := by
  induction cs with
  | nil => rfl
  | cons c cs ih => simp [choices, ih]

private theorem choice_length {α : Type} (cs : List α) (c : α) (rest : List α)
    (h : (c, rest) ∈ choices cs) : rest.length + 1 = cs.length := by
  induction cs generalizing c rest with
  | nil => simp [choices] at h
  | cons a cs ih =>
    simp only [choices, List.mem_cons, List.mem_map] at h
    rcases h with h | ⟨⟨b, bs⟩, hb, he⟩
    · cases h; simp
    · cases he
      have := ih b bs hb
      simp; omega

private theorem selected_count {α : Type} (owns : α → Bool) (cs : List α) :
    sum (fun p => if owns p.1 then 1 else 0) (choices cs) = cs.countP owns := by
  induction cs with
  | nil => rfl
  | cons c cs ih =>
    simp only [choices, sum, sum_map]
    rw [ih]
    cases hown : owns c <;> simp [hown, Nat.add_comm]

private theorem choice_count {α : Type} (owns : α → Bool) (cs : List α)
    (c : α) (rest : List α) (h : (c, rest) ∈ choices cs) :
    (if owns c then 1 else 0) + rest.countP owns = cs.countP owns := by
  induction cs generalizing c rest with
  | nil => simp [choices] at h
  | cons a cs ih =>
    simp only [choices, List.mem_cons, List.mem_map] at h
    rcases h with h | ⟨⟨b, bs⟩, hb, he⟩
    · cases h; cases hown : owns a <;> simp [hown, Nat.add_comm]
    · cases he
      have := ih b bs hb
      cases ha : owns a <;> cases hb : owns b <;> simp_all <;> omega

private theorem remaining_count {α : Type} (owns : α → Bool) (cs : List α) :
    sum (fun p => p.2.countP owns) (choices cs) =
      (cs.length - 1) * cs.countP owns := by
  have h := sum_congr (f := fun p => (if owns p.1 then 1 else 0) + p.2.countP owns)
    (g := fun _ => cs.countP owns) (choices cs)
    (fun (c, rest) hc => choice_count owns cs c rest hc)
  rw [sum_add, selected_count, sum_const, choices_length] at h
  cases cs with
  | nil => rfl
  | cons c cs =>
    simp only [List.length_cons, Nat.add_sub_cancel] at h ⊢
    rw [Nat.add_mul, Nat.one_mul] at h
    omega

private theorem mass_succ (n t : Nat) (hn : 0 < n) :
    mass n (t + 1) = n * mass (n - 1) t := by
  cases n <;> simp_all [mass]

private theorem payoutMass_cons {α : Type} (owns : α → Bool)
    (p : Nat) (ps : List Nat) (cs : List α) :
    payoutMass owns (p :: ps) cs =
      cs.countP owns * p * mass (cs.length - 1) ps.length +
        sum (fun x => payoutMass owns ps x.2) (choices cs) := by
  rw [payoutMass]
  rw [sum_add]
  congr 1
  calc
    _ = sum (fun x => ((if owns x.1 then 1 else 0) * p) *
        mass (cs.length - 1) ps.length) (choices cs) := by
      apply sum_congr
      intro (c, rest) hc
      have hl := choice_length cs c rest hc
      have : rest.length = cs.length - 1 := by omega
      rw [this]
      cases owns c <;> simp
    _ = _ := by rw [sum_mul, sum_mul, selected_count]

/-- Each ticket occurs equally often in every payout position. The final
position may have a different payout; only the sum of payments matters. -/
theorem payoutMass_eq {α : Type} (owns : α → Bool) (p : Nat)
    (ps : List Nat) (cs : List α) (ht : (p :: ps).length ≤ cs.length) :
    payoutMass owns (p :: ps) cs =
      cs.countP owns * sum id (p :: ps) * mass (cs.length - 1) ps.length := by
  induction ps generalizing p cs with
  | nil =>
    rw [payoutMass_cons]
    simp [payoutMass, sum, mass, sum_const]
  | cons q qs ih =>
    rw [payoutMass_cons]
    have hs : sum (fun x => payoutMass owns (q :: qs) x.2) (choices cs) =
        (cs.length - 1) * cs.countP owns * sum id (q :: qs) *
          mass (cs.length - 2) qs.length := by
      calc
        _ = sum (fun x => x.2.countP owns * sum id (q :: qs) *
            mass (cs.length - 2) qs.length) (choices cs) := by
          apply sum_congr
          intro (c, rest) hc
          have hl := choice_length cs c rest hc
          have he : rest.length - 1 = cs.length - 2 := by omega
          rw [ih q rest (by simp only [List.length_cons] at ht ⊢; omega), he]
        _ = _ := by rw [sum_mul, sum_mul, remaining_count]
    rw [hs]
    have hn : 0 < cs.length - 1 := by simp only [List.length_cons] at ht; omega
    simp only [List.length_cons]
    rw [mass_succ _ _ hn]
    have he : cs.length - 1 - 1 = cs.length - 2 := by omega
    rw [he]
    simp only [sum, id_eq, Nat.mul_add, Nat.add_mul]
    ac_rfl

/-- Cross-multiplied exact expected share for any nonempty payout schedule
of at most the number of available tickets. -/
theorem expected_share {α : Type} (owns : α → Bool) (p : Nat)
    (ps : List Nat) (cs : List α) (ht : (p :: ps).length ≤ cs.length) :
    payoutMass owns (p :: ps) cs * cs.length =
      cs.countP owns * sum id (p :: ps) * mass cs.length (p :: ps).length := by
  rw [payoutMass_eq owns p ps cs ht]
  have hn : 0 < cs.length := by simp only [List.length_cons] at ht; omega
  rw [List.length_cons, mass_succ _ _ hn]
  ac_rfl

private theorem sum_flatMap {α β : Type} (f : β → Nat) (g : α → List β)
    (xs : List α) : sum f (xs.flatMap g) = sum (fun x => sum f (g x)) xs := by
  have ha : ∀ ys zs : List β, sum f (ys ++ zs) = sum f ys + sum f zs := by
    intro ys zs
    induction ys with
    | nil => simp [sum]
    | cons y ys ih => simp [sum, ih, Nat.add_assoc]
  induction xs with
  | nil => rfl
  | cons x xs ih => simp [sum, ha, ih]

/-- Explicit finite roll paths. At step `j`, the range is `n-j`.
The list contains each path once; no division approximates a probability. -/
def paths : Nat → Nat → List (List Nat)
  | _, 0 => [[]]
  | n, t + 1 => (List.range n).flatMap fun r => (paths (n - 1) t).map (r :: ·)

/-- The total counting mass is the product of the successive roll ranges. -/
theorem paths_length (n t : Nat) : (paths n t).length = mass n t := by
  induction t generalizing n with
  | zero => simp [paths, mass]
  | succ t ih =>
    cases n with
    | zero => simp [paths, mass]
    | succ n =>
      simp only [paths, List.length_flatMap, List.length_map, Nat.add_sub_cancel, ih]
      have h : ∀ xs : List Nat,
          (xs.map (fun _ => mass n t)).sum = xs.length * mass n t := by
        intro xs
        induction xs with
        | nil => simp
        | cons x xs ih => simp [ih, Nat.add_mul, Nat.add_comm]
      rw [h, List.length_range]
      rfl

private theorem picks_eq_choices {α : Type} (cs : List α) :
    (List.range cs.length).map (fun r => pickAt r cs) =
      (choices cs).map (fun (c, rest) => (some c, rest)) := by
  induction cs with
  | nil => rfl
  | cons c cs ih =>
    rw [List.length_cons, List.range_succ_eq_map]
    simp only [List.map_cons, List.map_map, choices, pickAt, Function.comp_def]
    congr 1
    have h := congrArg (List.map (fun (p : Option α × List α) => (p.1, c :: p.2))) ih
    simpa only [List.map_map, Function.comp_def] using h

private theorem sum_pickAt {α : Type} (f : Option α × List α → Nat) (cs : List α) :
    sum (fun r => f (pickAt r cs)) (List.range cs.length) =
      sum (fun (c, rest) => f (some c, rest)) (choices cs) := by
  rw [← sum_map, picks_eq_choices, sum_map]

/-- Execute an explicit roll path with a payout schedule. This is linked
below to uniform `selectAt` and the existing winner loop. -/
def drawPath : List Nat → List Nat → List Candidate → List Winner
  | [], _, _ => []
  | _ :: _, [], _ => []
  | p :: ps, r :: rs, cs =>
    match pickAt r cs with
    | (none, _) => []
    | (some c, rest) => { utxoId := c.id, payout := p } :: drawPath ps rs rest

/-- Counting the payouts of the executed finite paths gives `payoutMass`.
Thus the latter is a sum over actual paths, not a share postulate. -/
theorem payoutMass_paths (owns : Bytes → Bool) (ps : List Nat) (cs : List Candidate) :
    sum (fun rs => ownerWinnings owns (drawPath ps rs cs)) (paths cs.length ps.length) =
      payoutMass (fun c => owns c.id) ps cs := by
  induction ps generalizing cs with
  | nil => simp [drawPath, paths, ownerWinnings, payoutMass, sum]
  | cons p ps ih =>
    simp only [List.length_cons, paths, sum_flatMap, sum_map]
    have step : (fun r => sum (fun rs => ownerWinnings owns
        (drawPath (p :: ps) (r :: rs) cs)) (paths (cs.length - 1) ps.length)) =
        (fun r => (fun x : Option Candidate × List Candidate => match x with
          | (none, _) => 0
          | (some c, rest) => sum (fun rs =>
              (if owns c.id then p else 0) + ownerWinnings owns (drawPath ps rs rest))
              (paths (cs.length - 1) ps.length)) (pickAt r cs)) := by
      funext r
      cases h : pickAt r cs with
      | mk c rest => cases c <;> simp [drawPath, h, ownerWinnings, sum_const]
    rw [step]
    rw [sum_pickAt (fun x : Option Candidate × List Candidate => match x with
      | (none, _) => 0
      | (some c, rest) => sum (fun rs =>
          (if owns c.id then p else 0) + ownerWinnings owns (drawPath ps rs rest))
          (paths (cs.length - 1) ps.length)) cs]
    rw [payoutMass]
    apply sum_congr
    intro (c, rest) hc
    have hl := choice_length cs c rest hc
    have he : cs.length - 1 = rest.length := by omega
    simp only [he, sum_add, sum_const, paths_length, ih]
    rw [Nat.mul_comm]

private theorem uniform_remaining (slots : List Slot)
    (hw : ∀ s ∈ slots, s.weight = 1) :
    remainingTotal slots = (unusedCands slots).length := by
  induction slots with
  | nil => rfl
  | cons s ss ih =>
    have hs := hw s (by simp)
    have ht := ih (fun t ht => hw t (by simp [ht]))
    cases hu : s.used <;> simp [remainingTotal, unusedCands, hu, hs, ht] <;> omega

private theorem select_weights (roll cum : Nat) (slots : List Slot)
    (hw : ∀ s ∈ slots, s.weight = 1) :
    ∀ s ∈ (selectAt roll cum slots).2, s.weight = 1 := by
  induction slots generalizing cum with
  | nil => simp [selectAt]
  | cons s ss ih =>
    have hs := hw s (by simp)
    have ht : ∀ t ∈ ss, t.weight = 1 := fun t ht => hw t (by simp [ht])
    simp only [selectAt]
    split
    · intro t hm
      simp only [List.mem_cons] at hm
      rcases hm with rfl | hm
      · exact hs
      · exact ih _ ht t hm
    · split
      · intro t hm
        simp only [List.mem_cons] at hm
        rcases hm with rfl | hm
        · exact hs
        · exact ht t hm
      · intro t hm
        simp only [List.mem_cons] at hm
        rcases hm with rfl | hm
        · exact hs
        · exact ih _ ht t hm

/-- Operational uniform-selection bridge: a weight-one cumulative walk
removes exactly the indexed unused ticket, preserving the others' order. -/
theorem selectAt_uniform_index (slots : List Slot) (r cum : Nat)
    (hw : ∀ s ∈ slots, s.weight = 1) :
    ((selectAt (cum + r) cum slots).1,
      unusedCands (selectAt (cum + r) cum slots).2) = pickAt r (unusedCands slots) := by
  induction slots generalizing cum r with
  | nil => simp [selectAt, unusedCands, pickAt]
  | cons s ss ih =>
    have hs := hw s (by simp)
    have ht : ∀ t ∈ ss, t.weight = 1 := fun t ht => hw t (by simp [ht])
    cases hu : s.used
    · cases r with
      | zero => simp [selectAt, hu, hs, unusedCands, pickAt]
      | succ r =>
        have hn : ¬ cum + 1 + r < cum + 1 := by omega
        have he : cum + (r + 1) = (cum + 1) + r := by omega
        have h := ih r (cum + 1) ht
        simp only [selectAt, hu, hs, Bool.false_eq_true, ↓reduceIte, unusedCands,
          pickAt, he, hn]
        exact congrArg (fun p : Option Candidate × List Candidate => (p.1, s.cand :: p.2)) h
    · simpa [selectAt, hu, unusedCands] using ih r cum ht

private theorem pickAt_valid {α : Type} (cs : List α) (r : Nat) (hr : r < cs.length) :
    ∃ c rest, pickAt r cs = (some c, rest) ∧ (c, rest) ∈ choices cs := by
  have hm : pickAt r cs ∈ (List.range cs.length).map (fun j => pickAt j cs) :=
    List.mem_map.mpr ⟨r, List.mem_range.mpr hr, rfl⟩
  rw [picks_eq_choices] at hm
  obtain ⟨⟨c, rest⟩, hc, he⟩ := List.mem_map.mp hm
  exact ⟨c, rest, he.symm, hc⟩

private theorem uniform_step (slots : List Slot) (r : Nat)
    (hw : ∀ s ∈ slots, s.weight = 1) (hr : r < (unusedCands slots).length) :
    ∃ c ss, selectAt r 0 slots = (some c, ss) ∧
      (∀ s ∈ ss, s.weight = 1) ∧ (c, unusedCands ss) ∈ choices (unusedCands slots) := by
  obtain ⟨c, rest, hp, hc⟩ := pickAt_valid (unusedCands slots) r hr
  have h := selectAt_uniform_index slots r 0 hw
  simp only [Nat.zero_add, hp] at h
  cases he : selectAt r 0 slots with
  | mk cand ss =>
    simp only [he, Prod.mk.injEq] at h
    obtain ⟨h1, h2⟩ := h
    subst cand
    refine ⟨c, ss, rfl, ?_, ?_⟩
    · simpa [he] using select_weights r 0 slots hw
    · simpa [h2] using hc

private theorem paths_cons (n t r : Nat) (rs : List Nat) :
    r :: rs ∈ paths n (t + 1) ↔ r < n ∧ rs ∈ paths (n - 1) t := by
  simp [paths, List.mem_flatMap, List.mem_map, List.mem_range]

private theorem paths_nil (n t : Nat) : [] ∈ paths n t ↔ t = 0 := by
  cases t <;> simp [paths, List.mem_flatMap, List.mem_map]

private theorem path_length (n t : Nat) (rs : List Nat) (h : rs ∈ paths n t) :
    rs.length = t := by
  induction t generalizing n rs with
  | zero => simpa [paths] using h
  | succ t ih =>
    cases rs with
    | nil => simp [paths_nil] at h
    | cons r rs =>
      obtain ⟨_, ht⟩ := (paths_cons n t r rs).mp h
      simp [ih _ _ ht]

private theorem drawPath_loop (roll : Nat → Nat → Nat) (q : Nat)
    (t i : Nat) (slots : List Slot) (rs : List Nat)
    (hw : ∀ s ∈ slots, s.weight = 1) (ht : t ≤ (unusedCands slots).length)
    (hp : rs ∈ paths (unusedCands slots).length t)
    (hr : ∀ j, j < t → ∀ m, roll (i + j) m = (rs[j]?).getD 0) :
    drawPath (List.replicate t q) rs (unusedCands slots) =
      pickWinnersF roll q i t slots := by
  induction t generalizing i slots rs with
  | zero => simp [drawPath, pickWinnersF]
  | succ t ih =>
    cases rs with
    | nil => simp [paths_nil] at hp
    | cons r rs =>
      obtain ⟨hrange, htail⟩ := (paths_cons _ t r rs).mp hp
      obtain ⟨c, ss, he, hw', hc⟩ := uniform_step slots r hw hrange
      have hl := choice_length (unusedCands slots) c (unusedCands ss) hc
      have hlen : (unusedCands slots).length - 1 = (unusedCands ss).length := by omega
      have hrem : remainingTotal slots ≠ 0 := by rw [uniform_remaining slots hw]; omega
      have hroll : roll i (remainingTotal slots) = r := by
        simpa using hr 0 (by omega) (remainingTotal slots)
      have hpick := selectAt_uniform_index slots r 0 hw
      simp only [Nat.zero_add, he] at hpick
      have hnext : ∀ j, j < t → ∀ m, roll (i + 1 + j) m = (rs[j]?).getD 0 := by
        intro j hj m
        simpa [Nat.add_assoc, Nat.add_comm, Nat.add_left_comm] using hr (j + 1) (by omega) m
      have hrec := ih (i + 1) ss rs hw' (by omega) (by simpa [hlen] using htail) hnext
      simp only [List.replicate_succ, drawPath, ← hpick, pickWinnersF, hrem,
        ↓reduceIte, hroll, he]
      rw [hrec]

/-- An in-range path, interpreted as a roll function, executes exactly the
uniform existing loop. Combined with the refactor theorem above this
anchors the finite measure to the model's selection implementation. -/
theorem uniform_path_loop (q t : Nat) (slots : List Slot) (rs : List Nat)
    (hw : ∀ s ∈ slots, s.weight = 1) (ht : t ≤ (unusedCands slots).length)
    (hp : rs ∈ paths (unusedCands slots).length t) :
    drawPath (List.replicate t q) rs (unusedCands slots) =
      pickWinnersF (fun j _ => (rs[j]?).getD 0) q 0 t slots := by
  exact drawPath_loop _ q t 0 slots rs hw ht hp (by intros; simp)

private theorem drawPath_length (ps rs : List Nat) (cs : List Candidate)
    (ht : ps.length ≤ cs.length) (hp : rs ∈ paths cs.length ps.length) :
    (drawPath ps rs cs).length = ps.length := by
  induction ps generalizing cs rs with
  | nil => rfl
  | cons p ps ih =>
    cases rs with
    | nil => simp [paths_nil] at hp
    | cons r rs =>
      obtain ⟨hr, htail⟩ := (paths_cons _ _ r rs).mp hp
      obtain ⟨c, rest, he, hc⟩ := pickAt_valid cs r hr
      have hl := choice_length cs c rest hc
      have he' : cs.length - 1 = rest.length := by omega
      simp only [drawPath, he, List.length_cons]
      rw [ih rs rest (by simp only [List.length_cons] at ht; omega)
        (by simpa [he'] using htail)]

private theorem drawPath_total (ps rs : List Nat) (cs : List Candidate)
    (ht : ps.length ≤ cs.length) (hp : rs ∈ paths cs.length ps.length) :
    totalPaid (drawPath ps rs cs) = sum id ps := by
  induction ps generalizing cs rs with
  | nil => rfl
  | cons p ps ih =>
    cases rs with
    | nil => simp [paths_nil] at hp
    | cons r rs =>
      obtain ⟨hr, htail⟩ := (paths_cons _ _ r rs).mp hp
      obtain ⟨c, rest, he, hc⟩ := pickAt_valid cs r hr
      have hl := choice_length cs c rest hc
      have he' : cs.length - 1 = rest.length := by omega
      simp only [drawPath, he, totalPaid, sum, id_eq]
      rw [ih rs rest (by simp only [List.length_cons] at ht; omega)
        (by simpa [he'] using htail)]

/-- Equal payments, with the entire division remainder in the last slot. -/
def payments (q extra : Nat) : Nat → List Nat
  | 0 => []
  | 1 => [q + extra]
  | n + 2 => q :: payments q extra (n + 1)

private theorem payments_length (q e n : Nat) : (payments q e n).length = n := by
  induction n with
  | zero => rfl
  | succ n ih =>
    cases n with
    | zero => rfl
    | succ n => simp [payments, ih]

private theorem payments_sum (q e n : Nat) (hn : 0 < n) :
    sum id (payments q e n) = n * q + e := by
  induction n with
  | zero => omega
  | succ n ih =>
    cases n with
    | zero => simp [payments, sum]
    | succ n =>
      rw [payments, sum, ih (by omega)]
      simp only [id_eq, Nat.add_mul, Nat.one_mul]
      omega

private theorem drawPath_remainder (q e t : Nat) (rs : List Nat) (cs : List Candidate)
    (ht : t ≤ cs.length) (hp : rs ∈ paths cs.length t) :
    addToLast e (drawPath (List.replicate t q) rs cs) =
      drawPath (payments q e t) rs cs := by
  induction t generalizing cs rs with
  | zero => simp [drawPath, payments, addToLast]
  | succ t ih =>
    cases rs with
    | nil => simp [paths_nil] at hp
    | cons r rs =>
      obtain ⟨hr, htail⟩ := (paths_cons _ _ r rs).mp hp
      obtain ⟨c, rest, he, hc⟩ := pickAt_valid cs r hr
      have hl := choice_length cs c rest hc
      have he' : cs.length - 1 = rest.length := by omega
      have htail' : rs ∈ paths rest.length t := by simpa [he'] using htail
      cases t with
      | zero => simp [List.replicate_succ, drawPath, he, payments, addToLast]
      | succ t =>
        have hrec := ih rs rest (by omega) htail'
        have hlen := drawPath_length (List.replicate (t + 1) q) rs rest
          (by simp; omega) (by simpa using htail')
        cases hd : drawPath (List.replicate (t + 1) q) rs rest with
        | nil => simp [hd] at hlen
        | cons w ws =>
          simp only [List.replicate_succ, drawPath, he, payments]
          rw [← hrec]
          change addToLast e ({ utxoId := c.id, payout := q } ::
              drawPath (List.replicate (t + 1) q) rs rest) =
            { utxoId := c.id, payout := q } :: addToLast e
              (drawPath (List.replicate (t + 1) q) rs rest)
          rw [hd]
          rfl

/-- Weight-one slots used by `SelectionMode.uniform`. -/
def uniformSlots (cs : List Candidate) : List Slot :=
  cs.map fun c => { cand := c, weight := 1, used := false }

private theorem uniformSlots_unused (cs : List Candidate) :
    unusedCands (uniformSlots cs) = cs := by
  induction cs with
  | nil => rfl
  | cons c cs ih =>
    change c :: unusedCands (uniformSlots cs) = c :: cs
    rw [ih]

private theorem uniformSlots_weights (cs : List Candidate) :
    ∀ s ∈ uniformSlots cs, s.weight = 1 := by
  intro s hs
  obtain ⟨c, _, rfl⟩ := List.mem_map.mp hs
  rfl

/-- The model's uniform winner loop and remainder step, with roll source
parameterized. `t` is `min(winnersPerDraw, eligible.length)` at the caller. -/
def draw (roll : Nat → Nat → Nat) (pool t : Nat) (cs : List Candidate) : List Winner :=
  let ws := pickWinnersF roll (pool / t) 0 t (uniformSlots cs)
  addToLast (pool - totalPaid ws) ws

/-- Sum of the actual ideal draw's owner payouts over all finite roll paths. -/
def winningsMass (owns : Bytes → Bool) (pool t : Nat) (cs : List Candidate) : Nat :=
  sum (fun rs => ownerWinnings owns (draw (fun j _ => (rs[j]?).getD 0) pool t cs))
    (paths cs.length t)

private theorem sum_replicate (q t : Nat) : sum id (List.replicate t q) = t * q := by
  induction t with
  | zero => simp [sum]
  | succ t ih => simp [List.replicate_succ, sum, ih, Nat.add_mul, Nat.add_comm]

/-- Remainder-aware operational bridge for each valid path. -/
theorem draw_path (pool t : Nat) (cs : List Candidate) (rs : List Nat)
    (ht : t ≤ cs.length) (hp : rs ∈ paths cs.length t) :
    draw (fun j _ => (rs[j]?).getD 0) pool t cs =
      drawPath (payments (pool / t) (pool - t * (pool / t)) t) rs cs := by
  have h := uniform_path_loop (pool / t) t (uniformSlots cs) rs
    (uniformSlots_weights cs) (by simpa [uniformSlots_unused] using ht)
    (by simpa [uniformSlots_unused] using hp)
  rw [uniformSlots_unused] at h
  unfold draw
  dsimp only
  rw [← h, drawPath_total _ _ _ (by simpa using ht) (by simpa using hp), sum_replicate]
  exact drawPath_remainder _ _ t rs cs ht hp

private theorem winningsMass_eq (owns : Bytes → Bool) (pool t : Nat)
    (cs : List Candidate) (ht : t ≤ cs.length) :
    winningsMass owns pool t cs = payoutMass (fun c => owns c.id)
      (payments (pool / t) (pool - t * (pool / t)) t) cs := by
  unfold winningsMass
  calc
    _ = sum (fun rs => ownerWinnings owns
        (drawPath (payments (pool / t) (pool - t * (pool / t)) t) rs cs))
        (paths cs.length t) := by
      apply sum_congr
      intro rs hrs
      rw [draw_path pool t cs rs ht hrs]
    _ = _ := by
      have h := payoutMass_paths owns (payments (pool / t) (pool - t * (pool / t)) t) cs
      simpa [payments_length] using h

/-- **ST-1. Exact uniform expected share**, including the remainder paid
only to the last winner. `winningsMass` executes `pickWinnersF`, and `mass`
is the proved finite path count. Tickets are occurrences, not distinct IDs.
Zero pool and underfilled draws are allowed; zero winners are excluded. -/
theorem exact_share (owns : Bytes → Bool) (pool t : Nat) (cs : List Candidate)
    (ht0 : 0 < t) (ht : t ≤ cs.length) :
    winningsMass owns pool t cs * cs.length =
      cs.countP (fun c => owns c.id) * pool * mass cs.length t := by
  rw [winningsMass_eq owns pool t cs ht]
  have hlen := payments_length (pool / t) (pool - t * (pool / t)) t
  have hsum := payments_sum (pool / t) (pool - t * (pool / t)) t ht0
  have hbound : t * (pool / t) ≤ pool := by
    have := Nat.mod_add_div pool t
    omega
  rw [Nat.add_sub_of_le hbound] at hsum
  cases he : payments (pool / t) (pool - t * (pool / t)) t with
  | nil => simp [he] at hlen; omega
  | cons p ps =>
    have h := expected_share (fun c => owns c.id) p ps cs (by rw [← he, hlen]; exact ht)
    simpa [← he, hlen, hsum] using h

/-- **ST-1 cap corollary.** The ideal expectation is at most the owned
ticket count times the lottery base fee. -/
theorem base_fee_bound (owns : Bytes → Bool) (pool t : Nat) (cs : List Candidate)
    (ht0 : 0 < t) (ht : t ≤ cs.length) (hpool : pool ≤ sybilRewardCap cs.length) :
    winningsMass owns pool t cs ≤
      cs.countP (fun c => owns c.id) * LOTTERY_BASE_FEE_PICO * mass cs.length t := by
  have hcap : sybilRewardCap cs.length ≤ cs.length * LOTTERY_BASE_FEE_PICO := by
    unfold sybilRewardCap satMul
    split <;> omega
  have he := exact_share owns pool t cs ht0 ht
  have hn : 0 < cs.length := by omega
  apply Nat.le_of_mul_le_mul_right (c := cs.length) (hc := hn)
  rw [he]
  calc
    _ ≤ cs.countP (fun c => owns c.id) * (cs.length * LOTTERY_BASE_FEE_PICO) *
        mass cs.length t := Nat.mul_le_mul_right _ (Nat.mul_le_mul_left _ (Nat.le_trans hpool hcap))
    _ = _ := by ac_rfl

end Ideal

/-- `drawEligible` with its winner-loop roll source supplied as a parameter. -/
def drawEligibleF (roll : Nat → Nat → Nat) (eligible : List Candidate)
    (poolAmount blockHeight : Nat) (seed : Bytes) (cfg : DrawConfig) : Option DrawResult :=
  let mv := (maxValue? eligible).getD 1
  let slots := eligible.map fun c => ({ cand := c, weight := c.weight cfg.mode mv, used := false } : Slot)
  if remainingTotal slots = 0 then none
  else
    let n := minN cfg.winnersPerDraw eligible.length
    let ws := pickWinnersF roll (poolAmount / n) 0 n slots
    some { blockHeight, poolAmount, winners := addToLast (poolAmount - totalPaid ws) ws, seed }

/-- The full eligible draw, not just its loop, refactors exactly. -/
theorem drawEligible_eq_drawEligibleF (P : Prims) (eligible : List Candidate)
    (poolAmount blockHeight : Nat) (seed : Bytes) (cfg : DrawConfig) :
    drawEligible P eligible poolAmount blockHeight seed cfg =
      drawEligibleF (fun i m => verifiableRandomU128 P seed i % m)
        eligible poolAmount blockHeight seed cfg := by
  simp only [drawEligible, drawEligibleF, pickWinners_eq_pickWinnersF]

/-- The ideal distribution specializes the actual uniform eligible draw. -/
theorem drawEligibleF_uniform (roll : Nat → Nat → Nat) (eligible : List Candidate)
    (poolAmount blockHeight : Nat) (seed : Bytes) (cfg : DrawConfig)
    (hu : cfg.mode = .uniform) (hn : 0 < eligible.length) :
    drawEligibleF roll eligible poolAmount blockHeight seed cfg =
      some { blockHeight, poolAmount,
             winners := Ideal.draw roll poolAmount (minN cfg.winnersPerDraw eligible.length) eligible, seed } := by
  have h : remainingTotal (Ideal.uniformSlots eligible) = eligible.length := by
    rw [Ideal.uniform_remaining _ (Ideal.uniformSlots_weights eligible), Ideal.uniformSlots_unused]
  have hn' : remainingTotal (Ideal.uniformSlots eligible) ≠ 0 := by omega
  simp only [drawEligibleF, hu, Candidate.weight]
  change (if remainingTotal (Ideal.uniformSlots eligible) = 0 then none else _) = _
  rw [ite_eq_right hn']
  rfl

namespace Modulo

/-- Exact number of uniform raw words whose modulo reduction is `j`. -/
def residueCount (U m j : Nat) : Nat :=
  (List.range U).countP (fun r => decide (r % m = j))

/-- Every residue has floor or ceiling multiplicity. This is an exact
finite count, not an assumption about SHA-256. -/
theorem residue_count (U m j : Nat) (hm : 0 < m) (hj : j < m) :
    residueCount U m j = U / m + (if j < U % m then 1 else 0) := by
  induction U with
  | zero => simp [residueCount]
  | succ U ih =>
    have hs : residueCount (U + 1) m j = residueCount U m j +
        (if U % m = j then 1 else 0) := by
      simp [residueCount, List.range_succ]
    rw [hs, ih]
    have hmod := Nat.mod_lt U hm
    have hsplit := Nat.mod_add_div U m
    by_cases hwrap : U % m + 1 = m
    · have hu : U + 1 = m * (U / m + 1) := by rw [Nat.mul_add, Nat.mul_one]; omega
      have hd : (U + 1) / m = U / m + 1 := by
        rw [hu, Nat.mul_div_right _ hm]
      have hmod' : (U + 1) % m = 0 := by rw [hu]; simp
      rw [hd, hmod']
      simp only [Nat.not_lt_zero, ↓reduceIte]
      split <;> split <;> omega
    · have hr : U % m + 1 < m := by omega
      have hu : U + 1 = (U % m + 1) + m * (U / m) := by omega
      have hd : (U + 1) / m = U / m := by
        rw [hu, Nat.add_mul_div_left _ _ hm, Nat.div_eq_of_lt hr, Nat.zero_add]
      have hmod' : (U + 1) % m = U % m + 1 := by
        rw [hu, Nat.add_mul_mod_self_left, Nat.mod_eq_of_lt hr]
      rw [hd, hmod']
      split <;> split <;> split <;> omega

private theorem residue_bound (U m j : Nat) (hm : 0 < m) (hj : j < m) :
    residueCount U m j * m ≤ U + m := by
  rw [residue_count U m j hm hj]
  have h := Nat.div_mul_le_self U m
  split <;> simp only [Nat.add_mul, Nat.one_mul, Nat.zero_mul] <;> omega

private theorem sum_append {α : Type} (f : α → Nat) (xs ys : List α) :
    Ideal.sum f (xs ++ ys) = Ideal.sum f xs + Ideal.sum f ys := by
  induction xs with
  | nil => simp [Ideal.sum]
  | cons x xs ih => simp [Ideal.sum, ih, Nat.add_assoc]

private theorem sum_singleton_range (r m : Nat) :
    Ideal.sum (fun j => if r = j then 1 else 0) (List.range m) =
      if r < m then 1 else 0 := by
  induction m with
  | zero => simp [Ideal.sum]
  | succ m ih =>
    rw [List.range_succ, sum_append, ih]
    simp only [Ideal.sum, Nat.add_zero]
    split <;> split <;> split <;> omega

/-- The residue buckets partition the entire raw-word domain. -/
theorem residue_total (U m : Nat) (hm : 0 < m) :
    Ideal.sum (residueCount U m) (List.range m) = U := by
  induction U with
  | zero =>
    change Ideal.sum (fun _ : Nat => 0) (List.range m) = 0
    simp [Ideal.sum_const]
  | succ U ih =>
    have hs : ∀ j, residueCount (U + 1) m j = residueCount U m j +
        (if U % m = j then 1 else 0) := by
      intro j
      simp [residueCount, List.range_succ]
    have he := Ideal.sum_congr (List.range m) (fun j _ => hs j)
    rw [he, Ideal.sum_add, ih, sum_singleton_range]
    simp [Nat.mod_lt U hm]

/-- Multiplicity of a reduced roll path under independent uniform words.
Independence is exactly what makes these per-step counts multiply. -/
def pathWeight (U : Nat) : Nat → List Nat → Nat
  | _, [] => 1
  | n, r :: rs => residueCount U n r * pathWeight U (n - 1) rs

/-- Normalization: grouping raw IID tuples by reduced roll paths preserves
the raw domain's total mass `U^t`, including paths of fewer than four draws. -/
theorem pathWeight_total (U n t : Nat) (ht : t ≤ n) :
    Ideal.sum (pathWeight U n) (Ideal.paths n t) = U ^ t := by
  induction t generalizing n with
  | zero => simp [Ideal.paths, Ideal.sum, pathWeight]
  | succ t ih =>
    have hn : 0 < n := by omega
    simp only [Ideal.paths, Ideal.sum_flatMap, Ideal.sum_map, pathWeight]
    have he : ∀ r, Ideal.sum (fun rs => residueCount U n r * pathWeight U (n - 1) rs)
        (Ideal.paths (n - 1) t) = residueCount U n r * U ^ t := by
      intro r
      simp only [Nat.mul_comm (residueCount U n r), Ideal.sum_mul, ih (n - 1) (by omega)]
    have hs := Ideal.sum_congr (List.range n) (fun r _ => he r)
    rw [hs, Ideal.sum_mul, residue_total U n hn, Nat.pow_succ]
    ac_rfl

private theorem path_bound (U ρ n t : Nat) (rs : List Nat)
    (hn : n ≤ ρ) (ht : t ≤ n) (hp : rs ∈ Ideal.paths n t) :
    pathWeight U n rs * Ideal.mass n t ≤ (U + ρ) ^ t := by
  induction t generalizing n rs with
  | zero =>
    have : rs = [] := by simpa [Ideal.paths] using hp
    subst rs
    simp [pathWeight, Ideal.mass]
  | succ t ih =>
    cases rs with
    | nil => simp [Ideal.paths_nil] at hp
    | cons r rs =>
      obtain ⟨hr, htail⟩ := (Ideal.paths_cons n t r rs).mp hp
      have hn0 : 0 < n := by omega
      have hb := residue_bound U n r hn0 hr
      have hrec := ih (n - 1) rs (by omega) (by omega) htail
      rw [pathWeight, Ideal.mass_succ _ _ hn0]
      calc
        _ = (residueCount U n r * n) * (pathWeight U (n - 1) rs * Ideal.mass (n - 1) t) := by ac_rfl
        _ ≤ (U + ρ) * (U + ρ) ^ t := Nat.mul_le_mul (by omega) hrec
        _ = _ := by rw [Nat.pow_succ]; ac_rfl

private theorem sum_le {α : Type} (f g : α → Nat) (xs : List α)
    (h : ∀ x ∈ xs, f x ≤ g x) : Ideal.sum f xs ≤ Ideal.sum g xs := by
  induction xs with
  | nil => exact Nat.le_refl _
  | cons x xs ih =>
    apply Nat.add_le_add (h x (by simp)) (ih (fun y hy => h y (by simp [hy])))

/-- Raw-word payout count, after grouping raw tuples by reduced roll path. -/
def winningsMass (U : Nat) (owns : Bytes → Bool) (pool t : Nat)
    (cs : List Candidate) : Nat :=
  Ideal.sum (fun rs => ownerWinnings owns
      (Ideal.draw (fun j _ => (rs[j]?).getD 0) pool t cs) * pathWeight U cs.length rs)
    (Ideal.paths cs.length t)

private theorem ideal_mass_pos (n t : Nat) (ht : t ≤ n) : 0 < Ideal.mass n t := by
  induction t generalizing n with
  | zero => simp [Ideal.mass]
  | succ t ih =>
    have hn : 0 < n := by omega
    rw [Ideal.mass_succ _ _ hn]
    exact Nat.mul_pos hn (ih (n - 1) (by omega))

/-- Multiplicative modulo slack for any word-domain size. After division
by `U^t`, this is the ideal ticket share times `(1 + n/U)^t`. -/
theorem expectation_slack (U : Nat) (owns : Bytes → Bool) (pool t : Nat)
    (cs : List Candidate) (ht0 : 0 < t) (ht : t ≤ cs.length) :
    winningsMass U owns pool t cs * cs.length ≤
      cs.countP (fun c => owns c.id) * pool * (U + cs.length) ^ t := by
  have hb : winningsMass U owns pool t cs * Ideal.mass cs.length t ≤
      Ideal.winningsMass owns pool t cs * (U + cs.length) ^ t := by
    unfold winningsMass Ideal.winningsMass
    rw [← Ideal.sum_mul, ← Ideal.sum_mul]
    apply sum_le
    intro rs hrs
    rw [Nat.mul_assoc]
    exact Nat.mul_le_mul_left _ (path_bound U cs.length cs.length t rs (by omega) ht hrs)
  have hs := Ideal.exact_share owns pool t cs ht0 ht
  have hmul := Nat.mul_le_mul_right cs.length hb
  have he1 : winningsMass U owns pool t cs * Ideal.mass cs.length t * cs.length =
      (winningsMass U owns pool t cs * cs.length) * Ideal.mass cs.length t := by ac_rfl
  have he2 : Ideal.winningsMass owns pool t cs * (U + cs.length) ^ t * cs.length =
      (Ideal.winningsMass owns pool t cs * cs.length) * (U + cs.length) ^ t := by ac_rfl
  rw [he1, he2, hs] at hmul
  apply Nat.le_of_mul_le_mul_right (c := Ideal.mass cs.length t) (hc := ideal_mass_pos _ _ ht)
  simpa only [Nat.mul_assoc, Nat.mul_comm, Nat.mul_left_comm] using hmul

/-- **Trusted SHA bridge.** `hIID` states explicitly that the observed
reduced-path multiplicities are those of independent uniform u128 words.
It is not proved for SHA-256. With at most four draws the relative slack
is at most `(1 + n/2^128)^4`, written without rational arithmetic. -/
theorem u128_iid_bridge (owns : Bytes → Bool) (pool t : Nat) (cs : List Candidate)
    (observed : List Nat → Nat) (ht0 : 0 < t) (ht : t ≤ cs.length) (ht4 : t ≤ 4)
    (hIID : ∀ rs ∈ Ideal.paths cs.length t,
      observed rs = pathWeight (2 ^ 128) cs.length rs) :
    Ideal.sum (fun rs => ownerWinnings owns
        (Ideal.draw (fun j _ => (rs[j]?).getD 0) pool t cs) * observed rs)
        (Ideal.paths cs.length t) * cs.length * (2 ^ 128) ^ (4 - t) ≤
      cs.countP (fun c => owns c.id) * pool * (2 ^ 128 + cs.length) ^ 4 := by
  have he : Ideal.sum (fun rs => ownerWinnings owns
      (Ideal.draw (fun j _ => (rs[j]?).getD 0) pool t cs) * observed rs)
      (Ideal.paths cs.length t) = winningsMass (2 ^ 128) owns pool t cs := by
    apply Ideal.sum_congr
    intro rs hrs
    rw [hIID rs hrs]
  rw [he]
  have hb := expectation_slack (2 ^ 128) owns pool t cs ht0 ht
  have hp : (2 ^ 128) ^ (4 - t) ≤ (2 ^ 128 + cs.length) ^ (4 - t) :=
    Nat.pow_le_pow_left (by omega) _
  calc
    _ ≤ (cs.countP (fun c => owns c.id) * pool * (2 ^ 128 + cs.length) ^ t) *
        (2 ^ 128 + cs.length) ^ (4 - t) := Nat.mul_le_mul hb hp
    _ = _ := by rw [Nat.mul_assoc, ← Nat.pow_add, Nat.add_sub_of_le ht4]

end Modulo

private theorem scan_complete (cap h : Nat) (cfg : DrawConfig) (rows : List Utxo)
    (taken : Nat) (hc : taken + rows.countP (fun u => u.toCandidate.isEligible h cfg) ≤ cap) :
    scan cap h cfg rows taken =
      (rows.filter (fun u => u.toCandidate.isEligible h cfg)).map Utxo.toCandidate := by
  induction rows generalizing taken with
  | nil => rfl
  | cons u us ih =>
    by_cases hstop : cap ≤ taken
    · have hz : ((u :: us).filter (fun v => v.toCandidate.isEligible h cfg)).length = 0 := by
        rw [← List.countP_eq_length_filter]
        omega
      have he := List.eq_nil_of_length_eq_zero hz
      simp [scan, hstop, he]
    · cases hu : u.toCandidate.isEligible h cfg
      · simp only [scan, hstop, hu, Bool.false_eq_true, ↓reduceIte, List.filter_cons]
        apply ih
        simpa [hu] using hc
      · simp only [scan, hstop, hu, ↓reduceIte, List.filter_cons, List.map_cons]
        congr 1
        apply ih
        simp [hu] at hc
        omega

/-- Below the **global** eligible-row cap, the candidate read contains
every eligible ticket, up to rotation. The bound is not on the already
truncated returned list. No assumption about the offset hash is needed. -/
theorem lotteryCandidates_complete (P : Prims) (rows : List Utxo) (h : Nat)
    (seed : Bytes) (cfg : DrawConfig)
    (hc : rows.countP (fun u => u.toCandidate.isEligible h cfg) ≤ MAX_LOTTERY_CANDIDATES) :
    (lotteryCandidates P rows h seed cfg).Perm
      ((rows.filter (fun u => u.toCandidate.isEligible h cfg)).map Utxo.toCandidate) := by
  have hp := rotate_perm (offsetKey P seed h) rows
  unfold lotteryCandidates
  rw [scan_complete _ _ _ _ 0 (by rw [hp.countP_eq]; simpa using hc)]
  exact (hp.filter (fun u => u.toCandidate.isEligible h cfg)).map Utxo.toCandidate

namespace Ideal

/-- A real candidate-read input, with the payout supplied by accounting.
The theorem below supplies only the ideal expected-share premise of R3;
R3's lifetime, pricing and fee premises remain explicit. -/
structure Input where
  rows : List Utxo
  height : Nat
  seed : Bytes
  pool : Nat

def Input.eligible (d : Input) : List Candidate :=
  (d.rows.filter (fun u => u.toCandidate.isEligible d.height canonicalConfig.draw)).map Utxo.toCandidate

def Input.candidates (d : Input) (P : Prims) : List Candidate :=
  lotteryCandidates P d.rows d.height d.seed canonicalConfig.draw

def Input.n (d : Input) : Nat := d.eligible.length

def Input.t (d : Input) : Nat := minN canonicalConfig.draw.winnersPerDraw d.n

def Input.held (d : Input) (owns : Bytes → Bool) : Nat := d.eligible.countP (fun c => owns c.id)

def Input.observe (d : Input) (P : Prims) (owns : Bytes → Bool) (scale : Nat) : DrawObs :=
  { held := d.held owns, n := d.n, payout := d.pool,
    win := winningsMass owns d.pool d.t (d.candidates P) * scale }

private theorem input_perm (P : Prims) (d : Input) (hc : d.n ≤ MAX_LOTTERY_CANDIDATES) :
    (d.candidates P).Perm d.eligible := by
  apply lotteryCandidates_complete
  simpa [Input.n, Input.eligible, List.countP_eq_length_filter] using hc

private theorem input_t (d : Input) (hn : 0 < d.n) : 0 < d.t ∧ d.t ≤ d.n := by
  simp only [Input.t, canonicalConfig, DrawConfig.default, minN]
  by_cases h : 4 ≤ d.n <;> simp [h] <;> omega

/-- **ST-4's share premise is discharged**, for the full eligible set
within the candidate cap. `D = mass * scale` is exact common-denominator
conversion, not an assumed winnings bound. -/
theorem observation_share (P : Prims) (d : Input) (owns : Bytes → Bool) (scale D : Nat)
    (hn : 0 < d.n) (hc : d.n ≤ MAX_LOTTERY_CANDIDATES)
    (hD : D = mass d.n d.t * scale) :
    (d.observe P owns scale).win * (d.observe P owns scale).n =
      (d.observe P owns scale).held * (d.observe P owns scale).payout * D := by
  have hp := input_perm P d hc
  have hl : (d.candidates P).length = d.n := hp.length_eq
  have ht := input_t d hn
  have hs := exact_share owns d.pool d.t (d.candidates P) ht.1 (by rw [hl]; exact ht.2)
  rw [hl, hp.countP_eq] at hs
  unfold Input.observe Input.held
  dsimp only
  rw [hD]
  calc
    _ = (winningsMass owns d.pool d.t (d.candidates P) * d.n) * scale := by ac_rfl
    _ = _ := by rw [hs]; ac_rfl

/-- R3 applied to ideal model draws, with no abstract `hshare` premise.
Each input's globally eligible rows must fit within the candidate cap.
Varying path denominators are converted exactly to the shared `D`. -/
theorem r3_from_model (P : Prims) (owns : Bytes → Bool) (L p k outlay D : Nat)
    (ds : List Input) (scale : Input → Nat) (hL : 0 < L) (_hD : 0 < D)
    (hwindow : ∀ d ∈ ds, 0 < d.n ∧ d.n ≤ MAX_LOTTERY_CANDIDATES)
    (hdenom : ∀ d ∈ ds, D = mass d.n d.t * scale d)
    (hprice : ∀ d ∈ ds, d.pool * L ≤ d.n * p)
    (hlife : sumN (ds.map (fun d => d.held owns)) ≤ k * L)
    (hfee : k * p ≤ outlay) :
    sumN (ds.map (fun d => (d.observe P owns (scale d)).win)) ≤ outlay * D := by
  suffices h : sumN ((ds.map (fun d => d.observe P owns (scale d))).map DrawObs.win) ≤ outlay * D by
    simpa only [List.map_map, Function.comp_def] using h
  apply r3_no_split_profit L p k outlay D (ds.map (fun d => d.observe P owns (scale d))) hL
  · intro obs hobs
    obtain ⟨d, hd, rfl⟩ := List.mem_map.mp hobs
    exact Nat.le_of_eq (observation_share P d owns (scale d) D (hwindow d hd).1
      (hwindow d hd).2 (hdenom d hd))
  · intro obs hobs
    obtain ⟨d, hd, rfl⟩ := List.mem_map.mp hobs
    exact ⟨(hwindow d hd).1, hprice d hd⟩
  · simpa [List.map_map, Function.comp_def, Input.observe] using hlife
  · exact hfee

private theorem winnings_le_iff (owns : Bytes → Bool) (pool t cost : Nat)
    (cs : List Candidate) (ht0 : 0 < t) (ht : t ≤ cs.length) :
    winningsMass owns pool t cs ≤ cost * mass cs.length t ↔
      cs.countP (fun c => owns c.id) * pool ≤ cost * cs.length := by
  have hn : 0 < cs.length := by omega
  have hm := Modulo.ideal_mass_pos cs.length t ht
  rw [← Nat.mul_le_mul_right_iff hn, exact_share owns pool t cs ht0 ht]
  have he : cost * mass cs.length t * cs.length = cost * cs.length * mass cs.length t := by ac_rfl
  rw [he, Nat.mul_le_mul_right_iff hm]

/-- C1's stationary-economy inequality is precisely the ideal model's
expected-profit test when the stated fee-recycling formula gives the
draw's payout. `hpayout` is the economy premise, not a consensus fact. -/
theorem splitUnprofitable_model_iff (owns : Bytes → Bool) (cs : List Candidate)
    (R0 c ρo pm k payout t : Nat) (ht0 : 0 < t) (ht : t ≤ cs.length)
    (hn : cs.length = ρo + k + 1) (hk : cs.countP (fun x => owns x.id) = k)
    (hpayout : 1000 * payout = 1000 * R0 + pm * k * c) :
    winningsMass owns payout t cs ≤ k * c * mass cs.length t ↔
      splitUnprofitable R0 c ρo pm k := by
  rw [winnings_le_iff owns payout t (k * c) cs ht0 ht, hn, hk]
  rw [← Nat.mul_le_mul_left_iff (show 0 < 1000 by decide)]
  unfold splitUnprofitable
  have he : 1000 * (k * payout) = k * (1000 * payout) := by ac_rfl
  rw [he, hpayout]
  simp only [Nat.mul_assoc]

private theorem cap_below_window (n : Nat) (hn : n ≤ MAX_LOTTERY_CANDIDATES) :
    sybilRewardCap n = n * LOTTERY_BASE_FEE_PICO := by
  have hb := Nat.mul_le_mul_right LOTTERY_BASE_FEE_PICO hn
  have hmax : MAX_LOTTERY_CANDIDATES * LOTTERY_BASE_FEE_PICO ≤ U128_MAX := by decide
  simp [sybilRewardCap, satMul, Nat.le_trans hb hmax]

private theorem common_budget_marginal (B b n k : Nat) :
    (k + 1) * min B ((n + k) * b) * n ≤
      min B (n * b) * (n + k) + k * b * (n * (n + k)) := by
  by_cases hB : B ≤ n * b
  · have hb : n * b ≤ (n + k) * b := Nat.mul_le_mul_right b (by omega)
    rw [Nat.min_eq_left hB, Nat.min_eq_left (Nat.le_trans hB hb)]
    calc
      _ ≤ (k + 1) * B * (n + k) := Nat.mul_le_mul_left _ (by omega)
      _ = B * (n + k) + k * B * (n + k) := by
        simp only [Nat.add_mul, Nat.one_mul]
        omega
      _ ≤ B * (n + k) + k * (n * b) * (n + k) :=
        Nat.add_le_add_left (Nat.mul_le_mul_right _ (Nat.mul_le_mul_left k hB)) _
      _ = _ := by ac_rfl
  · rw [Nat.min_eq_right (show n * b ≤ B by omega)]
    calc
      _ ≤ (k + 1) * ((n + k) * b) * n :=
        Nat.mul_le_mul_right _ (Nat.mul_le_mul_left _ (Nat.min_le_right _ _))
      _ = _ := by
        simp only [Nat.add_mul, Nat.mul_add, Nat.one_mul]
        ac_rfl

/-- **Marginal ideal-share bound under a common budget.** The two draws
share `B`, the available payout before the Sybil clamp. This is essential:
the cap alone does not constrain an unrelated change in available funds.
The two finite path counts are kept as distinct exact denominators. -/
theorem marginal_common_budget (owns : Bytes → Bool) (before after : List Candidate)
    (B k tb ta : Nat) (hb0 : 0 < tb) (hb : tb ≤ before.length)
    (ha0 : 0 < ta) (ha : ta ≤ after.length)
    (hn : after.length = before.length + k)
    (hbheld : before.countP (fun c => owns c.id) = 1)
    (haheld : after.countP (fun c => owns c.id) = k + 1)
    (hcap : after.length ≤ MAX_LOTTERY_CANDIDATES) :
    winningsMass owns (min B (sybilRewardCap after.length)) ta after * mass before.length tb ≤
      winningsMass owns (min B (sybilRewardCap before.length)) tb before * mass after.length ta +
        k * LOTTERY_BASE_FEE_PICO * mass before.length tb * mass after.length ta := by
  have hbcap : before.length ≤ MAX_LOTTERY_CANDIDATES := by omega
  rw [cap_below_window after.length hcap, cap_below_window before.length hbcap]
  have hbs := exact_share owns (min B (before.length * LOTTERY_BASE_FEE_PICO)) tb before hb0 hb
  have has := exact_share owns (min B (after.length * LOTTERY_BASE_FEE_PICO)) ta after ha0 ha
  rw [hbheld, Nat.one_mul] at hbs
  rw [haheld] at has
  have hm := common_budget_marginal B LOTTERY_BASE_FEE_PICO before.length k
  rw [← hn] at hm
  have hp := Nat.mul_le_mul_right (mass before.length tb * mass after.length ta) hm
  have hden : 0 < before.length * after.length := Nat.mul_pos (by omega) (by omega)
  apply Nat.le_of_mul_le_mul_right (c := before.length * after.length) (hc := hden)
  calc
    _ = (winningsMass owns (min B (after.length * LOTTERY_BASE_FEE_PICO)) ta after * after.length) *
        before.length * (mass before.length tb) := by ac_rfl
    _ = ((k + 1) * min B (after.length * LOTTERY_BASE_FEE_PICO) * before.length) *
        (mass before.length tb * mass after.length ta) := by rw [has]; ac_rfl
    _ ≤ (min B (before.length * LOTTERY_BASE_FEE_PICO) * after.length +
        k * LOTTERY_BASE_FEE_PICO * (before.length * after.length)) *
        (mass before.length tb * mass after.length ta) := hp
    _ = (winningsMass owns (min B (before.length * LOTTERY_BASE_FEE_PICO)) tb before * before.length) *
        after.length * mass after.length ta +
        (k * LOTTERY_BASE_FEE_PICO * mass before.length tb * mass after.length ta) *
          (before.length * after.length) := by
      rw [hbs]
      simp only [Nat.add_mul]
      ac_rfl
    _ = _ := by simp only [Nat.add_mul]; ac_rfl

/-- Without the common-budget premise, the cap-only marginal claim is
false. This arithmetic counterexample is not a reachable-consensus claim:
before has two tickets/payout 1; after has three tickets/payout 3*base. -/
theorem marginal_cap_only_refuted :
    ∃ R R' : Nat, R ≤ 2 * LOTTERY_BASE_FEE_PICO ∧ R' ≤ 3 * LOTTERY_BASE_FEE_PICO ∧
      R * 3 + LOTTERY_BASE_FEE_PICO * (2 * 3) < 2 * R' * 2 := by
  exact ⟨1, 3 * LOTTERY_BASE_FEE_PICO, by decide, by decide, by decide⟩

/-- **CE-2.** At 10,000 globally eligible tickets, epoch-1 reward 25 BTH
and emission-only payout 2.5 BTH, one draw's **expected** winnings exceed
the one-time ticket cost whenever `0 < held` and `c ≤ 6.25e6` pico.
The latter is the 1e8-pico relay fee divided among 16 outputs, not the fee
constant itself. Nothing here guarantees an individual realized win. -/
theorem ce2_expected_profit (P : Prims) (d : Input) (owns : Bytes → Bool) (c : Nat)
    (hn : d.n = 10000) (hk : 0 < d.held owns) (hc : c ≤ 6250000) :
    let cap := rewardCap (d.candidates P) d.height 25000000000000 canonicalConfig
    let payout := (computePoolAccounting 0 2500000000000 0 cap canonicalConfig).payout
    payout = 2500000000000 ∧
      d.held owns * c * mass d.n 4 < winningsMass owns payout 4 (d.candidates P) := by
  have hglob : d.rows.countP (fun u => u.toCandidate.isEligible d.height canonicalConfig.draw) = 10000 := by
    simpa [Input.n, Input.eligible, List.countP_eq_length_filter] using hn
  have hcap := rewardCap_saturated P d.rows d.height 25000000000000 d.seed canonicalConfig
    (by simp [MAX_LOTTERY_CANDIDATES, hglob]) (by decide)
  dsimp only [Input.candidates]
  rw [hcap]
  change 2500000000000 = 2500000000000 ∧
    d.held owns * c * mass d.n 4 < winningsMass owns 2500000000000 4 (d.candidates P)
  refine ⟨rfl, ?_⟩
  have hp := input_perm P d (by simp [hn, MAX_LOTTERY_CANDIDATES])
  have hl : (d.candidates P).length = d.n := hp.length_eq
  apply Nat.lt_of_not_ge
  intro hle
  have htest := (winnings_le_iff owns 2500000000000 4 (d.held owns * c)
    (d.candidates P) (by decide) (by rw [hl, hn]; decide)).mp (by simpa [hl] using hle)
  rw [hp.countP_eq, hl, hn] at htest
  change d.held owns * 2500000000000 ≤ d.held owns * c * 10000 at htest
  rw [Nat.mul_assoc, Nat.mul_le_mul_left_iff hk] at htest
  omega

end Ideal

private def regressionTicket (i : Nat) : Candidate :=
  { id := [i.toUInt8], value := 1000000, clusterFactor := 1000, creationBlock := 0 }

/-- Kernel-evaluated fixtures: zero pool, no winners, one ticket,
underfilled draws with a zero quotient, and a nonzero remainder in a
four-of-five draw. These execute the winner-loop path measure. -/
theorem ideal_draw_regressions :
    Ideal.winningsMass (fun id => id == [0]) 0 2 [regressionTicket 0, regressionTicket 1] = 0 ∧
    Ideal.winningsMass (fun id => id == [0]) 7 0 [regressionTicket 0] = 0 ∧
    Ideal.winningsMass (fun id => id == [0]) 7 1 [regressionTicket 0] = 7 ∧
    Ideal.winningsMass (fun id => id == [0]) 1 2 [regressionTicket 0, regressionTicket 1] = 1 ∧
    Ideal.winningsMass (fun id => id == [0]) 7 4
      [regressionTicket 0, regressionTicket 1, regressionTicket 2, regressionTicket 3, regressionTicket 4] = 168 := by
  set_option maxRecDepth 8192 in
    decide

/-- A toy 3-bit domain demonstrates strict modulo bias: the last ticket
gets 192/512 = 3/8 of a one-pico pool, instead of the ideal 2/6 = 1/3.
It prevents interpreting the real-hash bridge as an exact equality. -/
theorem modulo_bias_regression :
    Ideal.winningsMass (fun id => id == [2]) 1 3
      [regressionTicket 0, regressionTicket 1, regressionTicket 2] = 2 ∧
    Modulo.winningsMass 8 (fun id => id == [2]) 1 3
      [regressionTicket 0, regressionTicket 1, regressionTicket 2] = 192 ∧
    2 * (8 ^ 3) < 192 * 6 := by
  decide

end Botho.Lottery
