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
  STMT <name> <serialization>   for each NAME given on the command line
                                that is a declaration: the canonical
                                serialization of its statement closure
                                (#1805, see `statementClosure`). The
                                verifier hashes this field with SHA-256.
  STMTPP <name> <kind> <decl> <type> <value>
                                one line per declaration of that closure,
                                NAME's own first: pretty-printed type and,
                                for a `def`, value (empty otherwise), with
                                `\\`, newline, tab and CR escaped. For the
                                reviewer; the hash is what binds.
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

/-! ## Statement binding (#1805)

The verify job checks the `.olean` files an untrusted build uploaded, and
nothing else ties those files to the sources. So every theorem listed in
`Theorems.lean` is bound to a reviewed record, `formal/statements.txt`, read
from the base commit: the verifier recomputes the record entry below from the
compiled library and requires an identical entry in the base record.

The entry covers the theorem's *statement closure*: the theorem's own type,
plus every declaration of the ROOT library that the type reaches, followed
transitively through what each such declaration means:

  * a theorem contributes its type only (a proof is irrelevant; `leanchecker`
    checked it);
  * a `def` or `opaque` contributes its type and value;
  * an inductive contributes its whole mutual block (types and constructor
    types); a constructor or recursor pulls in its inductive;
  * any other declaration contributes its type.

Declarations from other modules (only `Init` is importable, and it is loaded
from the trusted toolchain, never from the artifact) are referenced by name
and not expanded. So `satAdd_le_cap : satAdd cap a b ≤ cap` is bound both to
its type and to the body of `Botho.satAdd`: rewriting the body in the
artifact changes the hash as surely as rewriting the type.

The serialization is exact (de Bruijn indices, structural names, levels and
literals, every field of each declaration kind) except for binder names and
metadata, which do not change what a statement means. Strings and name
components are quoted with `String.quote`, so the serialization never holds
a raw tab or newline. -/

/-- Escape `\`, newline, tab and CR so that a field fits on one report line. -/
def escField (s : String) : String :=
  s.replace "\\" "\\\\" |>.replace "\n" "\\n" |>.replace "\t" "\\t" |>.replace "\r" "\\r"

/-- Structural, unambiguous rendering of a name (string and numeric parts are
distinguished, so `«a.b»` and `a.b` differ). -/
def serName : Name → String
  | .anonymous => "_"
  | .str p s => serName p ++ "." ++ s.quote
  | .num p n => serName p ++ "#" ++ toString n

def serLevel : Level → String
  | .zero => "0"
  | .succ l => "(s " ++ serLevel l ++ ")"
  | .max a b => "(m " ++ serLevel a ++ " " ++ serLevel b ++ ")"
  | .imax a b => "(i " ++ serLevel a ++ " " ++ serLevel b ++ ")"
  | .param n => "(p " ++ serName n ++ ")"
  | .mvar _ => "(?l)"

def serBinder : BinderInfo → String
  | .default => "d"
  | .implicit => "i"
  | .strictImplicit => "s"
  | .instImplicit => "c"

def serLevels (ls : List Level) : String :=
  "[" ++ " ".intercalate (ls.map serLevel) ++ "]"

def serParams (ns : List Name) : String :=
  "[" ++ " ".intercalate (ns.map serName) ++ "]"

/-- Exact rendering of an expression up to binder names and metadata. -/
partial def serExpr (acc : String) : Expr → String
  | .bvar i => acc ++ s!"(b {i})"
  | .fvar _ => acc ++ "(?f)"
  | .mvar _ => acc ++ "(?m)"
  | .sort l => acc ++ "(S " ++ serLevel l ++ ")"
  | .const n ls => acc ++ "(C " ++ serName n ++ " " ++ serLevels ls ++ ")"
  | .app f a => serExpr (serExpr (acc ++ "(A ") f ++ " ") a ++ ")"
  | .lam _ t b bi => serExpr (serExpr (acc ++ "(L " ++ serBinder bi ++ " ") t ++ " ") b ++ ")"
  | .forallE _ t b bi =>
      serExpr (serExpr (acc ++ "(F " ++ serBinder bi ++ " ") t ++ " ") b ++ ")"
  | .letE _ t v b _ =>
      serExpr (serExpr (serExpr (acc ++ "(Z ") t ++ " ") v ++ " ") b ++ ")"
  | .lit (.natVal n) => acc ++ s!"(N {n})"
  | .lit (.strVal s) => acc ++ "(T " ++ s.quote ++ ")"
  | .mdata _ e => serExpr acc e
  | .proj s i e => serExpr (acc ++ "(P " ++ serName s ++ s!" {i} ") e ++ ")"

def serSafety : DefinitionSafety → String
  | .safe => "safe"
  | .unsafe => "unsafe"
  | .partial => "partial"

def serBool (b : Bool) : String := if b then "1" else "0"

def serNames (ns : List Name) : String := serParams ns

/-- Exact rendering of one declaration: kind, name, universe parameters and
every field that can change what the declaration means. -/
def serConst (acc : String) (ci : ConstantInfo) : String :=
  let head (k : String) := acc ++ "(" ++ k ++ " " ++ serName ci.name ++ " " ++
    serParams ci.levelParams ++ " "
  match ci with
  | .defnInfo v =>
      serExpr (serExpr (head "D" ++ serSafety v.safety ++ " ") v.type ++ " ") v.value ++ ")"
  | .thmInfo v => serExpr (head "H") v.type ++ ")"
  | .opaqueInfo v =>
      serExpr (serExpr (head "O" ++ serBool v.isUnsafe ++ " ") v.type ++ " ") v.value ++ ")"
  | .axiomInfo v => serExpr (head "X" ++ serBool v.isUnsafe ++ " ") v.type ++ ")"
  | .quotInfo v =>
      let k := match v.kind with
        | .type => "type" | .ctor => "ctor" | .lift => "lift" | .ind => "ind"
      serExpr (head "Q" ++ k ++ " ") v.type ++ ")"
  | .inductInfo v =>
      serExpr (head "I" ++ s!"{v.numParams} {v.numIndices} {v.numNested} " ++
        serNames v.all ++ " " ++ serNames v.ctors ++ " " ++ serBool v.isRec ++
        serBool v.isUnsafe ++ serBool v.isReflexive ++ " ") v.type ++ ")"
  | .ctorInfo v =>
      serExpr (head "K" ++ serName v.induct ++ s!" {v.cidx} {v.numParams} {v.numFields} " ++
        serBool v.isUnsafe ++ " ") v.type ++ ")"
  | .recInfo v =>
      let acc := serExpr (head "R" ++ serNames v.all ++
        s!" {v.numParams} {v.numIndices} {v.numMotives} {v.numMinors} " ++
        serBool v.k ++ serBool v.isUnsafe ++ " ") v.type
      v.rules.foldl (init := acc) (fun acc r =>
        serExpr (acc ++ " (r " ++ serName r.ctor ++ s!" {r.nfields} ") r.rhs ++ ")") ++ ")"

/-- Is `c` declared in ROOT or a ROOT.* module of the imported library? -/
def isLibConst (env : Environment) (root c : Name) : Bool :=
  match env.getModuleIdxFor? c with
  | some idx => match env.header.moduleNames[idx.toNat]? with
    | some m => root.isPrefixOf m
    | none => false
  | none => false

/-- The names a declaration's *meaning* refers to (see the section header):
a theorem's type only, a definition's type and value, an inductive's whole
block, a constructor's or recursor's inductive. -/
def meaningDeps (ci : ConstantInfo) : Array Name :=
  match ci with
  | .thmInfo v => v.type.getUsedConstants
  | .defnInfo v => v.type.getUsedConstants ++ v.value.getUsedConstants
  | .opaqueInfo v => v.type.getUsedConstants ++ v.value.getUsedConstants
  | .inductInfo v => v.type.getUsedConstants ++ v.all.toArray ++ v.ctors.toArray
  | .ctorInfo v => v.type.getUsedConstants.push v.induct
  | .recInfo v =>
      v.rules.foldl (init := v.type.getUsedConstants ++ v.all.toArray)
        fun acc r => acc ++ r.rhs.getUsedConstants
  | ci => ci.type.getUsedConstants

/-- The statement closure of `thm`: `thm` itself, then every ROOT
declaration reachable through `meaningDeps`, sorted by name. -/
def statementClosure (env : Environment) (root thm : Name) : Array ConstantInfo := Id.run do
  let some start := env.find? thm | return #[]
  let mut seen : NameSet := ({} : NameSet).insert thm
  let mut todo : Array Name := meaningDeps start
  let mut found : Array ConstantInfo := #[]
  while h : 0 < todo.size do
    let c := todo[todo.size - 1]
    todo := todo.pop
    if seen.contains c || !isLibConst env root c then continue
    seen := seen.insert c
    let some ci := env.find? c | continue
    found := found.push ci
    todo := todo ++ meaningDeps ci
  let sorted := found.qsort (fun a b => Name.lt a.name b.name)
  return #[start] ++ sorted

def kindWord : ConstantInfo → String
  | .defnInfo _ => "def"
  | .thmInfo _ => "theorem"
  | .opaqueInfo _ => "opaque"
  | .axiomInfo _ => "axiom"
  | .quotInfo _ => "quot"
  | .inductInfo _ => "inductive"
  | .ctorInfo _ => "constructor"
  | .recInfo _ => "recursor"

/-- The pretty-printer options for the record text (#1805, #1824). The text
should show what the hash binds, so it prints every argument (`pp.explicit`:
`@Eq.{1} Nat ...`, implicit type arguments included, so a statement over
`Nat` and the same statement over `Fin 2` print differently), universe
levels (`pp.universes`), the types of numerals (`pp.numericTypes`; with
`pp.explicit` most numerals already print as `@OfNat.ofNat Nat ...`), the
binder types of `fun` and `let` (`pp.funBinderTypes`, `pp.letVarTypes`),
full names (`pp.fullNames`), and applications in prefix form
(`pp.fieldNotation false`: `Nat.decLe a b`, not `a.decLe b`). Proofs still
print as `⋯`: by proof irrelevance two proofs of one proposition are
definitionally equal, so a proof term cannot change what a statement means,
and proof terms inside definitions can be large. Changing these options
changes every record block's text, not its hash; regenerate statements.txt
with propose-statements.sh --write in the same pull request. -/
def ppOptions : Options :=
  ({} : Options)
    |>.setBool `pp.fullNames true
    |>.setBool `pp.explicit true
    |>.setBool `pp.universes true
    |>.setBool `pp.numericTypes true
    |>.setBool `pp.funBinderTypes true
    |>.setBool `pp.letVarTypes true
    |>.setBool `pp.fieldNotation false

/-- Pretty-print `e` for the reviewer, on one logical line where possible,
with `ppOptions`. The environment was imported with `loadExts := false`, so
no notation, unexpander or delaborator from the model (or from `Init`'s
extensions) is used: applications print in prefix form with every argument
visible (`@LE.le.{0} Nat instLENat x y`). Only Lean's built-in delaborator
runs; no model code. -/
def ppStmt (env : Environment) (e : Expr) : IO String := do
  try
    let f ← PrettyPrinter.ppExprLegacy env {} {} ppOptions e
    return f.pretty 100000
  catch err =>
    return s!"<pretty-printer error: {err}>"

/-- The STMT and STMTPP report lines for `thm`. -/
def statementLines (env : Environment) (root : Name) (label : String) (thm : Name) :
    IO (Array String) := do
  let closure := statementClosure env root thm
  let ser := closure.foldl (init := "") serConst
  let mut lines := #[s!"STMT\t{label}\t{ser}"]
  for ci in closure do
    -- Recursors are generated by the kernel from their inductive (and
    -- leanchecker regenerates and compares them), so only the hash covers
    -- them; their types are long and add nothing for the reviewer.
    let ty ← if ci matches .recInfo _ then pure "" else ppStmt env ci.type
    let val ← match ci with
      | .defnInfo v => ppStmt env v.value
      | _ => pure ""
    lines := lines.push
      s!"STMTPP\t{label}\t{kindWord ci}\t{escField (toString ci.name)}\t{escField ty}\t{escField val}"
  return lines

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
      lines := lines ++ (← statementLines env rootName n name)
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
