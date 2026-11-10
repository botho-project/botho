/-
DRT entry points for the cluster-tag model (`Botho.Tags`, #1840).

Same wire format as `Drt/Codec.lean` (integers as decimal strings). A tag
vector is a list of `[cluster, weight]` pairs; a coin is
`{"value": V, "tags": [...]}`. `Drt.dispatch` falls through to
`Drt.Tags.dispatch` for the names below.

Since #1863 / #1870 the legs compare the fixed code: `merge_weighted`
against `Botho.Tags.mergeWeightedSafe` (it saturates, never panics), and
`cluster_tag_inheritance` against the protocol 8.0.0 rules
`Botho.Tags.inheritOk₈` (Rule 1, Rule 2, then C6). The pre-fix models
(`mergeWeighted`, `inheritOk` alone) stay in `formal/` as the record of
#1858 / #1857. The protocol 10.0.0 legs (#1687) `cluster_tag_retention` and
`import_floor_from_inputs` compare against `Botho.Tags.retainOk` and
`Botho.Tags.importFloorIn`.
-/
import Lean.Data.Json
import Botho
import Drt.Sha256

namespace Drt.Tags

open Lean (Json)
open Botho.Tags

abbrev Dec := Except String

def field (j : Json) (k : String) : Dec Json :=
  match j.getObjVal? k with
  | .ok v => .ok v
  | .error _ => .error s!"missing field '{k}'"

def asNat (j : Json) : Dec Nat := do
  let s ← j.getStr?
  match s.toNat? with
  | some n => pure n
  | none => throw s!"not a decimal string: {s}"

def asArr (j : Json) : Dec (List Json) := do pure (← j.getArr?).toList

def nat (j : Json) (k : String) : Dec Nat := do asNat (← field j k)
def arr (j : Json) (k : String) : Dec (List Json) := do asArr (← field j k)

def decEntry (j : Json) : Dec Entry := do
  match ← asArr j with
  | [c, w] => pure ⟨← asNat c, ← asNat w⟩
  | _ => throw "an entry is [cluster, weight]"

def decTags (j : Json) : Dec TagVec := do (← asArr j).mapM decEntry

def decCoin (j : Json) : Dec Coin := do
  pure ⟨← nat j "value", ← decTags (← field j "tags")⟩

def encNat (n : Nat) : Json := Json.str (toString n)

def encTags (t : TagVec) : Json :=
  Json.arr (t.map fun e => Json.arr #[encNat e.cluster, encNat e.weight]).toArray

/-- `BRIDGE_IMPORT_EPOCH_BLOCKS` (`cluster-tax/src/bridge_import.rs:61`). -/
def BRIDGE_IMPORT_EPOCH_BLOCKS : Nat := 17280

/-- `n.to_le_bytes()` for a `u64`. -/
def leBytes8 (n : Nat) : List UInt8 :=
  (List.range 8).map fun i => (n / 2 ^ (8 * i) % 256).toUInt8

/-- `import_cluster_id_for_height(height).0`: the first 8 bytes,
little-endian, of `SHA-256("bridge-import" ‖ (height / K).to_le_bytes())`.
Computed here independently of the Rust (the model takes the import
cluster as a parameter). -/
def importClusterForHeight (height : Nat) : Nat :=
  let pre := "bridge-import".toUTF8.toList ++ leBytes8 (height / BRIDGE_IMPORT_EPOCH_BLOCKS)
  let d := Drt.Sha256.hashList pre
  (List.range 8).foldl (fun acc i => acc + (d.getD i 0).toNat * 2 ^ (8 * i)) 0

/-- The first failing cluster in ascending order, as `BTreeMap` iteration
returns it (the error `check_cluster_tag_inheritance` reports). -/
def firstFailure (rings : List (List Coin)) (outs : List Coin) : Option Nat :=
  (inputClusters rings).foldl
    (fun best c => if clusterOk rings outs c then best else
      match best with
      | none => some c
      | some b => some (min b c)) none

def decInput (j : Json) : Dec Input₁₀ := do
  pure ⟨← nat j "pseudo", ← (← arr j "ring").mapM decCoin⟩

/-- The smallest failing cluster of output `o` (`BTreeMap` order), if any. -/
def retentionFailure (ins : List Input₁₀) (o : Coin) : Option Nat :=
  (inputClusters (ins.map Input₁₀.ring)).foldl
    (fun best c =>
      if required₁₀ ins c < MIN_STORED_WEIGHT || keepsWeight c (required₁₀ ins c) o then best
      else match best with
        | none => some c
        | some b => some (min b c)) none

/-- The first `(output, cluster)` the retained-weight floor rejects, scanning
outputs in order (the error `first_tag_retention_violation` reports). -/
def firstRetentionViolation (ins : List Input₁₀) : Nat → List Coin → Option (Nat × Nat × Coin)
  | _, [] => none
  | i, o :: os =>
    match retentionFailure ins o with
    | some c => some (i, c, o)
    | none => firstRetentionViolation ins (i + 1) os

def dispatch (fn : String) (a : Json) : Option (Dec Json) :=
  match fn with
  | "merge_weighted" => some do
    let inputs ← (← arr a "inputs").mapM fun j => do
      pure ((← decTags (← field j "tags")), (← nat j "value"))
    pure (Json.mkObj [("entries", encTags (mergeWeightedSafe inputs (← nat a "rate")))])
  | "cluster_tag_inheritance" => some do
    let rings ← (← arr a "rings").mapM fun r => do (← asArr r).mapM decCoin
    let outs ← (← arr a "outputs").mapM decCoin
    let imp := importClusterForHeight (← nat a "height")
    let ok := inheritOk₈ rings outs imp
    -- The reported reason, in the order block validation applies the rules;
    -- `inheritOk₈` must agree with it.
    let r := match firstMalformed outs with
      | some i => (false, Json.mkObj [("rule1", encNat i)])
      | none =>
        match firstOffRing rings outs imp with
        | some (i, c) => (false, Json.mkObj [("rule2",
            Json.mkObj [("output", encNat i), ("cluster", encNat c)])])
        | none =>
          match inheritOk rings outs, firstFailure rings outs with
          | true, _ => (true, Json.mkObj [("accepted", Json.bool true)])
          | false, none =>
            (false, Json.mkObj [("model_error", Json.str "rejected with no failing cluster")])
          | false, some c =>
            let m := inputMass c rings
            (false, Json.mkObj [("cluster", encNat c), ("actual", encNat (outputMass c outs)),
              ("expected", encNat (expected m)), ("input", encNat m),
              ("tolerance", encNat (tolerance m))])
    pure (if r.1 == ok then r.2
      else Json.mkObj [("model_error", Json.str "inheritOk₈ disagrees with the reported rule")])
  | "effective_cluster_wealth" => some do
    let outs ← (← arr a "outputs").mapM decCoin
    let table ← (← arr a "wealth").mapM fun j => do
      match ← asArr j with
      | [c, w] => pure (← asNat c, ← asNat w)
      | _ => throw "a wealth row is [cluster, wealth]"
    let env : FeeEnv :=
      { wealth := fun c => ((table.find? (·.1 == c)).map (·.2)).getD 0,
        isImport := fun _ => false, curve := id, implied := fun _ => 0 }
    -- Protocol 9.0.0 (#1876): the exact claimed wealth. The pre-fix
    -- saturating `effectiveWealth` stays in the model as the historical record.
    pure (encNat (effectiveWealthExact env outs))
  | "import_floor_factor" => some do
    let outs ← (← arr a "outputs").mapM decCoin
    let imports ← (← arr a "imports").mapM asNat
    let env : FeeEnv :=
      { wealth := fun _ => 0, isImport := fun c => imports.contains c, curve := id,
        implied := fun _ => 0 }
    pure (encNat (importFloor env outs))
  | "cluster_tag_retention" => some do
    let ins ← (← arr a "inputs").mapM decInput
    let outs ← (← arr a "outputs").mapM decCoin
    let ok := retainOk ins outs
    let r := match firstRetentionViolation ins 0 outs with
      | none => (true, Json.mkObj [("accepted", Json.bool true)])
      | some (i, c, o) =>
        (false, Json.mkObj [("output", encNat i), ("actual", encNat (weightOn c o.tags)),
          ("cluster", encNat c), ("required", encNat (required₁₀ ins c))])
    pure (if r.1 == ok then r.2
      else Json.mkObj [("model_error", Json.str "retainOk disagrees with the reported violation")])
  | "import_floor_from_inputs" => some do
    let ins ← (← arr a "inputs").mapM decInput
    let imports ← (← arr a "imports").mapM asNat
    let env : FeeEnv :=
      { wealth := fun _ => 0, isImport := fun c => imports.contains c, curve := id,
        implied := fun _ => 0 }
    pure (encNat (importFloorIn env ins))
  | _ => none

end Drt.Tags
