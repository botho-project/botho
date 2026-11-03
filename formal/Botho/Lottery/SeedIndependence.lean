/- Seed and winner-id independence for the existing LotteryV2 model (#1850).
No assertion here equates the model with Rust; see formal/lottery.md. -/
import Botho.Lottery.Drawing

namespace Botho.Lottery

/-- Current-block fee, reward and emission fields do not reach the seed. -/
theorem seed_body_independent (P : Prims) (r : Reads) (b₁ b₂ : DrawBlock)
    (hh : b₁.height = b₂.height) (hp : b₁.prevBlockHash = b₂.prevBlockHash) :
    seed P r b₁ = seed P r b₂ := by
  simp only [seed, hh, hp]

/-- The seed reads the committed tip, independently of the pool and rows. -/
theorem seed_reads_tip_only (P : Prims) (r₁ r₂ : Reads) (b : DrawBlock)
    (ht : r₁.tip = r₂.tip) : seed P r₁ b = seed P r₂ b := by
  simp only [seed, ht]

/-- Successful seed generation factors through the parent's PoW output,
parent previous hash and current height. Parent-link validation still applies. -/
theorem seed_factors (P : Prims) (r : Reads) (b : DrawBlock) (s : Bytes)
    (hs : seed P r b = .ok s) :
    s = lotterySeedV2 P (P.powHash r.tip) r.tip.prevBlockHash b.height := by
  unfold seed at hs
  split at hs <;> simp_all

/-- Equal PoW output, parent previous hash and height give equal successful
seeds, even if other parent header fields differ. -/
theorem seed_eq_of_pow_fields (P : Prims) (r₁ r₂ : Reads) (b₁ b₂ : DrawBlock)
    (s₁ s₂ : Bytes) (hs₁ : seed P r₁ b₁ = .ok s₁) (hs₂ : seed P r₂ b₂ = .ok s₂)
    (hp : P.powHash r₁.tip = P.powHash r₂.tip)
    (hv : r₁.tip.prevBlockHash = r₂.tip.prevBlockHash) (hh : b₁.height = b₂.height) :
    s₁ = s₂ := by
  rw [seed_factors P r₁ b₁ s₁ hs₁, seed_factors P r₂ b₂ s₂ hs₂, hp, hv, hh]

/-- Decomposition of every successful drawing: the actual candidate argument
is the seeded window of the pinned N−1 post-state, at block N's height.
This is the #1689 surface (https://github.com/botho-project/botho/issues/1689);
a change to pre-state versus post-state timing must update this theorem and its Rust tie. -/
theorem candidates_of_postState (P : Prims) (r : Reads) (b : DrawBlock)
    (res : BlockLotteryResult) (acc : Accounting) (h : drawing P r b = .ok (res, acc)) :
    ∃ s fees, seed P r b = .ok s ∧ sumFeesChecked 0 b.fees = some fees ∧
      let candidates := lotteryCandidates P r.rows b.height s canonicalConfig.draw
      acc = computePoolAccounting fees b.emissionShare r.lotteryPool
        (rewardCap candidates b.height b.reward canonicalConfig) canonicalConfig ∧
      res = drawLotteryWinnersSeeded P candidates fees acc b.height s canonicalConfig := by
  unfold drawing at h
  simp only [show ¬ CANDIDATE_MAX_AWARDS < canonicalConfig.draw.winnersPerDraw from by decide,
    ↓reduceIte] at h
  split at h
  · cases h
  · rename_i s hs
    split at h
    · cases h
    · rename_i fees hf
      split at h
      · cases h
      · cases h
        exact ⟨s, fees, hs, hf, rfl, rfl⟩

private theorem pickWinners_ids (P : Prims) (s : Bytes) (p q i n : Nat) (slots : List Slot) :
    (pickWinners P s p i n slots).map Winner.utxoId =
      (pickWinners P s q i n slots).map Winner.utxoId := by
  induction n generalizing i slots with
  | zero => rfl
  | succ n ih =>
    simp only [pickWinners]
    split
    · rfl
    · split <;> simp only [List.map_cons, ih]

private theorem addToLast_ids (n : Nat) (ws : List Winner) :
    (addToLast n ws).map Winner.utxoId = ws.map Winner.utxoId := by
  induction ws with
  | nil => rfl
  | cons w ws ih =>
    cases ws with
    | nil => rfl
    | cons v vs => simpa only [addToLast, List.map_cons] using congrArg (w.utxoId :: ·) ih

private theorem drawEligible_ids (P : Prims) (cs : List Candidate) (p q h : Nat)
    (s : Bytes) (cfg : DrawConfig) :
    ((drawEligible P cs p h s cfg).map (fun r => r.winners.map Winner.utxoId)) =
      ((drawEligible P cs q h s cfg).map (fun r => r.winners.map Winner.utxoId)) := by
  simp only [drawEligible]
  split
  · rfl
  · simp only [Option.map_some, addToLast_ids, pickWinners_ids P s _ q]

private theorem drawWinners_ids (P : Prims) (cs : List Candidate) (p q h : Nat)
    (s : Bytes) (cfg : DrawConfig) (hp : p ≠ 0) (hq : q ≠ 0) :
    ((drawWinnersWithSeed P cs p h s cfg).map (fun r => r.winners.map Winner.utxoId)) =
      ((drawWinnersWithSeed P cs q h s cfg).map (fun r => r.winners.map Winner.utxoId)) := by
  simp only [drawWinnersWithSeed, hp, hq, decide_false, Bool.or_false]
  split
  · rfl
  · exact drawEligible_ids P _ p q h s cfg

/-- For fixed seed, candidates and height, positive payout magnitudes and fees
change amounts but not selected IDs. This holds for every draw configuration. -/
theorem drawLotteryWinnersSeeded_ids (P : Prims) (cs : List Candidate) (f₁ f₂ : Nat)
    (a₁ a₂ : Accounting) (h : Nat) (s : Bytes) (cfg : FeeConfig)
    (hp₁ : a₁.payout ≠ 0) (hp₂ : a₂.payout ≠ 0) :
    (drawLotteryWinnersSeeded P cs f₁ a₁ h s cfg).winners.map Winner.utxoId =
      (drawLotteryWinnersSeeded P cs f₂ a₂ h s cfg).winners.map Winner.utxoId := by
  have hd := drawWinners_ids P cs a₁.payout a₂.payout h s cfg.draw hp₁ hp₂
  simp only [drawLotteryWinnersSeeded, hp₁, hp₂, ↓reduceIte]
  cases h₁ : drawWinnersWithSeed P cs a₁.payout h s cfg.draw <;>
    cases h₂ : drawWinnersWithSeed P cs a₂.payout h s cfg.draw <;>
    simp_all [BlockLotteryResult.noWinners]

/-- On successful drawings with positive payouts, changing fees, reward,
emission share or stored lottery pool cannot change winner IDs. Rows, tip,
height and parent link are held fixed; payout amounts may change. -/
theorem winners_ids_independent_of_amounts (P : Prims) (r₁ r₂ : Reads)
    (b₁ b₂ : DrawBlock) (res₁ res₂ : BlockLotteryResult) (a₁ a₂ : Accounting)
    (ht : r₁.tip = r₂.tip) (hr : r₁.rows = r₂.rows)
    (hh : b₁.height = b₂.height) (hp : b₁.prevBlockHash = b₂.prevBlockHash)
    (hd₁ : drawing P r₁ b₁ = .ok (res₁, a₁))
    (hd₂ : drawing P r₂ b₂ = .ok (res₂, a₂))
    (ha₁ : a₁.payout ≠ 0) (ha₂ : a₂.payout ≠ 0) :
    res₁.winners.map Winner.utxoId = res₂.winners.map Winner.utxoId := by
  obtain ⟨s₁, f₁, hs₁, _, _, he₁⟩ := candidates_of_postState P r₁ b₁ res₁ a₁ hd₁
  obtain ⟨s₂, f₂, hs₂, _, _, he₂⟩ := candidates_of_postState P r₂ b₂ res₂ a₂ hd₂
  have hs : s₁ = s₂ := by
    have he := (seed_reads_tip_only P r₁ r₂ b₁ ht).trans
      (seed_body_independent P r₂ b₁ b₂ hh hp)
    rw [hs₁, hs₂] at he
    exact Except.ok.inj he
  rw [he₁, he₂, hr, hh, hs]
  exact drawLotteryWinnersSeeded_ids P _ f₁ f₂ a₁ a₂ _ s₂ canonicalConfig ha₁ ha₂

end Botho.Lottery
