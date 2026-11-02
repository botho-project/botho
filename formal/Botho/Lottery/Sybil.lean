/-
The Sybil-split question, without probability (#1708 part C1, epic #1704).

The design claim. `sybil_reward_cap`'s doc comment
(`cluster-tax/src/lottery.rs`) and research §9.1
(`docs/research/ct-compatible-lottery-selection.md`) said that a splitter
creating `k` of the `ρ` eligible outputs wins expected `k/ρ · R = k ·
base_fee`, "exactly the `k · base_fee` it pays in base fees", so splitting is
net-zero. Two facts about the code break the conclusion, although the per-draw
inequality `E[win] ≤ k · base_fee` itself is true (#1708 part C2, #1851,
proves it):

- no consensus rule charges `LOTTERY_BASE_FEE_PICO`; it appears only in the
  cap (the fee floor is #1684, and is not modelled here);
- a ticket is paid for once but stays eligible for `TICKET_LIFE` = 9,281
  draws (ages 720..=10,000).

So the statement refuted here is "winnings ≤ the fee a ticket is charged".
This module proves, about the model of #1707 (`drawing`, `rewardCap`,
`sybilRewardCap`, `countEligible`, `lotteryCandidates`):

1. **Counterexample** (`ce_owner_wins_three_base_fees`,
   `sybil_net_zero_refuted`). Four eligible rows, an owner of three. For
   every hash function `P` and every height in the 9,281-block eligibility
   window, the uniform draw pays every row, and the owner receives
   `3 · LOTTERY_BASE_FEE_PICO` = 7.5e11 pico in that one draw. With any
   per-ticket cost `c < LOTTERY_BASE_FEE_PICO` (every measured value is at
   most 6.25e6 pico, research §9.1.1), the owner is ahead after a single
   draw. The per-draw equality `k · base_fee` holds in the instance; the
   net-zero conclusion does not.
2. **The cap is inert** (`rewardCap_inert`, `lotteryCandidates_length`,
   `countEligible_lotteryCandidates`, `rewardCap_saturated`). Once
   `ρ · base_fee ≥ block_reward` the cap is the block reward. ρ is the
   truncated count `min(10,000, eligible rows)`, so at saturation the cap is
   the block reward for every reward up to 2,500 BTH.
3. **Profit as a function of the split count** (`splitUnprofitable_iff`,
   `split_never_pays_iff`, `split_never_pays_iff_fixed`, and the two
   reference instances). A stated stationary-economy model, not the node.
4. **The R3 premise** (`r3_no_split_profit`): if every eligible output pays
   at least `p` and each draw's payout satisfies `payout · L ≤ N · p`, no
   holder's expected winnings exceed its outlay, for any split.
5. **Current code violates both R3 premises** (`r3_eligibility_unpriced`,
   `r3_pricing_violated`).

Status: proven (model). The model is tested against the Rust code by the
#1818 DRT legs (`drawing`, `reward_cap`, `count_eligible`,
`lottery_candidates`); no new definition here mirrors Rust code, so no new
leg is needed. Every hash is the `Prims` parameter, so no axiom is used.

