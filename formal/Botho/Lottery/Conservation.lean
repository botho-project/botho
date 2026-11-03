/-
Pool/fee conservation and Rust widths for LotteryV2 (#1849, #1708 part A).
Status: proven (model); the existing split_fees, draw_winners_with_seed,
compute_pool_accounting and drawing DRT legs test the Rust correspondence.
No ledger reachability or cryptographic claim is made here. Saturation loss
is retained explicitly so the whole-block proof (#1839) can discharge it.
-/
import Botho.Lottery.SeedIndependence
import Botho.Supply.Accounting

namespace Botho.Lottery

/-- A valid percentage conserves fees, reusing the Supply arithmetic lemmas.
`> 1000` is unreachable in consensus: `canonicalConfig` pins 800. The
extraction counterpart is #1838's `BothoAeneas.PoolAccounting.split_fees_permille_spec`;
this theorem is about the model, not the extracted Rust. -/
theorem splitFees_conserves (cfg : FeeConfig) (F : Nat)
    (hp : cfg.poolFractionPermille ≤ 1000) :
    (splitFees cfg F).1 + (splitFees cfg F).2 = F := by
  have hq := Botho.Supply.fee_quotient_le F cfg.poolFractionPermille hp
  have hm := Nat.mod_le (F * cfg.poolFractionPermille / 1000) (2 ^ 64)
  have hb : (splitFees cfg F).1 ≤ F := by
    simp only [splitFees, truncU64]
    omega
  rw [Botho.Supply.splitFees_total]
  exact Nat.max_eq_left hb

/-- The canonical 800-permille split conserves every fee unit; the integer
rounding residue goes to burn. -/
theorem splitFees_canonical_conserves (F : Nat) :
    (splitFees canonicalConfig F).1 + (splitFees canonicalConfig F).2 = F :=
  splitFees_conserves canonicalConfig F (by decide)

private theorem pickWinners_paid_le (P : Prims) (seed : Bytes) (q i n : Nat)
    (slots : List Slot) : totalPaid (pickWinners P seed q i n slots) ≤ n * q := by
  induction n generalizing i slots with
  | zero => simp [pickWinners, totalPaid]
  | succ n ih =>
    simp only [pickWinners]
    split
    · simp [totalPaid]
    · split
      · rename_i c slots' hs
        simp only [totalPaid, Nat.succ_mul]
        have := ih (i + 1) slots'
        omega
      · rename_i slots' hs
        have := ih (i + 1) slots'
        simp only [Nat.succ_mul]
        omega

private theorem totalPaid_addToLast (extra : Nat) (ws : List Winner) (hn : ws ≠ []) :
    totalPaid (addToLast extra ws) = totalPaid ws + extra := by
  induction ws with
  | nil => contradiction
  | cons w ws ih =>
    cases ws with
    | nil => simp [addToLast, totalPaid]
    | cons v vs =>
      simp only [addToLast, totalPaid]
      have hi := ih (by simp)
      simp only [totalPaid] at hi
      omega

/-- A nonempty draw pays the full pool, including the remainder. Selection
may stop early when remaining weight is zero: no exact winner-count or
positive-weight hypothesis is needed. -/
theorem totalPaid_drawEligible (P : Prims) (eligible : List Candidate)
    (poolAmount blockHeight : Nat) (seed : Bytes) (cfg : DrawConfig) (r : DrawResult)
    (hd : drawEligible P eligible poolAmount blockHeight seed cfg = some r)
    (hn : r.winners ≠ []) : totalPaid r.winners = poolAmount := by
  unfold drawEligible at hd
  dsimp only at hd
  split at hd
  · cases hd
  · cases hd
    dsimp only at hn ⊢
    have hbound := pickWinners_paid_le P seed
      (poolAmount / minN cfg.winnersPerDraw eligible.length) 0
      (minN cfg.winnersPerDraw eligible.length)
      (eligible.map fun c => ({ cand := c, weight := c.weight cfg.mode ((maxValue? eligible).getD 1), used := false } : Slot))
    have hdiv := Nat.mul_div_le poolAmount (minN cfg.winnersPerDraw eligible.length)
    rw [totalPaid_addToLast]
    · omega
    · intro he
      rw [he] at hn
      exact hn rfl

private theorem drawWinnersWithSeed_paid (P : Prims) (cs : List Candidate)
    (pool h : Nat) (s : Bytes) (cfg : DrawConfig) (r : DrawResult)
    (hd : drawWinnersWithSeed P cs pool h s cfg = some r) (hn : r.winners ≠ []) :
    totalPaid r.winners = pool := by
  unfold drawWinnersWithSeed at hd
  dsimp only at hd
  split at hd
  · cases hd
  · exact totalPaid_drawEligible P _ pool h s cfg r hd hn

private theorem drawWinnersWithSeed_some_candidates (P : Prims) (cs : List Candidate)
    (pool h : Nat) (s : Bytes) (cfg : DrawConfig) (r : DrawResult)
    (hd : drawWinnersWithSeed P cs pool h s cfg = some r) : cs ≠ [] := by
  intro he
  subst cs
  simp [drawWinnersWithSeed] at hd

private theorem drawLottery_paid (P : Prims) (cs : List Candidate) (F : Nat)
    (a : Accounting) (h : Nat) (s : Bytes) (cfg : FeeConfig)
    (hguard : ¬(a.payout ≠ 0 ∧ !cs.isEmpty ∧
      (drawLotteryWinnersSeeded P cs F a h s cfg).winners.isEmpty)) :
    totalPaid (drawLotteryWinnersSeeded P cs F a h s cfg).winners =
      (drawLotteryWinnersSeeded P cs F a h s cfg).poolAmount ∧
    (drawLotteryWinnersSeeded P cs F a h s cfg).poolAmount ≤ a.payout := by
  unfold drawLotteryWinnersSeeded at hguard ⊢
  split
  · simp [BlockLotteryResult.noWinners, totalPaid]
  · rename_i hp
    split
    · rename_i r hd
      have hc := drawWinnersWithSeed_some_candidates P cs a.payout h s cfg.draw r hd
      have hn : r.winners ≠ [] := by
        simp only [hp, ↓reduceIte, hd] at hguard
        intro he
        apply hguard
        exact ⟨hp, by simpa using hc, by simp [he]⟩
      exact ⟨drawWinnersWithSeed_paid P cs a.payout h s cfg.draw r hd hn, Nat.le_refl _⟩
    · simp [BlockLotteryResult.noWinners, totalPaid]

private theorem minN_le_left (a b : Nat) : minN a b ≤ a := by
  unfold minN
  split <;> omega

private theorem minN_le_right (a b : Nat) : minN a b ≤ b := by
  unfold minN
  split <;> omega

/-- Successful drawing records exactly the total actually distributed, and
that distribution is bounded by payout and available funds. The guard rules
out positive-payout, nonempty-candidate draws with an empty winner list. -/
theorem drawing_paid (P : Prims) (r : Reads) (b : DrawBlock)
    (res : BlockLotteryResult) (acc : Accounting) (hd : drawing P r b = .ok (res, acc)) :
    totalPaid res.winners = res.poolAmount ∧ res.poolAmount ≤ acc.payout ∧
      acc.payout ≤ acc.available := by
  unfold drawing at hd
  simp only [show ¬ CANDIDATE_MAX_AWARDS < canonicalConfig.draw.winnersPerDraw from by decide,
    ↓reduceIte] at hd
  split at hd
  · cases hd
  · split at hd
    · cases hd
    · split at hd
      · cases hd
      · rename_i hguard
        cases hd
        have hm := drawLottery_paid P _ _ _ _ _ _ hguard
        exact ⟨hm.1, hm.2, minN_le_left _ _⟩

private theorem drawLottery_burn (P : Prims) (cs : List Candidate) (F : Nat)
    (a : Accounting) (h : Nat) (s : Bytes) (cfg : FeeConfig) :
    (drawLotteryWinnersSeeded P cs F a h s cfg).burnAmount = a.feeBurn := by
  unfold drawLotteryWinnersSeeded
  split
  · rfl
  · split <;> rfl

private theorem satAdd_le_sum (cap a b : Nat) : satAdd cap a b ≤ a + b := by
  unfold satAdd
  split <;> omega

private theorem satAdd_le_cap (cap a b : Nat) : satAdd cap a b ≤ cap := by
  unfold satAdd
  split <;> omega

private theorem accounting_available_le_sum (F E pool cap : Nat) (cfg : FeeConfig) :
    (computePoolAccounting F E pool cap cfg).available ≤ pool + E + (splitFees cfg F).1 := by
  have h₁ := satAdd_le_sum U128_MAX pool E
  have h₂ := satAdd_le_sum U128_MAX (satAdd U128_MAX pool E) (splitFees cfg F).1
  simp only [computePoolAccounting]
  omega

private theorem accounting_available_of_fit (F E pool cap : Nat) (cfg : FeeConfig)
    (hfit : pool + E + (splitFees cfg F).1 ≤ U128_MAX) :
    (computePoolAccounting F E pool cap cfg).available = pool + E + (splitFees cfg F).1 := by
  have hfirst : pool + E ≤ U128_MAX := by omega
  simp only [computePoolAccounting, satAdd, ite_eq_left hfirst, ite_eq_left hfit]

/-- Every successful drawing conserves pool inflow and fees once the exact
saturating-addition loss is accounted for. There is no no-saturation or
ledger-reachability hypothesis. Zero-payout and no-candidate branches are included. -/
theorem drawing_conserves (P : Prims) (r : Reads) (b : DrawBlock)
    (res : BlockLotteryResult) (acc : Accounting) (F : Nat)
    (hd : drawing P r b = .ok (res, acc)) (hf : sumFeesChecked 0 b.fees = some F) :
    let loss := (r.lotteryPool + b.emissionShare + acc.feePool) - acc.available
    totalPaid res.winners + res.burnAmount + acc.carryoverAfter res.poolAmount + loss =
      r.lotteryPool + b.emissionShare + F := by
  obtain ⟨s, fees, _, hfees, ha, hr⟩ := candidates_of_postState P r b res acc hd
  have he : fees = F := Option.some.inj (hfees.symm.trans hf)
  subst fees
  have hp := drawing_paid P r b res acc hd
  have hpaid : res.poolAmount ≤ acc.available := Nat.le_trans hp.2.1 hp.2.2
  have hburn : res.burnAmount = acc.feeBurn := by
    rw [hr]
    exact drawLottery_burn P _ _ _ _ _ _
  have hsplit : acc.feePool + acc.feeBurn = F := by
    rw [ha]
    exact splitFees_canonical_conserves F
  have hav : acc.available ≤ r.lotteryPool + b.emissionShare + acc.feePool := by
    rw [ha]
    exact accounting_available_le_sum F b.emissionShare r.lotteryPool _ canonicalConfig
  dsimp only
  rw [hp.1, hburn]
  simp only [Accounting.carryoverAfter, satSub, ite_eq_left hpaid]
  omega

/-- When the raw pool inflow fits u128, the explicit loss is zero and the
usual conservation identity follows. #1839 can discharge this premise from
its reachable-state supply bound; it is not assumed by drawing_conserves. -/
theorem drawing_conserves_no_loss (P : Prims) (r : Reads) (b : DrawBlock)
    (res : BlockLotteryResult) (acc : Accounting) (F : Nat)
    (hd : drawing P r b = .ok (res, acc)) (hf : sumFeesChecked 0 b.fees = some F)
    (hfit : r.lotteryPool + b.emissionShare + acc.feePool ≤ U128_MAX) :
    (r.lotteryPool + b.emissionShare + acc.feePool) - acc.available = 0 ∧
    totalPaid res.winners + res.burnAmount + acc.carryoverAfter res.poolAmount =
      r.lotteryPool + b.emissionShare + F := by
  obtain ⟨s, fees, _, hfees, ha, _⟩ := candidates_of_postState P r b res acc hd
  have he : fees = F := Option.some.inj (hfees.symm.trans hf)
  subst fees
  have hav : acc.available = r.lotteryPool + b.emissionShare + acc.feePool := by
    rw [ha] at hfit ⊢
    exact accounting_available_of_fit F b.emissionShare r.lotteryPool _ canonicalConfig hfit
  have hc := drawing_conserves P r b res acc F hd hf
  dsimp only at hc
  rw [hav] at hc ⊢
  constructor <;> omega

private theorem sumFeesChecked_le (acc F : Nat) (fees : List Nat)
    (ha : acc ≤ U64_MAX) (hf : sumFeesChecked acc fees = some F) : F ≤ U64_MAX := by
  induction fees generalizing acc with
  | nil => simp only [sumFeesChecked, Option.some.injEq] at hf; omega
  | cons f fs ih =>
    simp only [sumFeesChecked] at hf
    split at hf
    · rename_i acc' hc
      have hacc : acc' ≤ U64_MAX := by
        unfold checkedAdd at hc
        split at hc <;> simp_all
      exact ih acc' hacc hf
    · cases hf

private theorem winner_le_totalPaid (ws : List Winner) (w : Winner) (hw : w ∈ ws) :
    w.payout ≤ totalPaid ws := by
  induction ws with
  | nil => simp at hw
  | cons v vs ih =>
    simp only [List.mem_cons] at hw
    simp only [totalPaid]
    rcases hw with rfl | hw
    · omega
    · have := ih hw; omega

/-- Rust-width inputs give a u96 fee product, u128 available pool, and u64
payouts (individually and in total). Checked fee summation and the reward cap
supply the output bounds; the other input bounds record the Rust domain. -/
theorem drawing_widths (P : Prims) (r : Reads) (b : DrawBlock)
    (res : BlockLotteryResult) (acc : Accounting) (F : Nat)
    (hd : drawing P r b = .ok (res, acc)) (hf : sumFeesChecked 0 b.fees = some F)
    (hr : b.reward ≤ U64_MAX) (_he : b.emissionShare ≤ U64_MAX)
    (_hf : ∀ f ∈ b.fees, f ≤ U64_MAX) (_hp : r.lotteryPool ≤ U128_MAX) :
    F * canonicalConfig.poolFractionPermille < 2 ^ 96 ∧
    acc.available ≤ U128_MAX ∧ acc.payout ≤ U64_MAX ∧
    (∀ w ∈ res.winners, w.payout ≤ U64_MAX) ∧ totalPaid res.winners ≤ U64_MAX := by
  obtain ⟨s, fees, _, hfees, ha, _⟩ := candidates_of_postState P r b res acc hd
  have he : fees = F := Option.some.inj (hfees.symm.trans hf)
  subst fees
  have hF := sumFeesChecked_le 0 F b.fees (by simp [U64_MAX]) hf
  have hprod := Nat.mul_le_mul_right 800 hF
  have hconst : U64_MAX * 800 < 2 ^ 96 := by decide
  have hav : acc.available ≤ U128_MAX := by
    rw [ha]
    exact satAdd_le_cap _ _ _
  have hpay : acc.payout ≤ U64_MAX := by
    rw [ha]
    have h₁ := minN_le_right
      (computePoolAccounting F b.emissionShare r.lotteryPool
        (rewardCap (lotteryCandidates P r.rows b.height s canonicalConfig.draw)
          b.height b.reward canonicalConfig) canonicalConfig).available
      (rewardCap (lotteryCandidates P r.rows b.height s canonicalConfig.draw)
        b.height b.reward canonicalConfig)
    have h₂ := minN_le_right
      (sybilRewardCap (countEligible (lotteryCandidates P r.rows b.height s canonicalConfig.draw)
        b.height canonicalConfig.draw)) b.reward
    exact Nat.le_trans h₁ (Nat.le_trans h₂ hr)
  have hp := drawing_paid P r b res acc hd
  have ht : totalPaid res.winners ≤ U64_MAX := by omega
  refine ⟨?_, hav, hpay, ?_, ht⟩
  · exact Nat.lt_of_le_of_lt hprod hconst
  · intro w hw
    exact Nat.le_trans (winner_le_totalPaid res.winners w hw) ht

end Botho.Lottery
