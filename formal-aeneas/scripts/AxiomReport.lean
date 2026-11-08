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
  COMPILERREC <decl> <parent>  authenticated canonical recursion companion
  MODULE <mod>                 each package module that was loaded
  DONE <count>                 last line; <count> package declarations swept

Package modules are `MonetaryKernels`, `PoolAccounting` and `BothoAeneas` /
`BothoAeneas.*`, plus the exact five canonical lottery modules. All owned
bodies are walked, including compiler recursion companions. The only safety
classification exception is the pinned compiler's partial companion of one
of the twelve reviewed safe recursive canonical definitions, checked below.
It is not a suffix-only or user-written unsafe/partial exemption.
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

/-- The reviewed recursive declarations in the unchanged canonical closure.
Adding another one requires explicit gate review; no broad namespace exemption. -/
def canonicalRecModule? (parent : Name) : Option Name :=
  if #[`Botho.Lottery.bytesLt, `Botho.Lottery.leBytes, `Botho.Lottery.ofLeBytes,
       `Botho.Lottery.sumN, `Botho.Lottery.zeros].contains parent then
    some `Botho.Lottery.Arith
  else if parent == `Botho.Lottery.countEligible then some `Botho.Lottery.Candidate
  else if #[`Botho.Lottery.addToLast, `Botho.Lottery.maxValue?, `Botho.Lottery.pickWinners,
            `Botho.Lottery.remainingTotal, `Botho.Lottery.selectAt,
            `Botho.Lottery.totalPaid].contains parent then some `Botho.Lottery.Draw
  else none

/-- Recognize the pinned Lean compiler's recursion artifacts without executing
imported extensions. `recExt.isTagged` reads the imported module entries, which
`finalizeImport` installs even with `loadExts := false`. A same-named safe
nonrecursive parent is insufficient. Keep every body in the axiom walk. -/
def canonicalRecParent? (env : Environment) (c : Name) (v : DefinitionVal) : Option Name := do
  let .str parent "_unsafe_rec" := c | none
  let expectedModule ← canonicalRecModule? parent
  let idx ← env.getModuleIdxFor? parent
  let mod ← env.header.moduleNames[idx.toNat]?
  guard (mod == expectedModule && env.getModuleIdxFor? c == some idx)
  let some (.defnInfo original) := env.find? parent | none
  guard (original.safety == .safe && v.safety == .partial)
  guard (original.type == v.type && original.levelParams == v.levelParams)
  guard (original.all == [parent] && v.all == [c])
  guard (Meta.recExt.isTagged env parent && v.value.getUsedConstants.contains c)
  return parent

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
        if v.safety != .safe then
          if let some parent := canonicalRecParent? env c v then
            lines := lines.push s!"COMPILERREC\t{c}\t{parent}"
          else
            lines := lines.push s!"UNSAFE\t{c}"
    | _ => pure ()
  for mod in env.header.moduleNames do
    if isPkgModule mod then lines := lines.push s!"MODULE\t{mod}"
  lines := lines.push s!"DONE\t{swept}"
  IO.FS.writeFile out ("\n".intercalate lines.toList ++ "\n")
  return 0
