/-
Independent axiom report for the formal gates (#1770, #1799).

Run as `lean --run scripts/AxiomReport.lean OUT ROOT [NAME ...]`, with
`LEAN_PATH` pointing at the compiled library (the verify job,
`verify-artifacts.sh`), or under `lake env` (the in-tree gates).

It imports the compiled module ROOT (for example `Botho`) without loading
environment extensions, and walks declaration bodies itself. It does not use
`#print axioms`, `Lean.collectAxioms`, or the axiom data that Lean caches in
.olean files. Nothing in the model is elaborated, so a model cannot change
what this program reports by overriding `#print axioms`, by printing fake
result lines, or by hiding warnings.

No model code runs here (#1799). `importModules (loadExts := false)` only
reads the .olean files: `[init]` / `initialize` declarations run in
`finalizePersistentExtensions`, which only `loadExts := true` calls (see
`finalizeImport` in Lean/Environment.lean, v4.35.0). Never set `loadExts`,
never call `evalConst` on a model declaration, and never import the model
from this file's own header (`import Botho` here would run its initializers
while Lean elaborates this file).

It writes to the file OUT (not stdout). One line per record, tab-separated:

  AXIOMS <name> <ax1,ax2,...>   for each NAME given on the command line
  MISSING <name>                a NAME that is not a declaration
  SORRY <decl>                  a declaration defined in ROOT or ROOT.* that
                                reaches `sorryAx` (whether or not it is listed)
  MODULE <mod>                  each compiled module ROOT or ROOT.*
  IMPORT <mod> <imported>       each direct import of such a module
  AXIOMDECL <name>              each `axiom` declared in such a module
  INIT <mod> <decl>             each `[init]` / `[builtin_init]` declaration
                                of such a module (recorded, never run)
  DONE <count>                  last line; <count> is the number of
                                ROOT-module declarations swept

The calling script checks the kernel has accepted every declaration by
running `leanchecker` first. This program trusts the declaration bodies.
-/
import Lean

open Lean

/-- Memo table: block key (see `blockKey`) to the sorted axioms it reaches. -/
abbrev Memo := Std.HashMap Name (Array Name)

/-- The memo key of a constant. A constructor and every inductive of a mutual
block share one key (the block's first inductive), because their types refer
to each other. With blocks collapsed the dependency graph is acyclic: Lean
adds a block atomically, and a constant can only refer to earlier ones. So the
memo is exact and does not depend on the order constants are visited in. -/
def blockKey (env : Environment) (c : Name) : Name :=
  let ind := match env.find? c with
    | some (.ctorInfo v) => v.induct
    | _ => c
  match env.find? ind with
  | some (.inductInfo v) => v.all.headD ind
  | _ => c

/-- The expressions a constant's axioms come from: type and value, or, for an
inductive block, the types of every inductive and constructor in it. -/
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
  | some info => [info.type]
  | none => []

/-- Axioms reachable from `c`. Walks declaration bodies directly. A
provisional empty entry stops the walk from looping on a cyclic environment
(which `leanchecker` would reject anyway). -/
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

def main (args : List String) : IO UInt32 := do
  let out :: root :: names := args
    | IO.eprintln "usage: AxiomReport.lean OUT ROOT [NAME ...]"
      return 2
  let rootName := root.toName
  initSearchPath (← findSysroot)
  let env ← importModules #[{ module := rootName }] {} (loadExts := false)
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
  -- Sweep every declaration owned by ROOT or a ROOT.* module.
  let mut swept := 0
  for (c, _) in env.constants.map₁.toList do
    let some idx := env.getModuleIdxFor? c | continue
    let some mod := env.header.moduleNames[idx.toNat]? | continue
    unless rootName.isPrefixOf mod do continue
    swept := swept + 1
    let (axs, m) := (axiomsOf env c).run memo
    memo := m
    if axs.contains ``sorryAx then
      lines := lines.push s!"SORRY\t{c}"
  -- Module-level records (#1799): the module set, the direct imports, the
  -- declared axioms and the initializers of every ROOT module. Read from the
  -- module data only; nothing is run.
  for h : idx in *...env.header.moduleNames.size do
    let mod := env.header.moduleNames[idx]
    unless rootName.isPrefixOf mod do continue
    lines := lines.push s!"MODULE\t{mod}"
    let some data := env.header.moduleData[idx]? | continue
    let mut seen : NameSet := {}
    for imp in data.imports do
      unless seen.contains imp.module do
        seen := seen.insert imp.module
        lines := lines.push s!"IMPORT\t{mod}\t{imp.module}"
    for c in data.constants do
      if let .axiomInfo _ := c then
        lines := lines.push s!"AXIOMDECL\t{c.name}"
    let mut inits : NameSet := {}
    for attr in [regularInitAttr, builtinInitAttr] do
      for (decl, _) in attr.ext.getModuleEntries env idx do
        inits := inits.insert decl
      for (decl, _) in attr.ext.getModuleIREntries env idx do
        inits := inits.insert decl
    -- Fallback on the raw module entries, in case the registered extensions
    -- ever stop matching the names stored in the .olean.
    if inits.isEmpty then
      for (extName, entries) in data.entries do
        if (extName == `Lean.regularInitAttr || extName == `Lean.builtinInitAttr)
            && !entries.isEmpty then
          inits := inits.insert extName
    for decl in inits.toArray.qsort Name.lt do
      lines := lines.push s!"INIT\t{mod}\t{decl}"
  lines := lines.push s!"DONE\t{swept}"
  IO.FS.writeFile out ("\n".intercalate lines.toList ++ "\n")
  return 0
