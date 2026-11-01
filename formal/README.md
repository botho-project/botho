# formal/ - Lean 4 models and proofs

This directory holds Lean 4 models of selected Botho mechanisms, proofs about
those models, and the gates that keep the proofs honest. It is the scaffold for
epic #1704, set up by #1705. It currently contains one toy model
(`Botho/Basic.lean`) that mirrors no Rust code.

The approach follows Cedar's verification-guided development: write a small
executable Lean model of a Rust component, prove properties about the model,
and use differential random testing (DRT) to check that the Rust code and the
model agree.

## The model, prove and DRT loop

1. **Model.** Write an executable Lean definition (`def`, not `axiom`) that
   mirrors a Rust function or state transition, in `Botho/<Area>.lean`. Keep it
   as close to the Rust structure as the proofs allow. Cite the Rust file and
   function it mirrors in the module docstring.
2. **Prove.** State and prove properties of the model (conservation, bounds,
   determinism, monotonicity) as `theorem`s in the same module. Add each
   exported theorem to `Theorems.lean` (see below).
3. **DRT.** Compile the model to an executable and run it against the Rust
   implementation on random and boundary inputs, comparing outputs. A proof
   only says something about Rust if DRT shows the model and the code agree.
   The DRT harness lands with the first real model (a child issue of #1704).
4. **Mirror.** Add the mirrored Rust files to the `pattern` in
   `.github/workflows/formal.yml` so that edits to them re-run the Lean job.

## Labeling rule: proven / tested / trusted

Every claim made about a Rust component carries exactly one of these labels in
its module docstring, and in any document that cites it:

| Label | Meaning |
|-------|---------|
| **proven** | A theorem about the Lean model, listed in `Theorems.lean`, whose axioms are only `propext`, `Classical.choice` and `Quot.sound`. Says nothing about Rust unless DRT also passes. |
| **tested** | The Rust code and the Lean model agree under DRT. This is evidence, not proof, that a proven property holds for the Rust code. |
| **trusted** | Depends on an allow-listed opaque axiom (for example a `sha256` or `randomx` stand-in), or on an assumption about code outside the model. Name the axiom or assumption next to the claim. |

Never write "verified" on its own. Write "proven (model)", "proven + tested",
or "trusted (axiom `Botho.sha256`)".

## Building locally

Install elan, the Lean toolchain manager:

```bash
curl -sSfL https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh | sh -s -- -y --default-toolchain none
# or: brew install elan-init
```

Then build. elan reads `lean-toolchain` and installs the pinned version
automatically:

```bash
cd formal
lake build                      # type-checks every module under Botho/
scripts/check-sorry.sh          # sorry gate (scans, then builds)
scripts/check-axioms.sh         # axiom gate
scripts/selftest-gates.sh       # proves both gates reject injected defects
scripts/selftest-verify.sh      # proves the verify job rejects the bypass fixtures
```

To run the verify job's checks on your own build, stage the `.olean` files
the way CI uploads them and point the verifier at them:

```bash
rsync -am --include='*/' --include='*.olean' --exclude='*' \
  .lake/build/lib/lean/ /tmp/formal-lib/
scripts/verify-artifacts.sh --lib /tmp/formal-lib --sources .
```

`verify-artifacts.sh --sysroot DIR` and `selftest-verify.sh`'s `LEAN_SYSROOT`
name the toolchain directory when `lean --print-prefix` does not find it.
`verify-artifacts.sh` passes that directory to every Lean process as
`LEAN_SYSROOT`, so `Init` always comes from it, never from a `lean` on `PATH`;
with `--sysroot` it needs no `lean` on `PATH` at all. It and the gates need a
`python3` with `tomllib` (3.11 or later).

The scripts take an optional `LAKE=/path/to/lake` environment variable if the
`lake` on your `PATH` is not the elan proxy.

The toolchain is pinned to an exact stable release in `lean-toolchain`. Bump it
deliberately in its own PR. Never use `stable` or `nightly`. The project has no
dependencies (no Mathlib). Whether to add Mathlib is decided in #1708, because
it adds substantial CI time.

## Gates

CI (`.github/workflows/formal.yml`) runs two jobs on opposite sides of a trust
boundary (#1799; see [Limits](#limits)):

- **build** (untrusted) checks out the pull request, runs the gates below
  (which run `lake build`) and the self-tests, and uploads the compiled
  `.olean` files as the artifact `formal-lib`. Its results are fast feedback
  only: model code runs during the build and could fake them.
- **verify** (trusted) runs `scripts/verify-artifacts.sh` from the **base**
  commit over that artifact. See [The verify job](#the-verify-job).

The required check is `formal-gate`. It passes when both jobs succeeded, or
when both were skipped because nothing under `formal/`, the workflow, or a
mirrored Rust file changed.

The in-tree gates (#1770). The build job runs them in full; the verify job
runs the base commit's `check-sorry.sh --no-build`, which is the layout check
and the source scan below, over the pull request's sources as data:

- **Build configuration.** Before anything else, both gates check that
  there is no `lakefile.lean` (a Lean program that runs on every `lake`
  call) and that `lakefile.toml` passes the allow-list in
  `scripts/gate-common.sh`:
  - *Layout:* no package `srcDir`, one `[[lean_lib]]` named `Botho` with no
    `srcDir`, `roots`, `globs` or `root`, and no `[[lean_exe]]`. These are
    the keys the verify job's layout check compares with the base's
    `lakefile.toml`. So Lake compiles only `Botho.lean` and the modules it
    imports, all in `Botho/`.
  - *Keys:* every other key must be on an allow-list of keys that cannot
    load or run code: package metadata (`name`, `version`, `defaultTargets`,
    `description`, `keywords`, `homepage`, `license`) and `leanOptions`
    with plain values for a short list of options (`autoImplicit`,
    `relaxedAutoImplicit`, `maxHeartbeats`, `maxRecDepth`, the
    `synthInstance` limits, `linter.*`, `pp.*`). A `[[lean_lib]]` may carry
    only its `name`. Everything else is rejected, known or unknown:
    `[[require]]`, `moreLeanArgs`, `weakLeanArgs`, `moreServerOptions`,
    `moreLinkArgs`, `plugins`, `dynlibs`, `precompileModules`,
    `extraDepTargets`, `buildDir`, `testDriver`, and options such as
    `debug.skipKernelTC`.
  - *No dependencies:* `lake-manifest.json`, if present, must list no
    packages.

  `lean-toolchain` is not pinned by the gates, so toolchain bumps pass; see
  [The verify job](#the-verify-job).
- **Source scan** (`scripts/gate_scan.py`). Both gates run it before their
  first `lake` call, and stop if it fails, so the in-tree gates never build a
  tree the scan rejected. It checks:
  - *Layout.* The model is `Botho.lean` and `Botho/**/*.lean`, the manifest
    is `Theorems.lean`, all regular files (no symlinks under `Botho/`), and
    there is no other `*.lean` file outside `.lake/` and `scripts/`.
  - *Import allow-list.* Every scanned file may import only `Botho` modules
    that are scanned files (`import Botho.X.Y` must be `Botho/X/Y.lean`). Not
    `Lean`, `Std`, `Lake`, `Init.*`, Mathlib or anything else. A model that
    needs another import must extend the list in `gate_scan.py` in a reviewed
    diff. `prelude`, `module`, import modifiers (`public`, `meta`,
    `import all`) and an `import` after the header are rejected. Multi-line
    headers, comments between imports and `«»`-quoted names are parsed the
    way Lean parses them.
  - *Tokens*, as listed under the sorry gate below.
- **Sorry gate** (`scripts/check-sorry.sh`) fails if `lake build` warns that a
  declaration uses `sorry`, or if a sweep of the compiled library finds any
  declaration that reaches `sorryAx`. The sweep (`scripts/AxiomReport.lean`)
  walks declaration bodies directly, so it still works when the warning is
  hidden (for example by `#guard_msgs`). The gate also fails if the source
  scan finds `sorry`, `sorryAx`, `admit` or `native_decide` (bare,
  `_root_.`-qualified, or with explicit universes such as `sorryAx.{0}`)
  outside comments, string literals, raw strings (`r#"..."#`) and char
  literals. Code inside `{...}` string interpolation is scanned.
  `native_decide` trusts the compiler, so it is allowed only on a line marked
  `-- formal-gate: allow native_decide`, and the compiler-trust axioms it
  introduces (such as `Lean.ofReduceBool`) must also be on the allow-list.
  The scan also rejects commands that run code or hide messages at compile
  time (`#eval`, `#guard`, `#guard_expr`, `run_cmd`, `run_elab`, `run_meta`,
  `run_tac`, `by_elab`, `#guard_msgs`, `initialize`, and `#print` outside
  `Theorems.lean`), the evaluators that run compiled code while the model is
  elaborated (`native` in any form, such as `decide +native` or
  `native := true`, and `reduceBool`, `reduceNat`, `ofReduceBool`,
  `ofReduceNat`, `trustCompiler`), the prelude IO surface (`IO`, `EIO`,
  `BaseIO`, `ST`, `EST`, `EStateM`, `RealWorld`, `Void`, `System`,
  `FilePath` as a name or any dotted component, so `IO.FS`, `IO.Process`,
  `IO.Ref` and `ST.Ref` too; the `unsafe` keyword; and any name starting
  with `unsafe` or ending in `Unsafe`, such as `unsafeCast` or
  `unsafeBaseIO`), metaprogramming commands that can expand to
  them (`macro`, `macro_rules`, `syntax`, `elab`, `elab_rules`,
  `declare_syntax_cat`), simprocs (`simproc`, `dsimproc`), and the option
  `debug.skipKernelTC`. Inside `@[...]` and `attribute [...]` it rejects
  attributes that run code at build or import time, bind native or
  substitute code, or extend the elaborator: `init`, `builtin_*`, `extern`,
  `implemented_by`, `export`, `macro`, `*_elab`, `*_parser`, `tactic`,
  `delab` and similar.
- **Axiom gate** (`scripts/check-axioms.sh`) runs the layout check and the
  source scan, then checks every theorem listed in `Theorems.lean` in four
  layers:
  1. `Theorems.lean` must be a plain manifest: comments, `import` lines, then
     one `#print axioms NAME` per line, no duplicates, nothing else.
  2. `leanchecker` replays every declaration of the `Botho` library in the
     kernel, so a declaration admitted with kernel checking off fails.
  3. `lean --json Theorems.lean` must print only Lean messages and no errors.
     Exactly one result is accepted per manifest command, matched by source
     position and theorem name, so text printed elsewhere (by `#eval` or an
     imported initializer) cannot stand in for a result.
  4. `scripts/AxiomReport.lean` imports the compiled library without
     elaborating its syntax and computes each theorem's axioms itself. These
     must equal Lean's `#print axioms` results, which catches a model that
     redefines `#print axioms`. The independent result is checked against
     `axioms-allowlist.txt` and must not contain `sorryAx`. Like the sorry
     gate, it also fails if any library declaration reaches `sorryAx`.
- Both gates read their inputs (`axioms-allowlist.txt`, `Theorems.lean`,
  `scripts/AxiomReport.lean`) before the first `lake` call and use only those
  copies afterwards, and the axiom gate runs the independent walk before it
  compiles `Theorems.lean`. This keeps local runs honest against cruder
  tricks; it is not what the verdict rests on (see [Limits](#limits)).
- **Self-test** (`scripts/selftest-gates.sh`) copies the tree, injects a
  `sorry`, an `admit`, a `native_decide`, a bogus axiom, a `sorryAx` and an
  empty manifest, and asserts that the matching gate fails each time. It also
  checks that the clean tree passes and that the tokens are ignored inside
  comments, strings, raw strings and char literals. Since #1724 it also covers
  `sorry` hidden after a `'"'` char literal or an `r#"a"b"#` raw string, a
  `_root_.sorryAx` term, and primed theorem names in the axiom gate. Since
  #1770 it covers each known bypass of the in-tree gates, and checks that the
  gate fails for the intended reason: a fake `#print axioms` line printed by
  `#eval` or an imported initializer, `#print axioms` redefined by
  `macro_rules`, a theorem admitted with `debug.skipKernelTC`, a `sorry`
  hidden by `#guard_msgs`, `sorryAx.{0}`, `s!"{sorry}"`, duplicate manifest
  lines, a `lakefile.lean`, an `@[init]` initializer that rewrites
  `axioms-allowlist.txt` during the build, one that erases its own `@[init]`
  line, each forbidden attribute and metaprogramming command, a simproc that
  overwrites a compiled `.olean`, a bare `import Lean`, each import-header
  form, symlinked or stray sources, and layout changes (`srcDir`, an extra
  `lean_lib`, a `lean_exe`, `roots`, `globs`). Since the #1783 review it
  also covers a `[[require]]` dependency whose `lakefile.lean` rewrites a
  model source (its code must never run), a manifest that lists a package,
  each disallowed lakefile key and option, and (#1810) the `decide +native`
  split, `#guard` and prelude `#eval` `.olean` swaps, with `--no-scan`
  controls that show each swap fools the later layers, plus each banned IO
  name and evaluator. Passing controls cover a second module with quoted
  multi-line imports, a commented lakefile, the verify job's benign lakefile
  edit, allow-listed metadata and `leanOptions` in both TOML spellings, a
  tree without `lake-manifest.json`, names that merely contain a banned
  name, and the verify job's `--no-build` call with `LAKE=false`.
- **Workflow self-test** (`scripts/selftest-workflow.sh`, #1804) takes the
  `run:` block of the verify job's verify step from
  `.github/workflows/formal.yml` and runs it with stub verifiers. It checks
  that the step fails, and runs no verifier, when the base commit has no
  `verify-artifacts.sh` (even if the pull request has one), and that
  otherwise it runs the base copy with the base allow-list and lakefile.
  It needs no Lean toolchain.
- **Verify self-test** (`scripts/selftest-verify.sh`) runs the verify job's
  checks against the fixtures in `scripts/fixtures/verify/`: the bypasses
  found in the reviews of PR #1783 (an `@[init]` that widens the allow-list,
  a simproc that swaps an `.olean`, a `srcDir` move with an initializer, and
  `decide +native` with an IO world-token cast), a pull request that edits
  its own allow-list, a module with an initializer, and an artifact that
  carries an `Init` module, a `srcDir` decoy that compiles a weakened copy
  of the model while the scanned `Botho/` stays unchanged, and (accepted) a
  benign lakefile edit. Each fixture is built the way the build job
  builds it, and the verifier must reject it with the clean allow-list, both
  with and without the source scan. The verifier reads a copy of the
  sources taken before the build, as the verify job reads its own checkout.
  Four `scan-only` fixtures leave an artifact that is an honest build of a
  different source, so only the base's scan or lakefile check can stop
  them: a `[[require]]` dependency whose `lakefile.lean` rewrites
  `Botho/Cheat.lean` before it is compiled, and (#1810) `decide +native`
  split across modules, `#guard`, and a prelude `#eval`, each overwriting
  compiled `.olean` files. For these the self-test shows both halves: the
  full verifier rejects them, and the checks without the scan accept the
  forged `one_eq_two`. The self-test also checks that no
  fixture initializer runs inside the verifier, and, as a control, that the
  same initializer does run when the library is imported through the Lean
  frontend. The clean tree is also verified with no `lean`, `lake` or `elan`
  on `PATH`, as in the verify job.

### The verify job

`scripts/verify-artifacts.sh` is the authority for the verdict. The verify job
checks out `formal/scripts/` (this script, `AxiomReport.lean`,
`check-sorry.sh`), `axioms-allowlist.txt` and `lakefile.toml` from the base
commit of the pull request (on a push, the pushed commit), never from the
pull request head. A pull request that edits these files changes nothing the
verify job reads until it is merged. It then:

1. checks that the pull request's `lakefile.toml` compiles the same sources
   as the base's. Both are parsed with Python's `tomllib`, and the
   package-level `srcDir` and, for every `[[lean_lib]]` and `[[lean_exe]]`,
   `name`, `srcDir`, `roots`, `globs` and `root` must be identical (so the
   set of libraries and executables is too). Which other keys may change is
   decided in step 2 by the base's key allow-list (no `[[require]]`, no
   code-loading keys; see Build configuration above). A `lakefile.lean` on either
   side, or a `lakefile.toml` that does not parse, is rejected. Without this
   a pull request could leave the scanned `Botho/` as a decoy and compile a
   different copy (`srcDir = "src"`) whose theorems state something else;
2. runs the base commit's source scan (`check-sorry.sh --no-build`: the
   build-layout check and `gate_scan.py` described above) over the pull
   request's sources, read as data;
3. takes in the artifact: only regular files named `Botho.olean` or
   `Botho/**/*.olean` (and the `.olean.server`, `.olean.private` and `.ir`
   parts), copied to a private directory that is the only entry on
   `LEAN_PATH`, so the artifact cannot shadow a toolchain module;
4. replays every `Botho` declaration in the kernel with `leanchecker`;
5. walks the compiled library with `AxiomReport.lean`, and fails if a
   theorem listed in `Theorems.lean` (parsed as text, never compiled) is
   missing or depends on an axiom outside the base allow-list or on
   `sorryAx`; if any library declaration reaches `sorryAx`; if a library
   module declares an axiom outside the base allow-list, imports anything
   but `Init` and `Botho` modules, or declares an `[init]` initializer; or if
   the compiled modules differ from the source modules.

It never runs `lake`, never compiles a model source, installs the toolchain
fresh (an official `leanprover/lean4` release only) and uses no cache.

## Limits

**What the trust boundary guarantees.** No model code runs in the verify job,
so model code cannot change the verify job's inputs (the gate scripts, the
allow-list, the report program) or its output. This closes the bypasses
found in the reviews of #1783: each one ran model code during `lake build` to
rewrite something the gate read in the same job.

**Why no model code runs there.** `leanchecker` and `AxiomReport.lean` load the
`.olean` files with `importModules (loadExts := false)`. Lean runs `[init]`
and `initialize` declarations only from `finalizePersistentExtensions`, which
only `loadExts := true` calls (`finalizeImport` in `Lean/Environment.lean`,
v4.35.0). The Lean frontend does load extensions: elaborating any file that
imports the model (`import Botho`, `lean Theorems.lean`) runs the model's
initializers. So the verify job never passes a model file, or a file that
imports the model, to the frontend, and it does not compile `Theorems.lean`
(the build job's axiom gate does; the verify job parses it as text).
`AxiomReport.lean` itself is elaborated, but it imports only `Lean` and loads
the model at run time. `selftest-verify.sh` checks this with a canary
initializer and the frontend control described above.

**What it does not guarantee: the artifact matches the sources.** The build
job is untrusted, so the `.olean` files it uploads are whatever model code
left on disk. The verify job proves that the uploaded library is
kernel-valid and that the listed theorems, as compiled, depend only on
allowed axioms. It does not prove that those theorems state what the
sources say. The `simproc-swap` and `native-world-swap` fixtures replace
`one_eq_two : 1 = 2` with `one_eq_two : True` in the compiled library, and
the later checks reject them only because of other evidence they leave
behind (an `import Lean`, a declared axiom). A build-time attack that also
rewrites those modules leaves a library that passes every artifact check:
the `native-split-swap`, `guard-swap`, `eval-swap` and `require-dep`
fixtures do exactly that. Two things stand in the way of such an attack:

- the base commit's source scan, which the pull request cannot weaken, has to
  miss every way the attack runs code during the build; and
- review: the sources of such a pull request contain both the false proof
  and the code that runs during the build.

Statement substitution through build configuration alone, with no code run
during the build, is closed by step 1: Lake compiles only the sources the
base's layout names, and those are the sources the scan reads and the
intake's module-set check compares (the `lakefile-decoy` fixture; it passes
every other check). That check covers the layout keys of Lake v4.35.0's
TOML configuration. Build configuration that loads or runs code is closed
by the base's key allow-list in step 2. A `[[require]]` dependency's
`lakefile.lean` is a Lean program that Lake elaborates, `#eval` included, on
every `lake` call, and its source is never scanned, so it could rewrite a
model source before Lake compiles it and leave an artifact that is an
honest build of the rewritten source (the `require-dep` fixture; no check
on the artifact can tell). The allow-list rejects `[[require]]`, a manifest
that lists a package, `moreLeanArgs`, `weakLeanArgs`, `plugins`, `dynlibs`,
`precompileModules`, `extraDepTargets`, any unknown key and any
`leanOptions` entry outside a short list. Binding each theorem's compiled statement
to a reviewed record is tracked in #1805.

Binding the artifact to the sources needs a build that runs no model code,
which Lean does not offer (macros, `decide +native` and initializers all run
user code during elaboration), or a rebuild under a sandbox that confines
each module's elaboration to its own output.

**The in-tree gates.** In the build job, model code runs during
`lake build`, and from then on nothing that reads `.lake/` can be trusted:
build-time code can replace a module's `.olean` with a forged one, and then
`leanchecker`, the walk and `#print axioms` in that job all check the forged
output. The in-tree gates' snapshots, kernel replay and walk only defend
against cruder tricks. Their source scan is what carries over to the verify
job: there it runs from the base commit and decides, before any of the
artifact checks, whether the sources may run code at build time at all.
Without `import Lean` a model cannot define a simproc, elaborator, tactic or
`deriving` handler, so what is left is the prelude. The scan covers it in
two layers (#1810). First, it bans the evaluators that run compiled code
while a module is elaborated: `#eval`, `#guard` (which evaluates its term
with the compiler and needs no `native` token), `native` in every spelling
(`native_decide`, `decide +native`, `native := true`), `Lean.reduceBool`
and its relatives, and initializers. Second, it bans the prelude IO surface
(`IO`, `EIO`, `BaseIO`, `ST`, `EST`, `EStateM`, `RealWorld`, `Void`,
`System`, `FilePath`, `unsafe*`), so an evaluator missing from the first
list has no named effect left to run: the world-token trick (`cast` through
`axiom : Void Unit = Void IO.RealWorld`) and every file or process call are
spelled with these names. Both layers are denylists, and a denylist can
miss routes. A pure model needs none of these names, so the lists can stay
broad. Hence the review rule below.

**Other limits.**

- A human must review every change to the model (`Botho.lean`, `Botho/`,
  `Theorems.lean`, `lakefile.toml`, `lean-toolchain`, `lake-manifest.json`,
  `axioms-allowlist.txt`) for IO and metaprogramming, with the same care as
  a change to `formal/scripts/`. A green `formal-gate` does not replace that
  review.

- The `.olean` format is a memory image that Lean maps without validating
  it. A hand-crafted `.olean` that corrupts the memory of `leanchecker` or of
  `AxiomReport.lean` is out of scope. The verify job holds no secrets and a
  read-only token.
- The workflow file comes from the pull request, as it does for every
  `pull_request` workflow. A pull request can edit `formal.yml` itself,
  including the verify job. Such an edit, and any edit under
  `formal/scripts/`, is visible in review and needs a human reviewer.
  `.github/CODEOWNERS` (#1804) names a code owner for `formal/scripts/`,
  `formal/axioms-allowlist.txt`, `formal/lakefile.toml`,
  `formal/lean-toolchain`, `.github/workflows/formal.yml` and
  `.github/CODEOWNERS` itself.
- **Two branch-protection settings are not in place.** They need a
  repository admin, and no pull request can set them. Until an admin turns
  them on, CODEOWNERS only requests a review and a red `formal-gate` does not
  block a merge:
  - *Require review from Code Owners* on `main`
    (`require_code_owner_reviews`, today `false`);
  - *Require status checks to pass*, with `formal-gate` as a required check
    (`main` has no required checks today).

  A ruleset that pins `formal.yml` to `main` ("Require workflows to pass
  before merging") would take the workflow out of the pull request's hands.
  That rule needs GitHub Enterprise Cloud; the `botho-project` organization
  is on the Free plan, so it is not available.
- The verify job runs only the base commit's `verify-artifacts.sh`. If the
  base commit has none, the job fails; it never falls back to the pull
  request's copy (#1804). `scripts/selftest-workflow.sh` runs the job's
  script against stub verifiers to check this.
- The pull request picks the toolchain version, so a toolchain bump is
  verified with the bumped toolchain. Only official `leanprover/lean4`
  releases are accepted.
- That includes old ones: a pull request can pin an older official release
  in `lean-toolchain`, and the verify job checks the artifact with that
  older `leanchecker`. The checks do not reject a downgrade; `lean-toolchain`
  is under CODEOWNERS, so a code owner reviews it.
- `leanchecker` trusts the toolchain's own library (`Init`). The artifact
  cannot replace it (step 3 above), and it is loaded from `--sysroot` through
  `LEAN_SYSROOT`, never from a `lean` found on `PATH`.
- The artifact download relies on `actions/download-artifact` (v4.1.3 and
  later) to refuse archive entries outside the target directory (zip-slip).
  Intake then accepts only regular files with library names.

## Adding a theorem

1. Prove it in a module under `Botho/` that `Botho.lean` imports. Do not
   declare anything in `Theorems.lean`; the axiom gate rejects it.
2. Add one line to `Theorems.lean`:

   ```lean
   #print axioms Botho.my_theorem
   ```

   Group lines under a comment naming the module and its label (proven /
   tested / trusted).
3. Run `scripts/check-axioms.sh`. If it reports a non-allow-listed axiom,
   either remove the dependency or follow the next section.

## Adding an allow-listed opaque axiom

Some primitives (hashes, RandomX) cannot be modeled usefully. They enter as
opaque `axiom` declarations, for example
`axiom Botho.sha256 : ByteArray → ByteArray`, and theorems may depend on them.
To add one:

1. Declare the axiom in the model module with a docstring that says what it
   stands for and which Rust function it abstracts.
2. Add its fully-qualified name to `axioms-allowlist.txt` with a one-line
   justification comment on the line above it:

   ```text
   # SHA-256 as used by botho/src/block.rs; modeled as an opaque function.
   Botho.sha256
   ```

3. Label every theorem that depends on it **trusted** and name the axiom.

The verify job reads the allow-list from the base commit, so a new axiom
fails the verify job until its allow-list entry is on `main`. Land the
allow-list line (with the axiom's justification) in its own pull request
first, then the model that declares and uses it.

Only opaque functions belong on the allow-list. Never add an axiom that states
a property (for example "sha256 is injective") without a design discussion on
the relevant issue. Such an axiom can make the model inconsistent.
