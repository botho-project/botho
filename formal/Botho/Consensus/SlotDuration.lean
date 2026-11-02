/-
Dynamic slot duration (#1841, epic #1704).

Mirrors `ConsensusService::current_slot_duration` and
`ConsensusService::record_block` (`botho/src/consensus/service.rs`), with the
tiers of `block::dynamic_timing::BLOCK_TIME_LEVELS` (`botho/src/block.rs`).
This is NOT consensus state: each node computes it from its own window of the
last `SMOOTHING_WINDOW = 10` blocks it recorded (`commands/run.rs` feeds it
`block.header.timestamp` and `block.transactions.len()`), and it only paces
that node's SCP slots. `dynamic_timing::compute_block_time` (no production
caller, fallback 20 s) is NOT mirrored.

The Rust code compares an `f64` rate, `total_txs as f64 / window_time as f64`,
against the thresholds `20.0, 5.0, 1.0, 0.2`. The model compares exact
integers instead (`20 * w ≤ total`, ..., `w ≤ 5 * total`). The two agree
whenever `total_txs` and `window_time` are below `2^53` (so the conversions
are exact) and `window_time < 5 * 10^14`: the f64 quotient is correctly
rounded and rounding is monotone; `20, 5, 1` are exact doubles; and for
`0.2` (the double is slightly above `1/5`) an exact quotient below `1/5` is
at least `1/(5w)` below it, more than the half-ulp that could round it up.
That argument is **trusted** (not formalised: the model has no floats);
timestamps are bounded by the 2 h future rule, so real windows are far
inside it. The vectors in `Botho/Difficulty/Vectors.lean` test the model
against the Rust method (**tested**). `Duration::as_secs` drops sub-second
`config.slot_duration` parts in the fallback paths; the model takes the
configured value in whole seconds.

Labels: theorems here are **proven (model)**.
-/
namespace Botho.Consensus

/-- `RecentBlockInfo` (`consensus/service.rs`). -/
structure RecentBlockInfo where
  timestamp : Nat
  txCount : Nat
deriving DecidableEq, Repr

/-- `dynamic_timing::SMOOTHING_WINDOW`. -/
def SMOOTHING_WINDOW : Nat := 10

/-- `ConsensusService::record_block`: push the block, keep the last 10. -/
def recordBlock (w : List RecentBlockInfo) (b : RecentBlockInfo) : List RecentBlockInfo :=
  (w ++ [b]).drop ((w.length + 1) - SMOOTHING_WINDOW)

/-- Total transactions in the window (`blocks.iter().map(|b| b.tx_count).sum()`). -/
def txTotal : List RecentBlockInfo → Nat
  | [] => 0
  | b :: bs => b.txCount + txTotal bs

def firstTs (w : List RecentBlockInfo) : Nat := (w.head?.map (·.timestamp)).getD 0

def lastTs (w : List RecentBlockInfo) : Nat := (w.getLast?.map (·.timestamp)).getD 0

/-- The `BLOCK_TIME_LEVELS` scan, `tx_rate = total / window` in exact
arithmetic: `>= 20` → 3 s, `>= 5` → 5 s, `>= 1` → 10 s, `>= 0.2` → 20 s,
else 40 s (`MAX_BLOCK_TIME`). -/
def tierOf (total window : Nat) : Nat :=
  if 20 * window ≤ total then 3
  else if 5 * window ≤ total then 5
  else if window ≤ total then 10
  else if window ≤ 5 * total then 20
  else 40

/-- `ConsensusService::current_slot_duration`, in whole seconds. `cfg` is
`config.slot_duration.as_secs()`, `dynamic` is `config.dynamic_timing`. -/
def slotSecs (dynamic : Bool) (cfg : Nat) (w : List RecentBlockInfo) : Nat :=
  if dynamic = false then cfg
  else if w.length < 2 then cfg
  else if lastTs w - firstTs w = 0 then cfg
  else tierOf (txTotal w) (lastTs w - firstTs w)

/-- The tier set. -/
def TIERS : List Nat := [3, 5, 10, 20, 40]

theorem tierOf_mem (total window : Nat) : tierOf total window ∈ TIERS := by
  unfold tierOf TIERS
  split <;> (try split) <;> (try split) <;> (try split) <;> simp

