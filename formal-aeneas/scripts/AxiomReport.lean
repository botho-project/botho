/-
Axiom report for the formal-aeneas gate (#1821).

Run as `lake env lean --run scripts/AxiomReport.lean OUT [NAME ...]` from
`formal-aeneas/`, after `lake build`.

It imports the compiled modules `MonetaryKernels` and `PoolAccounting` (the
Aeneas extraction) and `BothoAeneas` (the proofs) with `loadExts := false`, so no environment
extension and no `[init]` declaration of any imported module runs, and walks
the declaration bodies itself. It does not use `#print axioms` or
`Lean.collectAxioms`, so nothing written in the proofs can change what it
reports by redefining those commands. It is a cut-down version of
`formal/scripts/AxiomReport.lean` (without the statement binding of #1805).

It writes to the file OUT, one tab-separated record per line:

  AXIOMS <name> <ax1,ax2,...>  for each NAME given on the command line
  MISSING <name>               a NAME that is not a declaration
  OWN <decl> <ax1,ax2,...>     each declaration of a package module (see
                               `isPkgModule`) and the axioms it reaches
  SORRY <decl>                 a package declaration that reaches `sorryAx`
  AXIOMDECL <decl>             an `axiom` declared in a package module
  OPAQUEDECL <decl>            an `opaque` declared in a package module
  UNSAFE <decl>                an `unsafe` or `partial` definition in a
                               package module
  MODULE <mod>                 each package module that was loaded
  DONE <count>                 last line; <count> package declarations swept

Package modules are `MonetaryKernels`, `PoolAccounting` and `BothoAeneas` /
`BothoAeneas.*`.
Every other module (Lean core, Mathlib, Aeneas) is a dependency: its axioms
show up in the AXIOMS / OWN lists of the package declarations that reach
them, and the calling script checks those lists against the allow-list.
-/
import Lean

open Lean

/-- Memo table: block key to the sorted axioms it reaches. -/
abbrev Memo := Std.HashMap Name (Array Name)

/-- A constructor and every inductive of a mutual block share one key, so the
dependency graph is acyclic and the memo exact (see formal/scripts). -/
def blockKey (env : Environment) (c : Name) : Name :=
  let ind := match env.find? c with
    | some (.ctorInfo v) => v.induct
    | _ => c
  match env.find? ind with
  | some (.inductInfo v) => v.all.headD ind
  | _ => c

def exprsOf (env : Environment) (key : Name) : List Expr :=
  match env.find? key with
  | some (.defnInfo v) => [v.type, v.value]
  | some (.thmInfo v) => [v.type, v.value]
  | some (.opaqueInfo v) => [v.type, v.value]
  | some (.inductInfo v) =>
      v.all.flatMap fun i => match env.find? i with
        | some (.inductInfo w) =>
            w.type :: w.ctors.filterMap fun ctor => (env.find? ctor).map (·.type)
        | _ => []
  | some (.recInfo v) => v.type :: v.rules.map (·.rhs)
  | some info => [info.type]
  | none => []

/-- Axioms reachable from `c`, walking declaration bodies directly. -/
partial def axiomsOf (env : Environment) (c : Name) : StateM Memo (Array Name) := do
  let key := blockKey env c
  if let some axs := (← get)[key]? then
    return axs
  modify (·.insert key #[])
  let mut acc : NameSet := {}
  if let some (.axiomInfo _) := env.find? key then
    acc := acc.insert key
  for e in exprsOf env key do
    for d in e.getUsedConstants do
      for a in (← axiomsOf env d) do
        acc := acc.insert a
  let result := acc.toArray.qsort Name.lt
  modify (·.insert key result)
  return result

def isPkgModule (m : Name) : Bool :=
  m == `MonetaryKernels || m == `PoolAccounting || (`BothoAeneas).isPrefixOf m ||
    #[`Botho.Lottery.Pool, `Botho.Lottery.Draw, `Botho.Lottery.Prims,
      `Botho.Lottery.Candidate, `Botho.Lottery.Arith].contains m

def main (args : List String) : IO UInt32 := do
  let out :: names := args
    | IO.eprintln "usage: AxiomReport.lean OUT [NAME ...]"
      return 2
  initSearchPath (← findSysroot)
  let env ← importModules
    #[{ module := `MonetaryKernels }, { module := `PoolAccounting }, { module := `BothoAeneas }] {}
    (loadExts := false)
  let mut lines : Array String := #[]
  let mut memo : Memo := {}
  for n in names do
    let name := n.toName
    if (env.find? name).isNone then
      lines := lines.push s!"MISSING\t{n}"
    else
      let (axs, m) := (axiomsOf env name).run memo
      memo := m
      lines := lines.push s!"AXIOMS\t{n}\t{",".intercalate (axs.toList.map toString)}"
  let mut swept := 0
  let mut own : Array (Name × ConstantInfo) := #[]
  for (c, ci) in env.constants.map₁.toList do
    let some idx := env.getModuleIdxFor? c | continue
    let some mod := env.header.moduleNames[idx.toNat]? | continue
    if isPkgModule mod then own := own.push (c, ci)
  own := own.qsort (fun a b => Name.lt a.1 b.1)
  for (c, ci) in own do
    swept := swept + 1
    let (axs, m) := (axiomsOf env c).run memo
    memo := m
    lines := lines.push s!"OWN\t{c}\t{",".intercalate (axs.toList.map toString)}"
    if axs.contains ``sorryAx then
      lines := lines.push s!"SORRY\t{c}"
    match ci with
    | .axiomInfo _ => lines := lines.push s!"AXIOMDECL\t{c}"
    | .opaqueInfo _ => lines := lines.push s!"OPAQUEDECL\t{c}"
    | .defnInfo v =>
        if v.safety != .safe then lines := lines.push s!"UNSAFE\t{c}"
    | _ => pure ()
  for mod in env.header.moduleNames do
    if isPkgModule mod then lines := lines.push s!"MODULE\t{mod}"
  lines := lines.push s!"DONE\t{swept}"
  IO.FS.writeFile out ("\n".intercalate lines.toList ++ "\n")
  return 0
