/-
DRT entry points for the cluster-tag model (`Botho.Tags`, #1840).

Same wire format as `Drt/Codec.lean` (integers as decimal strings). A tag
vector is a list of `[cluster, weight]` pairs; a coin is
`{"value": V, "tags": [...]}`. `Drt.dispatch` falls through to
`Drt.Tags.dispatch` for the names below.
-/
import Lean.Data.Json
import Botho

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

/-- The first failing cluster in ascending order, as `BTreeMap` iteration
returns it (the error `check_cluster_tag_inheritance` reports). -/
def firstFailure (rings : List (List Coin)) (outs : List Coin) : Option Nat :=
  (inputClusters rings).foldl
    (fun best c => if clusterOk rings outs c then best else
      match best with
      | none => some c
      | some b => some (min b c)) none

def dispatch (fn : String) (a : Json) : Option (Dec Json) :=
  match fn with
  | "merge_weighted" => some do
    let inputs ← (← arr a "inputs").mapM fun j => do
      pure ((← decTags (← field j "tags")), (← nat j "value"))
    pure (match mergeWeighted inputs (← nat a "rate") with
      | none => Json.mkObj [("panic", Json.bool true)]
      | some t => Json.mkObj [("entries", encTags t)])
  | "cluster_tag_inheritance" => some do
    let rings ← (← arr a "rings").mapM fun r => do (← asArr r).mapM decCoin
    let outs ← (← arr a "outputs").mapM decCoin
    let ok := inheritOk rings outs
    pure (match ok, firstFailure rings outs with
      | true, _ => Json.mkObj [("accepted", Json.bool true)]
      | false, none => Json.mkObj [("model_error", Json.str "rejected with no failing cluster")]
      | false, some c =>
        let m := inputMass c rings
        Json.mkObj [("cluster", encNat c), ("actual", encNat (outputMass c outs)),
          ("expected", encNat (expected m)), ("input", encNat m),
          ("tolerance", encNat (tolerance m))])
  | "effective_cluster_wealth" => some do
    let outs ← (← arr a "outputs").mapM decCoin
    let table ← (← arr a "wealth").mapM fun j => do
      match ← asArr j with
      | [c, w] => pure (← asNat c, ← asNat w)
      | _ => throw "a wealth row is [cluster, wealth]"
    let env : FeeEnv :=
      { wealth := fun c => ((table.find? (·.1 == c)).map (·.2)).getD 0,
        isImport := fun _ => false, curve := id, implied := fun _ => 0 }
    pure (encNat (effectiveWealth env outs))
  | "import_floor_factor" => some do
    let outs ← (← arr a "outputs").mapM decCoin
    let imports ← (← arr a "imports").mapM asNat
    let env : FeeEnv :=
      { wealth := fun _ => 0, isImport := fun c => imports.contains c, curve := id,
        implied := fun _ => 0 }
    pure (encNat (importFloor env outs))
  | _ => none

end Drt.Tags
