import MonetaryKernels
import BothoAeneas.Basic

/-!
# `MonetaryPolicy::halving_reward`, proven about the extraction

Mirrors (by extraction, not by hand): `cluster-tax/src/monetary.rs`,
`MonetaryPolicy::halving_reward` and its callees `is_halving_phase` and
`tail_emission_start_height`, as generated into
`generated/MonetaryKernels.lean` by `regen.sh`.

Label: **proven (extraction)**. These theorems are about the Lean that Aeneas
generated from the Rust source. The extraction keeps Rust's panics: an
overflowing `*`, a division by zero and a shift by 64 or more are all `fail`.
So "returns `ok`" below means the Rust function does not panic. Overflow
checks are on in release builds too (`overflow-checks = true`, #663), so this
is the production behaviour.

Hypotheses used throughout, named after the policy fields:

* `hfit`: `halving_interval * halving_count` fits in `u64`. Without it,
  `tail_emission_start_height` overflows and every call fails (also an
  observation, not a theorem here).
* `hcount`: `halving_count ≤ 64`. The issue (#1821) asked for
  `halving_count < 64`; the theorems hold for the weaker `≤ 64`, which the
  stricter form implies. With `halving_count > 64` and a non-zero interval,
  a phase-1 height reaches a shift by 64, which panics (an observation from
  the code, not a theorem here).
-/

open Aeneas Aeneas.Std Result
open bth_cluster_tax

namespace BothoAeneas.HalvingReward

abbrev Policy := monetary.MonetaryPolicy

theorem tail_emission_start_height_spec (p : Policy)
    (hfit : p.halving_interval.val * p.halving_count.val ≤ U64.max) :
    monetary.MonetaryPolicy.tail_emission_start_height p ⦃ s =>
      s.val = p.halving_interval.val * p.halving_count.val ⦄ := by
  unfold monetary.MonetaryPolicy.tail_emission_start_height
  step*

/-- The whole behaviour of `halving_reward` under the two hypotheses. -/
theorem halving_reward_spec (p : Policy) (height : U64)
    (hfit : p.halving_interval.val * p.halving_count.val ≤ U64.max)
    (hcount : p.halving_count.val ≤ 64) :
    monetary.MonetaryPolicy.halving_reward p height ⦃ r =>
      (height.val < p.halving_interval.val * p.halving_count.val →
        ∃ x, r = some x ∧
          x.val = p.initial_reward.val >>> (height.val / p.halving_interval.val)) ∧
      (p.halving_interval.val * p.halving_count.val ≤ height.val → r = none) ⦄ := by
  unfold monetary.MonetaryPolicy.halving_reward monetary.MonetaryPolicy.is_halving_phase
  step with tail_emission_start_height_spec as ⟨ s, hs ⟩
  split
  · rename_i hlt
    have hI : p.halving_interval.val ≠ 0 := by
      intro h0; simp [h0] at hs; scalar_tac
    have hq : height.val / p.halving_interval.val < 64 := by
      have : height.val < p.halving_interval.val * 64 := by
        have := Nat.mul_le_mul_left p.halving_interval.val hcount
        scalar_tac
      exact Nat.div_lt_of_lt_mul (by simpa [Nat.mul_comm] using this)
    step*
  · rename_i hge
    simp only [WP.spec_ok]
    scalar_tac

/-- `halving_reward` never fails (never panics or diverges) when
`halving_interval * halving_count` fits in `u64` and `halving_count ≤ 64`. -/
theorem halving_reward_no_fail (p : Policy) (height : U64)
    (hfit : p.halving_interval.val * p.halving_count.val ≤ U64.max)
    (hcount : p.halving_count.val ≤ 64) :
    ∃ r, monetary.MonetaryPolicy.halving_reward p height = ok r := by
  obtain ⟨r, hr, -⟩ := eq_ok_of_spec (halving_reward_spec p height hfit hcount)
  exact ⟨r, hr⟩

/-- Phase 1 (`height < halving_interval * halving_count`): the reward is
`initial_reward >>> (height / halving_interval)`. -/
theorem halving_reward_phase1 (p : Policy) (height : U64)
    (hfit : p.halving_interval.val * p.halving_count.val ≤ U64.max)
    (hcount : p.halving_count.val ≤ 64)
    (hphase : height.val < p.halving_interval.val * p.halving_count.val) :
    ∃ x, monetary.MonetaryPolicy.halving_reward p height = ok (some x) ∧
      x.val = p.initial_reward.val >>> (height.val / p.halving_interval.val) := by
  obtain ⟨r, hr, h1, -⟩ := eq_ok_of_spec (halving_reward_spec p height hfit hcount)
  obtain ⟨x, rfl, hx⟩ := h1 hphase
  exact ⟨x, hr, hx⟩

/-- Phase 2 (`height ≥ halving_interval * halving_count`): `none`. -/
theorem halving_reward_phase2 (p : Policy) (height : U64)
    (hfit : p.halving_interval.val * p.halving_count.val ≤ U64.max)
    (hcount : p.halving_count.val ≤ 64)
    (hphase : p.halving_interval.val * p.halving_count.val ≤ height.val) :
    monetary.MonetaryPolicy.halving_reward p height = ok none := by
  obtain ⟨r, hr, -, h2⟩ := eq_ok_of_spec (halving_reward_spec p height hfit hcount)
  rw [hr, h2 hphase]

/-- Monotone non-increasing in height: if a later height still gets a
phase-1 reward, then every earlier height gets a phase-1 reward that is at
least as large. -/
theorem halving_reward_antitone (p : Policy) (h₁ h₂ : U64)
    (hfit : p.halving_interval.val * p.halving_count.val ≤ U64.max)
    (hcount : p.halving_count.val ≤ 64)
    (hle : h₁.val ≤ h₂.val) (r₂ : U64)
    (hr₂ : monetary.MonetaryPolicy.halving_reward p h₂ = ok (some r₂)) :
    ∃ r₁, monetary.MonetaryPolicy.halving_reward p h₁ = ok (some r₁) ∧
      r₂.val ≤ r₁.val := by
  -- `h₂` is in phase 1, otherwise the result would be `none`.
  have hphase₂ : h₂.val < p.halving_interval.val * p.halving_count.val := by
    apply Nat.lt_of_not_le
    intro hge
    rw [halving_reward_phase2 p h₂ hfit hcount hge] at hr₂
    simp at hr₂
  obtain ⟨x₂, hx₂, hv₂⟩ := halving_reward_phase1 p h₂ hfit hcount hphase₂
  obtain ⟨x₁, hx₁, hv₁⟩ :=
    halving_reward_phase1 p h₁ hfit hcount (Nat.lt_of_le_of_lt hle hphase₂)
  refine ⟨x₁, hx₁, ?_⟩
  rw [hr₂] at hx₂
  simp only [ok.injEq, Option.some.injEq] at hx₂
  subst hx₂
  rw [hv₁, hv₂, Nat.shiftRight_eq_div_pow, Nat.shiftRight_eq_div_pow]
  apply Nat.div_le_div_left
  · exact Nat.pow_le_pow_right (by decide) (Nat.div_le_div_right hle)
  · exact Nat.two_pow_pos _

/-- Once the tail phase starts it never ends: `none` at a height means
`none` at every later height. -/
theorem halving_reward_none_stays (p : Policy) (h₁ h₂ : U64)
    (hfit : p.halving_interval.val * p.halving_count.val ≤ U64.max)
    (hcount : p.halving_count.val ≤ 64)
    (hle : h₁.val ≤ h₂.val)
    (hr₁ : monetary.MonetaryPolicy.halving_reward p h₁ = ok none) :
    monetary.MonetaryPolicy.halving_reward p h₂ = ok none := by
  apply halving_reward_phase2 p h₂ hfit hcount
  apply Nat.le_trans _ hle
  apply Nat.le_of_not_lt
  intro hlt
  obtain ⟨x, hx, -⟩ := halving_reward_phase1 p h₁ hfit hcount hlt
  rw [hx] at hr₁
  simp at hr₁

end BothoAeneas.HalvingReward
