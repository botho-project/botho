/-
Flat FBAS model and quorum-intersection checker (#1714, epic #1704).

## What this mirrors

- `symmetric_quorum_has_intersection(n, threshold)`,
  `botho/src/commands/run.rs`: the node's quorum-promotion gate check. For
  `n <= MAX_EXACT_SIM_NODES` (13) it runs
  `bth_quorum_sim::Fbas::symmetric(n, t).has_quorum_intersection()`
  (`consensus/quorum-sim/src/{model,analysis}.rs`), above that the analytic
  rule `threshold > n / 2`.
- `gated_scp_quorum_set`, `botho/src/commands/run.rs`: builds a flat
  `t`-of-`n` candidate (Recommended: `QuorumConfig::effective_threshold`;
  Explicit: the operator threshold clamped into `1..=n`), refuses it when the
  check above fails, and at the initial seed falls back to `n / 2 + 1`.
- `config::byzantine_tolerance(n, t)`, `botho/src/config.rs`:
  `min(n - t, 2t - n - 1)`, saturating. Reported as
  `node_getStatus.quorumByzantineTolerance`.
- `QuorumConfig::is_bft_fault_tolerant`, `botho/src/config.rs`: in Recommended
  mode, before #1864, `node_count >= 4`, reported as
  `node_getStatus.quorumFaultTolerant`. #1864 fixed it (#1843) to
  `byzantine_tolerance(n, effective_threshold) >= 1`; the model below keeps
  the pre-#1864 flag as a historical finding.
- `QuorumConfig::effective_threshold`, `botho/src/config.rs`: Crash
  `n / 2 + 1`, Bft `n - (n - 1) / 3`.
- The P4.4 curated path (#709, `evaluate_operator_action` in `run.rs`): one
  signed pin/unpin applied to a config clone and re-run through the same gate.
  The gate only ever sees the local node's own candidate.

## The model

A node set is a `Nat → Bool` predicate. An FBAS (`Fbas`) is a list of nodes
(the universe) and, per node, a *flat* quorum set (`QSet`): a threshold over a
member predicate, the shape `bth_quorum_sim::QuorumSet` and Botho's
`QuorumSet` (no inner sets) have. Counting (`cnt`) is over the universe list.
A quorum (`IsQuorum`) is a non-empty set of nodes each of whose quorum sets it
satisfies, the standard FBAS definition (and `Fbas::is_quorum` in
`quorum-sim`).

Byzantine faults follow Mazieres, "The Stellar Consensus Protocol: A Federated
Model for Internet-level Consensus" (2015), section 4:

- `delete F B` is the paper's `(V, Q)^B`: remove `B` from the universe and
  from every slice. For a flat `t`-of-`M` quorum set the slices are the
  `t`-subsets `q` of `M`, and a set `U` disjoint from `B` contains some
  `q \ B` iff `|M ∩ U| >= t - |M ∩ B|`. So the deleted quorum set is
  `(t - |M ∩ B|)`-of-`(M \ B)` (`delete_satisfies_iff` checks this reading).
- `IntersectsDespite F B` is "quorum intersection despite `B`"
  (`delete F B` enjoys quorum intersection), `AvailableDespite F B` is
  "quorum availability despite `B`" (`V \ B` is a quorum, or `B = V`), and
  `IsDSet F B` is their conjunction. A node outside a DSet that contains every
  ill-behaved node is *intact*: SCP guarantees safety (and liveness) to intact
  nodes. So "the configuration tolerates `f` Byzantine nodes" means: every
  `B` with `|B| = f` is a DSet.

Out of scope, as #1714 says: SCP's federated-voting safety and liveness given
these properties. That is the paper's Theorems 11-12 and the mechanised proofs
of Losa, Gafni and Mazieres (Ivy) and the TLA+ SCP specifications; this module
only decides the FBAS-level preconditions those results assume.

## Status labels (formal/README.md)

Every theorem here is **proven (model)**. The Rust side is tied in by
`#[cfg(test)]` checks in `botho/src/commands/run.rs`, which are **tested**
evidence, not proof:

- exhaustive `1 <= t <= n <= 64`: `symmetric_quorum_has_intersection(n, t) ==
  (2t > n)` (the `sym_intersects_iff_pos` closed form, both Rust branches);
- exhaustive `byzantine_tolerance` against the `byzTol_iff_dset` closed form;
- the concrete heterogeneous instances below (lead B forks, the rotation
  instances, the not-necessary example) against `bth_quorum_sim`'s
  brute-force analyser, with a test-only Mazieres `delete` for the despite-`f`
  ones.

The theorems say nothing about SCP message handling, about how a node's
connected-peer view is formed, or about `QuorumSet` validation; those are
trusted assumptions about code outside the model.
-/
namespace Botho.Quorum

/-! ## Counting -/

/-- A set of nodes, as a predicate. -/
abbrev NodeSet := Nat → Bool

/-- Number of elements of `l` (with multiplicity) that satisfy `P`. -/
def cnt (P : NodeSet) : List Nat → Nat
  | [] => 0
  | v :: l => (if P v = true then 1 else 0) + cnt P l

@[simp] theorem cnt_nil (P : NodeSet) : cnt P [] = 0 := rfl

@[simp] theorem cnt_cons (P : NodeSet) (v : Nat) (l : List Nat) :
    cnt P (v :: l) = (if P v = true then 1 else 0) + cnt P l := rfl

theorem cnt_le_length (P : NodeSet) (l : List Nat) : cnt P l ≤ l.length := by
  induction l with
  | nil => simp
  | cons v l ih => simp only [cnt_cons, List.length_cons]; split <;> omega

theorem cnt_congr {P Q : NodeSet} {l : List Nat}
    (h : ∀ x ∈ l, P x = Q x) : cnt P l = cnt Q l := by
  induction l with
  | nil => rfl
  | cons v l ih =>
    simp only [cnt_cons]
    rw [h v (by simp), ih (fun x hx => h x (by simp [hx]))]

theorem cnt_mono {P Q : NodeSet} {l : List Nat}
    (h : ∀ x ∈ l, P x = true → Q x = true) : cnt P l ≤ cnt Q l := by
  induction l with
  | nil => simp
  | cons v l ih =>
    simp only [cnt_cons]
    have ih' := ih (fun x hx => h x (by simp [hx]))
    have hv := h v (by simp)
    by_cases hp : P v = true
    · simp [hp, hv hp]; omega
    · simp [hp]; split <;> omega

theorem cnt_false (l : List Nat) : cnt (fun _ => false) l = 0 := by
  induction l with
  | nil => rfl
  | cons v l ih => simp [ih]

theorem cnt_true (l : List Nat) : cnt (fun _ => true) l = l.length := by
  induction l with
  | nil => rfl
  | cons v l ih => simp [ih]; omega

/-- Counting over a filtered list is counting the conjunction. -/
theorem cnt_filter (P Q : NodeSet) (l : List Nat) :
    cnt P (l.filter Q) = cnt (fun x => P x && Q x) l := by
  induction l with
  | nil => rfl
  | cons v l ih =>
    by_cases hq : Q v = true
    · simp [hq, ih]
    · simp [hq, ih]

theorem length_filter_eq_cnt (Q : NodeSet) (l : List Nat) :
    (l.filter Q).length = cnt Q l := by
  have h := cnt_filter (fun _ => true) Q l
  simp only [Bool.true_and] at h
  rw [← h, cnt_true]

/-- Inclusion-exclusion for two predicates. -/
theorem cnt_or_add_and (P Q : NodeSet) (l : List Nat) :
    cnt (fun x => P x || Q x) l + cnt (fun x => P x && Q x) l = cnt P l + cnt Q l := by
  induction l with
  | nil => rfl
  | cons v l ih =>
    simp only [cnt_cons]
    cases P v <;> cases Q v <;> simp <;> omega

theorem cnt_or_le (P Q : NodeSet) (l : List Nat) :
    cnt (fun x => P x || Q x) l ≤ cnt P l + cnt Q l := by
  have := cnt_or_add_and P Q l; omega

theorem cnt_le_or_left (P Q : NodeSet) (l : List Nat) :
    cnt P l ≤ cnt (fun x => P x || Q x) l :=
  cnt_mono (fun x _ h => by simp [h])

theorem cnt_le_or_right (P Q : NodeSet) (l : List Nat) :
    cnt Q l ≤ cnt (fun x => P x || Q x) l :=
  cnt_mono (fun x _ h => by simp [h])

/-- `cnt` of `P` and of its complement add up to the length. -/
theorem cnt_add_cnt_not (P : NodeSet) (l : List Nat) :
    cnt P l + cnt (fun x => !P x) l = l.length := by
  induction l with
  | nil => rfl
  | cons v l ih => simp only [cnt_cons, List.length_cons]; cases P v <;> simp <;> omega

theorem cnt_pos_of_mem {P : NodeSet} {l : List Nat} {v : Nat}
    (hv : v ∈ l) (hp : P v = true) : 1 ≤ cnt P l := by
  induction l with
  | nil => simp at hv
  | cons w l ih =>
    simp only [cnt_cons]
    rcases List.mem_cons.mp hv with h | h
    · subst h; simp [hp]
    · have := ih h; omega

theorem exists_mem_of_cnt_pos {P : NodeSet} {l : List Nat}
    (h : 1 ≤ cnt P l) : ∃ v ∈ l, P v = true := by
  induction l with
  | nil => simp at h
  | cons w l ih =>
    simp only [cnt_cons] at h
    by_cases hw : P w = true
    · exact ⟨w, by simp, hw⟩
    · simp [hw] at h
      obtain ⟨v, hv, hp⟩ := ih h
      exact ⟨v, by simp [hv], hp⟩

/-- Two sets disjoint on `l` have at most `|l|` elements together. -/
theorem cnt_add_le_of_disjoint {S T : NodeSet} {l : List Nat}
    (h : ∀ x ∈ l, ¬ (S x = true ∧ T x = true)) :
    cnt S l + cnt T l ≤ l.length := by
  induction l with
  | nil => simp
  | cons v l ih =>
    simp only [cnt_cons, List.length_cons]
    have ih' := ih (fun x hx => h x (by simp [hx]))
    have hv := h v (by simp)
    cases hs : S v <;> cases ht : T v <;> simp_all <;> omega

/-- Picking two disjoint subsets of prescribed sizes inside `P` and `Q`.

On a duplicate-free list, if `a <= |P|`, `b <= |Q|` and `a + b <= |P ∪ Q|`,
there are `S ⊆ P` and `T ⊆ Q` with exactly `a` and `b` elements, disjoint on
the list. This is the witness construction behind every "no" direction. -/
theorem disjoint_pick (P Q : NodeSet) :
    ∀ (l : List Nat), l.Nodup → ∀ (a b : Nat),
      a ≤ cnt P l → b ≤ cnt Q l → a + b ≤ cnt (fun x => P x || Q x) l →
      ∃ S T : NodeSet,
        (∀ x ∈ l, S x = true → P x = true) ∧ (∀ x ∈ l, T x = true → Q x = true) ∧
        (∀ x ∈ l, ¬ (S x = true ∧ T x = true)) ∧ cnt S l = a ∧ cnt T l = b := by
  intro l
  induction l with
  | nil =>
    intro _ a b ha hb _
    simp at ha hb
    exact ⟨fun _ => false, fun _ => false, by simp, by simp, by simp, by simp [ha], by simp [hb]⟩
  | cons v l ih =>
    intro hnd a b ha hb hab
    have hv : v ∉ l := (List.nodup_cons.mp hnd).1
    have hl : l.Nodup := (List.nodup_cons.mp hnd).2
    have hpq := cnt_or_le P Q l
    have hp_le := cnt_le_or_left P Q l
    have hq_le := cnt_le_or_right P Q l
    -- extend a tail witness by a decision `(s, t)` at the head
    have extend : ∀ (s t : Bool) (S' T' : NodeSet),
        (s = true → P v = true) → (t = true → Q v = true) → ¬ (s = true ∧ t = true) →
        (∀ x ∈ l, S' x = true → P x = true) → (∀ x ∈ l, T' x = true → Q x = true) →
        (∀ x ∈ l, ¬ (S' x = true ∧ T' x = true)) →
        cnt S' l + (if s = true then 1 else 0) = a → cnt T' l + (if t = true then 1 else 0) = b →
        ∃ S T : NodeSet,
          (∀ x ∈ v :: l, S x = true → P x = true) ∧ (∀ x ∈ v :: l, T x = true → Q x = true) ∧
          (∀ x ∈ v :: l, ¬ (S x = true ∧ T x = true)) ∧ cnt S (v :: l) = a ∧
          cnt T (v :: l) = b := by
      intro s t S' T' hs ht hst hS' hT' hd' hca hcb
      refine ⟨fun x => if x = v then s else S' x, fun x => if x = v then t else T' x,
        ?_, ?_, ?_, ?_, ?_⟩
      · intro x hx h
        by_cases hxv : x = v
        · subst hxv; simp at h; exact hs h
        · simp [hxv] at h
          exact hS' x ((List.mem_cons.mp hx).resolve_left hxv) h
      · intro x hx h
        by_cases hxv : x = v
        · subst hxv; simp at h; exact ht h
        · simp [hxv] at h
          exact hT' x ((List.mem_cons.mp hx).resolve_left hxv) h
      · intro x hx h
        by_cases hxv : x = v
        · subst hxv; simp at h; exact hst h
        · simp [hxv] at h
          exact hd' x ((List.mem_cons.mp hx).resolve_left hxv) h
      · have : cnt (fun x => if x = v then s else S' x) l = cnt S' l :=
          cnt_congr (fun x hx => by
            have : x ≠ v := fun h => hv (h ▸ hx)
            simp [this])
        simp only [cnt_cons, this, ↓reduceIte]; omega
      · have : cnt (fun x => if x = v then t else T' x) l = cnt T' l :=
          cnt_congr (fun x hx => by
            have : x ≠ v := fun h => hv (h ▸ hx)
            simp [this])
        simp only [cnt_cons, this, ↓reduceIte]; omega
    simp only [cnt_cons] at ha hb hab
    cases hP : P v <;> cases hQ : Q v <;> simp only [hP, hQ, Bool.or_self, Bool.or_true,
      Bool.true_or, Bool.false_eq_true, ↓reduceIte] at ha hb hab
    · -- v in neither
      obtain ⟨S', T', h1, h2, h3, h4, h5⟩ := ih hl a b (by omega) (by omega) (by omega)
      exact extend false false S' T' (by simp) (by simp) (by simp) h1 h2 h3 (by simp [h4])
        (by simp [h5])
    · -- v in Q only
      by_cases hb0 : b = 0
      · obtain ⟨S', T', h1, h2, h3, h4, h5⟩ := ih hl a b (by omega) (by omega) (by omega)
        exact extend false false S' T' (by simp) (by simp) (by simp) h1 h2 h3 (by simp [h4])
          (by simp [h5])
      · obtain ⟨S', T', h1, h2, h3, h4, h5⟩ := ih hl a (b - 1) (by omega) (by omega) (by omega)
        exact extend false true S' T' (by simp) (fun _ => hQ) (by simp) h1 h2 h3 (by simp [h4])
          (by simp [h5]; omega)
    · -- v in P only
      by_cases ha0 : a = 0
      · obtain ⟨S', T', h1, h2, h3, h4, h5⟩ := ih hl a b (by omega) (by omega) (by omega)
        exact extend false false S' T' (by simp) (by simp) (by simp) h1 h2 h3 (by simp [h4])
          (by simp [h5])
      · obtain ⟨S', T', h1, h2, h3, h4, h5⟩ := ih hl (a - 1) b (by omega) (by omega) (by omega)
        exact extend true false S' T' (fun _ => hP) (by simp) (by simp) h1 h2 h3
          (by simp [h4]; omega) (by simp [h5])
    · -- v in both: give it to T if T needs it, else to S if S needs it
      by_cases hbq : cnt Q l < b
      · obtain ⟨S', T', h1, h2, h3, h4, h5⟩ := ih hl a (b - 1) (by omega) (by omega) (by omega)
        exact extend false true S' T' (by simp) (fun _ => hQ) (by simp) h1 h2 h3 (by simp [h4])
          (by simp [h5]; omega)
      · by_cases ha0 : a = 0
        · obtain ⟨S', T', h1, h2, h3, h4, h5⟩ := ih hl a b (by omega) (by omega) (by omega)
          exact extend false false S' T' (by simp) (by simp) (by simp) h1 h2 h3 (by simp [h4])
            (by simp [h5])
        · obtain ⟨S', T', h1, h2, h3, h4, h5⟩ := ih hl (a - 1) b (by omega) (by omega) (by omega)
          exact extend true false S' T' (fun _ => hP) (by simp) (by simp) h1 h2 h3
            (by simp [h4]; omega) (by simp [h5])

/-! ## Flat FBAS -/

/-- A flat quorum set: `threshold` of the nodes in `members`. -/
structure QSet where
  threshold : Nat
  members : NodeSet

/-- A federated Byzantine agreement system with flat quorum sets: a universe
of nodes and each node's quorum set. -/
structure Fbas where
  nodes : List Nat
  qset : Nat → QSet

/-- `S` contains at least `q.threshold` of `q`'s members (counted over the
universe `U`): `QuorumSet::is_satisfied_by` in `quorum-sim`. -/
def Satisfies (U : List Nat) (q : QSet) (S : NodeSet) : Prop :=
  q.threshold ≤ cnt (fun x => q.members x && S x) U

instance (U : List Nat) (q : QSet) (S : NodeSet) : Decidable (Satisfies U q S) :=
  inferInstanceAs (Decidable (_ ≤ _))

/-- A quorum: non-empty (on the universe) and satisfying the quorum set of each
of its members. -/
def IsQuorum (F : Fbas) (S : NodeSet) : Prop :=
  (∃ v ∈ F.nodes, S v = true) ∧ ∀ v ∈ F.nodes, S v = true → Satisfies F.nodes (F.qset v) S

instance (F : Fbas) (S : NodeSet) : Decidable (IsQuorum F S) :=
  inferInstanceAs (Decidable (_ ∧ _))

/-- Quorum intersection: every two quorums share a node. -/
def Intersects (F : Fbas) : Prop :=
  ∀ S T : NodeSet, IsQuorum F S → IsQuorum F T → ∃ v ∈ F.nodes, S v = true ∧ T v = true

/-- Mazieres' `(V, Q)^B` for flat quorum sets: drop `B` from the universe and
from every quorum set; each threshold falls by the number of its members in
`B` (see the module docstring). -/
def delete (F : Fbas) (B : NodeSet) : Fbas where
  nodes := F.nodes.filter (fun x => !B x)
  qset v :=
    { threshold := (F.qset v).threshold - cnt (fun x => (F.qset v).members x && B x) F.nodes
      members := fun x => (F.qset v).members x && !B x }

/-- Quorum intersection despite `B` (Mazieres, Def. "DSet", condition 1). -/
def IntersectsDespite (F : Fbas) (B : NodeSet) : Prop := Intersects (delete F B)

/-- Quorum availability despite `B` (Mazieres, Def. "DSet", condition 2). -/
def AvailableDespite (F : Fbas) (B : NodeSet) : Prop :=
  (∀ v ∈ F.nodes, B v = true) ∨ IsQuorum F (fun x => !B x)

/-- `B` is a dispensable set: the well-behaved nodes outside it are intact. -/
def IsDSet (F : Fbas) (B : NodeSet) : Prop := IntersectsDespite F B ∧ AvailableDespite F B

/-- The `delete` reading of Byzantine nodes: a node's deleted quorum set is
satisfied by `S` exactly when its original quorum set is satisfied by the
honest part of `S` together with all of `B` (the faulty nodes "vote with
anyone"). This is the flat-quorum-set form of Mazieres' `q \ B` slices. -/
theorem delete_satisfies_iff (F : Fbas) (B S : NodeSet) (v : Nat) :
    Satisfies (delete F B).nodes ((delete F B).qset v) S ↔
      Satisfies F.nodes (F.qset v) (fun x => (S x && !B x) || B x) := by
  simp only [Satisfies, delete, cnt_filter]
  have h := cnt_or_add_and (fun x => (F.qset v).members x && S x && !B x)
    (fun x => (F.qset v).members x && B x) F.nodes
  have e1 : cnt (fun x => (F.qset v).members x && ((S x && !B x) || B x)) F.nodes =
      cnt (fun x => ((F.qset v).members x && S x && !B x) || ((F.qset v).members x && B x))
        F.nodes :=
    cnt_congr (fun x _ => by cases (F.qset v).members x <;> cases S x <;> cases B x <;> rfl)
  have e2 : cnt (fun x => ((F.qset v).members x && S x && !B x) && ((F.qset v).members x && B x))
      F.nodes = 0 := by
    rw [cnt_congr (Q := fun _ => false) (fun x _ => by
      cases (F.qset v).members x <;> cases S x <;> cases B x <;> rfl)]
    exact cnt_false _
  have e3 : cnt (fun x => ((F.qset v).members x && !B x) && S x && !B x) F.nodes =
      cnt (fun x => (F.qset v).members x && S x && !B x) F.nodes :=
    cnt_congr (fun x _ => by cases (F.qset v).members x <;> cases S x <;> cases B x <;> rfl)
  rw [e1, e3]
  omega

theorem delete_empty (F : Fbas) : delete F (fun _ => false) = F := by
  cases F with
  | mk nodes qset =>
    simp only [delete, Bool.not_false, Bool.and_true, Bool.and_false, cnt_false,
      Nat.sub_zero]
    rw [show List.filter (fun _ => true) nodes = nodes from List.filter_eq_self.mpr (by simp)]

theorem intersectsDespite_empty_iff (F : Fbas) :
    IntersectsDespite F (fun _ => false) ↔ Intersects F := by
  simp [IntersectsDespite, delete_empty]

/-! ## Uniform FBAS: every node shares one threshold over the whole universe -/

/-- For an FBAS whose nodes all use a `k`-of-(everyone) quorum set over a
duplicate-free universe `U`, quorums are exactly the non-empty subsets with at
least `k` elements, so quorum intersection holds iff `|U| < 2k` or `|U| <= 1`. -/
theorem uniform_intersects_iff (F : Fbas) (k : Nat) (M : NodeSet)
    (hnd : F.nodes.Nodup) (hq : ∀ v, F.qset v = ⟨k, M⟩) (hM : ∀ x ∈ F.nodes, M x = true) :
    Intersects F ↔ F.nodes.length < 2 * k ∨ F.nodes.length ≤ 1 := by
  have cntM : ∀ S : NodeSet, cnt (fun x => M x && S x) F.nodes = cnt S F.nodes :=
    fun S => cnt_congr (fun x hx => by simp [hM x hx])
  constructor
  · intro hI
    apply Classical.byContradiction
    intro hn
    have hm : 2 * (max k 1) ≤ F.nodes.length := by omega
    have ht := cnt_true F.nodes
    obtain ⟨S, T, -, -, hd, hS, hT⟩ :=
      disjoint_pick (fun _ => true) (fun _ => true) F.nodes hnd (max k 1) (max k 1)
        (by omega) (by omega) (by simp only [Bool.or_self]; omega)
    have quorum : ∀ X : NodeSet, cnt X F.nodes = max k 1 → IsQuorum F X := by
      intro X hX
      refine ⟨exists_mem_of_cnt_pos (by omega), fun v _ _ => ?_⟩
      simp only [Satisfies, hq, cntM]; omega
    obtain ⟨v, hv, hsv, htv⟩ := hI S T (quorum S hS) (quorum T hT)
    exact hd v hv ⟨hsv, htv⟩
  · intro hn S T hS hT
    apply Classical.byContradiction
    intro hne
    have hd : ∀ x ∈ F.nodes, ¬ (S x = true ∧ T x = true) :=
      fun x hx h => hne ⟨x, hx, h⟩
    obtain ⟨⟨vs, hvs, hsv⟩, hSs⟩ := hS
    obtain ⟨vt, hvt, htv⟩ := hT.1
    have hs1 := cnt_pos_of_mem hvs hsv
    have ht1 := cnt_pos_of_mem hvt htv
    have hsk : k ≤ cnt S F.nodes := by
      have := hSs vs hvs hsv; simp only [Satisfies, hq, cntM] at this; exact this
    have htk : k ≤ cnt T F.nodes := by
      have := hT.2 vt hvt htv; simp only [Satisfies, hq, cntM] at this; exact this
    have := cnt_add_le_of_disjoint hd
    omega

/-! ## Symmetric `t`-of-`n` (Botho's flat quorum, `Fbas::symmetric`) -/

/-- `n` nodes `0..n-1`, each with the quorum set `t`-of-all (self included):
`bth_quorum_sim::Fbas::symmetric(n, t)`. -/
def sym (n t : Nat) : Fbas := ⟨List.range n, fun _ => ⟨t, fun x => decide (x < n)⟩⟩

/-- **Symmetric quorum intersection, exact.** `t`-of-`n` has quorum
intersection iff `2t > n` or `n <= 1`. -/
theorem sym_intersects_iff (n t : Nat) : Intersects (sym n t) ↔ n < 2 * t ∨ n ≤ 1 := by
  have := uniform_intersects_iff (sym n t) t (fun x => decide (x < n)) List.nodup_range
    (fun _ => rfl) (fun x hx => by simpa [sym] using hx)
  simpa [sym] using this

/-- For the thresholds the gate can install (`t >= 1`; Explicit clamps into
`1..=n`), the rule is exactly `2t > n`: the closed form
`symmetric_quorum_has_intersection` must compute on both of its branches. -/
theorem sym_intersects_iff_pos (n t : Nat) (ht : 1 ≤ t) :
    Intersects (sym n t) ↔ n < 2 * t := by
  rw [sym_intersects_iff]; omega

/-- The analytic branch of `symmetric_quorum_has_intersection` (`n > 13`):
`threshold > n / 2` is the same predicate as `2 * threshold > n`. -/
theorem analytic_rule_eq (n t : Nat) : n / 2 < t ↔ n < 2 * t := by omega

/-- The initial-seed fallback of `gated_scp_quorum_set`, `n / 2 + 1`-of-`n`,
always has quorum intersection. -/
theorem seed_fallback_intersects (n : Nat) : Intersects (sym n (n / 2 + 1)) :=
  (sym_intersects_iff_pos n _ (by omega)).mpr (by omega)

theorem sym_delete_length (n t : Nat) (B : NodeSet) :
    ((delete (sym n t) B).nodes).length = n - cnt B (List.range n) := by
  simp only [delete, sym, length_filter_eq_cnt]
  have := cnt_add_cnt_not B (List.range n)
  simp only [List.length_range] at this
  omega

theorem cnt_range_and_lt (n : Nat) (B : NodeSet) :
    cnt (fun x => decide (x < n) && B x) (List.range n) = cnt B (List.range n) :=
  cnt_congr (fun x hx => by simp [List.mem_range.mp hx])

/-- **Symmetric quorum intersection despite `B`, exact.** With
`f = |B ∩ {0..n-1}|`: `t`-of-`n` has quorum intersection despite `B` iff
`2t - n > f` (as `n + f < 2t`) or at most one well-behaved node is left. -/
theorem sym_despite_iff (n t : Nat) (B : NodeSet) :
    IntersectsDespite (sym n t) B ↔
      n + cnt B (List.range n) < 2 * t ∨ n ≤ cnt B (List.range n) + 1 := by
  have hf : cnt B (List.range n) ≤ n := by
    have := cnt_le_length B (List.range n); simpa using this
  have h := uniform_intersects_iff (delete (sym n t) B) (t - cnt B (List.range n))
    (fun x => decide (x < n) && !B x)
    (List.nodup_range.filter _)
    (fun v => by simp only [delete, sym, cnt_range_and_lt])
    (fun x hx => by
      simp only [delete, sym, List.mem_filter, List.mem_range] at hx
      simp [hx.1, hx.2])
  rw [sym_delete_length] at h
  simp only [IntersectsDespite, h]
  omega

/-- Quorum availability despite `B` for `t`-of-`n`: `B` is everyone, or the
`n - f` well-behaved nodes still meet the threshold. -/
theorem sym_available_iff (n t : Nat) (B : NodeSet) :
    AvailableDespite (sym n t) B ↔
      cnt B (List.range n) = n ∨
        (cnt B (List.range n) < n ∧ t + cnt B (List.range n) ≤ n) := by
  have hc := cnt_add_cnt_not B (List.range n)
  simp only [List.length_range] at hc
  have hall : (∀ v ∈ (sym n t).nodes, B v = true) ↔ cnt B (List.range n) = n := by
    constructor
    · intro h
      have : cnt B (List.range n) = cnt (fun _ => true) (List.range n) :=
        cnt_congr (fun x hx => h x hx)
      simpa [cnt_true] using this
    · intro h v hv
      apply Classical.byContradiction
      intro hb
      have := cnt_pos_of_mem (P := fun x => !B x) (l := List.range n) hv (by simpa using hb)
      omega
  have hq : IsQuorum (sym n t) (fun x => !B x) ↔
      (cnt B (List.range n) < n ∧ t + cnt B (List.range n) ≤ n) := by
    have e : cnt (fun x => decide (x < n) && !B x) (List.range n) =
        cnt (fun x => !B x) (List.range n) :=
      cnt_congr (fun x hx => by simp [List.mem_range.mp hx])
    constructor
    · rintro ⟨⟨v, hv, hb⟩, hs⟩
      have h1 := cnt_pos_of_mem (P := fun x => !B x) (l := List.range n) hv hb
      have := hs v hv hb
      simp only [Satisfies, sym, e] at this
      omega
    · rintro ⟨h1, h2⟩
      refine ⟨exists_mem_of_cnt_pos (by simp only [sym]; omega), fun v _ _ => ?_⟩
      simp only [Satisfies, sym, e]; omega
  simp only [AvailableDespite, hall, hq]

/-! ## `config::byzantine_tolerance` -/

/-- `config::byzantine_tolerance(n, t)`:
`n.saturating_sub(t).min((2 * t).saturating_sub(n + 1))`. -/
def byzTol (n t : Nat) : Nat := min (n - t) (2 * t - (n + 1))

/-- **`byzantine_tolerance` is the largest tolerated `f`.** For `f >= 1` faulty
nodes among `n` with at least two well-behaved nodes left, `f <=
byzantine_tolerance(n, t)` iff `B` is a DSet of `t`-of-`n` (quorum
intersection *and* availability despite `B`). -/
theorem byzTol_iff_dset (n t : Nat) (B : NodeSet) (hf1 : 1 ≤ cnt B (List.range n))
    (hf2 : cnt B (List.range n) + 2 ≤ n) :
    cnt B (List.range n) ≤ byzTol n t ↔ IsDSet (sym n t) B := by
  simp only [IsDSet, sym_despite_iff, sym_available_iff, byzTol, Nat.le_min]
  omega

/-- The "at least two well-behaved nodes" hypothesis of `byzTol_iff_dset` is
needed, and its failure is on the safe side: `1`-of-`2` with one faulty node
leaves a lone honest node, which is trivially intact, but
`byzantine_tolerance(2, 1) = 0` under-reports it. Harmless (conservative). -/
theorem byzTol_conservative_edge :
    IsDSet (sym 2 1) (fun x => x == 1) ∧ byzTol 2 1 = 0 := by
  have hc : cnt (fun x => x == 1) (List.range 2) = 1 := by decide
  refine ⟨⟨?_, ?_⟩, by decide⟩
  · rw [sym_despite_iff, hc]; omega
  · rw [sym_available_iff, hc]; omega

/-- `byzantine_tolerance` is positive exactly when one fault is tolerated:
`t + 1 <= n` (availability) and `n + 1 < 2t` (intersection despite one). -/
theorem byzTol_pos_iff (n t : Nat) : 1 ≤ byzTol n t ↔ t + 1 ≤ n ∧ n + 1 < 2 * t := by
  simp only [byzTol, Nat.le_min]; omega

/-! ## Recommended-mode thresholds and the `quorumFaultTolerant` flag (lead A) -/

/-- `effective_threshold` under `FaultModel::Crash` (the default): `n / 2 + 1`. -/
def crashThreshold (n : Nat) : Nat := n / 2 + 1

/-- `effective_threshold` under `FaultModel::Bft`: `n - (n - 1) / 3`. -/
def bftThreshold (n : Nat) : Nat := n - (n - 1) / 3

/-- `QuorumConfig::is_bft_fault_tolerant` in Recommended mode **as it was
before #1864**: `n >= 4`, whatever the fault model and threshold. This was
`quorumFaultTolerant`. #1864 fixed the flag (#1843) to follow the installed
set's tolerance, `1 <= byzTol n t`; this definition models the old flag. -/
def recommendedFaultTolerant (n : Nat) : Bool := decide (4 ≤ n)

/-- In Recommended mode the gate never refuses: both fault models' thresholds
always give an intersecting symmetric quorum. So the check is a no-op there;
only Explicit thresholds can be refused. -/
theorem recommended_gate_never_refuses (n : Nat) (hn : 1 ≤ n) :
    Intersects (sym n (crashThreshold n)) ∧ Intersects (sym n (bftThreshold n)) := by
  constructor
  · exact (sym_intersects_iff_pos n _ (by simp [crashThreshold])).mpr
      (by simp only [crashThreshold]; omega)
  · exact (sym_intersects_iff_pos n _ (by simp only [bftThreshold]; omega)).mpr
      (by simp only [bftThreshold]; omega)

/-- **Lead A, refuted (counterexample).** In Recommended + Crash mode at every
odd `n >= 5`, `quorumFaultTolerant` is `true` while the installed
`n / 2 + 1`-of-`n` quorum has `quorumByzantineTolerance = 0` and loses quorum
intersection once any single node is faulty. The flag overclaims. (At `n = 5`:
3-of-5, `2t - n = 1`, not `> 1`.) Filed as #1843. This is a historical finding
about the flag **before #1864**, which fixed it on main; the Rust test
`crash_odd_n_flag_matches_tolerance_through_gate` now checks the fixed flag. -/
theorem recommended_crash_flag_overclaims (n : Nat) (h4 : 4 ≤ n) (hodd : n % 2 = 1)
    (B : NodeSet) (hB : cnt B (List.range n) = 1) :
    recommendedFaultTolerant n = true ∧ byzTol n (crashThreshold n) = 0 ∧
      ¬ IntersectsDespite (sym n (crashThreshold n)) B := by
  refine ⟨by simp [recommendedFaultTolerant, h4], ?_, ?_⟩
  · simp only [byzTol, crashThreshold]; omega
  · rw [sym_despite_iff, hB]; simp only [crashThreshold]; omega

/-- For even `n >= 4` under Crash the flag is right: one fault is tolerated. -/
theorem recommended_crash_flag_even_sound (n : Nat) (h4 : 4 ≤ n) (heven : n % 2 = 0) :
    1 ≤ byzTol n (crashThreshold n) := by
  simp only [byzTol, crashThreshold, Nat.le_min]; omega

/-- Under `FaultModel::Bft` the flag is right: `byzantine_tolerance` of the
installed quorum is exactly `(n - 1) / 3`, which is `>= 1` iff `n >= 4`. -/
theorem recommended_bft_byzTol (n : Nat) (hn : 1 ≤ n) :
    byzTol n (bftThreshold n) = (n - 1) / 3 := by
  simp only [byzTol, bftThreshold]; omega

/-! ## Checkers over heterogeneous flat FBAS -/

/-- Pairwise checker, despite up to `f` faulty nodes: for every two nodes
`v, w`, `|M_v ∪ M_w| + f < t_v + t_w`. With `f = 0` and two configurations
this is lead C's `t_A + t_B > |S_A ∪ S_B|`. It is `O(k^2)` in the number of
distinct quorum sets, cheap enough for the gate. -/
def checkDespite (F : Fbas) (f : Nat) : Bool :=
  F.nodes.all fun v => F.nodes.all fun w =>
    decide (cnt (fun x => (F.qset v).members x || (F.qset w).members x) F.nodes + f <
      (F.qset v).threshold + (F.qset w).threshold)

/-- The pairwise checker with no faulty nodes. -/
def check (F : Fbas) : Bool := checkDespite F 0

theorem cnt_sum4_le (P1 P2 P3 P4 Q1 Q2 : NodeSet) (l : List Nat)
    (h : ∀ x ∈ l, (if P1 x = true then 1 else 0) + (if P2 x = true then 1 else 0) +
        (if P3 x = true then 1 else 0) + (if P4 x = true then 1 else 0) ≤
        (if Q1 x = true then 1 else 0) + (if Q2 x = true then 1 else 0)) :
    cnt P1 l + cnt P2 l + cnt P3 l + cnt P4 l ≤ cnt Q1 l + cnt Q2 l := by
  induction l with
  | nil => simp
  | cons v l ih =>
    simp only [cnt_cons]
    have := ih (fun x hx => h x (by simp [hx]))
    have := h v (by simp)
    omega

/-- **Checker soundness despite faults.** If the pairwise checker accepts `F`
at `f`, then `F` has quorum intersection despite every `B` with at most `f`
members in the universe. -/
theorem checkDespite_sound (F : Fbas) (f : Nat) (B : NodeSet)
    (hc : checkDespite F f = true) (hB : cnt B F.nodes ≤ f) : IntersectsDespite F B := by
  intro S T hS hT
  apply Classical.byContradiction
  intro hne
  obtain ⟨⟨v, hv', hsv⟩, hSs⟩ := hS
  obtain ⟨⟨w, hw', htw⟩, hTs⟩ := hT
  have hsat_v := hSs v hv' hsv
  have hsat_w := hTs w hw' htw
  simp only [delete, List.mem_filter] at hv' hw'
  simp only [Satisfies, delete, cnt_filter] at hsat_v hsat_w
  have hpair : cnt (fun x => (F.qset v).members x || (F.qset w).members x) F.nodes + f <
      (F.qset v).threshold + (F.qset w).threshold := by
    simp only [checkDespite, List.all_eq_true, decide_eq_true_eq] at hc
    exact hc v hv'.1 w hw'.1
  have hsum := cnt_sum4_le
    (fun x => (F.qset v).members x && B x)
    (fun x => (F.qset v).members x && !B x && S x && !B x)
    (fun x => (F.qset w).members x && B x)
    (fun x => (F.qset w).members x && !B x && T x && !B x)
    (fun x => (F.qset v).members x || (F.qset w).members x) B F.nodes
    (fun x hx => by
      have hd : B x = false → ¬ (S x = true ∧ T x = true) := fun hb h =>
        hne ⟨x, List.mem_filter.mpr ⟨hx, by simp [hb]⟩, h⟩
      cases hmv : (F.qset v).members x <;> cases hmw : (F.qset w).members x <;>
        cases hb : B x <;> cases hs : S x <;> cases ht : T x <;> simp_all)
  omega

theorem checkDespite_zero_sound (F : Fbas) (hc : check F = true) : Intersects F := by
  rw [← intersectsDespite_empty_iff]
  exact checkDespite_sound F 0 _ hc (Nat.le_of_eq (cnt_false _))

/-- **Checker soundness.** If the pairwise checker accepts `F`, every two
quorums of `F` intersect. -/
theorem checker_sound (F : Fbas) (hc : check F = true) : Intersects F :=
  checkDespite_zero_sound F hc

/-- **Checker completeness for the symmetric case.** For every threshold the
gate can install (`t >= 1`), the pairwise checker accepts `t`-of-`n` exactly
when it intersects. (`t = 0` is excluded for a reason: `0`-of-`1` intersects
trivially but fails the bound.) -/
theorem checker_complete_sym (n t : Nat) (ht : 1 ≤ t) :
    check (sym n t) = true ↔ Intersects (sym n t) := by
  have e : cnt (fun x => decide (x < n) || decide (x < n)) (List.range n) = n := by
    rw [cnt_congr (Q := fun _ => true) (fun x hx => by simp [List.mem_range.mp hx]), cnt_true,
      List.length_range]
  constructor
  · exact checker_sound _
  · intro h
    rw [sym_intersects_iff] at h
    simp only [check, checkDespite, sym, List.all_eq_true, decide_eq_true_eq, e]
    intro _ _ _ _; omega

/-- The despite-`f` checker is also exact on every `t`-of-`n` with `n ≥ 1`
(any `t` and `f`): it accepts iff `n + f < 2t`, the `sym_despite_iff` bound. -/
theorem checkDespite_sym_iff (n t f : Nat) (hn : 1 ≤ n) :
    checkDespite (sym n t) f = true ↔ n + f < 2 * t := by
  have e : cnt (fun x => decide (x < n) || decide (x < n)) (List.range n) = n := by
    rw [cnt_congr (Q := fun _ => true) (fun x hx => by simp [List.mem_range.mp hx]), cnt_true,
      List.length_range]
  simp only [checkDespite, sym, List.all_eq_true, decide_eq_true_eq, e]
  constructor
  · intro h; have := h 0 (by simp; omega) 0 (by simp; omega); omega
  · intro h _ _ _ _; omega

/-! ### Exact brute-force checker (the `bth_quorum_sim` enumeration) -/

/-- Every sub-list of `l` (the powerset, as lists). -/
def subsetsOf : List Nat → List (List Nat)
  | [] => [[]]
  | v :: l => subsetsOf l ++ (subsetsOf l).map (v :: ·)

/-- The node set a sub-list stands for. -/
def memOf (s : List Nat) : NodeSet := fun x => s.contains x

theorem subsetsOf_complete (S : NodeSet) : ∀ l : List Nat,
    ∃ s ∈ subsetsOf l, (∀ y ∈ s, y ∈ l) ∧ ∀ x ∈ l, memOf s x = S x := by
  intro l
  induction l with
  | nil => exact ⟨[], by simp [subsetsOf], by simp, by simp⟩
  | cons v l ih =>
    obtain ⟨s, hs, hsub, hag⟩ := ih
    by_cases hv : S v = true
    · refine ⟨v :: s, by simp [subsetsOf, hs], ?_, ?_⟩
      · intro y hy
        rcases List.mem_cons.mp hy with h | h
        · simp [h]
        · simp [hsub y h]
      · intro x hx
        by_cases hxv : x = v
        · subst hxv; simp [memOf, hv]
        · have hx' := (List.mem_cons.mp hx).resolve_left hxv
          rw [← hag x hx']
          simp only [memOf, List.contains_cons]
          simp [hxv]
    · refine ⟨s, by simp [subsetsOf, hs], fun y hy => by simp [hsub y hy], ?_⟩
      intro x hx
      by_cases hxv : x = v
      · subst hxv
        by_cases hin : x ∈ l
        · exact hag x hin
        · have : x ∉ s := fun h => hin (hsub x h)
          simp only [memOf]
          simp [this, hv]
      · exact hag x ((List.mem_cons.mp hx).resolve_left hxv)

theorem isQuorum_congr (F : Fbas) {S S' : NodeSet} (h : ∀ x ∈ F.nodes, S x = S' x) :
    IsQuorum F S ↔ IsQuorum F S' := by
  have hc : ∀ q : QSet, cnt (fun x => q.members x && S x) F.nodes =
      cnt (fun x => q.members x && S' x) F.nodes :=
    fun q => cnt_congr (fun x hx => by rw [h x hx])
  simp only [IsQuorum, Satisfies, hc]
  constructor
  · rintro ⟨⟨v, hv, hs⟩, hq⟩
    exact ⟨⟨v, hv, (h v hv) ▸ hs⟩, fun w hw hs' => hq w hw ((h w hw).symm ▸ hs')⟩
  · rintro ⟨⟨v, hv, hs⟩, hq⟩
    exact ⟨⟨v, hv, (h v hv).symm ▸ hs⟩, fun w hw hs' => hq w hw ((h w hw) ▸ hs')⟩

/-- Exact quorum-intersection check by enumerating every pair of node subsets,
as `bth_quorum_sim::Fbas::has_quorum_intersection` does (`2^n` subsets). -/
def bruteIntersects (F : Fbas) : Bool :=
  (subsetsOf F.nodes).all fun s => (subsetsOf F.nodes).all fun t =>
    !(decide (IsQuorum F (memOf s))) || !(decide (IsQuorum F (memOf t))) ||
      F.nodes.any (fun x => memOf s x && memOf t x)

/-- **The brute-force checker is sound and complete** for every flat FBAS. -/
theorem bruteIntersects_iff (F : Fbas) : bruteIntersects F = true ↔ Intersects F := by
  simp only [bruteIntersects, List.all_eq_true, Bool.or_eq_true, Bool.not_eq_true',
    decide_eq_false_iff_not, List.any_eq_true, Bool.and_eq_true]
  constructor
  · intro h S T hS hT
    obtain ⟨s, hs, -, hsa⟩ := subsetsOf_complete S F.nodes
    obtain ⟨t, ht, -, hta⟩ := subsetsOf_complete T F.nodes
    rcases h s hs t ht with (h1 | h1) | ⟨x, hx, h2, h3⟩
    · exact absurd ((isQuorum_congr F hsa).mpr hS) h1
    · exact absurd ((isQuorum_congr F hta).mpr hT) h1
    · exact ⟨x, hx, (hsa x hx) ▸ h2, (hta x hx) ▸ h3⟩
  · intro h s _ t _
    by_cases hs : IsQuorum F (memOf s)
    · by_cases ht : IsQuorum F (memOf t)
      · right; exact h _ _ hs ht
      · left; right; exact ht
    · left; left; exact hs

/-- **The pairwise checker is not complete for heterogeneous FBAS.** Nodes
`0, 1, 2` use 2-of-`{0,1,2}`, node `3` uses 2-of-`{0,1,3}`. Every quorum holds
two of `{0,1,2}`, so quorums intersect, but the pair `(0, 3)` fails the
pairwise bound (`|{0,1,2,3}| = 4 = 2 + 2`). A gate that checked only the
pairwise condition would refuse this safe configuration. -/
def notNecessaryEx : Fbas :=
  ⟨List.range 4, fun v => if v = 3 then ⟨2, fun x => x == 0 || x == 1 || x == 3⟩
    else ⟨2, fun x => decide (x < 3)⟩⟩

theorem checker_not_complete :
    Intersects notNecessaryEx ∧ check notNecessaryEx = false :=
  ⟨(bruteIntersects_iff _).mp (by decide), by decide⟩

/-! ## Item 2: mixed configurations during a curated rotation -/

/-- A fleet over nodes `0..N-1` part-way through a rolling update from quorum
set `A` to quorum set `B`: nodes with `runsB` already run `B`.

**Abstraction.** Every node runs `A` or `B` *verbatim*. The node's Explicit
gate (`gated_scp_quorum_set`, `botho/src/commands/run.rs`) always adds the
local node to the configured roster, so a node that is not on roster `S` runs
`S ∪ {self}`, not `S`. Theorems that quantify over every split therefore range
over some quorum sets no node would run (a roster that omits the node running
it) and do not cover the one such a node does run. The counterexamples and
`rotation_condition_necessary` only use self-listed nodes, so they are
unaffected. For the production instances (`prod_4of5_rotate_*`) the Rust test
`lean_rotation_instances_through_real_gate` re-checks every split with the
quorum sets the real gate builds, and the conclusions hold there too. -/
def mixed (N : Nat) (A B : QSet) (runsB : NodeSet) : Fbas :=
  ⟨List.range N, fun v => if runsB v = true then B else A⟩

/-- **Transitional safety, sufficient condition (lead C, proven).** If
`|S_A| + f < 2 t_A`, `|S_B| + f < 2 t_B` and `|S_A ∪ S_B| + f < t_A + t_B`, then
*every* mixed fleet (any split of who runs which) has quorum intersection
despite any `B` with at most `f` members. This is the check a fleet-level gate
could run against the previous configuration. Splits are over the `mixed`
abstraction (each node runs `A` or `B` verbatim; see `mixed`). -/
theorem rotation_sound (N f : Nat) (A B : QSet) (runsB Byz : NodeSet)
    (hAA : cnt A.members (List.range N) + f < 2 * A.threshold)
    (hBB : cnt B.members (List.range N) + f < 2 * B.threshold)
    (hAB : cnt (fun x => A.members x || B.members x) (List.range N) + f <
      A.threshold + B.threshold)
    (hf : cnt Byz (List.range N) ≤ f) :
    IntersectsDespite (mixed N A B runsB) Byz := by
  apply checkDespite_sound _ f _ _ hf
  have eA : cnt (fun x => A.members x || A.members x) (List.range N) =
      cnt A.members (List.range N) := cnt_congr (fun x _ => by simp)
  have eB : cnt (fun x => B.members x || B.members x) (List.range N) =
      cnt B.members (List.range N) := cnt_congr (fun x _ => by simp)
  have eBA : cnt (fun x => B.members x || A.members x) (List.range N) =
      cnt (fun x => A.members x || B.members x) (List.range N) :=
    cnt_congr (fun x _ => by simp [Bool.or_comm])
  simp only [checkDespite, mixed, List.all_eq_true]
  intro v _ w _
  apply decide_eq_true
  cases runsB v <;> cases runsB w <;>
    simp only [Bool.false_eq_true, ↓reduceIte, eA, eB, eBA] <;> omega

/-- **Transitional safety, the condition is also necessary (f = 0).** If both
endpoints are satisfiable (`t <= |S|`, as the gate's clamp guarantees) and
`t_A + t_B <= |S_A ∪ S_B|`, there is a split of the fleet, with every node
running a configuration that lists itself, whose mixed FBAS has two disjoint
quorums. So the pairwise condition is exactly what "safe under every rollout
order" means for two configurations, and a gate checking it refuses nothing
that is safe in every order. -/
theorem rotation_condition_necessary (N : Nat) (A B : QSet)
    (hcover : ∀ x < N, (A.members x || B.members x) = true)
    (hA1 : 1 ≤ A.threshold) (hB1 : 1 ≤ B.threshold)
    (hA : A.threshold ≤ cnt A.members (List.range N))
    (hB : B.threshold ≤ cnt B.members (List.range N))
    (hAB : A.threshold + B.threshold ≤
      cnt (fun x => A.members x || B.members x) (List.range N)) :
    ∃ runsB : NodeSet,
      (∀ x < N, runsB x = true → B.members x = true) ∧
      (∀ x < N, runsB x = false → A.members x = true) ∧
      ¬ Intersects (mixed N A B runsB) := by
  obtain ⟨X, Y, hX, hY, hd, hcX, hcY⟩ :=
    disjoint_pick A.members B.members (List.range N) List.nodup_range _ _ hA hB hAB
  refine ⟨fun x => Y x || !A.members x, ?_, ?_, ?_⟩
  · intro x hx h
    cases hy : Y x
    · have hcv := hcover x hx
      simp only [hy, Bool.false_or, Bool.not_eq_true'] at h
      simpa [h] using hcv
    · exact hY x (List.mem_range.mpr hx) hy
  · intro x _ h
    simp only [Bool.or_eq_false_iff, Bool.not_eq_false'] at h
    exact h.2
  · intro hI
    have eX : cnt (fun x => A.members x && X x) (List.range N) = cnt X (List.range N) :=
      cnt_congr (fun x hx => by
        cases hxx : X x
        · simp
        · simp [hX x hx hxx])
    have eY : cnt (fun x => B.members x && Y x) (List.range N) = cnt Y (List.range N) :=
      cnt_congr (fun x hx => by
        cases hyy : Y x
        · simp
        · simp [hY x hx hyy])
    have qX : IsQuorum (mixed N A B (fun x => Y x || !A.members x)) X := by
      refine ⟨exists_mem_of_cnt_pos (by simp only [mixed]; omega), fun v hv hxv => ?_⟩
      have hyv : Y v = false := by
        cases h : Y v
        · rfl
        · exact absurd ⟨hxv, h⟩ (hd v hv)
      have hav : A.members v = true := hX v hv hxv
      simp only [Satisfies, mixed, hyv, hav, Bool.not_true, Bool.or_false,
        Bool.false_eq_true, ↓reduceIte, eX]
      omega
    have qY : IsQuorum (mixed N A B (fun x => Y x || !A.members x)) Y := by
      refine ⟨exists_mem_of_cnt_pos (by simp only [mixed]; omega), fun v _ hyv => ?_⟩
      simp only [Satisfies, mixed, hyv, Bool.true_or, ↓reduceIte, eY]
      omega
    obtain ⟨x, hx, h1, h2⟩ := hI X Y qX qY
    exact hd x hx ⟨h1, h2⟩

/-- **A single P4.4 pin/unpin is transitionally safe** when the Explicit
threshold is unchanged and the larger roster passes the despite-`f` bound.
Both rosters sit inside one roster `L` (the larger endpoint), so the mixed
fleet's union is `L`. With `f = 0` this is the gate's own check on the larger
endpoint: if both endpoints pass the gate, every mixed fleet intersects.
"Every mixed fleet" is over the `mixed` abstraction, where each node runs `A`
or `B` verbatim; the real gate adds the local node to the roster (see
`mixed`). -/
theorem nested_rotation_safe (N f t : Nat) (A B : QSet) (L runsB Byz : NodeSet)
    (hA : ∀ x < N, A.members x = true → L x = true)
    (hB : ∀ x < N, B.members x = true → L x = true)
    (htA : A.threshold = t) (htB : B.threshold = t)
    (hL : cnt L (List.range N) + f < 2 * t) (hf : cnt Byz (List.range N) ≤ f) :
    IntersectsDespite (mixed N A B runsB) Byz := by
  have mA : cnt A.members (List.range N) ≤ cnt L (List.range N) :=
    cnt_mono (fun x hx h => hA x (List.mem_range.mp hx) h)
  have mB : cnt B.members (List.range N) ≤ cnt L (List.range N) :=
    cnt_mono (fun x hx h => hB x (List.mem_range.mp hx) h)
  have mAB : cnt (fun x => A.members x || B.members x) (List.range N) ≤ cnt L (List.range N) :=
    cnt_mono (fun x hx h => by
      cases ha : A.members x
      · simp [ha] at h; exact hB x (List.mem_range.mp hx) h
      · exact hA x (List.mem_range.mp hx) ha)
  exact rotation_sound N f A B runsB Byz (by omega) (by omega) (by omega) hf

/-! ## Lead B: Recommended mode builds each node's quorum set from its own view -/

/-- A Recommended-mode fleet: node `v` runs `thr |view v|`-of-`view v`, where
`view v` is `v` plus its connected peers (`gated_scp_quorum_set`, Recommended
branch, below the auto cap). -/
def viewFbas (N : Nat) (view : Nat → NodeSet) (thr : Nat → Nat) : Fbas :=
  ⟨List.range N, fun v => ⟨thr (cnt (view v) (List.range N)), view v⟩⟩

/-- Two triangles `{0,1,2}` and `{2,3,4}` joined at hub `2` (a symmetric
connection graph: `x` sees `y` iff `y` sees `x`). -/
def bowtieCrashView (v : Nat) : NodeSet := fun x =>
  if v = 2 then decide (x < 5) else if v < 2 then decide (x < 3) else decide (2 ≤ x ∧ x < 5)

/-- **Lead B, refuted (counterexample), Crash mode.** Every node's own view is
symmetric and passes the gate (`recommended_gate_never_refuses`), yet the fleet
has disjoint quorums `{0,1}` and `{3,4}`: nodes 0 and 1 run 2-of-`{0,1,2}`,
nodes 3 and 4 run 2-of-`{2,3,4}`. The node-local check cannot see it.
Filed as #1844 (see also #690). -/
theorem recommended_crash_bowtie_fork :
    (∀ v < 5, ∀ x < 5, bowtieCrashView v x = bowtieCrashView x v) ∧
    (∀ v < 5, bowtieCrashView v v = true) ∧
    (∀ v < 5, Intersects (sym (cnt (bowtieCrashView v) (List.range 5))
      (crashThreshold (cnt (bowtieCrashView v) (List.range 5))))) ∧
    ¬ Intersects (viewFbas 5 bowtieCrashView crashThreshold) := by
  refine ⟨by decide, by decide, fun v hv => ?_, fun h => ?_⟩
  · have : 1 ≤ cnt (bowtieCrashView v) (List.range 5) :=
      cnt_pos_of_mem (List.mem_range.mpr hv) (by
        have : ∀ v < 5, bowtieCrashView v v = true := by decide
        exact this v hv)
    exact (recommended_gate_never_refuses _ this).1
  · obtain ⟨x, hx, h1, h2⟩ := h (fun x => decide (x < 2)) (fun x => decide (3 ≤ x))
      (by decide) (by decide)
    simp only [decide_eq_true_eq] at h1 h2
    omega

/-- Two 4-cliques `{0,1,2,3}` and `{3,4,5,6}` sharing hub `3`. -/
def bowtieBftView (v : Nat) : NodeSet := fun x =>
  if v = 3 then decide (x < 7) else if v < 3 then decide (x < 4) else decide (3 ≤ x ∧ x < 7)

/-- **Lead B, refuted (counterexample), Bft mode.** Nodes 0-2 run
3-of-`{0,1,2,3}`, nodes 4-6 run 3-of-`{3,4,5,6}`, hub 3 runs 5-of-7; every
view passes the gate, and `{0,1,2}`, `{4,5,6}` are disjoint quorums. #1844. -/
theorem recommended_bft_bowtie_fork :
    (∀ v < 7, ∀ x < 7, bowtieBftView v x = bowtieBftView x v) ∧
    (∀ v < 7, bowtieBftView v v = true) ∧
    (∀ v < 7, Intersects (sym (cnt (bowtieBftView v) (List.range 7))
      (bftThreshold (cnt (bowtieBftView v) (List.range 7))))) ∧
    ¬ Intersects (viewFbas 7 bowtieBftView bftThreshold) := by
  refine ⟨by decide, by decide, fun v hv => ?_, fun h => ?_⟩
  · have : 1 ≤ cnt (bowtieBftView v) (List.range 7) :=
      cnt_pos_of_mem (List.mem_range.mpr hv) (by
        have : ∀ v < 7, bowtieBftView v v = true := by decide
        exact this v hv)
    exact (recommended_gate_never_refuses _ this).2
  · obtain ⟨x, hx, h1, h2⟩ := h (fun x => decide (x < 3)) (fun x => decide (4 ≤ x))
      (by decide) (by decide)
    simp only [decide_eq_true_eq] at h1 h2
    omega

/-! ## The 4-of-5 production configuration -/

/-- Explicit 4-of-5 intersects, as the gate decides. -/
theorem prod_4of5_intersects : Intersects (sym 5 4) :=
  (sym_intersects_iff_pos 5 4 (by omega)).mpr (by omega)

/-- `quorumByzantineTolerance` for 4-of-5 is `1`. -/
theorem prod_4of5_byzTol : byzTol 5 4 = 1 := by decide

/-- Item 1 at 4-of-5: every single faulty node is a DSet (intersection and
availability despite it), so the advertised `f = 1` holds. -/
theorem prod_4of5_dset_one (B : NodeSet) (hB : cnt B (List.range 5) = 1) :
    IsDSet (sym 5 4) B :=
  (byzTol_iff_dset 5 4 B (by omega) (by omega)).mp (by rw [hB]; decide)

/-- ... and `f = 1` is tight, but on the liveness side: with two faulty nodes
4-of-5 still has quorum intersection (the deleted FBAS is 2-of-3), but the
three well-behaved nodes cannot form a quorum. `byzantine_tolerance(5, 4) =
min(n - t, 2t - n - 1) = min(1, 2)`: safety would survive two faults. -/
theorem prod_4of5_two_faults (B : NodeSet) (hB : cnt B (List.range 5) = 2) :
    IntersectsDespite (sym 5 4) B ∧ ¬ AvailableDespite (sym 5 4) B := by
  rw [sym_despite_iff, sym_available_iff, hB]; omega

/-- The 4-of-5 roster `{0,1,2,3,4}`. -/
def prodA : QSet := ⟨4, fun x => decide (x < 5)⟩

/-- 4-of-5 with member 4 swapped for 5: `{0,1,2,3,5}`. -/
def prodSwapOne : QSet := ⟨4, fun x => decide (x < 4) || x == 5⟩

/-- 4-of-5 with members 3 and 4 swapped for 5 and 6: `{0,1,2,5,6}`. -/
def prodSwapTwo : QSet := ⟨4, fun x => decide (x < 3) || x == 5 || x == 6⟩

/-- **Item 2 at 4-of-5, one-member swap: safe.** For every split in the
`mixed` abstraction (each node runs `prodA` or `prodSwapOne` verbatim), the
mixed fleet keeps quorum intersection despite any one faulty node
(`|∪| = 6`, `4 + 4 - 6 = 2 > 1`). The real gate gives an off-roster node the
roster plus itself instead (see `mixed`); the Rust test
`lean_rotation_instances_through_real_gate` checks those quorum sets in all
64 splits. -/
theorem prod_4of5_rotate_one_despite_one (runsB Byz : NodeSet)
    (hf : cnt Byz (List.range 6) ≤ 1) :
    IntersectsDespite (mixed 6 prodA prodSwapOne runsB) Byz :=
  rotation_sound 6 1 prodA prodSwapOne runsB Byz (by decide) (by decide) (by decide) hf

/-- Item 2 at 4-of-5, two-member swap: for every split in the `mixed`
abstraction the mixed fleet still intersects (`|∪| = 7 < 8`; the real-gate
quorum sets are checked in all 128 splits by the Rust test
`lean_rotation_instances_through_real_gate`) ... -/
theorem prod_4of5_rotate_two_intersects (runsB : NodeSet) :
    Intersects (mixed 7 prodA prodSwapTwo runsB) := by
  rw [← intersectsDespite_empty_iff]
  exact rotation_sound 7 0 prodA prodSwapTwo runsB _ (by decide) (by decide) (by decide)
    (Nat.le_of_eq (cnt_false _))

/-- **... but not despite one faulty node (counterexample).** With nodes 1, 3, 4
still on the old roster, nodes 2, 5, 6 on the new one and node 0 faulty,
`{1,3,4}` and `{2,5,6}` are disjoint quorums of the deleted FBAS. Every node
reports `quorumByzantineTolerance = 1` (each runs a 4-of-5) throughout.
Filed as #1845. -/
theorem prod_4of5_rotate_two_not_despite_one :
    ¬ IntersectsDespite (mixed 7 prodA prodSwapTwo (fun x => x == 2 || x == 5 || x == 6))
      (fun x => x == 0) := by
  intro h
  obtain ⟨x, hx, h1, h2⟩ := h (fun x => x == 1 || x == 3 || x == 4) (fun x => x == 2 || x == 5 || x == 6)
    (by decide) (by decide)
  revert h1 h2
  cases x with
  | zero => decide
  | succ x => cases x with
    | zero => decide
    | succ x => cases x with
      | zero => decide
      | succ x => cases x with
        | zero => decide
        | succ x => cases x with
          | zero => decide
          | succ x => cases x with
            | zero => decide
            | succ x => cases x with
              | zero => decide
              | succ x => simp

/-- **Re-rostering without a transitional check forks (counterexample).** Both
endpoints, 2-of-`{0,1,2}` and 2-of-`{2,3,4}`, pass the gate, but the fleet
mid-rollout (nodes 3, 4 on the new roster) has disjoint quorums `{0,1}` and
`{3,4}`. Nothing in the node-local gate compares a candidate with the
configuration the rest of the fleet still runs. Filed as #1845. -/
theorem reroster_mixed_fork :
    Intersects (sym 3 2) ∧
    ¬ Intersects (mixed 5 ⟨2, fun x => decide (x < 3)⟩ ⟨2, fun x => decide (2 ≤ x ∧ x < 5)⟩
      (fun x => decide (3 ≤ x))) := by
  refine ⟨(sym_intersects_iff_pos 3 2 (by omega)).mpr (by omega), fun h => ?_⟩
  obtain ⟨x, hx, h1, h2⟩ := h (fun x => decide (x < 2)) (fun x => decide (3 ≤ x))
    (by decide) (by decide)
  simp only [decide_eq_true_eq] at h1 h2
  omega

end Botho.Quorum