Out of scope (sims, #1683 and #1761): variance, timing, profitability above
10,000 eligible outputs (window inclusion by offset and key gaps), the
ideal-roll expectation and modulo bias (#1708 part C2, #1851).
-/
import Botho.Lottery.Drawing

namespace Botho.Lottery

/-! ## Constants -/

/-- The number of consecutive draws an output is eligible in under the
canonical draw configuration: ages `minUtxoAge ..= circulationWindow`,
that is 720..=10,000, so 9,281 draws. -/
def TICKET_LIFE : Nat :=
  DrawConfig.default.circulationWindow - DrawConfig.default.minUtxoAge + 1

theorem TICKET_LIFE_eq : TICKET_LIFE = 9281 := rfl

/-! ## The uniform draw over at most `winnersPerDraw` candidates -/

/-- The candidates of the slots not yet drawn, in order. -/
def unusedCands : List Slot → List Candidate
  | [] => []
  | s :: ss => if s.used then unusedCands ss else s.cand :: unusedCands ss

/-- What a set of owned ids receives from a list of winners. -/
def ownerWinnings (owns : Bytes → Bool) : List Winner → Nat
  | [] => 0
  | w :: ws => (if owns w.utxoId then w.payout else 0) + ownerWinnings owns ws

private theorem remainingTotal_uniform :
    ∀ slots : List Slot, (∀ s ∈ slots, s.weight = 1) →
      remainingTotal slots = (unusedCands slots).length
  | [], _ => rfl
  | s :: ss, h => by
    have hs : s.weight = 1 := h s (List.mem_cons_self ..)
    have ih := remainingTotal_uniform ss (fun t ht => h t (List.mem_cons_of_mem _ ht))
    cases hu : s.used <;> simp [remainingTotal, unusedCands, hu, hs, ih] <;> omega

/-- Under weight 1, a roll inside the remaining range selects an undrawn
candidate, and the undrawn candidates afterwards are the others. -/
private theorem selectAt_uniform (roll : Nat) :
    ∀ (slots : List Slot) (cum : Nat), (∀ s ∈ slots, s.weight = 1) →
      cum ≤ roll → roll < cum + remainingTotal slots →
      ∃ c slots', selectAt roll cum slots = (some c, slots') ∧
        (∀ s ∈ slots', s.weight = 1) ∧
        (unusedCands slots).Perm (c :: unusedCands slots')
  | [], cum, _, h1, h2 => by simp [remainingTotal] at h2; omega
  | s :: ss, cum, hw, h1, h2 => by
    have hs : s.weight = 1 := hw s (List.mem_cons_self ..)
    have hw' : ∀ t ∈ ss, t.weight = 1 := fun t ht => hw t (List.mem_cons_of_mem _ ht)
    cases hu : s.used
    · -- `s` is undrawn: the walk adds its weight 1.
      by_cases hr : roll < cum + 1
      · refine ⟨s.cand, { s with used := true } :: ss, ?_, ?_, ?_⟩
        · simp [selectAt, hu, hs, hr]
        · intro t ht
          simp only [List.mem_cons] at ht
          rcases ht with rfl | ht
          · exact hs
          · exact hw' t ht
        · simp [unusedCands, hu]
      · have h2' : roll < (cum + 1) + remainingTotal ss := by
          simp [remainingTotal, hu, hs] at h2; omega
        obtain ⟨c, ss', he, hw'', hp⟩ :=
          selectAt_uniform roll ss (cum + 1) hw' (by omega) h2'
        refine ⟨c, s :: ss', ?_, ?_, ?_⟩
        · simp [selectAt, hu, hs, hr, he]
        · intro t ht
          simp only [List.mem_cons] at ht
          rcases ht with rfl | ht
          · exact hs
          · exact hw'' t ht
        · simp only [unusedCands, hu]
          exact (List.Perm.cons _ hp).trans (List.Perm.swap _ _ _)
    · -- `s` is drawn: the walk skips it.
      have h2' : roll < cum + remainingTotal ss := by
        simp [remainingTotal, hu] at h2; omega
      obtain ⟨c, ss', he, hw'', hp⟩ := selectAt_uniform roll ss cum hw' h1 h2'
      refine ⟨c, s :: ss', ?_, ?_, ?_⟩
      · simp [selectAt, hu, he]
      · intro t ht
        simp only [List.mem_cons] at ht
        rcases ht with rfl | ht
        · exact hs
        · exact hw'' t ht
      · simp only [unusedCands, hu]
        exact hp

/-- Under weight 1, `n` rolls against `n` undrawn candidates draw each of them
exactly once, whatever the rolls are. -/
private theorem pickWinners_uniform (P : Prims) (seed : Bytes) (q : Nat) :
    ∀ (n i : Nat) (slots : List Slot), (∀ s ∈ slots, s.weight = 1) →
      (unusedCands slots).length = n →
      ((pickWinners P seed q i n slots).map (·.utxoId)).Perm
          ((unusedCands slots).map (·.id)) ∧
        ∀ w ∈ pickWinners P seed q i n slots, w.payout = q
  | 0, i, slots, _, hn => by
    have : unusedCands slots = [] := List.eq_nil_of_length_eq_zero hn
    simp [pickWinners, this]
  | n + 1, i, slots, hw, hn => by
    have hrem : remainingTotal slots = n + 1 := by
      rw [remainingTotal_uniform slots hw, hn]
    have hroll : verifiableRandomU128 P seed i % (n + 1) < 0 + remainingTotal slots := by
      rw [hrem, Nat.zero_add]; exact Nat.mod_lt _ (by omega)
    obtain ⟨c, slots', he, hw', hp⟩ :=
      selectAt_uniform (verifiableRandomU128 P seed i % (n + 1)) slots 0 hw
        (Nat.zero_le _) hroll
    have hn' : (unusedCands slots').length = n := by
      have := hp.length_eq; simp at this; omega
    obtain ⟨ihp, ihq⟩ := pickWinners_uniform P seed q n (i + 1) slots' hw' hn'
    have hunfold : pickWinners P seed q i (n + 1) slots =
        { utxoId := c.id, payout := q } :: pickWinners P seed q (i + 1) n slots' := by
      simp [pickWinners, hrem, he]
    rw [hunfold]
    refine ⟨?_, ?_⟩
    · simp only [List.map_cons]
      exact (List.Perm.cons _ ihp).trans (hp.map (fun x : Candidate => x.id)).symm
    · intro w hwm
      simp only [List.mem_cons] at hwm
      rcases hwm with rfl | hwm
      · rfl
      · exact ihq w hwm

private theorem addToLast_zero : ∀ ws : List Winner, addToLast 0 ws = ws
  | [] => rfl
  | [w] => by simp [addToLast]
  | w :: v :: ws => by
    simp only [addToLast]
    rw [addToLast_zero (v :: ws)]

private theorem totalPaid_const (q : Nat) :
    ∀ ws : List Winner, (∀ w ∈ ws, w.payout = q) → totalPaid ws = q * ws.length
  | [], _ => by simp [totalPaid]
  | w :: ws, h => by
    have hw : w.payout = q := h w (List.mem_cons_self ..)
    have ih := totalPaid_const q ws (fun v hv => h v (List.mem_cons_of_mem _ hv))
    simp [totalPaid, hw, ih, Nat.mul_succ]
    omega

private theorem ownerWinnings_const (owns : Bytes → Bool) (q : Nat) :
    ∀ ws : List Winner, (∀ w ∈ ws, w.payout = q) →
      ownerWinnings owns ws = q * (ws.map (·.utxoId)).countP owns
  | [], _ => by simp [ownerWinnings]
  | w :: ws, h => by
    have hw : w.payout = q := h w (List.mem_cons_self ..)
    have ih := ownerWinnings_const owns q ws (fun v hv => h v (List.mem_cons_of_mem _ hv))
    cases ho : owns w.utxoId <;> simp [ownerWinnings, ho, hw, ih, Nat.mul_succ]
    omega

private theorem unusedCands_fresh (f : Candidate → Nat) :
    ∀ l : List Candidate,
      unusedCands (l.map fun c => ({ cand := c, weight := f c, used := false } : Slot)) = l
  | [] => rfl
  | c :: cs => by simp [unusedCands, unusedCands_fresh f cs]

/-- **Every candidate wins when there are at most `winnersPerDraw` of them**
(uniform mode). For every hash function, every seed and every pool,
`drawEligible` returns winners that are exactly the eligible candidates, each
paid `pool / ρ` before the remainder goes to the last one. The rolls only
choose the order. This is what makes the counterexample below hold for all
`P`. Status: proven (model). -/
theorem drawEligible_uniform_all_win (P : Prims) (eligible : List Candidate)
    (pool blockHeight : Nat) (seed : Bytes) (cfg : DrawConfig)
    (hmode : cfg.mode = .uniform) (hne : eligible ≠ [])
    (hlen : eligible.length ≤ cfg.winnersPerDraw) :
    ∃ ws : List Winner,
      drawEligible P eligible pool blockHeight seed cfg =
        some { blockHeight, poolAmount := pool,
               winners := addToLast (pool - totalPaid ws) ws, seed } ∧
      (ws.map (·.utxoId)).Perm (eligible.map (·.id)) ∧
      ∀ w ∈ ws, w.payout = pool / eligible.length := by
  have hw1 : ∀ c : Candidate, c.weight cfg.mode ((maxValue? eligible).getD 1) = 1 := by
    intro c; rw [hmode]; rfl
  let slots := eligible.map fun c =>
    ({ cand := c, weight := c.weight cfg.mode ((maxValue? eligible).getD 1), used := false } : Slot)
  have hslots : ∀ s ∈ slots, s.weight = 1 := by
    intro s hs
    simp only [slots, List.mem_map] at hs
    obtain ⟨c, _, rfl⟩ := hs
    exact hw1 c
  have hun : unusedCands slots = eligible := unusedCands_fresh _ eligible
  have hrem : remainingTotal slots = eligible.length := by
    rw [remainingTotal_uniform slots hslots, hun]
  have hpos : eligible.length ≠ 0 := by
    intro h0; exact hne (List.eq_nil_of_length_eq_zero h0)
  have hnum : minN cfg.winnersPerDraw eligible.length = eligible.length := by
    unfold minN; split <;> omega
  obtain ⟨hp, hq⟩ := pickWinners_uniform P seed (pool / eligible.length)
    eligible.length 0 slots hslots (by rw [hun])
  refine ⟨pickWinners P seed (pool / eligible.length) 0 eligible.length slots, ?_, ?_, hq⟩
  · simp only [drawEligible]
    rw [ite_eq_right (by rw [hrem]; exact hpos), hnum]
  · rw [hun] at hp; exact hp

/-! ## CE-1: the deterministic counterexample -/

/-- A 36-byte UTXO id whose first byte is `k`. -/
def ceId (k : UInt8) : Bytes := k :: zeros 35

/-- A row at the dust floor (`min_utxo_value` = 1e6 pico), created at
height 0, factor 1×. -/
def ceRow (k : UInt8) : Utxo :=
  { id := ceId k, amount := 1000000, createdAt := 0, clusterFactor := 1000 }

/-- The UTXO table: four rows, in key order. -/
def ceRows : List Utxo := [ceRow 0, ceRow 1, ceRow 2, ceRow 3]

/-- The splitter owns three of the four rows. -/
def ceOwner (id : Bytes) : Bool :=
  decide (id = ceId 1) || decide (id = ceId 2) || decide (id = ceId 3)

/-- The parent header of the drawn block, at height `h - 1`. -/
def ceTip (h : Nat) : Header :=
  { version := 0, prevBlockHash := zeros 32, txRoot := zeros 32, timestamp := 0,
    height := h - 1, difficulty := 0, nonce := 0, minterViewKey := zeros 32,
    minterSpendKey := zeros 32 }

/-- The pinned pre-state: an empty stored pool, the tip, the four rows. -/
def ceReads (h : Nat) : Reads := { lotteryPool := 0, tip := ceTip h, rows := ceRows }

/-- The drawn block at height `h`, on the tip's hash, epoch-1 shaped: block
reward 25 BTH, lottery emission share 2.5 BTH, no transactions. -/
def ceBlock (P : Prims) (h : Nat) : DrawBlock :=
  { height := h, prevBlockHash := headerHash P (ceTip h), fees := [],
    reward := 25000000000000, emissionShare := 2500000000000 }

private theorem scan_all (cap blockHeight : Nat) (cfg : DrawConfig) :
    ∀ (l : List Utxo) (taken : Nat),
      (∀ u ∈ l, u.toCandidate.isEligible blockHeight cfg = true) →
      taken + l.length ≤ cap →
      scan cap blockHeight cfg l taken = l.map Utxo.toCandidate
  | [], _, _, _ => rfl
  | u :: us, taken, he, hl => by
    have hu := he u (List.mem_cons_self ..)
    have ih := scan_all cap blockHeight cfg us (taken + 1)
      (fun v hv => he v (List.mem_cons_of_mem _ hv)) (by simp at hl; omega)
    have hlt : ¬ cap ≤ taken := by simp at hl; omega
    simp [scan, hlt, hu, ih]

/-- `rotate` reorders the rows. -/
theorem rotate_perm (offset : Bytes) (rows : List Utxo) :
    (rotate offset rows).Perm rows := by
  unfold rotate
  have h := List.filter_append_perm (fun u : Utxo => bytesLt u.id offset) rows
  exact List.perm_append_comm.trans h

private theorem countEligible_eq_countP (blockHeight : Nat) (cfg : DrawConfig) :
    ∀ l : List Candidate,
      countEligible l blockHeight cfg = l.countP (·.isEligible blockHeight cfg)
  | [] => rfl
  | c :: cs => by
    rw [countEligible, countEligible_eq_countP blockHeight cfg cs, List.countP_cons]
    cases c.isEligible blockHeight cfg <;> simp <;> omega

private theorem ce_candidates (P : Prims) (h : Nat) (s : Bytes)
    (hlo : 720 ≤ h) (hhi : h ≤ 10000) :
    ∃ cands, lotteryCandidates P ceRows h s canonicalConfig.draw = cands ∧
      cands.Perm (ceRows.map Utxo.toCandidate) ∧
      ∀ c ∈ cands, c.isEligible h canonicalConfig.draw = true := by
  have helig : ∀ u ∈ rotate (offsetKey P s h) ceRows,
      u.toCandidate.isEligible h canonicalConfig.draw = true := by
    intro u hu
    have hu' := (rotate_perm _ ceRows).subset hu
    simp only [ceRows, List.mem_cons, List.not_mem_nil, or_false] at hu'
    rcases hu' with rfl | rfl | rfl | rfl <;>
      simp [Utxo.toCandidate, Candidate.isEligible, ceRow, canonicalConfig,
        DrawConfig.default, CIRCULATION_WINDOW_BLOCKS, satSub] <;> omega
  have hlen : 0 + (rotate (offsetKey P s h) ceRows).length ≤ MAX_LOTTERY_CANDIDATES := by
    rw [(rotate_perm _ ceRows).length_eq]; decide
  refine ⟨_, rfl, ?_, ?_⟩
  · unfold lotteryCandidates
    rw [scan_all _ _ _ _ 0 helig hlen]
    exact (rotate_perm _ ceRows).map _
  · unfold lotteryCandidates
    rw [scan_all _ _ _ _ 0 helig hlen]
    intro c hc
    simp only [List.mem_map] at hc
    obtain ⟨u, hu, rfl⟩ := hc
    exact helig u hu

/-- **CE-1, exact.** In the four-row pre-state `ceReads h`, at every height
`h` of the rows' eligibility window (720..=10,000, which is `TICKET_LIFE` =
9,281 draws) and for every hash function `P`, `drawing` succeeds and the
owner of three rows receives exactly `3 · LOTTERY_BASE_FEE_PICO` = 7.5e11
pico.

Parameters: ρ = 4 eligible outputs (below `MAX_LOTTERY_CANDIDATES` =
10,000, so no truncation), `LOTTERY_BASE_FEE_PICO` = 2.5e11, cap
`min(4 · 2.5e11, 25e12)` = 1e12, payout `min(2.5e12, 1e12)` = 1e12, four
winners of 2.5e11 each. The per-draw equality `k · base_fee` of the design
holds here. Status: proven (model). -/
theorem ce_owner_wins_three_base_fees (P : Prims) (h : Nat)
    (hlo : 720 ≤ h) (hhi : h ≤ 10000) :
    ∃ res acc, drawing P (ceReads h) (ceBlock P h) = .ok (res, acc) ∧
      ownerWinnings ceOwner res.winners = 3 * LOTTERY_BASE_FEE_PICO := by
  -- The seed check passes: the block sits at `tip.height + 1` on `tip.hash()`.
  have hseed : seed P (ceReads h) (ceBlock P h) =
      .ok (lotterySeedV2 P (P.powHash (ceTip h)) (ceTip h).prevBlockHash h) := by
    have hadd : checkedAdd U64_MAX (ceTip h).height 1 = some h := by
      unfold checkedAdd
      split
      · simp only [ceTip] at *; congr 1; omega
      · simp only [ceTip, U64_MAX] at *; omega
    simp [seed, ceReads, ceBlock, hadd]
  generalize hs : lotterySeedV2 P (P.powHash (ceTip h)) (ceTip h).prevBlockHash h = s at hseed
  obtain ⟨cands, hc, hperm, helig⟩ := ce_candidates P h s hlo hhi
  have hlen : cands.length = 4 := by rw [hperm.length_eq]; rfl
  have hcount : countEligible cands h canonicalConfig.draw = 4 := by
    rw [countEligible_eq_countP, List.countP_eq_length.mpr helig, hlen]
  have hcap : rewardCap cands h 25000000000000 canonicalConfig = 1000000000000 := by
    simp only [rewardCap]; rw [hcount]; decide
  have hacc : computePoolAccounting 0 2500000000000 0 1000000000000 canonicalConfig =
      { feePool := 0, feeBurn := 0, emissionShare := 2500000000000,
        available := 2500000000000, payout := 1000000000000 } := by decide
  have hfilt : cands.filter (fun c => c.isEligible h canonicalConfig.draw) = cands :=
    List.filter_eq_self.mpr helig
  have hne : cands ≠ [] := by intro h0; rw [h0] at hlen; simp at hlen
  obtain ⟨ws, hdraw, hwp, hwq⟩ := drawEligible_uniform_all_win P cands 1000000000000 h s
    canonicalConfig.draw rfl hne (by rw [hlen]; decide)
  rw [hlen] at hwq
  have hwlen : ws.length = 4 := by
    have := hwp.length_eq; simp at this; omega
  have htot : totalPaid ws = 1000000000000 := by
    rw [totalPaid_const _ ws hwq, hwlen]
  have hext : (1000000000000 : Nat) - totalPaid ws = 0 := by rw [htot]
  have hwin : drawWinnersWithSeed P cands 1000000000000 h s canonicalConfig.draw =
      some { blockHeight := h, poolAmount := 1000000000000, winners := ws, seed := s } := by
    simp only [drawWinnersWithSeed, hfilt]
    rw [ite_eq_right (by simp [hne])]
    rw [hdraw, hext, addToLast_zero]
  have hwsne : ws ≠ [] := by intro h0; rw [h0] at hwlen; simp at hwlen
  refine ⟨{ blockHeight := h, totalFees := 0, poolAmount := 1000000000000,
            burnAmount := 0, winners := ws, seed := s },
          { feePool := 0, feeBurn := 0, emissionShare := 2500000000000,
            available := 2500000000000, payout := 1000000000000 }, ?_, ?_⟩
  · have hrows : (ceReads h).rows = ceRows := rfl
    simp only [drawing, hseed, hrows]
    simp only [ceBlock, ceReads, sumFeesChecked]
    simp only [hc, hcap, hacc]
    simp only [drawLotteryWinnersSeeded, hwin]
    simp [hwsne, CANDIDATE_MAX_AWARDS, canonicalConfig, DrawConfig.default]
  · show ownerWinnings ceOwner ws = 3 * LOTTERY_BASE_FEE_PICO
    rw [ownerWinnings_const _ _ ws hwq, hwp.countP_eq]
    have hids : (ceRows.map Utxo.toCandidate).map (·.id) = [ceId 0, ceId 1, ceId 2, ceId 3] := rfl
    rw [(hperm.map (·.id)).countP_eq, hids]
    decide

/-- **CE-1: the net-zero claim is false.** `sybil_reward_cap`'s doc comment
and research §9.1 concluded that a splitter "wins back exactly the base fee
it pays: splitting is net-zero". In the four-row instance above, if each of
the owner's three tickets cost `c` once, with `c < LOTTERY_BASE_FEE_PICO`,
then after a **single** draw (at any of the 9,281 heights the rows are
eligible in, for every hash function) the owner's winnings exceed its
three-ticket outlay.

`hfee` is the per-ticket cost consensus charges. It is a hypothesis because
the consensus fee floor is not modelled (#1684): no consensus rule charges
`LOTTERY_BASE_FEE_PICO`. Measured per-ticket costs are 870 to 117,032 pico
under the consensus floor and 6.25e6 pico with the mempool's `MIN_TX_FEE`
(research §9.1.1), all far below 2.5e11. Parameters: ρ = 4,
`LOTTERY_BASE_FEE_PICO` = 2.5e11, `MAX_LOTTERY_CANDIDATES` = 10,000 (not
reached), cap 1e12, payout 1e12. Status: proven (model). -/
theorem sybil_net_zero_refuted (P : Prims) (h c : Nat)
    (hlo : 720 ≤ h) (hhi : h ≤ 10000) (hfee : c < LOTTERY_BASE_FEE_PICO) :
    ∃ res acc, drawing P (ceReads h) (ceBlock P h) = .ok (res, acc) ∧
      3 * c < ownerWinnings ceOwner res.winners := by
  obtain ⟨res, acc, hd, hw⟩ := ce_owner_wins_three_base_fees P h hlo hhi
  exact ⟨res, acc, hd, by rw [hw]; omega⟩

/-! ## ST-2: the Sybil cap is inert -/

/-- **The cap is inert once `ρ · base_fee ≥ block_reward`.** Then
`rewardCap` is the block reward. With `LOTTERY_BASE_FEE_PICO` = 0.25 BTH that
is ρ ≥ 100 at a 25 BTH reward and ρ ≥ 200 at 50 BTH. `hreward` is the Rust
width (`block_reward` is a `u64`, so this is weaker). Status: proven
(model). -/
theorem rewardCap_inert (cands : List Candidate) (blockHeight reward : Nat)
    (cfg : FeeConfig) (hreward : reward ≤ U128_MAX)
    (hcap : reward ≤ countEligible cands blockHeight cfg.draw * LOTTERY_BASE_FEE_PICO) :
    rewardCap cands blockHeight reward cfg = reward := by
  simp only [rewardCap, sybilRewardCap, satMul, minN]
  split
  · split <;> omega
  · split <;> omega

private theorem scan_length (cap blockHeight : Nat) (cfg : DrawConfig) :
    ∀ (l : List Utxo) (taken : Nat),
      (scan cap blockHeight cfg l taken).length =
        min (cap - taken) (l.countP (·.toCandidate.isEligible blockHeight cfg))
  | [], _ => by simp [scan]
  | u :: us, taken => by
    have ih := scan_length cap blockHeight cfg us (taken + 1)
    have ih0 := scan_length cap blockHeight cfg us taken
    by_cases hc : cap ≤ taken
    · simp [scan, hc]
    · cases he : u.toCandidate.isEligible blockHeight cfg
      · simp [scan, hc, he, ih0]
      · simp [scan, hc, he, ih]; omega

private theorem scan_eligible (cap blockHeight : Nat) (cfg : DrawConfig) :
    ∀ (l : List Utxo) (taken : Nat),
      ∀ c ∈ scan cap blockHeight cfg l taken, c.isEligible blockHeight cfg = true
  | [], _ => by simp [scan]
  | u :: us, taken => by
    intro c hc
    by_cases hcap : cap ≤ taken
    · simp [scan, hcap] at hc
    · cases he : u.toCandidate.isEligible blockHeight cfg
      · simp only [scan, hcap, he] at hc
        exact scan_eligible cap blockHeight cfg us taken c (by simpa using hc)
      · simp only [scan, hcap, he] at hc
        simp only [ite_false, ite_true, List.mem_cons] at hc
        rcases hc with rfl | hc
        · exact he
        · exact scan_eligible cap blockHeight cfg us (taken + 1) c hc

/-- **ρ is truncated at 10,000.** The candidate read returns
`min(MAX_LOTTERY_CANDIDATES, number of eligible rows)` candidates, whatever
the hash and the seed. So the ρ the cap sees is not the untruncated eligible
count once more than 10,000 outputs are eligible. Status: proven (model). -/
theorem lotteryCandidates_length (P : Prims) (rows : List Utxo) (blockHeight : Nat)
    (s : Bytes) (cfg : DrawConfig) :
    (lotteryCandidates P rows blockHeight s cfg).length =
      min MAX_LOTTERY_CANDIDATES (rows.countP (·.toCandidate.isEligible blockHeight cfg)) := by
  unfold lotteryCandidates
  rw [scan_length, (rotate_perm _ rows).countP_eq]
  rfl

/-- **`count_eligible` over the candidate read counts every candidate**: the
read already filters by the same eligibility test. So ρ is the length of
the candidate list. Status: proven (model). -/
theorem countEligible_lotteryCandidates (P : Prims) (rows : List Utxo)
    (blockHeight : Nat) (s : Bytes) (cfg : DrawConfig) :
    countEligible (lotteryCandidates P rows blockHeight s cfg) blockHeight cfg =
      (lotteryCandidates P rows blockHeight s cfg).length := by
  rw [countEligible_eq_countP, List.countP_eq_length]
  exact scan_eligible _ _ _ _ _

/-- **At saturation the cap is the block reward.** With at least 10,000
eligible rows, ρ = 10,000, `sybilRewardCap ρ` = 2,500 BTH, and `rewardCap`
is the block reward for every reward up to 2,500 BTH, which covers every
block reward of the emission schedule (at most 50 BTH; the schedule is an
input of this model, #1710). So `sybil_reward_cap` never binds at
production parameters. Status: proven (model). -/
theorem rewardCap_saturated (P : Prims) (rows : List Utxo) (blockHeight reward : Nat)
    (s : Bytes) (cfg : FeeConfig)
    (hsat : MAX_LOTTERY_CANDIDATES ≤ rows.countP (·.toCandidate.isEligible blockHeight cfg.draw))
    (hreward : reward ≤ MAX_LOTTERY_CANDIDATES * LOTTERY_BASE_FEE_PICO) :
    rewardCap (lotteryCandidates P rows blockHeight s cfg.draw) blockHeight reward cfg = reward := by
  apply rewardCap_inert
  · simp [MAX_LOTTERY_CANDIDATES, LOTTERY_BASE_FEE_PICO, U128_MAX] at hreward ⊢; omega
  · rw [countEligible_lotteryCandidates, lotteryCandidates_length]
    have : min MAX_LOTTERY_CANDIDATES
        (rows.countP (·.toCandidate.isEligible blockHeight cfg.draw)) = MAX_LOTTERY_CANDIDATES :=
      Nat.min_eq_left hsat
    rw [this]; exact hreward

/-! ## ST-3: profit as a function of the split count (stationary economy)

This section is about a **stated economy model**, not about the node. It is
the closed form of the #1761 sim (`cluster-tax/src/simulation/
lottery_reward_cap_faithful.rs`, `evaluate` and `break_even_cost`):

- every block, the organic economy creates `ρ_o` eligible outputs, the
  coinbase adds one free ticket, and the splitter creates `k` tickets at a
  cost of `c` pico each;
- every output lives the same number of draws, so the splitter's stationary
  share of the eligible set is `k / (ρ_o + k + 1)`, and under a uniform draw
  its expected winnings per block are that share of the payout `R`
  (#1708 part C2 proves "expected share = ticket share" for the model's
  draw);
- the payout is `R(k) = R0 + pm/1000 · k · c`: the payout with no splitter
  plus the share `pm` (per mille) of the splitter's own fees that return to
  the pool. `pm = 800` is the canonical fee split while the payout is below
  the cap (no backlog); `pm = 0` is the regime where the payout does not
  move with `k` (the block-reward clamp binds, as in a backlog drain).

Division is avoided by cross-multiplying. -/

/-- The splitter's expected winnings per block, `k · R(k) / (ρ_o + k + 1)`,
are at most its ticket cost per block, `k · c`, with `R(k) = R0 + pm · k ·
c / 1000`. Both sides are multiplied by `1000 · (ρ_o + k + 1)`. -/
def splitUnprofitable (R0 c ρo pm k : Nat) : Prop :=
  k * (1000 * R0 + pm * k * c) ≤ 1000 * (k * c) * (ρo + k + 1)

private theorem split_core (R0 c ρo pm q k : Nat) :
    (pm + q) * R0 + pm * k * c ≤ (pm + q) * c * (ρo + k + 1) ↔
      (pm + q) * R0 ≤ c * ((pm + q) * (ρo + 1) + q * k) := by
  have e1 : (pm + q) * c * (ρo + k + 1) =
      c * ((pm + q) * (ρo + 1) + q * k) + pm * k * c := by
    simp only [Nat.mul_add, Nat.mul_one, Nat.mul_comm,
      Nat.mul_left_comm]
    omega
  rw [e1]
  omega

/-- **Net ≤ 0 at split count `k`** exactly when
`1000 · R0 ≤ c · (1000 · (ρ_o + 1) + (1000 − pm) · k)`. Status: proven
(economy model). -/
theorem splitUnprofitable_iff (R0 c ρo pm k : Nat) (hk : 0 < k) (hpm : pm ≤ 1000) :
    splitUnprofitable R0 c ρo pm k ↔
      1000 * R0 ≤ c * (1000 * (ρo + 1) + (1000 - pm) * k) := by
  obtain ⟨q, hq⟩ : ∃ q, 1000 = pm + q := ⟨1000 - pm, by omega⟩
  have hsub : 1000 - pm = q := by omega
  unfold splitUnprofitable
  rw [hsub, hq]
  have e2 : (pm + q) * (k * c) * (ρo + k + 1) = k * ((pm + q) * c * (ρo + k + 1)) := by
    simp only [Nat.mul_assoc, Nat.mul_comm, Nat.mul_left_comm]
  rw [e2, Nat.mul_le_mul_left_iff hk]
  exact split_core R0 c ρo pm q k

/-- **Splitting never pays, for any split count, iff it does not pay at
`k = 1`**: `1000 · R0 ≤ c · (1000 · (ρ_o + 1) + 1000 − pm)`, that is the
per-ticket cost is at least `c* = 1000 · R0 / (1000 · (ρ_o + 2) − pm)`.
At `pm = 800` this is `R0 / (ρ_o + 1.2)`. Status: proven (economy
model). -/
theorem split_never_pays_iff (R0 c ρo pm : Nat) (hpm : pm ≤ 1000) :
    (∀ k, 0 < k → splitUnprofitable R0 c ρo pm k) ↔
      1000 * R0 ≤ c * (1000 * (ρo + 1) + (1000 - pm)) := by
  constructor
  · intro h
    have := (splitUnprofitable_iff R0 c ρo pm 1 (by omega) hpm).mp (h 1 (by omega))
    simpa using this
  · intro h k hk
    rw [splitUnprofitable_iff R0 c ρo pm k hk hpm]
    refine Nat.le_trans h (Nat.mul_le_mul_left c ?_)
    have : (1000 - pm) * 1 ≤ (1000 - pm) * k := Nat.mul_le_mul_left _ hk
    omega

/-- **The fixed-payout form**: when the payout does not move with `k`
(`pm = 0`), splitting never pays iff `R ≤ c · (ρ_o + 2)`, that is
`c ≥ c* = R / (ρ_o + 2)`. This is the closed form the sim's
`break_even_matches_closed_form` test checks. Status: proven (economy
model). -/
theorem split_never_pays_iff_fixed (R c ρo : Nat) :
    (∀ k, 0 < k → splitUnprofitable R c ρo 0 k) ↔ R ≤ c * (ρo + 2) := by
  rw [split_never_pays_iff R c ρo 0 (by omega)]
  have : c * (1000 * (ρo + 1) + (1000 - 0)) = 1000 * (c * (ρo + 2)) := by
    simp only [Nat.sub_zero, Nat.mul_add, Nat.add_mul, Nat.mul_assoc,
      Nat.mul_comm]
    omega
  rw [this]
  omega

/-- **Reference point, no backlog** (research §9.1.1, epoch 1, ρ_o = 20,
`R0` = 2.5 BTH emission share + 0.8 · 1e9 pico organic fees = 2.5008 BTH,
80% fee recycling): splitting never pays iff a ticket costs at least
117,962,264,151 pico, that is `c*` = 0.118 BTH. The measured cost with
`MIN_TX_FEE` is 6.25e6 pico, about 19,000× below. Status: proven (economy
model). -/
theorem break_even_reference (c : Nat) :
    (∀ k, 0 < k → splitUnprofitable 2500800000000 c 20 800 k) ↔ 117962264151 ≤ c := by
  rw [split_never_pays_iff _ _ _ _ (by omega)]
  omega

/-- **Reference point, backlog drain** (epoch 1, ρ_o = 20, payout clamped
at the 25 BTH block reward, so `pm = 0`): splitting never pays iff a ticket
costs at least 1,136,363,636,364 pico, `c*` = 25/22 BTH ≈ 1.136 BTH.
Status: proven (economy model). -/
theorem break_even_backlog_reference (c : Nat) :
    (∀ k, 0 < k → splitUnprofitable 25000000000000 c 20 0 k) ↔ 1136363636364 ≤ c := by
  rw [split_never_pays_iff_fixed]
  omega

/-- **At the reference point the split pays**: with the `MIN_TX_FEE`
per-ticket cost (6.25e6 pico at 16 outputs per transaction) and the
proposer-slot split k* = 1,584 tickets per block, the splitter's expected
winnings exceed its cost. Status: proven (economy model). -/
theorem split_pays_at_reference : ¬ splitUnprofitable 2500800000000 6250000 20 800 1584 := by
  unfold splitUnprofitable
  decide

/-! ## ST-4: the R3 premise (#1837 option 1)

"Price tickets at their expected payout": if (i) every eligible output,
coinbase and payout outputs included, is charged at least `p`, and (ii) each
draw's payout is at most `N · p / L` for the untruncated eligible count `N`
and the ticket life `L`, then no holder can profit from splitting, for any
number of tickets. -/

/-- One draw in the life of a holder's tickets. -/
structure DrawObs where
  /-- How many of the holder's tickets are eligible in this draw. -/
  held : Nat
  /-- The untruncated number of eligible outputs. -/
  n : Nat
  /-- The draw's payout (`Accounting.payout`). -/
  payout : Nat
  /-- The holder's expected winnings in this draw, in units of `1/D` pico
  (`D` is the common denominator of the theorem below). -/
  win : Nat

/-- **The R3 premise.** Let a holder own `k` tickets, each eligible in at
most `L` draws (`hlife`), and let `ds` be the draws its tickets take part
in. Suppose:

- `hshare`: in each draw its expected winnings are at most its ticket share
  of the payout, `win / D ≤ held · payout / n`. This is the uniform-draw
  expectation, which #1708 part C2 proves for the model's draw when
  `n ≤ MAX_LOTTERY_CANDIDATES` (so the window is the whole eligible set);
  until then it is an assumption here;
- `hprice` (R3 condition ii): each draw pays `payout · L ≤ n · p`, with `n`
  the untruncated eligible count;
- `hfee` (R3 condition i): the holder paid at least `p` per ticket,
  `k · p ≤ outlay`. Consensus charges no such price today (#1684); the
  theorem keeps it as an explicit hypothesis.

Then the holder's expected winnings over all its tickets' lives are at most
its outlay: `Σ win ≤ outlay · D`. Status: proven (model of the premise). -/
theorem r3_no_split_profit (L p k outlay D : Nat) (ds : List DrawObs) (hL : 0 < L)
    (hshare : ∀ d ∈ ds, d.win * d.n ≤ d.held * d.payout * D)
    (hprice : ∀ d ∈ ds, 0 < d.n ∧ d.payout * L ≤ d.n * p)
    (hlife : sumN (ds.map DrawObs.held) ≤ k * L)
    (hfee : k * p ≤ outlay) :
    sumN (ds.map DrawObs.win) ≤ outlay * D := by
  -- Per draw: `win · L ≤ held · p · D`.
  have per : ∀ d ∈ ds, d.win * L ≤ d.held * (p * D) := by
    intro d hd
    obtain ⟨hn, hpr⟩ := hprice d hd
    have h1 : d.win * d.n * L ≤ d.held * d.payout * D * L :=
      Nat.mul_le_mul_right L (hshare d hd)
    have h2 : d.held * d.payout * D * L = d.held * D * (d.payout * L) := by
      simp only [Nat.mul_comm, Nat.mul_left_comm]
    have h3 : d.held * D * (d.payout * L) ≤ d.held * D * (d.n * p) :=
      Nat.mul_le_mul_left _ hpr
    have h4 : d.win * d.n * L = (d.win * L) * d.n := by
      simp only [Nat.mul_comm, Nat.mul_left_comm]
    have h5 : d.held * D * (d.n * p) = (d.held * (p * D)) * d.n := by
      simp only [Nat.mul_comm, Nat.mul_left_comm]
    have : (d.win * L) * d.n ≤ (d.held * (p * D)) * d.n := by
      rw [← h4, ← h5]; exact Nat.le_trans h1 (h2 ▸ h3)
    exact Nat.le_of_mul_le_mul_right this hn
  -- Summed: `L · Σ win ≤ p · D · Σ held`.
  have sum : ∀ es : List DrawObs, (∀ d ∈ es, d.win * L ≤ d.held * (p * D)) →
      sumN (es.map DrawObs.win) * L ≤ sumN (es.map DrawObs.held) * (p * D) := by
    intro es
    induction es with
    | nil => intro _; simp [sumN]
    | cons e es ih =>
      intro hes
      have he := hes e (List.mem_cons_self ..)
      have ht := ih (fun d hd => hes d (List.mem_cons_of_mem _ hd))
      simp only [List.map_cons, sumN, Nat.add_mul]
      omega
  have hs := sum ds per
  have hk : sumN (ds.map DrawObs.held) * (p * D) ≤ k * L * (p * D) :=
    Nat.mul_le_mul_right _ hlife
  have hk' : k * L * (p * D) = (k * p * D) * L := by
    simp only [Nat.mul_comm, Nat.mul_left_comm]
  have : sumN (ds.map DrawObs.win) * L ≤ (k * p * D) * L := by
    rw [← hk']; exact Nat.le_trans hs hk
  have hw := Nat.le_of_mul_le_mul_right this hL
  exact Nat.le_trans hw (Nat.mul_le_mul_right D hfee)

/-! ## Current code violates both R3 premises -/

/-- **R3 condition (i) fails: eligibility is unpriced.** A row's
eligibility under the canonical configuration is decided by its age and
value alone: any row aged 720..=10,000 blocks with at least 1e6 pico is a
ticket, whatever it paid. `Utxo` carries no fee, and neither does the
consensus read (`lottery_candidates`): the coinbase and the four payout
outputs of every block (#1690), and every split output at the 870-pico
consensus floor, are tickets. So the `hfee` premise of
`r3_no_split_profit` cannot be discharged from consensus rules today
(#1684). Status: proven (model). -/
theorem r3_eligibility_unpriced (u : Utxo) (h : Nat)
    (hage : 720 ≤ h - u.createdAt) (hwin : h - u.createdAt ≤ 10000)
    (hval : 1000000 ≤ u.amount) :
    u.toCandidate.isEligible h canonicalConfig.draw = true := by
  have hs : satSub h u.createdAt = h - u.createdAt := by unfold satSub; split <;> omega
  simp only [Utxo.toCandidate, Candidate.isEligible, canonicalConfig, DrawConfig.default,
    CIRCULATION_WINDOW_BLOCKS, hs]
  simp only [Bool.and_eq_true, decide_eq_true_eq]
  exact ⟨⟨hage, hwin⟩, decide_eq_true hval⟩

/-- **R3 condition (ii) fails at production parameters.** Take a saturated
UTXO set (at least 10,000 eligible rows), the epoch-1 block reward of 25 BTH
and the epoch-1 lottery emission share of 2.5 BTH, with any stored pool and
fees. The code's cap is then the block reward (`rewardCap_saturated`), so
the payout is at least the 2.5 BTH emission share, and for **every**
per-ticket price `p ≤ LOTTERY_BASE_FEE_PICO`:

`payout · TICKET_LIFE > ρ · p`,

where ρ = 10,000 is the truncated count the cap uses. R3 needs
`payout · L ≤ N · p`, which here would take `p ≥ 2.32 BTH`. The cap is
`ρ · base_fee` with no division by the ticket life, and ρ is truncated at
`MAX_LOTTERY_CANDIDATES` (`lotteryCandidates_length`). Status: proven
(model). -/
theorem r3_pricing_violated (P : Prims) (rows : List Utxo) (h : Nat) (s : Bytes)
    (fees stored p : Nat)
    (hsat : MAX_LOTTERY_CANDIDATES ≤
      rows.countP (·.toCandidate.isEligible h canonicalConfig.draw))
    (hp : p ≤ LOTTERY_BASE_FEE_PICO) :
    countEligible (lotteryCandidates P rows h s canonicalConfig.draw) h canonicalConfig.draw * p <
      (computePoolAccounting fees 2500000000000 stored
        (rewardCap (lotteryCandidates P rows h s canonicalConfig.draw) h 25000000000000
          canonicalConfig) canonicalConfig).payout * TICKET_LIFE := by
  have hcap := rewardCap_saturated P rows h 25000000000000 s canonicalConfig hsat (by decide)
  have hrho : countEligible (lotteryCandidates P rows h s canonicalConfig.draw) h
      canonicalConfig.draw = MAX_LOTTERY_CANDIDATES := by
    rw [countEligible_lotteryCandidates, lotteryCandidates_length]
    exact Nat.min_eq_left hsat
  rw [hrho, hcap]
  have hpay : 2500000000000 ≤ (computePoolAccounting fees 2500000000000 stored
      25000000000000 canonicalConfig).payout := by
    have hge : ∀ m a b : Nat, a ≤ m → a ≤ satAdd m a b := by
      intro m a b ha; unfold satAdd; split <;> omega
    have hge2 : ∀ m a b : Nat, b ≤ m → b ≤ satAdd m a b := by
      intro m a b hb; unfold satAdd; split <;> omega
    have hle : ∀ m a b : Nat, satAdd m a b ≤ m ∨ a + b ≤ m := by
      intro m a b; unfold satAdd; split <;> omega
    have hmax : (2500000000000 : Nat) ≤ U128_MAX := by decide
    have h1 := hge2 U128_MAX stored 2500000000000 hmax
    have h2 : satAdd U128_MAX stored 2500000000000 ≤ U128_MAX := by
      unfold satAdd; split <;> omega
    have h3 := hge U128_MAX (satAdd U128_MAX stored 2500000000000)
      (splitFees canonicalConfig fees).1 h2
    simp only [computePoolAccounting, minN]
    split <;> omega
  have : MAX_LOTTERY_CANDIDATES * p < 2500000000000 * TICKET_LIFE := by
    simp only [MAX_LOTTERY_CANDIDATES, TICKET_LIFE_eq]
    simp only [LOTTERY_BASE_FEE_PICO] at hp
    omega
  exact Nat.lt_of_lt_of_le this (Nat.mul_le_mul_right _ hpay)

end Botho.Lottery
