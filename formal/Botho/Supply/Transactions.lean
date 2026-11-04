/-
Finite ordinary-transaction value flow (#1894). Status: proven (model).

The arithmetic follows `transaction/clsag/src/lib.rs::verify_ring_signatures`
(input fold, output fold, separate fee addition, equality), and
`botho/src/ledger/lottery_v2.rs::drawing` (aggregate fee fold), at 7e390555.
It reuses Lottery.checkedAdd and sumFeesChecked, without wrapping arithmetic.

Identities and semantic key images are abstract naturals, not wire encodings.
Economic unspent rows, retained stored rows (including spent ring decoys), and
known key history are separate. CLSAG source/amount/key binding, stable
injective semantic keys, and history/unspent correspondence are named trusted
premises, not axioms or claims about the Rust verifier or LMDB. Store/context
framing and successful-commit correspondence remain external obligations.
This layer does not compose emission, lottery payouts, genesis, or counters.
-/
import Botho.Lottery.Drawing

namespace Botho.Supply.Transactions

open Botho.Lottery

/-- An identified amount. Stored spent rows need not be economically unspent. -/
structure Entry where
  id : Nat
  amount : Nat
  deriving DecidableEq, Repr

/-- The real source identity is a semantic witness, not a public ring index.
The amount is the authenticated pseudo amount; `CLSAGBinding` connects it to
the persisted actual amount. -/
structure Input where
  source : Entry
  key : Nat
  deriving DecidableEq, Repr

structure Transaction where
  inputs : List Input
  outputs : List Entry
  fee : Nat
  deriving DecidableEq, Repr

/-- Unbounded accounting totals; only individual checked folds impose u64 bounds. -/
def value (rows : List Entry) : Nat := (rows.map Entry.amount).sum

def sources (txs : List Transaction) : List Entry :=
  txs.flatMap (fun tx => tx.inputs.map Input.source)

def outputs (txs : List Transaction) : List Entry := txs.flatMap Transaction.outputs

def fees (txs : List Transaction) : Nat := (txs.map Transaction.fee).sum

/-- Successful generalized folds require an in-range initial accumulator for
an in-range result. In particular, the empty fold does not check its input. -/
theorem checked_sum_exact (xs : List Nat) (acc total : Nat)
    (ha : acc ≤ U64_MAX) (h : sumFeesChecked acc xs = some total) :
    total = acc + xs.sum ∧ total ≤ U64_MAX := by
  induction xs generalizing acc with
  | nil => simp [sumFeesChecked] at h; subst total; simp [ha]
  | cons x xs ih =>
    simp only [sumFeesChecked] at h
    cases hc : checkedAdd U64_MAX acc x with
    | none => simp [hc] at h
    | some next =>
      simp only [hc] at h
      have hn : next = acc + x ∧ next ≤ U64_MAX := by
        unfold checkedAdd at hc
        split at hc <;> simp_all
      have hi := ih next hn.2 h
      simp only [List.sum_cons]
      omega

/-- Three separate checked operations mirror the Rust order exactly. -/
def checkTransaction (tx : Transaction) : Option Nat := do
  let i ← sumFeesChecked 0 (tx.inputs.map (fun x => x.source.amount))
  let o ← sumFeesChecked 0 (tx.outputs.map Entry.amount)
  let of ← checkedAdd U64_MAX o tx.fee
  if i = of then some i else none

/-- Per-transaction equality follows from executable checks, not a balance premise. -/
theorem transaction_balance (tx : Transaction) (total : Nat)
    (h : checkTransaction tx = some total) :
    value (tx.inputs.map Input.source) = value tx.outputs + tx.fee ∧
      total = value (tx.inputs.map Input.source) ∧ total ≤ U64_MAX := by
  unfold checkTransaction at h
  cases hi : sumFeesChecked 0 (tx.inputs.map (fun x => x.source.amount)) with
  | none => simp [hi] at h
  | some i =>
    cases ho : sumFeesChecked 0 (tx.outputs.map Entry.amount) with
    | none => simp [hi, ho] at h
    | some o =>
      cases hf : checkedAdd U64_MAX o tx.fee with
      | none => simp [hi, ho, hf] at h
      | some of =>
        simp only [hi, ho, hf, bind, Option.bind] at h
        split at h
        · have he : i = total := Option.some.inj h
          have hir := checked_sum_exact _ 0 i (by omega) hi
          have hor := checked_sum_exact _ 0 o (by omega) ho
          have hfr : of = o + tx.fee := by
            unfold checkedAdd at hf
            split at hf <;> simp_all
          simp only [value, List.map_map, Function.comp_def]
          omega
        · cases h

def checkBalances : List Transaction → Option Unit
  | [] => some ()
  | tx :: txs => do
    let _ ← checkTransaction tx
    checkBalances txs

/-- The aggregate fee check is additional to each transaction's arithmetic. -/
def checkBatch (txs : List Transaction) : Option Nat := do
  let _ ← checkBalances txs
  sumFeesChecked 0 (txs.map Transaction.fee)

private theorem balances_exact (txs : List Transaction)
    (h : checkBalances txs = some ()) :
    value (sources txs) = value (outputs txs) + fees txs := by
  induction txs with
  | nil => simp [sources, outputs, value, fees]
  | cons tx txs ih =>
    simp only [checkBalances] at h
    cases hc : checkTransaction tx with
    | none => simp [hc] at h
    | some total =>
      simp only [hc] at h
      have ht := transaction_balance tx total hc
      have hi := ih h
      simp only [sources, outputs, fees, value, List.flatMap_cons,
        List.map_append, List.sum_append, List.map_cons, List.sum_cons] at *
      omega

/-- Global inputs and outputs are Nat totals and need not fit u64. Only fees
have the separate aggregate checked-fold bound. -/
theorem batch_balance (txs : List Transaction) (feeTotal : Nat)
    (h : checkBatch txs = some feeTotal) :
    value (sources txs) = value (outputs txs) + fees txs ∧
      feeTotal = fees txs ∧ feeTotal ≤ U64_MAX := by
  unfold checkBatch at h
  cases hb : checkBalances txs with
  | none => simp [hb] at h
  | some u =>
    cases u
    simp only [hb] at h
    have hf := checked_sum_exact _ 0 feeTotal (by omega) h
    exact ⟨balances_exact txs hb, by simpa [fees] using hf⟩

/-- First-match lookup; uniqueness below rules out ambiguous stored amounts. -/
def lookup (id : Nat) : List Entry → Option Nat
  | [] => none
  | row :: rows => if row.id = id then some row.amount else lookup id rows

def Unique (rows : List Entry) : Prop := (rows.map Entry.id).Nodup

/-- Delete by identity only after matching the authenticated amount. -/
def removeSource (source : Entry) (rows : List Entry) : Option (List Entry) :=
  if lookup source.id rows = some source.amount then
    some (rows.filter (fun row => row.id != source.id))
  else none

/-- All claimed sources are supplied before insertion of any created output. -/
def consume : List Entry → List Entry → Option (List Entry)
  | rows, [] => some rows
  | rows, source :: rest => do
    let remaining ← removeSource source rows
    consume remaining rest

private theorem lookup_none (id : Nat) (rows : List Entry) :
    lookup id rows = none ↔ id ∉ rows.map Entry.id := by
  induction rows with
  | nil => simp [lookup]
  | cons row rows ih =>
    simp only [lookup, List.map_cons, List.mem_cons]
    split <;> simp_all [eq_comm]

private theorem lookup_filter (id other : Nat) (rows : List Entry) (hne : id ≠ other) :
    lookup id (rows.filter (fun row => row.id != other)) = lookup id rows := by
  induction rows with
  | nil => rfl
  | cons row rows ih =>
    by_cases hr : row.id = other
    · simp [hr, lookup, ih, Ne.symm hne]
    · simp [hr, lookup, ih]

private theorem unique_filter (rows : List Entry) (id : Nat) (h : Unique rows) :
    Unique (rows.filter (fun row => row.id != id)) := by
  exact h.sublist (List.Sublist.map _ List.filter_sublist)

private theorem remove_exact (rows : List Entry) (source : Entry) (remaining : List Entry)
    (hu : Unique rows) (h : removeSource source rows = some remaining) :
    source.amount + value remaining = value rows ∧ Unique remaining ∧
      (∀ id, id ≠ source.id → lookup id remaining = lookup id rows) := by
  unfold removeSource at h
  split at h
  next hl =>
    have he := Option.some.inj h
    subst remaining
    refine ⟨?_, unique_filter rows source.id hu, fun id hn => lookup_filter id source.id rows hn⟩
    induction rows with
    | nil => simp [lookup] at hl
    | cons row rows ih =>
      simp only [Unique, List.map_cons, List.nodup_cons] at hu
      by_cases hr : row.id = source.id
      · have ha : row.amount = source.amount := by simpa [lookup, hr] using hl
        have hn : lookup source.id rows = none := (lookup_none _ _).2 (by simpa [hr] using hu.1)
        have hall : rows.filter (fun r => r.id != source.id) = rows := by
          apply List.filter_eq_self.mpr
          intro r hm
          simp only [bne_iff_ne, ne_eq]
          intro he
          apply hu.1
          exact List.mem_map.mpr ⟨r, hm, by omega⟩
        simp [hr, hall, value, ha]
      · have hl' : lookup source.id rows = some source.amount := by simpa [lookup, hr] using hl
        have hi := ih hu.2 hl'
        simp [hr, value] at *
        omega
  next => cases h

/-- Successful finite consumption partitions the prestate and preserves every
unselected lookup, including its amount. No subtraction truncation is assumed. -/
theorem consume_partition (rows selected remaining : List Entry)
    (hu : Unique rows) (h : consume rows selected = some remaining) :
    value selected + value remaining = value rows ∧ Unique remaining ∧
      (∀ id, id ∉ selected.map Entry.id → lookup id remaining = lookup id rows) := by
  induction selected generalizing rows with
  | nil => simp only [consume, Option.some.injEq] at h; subst remaining; simp [value, hu]
  | cons source rest ih =>
    simp only [consume] at h
    cases hr : removeSource source rows with
    | none => simp [hr] at h
    | some middle =>
      simp only [hr] at h
      have hm := remove_exact rows source middle hu hr
      have ht := ih middle hm.2.1 h
      refine ⟨?_, ht.2.1, ?_⟩
      · simp only [value, List.map_cons, List.sum_cons] at *
        omega
      · intro id hn
        simp only [List.map_cons, List.mem_cons, not_or] at hn
        rw [ht.2.2 id hn.2, hm.2.2 id hn.1]

private theorem consume_exists (rows selected : List Entry)
    (hu : Unique rows) (hs : Unique selected)
    (ha : ∀ source ∈ selected, lookup source.id rows = some source.amount) :
    ∃ remaining, consume rows selected = some remaining := by
  induction selected generalizing rows with
  | nil => exact ⟨rows, rfl⟩
  | cons source rest ih =>
    simp only [Unique, List.map_cons, List.nodup_cons] at hs
    let middle := rows.filter (fun row => row.id != source.id)
    have hr : removeSource source rows = some middle := by
      simp [removeSource, ha source (by simp), middle]
    have hm := unique_filter rows source.id hu
    have ht : ∀ row ∈ rest, lookup row.id middle = some row.amount := by
      intro row hrow
      have hn : row.id ≠ source.id := by
        intro he
        exact hs.1 (List.mem_map.mpr ⟨row, hrow, he⟩)
      rw [lookup_filter _ _ _ hn]
      exact ha row (by simp [hrow])
    obtain ⟨remaining, hh⟩ := ih middle hm hs.2 ht
    exact ⟨remaining, by simp [consume, hr, hh]⟩

/-- The retained store includes spent decoys. History is a set-like list;
its order is immaterial to the correspondence predicates. -/
structure Store where
  unspent : List Entry
  retained : List Entry
  history : List Nat
  deriving DecidableEq, Repr

/-- Unique identities and identical amounts for the economic subset. -/
structure WellFormed (s : Store) : Prop where
  unspent_unique : Unique s.unspent
  retained_unique : Unique s.retained
  agrees : ∀ id amount, lookup id s.unspent = some amount →
    lookup id s.retained = some amount

/-- Trusted CLSAG binding: the witness identifies an authenticated retained
source, its actual amount, and its associated semantic key image. Public ring
membership alone is not this assertion. -/
def CLSAGBinding (s : Store) (keyOf : Nat → Nat) (claims : List Input) : Prop :=
  ∀ claim ∈ claims, lookup claim.source.id s.retained = some claim.source.amount ∧
    claim.key = keyOf claim.source.id

/-- Trusted stable/injective semantic association on the retained domain.
One function is used throughout the step; new identities need an extension. -/
def KeyAssociation (s : Store) (keyOf : Nat → Nat) : Prop :=
  ∀ a ∈ s.retained.map Entry.id, ∀ b ∈ s.retained.map Entry.id,
    keyOf a = keyOf b → a = b

/-- Trusted bidirectional correspondence for each authenticated stored row.
Freshness implies economic availability only through the right-to-left direction. -/
def UnspentCorrespondence (s : Store) (keyOf : Nat → Nat) : Prop :=
  ∀ id amount, lookup id s.retained = some amount →
    (lookup id s.unspent = some amount ↔ keyOf id ∉ s.history)

/-- Executable batch version of the writer's incremental key-image checks:
no key repeats within the batch or appears in the prestate history. -/
def checkKeys (history keys : List Nat) : Option (List Nat) :=
  if keys.Nodup ∧ (∀ key ∈ keys, key ∉ history) then some (history ++ keys) else none

private theorem keys_checked (history keys after : List Nat)
    (h : checkKeys history keys = some after) :
    keys.Nodup ∧ (∀ key ∈ keys, key ∉ history) ∧ after = history ++ keys := by
  unfold checkKeys at h
  split at h
  · simp_all
  · cases h

private theorem sources_unique (claims : List Input) (keyOf : Nat → Nat)
    (hk : (claims.map Input.key).Nodup)
    (hb : ∀ claim ∈ claims, claim.key = keyOf claim.source.id) :
    Unique (claims.map Input.source) := by
  induction claims with
  | nil => simp [Unique]
  | cons claim claims ih =>
    simp only [List.map_cons, List.nodup_cons] at hk
    simp only [Unique, List.map_cons, List.nodup_cons]
    refine ⟨?_, ih hk.2 (fun c hc => hb c (by simp [hc]))⟩
    intro hm
    obtain ⟨other, ho, he⟩ := List.mem_map.mp hm
    obtain ⟨c, hc, hcs⟩ := List.mem_map.mp ho
    have heq : c.key = claim.key := by
      rw [hb c (by simp [hc]), hb claim (by simp)]
      rw [← hcs] at he
      rw [he]
    exact hk.1 (List.mem_map.mpr ⟨c, hc, heq⟩)

/-- Fresh/distinct checked keys imply distinct available pre-block sources
under the separately named binding/correspondence assumptions. This constructs
successful removals and derives the spent bound; neither is an assumed premise.
Injectivity is retained as an explicit ledger invariant, and also yields
semantic key separation for distinct authenticated stored identities. -/
theorem checked_keys_sources (s : Store) (claims : List Input) (keyOf : Nat → Nat)
    (afterHistory : List Nat) (hw : WellFormed s)
    (hb : CLSAGBinding s keyOf claims) (ha : KeyAssociation s keyOf)
    (hc : UnspentCorrespondence s keyOf)
    (hk : checkKeys s.history (claims.map Input.key) = some afterHistory) :
    Unique (claims.map Input.source) ∧
    (∀ claim ∈ claims, lookup claim.source.id s.unspent = some claim.source.amount) ∧
    (∃ remaining, consume s.unspent (claims.map Input.source) = some remaining ∧
      value (claims.map Input.source) + value remaining = value s.unspent) ∧
    value (claims.map Input.source) ≤ value s.unspent ∧
    (∀ a ∈ s.retained.map Entry.id, ∀ b ∈ s.retained.map Entry.id,
      a ≠ b → keyOf a ≠ keyOf b) := by
  have hkeys := keys_checked _ _ _ hk
  have huniq := sources_unique claims keyOf hkeys.1 (fun c h => (hb c h).2)
  have hav : ∀ c ∈ claims, lookup c.source.id s.unspent = some c.source.amount := by
    intro c hm
    apply (hc c.source.id c.source.amount (hb c hm).1).2
    rw [← (hb c hm).2]
    exact hkeys.2.1 c.key (List.mem_map.mpr ⟨c, hm, rfl⟩)
  have hall : ∀ source ∈ claims.map Input.source,
      lookup source.id s.unspent = some source.amount := by
    intro source hs
    obtain ⟨c, hm, rfl⟩ := List.mem_map.mp hs
    exact hav c hm
  obtain ⟨remaining, hr⟩ := consume_exists s.unspent _ hw.unspent_unique huniq hall
  have hp := consume_partition s.unspent _ remaining hw.unspent_unique hr
  exact ⟨huniq, hav, ⟨remaining, hr, hp.1⟩, by omega,
    fun a hma b hmb hn he => hn (ha a hma b hmb he)⟩

/-- Freshness is tested against the whole retained domain, incrementally.
An old spent decoy cannot be overwritten, even though it is not unspent. -/
def insertFresh : Store → List Entry → Option Store
  | s, [] => some s
  | s, row :: rows =>
    if lookup row.id s.retained = none then
      insertFresh { s with unspent := row :: s.unspent, retained := row :: s.retained } rows
    else none

private theorem insert_one_wf (s : Store) (row : Entry) (hw : WellFormed s)
    (hf : lookup row.id s.retained = none) :
    WellFormed { s with unspent := row :: s.unspent, retained := row :: s.retained } := by
  have hu : lookup row.id s.unspent = none := by
    cases hh : lookup row.id s.unspent with
    | none => rfl
    | some amount => have := hw.agrees _ _ hh; simp_all
  refine ⟨?_, ?_, ?_⟩
  · simp only [Unique, List.map_cons, List.nodup_cons]
    exact ⟨(lookup_none _ _).1 hu, hw.unspent_unique⟩
  · simp only [Unique, List.map_cons, List.nodup_cons]
    exact ⟨(lookup_none _ _).1 hf, hw.retained_unique⟩
  · intro id amount h
    simp only [lookup] at *
    split at h
    · simp_all
    · simp_all only [↓reduceIte]
      exact hw.agrees _ _ h

private theorem lookup_append (id : Nat) (front back : List Entry) :
    lookup id (front ++ back) =
      match lookup id front with | some amount => some amount | none => lookup id back := by
  induction front with
  | nil => rfl
  | cons row rows ih => simp only [List.cons_append, lookup]; split <;> simp_all

private theorem insert_fresh_old (s after : Store) (created : List Entry)
    (h : insertFresh s created = some after) :
    ∀ id, id ∈ s.retained.map Entry.id → id ∉ created.map Entry.id := by
  induction created generalizing s with
  | nil => simp
  | cons row rows ih =>
    simp only [insertFresh] at h
    split at h
    next hf =>
      intro id hm
      have hne : id ≠ row.id := by
        intro he
        subst id
        exact (lookup_none _ _).1 hf hm
      have ht := ih _ h id (by simp [hm])
      simpa using And.intro hne ht
    next => cases h

/-- Fresh insertion preserves uniqueness, adds precisely the created value,
retains every old row/amount, and leaves the complete old key history unchanged. -/
theorem insertion_preserves (s after : Store) (created : List Entry)
    (hw : WellFormed s) (h : insertFresh s created = some after) :
    WellFormed after ∧ after.unspent = created.reverse ++ s.unspent ∧
      after.retained = created.reverse ++ s.retained ∧ after.history = s.history ∧
      value after.unspent = value created + value s.unspent ∧
      (∀ id, id ∈ s.retained.map Entry.id →
        lookup id after.retained = lookup id s.retained ∧
        lookup id after.unspent = lookup id s.unspent) := by
  have facts : WellFormed after ∧ after.unspent = created.reverse ++ s.unspent ∧
      after.retained = created.reverse ++ s.retained ∧ after.history = s.history := by
    induction created generalizing s with
    | nil => simp only [insertFresh, Option.some.injEq] at h; subst after; simp [hw]
    | cons row rows ih =>
      simp only [insertFresh] at h
      split at h
      next hf =>
        have ht := ih _ (insert_one_wf s row hw hf) h
        simpa [List.reverse_cons, List.append_assoc] using ht
      next => cases h
  refine ⟨facts.1, facts.2.1, facts.2.2.1, facts.2.2.2, ?_, ?_⟩
  · rw [facts.2.1]
    simp [value, List.map_reverse, List.sum_reverse]
  · intro id hm
    have hn : lookup id created.reverse = none := (lookup_none _ _).2 (by
      simpa using insert_fresh_old s after created h id hm)
    rw [facts.2.1, facts.2.2.1, lookup_append, lookup_append, hn]
    simp

private theorem lookup_filter_self (id : Nat) (rows : List Entry) :
    lookup id (rows.filter (fun row => row.id != id)) = none := by
  apply (lookup_none _ _).2
  intro hm
  obtain ⟨row, hr, he⟩ := List.mem_map.mp hm
  have hf := (List.mem_filter.mp hr).2
  simp [he] at hf

private theorem consume_lookup (rows selected remaining : List Entry)
    (h : consume rows selected = some remaining) :
    ∀ id, lookup id remaining = if id ∈ selected.map Entry.id then none else lookup id rows := by
  induction selected generalizing rows with
  | nil => simp only [consume, Option.some.injEq] at h; subst remaining; simp
  | cons source rest ih =>
    simp only [consume] at h
    cases hr : removeSource source rows with
    | none => simp [hr] at h
    | some middle =>
      simp only [hr] at h
      unfold removeSource at hr
      split at hr
      next =>
        have he := Option.some.inj hr
        subst middle
        intro id
        rw [ih _ h id]
        by_cases hi : id = source.id
        · simp [hi, lookup_filter_self]
        · simp [hi, lookup_filter _ _ _ hi]
      next => cases hr

private theorem remaining_wf (s : Store) (selected remaining : List Entry)
    (hw : WellFormed s) (h : consume s.unspent selected = some remaining)
    (history : List Nat) :
    WellFormed { s with unspent := remaining, history := history } := by
  refine ⟨(consume_partition _ _ _ hw.unspent_unique h).2.1, hw.retained_unique, ?_⟩
  intro id amount hl
  have he := consume_lookup _ _ _ h id
  split at he
  · simp_all
  · exact hw.agrees _ _ (he ▸ hl)

def claims (txs : List Transaction) : List Input := txs.flatMap Transaction.inputs

/-- Checked ordinary-transaction economic update. This is a ghost projection:
Rust retains all outputs, and tracks spending by key images. Created outputs
are inserted only after every source was read from and consumed from the
pre-block economic domain. No emissions or lottery outputs are included. -/
def ordinaryStep (s : Store) (txs : List Transaction) : Option Store := do
  let _ ← checkBatch txs
  let history ← checkKeys s.history ((claims txs).map Input.key)
  let remaining ← consume s.unspent (sources txs)
  insertFresh { s with unspent := remaining, history := history } (outputs txs)

/-- The ordinary step discharges the scalar transaction equality and spent
bound, while retaining the finite frame and key history. The executable
source checks bind every selected amount; the separate `checked_keys_sources`
theorem explains why authenticated fresh keys make those checks succeed. -/
theorem ordinary_step_preserves (s after : Store) (txs : List Transaction)
    (hw : WellFormed s) (h : ordinaryStep s txs = some after) :
    WellFormed after ∧
      value s.unspent + value (outputs txs) = value after.unspent + value (sources txs) ∧
      value (sources txs) = value (outputs txs) + fees txs ∧
      value (sources txs) ≤ value s.unspent ∧ fees txs ≤ U64_MAX ∧
      after.retained = (outputs txs).reverse ++ s.retained ∧
      after.history = s.history ++ (claims txs).map Input.key ∧
      (∀ id, id ∈ s.retained.map Entry.id →
        lookup id after.retained = lookup id s.retained ∧
        (id ∉ (sources txs).map Entry.id → lookup id after.unspent = lookup id s.unspent)) := by
  unfold ordinaryStep at h
  cases hb : checkBatch txs with
  | none => simp [hb] at h
  | some f =>
    cases hk : checkKeys s.history ((claims txs).map Input.key) with
    | none => simp [hb, hk] at h
    | some history =>
      cases hr : consume s.unspent (sources txs) with
      | none => simp [hb, hk, hr] at h
      | some remaining =>
        simp only [hb, hk, hr, bind, Option.bind] at h
        have hp := consume_partition _ _ _ hw.unspent_unique hr
        have hi := insertion_preserves _ after (outputs txs) (remaining_wf s _ _ hw hr history) h
        have hf := batch_balance txs f hb
        have hh := keys_checked _ _ _ hk
        refine ⟨hi.1, ?_, hf.1, ?_, ?_, hi.2.2.1, ?_, ?_⟩
        · have hv := hi.2.2.2.2.1
          dsimp at hv
          omega
        · omega
        · omega
        · exact hi.2.2.2.1.trans hh.2.2
        · intro id hm
          have hframe := hi.2.2.2.2.2 id hm
          exact ⟨hframe.1, fun hn => hframe.2.trans (hp.2.2 id hn)⟩

/-- New semantic identities must extend the association and introduce no key
already in the resulting known history. This is a cryptographic/ledger
extension obligation, not something inferred merely from a fresh outpoint. -/
def IdentityExtension (after : Store) (created : List Entry) (keyOf : Nat → Nat) : Prop :=
  KeyAssociation after keyOf ∧ ∀ row ∈ created, keyOf row.id ∉ after.history

private theorem lookup_some_entry (id amount : Nat) (rows : List Entry)
    (h : lookup id rows = some amount) : ∃ row ∈ rows, row.id = id ∧ row.amount = amount := by
  induction rows with
  | nil => cases h
  | cons row rows ih =>
    simp only [lookup] at h
    split at h
    next he => exact ⟨row, by simp, he, Option.some.inj h⟩
    next =>
      obtain ⟨r, hr, hi, ha⟩ := ih h
      exact ⟨r, by simp [hr], hi, ha⟩

private theorem bound_key_membership (s : Store) (cs : List Input) (keyOf : Nat → Nat)
    (hb : CLSAGBinding s keyOf cs) (ha : KeyAssociation s keyOf)
    (id amount : Nat) (hl : lookup id s.retained = some amount) :
    id ∈ (cs.map Input.source).map Entry.id ↔ keyOf id ∈ cs.map Input.key := by
  have hid : id ∈ s.retained.map Entry.id := by
    obtain ⟨row, hr, hi, _⟩ := lookup_some_entry id amount s.retained hl
    exact List.mem_map.mpr ⟨row, hr, hi⟩
  constructor
  · intro hm
    obtain ⟨source, hs, hi⟩ := List.mem_map.mp hm
    obtain ⟨c, hc, he⟩ := List.mem_map.mp hs
    apply List.mem_map.mpr
    refine ⟨c, hc, ?_⟩
    rw [(hb c hc).2, he, hi]
  · intro hm
    obtain ⟨c, hc, he⟩ := List.mem_map.mp hm
    have hs : c.source.id ∈ s.retained.map Entry.id := by
      obtain ⟨row, hr, hi, _⟩ := lookup_some_entry _ _ _ (hb c hc).1
      exact List.mem_map.mpr ⟨row, hr, hi⟩
    have hi := ha c.source.id hs id hid ((hb c hc).2.symm.trans he)
    exact List.mem_map.mpr ⟨c.source, List.mem_map.mpr ⟨c, hc, rfl⟩, hi⟩

private theorem consumption_correspondence (s : Store) (cs : List Input)
    (remaining : List Entry) (history : List Nat) (keyOf : Nat → Nat)
    (hb : CLSAGBinding s keyOf cs) (ha : KeyAssociation s keyOf)
    (hc : UnspentCorrespondence s keyOf)
    (hk : checkKeys s.history (cs.map Input.key) = some history)
    (hr : consume s.unspent (cs.map Input.source) = some remaining) :
    UnspentCorrespondence { s with unspent := remaining, history := history } keyOf := by
  intro id amount hl
  have hm := bound_key_membership s cs keyOf hb ha id amount hl
  have he := consume_lookup _ _ _ hr id
  have hp := hc id amount hl
  have hh := (keys_checked _ _ _ hk).2.2
  dsimp
  rw [he, hh]
  by_cases hs : id ∈ (cs.map Input.source).map Entry.id
  · rw [ite_eq_left hs]
    simp only [List.mem_append, hm.mp hs, or_true, not_true_eq_false, reduceCtorEq]
  · have hn : keyOf id ∉ cs.map Input.key := fun h => hs (hm.mpr h)
    rw [ite_eq_right hs]
    simpa only [List.mem_append, hn, or_false] using hp

private theorem insertion_correspondence (s after : Store) (created : List Entry)
    (keyOf : Nat → Nat) (hw : WellFormed s)
    (hc : UnspentCorrespondence s keyOf)
    (h : insertFresh s created = some after)
    (he : IdentityExtension after created keyOf) :
    UnspentCorrespondence after keyOf := by
  have hi := insertion_preserves s after created hw h
  intro id amount hl
  rw [hi.2.2.1, lookup_append] at hl
  rw [hi.2.1, lookup_append]
  cases hf : lookup id created.reverse with
  | none =>
    simp only [hf] at hl ⊢
    rw [hi.2.2.2.1]
    exact hc id amount hl
  | some a =>
    simp only [hf, Option.some.injEq] at hl ⊢
    obtain ⟨row, hr, hid, _⟩ := lookup_some_entry _ _ _ hf
    have hn := he.2 row (by simpa using hr)
    rw [hid] at hn
    exact iff_of_true hl hn

/-- The identity/history invariant is preserved for the next ordinary step
under the explicit semantic extension obligation for created identities.
Fresh outpoint checks alone do not establish this cryptographic obligation. -/
theorem ordinary_step_correspondence (s after : Store) (txs : List Transaction)
    (keyOf : Nat → Nat) (hw : WellFormed s)
    (hb : CLSAGBinding s keyOf (claims txs)) (ha : KeyAssociation s keyOf)
    (hc : UnspentCorrespondence s keyOf)
    (h : ordinaryStep s txs = some after)
    (he : IdentityExtension after (outputs txs) keyOf) :
    UnspentCorrespondence after keyOf ∧ KeyAssociation after keyOf := by
  unfold ordinaryStep at h
  cases hbatch : checkBatch txs with
  | none => simp [hbatch] at h
  | some f =>
    cases hk : checkKeys s.history ((claims txs).map Input.key) with
    | none => simp [hbatch, hk] at h
    | some history =>
      cases hr : consume s.unspent (sources txs) with
      | none => simp [hbatch, hk, hr] at h
      | some remaining =>
        simp only [hbatch, hk, hr, bind, Option.bind] at h
        have hs : (claims txs).map Input.source = sources txs := by
          simp [claims, sources, List.map_flatMap]
        have hmid := consumption_correspondence s (claims txs) remaining history keyOf
          hb ha hc hk (by simpa [hs] using hr)
        exact ⟨insertion_correspondence _ after (outputs txs) keyOf
          (remaining_wf s _ _ hw hr history) hmid h he, he.1⟩

private theorem insertion_exists_same_store (s t after : Store) (created : List Entry)
    (he : s.retained = t.retained) (h : insertFresh s created = some after) :
    ∃ result, insertFresh t created = some result := by
  induction created generalizing s t with
  | nil => exact ⟨t, rfl⟩
  | cons row rows ih =>
    simp only [insertFresh] at h ⊢
    split at h
    next hf =>
      rw [he] at hf
      rw [ite_eq_left hf]
      exact ih _ _ (by simp [he]) h
    next => cases h

/-- Composition usable from prestate and source data alone: arithmetic,
key-image and fresh-ID checks plus the named trust predicates construct a
successful finite update and discharge both scalar transaction premises.
`hnew` checks output IDs in the prestate; it supplies no expected poststate
and assumes no desired source removal. -/
theorem checked_ordinary_flow (s : Store) (txs : List Transaction) (keyOf : Nat → Nat)
    (feeTotal : Nat) (history : List Nat) (hw : WellFormed s)
    (hb : CLSAGBinding s keyOf (claims txs)) (ha : KeyAssociation s keyOf)
    (hc : UnspentCorrespondence s keyOf)
    (hbatch : checkBatch txs = some feeTotal)
    (hkeys : checkKeys s.history ((claims txs).map Input.key) = some history)
    (hnew : insertFresh s (outputs txs) ≠ none) :
    ∃ after, ordinaryStep s txs = some after ∧ WellFormed after ∧
      value (sources txs) = value (outputs txs) + fees txs ∧
      value (sources txs) ≤ value s.unspent ∧ fees txs ≤ U64_MAX ∧
      value s.unspent + value (outputs txs) = value after.unspent + value (sources txs) := by
  have hs : (claims txs).map Input.source = sources txs := by
    simp [claims, sources, List.map_flatMap]
  have hbnd := checked_keys_sources s (claims txs) keyOf history hw hb ha hc hkeys
  obtain ⟨remaining, hr, _⟩ := hbnd.2.2.1
  rw [hs] at hr
  cases hi : insertFresh s (outputs txs) with
  | none => exact False.elim (hnew hi)
  | some fresh =>
    obtain ⟨after, hinsert⟩ := insertion_exists_same_store s
      { s with unspent := remaining, history := history } fresh (outputs txs) rfl hi
    have hstep : ordinaryStep s txs = some after := by
      simp [ordinaryStep, hbatch, hkeys, hr, hinsert]
    have hp := ordinary_step_preserves s after txs hw hstep
    exact ⟨after, hstep, hp.1, hp.2.2.1, hp.2.2.2.1, hp.2.2.2.2.1, hp.2.1⟩

/- The following are model boundaries. Arithmetic-only fixtures do not assert
signature validity, fee-floor validity, unique identities, or full consensus
acceptance. In particular, zero fees are intentional arithmetic controls. -/
private def arithmeticTx (ins outs : List Nat) (fee : Nat) : Transaction :=
  { inputs := ins.map (fun amount => ⟨⟨0, amount⟩, 0⟩)
    outputs := outs.map (fun amount => ⟨0, amount⟩)
    fee := fee }

/-- Individual u64 checks do not bound the sum across transactions. The empty
fold also illustrates why `checked_sum_exact` requires its accumulator bound. -/
theorem arithmetic_success_boundaries :
    let tx := arithmeticTx [U64_MAX] [U64_MAX] 0
    checkTransaction tx = some U64_MAX ∧
    checkBatch [tx, tx] = some 0 ∧
    value (sources [tx, tx]) = 2 * U64_MAX ∧
    value (outputs [tx, tx]) = 2 * U64_MAX ∧
    U64_MAX < value (sources [tx, tx]) ∧ U64_MAX < value (outputs [tx, tx]) ∧
    sumFeesChecked 0 [] = some 0 ∧
    sumFeesChecked (U64_MAX + 1) [] = some (U64_MAX + 1) ∧
    checkTransaction (arithmeticTx [] [] 0) = some 0 := by decide

/-- Input sum, output sum, final output-plus-fee, equality and aggregate fee
failures are independent controls. Each fee-heavy transaction passes alone. -/
theorem arithmetic_rejection_boundaries :
    checkTransaction (arithmeticTx [U64_MAX, 1] [] 0) = none ∧
    checkTransaction (arithmeticTx [] [U64_MAX, 1] 0) = none ∧
    checkTransaction (arithmeticTx [U64_MAX] [U64_MAX] 1) = none ∧
    checkTransaction (arithmeticTx [2] [1] 0) = none ∧
    checkTransaction (arithmeticTx [U64_MAX] [] U64_MAX) = some U64_MAX ∧
    checkBatch [arithmeticTx [U64_MAX] [] U64_MAX,
      arithmeticTx [U64_MAX] [] U64_MAX] = none := by decide

private def boundaryStore : Store :=
  { unspent := [⟨1, 10⟩, ⟨2, 20⟩]
    retained := [⟨1, 10⟩, ⟨2, 20⟩, ⟨3, 30⟩]
    history := [103] }

private def boundaryTx : Transaction :=
  { inputs := [⟨⟨1, 10⟩, 101⟩], outputs := [⟨4, 7⟩], fee := 3 }

/-- Duplicate/missing/amount-mismatched sources, key reuse, and created-ID
reuse reject. The spent decoy 3 is unavailable as a fresh output ID. A newly
created output cannot fund a later transaction in the same block. -/
theorem identity_rejection_boundaries :
    consume boundaryStore.unspent [⟨1, 10⟩, ⟨1, 10⟩] = none ∧
    consume boundaryStore.unspent [⟨9, 10⟩] = none ∧
    consume boundaryStore.unspent [⟨1, 11⟩] = none ∧
    checkKeys boundaryStore.history [101, 101] = none ∧
    checkKeys boundaryStore.history [103] = none ∧
    insertFresh boundaryStore [⟨4, 7⟩, ⟨4, 8⟩] = none ∧
    insertFresh boundaryStore [⟨3, 1⟩] = none ∧
    ordinaryStep boundaryStore [boundaryTx,
      { inputs := [⟨⟨4, 7⟩, 104⟩], outputs := [⟨5, 7⟩], fee := 0 }] = none ∧
    ¬Unique [⟨1, 10⟩, ⟨1, 11⟩] := by
  unfold Unique
  decide

/-- A fresh key alone supplies no evidence of unspent identity. This is a
countermodel with the correspondence premise deliberately absent, not a claim
of a reachable production exploit. -/
theorem fresh_key_needs_correspondence :
    let s : Store := { unspent := [], retained := [⟨1, 10⟩], history := [99] }
    checkKeys s.history [101] = some [99, 101] ∧
    consume s.unspent [⟨1, 10⟩] = none ∧
    ¬UnspentCorrespondence s (fun id => 100 + id) := by
  refine ⟨by decide, by decide, ?_⟩
  intro h
  have bad := (h 1 10 (by rfl)).2 (by decide)
  cases bad

/-- A valid finite ordinary update keeps the untouched economic row, the spent
decoy and the old history while consuming exactly ten and creating seven. -/
theorem valid_update_boundary :
    let after : Store :=
      { unspent := [⟨4, 7⟩, ⟨2, 20⟩]
        retained := [⟨4, 7⟩, ⟨1, 10⟩, ⟨2, 20⟩, ⟨3, 30⟩]
        history := [103, 101] }
    ordinaryStep boundaryStore [boundaryTx] = some after ∧
    lookup 2 after.unspent = some 20 ∧ lookup 1 after.unspent = none ∧
    lookup 3 after.retained = some 30 ∧ 103 ∈ after.history ∧ 101 ∈ after.history ∧
    value boundaryStore.unspent = 30 ∧ value after.unspent = 27 ∧
    fees [boundaryTx] = 3 := by decide

end Botho.Supply.Transactions
