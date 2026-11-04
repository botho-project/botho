/-
Structured block supply composition (#1895). Status: proven (model), under
explicit cryptographic/ledger correspondence parameters, not production
equivalence or schedule reachability. Source baseline: 58be9e9e.

Reuses Supply.Accounting, Supply.Transactions and Lottery.Conservation.
The exact small emission path mirrors cluster-tax/src/monetary.rs; current
Rust spells saturated multiplication as checked_mul(...).unwrap_or(MAX).
No claim here proves the complete reward schedule, CLSAG, hashes, curves,
LMDB atomicity, output recoverability, or arbitrary max-height Rust calls.
Mined/burned counters are Nat additions here: their Rust u128 correspondence
holds on the derived reachable invariant domain, not arbitrary successful
model states with overflowing counters.
-/
import Botho.Supply.Transactions
import Botho.Lottery.Conservation

namespace Botho.Supply.Block

open Botho.Lottery
open Botho.Supply.Transactions

/-- `mainnet_policy().halving_interval`, used by MintingTx on every current network. -/
def productionInterval : Nat := 6307200

/-- Clamp before the u32 cast, exactly as the Rust emission path. -/
def emissionEpoch (height interval : Nat) : Nat := minN (height / interval) U32_MAX

/-- The explicit cast is included; the clamp will prove it lossless. -/
def emissionBps (height interval : Nat) : Nat :=
  if interval = 0 then 0
  else minN (satMul U32_MAX (emissionEpoch height interval % 2 ^ 32) 1000) 5000

/-- Product and quotient are u128 in Rust, then cast to u64. -/
def emission (height reward interval : Nat) : Nat :=
  truncU64 (reward * emissionBps height interval / 10000)

def coinbaseAmount (height reward interval : Nat) : Nat :=
  satSub reward (emission height reward interval)

private theorem minN_right (a b : Nat) : minN a b ≤ b := by
  unfold minN
  split <;> omega

/-- The u32 clamp/cast, u128 product and u64 quotient/cast are all exact on
typed inputs, and splitting reward into miner payout plus emission is exact.
Height and interval widths state the Rust input domain, not a reward schedule. -/
theorem emission_exact (height reward interval : Nat)
    (_hh : height ≤ U64_MAX) (hr : reward ≤ U64_MAX) (_hi : interval ≤ U64_MAX) :
    emissionEpoch height interval ≤ U32_MAX ∧
    emissionEpoch height interval % 2 ^ 32 = emissionEpoch height interval ∧
    emissionBps height interval ≤ 5000 ∧
    reward * emissionBps height interval ≤ U128_MAX ∧
    emission height reward interval = reward * emissionBps height interval / 10000 ∧
    emission height reward interval ≤ reward ∧
    coinbaseAmount height reward interval + emission height reward interval = reward := by
  have he : emissionEpoch height interval ≤ U32_MAX := minN_right (height / interval) U32_MAX
  have hc : emissionEpoch height interval % 2 ^ 32 = emissionEpoch height interval := by
    apply Nat.mod_eq_of_lt
    have hu : U32_MAX + 1 = 2 ^ 32 := by decide
    omega
  have hb : emissionBps height interval ≤ 5000 := by
    unfold emissionBps
    split
    · omega
    · exact minN_right _ _
  have hp := Nat.mul_le_mul hr hb
  have hw : U64_MAX * 5000 ≤ U128_MAX := by decide
  have hq : reward * emissionBps height interval / 10000 ≤ reward := by
    have hm := Nat.mul_le_mul_left reward (show emissionBps height interval ≤ 10000 by omega)
    have hd := Nat.div_le_div_right hm (c := 10000)
    simpa using hd
  have hcast : emission height reward interval = reward * emissionBps height interval / 10000 := by
    unfold emission truncU64
    apply Nat.mod_eq_of_lt
    unfold U64_MAX at hr
    omega
  have hproduct : reward * emissionBps height interval ≤ U128_MAX := Nat.le_trans hp hw
  have hemit : emission height reward interval ≤ reward := by rw [hcast]; exact hq
  refine ⟨he, hc, hb, hproduct, hcast, hemit, ?_⟩
  unfold coinbaseAmount satSub
  rw [ite_eq_left (by omega)]
  omega

/-- One shared outpoint encoding for every output class. Class labels never
partition the identity space; hash/index collisions remain representable. -/
structure Outpoint where
  hash : Bytes
  index : Nat
  deriving DecidableEq, Repr

def Outpoint.bytes (p : Outpoint) : Bytes := p.hash ++ leBytes 4 p.index

def Outpoint.id (p : Outpoint) : Nat := ofLeBytes p.bytes

def Outpoint.valid (p : Outpoint) : Bool := p.hash.length == 32 && p.index ≤ U32_MAX

structure Context where
  baseIndex : Nat
  tweak : Bytes
  deriving DecidableEq, Repr

inductive StoredContext where
  | direct (index : Nat)
  | lottery (context : Context)
  deriving DecidableEq, Repr

def StoredContext.derivation : StoredContext → Context
  | .direct i => ⟨i, zeros 32⟩
  | .lottery c => c

/-- Nonmonetary output data, including exact tag bytes. Tags are carried,
never converted into a second burn or amount in this module. -/
structure Fields where
  target : Bytes
  publicKey : Bytes
  ciphertext : Option Bytes
  memo : Option Bytes
  tags : Bytes
  deriving DecidableEq, Repr

structure Row where
  outpoint : Outpoint
  amount : Nat
  createdAt : Nat
  fields : Fields
  context : StoredContext
  deriving DecidableEq, Repr

def Row.entry (r : Row) : Entry := ⟨r.outpoint.id, r.amount⟩
def Row.candidate (r : Row) (factor : Nat) : Utxo :=
  ⟨r.outpoint.bytes, r.amount, r.createdAt, factor⟩

/-- Full retained rows in database key order, separate from economic stock. -/
structure State where
  store : Store
  rows : List Row
  tip : Header
  genesis : Bytes
  pool : Option Nat
  mined : Nat
  burned : Nat

/-- Scalar monetary projection. A missing pool is allowed by the prefix check
only at genesis, matching ProductionView::lottery_pool. -/
def monetary (s : State) : Botho.Supply.State :=
  ⟨value s.store.unspent, s.pool.getD 0, s.mined, s.burned⟩

/-- Exact finite domain/identity/amount agreement, including spent decoys.
Database ordering is tested separately; no economic-unspent candidate filter. -/
def candidateAgreement (s : State) : Bool :=
  s.rows.all (fun r => r.outpoint.valid && r.amount ≤ U64_MAX &&
    lookup r.outpoint.id s.store.retained == some r.amount) &&
  s.store.retained.all (fun e => s.rows.any (fun r => r.entry == e)) &&
  (s.rows.map (fun r => r.outpoint.id)).Nodup &&
  s.rows.Pairwise (fun a b => bytesLt a.outpoint.bytes b.outpoint.bytes = true)

/-- Public transaction bytes, visible monetary fields and separately supplied
real-source witnesses. Encoding correspondence binds the visible fields;
CLSAGBinding binds the hidden source identity. The model checks output IDs. -/
structure Ordinary where
  encoded : Bytes
  tx : Transaction
  fields : List Fields
  deriving DecidableEq, Repr

structure Summary where
  fees : Nat
  distributed : Nat
  burned : Nat
  seed : Bytes
  deriving DecidableEq, Repr

structure Record where
  ordinal : Nat
  winner : Outpoint
  amount : Nat
  target : Bytes
  publicKey : Bytes
  ciphertext : Option Bytes
  context : Context
  deriving DecidableEq, Repr

structure Wire where
  winner : Outpoint
  amount : Nat
  target : Bytes
  publicKey : Bytes
  ciphertext : Option Bytes
  deriving DecidableEq, Repr

structure Domain where
  genesis : Bytes
  parent : Bytes
  height : Nat
  ordinaryRoot : Bytes
  manifest : Bytes
  ordinal : Nat
  amount : Nat
  deriving DecidableEq, Repr

/-- A data-only cryptographic witness. The checks bind its entire source and
domain; DerivationCorrespondence separately binds target/context to Rust derive. -/
structure DerivationWitness where
  source : Row
  domain : Domain
  target : Bytes
  context : Context
  deriving DecidableEq, Repr

/-- These functions model cryptographic/encoding computations, not monetary
answers. Their production correspondence is an explicit external obligation. -/
structure Primitives where
  lottery : Prims
  transactionHash : Bytes → Bytes
  ordinaryRoot : List Bytes → Bytes
  manifest : List Winner → Bytes

structure Input where
  header : Header
  mintHeight : Nat
  reward : Nat
  policyInterval : Nat
  mintingEncoded : Bytes
  coinbaseFields : Fields
  candidateFactors : List Nat
  transactions : List Ordinary
  summary : Summary
  records : List Record
  wires : List Wire
  witnesses : List DerivationWitness

inductive Error where
  | inputWidth | mintingHeight | emissionPolicy | missingPool | candidateDomain
  | transaction | seedOrDrawing (reason : DrawingError) | summary | count
  | ordinal | winner | amount | wireFields | source | domain | derivation
  | ordinaryOutput | keyImage | consumedSource | createdIdentity
  deriving DecidableEq, Repr

def transactions (b : Input) : List Transaction := b.transactions.map Ordinary.tx

def reads (s : State) (b : Input) : Reads :=
  ⟨s.pool.getD 0, s.tip, (s.rows.zip b.candidateFactors).map (fun (r, f) => r.candidate f)⟩

def drawBlock (b : Input) : DrawBlock :=
  ⟨b.header.height, b.header.prevBlockHash, (transactions b).map Transaction.fee,
    b.reward, emission b.mintHeight b.reward productionInterval⟩

/-- Minting/header equality and the fixed production policy make emission a
computed value, never a caller-supplied DrawBlock.emissionShare. -/
def checkPrefix (s : State) (b : Input) : Except Error Unit :=
  if b.header.height > U64_MAX || b.reward > U64_MAX || s.tip.height > U64_MAX then
    .error .inputWidth
  else if b.mintHeight != b.header.height then .error .mintingHeight
  else if b.policyInterval != productionInterval then .error .emissionPolicy
  else if s.pool.isNone && s.tip.height != 0 then .error .missingPool
  else if !candidateAgreement s || b.candidateFactors.length != s.rows.length then .error .candidateDomain
  else .ok ()

def checkSummary (s : Summary) (d : BlockLotteryResult) : Except Error Unit :=
  if s.fees = d.totalFees ∧ s.distributed = d.poolAmount ∧
      s.burned = d.burnAmount ∧ s.seed = d.seed then .ok () else .error .summary

def ordinaryRoot (P : Primitives) (b : Input) : Bytes :=
  P.ordinaryRoot (b.transactions.map (fun tx => P.transactionHash tx.encoded))

def awardDomain (P : Primitives) (s : State) (b : Input)
    (winners : List Winner) (ordinal amount : Nat) : Domain :=
  ⟨s.genesis, b.header.prevBlockHash, b.header.height, ordinaryRoot P b,
    P.manifest winners, ordinal, amount⟩

def recordWire (r : Record) : Wire :=
  ⟨r.winner, r.amount, r.target, r.publicKey, r.ciphertext⟩

def recordWidths (r : Record) : Bool :=
  r.winner.valid && r.ordinal < 4 && r.amount ≤ U64_MAX &&
  r.target.length == 32 && r.publicKey.length == 32 &&
  r.context.baseIndex ≤ U32_MAX && r.context.tweak.length == 32 &&
  r.ciphertext.all (fun c => c.length == 1088)

/-- Ordinary output IDs are computed from the real transaction-hash domain and
index, then checked against the semantic IDs used by Transactions. -/
def ordinaryRows (hash : Bytes) (height index : Nat) :
    List Entry → List Fields → Except Error (List Row)
  | [], [] => .ok []
  | e :: es, f :: fs =>
    let outpoint : Outpoint := ⟨hash, index⟩
    if !outpoint.valid || e.id != outpoint.id then .error .ordinaryOutput
    else do
      let rest ← ordinaryRows hash height (index + 1) es fs
      pure (⟨outpoint, e.amount, height, f, .direct index⟩ :: rest)
  | _, _ => .error .ordinaryOutput

def allOrdinaryRows (P : Primitives) (height : Nat) : List Ordinary → Except Error (List Row)
  | [] => .ok []
  | tx :: txs => do
    let rows ← ordinaryRows (P.transactionHash tx.encoded) height 0 tx.tx.outputs tx.fields
    let rest ← allOrdinaryRows P height txs
    pure (rows ++ rest)

def coinbaseRow (P : Primitives) (b : Input) : Row :=
  { outpoint := ⟨headerHash P.lottery b.header, 0⟩
    amount := coinbaseAmount b.mintHeight b.reward productionInterval
    createdAt := b.header.height, fields := b.coinbaseFields, context := .direct 0 }

/-- One complete ordered structural binding. A supplied derivation witness
cannot choose its source/domain or payout amount independently of the draw. -/
def checkAward (P : Primitives) (s : State) (b : Input) (winners : List Winner)
    (ordinal : Nat) (w : Winner) (r : Record) (wire : Wire) (v : DerivationWitness) :
    Except Error Row :=
  if !recordWidths r then .error .inputWidth
  else if r.ordinal != ordinal then .error .ordinal
  else if r.winner.bytes != w.utxoId then .error .winner
  else if r.amount != w.payout then .error .amount
  else if wire != recordWire r then .error .wireFields
  else
    match s.rows.find? (fun source => source.outpoint.bytes == w.utxoId) with
    | none => .error .source
    | some source =>
      if v.source != source || source.createdAt ≥ b.header.height then .error .source
      else if v.domain != awardDomain P s b winners ordinal w.payout then .error .domain
      else if r.target != v.target || r.context != v.context ||
          r.publicKey != source.fields.publicKey || r.ciphertext != source.fields.ciphertext then
        .error .derivation
      else .ok ⟨⟨headerHash P.lottery b.header, 1 + ordinal⟩,
        wire.amount, b.header.height,
        ⟨wire.target, wire.publicKey, wire.ciphertext, none, source.fields.tags⟩,
        .lottery r.context⟩

def checkAwards (P : Primitives) (s : State) (b : Input) (winners : List Winner) :
    Nat → List Winner → List Record → List Wire → List DerivationWitness → Except Error (List Row)
  | _, [], [], [], [] => .ok []
  | i, w :: ws, r :: rs, wire :: wires, v :: vs => do
    let row ← checkAward P s b winners i w r wire v
    let rest ← checkAwards P s b winners (i + 1) ws rs wires vs
    pure (row :: rest)
  | _, _, _, _, _ => .error .count

private theorem ordinaryRows_value (hash : Bytes) (height index : Nat)
    (es : List Entry) (fs : List Fields) (rows : List Row)
    (h : ordinaryRows hash height index es fs = .ok rows) :
    value (rows.map Row.entry) = value es := by
  induction es generalizing index fs rows with
  | nil =>
    cases fs with
    | nil => cases h; rfl
    | cons => simp [ordinaryRows] at h
  | cons e es ih =>
    cases fs with
    | nil => simp [ordinaryRows] at h
    | cons f fs =>
      simp only [ordinaryRows] at h
      split at h
      · cases h
      · cases hr : ordinaryRows hash height (index + 1) es fs with
        | error err => simp [hr, bind, Except.bind] at h
        | ok rest =>
          simp only [hr, bind, Except.bind, pure, Except.pure, Except.ok.injEq] at h
          subst rows
          have hv := ih (index + 1) fs rest hr
          simp only [List.map_cons, value, List.sum_cons, Row.entry] at *
          omega

private theorem allOrdinaryRows_value (P : Primitives) (height : Nat)
    (txs : List Ordinary) (rows : List Row) (h : allOrdinaryRows P height txs = .ok rows) :
    value (rows.map Row.entry) = value (outputs (txs.map Ordinary.tx)) := by
  induction txs generalizing rows with
  | nil => cases h; rfl
  | cons tx txs ih =>
    simp only [allOrdinaryRows] at h
    cases hr : ordinaryRows (P.transactionHash tx.encoded) height 0 tx.tx.outputs tx.fields with
    | error err => simp [hr, bind, Except.bind] at h
    | ok front =>
      cases ht : allOrdinaryRows P height txs with
      | error err => simp [hr, ht, bind, Except.bind] at h
      | ok rest =>
        simp only [hr, ht, bind, Except.bind, pure, Except.pure, Except.ok.injEq] at h
        subst rows
        have hf := ordinaryRows_value _ _ _ _ _ front hr
        have hh := ih rest ht
        simp only [List.map_append, value, List.sum_append, outputs,
          List.map_cons, List.flatMap_cons] at *
        omega

private theorem checkAward_amount (P : Primitives) (s : State) (b : Input)
    (winners : List Winner) (i : Nat) (w : Winner) (r : Record) (wire : Wire)
    (v : DerivationWitness) (row : Row)
    (h : checkAward P s b winners i w r wire v = .ok row) : row.amount = w.payout := by
  unfold checkAward at h
  split at h
  · cases h
  · split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · rename_i ha
          split at h
          · cases h
          · rename_i hw
            split at h
            · cases h
            · split at h
              · cases h
              · split at h
                · cases h
                · split at h
                  · cases h
                  · cases h
                    have he : wire = recordWire r := by simpa using hw
                    have ham : r.amount = w.payout := by simpa using ha
                    simpa [he, recordWire] using ham

private theorem checkAwards_value (P : Primitives) (s : State) (b : Input)
    (all ws : List Winner) (i : Nat) (rs : List Record) (wires : List Wire)
    (vs : List DerivationWitness) (rows : List Row)
    (h : checkAwards P s b all i ws rs wires vs = .ok rows) :
    value (rows.map Row.entry) = totalPaid ws ∧ rows.length = ws.length ∧
      rs.length = ws.length ∧ wires.length = ws.length ∧ vs.length = ws.length := by
  induction ws generalizing i rs wires vs rows with
  | nil =>
    cases rs <;> cases wires <;> cases vs <;> simp [checkAwards] at h
    cases h
    simp [value, totalPaid]
  | cons w ws ih =>
    cases rs with
    | nil => cases h
    | cons r rs =>
      cases wires with
      | nil => cases h
      | cons wire wires =>
        cases vs with
        | nil => cases h
        | cons v vs =>
          simp only [checkAwards] at h
          cases ha : checkAward P s b all i w r wire v with
          | error err => simp [ha, bind, Except.bind] at h
          | ok row =>
            cases ht : checkAwards P s b all (i + 1) ws rs wires vs with
            | error err => simp [ha, ht, bind, Except.bind] at h
            | ok rest =>
              simp only [ha, ht, bind, Except.bind, pure, Except.pure, Except.ok.injEq] at h
              subst rows
              have hh := ih (i + 1) rs wires vs rest ht
              have hm := checkAward_amount P s b all i w r wire v row ha
              simp only [value, List.map_cons, List.sum_cons, totalPaid, Row.entry,
                List.length_cons] at *
              omega

structure Prepared where
  draw : BlockLotteryResult
  accounting : Accounting
  ordinary : List Row
  payouts : List Row

def created (P : Primitives) (b : Input) (p : Prepared) : List Row :=
  coinbaseRow P b :: p.ordinary ++ p.payouts

/-- All checks are performed from the common prestate and block data. The
writer insertion order is coinbase, ordinary outputs, then ordered payouts. -/
def prepare (P : Primitives) (s : State) (b : Input) : Except Error Prepared := do
  let _ ← checkPrefix s b
  if !(coinbaseRow P b).outpoint.valid then .error .inputWidth
  else
    match checkBatch (transactions b) with
    | none => .error .transaction
    | some _ =>
      match drawing P.lottery (reads s b) (drawBlock b) with
      | .error err => .error (.seedOrDrawing err)
      | .ok (draw, accounting) => do
        let _ ← checkSummary b.summary draw
        if draw.winners.length > 4 then .error .count
        else do
          let ordinary ← allOrdinaryRows P b.header.height b.transactions
          let payouts ← checkAwards P s b draw.winners 0 draw.winners b.records b.wires b.witnesses
          pure ⟨draw, accounting, ordinary, payouts⟩

private theorem prepare_facts (P : Primitives) (s : State) (b : Input) (p : Prepared)
    (h : prepare P s b = .ok p) :
    checkPrefix s b = .ok () ∧ (coinbaseRow P b).outpoint.valid = true ∧
    (∃ F, checkBatch (transactions b) = some F) ∧
    drawing P.lottery (reads s b) (drawBlock b) = .ok (p.draw, p.accounting) ∧
    checkSummary b.summary p.draw = .ok () ∧ p.draw.winners.length ≤ 4 ∧
    allOrdinaryRows P b.header.height b.transactions = .ok p.ordinary ∧
    checkAwards P s b p.draw.winners 0 p.draw.winners b.records b.wires b.witnesses = .ok p.payouts := by
  unfold prepare at h
  cases hp : checkPrefix s b with
  | error err => simp [hp, bind, Except.bind] at h
  | ok u =>
    cases u
    simp only [hp, bind, Except.bind] at h
    split at h
    · cases h
    · rename_i hv
      split at h
      · cases h
      · rename_i F hf
        split at h
        · cases h
        · rename_i draw acc hd
          cases hs : checkSummary b.summary draw with
          | error err => simp [hs] at h
          | ok u =>
            cases u
            simp only [hs] at h
            split at h
            · cases h
            · rename_i hn
              cases ho : allOrdinaryRows P b.header.height b.transactions with
              | error err => simp [ho] at h
              | ok ordinary =>
                cases ha : checkAwards P s b draw.winners 0 draw.winners b.records b.wires b.witnesses with
                | error err => simp [ho, ha] at h
                | ok payouts =>
                  simp only [ho, ha,
                    pure, Except.pure, Except.ok.injEq] at h
                  subst p
                  dsimp only
                  refine ⟨rfl, ?_, ⟨F, hf⟩, hd, ?_, ?_, rfl, ?_⟩ <;> simp_all

private theorem prefix_facts (s : State) (b : Input) (h : checkPrefix s b = .ok ()) :
    b.header.height ≤ U64_MAX ∧ b.reward ≤ U64_MAX ∧ s.tip.height ≤ U64_MAX ∧
    b.mintHeight = b.header.height ∧ b.policyInterval = productionInterval ∧
    candidateAgreement s = true ∧ b.candidateFactors.length = s.rows.length := by
  unfold checkPrefix at h
  split at h
  · cases h
  · rename_i hw
    split at h
    · cases h
    · rename_i hh
      split at h
      · cases h
      · rename_i hi
        split at h
        · cases h
        · split at h
          · cases h
          · simp_all

/-- Canonical payouts are exactly the created payout value, including no
winner, underfilled and remainder-to-last cases. Counts are bounded, not
assumed equal to four. Full wire/record checks are in prepare/checkAwards. -/
theorem prepared_payouts (P : Primitives) (s : State) (b : Input) (p : Prepared)
    (h : prepare P s b = .ok p) :
    value (p.payouts.map Row.entry) = p.draw.poolAmount ∧
    p.draw.poolAmount ≤ p.accounting.payout ∧ p.accounting.payout ≤ p.accounting.available ∧
    p.payouts.length = p.draw.winners.length ∧ p.payouts.length ≤ 4 ∧
    b.records.length = p.payouts.length ∧ b.wires.length = p.payouts.length ∧
    b.witnesses.length = p.payouts.length ∧
    value ((created P b p).map Row.entry) =
      value (outputs (transactions b)) +
      coinbaseAmount b.mintHeight b.reward productionInterval + p.draw.poolAmount := by
  have hf := prepare_facts P s b p h
  have hd := drawing_paid P.lottery (reads s b) (drawBlock b) p.draw p.accounting hf.2.2.2.1
  have ha := checkAwards_value P s b p.draw.winners p.draw.winners 0
    b.records b.wires b.witnesses p.payouts hf.2.2.2.2.2.2.2
  have ho := allOrdinaryRows_value P b.header.height b.transactions p.ordinary hf.2.2.2.2.2.2.1
  refine ⟨ha.1.trans hd.1, hd.2.1, hd.2.2, ha.2.1, by omega, by omega, by omega, by omega, ?_⟩
  simp only [created, List.map_cons, List.map_append, value, List.sum_cons,
    List.sum_append, coinbaseRow, Row.entry] at *
  dsimp only [transactions, value]
  omega

def finiteStep (s : Store) (txs : List Transaction) (newRows : List Entry) : Except Error Store :=
  match checkKeys s.history ((claims txs).map Transactions.Input.key) with
  | none => .error .keyImage
  | some history =>
    match consume s.unspent (sources txs) with
    | none => .error .consumedSource
    | some remaining =>
      match insertFresh { s with unspent := remaining, history := history } newRows with
      | none => .error .createdIdentity
      | some after => .ok after

private theorem finite_facts (s after : Store) (txs : List Transaction) (newRows : List Entry)
    (h : finiteStep s txs newRows = .ok after) :
    ∃ history remaining, checkKeys s.history ((claims txs).map Transactions.Input.key) = some history ∧
      consume s.unspent (sources txs) = some remaining ∧
      insertFresh { s with unspent := remaining, history := history } newRows = some after := by
  unfold finiteStep at h
  split at h
  · cases h
  · rename_i history hk
    split at h
    · cases h
    · rename_i remaining hr
      split at h
      · cases h
      · rename_i result hi
        cases h
        exact ⟨history, remaining, hk, hr, hi⟩


private theorem keys_checked (history keys after : List Nat)
    (h : checkKeys history keys = some after) :
    keys.Nodup ∧ (∀ key ∈ keys, key ∉ history) ∧ after = history ++ keys := by
  unfold checkKeys at h
  split at h
  · simp_all
  · cases h

private theorem finite_preserves (s after : Store) (txs : List Transaction) (newRows : List Entry)
    (hw : WellFormed s) (h : finiteStep s txs newRows = .ok after) :
    WellFormed after ∧
    value after.unspent + value (sources txs) = value s.unspent + value newRows ∧
    value (sources txs) ≤ value s.unspent ∧
    after.retained = newRows.reverse ++ s.retained ∧
    after.history = s.history ++ (claims txs).map Transactions.Input.key ∧
    (∀ id, id ∈ s.retained.map Entry.id →
      lookup id after.retained = lookup id s.retained ∧
      (id ∉ (sources txs).map Entry.id → lookup id after.unspent = lookup id s.unspent)) := by
  obtain ⟨history, remaining, hk, hr, hi⟩ := finite_facts s after txs newRows h
  have hp := consume_insert_preserves s after (sources txs) newRows remaining history hw hr hi
  have hh := (keys_checked _ _ _ hk).2.2
  exact ⟨hp.1, hp.2.1, hp.2.2.1, hp.2.2.2.1, hp.2.2.2.2.1.trans hh, hp.2.2.2.2.2⟩


private theorem finite_correspondence (s after : Store) (txs : List Transaction)
    (newRows : List Entry) (keyOf : Nat → Nat) (hw : WellFormed s)
    (hb : CLSAGBinding s keyOf (claims txs)) (ha : KeyAssociation s keyOf)
    (hc : UnspentCorrespondence s keyOf)
    (h : finiteStep s txs newRows = .ok after)
    (he : IdentityExtension after newRows keyOf) :
    UnspentCorrespondence after keyOf ∧ KeyAssociation after keyOf ∧
    Unique (sources txs) ∧
    (∀ c ∈ claims txs, lookup c.source.id s.unspent = some c.source.amount) := by
  obtain ⟨history, remaining, hk, hr, hi⟩ := finite_facts s after txs newRows h
  have hs : (claims txs).map Transactions.Input.source = sources txs := by
    simp [claims, sources, List.map_flatMap]
  have hav := checked_keys_sources s (claims txs) keyOf history hw hb ha hc hk
  have hcorr := consume_insert_correspondence s after (claims txs) newRows remaining history
    keyOf hw hb ha hc hk (by simpa [hs] using hr) hi he
  exact ⟨hcorr.1, hcorr.2, by simpa [hs] using hav.1, hav.2.1⟩


/-- Recompute the cap using the actual successful parent seed, not the
no-winner result's zero seed. -/
def payoutCap (P : Primitives) (s : State) (b : Input) : Nat :=
  match seed P.lottery (reads s b) (drawBlock b) with
  | .error _ => 0
  | .ok sd => rewardCap
      (lotteryCandidates P.lottery (reads s b).rows b.header.height sd canonicalConfig.draw)
      b.header.height b.reward canonicalConfig

def flow (P : Primitives) (s : State) (b : Input) (p : Prepared) : Flow :=
  ⟨value (sources (transactions b)), value (outputs (transactions b)), fees (transactions b),
    b.reward, emission b.mintHeight b.reward productionInterval, p.draw.poolAmount, payoutCap P s b⟩

def saturationLoss (s : State) (b : Input) (p : Prepared) : Nat :=
  s.pool.getD 0 + emission b.mintHeight b.reward productionInterval + p.accounting.feePool -
    p.accounting.available

private theorem prepared_accounting (P : Primitives) (s : State) (b : Input) (p : Prepared)
    (h : prepare P s b = .ok p) :
    p.accounting = computePoolAccounting (fees (transactions b))
      (emission b.mintHeight b.reward productionInterval) (s.pool.getD 0)
      (payoutCap P s b) canonicalConfig ∧ p.draw.burnAmount = p.accounting.feeBurn := by
  have hf := prepare_facts P s b p h
  obtain ⟨sd, F, hseed, hfees, ha, hd⟩ := candidates_of_postState
    P.lottery (reads s b) (drawBlock b) p.draw p.accounting hf.2.2.2.1
  have hF := checked_sum_exact _ 0 F (by omega) hfees
  have he : F = fees (transactions b) := by simpa [drawBlock, fees] using hF.1
  subst F
  refine ⟨?_, ?_⟩
  · unfold payoutCap
    rw [hseed]
    exact ha
  · rw [hd]
    unfold drawLotteryWinnersSeeded
    split
    · rfl
    · split <;> rfl

private theorem prepared_balance (P : Primitives) (s : State) (b : Input) (p : Prepared)
    (h : prepare P s b = .ok p) :
    value (sources (transactions b)) = value (outputs (transactions b)) + fees (transactions b) ∧
    fees (transactions b) ≤ U64_MAX ∧
    sumFeesChecked 0 (drawBlock b).fees = some (fees (transactions b)) := by
  obtain ⟨F, hb⟩ := (prepare_facts P s b p h).2.2.1
  have hf := batch_balance _ _ hb
  refine ⟨hf.1, by omega, ?_⟩
  unfold checkBatch at hb
  cases hc : checkBalances (transactions b) with
  | none => simp [hc] at hb
  | some u =>
    cases u
    simpa [hc, hf.2.1, drawBlock] using hb

private theorem prepared_height (P : Primitives) (s : State) (b : Input) (p : Prepared)
    (h : prepare P s b = .ok p) :
    b.header.height = s.tip.height + 1 ∧ s.tip.height < U64_MAX ∧
    b.header.height ≤ U64_MAX ∧ b.reward ≤ U64_MAX ∧ b.mintHeight = b.header.height := by
  have hp := prefix_facts s b (prepare_facts P s b p h).1
  obtain ⟨sd, F, hs, _, _, _⟩ := candidates_of_postState
    P.lottery (reads s b) (drawBlock b) p.draw p.accounting (prepare_facts P s b p h).2.2.2.1
  unfold seed at hs
  split at hs
  next hg =>
    have hc := hg.1
    unfold checkedAdd at hc
    split at hc
    · have he := Option.some.inj hc
      dsimp [reads, drawBlock] at he
      exact ⟨he.symm, by omega, hp.1, hp.2.1, hp.2.2.2.1⟩
    · cases hc
  next => cases hs

/-- New rows are inserted in writer order; the full retained view is then
sorted into database order for the next pinned read. Sorting changes no row.
Counter additions are Nat; `step_invariant` derives their u128 bounds from a
reachable prestate. This function alone does not assert overflow-free Rust
execution for arbitrary states. -/
def finish (P : Primitives) (s : State) (b : Input) (p : Prepared) (store : Store) : State :=
  { store
    rows := (s.rows ++ created P b p).mergeSort
      (fun a b => !bytesLt b.outpoint.bytes a.outpoint.bytes)
    tip := b.header, genesis := s.genesis
    pool := some (p.accounting.carryoverAfter p.draw.poolAmount)
    mined := s.mined + b.reward, burned := s.burned + p.draw.burnAmount }

structure Outcome where
  state : State
  plan : Prepared

def step (P : Primitives) (s : State) (b : Input) : Except Error Outcome := do
  let p ← prepare P s b
  let store ← finiteStep s.store (transactions b) ((created P b p).map Row.entry)
  pure ⟨finish P s b p store, p⟩

private theorem step_facts (P : Primitives) (s : State) (b : Input) (o : Outcome)
    (h : step P s b = .ok o) :
    prepare P s b = .ok o.plan ∧
    finiteStep s.store (transactions b) ((created P b o.plan).map Row.entry) = .ok o.state.store ∧
    o.state = finish P s b o.plan o.state.store := by
  unfold step at h
  cases hp : prepare P s b with
  | error err => simp [hp, bind, Except.bind] at h
  | ok p =>
    cases hf : finiteStep s.store (transactions b) ((created P b p).map Row.entry) with
    | error err => simp [hp, hf, bind, Except.bind] at h
    | ok store =>
      simp only [hp, hf, bind, Except.bind, pure, Except.pure,
        Except.ok.injEq] at h
      subst o
      exact ⟨rfl, hf, rfl⟩

/-- The finite output projection equals the existing scalar Supply.apply;
this is derived from checks and insertion, not supplied as an expected state. -/
theorem step_projects (P : Primitives) (s : State) (b : Input) (o : Outcome)
    (hw : WellFormed s.store) (h : step P s b = .ok o) :
    monetary o.state = Botho.Supply.apply (monetary s) (flow P s b o.plan) canonicalConfig ∧
    WellFormed o.state.store ∧
    value (sources (transactions b)) ≤ value s.store.unspent := by
  have hf := step_facts P s b o h
  have hfin := finite_preserves s.store o.state.store (transactions b)
    ((created P b o.plan).map Row.entry) hw hf.2.1
  have hpay := prepared_payouts P s b o.plan hf.1
  have ha := prepared_accounting P s b o.plan hf.1
  refine ⟨?_, hfin.1, hfin.2.2.1⟩
  have hu : value o.state.store.unspent = value s.store.unspent - value (sources (transactions b)) +
      value (outputs (transactions b)) + coinbaseAmount b.mintHeight b.reward productionInterval +
      o.plan.draw.poolAmount := by omega
  have hpool : o.state.pool = some (o.plan.accounting.carryoverAfter o.plan.draw.poolAmount) := by
    rw [hf.2.2]; rfl
  have hm : o.state.mined = s.mined + b.reward := by rw [hf.2.2]; rfl
  have hb : o.state.burned = s.burned + o.plan.draw.burnAmount := by rw [hf.2.2]; rfl
  simp only [monetary, Botho.Supply.apply, flow, ← ha.1, ← ha.2, hu, hpool,
    Option.getD_some, hm, hb, coinbaseAmount]

/-- Arbitrary successful model states retain the exact saturation loss.
The lossless reachable result below derives its disappearance. -/
theorem step_cross_addition (P : Primitives) (s : State) (b : Input) (o : Outcome)
    (h : step P s b = .ok o) :
    value ((created P b o.plan).map Row.entry) + o.state.pool.getD 0 +
      o.plan.draw.burnAmount + saturationLoss s b o.plan =
      value (sources (transactions b)) + b.reward + s.pool.getD 0 := by
  have hf := step_facts P s b o h
  have hb := prepared_balance P s b o.plan hf.1
  have hp := prepared_payouts P s b o.plan hf.1
  have hh := prepared_height P s b o.plan hf.1
  have he := emission_exact b.mintHeight b.reward productionInterval
    (by omega) hh.2.2.2.1 (by decide)
  have hc := drawing_conserves P.lottery (reads s b) (drawBlock b)
    o.plan.draw o.plan.accounting (fees (transactions b)) (prepare_facts P s b o.plan hf.1).2.2.2.1 hb.2.2
  have hpaid := drawing_paid P.lottery (reads s b) (drawBlock b)
    o.plan.draw o.plan.accounting (prepare_facts P s b o.plan hf.1).2.2.2.1
  have hpool : o.state.pool.getD 0 = o.plan.accounting.carryoverAfter o.plan.draw.poolAmount := by
    rw [hf.2.2]; rfl
  dsimp only [reads, drawBlock] at hc
  unfold saturationLoss
  rw [hpool]
  omega

/-- Full stored rows/contexts and every old known key are preserved. The
retained list and database-order row view are representations of the same
finite insertions; no extra row can be hidden in the monetary projection. -/
theorem step_frame (P : Primitives) (s : State) (b : Input) (o : Outcome)
    (hw : WellFormed s.store) (h : step P s b = .ok o) :
    o.state.rows.Perm (s.rows ++ created P b o.plan) ∧
    o.state.store.retained = ((created P b o.plan).map Row.entry).reverse ++ s.store.retained ∧
    o.state.store.history = s.store.history ++ (claims (transactions b)).map Transactions.Input.key ∧
    (∀ id, id ∈ s.store.retained.map Entry.id →
      lookup id o.state.store.retained = lookup id s.store.retained ∧
      (id ∉ (sources (transactions b)).map Entry.id →
        lookup id o.state.store.unspent = lookup id s.store.unspent)) := by
  have hf := step_facts P s b o h
  have hp := finite_preserves s.store o.state.store (transactions b)
    ((created P b o.plan).map Row.entry) hw hf.2.1
  refine ⟨?_, hp.2.2.2.1, hp.2.2.2.2.1, hp.2.2.2.2.2⟩
  rw [hf.2.2]
  exact List.mergeSort_perm _ _

/-- Derived financial and finite-state invariant. These are conclusions of
reachability, never fields of the successor constructor. -/
structure Invariant (s : State) : Prop where
  finite : WellFormed s.store
  conserved : Conserved (monetary s)
  height : s.tip.height ≤ U64_MAX
  issuance : s.mined ≤ s.tip.height * U64_MAX
  mined : s.mined ≤ U128_MAX
  burned : s.burned ≤ U128_MAX
  pool : s.pool.getD 0 ≤ U128_MAX

private theorem fee_member_bound (xs : List Nat) (x : Nat) (hx : x ∈ xs) : x ≤ xs.sum := by
  induction xs with
  | nil => simp at hx
  | cons a xs ih =>
    simp only [List.mem_cons] at hx
    simp only [List.sum_cons]
    rcases hx with rfl | hx
    · omega
    · have := ih hx; omega

/-- Exact Rust output-width facts, reusing the landed draw-width proof.
The pre-pool bound states the arbitrary-state Rust u128 input domain;
reachable steps derive it rather than adding it to the constructor. -/
theorem prepared_widths (P : Primitives) (s : State) (b : Input) (p : Prepared)
    (h : prepare P s b = .ok p) (hpool : s.pool.getD 0 ≤ U128_MAX) :
    emission b.mintHeight b.reward productionInterval ≤ b.reward ∧
    coinbaseAmount b.mintHeight b.reward productionInterval ≤ U64_MAX ∧
    fees (transactions b) ≤ U64_MAX ∧ p.accounting.available ≤ U128_MAX ∧
    p.accounting.payout ≤ U64_MAX ∧ (∀ w ∈ p.draw.winners, w.payout ≤ U64_MAX) ∧
    totalPaid p.draw.winners ≤ U64_MAX := by
  have hh := prepared_height P s b p h
  have hb := prepared_balance P s b p h
  have he := emission_exact b.mintHeight b.reward productionInterval (by omega) hh.2.2.2.1 (by decide)
  have hf : ∀ f ∈ (drawBlock b).fees, f ≤ U64_MAX := by
    intro f hm
    have hsum := checked_sum_exact _ 0 (fees (transactions b)) (by omega) hb.2.2
    have hmem := fee_member_bound (drawBlock b).fees f hm
    omega
  have hw := drawing_widths P.lottery (reads s b) (drawBlock b) p.draw p.accounting
    (fees (transactions b)) (prepare_facts P s b p h).2.2.2.1 hb.2.2 hh.2.2.2.1
    (by dsimp [drawBlock]; omega) hf hpool
  exact ⟨he.2.2.2.2.2.1, by omega, hb.2.1, hw.2.1, hw.2.2.1, hw.2.2.2.1, hw.2.2.2.2⟩

/-- Reachable-state budgets discharge pool saturation and counter overflow.
The checked successor supplies the strict old-height bound; this theorem does
not claim the earlier Rust header expression is panic-free at arbitrary max height. -/
theorem step_invariant (P : Primitives) (s : State) (b : Input) (o : Outcome)
    (hs : Invariant s) (h : step P s b = .ok o) :
    Invariant o.state ∧ saturationLoss s b o.plan = 0 ∧
    value ((created P b o.plan).map Row.entry) + o.state.pool.getD 0 + o.plan.draw.burnAmount =
      value (sources (transactions b)) + b.reward + s.pool.getD 0 := by
  have hf := step_facts P s b o h
  have hh := prepared_height P s b o.plan hf.1
  have hb := prepared_balance P s b o.plan hf.1
  have he := emission_exact b.mintHeight b.reward productionInterval (by omega) hh.2.2.2.1 (by decide)
  have hp := step_projects P s b o hs.finite h
  have hpay := prepared_payouts P s b o.plan hf.1
  have ha := prepared_accounting P s b o.plan hf.1
  have hbudget := issuance_step_fits s.tip.height s.mined b.reward hh.2.1 hs.issuance hh.2.2.2.1
  have htx : (flow P s b o.plan).spent = (flow P s b o.plan).outputs + (flow P s b o.plan).fees := hb.1
  have hspent : (flow P s b o.plan).spent ≤ (monetary s).unspent := hp.2.2
  have hem : (flow P s b o.plan).emission ≤ (flow P s b o.plan).reward := he.2.2.2.2.2.1
  have hfee : (flow P s b o.plan).fees ≤ U64_MAX := hb.2.1
  have hwidth : (monetary s).mined + (flow P s b o.plan).reward ≤ U128_MAX := hbudget.2
  have hdist : (flow P s b o.plan).distributed ≤
      (computePoolAccounting (flow P s b o.plan).fees (flow P s b o.plan).emission
        (monetary s).pool (flow P s b o.plan).cap canonicalConfig).available := by
    dsimp only [flow, monetary]
    rw [← ha.1]
    omega
  have hcon := apply_conserves (monetary s) (flow P s b o.plan) canonicalConfig
    hs.conserved hspent htx hem hfee (by decide) hwidth hdist
  rw [← hp.1] at hcon
  have hcounter := counters_fit (monetary s) (flow P s b o.plan) canonicalConfig
    hs.conserved hspent htx hfee (by decide) hwidth
  rw [← hp.1] at hcounter
  have hin := pool_inflow_fits (monetary s) (flow P s b o.plan) canonicalConfig
    hs.conserved hspent htx hem hfee (by decide) hwidth
  have hraw : s.pool.getD 0 + emission b.mintHeight b.reward productionInterval +
      o.plan.accounting.feePool ≤ U128_MAX := by
    rw [ha.1]
    exact hin
  have hnoloss := drawing_conserves_no_loss P.lottery (reads s b) (drawBlock b)
    o.plan.draw o.plan.accounting (fees (transactions b))
    (prepare_facts P s b o.plan hf.1).2.2.2.1 hb.2.2 hraw
  have hw := drawing_widths P.lottery (reads s b) (drawBlock b)
    o.plan.draw o.plan.accounting (fees (transactions b))
    (prepare_facts P s b o.plan hf.1).2.2.2.1 hb.2.2 hh.2.2.2.1 (by
      dsimp [drawBlock]
      omega) (by
        intro f hm
        have hmem := fee_member_bound (drawBlock b).fees f hm
        have hsum := checked_sum_exact _ 0 (fees (transactions b)) (by omega) hb.2.2
        omega) hs.pool
  have hpool : o.state.pool.getD 0 ≤ U128_MAX := by
    have hp' : o.state.pool.getD 0 = o.plan.accounting.carryoverAfter o.plan.draw.poolAmount := by
      rw [hf.2.2]; rfl
    rw [hp']
    unfold Accounting.carryoverAfter satSub
    split <;> omega
  have hheight : o.state.tip.height = b.header.height := by rw [hf.2.2]; rfl
  have hmined : o.state.mined = s.mined + b.reward := by rw [hf.2.2]; rfl
  have hloss : saturationLoss s b o.plan = 0 := hnoloss.1
  have hhnew : o.state.tip.height ≤ U64_MAX := by rw [hheight]; exact hh.2.2.1
  have hinew : o.state.mined ≤ o.state.tip.height * U64_MAX := by
    rw [hmined, hheight, hh.1]
    exact hbudget.1
  refine ⟨⟨hp.2.1, hcon, hhnew, hinew, hcounter.1, hcounter.2, hpool⟩, hloss, ?_⟩
  have hc := step_cross_addition P s b o h
  omega

/-- The actual fresh-ledger path stores genesis and zero metadata directly;
it creates no zero-valued coinbase or other output row. -/
def genesis (P : Primitives) (header : Header) : State :=
  let tip := { header with height := 0 }
  { store := ⟨[], [], []⟩, rows := [], tip, genesis := headerHash P.lottery tip,
    pool := none, mined := 0, burned := 0 }

theorem genesis_invariant (P : Primitives) (header : Header) (keyOf : Nat → Nat) :
    Invariant (genesis P header) ∧ KeyAssociation (genesis P header).store keyOf ∧
    UnspentCorrespondence (genesis P header).store keyOf ∧
    (genesis P header).store.unspent = [] ∧ (genesis P header).store.retained = [] ∧
    (genesis P header).store.history = [] ∧ (genesis P header).rows = [] ∧
    (genesis P header).tip.height = 0 ∧ (genesis P header).mined = 0 ∧
    (genesis P header).burned = 0 ∧ (genesis P header).pool.getD 0 = 0 := by
  have hw : WellFormed (genesis P header).store := by
    refine ⟨by simp [genesis, Unique], by simp [genesis, Unique], ?_⟩
    intro id amount h
    cases h
  refine ⟨⟨hw, ?_, ?_, ?_, ?_, ?_, ?_⟩, ?_, ?_, by rfl, by rfl, by rfl,
    by rfl, by rfl, by rfl, by rfl, by rfl⟩
  all_goals simp [genesis, monetary, value, Conserved, KeyAssociation, UnspentCorrespondence, lookup]

/-- External primitive/data correspondences. Each field concerns a named
implementation boundary, never a desired conservation equality or width. -/
structure Correspondences where
  ordinaryEncoding : Bytes → Transaction → List Fields → Prop
  mintingEncoding : Bytes → Nat → Nat → Fields → Prop
  hashImplementations : Primitives → Prop
  outpointEncoding : Outpoint → Nat → Prop
  authenticatedRow : Bytes → Header → Row → Prop
  candidateFactorRead : Header → List Row → List Nat → Prop
  derivation : Row → Domain → Bytes → Context → Prop
  recordEncoding : Record → Prop
  bodyCommitment : Bytes → Bytes → List Bytes → List Record → Summary → Prop
  contextDomainAgreement : Store → List Row → Prop
  rewardSchedule : Nat → Nat → Nat → Prop

/-- These are trusted source/encoding/curve/policy statements, separate from
the checked structural bindings and monetary results. Record derivation takes
the full checked source and domain, including inherited tags/context. -/
def TrustedInputs (C : Correspondences) (P : Primitives) (s : State) (b : Input) : Prop :=
  C.hashImplementations P ∧
  (∀ outpoint, outpoint.valid = true → C.outpointEncoding outpoint outpoint.id) ∧
  (∀ tx ∈ b.transactions, C.ordinaryEncoding tx.encoded tx.tx tx.fields) ∧
  C.mintingEncoding b.mintingEncoded b.mintHeight b.reward b.coinbaseFields ∧
  (∀ row ∈ s.rows, C.authenticatedRow s.genesis s.tip row) ∧
  C.candidateFactorRead s.tip s.rows b.candidateFactors ∧
  (∀ v ∈ b.witnesses, C.derivation v.source v.domain v.target v.context) ∧
  (∀ r ∈ b.records, C.recordEncoding r) ∧
  C.bodyCommitment b.header.txRoot b.mintingEncoded (b.transactions.map Ordinary.encoded) b.records b.summary ∧
  C.contextDomainAgreement s.store s.rows ∧
  C.rewardSchedule b.header.height s.mined b.reward

/-- Model reachability has only genesis, successful structured steps and named
trust/identity-extension premises. Conservation and width budgets are derived. -/
inductive Reachable (C : Correspondences) (P : Primitives) (keyOf : Nat → Nat) : State → Prop where
  | genesis (header : Header) : Reachable C P keyOf (genesis P header)
  | next (s : State) (b : Input) (o : Outcome)
      (previous : Reachable C P keyOf s) (checked : step P s b = .ok o)
      (trusted : TrustedInputs C P s b)
      (binding : CLSAGBinding s.store keyOf (claims (transactions b)))
      (extension : IdentityExtension o.state.store ((created P b o.plan).map Row.entry) keyOf) :
      Reachable C P keyOf o.state

/-- Every reachable model state conserves supply, has lossless u128 budgets,
and preserves the finite semantic-key invariant needed for the next step.
This induction is not canonical-production-schedule or cryptographic proof. -/
theorem reachable_invariants (C : Correspondences) (P : Primitives) (keyOf : Nat → Nat)
    (s : State) (h : Reachable C P keyOf s) :
    Invariant s ∧ KeyAssociation s.store keyOf ∧ UnspentCorrespondence s.store keyOf := by
  induction h with
  | genesis header =>
    have hg := genesis_invariant P header keyOf
    exact ⟨hg.1, hg.2.1, hg.2.2.1⟩
  | next before b o previous checked trusted binding extension ih =>
    have hf := step_facts P before b o checked
    have hc := finite_correspondence before.store o.state.store (transactions b)
      ((created P b o.plan).map Row.entry) keyOf ih.1.finite binding ih.2.1 ih.2.2 hf.2.1 extension
    exact ⟨(step_invariant P before b o ih.1 checked).1, hc.2.1, hc.1⟩

private theorem selectAt_exists (slots : List Slot) (roll cum : Nat)
    (hc : cum ≤ roll) (hr : roll < cum + remainingTotal slots) :
    ∃ c rest, selectAt roll cum slots = (some c, rest) := by
  induction slots generalizing cum with
  | nil => simp [remainingTotal] at hr; omega
  | cons slot slots ih =>
    cases hu : slot.used
    · simp only [selectAt, hu, Bool.false_eq_true, ↓reduceIte]
      split
      · exact ⟨slot.cand, _, rfl⟩
      · rename_i hn
        have ht : roll < cum + slot.weight + remainingTotal slots := by
          simp only [remainingTotal, hu, Bool.false_eq_true, ↓reduceIte] at hr
          omega
        obtain ⟨c, rest, he⟩ := ih (cum + slot.weight) (by omega) ht
        exact ⟨c, slot :: rest, by simp [he]⟩
    · simp only [selectAt, hu, ↓reduceIte]
      have ht : roll < cum + remainingTotal slots := by simpa [remainingTotal, hu] using hr
      obtain ⟨c, rest, he⟩ := ih cum hc ht
      exact ⟨c, slot :: rest, by simp [he]⟩

private theorem pick_nonempty (P : Prims) (sd : Bytes) (q i n : Nat) (slots : List Slot)
    (hn : 0 < n) (hw : 0 < remainingTotal slots) : pickWinners P sd q i n slots ≠ [] := by
  cases n with
  | zero => omega
  | succ n =>
    obtain ⟨c, rest, he⟩ := selectAt_exists slots
      (verifiableRandomU128 P sd i % remainingTotal slots) 0 (by omega) (by
        simpa using Nat.mod_lt (verifiableRandomU128 P sd i) hw)
    simp [pickWinners, show remainingTotal slots ≠ 0 by omega, he]

private theorem addToLast_nonempty (extra : Nat) (ws : List Winner) (h : ws ≠ []) :
    addToLast extra ws ≠ [] := by
  cases ws with
  | nil => contradiction
  | cons w ws => cases ws <;> simp [addToLast]

private theorem fresh_uniform_total (cs : List Candidate) :
    remainingTotal (cs.map (fun c => ({ cand := c, weight := 1, used := false } : Slot))) = cs.length := by
  induction cs with
  | nil => rfl
  | cons c cs ih => simp [remainingTotal, ih, Nat.add_comm]

private theorem canonical_eligible_nonempty (P : Prims) (cs : List Candidate)
    (pool height : Nat) (sd : Bytes) (hc : cs ≠ []) :
    ∃ r, drawEligible P cs pool height sd canonicalConfig.draw = some r ∧ r.winners ≠ [] := by
  have hlen : 0 < cs.length := List.length_pos_iff.mpr hc
  have ht : 0 < minN 4 cs.length := by unfold minN; split <;> omega
  have hrem := fresh_uniform_total cs
  have hn := pick_nonempty P sd (pool / minN 4 cs.length) 0 (minN 4 cs.length)
    (cs.map (fun c => ({ cand := c, weight := 1, used := false } : Slot))) ht (by omega)
  unfold drawEligible
  dsimp [canonicalConfig, DrawConfig.default, Candidate.weight]
  rw [ite_eq_right (by omega)]
  exact ⟨_, rfl, addToLast_nonempty _ _ hn⟩

private theorem scan_eligible (rows : List Utxo) (cap height taken : Nat) (cfg : DrawConfig) :
    ∀ c ∈ scan cap height cfg rows taken, c.isEligible height cfg = true := by
  induction rows generalizing taken with
  | nil => simp [scan]
  | cons row rows ih =>
    simp only [scan]
    split
    · simp
    · split
      · rename_i he
        intro c hm
        simp only [List.mem_cons] at hm
        rcases hm with rfl | hm
        · exact he
        · exact ih _ c hm
      · exact ih taken

private theorem canonical_draw_nonempty (P : Prims) (cs : List Candidate) (F : Nat)
    (acc : Accounting) (height : Nat) (sd : Bytes) (hp : acc.payout ≠ 0) (hc : cs ≠ [])
    (he : ∀ c ∈ cs, c.isEligible height canonicalConfig.draw = true) :
    (drawLotteryWinnersSeeded P cs F acc height sd canonicalConfig).winners ≠ [] := by
  have hfilter : cs.filter (fun c => c.isEligible height canonicalConfig.draw) = cs :=
    List.filter_eq_self.mpr he
  obtain ⟨r, hr, hn⟩ := canonical_eligible_nonempty P cs acc.payout height sd hc
  have hd : drawWinnersWithSeed P cs acc.payout height sd canonicalConfig.draw = some r := by
    simp [drawWinnersWithSeed, hfilter, hp, hc, hr]
  simpa [drawLotteryWinnersSeeded, hp, hd] using hn

/-- The guarded no-winner error is unreachable for the canonical uniform
configuration: candidate scanning keeps eligible rows and positive rolls
select at least one ticket. Other drawing errors remain explicit. -/
theorem canonical_no_winner_error (P : Prims) (r : Reads) (b : DrawBlock) :
    drawing P r b ≠ .error .noWinners := by
  unfold drawing
  simp only [show ¬CANDIDATE_MAX_AWARDS < canonicalConfig.draw.winnersPerDraw from by decide,
    ↓reduceIte]
  cases hs : seed P r b with
  | error err =>
    unfold seed at hs
    split at hs
    · cases hs
    · cases hs
      simp
  | ok sd =>
    cases hf : sumFeesChecked 0 b.fees with
    | none => simp
    | some F =>
      let cs := lotteryCandidates P r.rows b.height sd canonicalConfig.draw
      let acc := computePoolAccounting F b.emissionShare r.lotteryPool
        (rewardCap cs b.height b.reward canonicalConfig) canonicalConfig
      change (if acc.payout ≠ 0 ∧ !cs.isEmpty ∧
        (drawLotteryWinnersSeeded P cs F acc b.height sd canonicalConfig).winners.isEmpty then
        Except.error DrawingError.noWinners else
        Except.ok (drawLotteryWinnersSeeded P cs F acc b.height sd canonicalConfig, acc)) ≠
        (Except.error DrawingError.noWinners : Except DrawingError (BlockLotteryResult × Accounting))
      have hg : ¬(acc.payout ≠ 0 ∧ !cs.isEmpty ∧
        (drawLotteryWinnersSeeded P cs F acc b.height sd canonicalConfig).winners.isEmpty) := by
        intro h
        have hc : cs ≠ [] := by simpa using h.2.1
        have he : ∀ c ∈ cs, c.isEligible b.height canonicalConfig.draw = true :=
          scan_eligible _ _ _ _ _
        have hn := canonical_draw_nonempty P cs F acc b.height sd h.1 hc he
        simpa [hn] using h.2.2
      rw [ite_eq_right hg]
      simp

/- Kernel boundary fixtures. These synthetic hashes/fields and zero-fee
transactions test model branches, not signatures, curve validity, fee floors,
canonical schedule reachability, or production acceptance. -/
private def toyPrimitives : Primitives :=
  { lottery := { sha256 := fun _ => leBytes 32 250, powHash := fun _ => zeros 32 }
    transactionHash := fun bytes => bytes.take 32
    ordinaryRoot := fun _ => zeros 32
    manifest := fun _ => zeros 32 }

private def toyFields : Fields := ⟨leBytes 32 1, leBytes 32 2, none, none, [7]⟩
private def toyHeader : Header :=
  { version := 8, prevBlockHash := leBytes 32 250, txRoot := zeros 32
    timestamp := 0, height := 1, difficulty := 0, nonce := 0
    minterViewKey := zeros 32, minterSpendKey := zeros 32 }
private def toyInput : Input :=
  { header := toyHeader, mintHeight := 1, reward := 100, policyInterval := productionInterval
    mintingEncoded := zeros 32, coinbaseFields := toyFields, candidateFactors := []
    transactions := [], summary := ⟨0, 0, 0, zeros 32⟩, records := [], wires := [], witnesses := [] }
private def toyRow (id : Nat) : Row := ⟨⟨leBytes 32 id, 0⟩, 1000000, 0, toyFields, .direct 0⟩
private def toyState (n : Nat) : State :=
  let rows := (List.range n).map (fun i => toyRow (i + 1))
  { store := ⟨rows.map Row.entry, rows.map Row.entry, []⟩, rows
    tip := { toyHeader with height := 720 }, genesis := zeros 32
    pool := some 11, mined := n * 1000000 + 11, burned := 0 }
private def toyBase (n : Nat) : Input :=
  { toyInput with
    header := { toyHeader with height := 721 }, mintHeight := 721
    reward := 20, candidateFactors := List.replicate n 1000 }
private def toySource (s : State) (w : Winner) : Row :=
  (s.rows.find? (fun r => r.outpoint.bytes == w.utxoId)).getD (toyRow 0)
private def toyRecord (s : State) (i : Nat) (w : Winner) : Record :=
  let src := toySource s w
  ⟨i, src.outpoint, w.payout, leBytes 32 (i + 200), src.fields.publicKey,
    src.fields.ciphertext, ⟨0, leBytes 32 (i + 1)⟩⟩

/-- Fixture construction is deliberately separate from validation. The real
step still independently computes the canonical draw and all bindings. -/
private def fillToy (s : State) (b : Input) : Input :=
  match drawing toyPrimitives.lottery (reads s b) (drawBlock b) with
  | .error _ => b
  | .ok (draw, _) =>
    let rs := draw.winners.mapIdx (toyRecord s)
    let vs := draw.winners.mapIdx (fun i w =>
      let r := toyRecord s i w
      (⟨toySource s w, awardDomain toyPrimitives s b draw.winners i w.payout,
        r.target, r.context⟩ : DerivationWitness))
    { b with
      summary := ⟨draw.totalFees, draw.poolAmount, draw.burnAmount, draw.seed⟩
      records := rs, wires := rs.map recordWire, witnesses := vs }

private def stats (P : Primitives) (s : State) (b : Input) (o : Outcome) : List Nat :=
  [o.state.tip.height, o.state.mined, o.state.burned, o.state.pool.getD 0,
    value o.state.store.unspent, value ((created P b o.plan).map Row.entry),
    saturationLoss s b o.plan, (created P b o.plan).length]
private def observe (P : Primitives) (s : State) (b : Input) : Option (List Nat) :=
  match step P s b with
  | .error _ => none
  | .ok o => some (stats P s b o)
private def errorOf {α : Type} : Except Error α → Option Error
  | .error e => some e
  | .ok _ => none
private def firstRecord (b : Input) : Record :=
  b.records.headD ⟨0, ⟨zeros 32, 0⟩, 0, zeros 32, zeros 32, none, ⟨0, zeros 32⟩⟩
private def firstWitness (b : Input) : DerivationWitness :=
  b.witnesses.headD ⟨toyRow 0, ⟨[], [], 0, [], [], 0, 0⟩, [], ⟨0, []⟩⟩

/-- Zero interval/reward, epoch transitions, u32 clamp and saturated multiply. -/
theorem emission_boundaries :
    emissionBps 100 0 = 0 ∧ emission 100 0 1 = 0 ∧
    emissionBps 99 100 = 0 ∧ emissionBps 100 100 = 1000 ∧
    emissionBps 499 100 = 4000 ∧ emissionBps 500 100 = 5000 ∧
    emissionEpoch U64_MAX 1 = U32_MAX ∧
    satMul U32_MAX 4294968 1000 = U32_MAX ∧ emissionBps U64_MAX 1 = 5000 ∧
    emission U64_MAX U64_MAX 1 = U64_MAX / 2 ∧
    emission productionInterval 10 productionInterval = 1 := by
  set_option maxRecDepth 16384 in
    decide

/-- Fresh genesis has no outputs; its first finite successor creates only the
computed miner payout. Minting-height and policy mismatches reject. -/
theorem first_successor_boundary :
    let s := genesis toyPrimitives toyHeader
    observe toyPrimitives s toyInput = some [1, 100, 0, 0, 100, 100, 0, 1] ∧
    errorOf (step toyPrimitives s { toyInput with mintHeight := 2 }) = some .mintingHeight ∧
    errorOf (step toyPrimitives s { toyInput with policyInterval := 0 }) = some .emissionPolicy := by
  set_option maxRecDepth 16384 in
    decide

/-- One through four mature awards, with underfilled draws and remainder on
the last winner. Every case executes prepare, finite insertion and projection. -/
theorem award_count_boundaries :
    (List.range 4).map (fun j =>
      let s := toyState (j + 1)
      let b := fillToy s (toyBase (j + 1))
      observe toyPrimitives s b) =
      [some [721, 1000031, 0, 0, 1000031, 31, 0, 2],
       some [721, 2000031, 0, 0, 2000031, 31, 0, 3],
       some [721, 3000031, 0, 0, 3000031, 31, 0, 4],
       some [721, 4000031, 0, 0, 4000031, 31, 0, 5]] ∧
    (List.range 4).map (fun j => (fillToy (toyState (j + 1)) (toyBase (j + 1))).wires.map Wire.amount) =
      [[11], [5, 6], [3, 3, 5], [2, 2, 2, 5]] := by
  set_option maxRecDepth 16384 in
    decide

/-- Independent structural mutations fail before any finite write. -/
theorem payout_rejection_boundaries :
    let s := toyState 1
    let b := fillToy s (toyBase 1)
    let r := firstRecord b
    let v := firstWitness b
    errorOf (prepare toyPrimitives s { b with summary := { b.summary with fees := 1 } }) = some .summary ∧
    errorOf (prepare toyPrimitives s { b with records := [] }) = some .count ∧
    errorOf (prepare toyPrimitives s { b with records := [{ r with ordinal := 1 }] }) = some .ordinal ∧
    errorOf (prepare toyPrimitives s { b with records := [{ r with winner := ⟨leBytes 32 9, 0⟩ }] }) = some .winner ∧
    errorOf (prepare toyPrimitives s { b with records := [{ r with amount := 12 }] }) = some .amount ∧
    errorOf (prepare toyPrimitives s { b with wires := [{ recordWire r with target := zeros 32 }] }) = some .wireFields ∧
    errorOf (prepare toyPrimitives s { b with records := [{ r with context := ⟨1, leBytes 32 1⟩ }] }) = some .derivation ∧
    errorOf (prepare toyPrimitives s { b with witnesses := [{ v with source := toyRow 9 }] }) = some .source := by
  set_option maxRecDepth 16384 in
    decide

/-- Every derivation-domain component is checked, not just the amount. -/
theorem domain_rejection_boundaries :
    let s := toyState 1
    let b := fillToy s (toyBase 1)
    let v := firstWitness b
    let domains := [{ v.domain with genesis := [1] }, { v.domain with parent := [1] },
      { v.domain with height := 722 }, { v.domain with ordinaryRoot := [1] },
      { v.domain with manifest := [1] }, { v.domain with ordinal := 1 }, { v.domain with amount := 12 }]
    domains.map (fun d => errorOf (prepare toyPrimitives s { b with witnesses := [{ v with domain := d }] })) =
      List.replicate 7 (some .domain) := by
  set_option maxRecDepth 16384 in
    decide

/-- Shared byte IDs expose class collisions; no tagged identity sum hides
coinbase/ordinary/payout overlap. The last case rejects a same-block source. -/
theorem shared_identity_boundaries :
    let s := genesis toyPrimitives toyHeader
    let ordinary : Ordinary := ⟨leBytes 32 250,
      ⟨[], [⟨(Outpoint.mk (leBytes 32 250) 0).id, 0⟩], 0⟩, [toyFields]⟩
    let first : Ordinary := ⟨leBytes 32 1,
      ⟨[], [⟨(Outpoint.mk (leBytes 32 1) 0).id, 0⟩], 0⟩, [toyFields]⟩
    let second : Ordinary := ⟨leBytes 32 2,
      ⟨[⟨⟨(Outpoint.mk (leBytes 32 1) 0).id, 0⟩, 101⟩], [], 0⟩, []⟩
    let payoutId := (Outpoint.mk (leBytes 32 250) 1).id
    let ordinaryRow : Row := ⟨⟨leBytes 32 250, 1⟩, 1, 1, toyFields, .direct 1⟩
    let payoutRow : Row := { ordinaryRow with amount := 2, context := .lottery ⟨0, zeros 32⟩ }
    ordinaryRow.context ≠ payoutRow.context ∧ ordinaryRow.entry.id = payoutRow.entry.id ∧
    errorOf (step toyPrimitives s { toyInput with transactions := [ordinary] }) = some .createdIdentity ∧
    errorOf (finiteStep s.store [] [ordinaryRow.entry, payoutRow.entry]) = some .createdIdentity ∧
    errorOf (finiteStep ⟨[], [⟨payoutId, 1⟩], [9]⟩ [] [⟨payoutId, 2⟩]) = some .createdIdentity ∧
    errorOf (step toyPrimitives s { toyInput with transactions := [first, second] }) = some .consumedSource := by
  set_option maxRecDepth 16384 in
    decide

/-- Synthetic height/loss controls, not genesis-origin or production-valid
chains: the final admissible u64 height succeeds and its successor is rejected
by the checked seed guard. An arbitrary seeded pool retains visible loss. -/
theorem height_and_loss_boundaries :
    let s := genesis toyPrimitives toyHeader
    let high := { s with tip := { s.tip with height := U64_MAX - 1 }, pool := some 0 }
    let b := { toyInput with header := { toyHeader with height := U64_MAX }, mintHeight := U64_MAX, reward := 0 }
    let saturated := { s with tip := { s.tip with height := productionInterval - 1 }, pool := some U128_MAX }
    let e := { toyInput with header := { toyHeader with height := productionInterval }, mintHeight := productionInterval, reward := 10 }
    observe toyPrimitives high b = some [U64_MAX, 0, 0, 0, 0, 0, 0, 1] ∧
    errorOf (step toyPrimitives { high with tip := { high.tip with height := U64_MAX } } b) =
      some (.seedOrDrawing .seedParent) ∧
    observe toyPrimitives saturated e = some [productionInterval, 10, 0, U128_MAX, 9, 9, 1, 1] := by
  set_option maxRecDepth 16384 in
    decide

/-- Every summary field and bound wire field is checked independently. -/
theorem summary_and_wire_rejection_boundaries :
    let s := toyState 1
    let b := fillToy s (toyBase 1)
    let r := firstRecord b
    let summaries := [{ b.summary with fees := 1 }, { b.summary with distributed := 12 },
      { b.summary with burned := 1 }, { b.summary with seed := zeros 32 }]
    let wires := [{ recordWire r with winner := ⟨leBytes 32 9, 0⟩ },
      { recordWire r with amount := 12 }, { recordWire r with target := zeros 32 },
      { recordWire r with publicKey := zeros 32 }, { recordWire r with ciphertext := some [1] }]
    let records := [{ r with target := zeros 32 }, { r with publicKey := zeros 32 },
      { r with ciphertext := some (zeros 1088) }]
    records.map (fun record => errorOf (prepare toyPrimitives s
      { b with records := [record], wires := [recordWire record] })) =
      List.replicate 3 (some .derivation) ∧
    summaries.map (fun summary => errorOf (prepare toyPrimitives s { b with summary })) =
      List.replicate 4 (some .summary) ∧
    wires.map (fun wire => errorOf (prepare toyPrimitives s { b with wires := [wire] })) =
      List.replicate 5 (some .wireFields) := by
  set_option maxRecDepth 16384 in
    decide

end Botho.Supply.Block