/-- **Slot tiers.** The slot duration is always one of `3, 5, 10, 20, 40` s or
the configured value. Proven (model). -/
theorem slot_in_tiers (dynamic : Bool) (cfg : Nat) (w : List RecentBlockInfo) :
    slotSecs dynamic cfg w ∈ TIERS ∨ slotSecs dynamic cfg w = cfg := by
  unfold slotSecs
  split
  · exact Or.inr rfl
  · split
    · exact Or.inr rfl
    · split
      · exact Or.inr rfl
      · exact Or.inl (tierOf_mem _ _)

/-- The idle tier is chosen exactly when the window carries fewer than one
transaction per 5 s of window time. Proven (model). -/
theorem tierOf_idle_iff (total window : Nat) : tierOf total window = 40 ↔ 5 * total < window := by
  unfold tierOf
  split <;> (try split) <;> (try split) <;> (try split) <;> omega

theorem recordBlock_length (w : List RecentBlockInfo) (b : RecentBlockInfo) :
    (recordBlock w b).length = min (w.length + 1) SMOOTHING_WINDOW := by
  unfold recordBlock SMOOTHING_WINDOW
  simp only [List.length_drop, List.length_append, List.length_singleton]
  omega

theorem recordBlock_lastTs (w : List RecentBlockInfo) (b : RecentBlockInfo) :
    lastTs (recordBlock w b) = b.timestamp := by
  unfold lastTs recordBlock SMOOTHING_WINDOW
  have hk : (w.length + 1) - 10 < (w ++ [b]).length := by simp; omega
  rw [List.getLast?_drop]
  simp only [List.length_append, List.length_singleton]
  have hc : ¬ (w.length + 1 ≤ w.length + 1 - 10) := by omega
  simp only [hc, ↓reduceIte, List.getLast?_append, List.getLast?_singleton, Option.some_or,
    Option.map_some, Option.getD_some]

theorem recordBlock_firstTs_mem {w : List RecentBlockInfo} (hw : w ≠ []) (b : RecentBlockInfo) :
    ∃ x ∈ w, firstTs (recordBlock w b) = x.timestamp := by
  unfold firstTs recordBlock SMOOTHING_WINDOW
  have hlen : 0 < w.length := List.length_pos_iff.mpr hw
  have hk : (w.length + 1) - 10 < w.length := by omega
  rw [List.head?_drop, List.getElem?_append_left hk, List.getElem?_eq_getElem hk]
  exact ⟨_, List.getElem_mem hk, rfl⟩

/-- **Counterexample (a producer can force the idle tier).** Let a node's
window hold at least one block with a timestamp at or before `now`, and let a
producer add a block stamped `now + 2 h` (valid for that node, see
`Botho.Difficulty.forward_date_locks_out_honest`). If the resulting window
carries at most 1439 transactions, the node's slot duration becomes the idle
40 s, whatever the real load. With the default `max_txs_per_slot = 100` a
window of 10 blocks carries at most 1000, so the hypothesis holds for every
window of blocks built under that cap. Proven (model). -/
theorem forward_dated_block_forces_idle (cfg now : Nat) (w : List RecentBlockInfo)
    (b : RecentBlockInfo) (hw : w ≠ []) (hts : ∀ x ∈ w, x.timestamp ≤ now)
    (hb : b.timestamp = now + 7200) (htx : txTotal (recordBlock w b) ≤ 1439) :
    slotSecs true cfg (recordBlock w b) = 40 := by
  obtain ⟨x, hx, hf⟩ := recordBlock_firstTs_mem hw b
  have hxle := hts x hx
  have hlen := recordBlock_length w b
  have hpos : 0 < w.length := List.length_pos_iff.mpr hw
  unfold slotSecs
  rw [recordBlock_lastTs, hf, hb]
  have h2 : ¬ (recordBlock w b).length < 2 := by
    rw [hlen]; unfold SMOOTHING_WINDOW; omega
  simp only [Bool.true_eq_false, ↓reduceIte, h2]
  have hc : ¬ (now + 7200 - x.timestamp = 0) := by omega
  simp only [hc, ↓reduceIte]
  exact (tierOf_idle_iff _ _).mpr (by omega)

end Botho.Consensus
